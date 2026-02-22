use super::*;

#[cfg(feature = "p2p")]
use crate::network::{PeerHandshake, SyncRequest, SyncResponse, SnapshotData};

/// Try to fetch a snapshot from configured P2P bootstrap peers before creating genesis
/// This ensures non-genesis nodes sync from the network instead of creating their own chain
/// Uses exponential backoff retry for robustness - genesis node may still be starting
#[cfg(feature = "p2p")]
pub(crate) async fn try_presync_from_peers(bootstrap_peers: &[String], is_genesis_node: bool) -> Option<SyncSnapshot> {
    use std::time::Duration;
    
    if bootstrap_peers.is_empty() {
        if is_genesis_node {
            info!("No bootstrap peers configured, will create genesis locally (IS_GENESIS_NODE=true)");
        } else {
            warn!("No bootstrap peers configured but not marked as genesis node!");
        }
        return None;
    }
    
    // Retry configuration - validators should wait for genesis to be ready
    let max_retries = if is_genesis_node { 1 } else { 8 };
    let base_delay_secs = 5;
    
    for attempt in 1..=max_retries {
        info!("PRE-SYNC: Attempt {}/{} - syncing from {} bootstrap peer(s)...", 
              attempt, max_retries, bootstrap_peers.len());
        
        if let Some(snapshot) = try_presync_attempt(bootstrap_peers).await {
            return Some(snapshot);
        }
        
        if attempt < max_retries {
            let delay = base_delay_secs * (1 << (attempt - 1).min(4)); // Cap at 80s
            info!("PRE-SYNC: Attempt {} failed, retrying in {}s...", attempt, delay);
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
    }
    
    if !is_genesis_node {
        warn!("PRE-SYNC: All {} attempts failed! Validator node cannot create genesis.", max_retries);
    } else {
        info!("PRE-SYNC: All bootstrap peers failed, will create genesis locally");
    }
    None
}

/// Single attempt to sync from bootstrap peers
#[cfg(feature = "p2p")]
async fn try_presync_attempt(bootstrap_peers: &[String]) -> Option<SyncSnapshot> {
    use libp2p::{
        identity::Keypair,
        noise, tcp, yamux,
        request_response::{self, ProtocolSupport},
        swarm::SwarmEvent,
        Multiaddr, PeerId, StreamProtocol,
    };
    use libp2p::futures::StreamExt;
    use std::{env, time::Duration};
    use crate::cbor_codec::CborCodec;
    
    // Parse bootstrap peers - peer ID is now OPTIONAL (allows connecting without knowing peer ID)
    let mut targets: Vec<(Option<PeerId>, Multiaddr)> = Vec::new();
    for peer_str in bootstrap_peers {
        match peer_str.parse::<Multiaddr>() {
            Ok(addr) => {
                // Extract peer ID from multiaddr if present (last component /p2p/<peer_id>)
                let peer_id = addr.iter().find_map(|p| {
                    if let libp2p::multiaddr::Protocol::P2p(peer_id) = p {
                        Some(peer_id)
                    } else {
                        None
                    }
                });
                
                // Remove /p2p/ component from address for dialing
                let dial_addr: Multiaddr = addr.iter()
                    .filter(|p| !matches!(p, libp2p::multiaddr::Protocol::P2p(_)))
                    .collect();
                
                if let Some(ref pid) = peer_id {
                    info!("PRE-SYNC: Parsed peer {} at {}", pid, dial_addr);
                } else {
                    info!("PRE-SYNC: Parsed address {} (peer ID will be discovered)", dial_addr);
                }
                targets.push((peer_id, dial_addr));
            }
            Err(e) => {
                warn!("PRE-SYNC: Failed to parse multiaddr '{}': {}", peer_str, e);
            }
        }
    }
    
    if targets.is_empty() {
        warn!("PRE-SYNC: No valid bootstrap peers parsed from multiaddrs");
        return None;
    }
    
    // Create a temporary keypair for this sync connection
    let local_key = Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    info!("PRE-SYNC: Temporary peer ID: {}", local_peer_id);
    
    // Build minimal swarm for sync only
    // Use our custom CborCodec with 16MB limits to match the main node's protocol
    let cbor_codec: CborCodec<SyncRequest, SyncResponse> = CborCodec::new(
        16 * 1024 * 1024,  // 16 MB max request
        16 * 1024 * 1024,  // 16 MB max response
    );
    let swarm_result = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .map(|builder| {
            builder.with_behaviour(|_| {
                request_response::Behaviour::with_codec(
                    cbor_codec,
                    [(StreamProtocol::new("/rinku/sync/1.0.0"), ProtocolSupport::Full)],
                    request_response::Config::default()
                        .with_request_timeout(Duration::from_secs(30)),
                )
            })
        });
    
    let mut swarm = match swarm_result {
        Ok(Ok(builder)) => builder.with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60))).build(),
        _ => {
            warn!("PRE-SYNC: Failed to build P2P swarm");
            return None;
        }
    };
    
    // Try each bootstrap peer
    for (expected_peer_id, dial_addr) in targets {
        if let Some(ref pid) = expected_peer_id {
            info!("PRE-SYNC: Dialing peer {} at {}...", pid, dial_addr);
        } else {
            info!("PRE-SYNC: Dialing {} (discovering peer ID)...", dial_addr);
        }
        
        // Dial the peer - for unknown peer IDs, just dial the address
        if let Err(e) = swarm.dial(dial_addr.clone()) {
            warn!("PRE-SYNC: Failed to dial {}: {}", dial_addr, e);
            continue;
        }
        
        // Wait for connection and send request
        let timeout = tokio::time::sleep(Duration::from_secs(15));
        tokio::pin!(timeout);
        
    let mut connected_peer_id: Option<PeerId> = None;
    let mut handshake_sent = false;
    let mut handshake_complete = false;
    let mut snapshot_requested = false;
        
        loop {
            tokio::select! {
                _ = &mut timeout => {
                    if let Some(ref pid) = expected_peer_id {
                        warn!("PRE-SYNC: Timeout waiting for peer {}", pid);
                    } else {
                        warn!("PRE-SYNC: Timeout waiting for connection to {}", dial_addr);
                    }
                    break;
                }
                event = swarm.select_next_some() => {
                    match event {
                        SwarmEvent::ConnectionEstablished { peer_id: actual_peer, .. } => {
                            // Accept connection if: no expected peer ID, OR it matches expected
                            let should_accept = expected_peer_id.is_none() || expected_peer_id == Some(actual_peer);
                            
                            if should_accept {
                                info!("PRE-SYNC: Connected to {} (discovered)", actual_peer);
                                connected_peer_id = Some(actual_peer);
                                // Send handshake first to satisfy mainnet-mode requirements
                                let chain_id = env::var("CHAIN_ID").unwrap_or_else(|_| "rinku-mainnet".to_string());
                                let network_id = env::var("NETWORK_ID").unwrap_or_else(|_| "mainnet".to_string());
                                let handshake = PeerHandshake {
                                    protocol_version: "1.0.0".to_string(),
                                    chain_id,
                                    network_id,
                                    node_id: local_peer_id.to_string(),
                                    checkpoint_height: 0,
                                    validator_address: None,
                                    capabilities: vec!["sync".to_string()],
                                };
                                swarm.behaviour_mut().send_request(&actual_peer, SyncRequest::Handshake(handshake));
                                handshake_sent = true;
                                info!("PRE-SYNC: Sent handshake to {}", actual_peer);
                            } else if let Some(ref expected) = expected_peer_id {
                                warn!("PRE-SYNC: Connected to unexpected peer {} (expected {})", actual_peer, expected);
                            }
                        }
                        SwarmEvent::Behaviour(request_response::Event::Message {
                            message: request_response::Message::Response { response, .. },
                            ..
                        }) => {
                            match response {
                                SyncResponse::Handshake(_) => {
                                    handshake_complete = true;
                                    if let Some(actual_peer) = connected_peer_id {
                                        if !snapshot_requested {
                                            swarm.behaviour_mut().send_request(&actual_peer, SyncRequest::Snapshot);
                                            snapshot_requested = true;
                                            info!("PRE-SYNC: Handshake OK, sent snapshot request to {}", actual_peer);
                                        }
                                    }
                                }
                                SyncResponse::Snapshot(snapshot_data) => {
                                    info!(
                                        "PRE-SYNC: Received snapshot: {} accounts, {} checkpoints, {} txs",
                                        snapshot_data.accounts.len(),
                                        snapshot_data.checkpoints.len(),
                                        snapshot_data.recent_txs.len()
                                    );
                                    
                                    // Convert SnapshotData to SyncSnapshot
                                    let sync_snapshot = convert_snapshot_data_to_sync_snapshot(snapshot_data);
                                    return Some(sync_snapshot);
                                }
                                SyncResponse::Error { message } => {
                                    warn!("PRE-SYNC: Peer returned error response: {}", message);
                                }
                                _ => {
                                    if handshake_sent && !handshake_complete {
                                        warn!("PRE-SYNC: Unexpected response before handshake completed");
                                    } else {
                                        warn!("PRE-SYNC: Unexpected response type from peer");
                                    }
                                }
                            }
                        }
                        SwarmEvent::OutgoingConnectionError { peer_id: failed_for, error, .. } => {
                            warn!("PRE-SYNC: Connection failed to {}: {}", dial_addr, error);
                            // Check if this failure is for our expected peer or the address we dialed
                            if failed_for == expected_peer_id || expected_peer_id.is_none() {
                                break;
                            }
                        }
                        SwarmEvent::ConnectionClosed { peer_id: closed_peer, .. } => {
                            if Some(closed_peer) == connected_peer_id && (handshake_sent || snapshot_requested) {
                                warn!("PRE-SYNC: Connection closed before response from {}", closed_peer);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    
    // Don't log here - let the caller (try_presync_from_peers) handle it
    // since only the caller knows if this is a genesis node or validator
    None
}

/// Convert P2P SnapshotData to SyncSnapshot format
#[cfg(feature = "p2p")]
fn convert_snapshot_data_to_sync_snapshot(data: SnapshotData) -> SyncSnapshot {
    use rinku_core::types::{Account, Validator, Checkpoint, SignedTransaction, Transaction};
    
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    
    // Extract genesis hash from first checkpoint if available
    let genesis_hash = data.checkpoints.first().and_then(|c| c.genesis_hash.clone());
    
    // Convert accounts
    let accounts: HashMap<String, Account> = data.accounts.into_iter().map(|a| {
        (a.address.clone(), Account {
            address: a.address,
            balance: a.balance,
            nonce: a.nonce,
            first_seen: now_secs,
            staked: a.stake,
            unbonding: 0.0,
            unbonding_release: None,
            latest_balance_proof: None,
        })
    }).collect();
    
    // Convert validators
    let validators: HashMap<String, Validator> = data.validators.into_iter().map(|v| {
        (v.address.clone(), Validator {
            address: v.address,
            stake: v.stake,
            first_stake_time: now_secs * 1000,
            bls_public_key: Some(v.bls_public_key),
            missed_checkpoints: 0,
        })
    }).collect();
    
    // Convert checkpoints
    let checkpoints: Vec<Checkpoint> = data.checkpoints.iter().enumerate().map(|(i, c)| {
        let prev_hash = if i > 0 {
            data.checkpoints.get(i - 1).and_then(|p| p.hash.clone())
        } else {
            None
        };
        Checkpoint {
            height: c.height,
            hash: c.hash.clone().unwrap_or_else(|| rinku_core::sha256_hex(&format!("cp:{}", c.height))),
            previous_hash: prev_hash,
            tx_merkle_root: c.merkle_root.clone(),
            state_root: c.merkle_root.clone(),
            receipt_root: String::new(),
            tip_count: 0,
            timestamp: c.timestamp,
            validator_signatures: Vec::new(),
            aggregated_signature: c.signature.clone(),
            signer_bitmap: None,
            finalized_tx_hashes: Vec::new(),
            weight_trie_root: String::new(),
        }
    }).collect();
    
    // Convert transactions
    let dag_transactions: Vec<SignedTransaction> = data.recent_txs.into_iter().map(|t| {
        SignedTransaction {
            tx: Transaction {
                from: t.from,
                to: t.to,
                amount: t.amount,
                nonce: t.nonce,
                timestamp: t.timestamp,
                parents: t.parents,
                kind: None,
                gas_limit: None,
                gas_price: Some(t.gas_price),
                data: None,
                signature: Some(t.signature.clone()),
                memo: t.memo,
                references: t.references,
            },
            hash: t.hash,
            signature: t.signature,
        }
    }).collect();
    
    let genesis_time = checkpoints.first().map(|c| c.timestamp).unwrap_or(now_secs);
    let total_supply: f64 = accounts.values().map(|a| a.balance + a.staked).sum();
    
    SyncSnapshot {
        accounts,
        validators,
        checkpoints,
        gas_price: 0.001,
        total_supply,
        genesis_time,
        dag_transactions,
        total_transactions: 0,
        contracts: HashMap::new(),
        rewards_snapshot: None,
        emission_snapshot: None,
        slashing_snapshot: None,
        total_burned: 0.0,
        total_to_validators: 0.0,
        genesis_hash,
        finalized_tx_hashes: Vec::new(),
        tx_checkpoint_heights: HashMap::new(),
        weight_scores: HashMap::new(),
    }
}

/// Fallback for non-P2P builds - tries HTTP sync from NODE_PEERS
#[cfg(not(feature = "p2p"))]
pub(crate) async fn try_presync_from_peers(_bootstrap_peers: &[String], is_genesis_node: bool) -> Option<SyncSnapshot> {
    // Get HTTP peers from NODE_PEERS env var
    let http_peers: Vec<String> = std::env::var("NODE_PEERS")
        .map(|p| p.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
        .unwrap_or_default();
    
    if http_peers.is_empty() {
        if is_genesis_node {
            info!("PRE-SYNC: Genesis node, creating new chain");
        } else {
            warn!("PRE-SYNC: P2P feature not enabled and no NODE_PEERS configured");
            warn!("PRE-SYNC: Set NODE_PEERS to sync from existing network (e.g., NODE_PEERS=https://rinku-genesis.fly.dev)");
        }
        return None;
    }
    
    info!("PRE-SYNC: P2P not enabled, using HTTP sync from NODE_PEERS");
    try_http_presync(&http_peers, is_genesis_node).await
}
