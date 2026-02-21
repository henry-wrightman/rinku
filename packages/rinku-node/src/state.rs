use anyhow::Result;
use rinku_core::{
    dag::Dag,
    types::{Account, Checkpoint, SignedTransaction, Validator, AggregatedWeight},
    weight::{calculate_account_weight, WeightTrie},
};
use serde::{Deserialize, Serialize};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Result of add_transaction - distinguishes between fully accepted and buffered transactions
#[derive(Debug, Clone, PartialEq)]
pub enum TransactionResult {
    /// Transaction was fully validated and added to the DAG
    Accepted,
    /// Transaction has a future nonce and was buffered for later processing
    /// Gossip layer should NOT propagate buffered transactions
    Buffered,
}

use crate::bls::bls_verify;
use crate::config::NodeConfig;
use crate::consensus::VoteType;
use crate::emission::EmissionService;
use crate::storage::RedbStorage;
use crate::rewards::RewardsService;
use crate::slashing::SlashingService;

#[cfg(feature = "p2p")]
use crate::network::{PeerHandshake, SyncRequest, SyncResponse, SnapshotData};

/// Try to fetch a snapshot from configured P2P bootstrap peers before creating genesis
/// This ensures non-genesis nodes sync from the network instead of creating their own chain
/// Uses exponential backoff retry for robustness - genesis node may still be starting
#[cfg(feature = "p2p")]
async fn try_presync_from_peers(bootstrap_peers: &[String], is_genesis_node: bool) -> Option<SyncSnapshot> {
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

/// HTTP-based snapshot sync response (matches API response format)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HttpSnapshotResponse {
    accounts: HashMap<String, Account>,
    validators: HashMap<String, Validator>,
    checkpoints: Vec<Checkpoint>,
    gas_price: f64,
    total_supply: f64,
    genesis_time: u64,
    dag_transactions: Vec<SignedTransaction>,
    total_transactions: u64,
    #[allow(dead_code)]
    checkpoint_height: u64,
    #[serde(default)]
    contracts: HashMap<String, crate::contracts::ContractState>,
    #[serde(default)]
    rewards_snapshot: Option<crate::rewards::RewardsSnapshot>,
    #[serde(default)]
    emission_snapshot: Option<crate::emission::EmissionSnapshot>,
    #[serde(default)]
    slashing_snapshot: Option<crate::slashing::SlashingSnapshot>,
    #[serde(default)]
    total_burned: f64,
    #[serde(default)]
    total_to_validators: f64,
    #[serde(default)]
    genesis_hash: Option<String>,
    #[serde(default)]
    finalized_tx_hashes: Vec<String>,
    #[serde(default)]
    tx_checkpoint_heights: HashMap<String, u64>,
    #[serde(default)]
    weight_scores: HashMap<String, AggregatedWeight>,
}

/// Try HTTP-based sync from NODE_PEERS when P2P isn't available
/// Uses exponential backoff retry for robustness
async fn try_http_presync(http_peers: &[String], is_genesis_node: bool) -> Option<SyncSnapshot> {
    if is_genesis_node {
        info!("PRE-SYNC: Genesis node, skipping HTTP sync");
        return None;
    }
    
    if http_peers.is_empty() {
        info!("PRE-SYNC: No HTTP peers configured (NODE_PEERS empty)");
        return None;
    }
    
    info!("PRE-SYNC: Attempting HTTP sync from {} peer(s)", http_peers.len());
    
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .ok()?;
    
    // Retry with exponential backoff
    let delays = [5, 10, 20, 40, 80, 80, 80, 80]; // Total ~6 min max
    
    for (attempt, delay) in delays.iter().enumerate() {
        info!("PRE-SYNC: HTTP attempt {}/8...", attempt + 1);
        
        for peer in http_peers {
            let url = format!("{}/api/sync/snapshot", peer.trim_end_matches('/'));
            
            match client.get(&url).send().await {
                Ok(response) if response.status().is_success() => {
                    match response.json::<HttpSnapshotResponse>().await {
                        Ok(snapshot_resp) => {
                            info!(
                                "PRE-SYNC: HTTP received snapshot from {}: {} accounts, {} checkpoints",
                                peer,
                                snapshot_resp.accounts.len(),
                                snapshot_resp.checkpoints.len()
                            );
                            
                            return Some(SyncSnapshot {
                                accounts: snapshot_resp.accounts,
                                validators: snapshot_resp.validators,
                                checkpoints: snapshot_resp.checkpoints,
                                gas_price: snapshot_resp.gas_price,
                                total_supply: snapshot_resp.total_supply,
                                genesis_time: snapshot_resp.genesis_time,
                                dag_transactions: snapshot_resp.dag_transactions,
                                total_transactions: snapshot_resp.total_transactions,
                                contracts: snapshot_resp.contracts,
                                rewards_snapshot: snapshot_resp.rewards_snapshot,
                                emission_snapshot: snapshot_resp.emission_snapshot,
                                slashing_snapshot: snapshot_resp.slashing_snapshot,
                                total_burned: snapshot_resp.total_burned,
                                total_to_validators: snapshot_resp.total_to_validators,
                                genesis_hash: snapshot_resp.genesis_hash,
                                finalized_tx_hashes: snapshot_resp.finalized_tx_hashes,
                                tx_checkpoint_heights: snapshot_resp.tx_checkpoint_heights,
                                weight_scores: snapshot_resp.weight_scores,
                            });
                        }
                        Err(e) => {
                            warn!("PRE-SYNC: Failed to parse snapshot from {}: {}", peer, e);
                        }
                    }
                }
                Ok(response) => {
                    warn!("PRE-SYNC: HTTP {} from {}: status {}", url, peer, response.status());
                }
                Err(e) => {
                    warn!("PRE-SYNC: HTTP request to {} failed: {}", peer, e);
                }
            }
        }
        
        if attempt < delays.len() - 1 {
            info!("PRE-SYNC: Waiting {}s before retry...", delay);
            tokio::time::sleep(std::time::Duration::from_secs(*delay)).await;
        }
    }
    
    None
}

/// Fallback for non-P2P builds - tries HTTP sync from NODE_PEERS
#[cfg(not(feature = "p2p"))]
async fn try_presync_from_peers(_bootstrap_peers: &[String], is_genesis_node: bool) -> Option<SyncSnapshot> {
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

/// Snapshot of node state for efficient sync
/// Contains derived state (accounts) + checkpoint metadata + recent DAG
/// This is much smaller than full transaction history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSnapshot {
    pub accounts: HashMap<String, Account>,
    pub validators: HashMap<String, Validator>,
    pub checkpoints: Vec<Checkpoint>,
    pub gas_price: f64,
    pub total_supply: f64,
    pub genesis_time: u64,
    pub dag_transactions: Vec<SignedTransaction>,
    pub total_transactions: u64,
    #[serde(default)]
    pub contracts: HashMap<String, crate::contracts::ContractState>,
    #[serde(default)]
    pub rewards_snapshot: Option<crate::rewards::RewardsSnapshot>,
    #[serde(default)]
    pub emission_snapshot: Option<crate::emission::EmissionSnapshot>,
    #[serde(default)]
    pub slashing_snapshot: Option<crate::slashing::SlashingSnapshot>,
    #[serde(default)]
    pub total_burned: f64,
    #[serde(default)]
    pub total_to_validators: f64,
    #[serde(default)]
    pub genesis_hash: Option<String>,
    #[serde(default)]
    pub finalized_tx_hashes: Vec<String>,
    /// Maps transaction hash to checkpoint height for proper finality tracking
    #[serde(default)]
    pub tx_checkpoint_heights: HashMap<String, u64>,
    /// Aggregated weight scores for trust attestations
    #[serde(default)]
    pub weight_scores: HashMap<String, AggregatedWeight>,
}

/// Result of applying a sync snapshot, includes local-only accounts for push-back
#[derive(Debug, Clone)]
pub struct SyncApplyResult {
    pub dag_transactions_added: usize,
    pub local_only_accounts: HashMap<String, Account>,
}

pub struct DagNodeInfo {
    pub hash: String,
    pub from: String,
    pub to: String,
    pub amount: f64,
    pub fee: f64,
    pub nonce: u64,
    pub ts: u64,
    pub parents: Vec<String>,
    pub finalized: bool,
    pub weight: f64,
    pub kind: Option<rinku_core::types::TransactionKind>,
    pub sig: String,
}

/// Combined stats for dashboard - fetched with a single lock acquisition
#[derive(Clone, Debug)]
pub struct DashboardStats {
    pub dag_nodes: usize,
    pub tip_count: usize,
    pub account_count: usize,
    pub checkpoint_height: u64,
    pub finalized_count: usize,
    pub unfinalized_count: usize,
    pub total_transactions: u64,
    pub tips: Vec<String>,
    pub gas_price: f64,
    pub total_burned: f64,
    pub avg_gas: f64,
    pub latest_checkpoint_id: Option<String>,
}

use std::collections::VecDeque;

#[derive(Debug)]
pub struct StateInner {
    pub dag: Dag,
    pub accounts: HashMap<String, Account>,
    pub validators: HashMap<String, Validator>,
    pub checkpoints: Vec<Checkpoint>,
    pub contracts: HashMap<String, crate::contracts::ContractState>,
    pub current_gas_price: f64,
    pub total_supply: f64,
    pub genesis_time: u64,
    pub genesis_hash: Option<String>,
    pub total_burned: f64,
    pub total_to_validators: f64,
    pub txs_this_period: u64,
    pub period_start_ms: u64,
    pub total_transactions: u64,
    pub config: NodeConfig,
    pub last_checkpoint_time_ms: u64,
    pub finality_times_ms: VecDeque<u64>, // Rolling window for percentile calculations
    pub finality_sum_ms: u64,             // Sum of all finality times for accurate average
    pub finality_count: u64,              // Count of all finalized transactions
    pub finality_max_ms: u64,             // Track maximum finality time
    pub node_validator_address: Option<String>,
    pub node_bls_public_key: Option<String>,
    pub node_peer_id: Option<String>,
    pub node_listen_addr: Option<String>,
    // TPS calculation: track (timestamp_ms, finalized_tx_count) for sliding window
    pub finalized_tx_history: VecDeque<(u64, u64)>,
    // Track whether this node has ever successfully synced from the network
    // If false, we're a new node that should adopt peer's genesis hash
    pub has_synced_from_network: bool,
    /// Weight attestation trie for protocol-level trust scoring
    pub weight_trie: Option<WeightTrie>,
}

#[derive(Clone)]
pub struct NodeState {
    config: NodeConfig,
    pub inner: Arc<RwLock<StateInner>>,
    storage: Arc<RedbStorage>,
    pub emission: Arc<RwLock<EmissionService>>,
    pub slashing: Arc<RwLock<SlashingService>>,
    pub rewards: Arc<RwLock<RewardsService>>,
    start_time: std::time::Instant,
}

/// Result of state root computation with precomputed proofs
/// Proofs are computed from the same simulated account set used for state_root
/// to ensure proof verification will succeed
pub struct StateRootWithProofs {
    pub state_root: String,
    pub proofs: std::collections::HashMap<String, rinku_core::types::AccountStateProof>,
}

impl NodeState {
    pub fn storage(&self) -> &Arc<RedbStorage> {
        &self.storage
    }

    pub async fn get_chain_info(&self) -> (String, String) {
        let state = self.inner.read().await;
        (state.config.chain_id.clone(), state.config.network_id.clone())
    }
    
    pub async fn new(config: NodeConfig) -> Result<Self> {
        let storage = RedbStorage::open(&config.data_dir)?;
        let storage = Arc::new(storage);

        let inner =
            if let Some((accounts, validators, checkpoints, gas_price, supply, genesis, dag_entries)) =
                storage.load_snapshot()?
            {
                let tx_count = dag_entries.len() as u64;
                let checkpoint_count = checkpoints.len() as u64;
                info!(
                    "Restored from snapshot: {} accounts, {} txs, {} checkpoints",
                    accounts.len(),
                    tx_count,
                    checkpoint_count
                );
                let mut dag = Dag::new(config.max_dag_nodes);
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                    
                // Build a set of all hashes we're loading for parent validation
                let loaded_hashes: std::collections::HashSet<String> = dag_entries
                    .iter()
                    .map(|e| e.tx.hash.clone())
                    .collect();
                    
                for entry in dag_entries {
                    // Genesis transaction and txs from before checkpoints should be considered finalized
                    let is_genesis = entry.tx.tx.from == "genesis";
                    let is_finalized = entry.finalized || is_genesis || checkpoint_count > 0;
                    // Calculate weight from sender account
                    let tx_weight = if let Some(account) = accounts.get(&entry.tx.tx.from) {
                        calculate_account_weight(account, now_secs)
                    } else {
                        1.0
                    };
                    
                    // Use parents from DagSnapshotEntry (preferred) or fall back to tx.parents
                    // Filter out parents that aren't in the loaded snapshot (they were pruned)
                    let parents = if !entry.parents.is_empty() {
                        entry.parents.iter()
                            .filter(|p| loaded_hashes.contains(*p))
                            .cloned()
                            .collect()
                    } else {
                        entry.tx.tx.parents.iter()
                            .filter(|p| loaded_hashes.contains(*p))
                            .cloned()
                            .collect()
                    };
                    
                    let node = rinku_core::types::DagNode {
                        hash: entry.tx.hash.clone(),
                        tx: entry.tx.clone(),
                        parents,
                        children: Vec::new(),
                        weight: tx_weight,
                        finalized: is_finalized,
                        checkpoint_height: entry.checkpoint_height.or_else(|| {
                            if is_genesis {
                                Some(0)
                            } else if is_finalized {
                                Some(checkpoint_count)
                            } else {
                                None
                            }
                        }),
                        received_at_ms: Some(entry.tx.tx.timestamp),
                    };
                    let _ = dag.add_node(node);
                }
                
                // CRITICAL: Rebuild parent-child relationships after loading from snapshot.
                // Transactions may be loaded in arbitrary order (not topological), causing
                // children to be added before their parents. This breaks tip tracking.
                let tips_before = dag.tip_count();
                let (nodes_processed, tips_after, dangling_parents) = dag.rebuild_tips();
                info!(
                    "DAG tips rebuilt after snapshot load: {} nodes, {} tips -> {} tips",
                    nodes_processed, tips_before, tips_after
                );
                if dangling_parents > 0 {
                    warn!(
                        "DAG rebuild found {} dangling parent references (pruned parents not in snapshot)",
                        dangling_parents
                    );
                }
                
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let last_checkpoint_time = checkpoints
                    .last()
                    .map(|c| c.timestamp * 1000)
                    .unwrap_or(now_ms);
                let loaded_contracts = storage.load_contracts().unwrap_or_default();
                let contracts: HashMap<String, crate::contracts::ContractState> = loaded_contracts
                    .into_iter()
                    .map(|c| (c.contract_id.clone(), c))
                    .collect();
                info!("Loaded {} contracts from storage", contracts.len());
                
                let stored_genesis_hash = storage.load_genesis_hash().unwrap_or(None);
                info!("Loaded genesis hash: {:?}", stored_genesis_hash.as_ref().map(|h| &h[..16.min(h.len())]));
                
                StateInner {
                    dag,
                    accounts,
                    validators,
                    checkpoints,
                    contracts,
                    current_gas_price: gas_price,
                    total_supply: supply,
                    genesis_time: genesis,
                    genesis_hash: stored_genesis_hash,
                    total_burned: 0.0,
                    total_to_validators: 0.0,
                    txs_this_period: 0,
                    period_start_ms: now_ms,
                    total_transactions: tx_count,
                    config: config.clone(),
                    last_checkpoint_time_ms: last_checkpoint_time,
                    finality_times_ms: VecDeque::with_capacity(1000),
                    finality_sum_ms: 0,
                    finality_count: 0,
                    finality_max_ms: 0,
                    node_validator_address: None,
                    node_bls_public_key: None,
                    node_peer_id: None,
                    node_listen_addr: None,
                    finalized_tx_history: VecDeque::new(),
                    has_synced_from_network: true, // Restored from storage = already synced
                    weight_trie: {
                        let mut wt = WeightTrie::new();
                        if let Ok(Some(saved_weights)) = storage.load_weights() {
                            info!("Restoring {} transaction weight scores from storage", saved_weights.len());
                            wt.load_weights(saved_weights);
                        }
                        Some(wt)
                    },
                }
            } else {
                // Try to sync from P2P bootstrap peers BEFORE creating genesis
                // This ensures non-genesis nodes join the existing network
                if let Some(snapshot) = try_presync_from_peers(&config.p2p.bootstrap_peers, config.is_genesis_node).await {
                    // Use snapshot from peer instead of creating fresh genesis
                    info!("PRE-SYNC: Using state from network peer instead of creating new genesis");
                    
                    let now_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    
                    let mut dag = Dag::new(config.max_dag_nodes);
                    let checkpoint_count = snapshot.checkpoints.len() as u64;
                    
                    for tx in &snapshot.dag_transactions {
                        let is_genesis = tx.tx.from == "genesis";
                        let is_finalized = is_genesis || checkpoint_count > 0;
                        let tx_weight = if let Some(account) = snapshot.accounts.get(&tx.tx.from) {
                            calculate_account_weight(account, now_secs)
                        } else {
                            1.0
                        };
                        
                        // Use tx_checkpoint_heights from snapshot if available
                        let checkpoint_height = if is_genesis {
                            Some(0)
                        } else if let Some(&height) = snapshot.tx_checkpoint_heights.get(&tx.hash) {
                            Some(height)
                        } else if is_finalized {
                            Some(checkpoint_count)
                        } else {
                            None
                        };
                        
                        let node = rinku_core::types::DagNode {
                            hash: tx.hash.clone(),
                            tx: tx.clone(),
                            parents: tx.tx.parents.clone(),
                            children: Vec::new(),
                            weight: tx_weight,
                            finalized: is_finalized,
                            checkpoint_height,
                            received_at_ms: Some(tx.tx.timestamp),
                        };
                        let _ = dag.add_node(node);
                    }
                    
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    let last_checkpoint_time = snapshot.checkpoints
                        .last()
                        .map(|c| c.timestamp * 1000)
                        .unwrap_or(now_ms);
                    
                    // Save the genesis hash from peer
                    if let Some(ref genesis_hash) = snapshot.genesis_hash {
                        let _ = storage.save_genesis_hash(genesis_hash);
                        info!("PRE-SYNC: Saved peer genesis hash: {}", &genesis_hash[..16.min(genesis_hash.len())]);
                    }
                    
                    // Create inner state from snapshot
                    let inner = StateInner {
                        dag,
                        accounts: snapshot.accounts.clone(),
                        validators: snapshot.validators.clone(),
                        checkpoints: snapshot.checkpoints.clone(),
                        contracts: snapshot.contracts.clone(),
                        current_gas_price: snapshot.gas_price,
                        total_supply: snapshot.total_supply,
                        genesis_time: snapshot.genesis_time,
                        genesis_hash: snapshot.genesis_hash.clone(),
                        total_burned: snapshot.total_burned,
                        total_to_validators: snapshot.total_to_validators,
                        txs_this_period: 0,
                        period_start_ms: now_ms,
                        total_transactions: snapshot.total_transactions,
                        config: config.clone(),
                        last_checkpoint_time_ms: last_checkpoint_time,
                        finality_times_ms: VecDeque::with_capacity(1000),
                        finality_sum_ms: 0,
                        finality_count: 0,
                        finality_max_ms: 0,
                        node_validator_address: None,
                        node_bls_public_key: None,
                        node_peer_id: None,
                        node_listen_addr: None,
                        finalized_tx_history: VecDeque::new(),
                        has_synced_from_network: true,
                        weight_trie: Some(WeightTrie::new()),
                    };
                    
                    // Handle emission, slashing, rewards from snapshot
                    let emission = if let Some(em_snapshot) = snapshot.emission_snapshot {
                        EmissionService::from_json(em_snapshot)
                    } else {
                        EmissionService::new()
                    };
                    
                    let slashing = if let Some(sl_snapshot) = snapshot.slashing_snapshot {
                        SlashingService::from_json(sl_snapshot)
                    } else {
                        SlashingService::new()
                    };
                    
                    let rewards = if let Some(rw_snapshot) = snapshot.rewards_snapshot {
                        info!(
                            "PRE-SYNC: Restoring rewards: {} stakes, {} pending",
                            rw_snapshot.stakes.len(),
                            rw_snapshot.pending_rewards.len()
                        );
                        RewardsService::from_json(rw_snapshot)
                    } else {
                        RewardsService::new(crate::rewards::RewardConfig::default())
                    };
                    
                    let node_state = Self {
                        config,
                        inner: Arc::new(RwLock::new(inner)),
                        storage,
                        emission: Arc::new(RwLock::new(emission)),
                        slashing: Arc::new(RwLock::new(slashing)),
                        rewards: Arc::new(RwLock::new(rewards)),
                        start_time: std::time::Instant::now(),
                    };
                    
                    node_state.sync_stakes_to_accounts().await;
                    node_state.recalculate_dag_weights().await;
                    
                    return Ok(node_state);
                }
                
                // No peers available or pre-sync failed
                // IMPORTANT: Only genesis nodes should create genesis. Validators must sync from network.
                if !config.is_genesis_node && !config.p2p.bootstrap_peers.is_empty() {
                    return Err(anyhow::anyhow!(
                        "FATAL: Validator node failed to sync from bootstrap peers after retries. \
                         Cannot create independent genesis. Ensure genesis node is running and accessible. \
                         Set IS_GENESIS_NODE=true only for the first node in the network."
                    ));
                }
                
                info!("Creating fresh genesis (IS_GENESIS_NODE=true or no peers configured)");
                
                let genesis_time = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs();

                let mut accounts = HashMap::new();
                let faucet_balance = 1_000_000.0;
                accounts.insert(
                    "faucet".to_string(),
                    Account {
                        address: "faucet".to_string(),
                        balance: faucet_balance,
                        nonce: 0,
                        first_seen: genesis_time,
                        staked: 0.0,
                        unbonding: 0.0,
                        unbonding_release: None,
                        latest_balance_proof: None,
                    },
                );
                info!("Faucet account initialized with {} RKU", faucet_balance);

                let mut dag = Dag::new(config.max_dag_nodes);
                // Generate a proper 64-character hex hash for genesis
                let genesis_data = format!("genesis:{}", genesis_time);
                let genesis_hash = rinku_core::sha256_hex(&genesis_data);
                let genesis_tx = SignedTransaction {
                    tx: rinku_core::types::Transaction {
                        from: "genesis".to_string(),
                        to: "faucet".to_string(),
                        amount: faucet_balance,
                        nonce: 0,
                        timestamp: genesis_time * 1000,
                        parents: vec![],
                        kind: None,
                        gas_limit: None,
                        gas_price: Some(0.0),
                        data: None,
                        signature: Some("genesis-signature".to_string()),
                        memo: None,
                        references: None,
                    },
                    hash: genesis_hash.clone(),
                    signature: "genesis-signature".to_string(),
                };
                let genesis_node = rinku_core::types::DagNode {
                    hash: genesis_hash.clone(),
                    tx: genesis_tx,
                    parents: vec![],
                    children: vec![],
                    weight: 1.0,
                    finalized: true,
                    checkpoint_height: Some(0),
                    received_at_ms: Some(genesis_time * 1000),
                };
                let _ = dag.add_node(genesis_node);
                info!(
                    "Genesis transaction created: {}",
                    &genesis_hash[..16.min(genesis_hash.len())]
                );

                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                
                // Create genesis checkpoint (height 0) so genesis tx can have proofs generated
                // Note: This is a placeholder checkpoint - real BLS signatures added when checkpoint service starts
                // IMPORTANT: Hash must be computed using same format as compute_checkpoint_hash
                // Format: "{}:{}:{}:{}:{}:{}" with height, tx_merkle_root, state_root, receipt_root, tip_count, timestamp
                let genesis_state_root = "0".repeat(64);
                let genesis_receipt_root = "0".repeat(64);
                let genesis_tip_count = 1u32;
                let genesis_checkpoint_hash = rinku_core::sha256_hex(&format!(
                    "{}:{}:{}:{}:{}:{}",
                    0, // height
                    genesis_hash,
                    genesis_state_root,
                    genesis_receipt_root,
                    genesis_tip_count,
                    genesis_time // timestamp
                ));
                let genesis_checkpoint = rinku_core::types::Checkpoint {
                    height: 0,
                    hash: genesis_checkpoint_hash,
                    previous_hash: None,
                    tx_merkle_root: genesis_hash.clone(),
                    state_root: genesis_state_root,
                    receipt_root: genesis_receipt_root,
                    tip_count: genesis_tip_count,
                    timestamp: genesis_time,
                    validator_signatures: vec![], // Will be updated when checkpoint service starts
                    aggregated_signature: None,
                    signer_bitmap: None,
                    finalized_tx_hashes: vec![genesis_hash.clone()],
                    weight_trie_root: String::new(),
                };
                
                info!("Genesis hash created: {}", &genesis_hash[..16.min(genesis_hash.len())]);
                
                let _ = storage.save_genesis_hash(&genesis_hash);
                
                StateInner {
                    dag,
                    accounts,
                    validators: HashMap::new(),
                    checkpoints: vec![genesis_checkpoint],
                    contracts: HashMap::new(),
                    current_gas_price: config.gas.min_gas_price,
                    total_supply: config.tokenomics.genesis_allocation,
                    genesis_time,
                    genesis_hash: Some(genesis_hash),
                    total_burned: 0.0,
                    total_to_validators: 0.0,
                    txs_this_period: 0,
                    period_start_ms: now_ms,
                    total_transactions: 1,
                    config: config.clone(),
                    last_checkpoint_time_ms: now_ms,
                    finality_times_ms: VecDeque::with_capacity(1000),
                    finality_sum_ms: 0,
                    finality_count: 0,
                    finality_max_ms: 0,
                    node_validator_address: None,
                    node_bls_public_key: None,
                    node_peer_id: None,
                    node_listen_addr: None,
                    finalized_tx_history: VecDeque::new(),
                    has_synced_from_network: false, // Fresh node = hasn't synced yet
                    weight_trie: Some(WeightTrie::new()),
                }
            };

        // Load emission from storage or create fresh
        let emission = if let Some(snapshot) = storage.load_emission()? {
            info!(
                "Restored emission: {:.2} RKU emitted, {:.2} RKU burned",
                snapshot.total_emitted,
                snapshot.total_burned
            );
            EmissionService::from_json(snapshot)
        } else {
            EmissionService::new()
        };
        
        let slashing = SlashingService::new();

        // Load rewards from storage or create fresh
        let rewards = if let Some(snapshot) = storage.load_rewards()? {
            info!(
                "Restored rewards: {} stakes, {} pending",
                snapshot.stakes.len(),
                snapshot.pending_rewards.len()
            );
            RewardsService::from_json(snapshot)
        } else {
            RewardsService::new(crate::rewards::RewardConfig::default())
        };

        let node_state = Self {
            config,
            inner: Arc::new(RwLock::new(inner)),
            storage,
            emission: Arc::new(RwLock::new(emission)),
            slashing: Arc::new(RwLock::new(slashing)),
            rewards: Arc::new(RwLock::new(rewards)),
            start_time: std::time::Instant::now(),
        };
        
        // Sync stakes from RewardsService to account state
        node_state.sync_stakes_to_accounts().await;
        
        // Recalculate DAG weights based on current account state
        node_state.recalculate_dag_weights().await;
        
        // Ensure genesis hash is computed and persisted if not already
        // This handles upgrade from older databases that didn't store genesis hash
        let needs_genesis_hash = {
            let state = node_state.inner.read().await;
            state.genesis_hash.is_none()
        };
        if needs_genesis_hash {
            if let Some(hash) = node_state.get_genesis_hash().await {
                node_state.set_genesis_hash(hash.clone()).await;
                info!("Persisted genesis hash on startup: {}", &hash[..16.min(hash.len())]);
            }
        }
        
        Ok(node_state)
    }
    
    /// Sync all stakes from RewardsService to account.staked fields
    async fn sync_stakes_to_accounts(&self) {
        let rewards = self.rewards.read().await;
        let stakes: Vec<(String, f64, u64)> = rewards.get_all_stakes()
            .iter()
            .map(|s| (s.staker.clone(), s.amount, s.staked_at / 1000))
            .collect();
        drop(rewards);
        
        if stakes.is_empty() {
            return;
        }
        
        let mut state = self.inner.write().await;
        let mut synced = 0;
        for (address, amount, staked_at) in stakes {
            if let Some(account) = state.accounts.get_mut(&address) {
                account.staked = amount;
            } else {
                let mut account = Account::new(address.clone(), staked_at);
                account.staked = amount;
                state.accounts.insert(address, account);
            }
            synced += 1;
        }
        info!("Synced {} stakes to account state", synced);
    }
    
    /// Recalculate DAG node weights based on current account state
    /// This is needed on startup to fix weights for transactions that were added
    /// before their sender's stake was synced to account.staked
    async fn recalculate_dag_weights(&self) {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let mut state = self.inner.write().await;
        
        // Get all account weights first
        let account_weights: std::collections::HashMap<String, f64> = state.accounts
            .iter()
            .map(|(addr, acc)| (addr.clone(), calculate_account_weight(acc, now_secs)))
            .collect();
        
        // Update DAG node weights
        let mut updated = 0;
        for node in state.dag.nodes_mut() {
            let sender = &node.tx.tx.from;
            if let Some(&new_weight) = account_weights.get(sender) {
                if (node.weight - new_weight).abs() > 0.01 {
                    node.weight = new_weight;
                    updated += 1;
                }
            }
        }
        
        if updated > 0 {
            info!("Recalculated {} DAG node weights based on current account state", updated);
        }
    }

    pub async fn save_snapshot(&self) -> Result<()> {
        // Run memory cleanup before saving
        self.cleanup_old_data().await;
        
        let state = self.inner.read().await;
        
        // Log memory metrics for monitoring
        info!(
            "Memory metrics: DAG nodes={}, accounts={}, validators={}, checkpoints={}, contracts={}",
            state.dag.node_count(),
            state.accounts.len(),
            state.validators.len(),
            state.checkpoints.len(),
            state.contracts.len()
        );
        // Create DagSnapshotEntry for each node, preserving parent references
        let dag_entries: Vec<crate::storage::DagSnapshotEntry> = state.dag.nodes()
            .map(|node| crate::storage::DagSnapshotEntry {
                tx: node.tx.clone(),
                parents: node.parents.clone(),
                finalized: node.finalized,
                checkpoint_height: node.checkpoint_height,
            })
            .collect();
        self.storage.save_snapshot(
            &state.accounts,
            &state.validators,
            &state.checkpoints,
            state.current_gas_price,
            state.total_supply,
            state.genesis_time,
            &dag_entries,
        )?;

        // Save weight trie (trust scores)
        if let Some(ref weight_trie) = state.weight_trie {
            let weights = weight_trie.all_weights().clone();
            if !weights.is_empty() {
                self.storage.save_weights(&weights)?;
                info!("Saved {} transaction weight scores to storage", weights.len());
            }
        }
        drop(state);

        // Also save rewards/staking state
        let rewards = self.rewards.read().await;
        let rewards_snapshot = rewards.to_json();
        drop(rewards);
        self.storage.save_rewards(&rewards_snapshot)?;

        // Save emission state
        let emission = self.emission.read().await;
        let emission_snapshot = emission.to_json();
        drop(emission);
        self.storage.save_emission(&emission_snapshot)?;

        Ok(())
    }
    
    /// Periodic cleanup to prevent memory leaks
    async fn cleanup_old_data(&self) {
        const MAX_CHECKPOINTS: usize = 500;  // Keep last ~2 hours of checkpoints
        const MAX_ACCOUNTS: usize = 50000;   // Cap on accounts
        
        let mut state = self.inner.write().await;
        
        // Prune old checkpoints (keep most recent MAX_CHECKPOINTS)
        if state.checkpoints.len() > MAX_CHECKPOINTS {
            let to_remove = state.checkpoints.len() - MAX_CHECKPOINTS;
            state.checkpoints.drain(0..to_remove);
            info!("Pruned {} old checkpoints, {} remaining", to_remove, state.checkpoints.len());
        }
        
        // Prune zero-balance accounts with no stake (keep accounts under limit)
        if state.accounts.len() > MAX_ACCOUNTS {
            let mut removable: Vec<String> = state.accounts
                .iter()
                .filter(|(_, a)| a.balance < 0.001 && a.staked < 0.001)
                .map(|(k, _)| k.clone())
                .collect();
            
            // Remove oldest first (by first_seen)
            removable.sort_by(|a, b| {
                let a_time = state.accounts.get(a).map(|acc| acc.first_seen).unwrap_or(0);
                let b_time = state.accounts.get(b).map(|acc| acc.first_seen).unwrap_or(0);
                a_time.cmp(&b_time)
            });
            
            let to_remove = (state.accounts.len() - MAX_ACCOUNTS).min(removable.len());
            for key in removable.into_iter().take(to_remove) {
                state.accounts.remove(&key);
            }
            
            if to_remove > 0 {
                info!("Pruned {} inactive accounts, {} remaining", to_remove, state.accounts.len());
            }
        }
        
        drop(state);
        
        // Prune rewards data
        let mut rewards = self.rewards.write().await;
        let pruned = rewards.prune_old_data();
        if pruned > 0 {
            info!("Pruned {} expired witness entries", pruned);
        }
    }

    pub async fn get_uptime_seconds(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    pub async fn set_validator_info(&self, address: Option<String>, bls_public_key: Option<String>, allow_auto_register: bool) {
        use crate::validator_identity::MIN_VALIDATOR_STAKE;
        
        let mut state = self.inner.write().await;
        state.node_validator_address = address.clone();
        state.node_bls_public_key = bls_public_key.clone();
        
        // Register or update in state.validators so it gets synced to peers via snapshots
        if let Some(ref addr) = address {
            if let Some(existing) = state.validators.get_mut(addr) {
                // Update existing entry if BLS key is missing or different
                if existing.bls_public_key.is_none() || existing.bls_public_key != bls_public_key {
                    info!("Updating BLS key for validator {} in state.validators", addr);
                    existing.bls_public_key = bls_public_key.clone();
                }
            } else if allow_auto_register {
                // Create new entry ONLY if auto-registration is allowed
                // When GENESIS_VALIDATORS is set, we should NOT auto-register because
                // GENESIS_VALIDATORS is the authoritative source of truth
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let validator = Validator {
                    address: addr.clone(),
                    stake: MIN_VALIDATOR_STAKE,
                    first_stake_time: now_secs * 1000,
                    bls_public_key: bls_public_key.clone(),
                    missed_checkpoints: 0,
                };
                info!("Registering local validator {} in state.validators for peer sync", addr);
                state.validators.insert(addr.clone(), validator);
            } else {
                warn!(
                    "Local validator {} not in GENESIS_VALIDATORS - skipping auto-registration",
                    addr
                );
            }
        }
    }

    pub async fn get_validator_info(&self) -> (Option<String>, Option<String>) {
        let state = self.inner.read().await;
        (state.node_validator_address.clone(), state.node_bls_public_key.clone())
    }

    pub async fn set_peer_info(&self, peer_id: String, listen_addr: String) {
        let mut state = self.inner.write().await;
        state.node_peer_id = Some(peer_id);
        state.node_listen_addr = Some(listen_addr);
    }

    pub async fn get_bootstrap_info(&self) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
        let state = self.inner.read().await;
        (
            state.node_peer_id.clone(),
            state.node_listen_addr.clone(),
            state.node_validator_address.clone(),
            state.node_bls_public_key.clone(),
        )
    }

    pub async fn get_validator_bls_pubkey_bytes(&self, address: &str) -> Option<Vec<u8>> {
        let state = self.inner.read().await;
        let key = state.validators.get(address)?.bls_public_key.as_ref()?;
        let decoded = URL_SAFE_NO_PAD.decode(key).ok()
            .or_else(|| hex::decode(key).ok());
        decoded
    }

    pub async fn verify_slashing_evidence(&self, evidence: &crate::slashing::DoubleSignEvidence) -> bool {
        let pubkey = match self.get_validator_bls_pubkey_bytes(&evidence.validator).await {
            Some(key) => key,
            None => return false,
        };
        let sig2 = match evidence.signature2.as_ref() {
            Some(s) => s,
            None => return false,
        };

        let sig1_bytes = URL_SAFE_NO_PAD.decode(&evidence.signature1).ok()
            .or_else(|| hex::decode(&evidence.signature1).ok());
        let sig2_bytes = URL_SAFE_NO_PAD.decode(sig2).ok()
            .or_else(|| hex::decode(sig2).ok());
        let (Some(sig1), Some(sig2)) = (sig1_bytes, sig2_bytes) else {
            return false;
        };

        let hash1_ok = self.verify_signature_for_hash(
            &evidence.hash1,
            &sig1,
            &pubkey,
            evidence.checkpoint_height,
        );
        let hash2_ok = self.verify_signature_for_hash(
            &evidence.hash2,
            &sig2,
            &pubkey,
            evidence.checkpoint_height,
        );
        hash1_ok && hash2_ok
    }

    fn verify_signature_for_hash(
        &self,
        hash: &str,
        signature: &[u8],
        pubkey: &[u8],
        checkpoint_height: u64,
    ) -> bool {
        if bls_verify(hash.as_bytes(), signature, pubkey) {
            return true;
        }
        let vote_types = [VoteType::Prepare, VoteType::Commit, VoteType::Finalize];
        for vote_type in vote_types {
            let mut msg = Vec::new();
            msg.extend_from_slice(&[vote_type as u8]);
            msg.extend_from_slice(&checkpoint_height.to_le_bytes());
            msg.extend_from_slice(hash.as_bytes());
            if bls_verify(&msg, signature, pubkey) {
                return true;
            }
        }
        false
    }
    
    pub async fn get_genesis_hash(&self) -> Option<String> {
        let state = self.inner.read().await;
        if let Some(ref hash) = state.genesis_hash {
            return Some(hash.clone());
        }
        for node in state.dag.get_all_nodes() {
            if node.tx.tx.from == "genesis" {
                return Some(node.hash.clone());
            }
        }
        if let Some(first_checkpoint) = state.checkpoints.first() {
            return Some(first_checkpoint.tx_merkle_root.clone());
        }
        None
    }
    
    pub async fn set_genesis_hash(&self, hash: String) {
        let mut state = self.inner.write().await;
        state.genesis_hash = Some(hash.clone());
        drop(state);
        if let Err(e) = self.storage.save_genesis_hash(&hash) {
            warn!("Failed to persist genesis hash: {}", e);
        }
    }

    /// Check if this node has ever successfully synced from the network.
    /// If false, the node is new and should adopt the peer's genesis hash.
    pub async fn has_synced_from_network(&self) -> bool {
        let state = self.inner.read().await;
        state.has_synced_from_network
    }

    /// Mark this node as having synced from the network.
    /// Called after successfully applying a sync snapshot.
    pub async fn mark_synced_from_network(&self) {
        let mut state = self.inner.write().await;
        state.has_synced_from_network = true;
    }

    pub async fn get_account(&self, address: &str) -> Option<Account> {
        let state = self.inner.read().await;
        state.accounts.get(address).cloned()
    }

    pub async fn get_account_nonce(&self, address: &str) -> u64 {
        let state = self.inner.read().await;
        state.accounts.get(address).map(|a| a.nonce).unwrap_or(0)
    }

    /// Sync account nonce from peer during delta sync.
    /// Only updates if the peer's nonce is greater (prevents regression).
    pub async fn sync_account_nonce(&self, address: &str, peer_nonce: u64) {
        let mut state = self.inner.write().await;
        if let Some(account) = state.accounts.get_mut(address) {
            if peer_nonce > account.nonce {
                tracing::debug!(
                    "Syncing nonce for {}: {} -> {}",
                    address, account.nonce, peer_nonce
                );
                account.nonce = peer_nonce;
            }
        } else {
            // Create account if it doesn't exist with the peer's nonce
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let mut account = Account::new(address.to_string(), now);
            account.nonce = peer_nonce;
            state.accounts.insert(address.to_string(), account);
            tracing::debug!("Created account {} with nonce {}", address, peer_nonce);
        }
    }

    /// Merge accounts pushed from a peer.
    /// AUTHORITATIVE MERGE: Updates accounts when peer has higher/equal nonce
    /// This fixes balance divergence where nonces match but balances differ
    /// Returns (accounts_added, accounts_updated, accounts_balance_fixed)
    pub async fn merge_accounts_from_peer(&self, accounts: HashMap<String, Account>) -> (usize, usize, usize) {
        let mut state = self.inner.write().await;
        let mut added = 0;
        let mut updated = 0;
        let mut balance_fixed = 0;
        
        for (fingerprint, peer_account) in accounts {
            if let Some(local_account) = state.accounts.get_mut(&fingerprint) {
                // Account exists locally - AUTHORITATIVE SYNC
                if peer_account.nonce > local_account.nonce {
                    // Peer has more transactions - take their state
                    *local_account = peer_account;
                    updated += 1;
                } else if peer_account.nonce == local_account.nonce {
                    // Same nonce - check for balance/stake divergence
                    let balance_diff = (peer_account.balance - local_account.balance).abs();
                    let stake_diff = (peer_account.staked - local_account.staked).abs();
                    if balance_diff > 0.0001 || stake_diff > 0.0001 {
                        // Accept peer's state to fix divergence
                        // Peer is authoritative since they initiated the sync
                        info!(
                            "Balance fix (merge) for {}: local={:.6} peer={:.6}",
                            &fingerprint[..12.min(fingerprint.len())], local_account.balance, peer_account.balance
                        );
                        *local_account = peer_account;
                        balance_fixed += 1;
                    }
                }
                // If local has higher nonce, keep local
            } else {
                // Account doesn't exist locally - add it
                state.accounts.insert(fingerprint, peer_account);
                added += 1;
            }
        }
        
        if added > 0 || updated > 0 || balance_fixed > 0 {
            info!(
                "Merged accounts from peer: {} added, {} updated, {} balance-fixed, {} total",
                added, updated, balance_fixed, state.accounts.len()
            );
        }
        
        (added, updated, balance_fixed)
    }

    /// Get all accounts with fingerprints (for pushing to peer)
    pub async fn get_all_accounts_map(&self) -> HashMap<String, Account> {
        let state = self.inner.read().await;
        state.accounts.clone()
    }

    /// Get account count
    pub async fn get_account_count(&self) -> usize {
        let state = self.inner.read().await;
        state.accounts.len()
    }

    /// Update account's staked amount (syncs with RewardsService)
    pub async fn update_account_staked(&self, address: &str, staked_amount: f64, staked_at: Option<u64>) {
        let mut state = self.inner.write().await;
        if let Some(account) = state.accounts.get_mut(address) {
            account.staked = staked_amount;
            if let Some(ts) = staked_at {
                account.first_seen = ts;
            }
        } else {
            // Create account if doesn't exist
            let now = staked_at.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            });
            let mut account = Account::new(address.to_string(), now);
            account.staked = staked_amount;
            state.accounts.insert(address.to_string(), account);
        }
    }

    pub async fn apply_contract_transfer_effects(&self, effects: &[crate::contracts::TransferEffect]) -> anyhow::Result<()> {
        if effects.is_empty() {
            return Ok(());
        }
        let mut state = self.inner.write().await;
        for effect in effects {
            let from_balance = match state.accounts.get(&effect.from) {
                Some(acct) => acct.balance,
                None => {
                    tracing::error!(
                        "Contract transfer rejected: sender {} does not exist in state",
                        &effect.from[..16.min(effect.from.len())]
                    );
                    return Err(anyhow::anyhow!(
                        "Contract transfer sender {} not found in state",
                        effect.from
                    ));
                }
            };

            if from_balance < effect.amount {
                tracing::error!(
                    "Contract transfer rejected: {} has {:.8} but needs {:.8}",
                    &effect.from[..16.min(effect.from.len())],
                    from_balance,
                    effect.amount
                );
                return Err(anyhow::anyhow!(
                    "Contract transfer insufficient balance: {} has {} but needs {}",
                    effect.from, from_balance, effect.amount
                ));
            }

            if let Some(from_acct) = state.accounts.get_mut(&effect.from) {
                from_acct.balance -= effect.amount;
            }

            let to_acct = state
                .accounts
                .entry(effect.to.clone())
                .or_insert_with(|| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    Account::new(effect.to.clone(), now)
                });
            to_acct.balance += effect.amount;
            tracing::debug!(
                "Contract transfer applied: {} -> {} ({:.6} RKU)",
                &effect.from[..16.min(effect.from.len())],
                &effect.to[..16.min(effect.to.len())],
                effect.amount
            );
        }
        Ok(())
    }

    pub async fn get_or_create_account(&self, address: &str) -> Account {
        let mut state = self.inner.write().await;
        if let Some(account) = state.accounts.get(address) {
            account.clone()
        } else {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let account = Account::new(address.to_string(), now);
            state.accounts.insert(address.to_string(), account.clone());
            account
        }
    }

    pub async fn get_tips(&self) -> Vec<String> {
        let state = self.inner.read().await;
        state.dag.tips()
    }

    /// Get a weighted random sample of tips for new transactions (Sparse DAG Sampling)
    /// Returns at most MAX_SAMPLED_TIPS (16) tips, preferring higher-weight tips
    /// This prevents tip explosion while maintaining DAG connectivity
    pub async fn get_sampled_tips(&self) -> Vec<String> {
        let state = self.inner.read().await;
        state.dag.get_sampled_tips()
    }

    /// Get tip count without cloning the entire tips vector (more efficient for backpressure checks)
    pub async fn get_tip_count(&self) -> usize {
        let state = self.inner.read().await;
        state.dag.tip_count()
    }

    pub async fn get_dag_stats(&self) -> (usize, usize, usize) {
        let state = self.inner.read().await;
        (
            state.dag.node_count(),
            state.dag.tip_count(),
            state.accounts.len(),
        )
    }

    pub async fn get_finalized_stats(&self) -> (usize, usize) {
        let state = self.inner.read().await;
        let total = state.dag.node_count();
        let unfinalized = state.dag.unfinalized_count();
        let finalized = total.saturating_sub(unfinalized);
        (finalized, unfinalized)
    }

    /// Returns (avg_finality_ms, median_finality_ms, p95_finality_ms, last_checkpoint_age_ms, checkpoints_per_minute)
    pub async fn get_finality_timing(&self) -> (f64, f64, f64, u64, f64) {
        let state = self.inner.read().await;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let last_checkpoint_age = now_ms.saturating_sub(state.last_checkpoint_time_ms);

        // Calculate checkpoints per minute based on genesis time
        let elapsed_minutes = (now_ms / 1000).saturating_sub(state.genesis_time) as f64 / 60.0;
        let checkpoints_per_minute = if elapsed_minutes > 0.0 {
            state.checkpoints.len() as f64 / elapsed_minutes
        } else {
            0.0
        };

        if state.finality_times_ms.is_empty() {
            return (0.0, 0.0, 0.0, last_checkpoint_age, checkpoints_per_minute);
        }

        // Use rolling window for ALL calculations (avg, median, p95)
        // This gives recent performance, not historical data polluted by stalls
        let mut times: Vec<u64> = state.finality_times_ms.iter().copied().collect();
        times.sort();

        // Calculate average from rolling window (not all-time cumulative)
        let sum: u64 = times.iter().sum();
        let avg = sum as f64 / times.len() as f64;

        let median = times[times.len() / 2] as f64;
        let p95_idx = (times.len() as f64 * 0.95) as usize;
        let p95 = times
            .get(p95_idx)
            .copied()
            .unwrap_or(times[times.len() - 1]) as f64;

        (
            avg,
            median,
            p95,
            last_checkpoint_age,
            checkpoints_per_minute,
        )
    }

    pub async fn get_checkpoint_height(&self) -> u64 {
        let state = self.inner.read().await;
        // CRITICAL: Return the actual height of the last checkpoint, NOT the count!
        // After pruning, len() can be 500 while actual height is 516+
        state.checkpoints.last()
            .map(|cp| cp.height)
            .unwrap_or(0)
    }
    
    /// Get the checkpoint height at which a transaction was finalized
    pub async fn get_tx_checkpoint_height(&self, tx_hash: &str) -> Option<u64> {
        let state = self.inner.read().await;
        // Check DAG node checkpoint_height
        if let Some(node) = state.dag.get_node(tx_hash) {
            return node.checkpoint_height;
        }
        None
    }
    
    /// Get a checkpoint by height
    pub async fn get_checkpoint_by_height(&self, height: u64) -> Option<rinku_core::types::Checkpoint> {
        let state = self.inner.read().await;
        state.checkpoints.iter().find(|cp| cp.height == height).cloned()
    }
    
    /// Get the latest checkpoint
    pub async fn get_latest_checkpoint(&self) -> Option<rinku_core::types::Checkpoint> {
        let state = self.inner.read().await;
        state.checkpoints.last().cloned()
    }
    
    /// Get the node's unique identifier
    pub async fn get_node_id(&self) -> String {
        // Use a hash of the genesis hash as a simple node identifier
        let state = self.inner.read().await;
        state.genesis_hash.clone().unwrap_or_else(|| "unknown".to_string())
    }
    
    /// Get accounts by a list of addresses (for P2P sync)
    pub async fn get_accounts_by_addresses(&self, addresses: &[String]) -> Vec<crate::network::AccountData> {
        let state = self.inner.read().await;
        addresses.iter()
            .filter_map(|addr| {
                state.accounts.get(addr).map(|a| crate::network::AccountData {
                    address: a.address.clone(),
                    balance: a.balance,
                    nonce: a.nonce,
                    stake: a.staked,
                })
            })
            .collect()
    }
    
    /// Apply a checkpoint received from the network (via CheckpointAnnouncement)
    /// 
    /// SAFETY: We finalize transactions ONLY if our unfinalized set's merkle root
    /// matches the checkpoint's tx_merkle_root. This ensures we have the same
    /// transaction set as the leader before finalizing.
    /// 
    /// In production, this should verify:
    /// 1. Validator signatures meet quorum threshold
    /// 2. The checkpoint merkle roots match expected state
    /// For now in testnet mode, we trust the checkpoint if prev_hash links correctly.
    pub async fn apply_checkpoint(&self, checkpoint: rinku_core::types::Checkpoint, fast_path_executed: Option<&std::collections::HashSet<String>>) -> anyhow::Result<()> {
        use rinku_core::merkle::MerkleTree;
        
        let mut state = self.inner.write().await;
        
        // Validate this is the next expected checkpoint
        // CRITICAL: Use actual last checkpoint height, NOT len() which breaks after pruning
        let current_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
        let expected_height = current_height + 1;
        if checkpoint.height != expected_height {
            return Err(anyhow::anyhow!(
                "Checkpoint height mismatch: expected {}, got {} (current: {})",
                expected_height,
                checkpoint.height,
                current_height
            ));
        }
        
        // Validate prev_hash linkage (critical for chain integrity)
        if let Some(last_checkpoint) = state.checkpoints.last() {
            let expected_prev = &last_checkpoint.hash;
            let got_prev = checkpoint.previous_hash.as_deref().unwrap_or("");
            if got_prev != expected_prev {
                return Err(anyhow::anyhow!(
                    "Checkpoint prev_hash mismatch: expected {}, got {}",
                    &expected_prev[..16.min(expected_prev.len())],
                    &got_prev[..16.min(got_prev.len())]
                ));
            }
        }
        
        // Get our unfinalized transactions and compute merkle root
        // CRITICAL: Apply same propagation grace period as checkpoint creation
        // Only include transactions older than 5 seconds for merkle root calculation
        // This ensures leader and followers compute the same merkle root
        use crate::config::PROPAGATION_GRACE_MS;
        
        let now_ms_filter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let cutoff_time = now_ms_filter.saturating_sub(PROPAGATION_GRACE_MS);
        
        let mut unfinalized_hashes: Vec<String> = state
            .dag
            .get_unfinalized_nodes()
            .iter()
            .filter(|n| n.tx.tx.timestamp <= cutoff_time)
            .map(|n| n.hash.clone())
            .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
            .collect();
        
        // Sort for deterministic merkle root (must match leader's computation)
        unfinalized_hashes.sort();
        
        let our_merkle_root = if unfinalized_hashes.is_empty() {
            "0".repeat(64)
        } else {
            match MerkleTree::from_hex_leaves(&unfinalized_hashes) {
                Ok(tree) => tree.root(),
                Err(_) => "0".repeat(64),
            }
        };
        
        let height = checkpoint.height;
        let finalized_count;
        
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        
        // SAFE FINALIZATION: Only finalize if our transaction set matches the leader's
        // FINALITY-FIRST MODEL: Collect transactions for execution after marking finalized
        let mut txs_to_execute: Vec<SignedTransaction> = Vec::new();
        
        if our_merkle_root == checkpoint.tx_merkle_root && !unfinalized_hashes.is_empty() {
            // Our unfinalized set matches the checkpoint - safe to finalize
            for hash in &unfinalized_hashes {
                // Get transaction timestamp for finality time calculation
                if let Some(node) = state.dag.get_node(hash) {
                    // DOUBLE-EXECUTION GUARD: Skip already-finalized transactions
                    if node.finalized {
                        continue;
                    }
                    
                    let tx_timestamp = node.tx.tx.timestamp;
                    // FINALITY-FIRST: Clone transaction for execution (before mutable borrow)
                    let tx_clone = node.tx.clone();
                    
                    // Handle both seconds and milliseconds timestamp formats
                    let tx_time_ms = if tx_timestamp < 4_000_000_000 {
                        tx_timestamp * 1000  // Convert seconds to milliseconds
                    } else {
                        tx_timestamp
                    };
                    let finality_time_ms = now_ms.saturating_sub(tx_time_ms);
                    
                    // Cap finality time to 5 minutes (300s) to prevent sync/restore txs from polluting stats
                    const MAX_FINALITY_MS: u64 = 300_000;
                    if finality_time_ms <= MAX_FINALITY_MS {
                        state.finality_sum_ms += finality_time_ms;
                        state.finality_count += 1;
                        if finality_time_ms > state.finality_max_ms {
                            state.finality_max_ms = finality_time_ms;
                        }
                        if state.finality_times_ms.len() >= 1000 {
                            state.finality_times_ms.pop_front();
                        }
                        state.finality_times_ms.push_back(finality_time_ms);
                    }
                    // FINALITY-FIRST: Collect transaction for execution
                    txs_to_execute.push(tx_clone);
                }
                let _ = state.dag.mark_finalized(hash, height);
            }
            finalized_count = unfinalized_hashes.len();
            tracing::info!(
                "Applied checkpoint {} at height {} ({} txs finalized, merkle matched)",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                height,
                finalized_count
            );
        } else if our_merkle_root != checkpoint.tx_merkle_root && !unfinalized_hashes.is_empty() {
            // Merkle root mismatch - we have different transactions than the leader
            // Sync mechanism will reconcile the difference
            finalized_count = 0;
            tracing::info!(
                "Applied checkpoint {} at height {} (merkle mismatch: ours={} theirs={}, {} unfinalized)",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                height,
                &our_merkle_root[..16],
                &checkpoint.tx_merkle_root[..16.min(checkpoint.tx_merkle_root.len())],
                unfinalized_hashes.len()
            );
        } else {
            // No unfinalized transactions
            finalized_count = 0;
            tracing::info!(
                "Applied checkpoint {} at height {} (no unfinalized txs)",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                height
            );
        }
        
        // Add the checkpoint
        state.checkpoints.push(checkpoint.clone());
        state.last_checkpoint_time_ms = now_ms;
        
        // Release state lock before executing transactions
        drop(state);
        
        // FINALITY-FIRST MODEL: Execute finalized transactions (state changes happen here)
        // Skip core execution for transactions already executed on fast-path
        let empty_set = std::collections::HashSet::new();
        let fp_set = fast_path_executed.unwrap_or(&empty_set);
        for tx in &txs_to_execute {
            if fp_set.contains(&tx.hash) {
                tracing::debug!(
                    "apply_checkpoint: skipping core execution for fast-path-executed tx {}",
                    &tx.hash[..16.min(tx.hash.len())]
                );
                self.execute_finalized_transaction_rewards(tx).await;
            } else {
                self.execute_finalized_transaction(tx).await;
            }
        }
        
        Ok(())
    }

    /// Apply a checkpoint received with its finalized transaction hashes
    /// This is the preferred method when receiving CheckpointAnnouncement from the leader
    /// because it allows finalizing transactions even if merkle roots don't match
    /// 
    /// Returns the number of missing transactions that the leader finalized but we don't have.
    /// This is CRITICAL for proof storage decisions - proofs should ONLY be stored when
    /// missing_tx_count == 0, otherwise the proof values won't match local account state.
    pub async fn apply_checkpoint_with_finalized_hashes(
        &self, 
        checkpoint: Checkpoint,
        finalized_tx_hashes: Vec<String>,
        fast_path_executed: &std::collections::HashSet<String>,
    ) -> Result<usize> {
        let mut state = self.inner.write().await;
        
        // Validate checkpoint height
        // CRITICAL: Use actual checkpoint height, NOT len() which breaks after pruning
        let local_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
        if checkpoint.height <= local_height {
            return Err(anyhow::anyhow!(
                "Checkpoint height {} not greater than local height {}",
                checkpoint.height,
                local_height
            ));
        }
        
        let height = checkpoint.height;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        
        // FINALITY-FIRST MODEL: Collect transactions for execution after marking finalized
        let mut txs_to_execute: Vec<SignedTransaction> = Vec::new();
        
        // Track missing transactions - if we're missing any, our state differs from leader's
        let mut missing_tx_count = 0usize;
        
        // If leader provided finalized hashes, use them to finalize transactions
        // This solves the "merkle mismatch" problem where transactions stay pending
        let finalized_count = if !finalized_tx_hashes.is_empty() {
            let mut count = 0;
            let mut missing = 0;
            
            for hash in &finalized_tx_hashes {
                // Only finalize transactions we have in our DAG
                if let Some(node) = state.dag.get_node(hash) {
                    // DOUBLE-EXECUTION GUARD: Skip already-finalized transactions
                    if node.finalized {
                        continue;
                    }
                    
                    let tx_timestamp = node.tx.tx.timestamp;
                    // FINALITY-FIRST: Clone transaction for execution (before mutable borrow)
                    let tx_clone = node.tx.clone();
                    
                    // Handle both seconds and milliseconds timestamp formats
                    let tx_time_ms = if tx_timestamp < 4_000_000_000 {
                        tx_timestamp * 1000  // Convert seconds to milliseconds
                    } else {
                        tx_timestamp
                    };
                    let finality_time_ms = now_ms.saturating_sub(tx_time_ms);
                    
                    // Cap finality time to 5 minutes (300s) to prevent sync/restore txs from polluting stats
                    const MAX_FINALITY_MS: u64 = 300_000;
                    if finality_time_ms <= MAX_FINALITY_MS {
                        state.finality_sum_ms += finality_time_ms;
                        state.finality_count += 1;
                        if finality_time_ms > state.finality_max_ms {
                            state.finality_max_ms = finality_time_ms;
                        }
                        if state.finality_times_ms.len() >= 1000 {
                            state.finality_times_ms.pop_front();
                        }
                        state.finality_times_ms.push_back(finality_time_ms);
                    }
                    
                    // FINALITY-FIRST: Collect transaction for execution
                    txs_to_execute.push(tx_clone);
                    let _ = state.dag.mark_finalized(hash, height);
                    // Increment total_transactions at checkpoint finalization
                    state.total_transactions += 1;
                    count += 1;
                } else {
                    missing += 1;
                }
            }
            
            if missing > 0 {
                tracing::debug!(
                    "Checkpoint {} finalized {} txs, {} missing locally (will sync)",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    count, missing
                );
            }
            
            // Propagate missing count to function scope for proof generation decision
            missing_tx_count = missing;
            
            tracing::info!(
                "Applied checkpoint {} at height {} ({} of {} txs finalized from leader list)",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                height,
                count,
                finalized_tx_hashes.len()
            );
            
            count
        } else {
            // Fallback to old behavior if no hashes provided
            // (for backwards compatibility with older nodes)
            use crate::config::PROPAGATION_GRACE_MS;
            use rinku_core::merkle::MerkleTree;
            
            let now_ms_filter = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let cutoff_time = now_ms_filter.saturating_sub(PROPAGATION_GRACE_MS);
            
            let mut unfinalized_hashes: Vec<String> = state
                .dag
                .get_unfinalized_nodes()
                .iter()
                .filter(|n| n.tx.tx.timestamp <= cutoff_time)
                .map(|n| n.hash.clone())
                .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
                .collect();
            
            unfinalized_hashes.sort();
            
            let our_merkle_root = if unfinalized_hashes.is_empty() {
                "0".repeat(64)
            } else {
                match MerkleTree::from_hex_leaves(&unfinalized_hashes) {
                    Ok(tree) => tree.root(),
                    Err(_) => "0".repeat(64),
                }
            };
            
            if our_merkle_root == checkpoint.tx_merkle_root && !unfinalized_hashes.is_empty() {
                for hash in &unfinalized_hashes {
                    if let Some(node) = state.dag.get_node(hash) {
                        // DOUBLE-EXECUTION GUARD: Skip already-finalized transactions
                        if node.finalized {
                            continue;
                        }
                        
                        let tx_timestamp = node.tx.tx.timestamp;
                        // FINALITY-FIRST: Clone transaction for execution (before mutable borrow)
                        let tx_clone = node.tx.clone();
                        
                        // Handle both seconds and milliseconds timestamp formats
                        let tx_time_ms = if tx_timestamp < 4_000_000_000 {
                            tx_timestamp * 1000  // Convert seconds to milliseconds
                        } else {
                            tx_timestamp
                        };
                        let finality_time_ms = now_ms.saturating_sub(tx_time_ms);
                        
                        // Cap finality time to 5 minutes (300s) to prevent sync/restore txs from polluting stats
                        const MAX_FINALITY_MS: u64 = 300_000;
                        if finality_time_ms <= MAX_FINALITY_MS {
                            state.finality_sum_ms += finality_time_ms;
                            state.finality_count += 1;
                            if finality_time_ms > state.finality_max_ms {
                                state.finality_max_ms = finality_time_ms;
                            }
                            if state.finality_times_ms.len() >= 1000 {
                                state.finality_times_ms.pop_front();
                            }
                            state.finality_times_ms.push_back(finality_time_ms);
                        }
                        // FINALITY-FIRST: Collect transaction for execution
                        txs_to_execute.push(tx_clone);
                        // Increment total_transactions only upon finalization
                        state.total_transactions += 1;
                    }
                    let _ = state.dag.mark_finalized(hash, height);
                }
                tracing::info!(
                    "Applied checkpoint {} at height {} ({} txs finalized, merkle matched, no leader list)",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    height,
                    unfinalized_hashes.len()
                );
                unfinalized_hashes.len()
            } else {
                tracing::warn!(
                    "Applied checkpoint {} at height {} (no finalized txs - merkle mismatch and no leader list)",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    height
                );
                0
            }
        };
        
        // Add the checkpoint with finalized hashes
        let mut checkpoint_with_hashes = checkpoint.clone();
        if checkpoint_with_hashes.finalized_tx_hashes.is_empty() && !finalized_tx_hashes.is_empty() {
            checkpoint_with_hashes.finalized_tx_hashes = finalized_tx_hashes;
        }
        state.checkpoints.push(checkpoint_with_hashes);
        state.last_checkpoint_time_ms = now_ms;
        
        // Suppress unused variable warning
        let _ = finalized_count;
        
        // Release state lock before executing transactions
        drop(state);
        
        // FINALITY-FIRST MODEL: Execute finalized transactions (state changes happen here)
        // Skip core execution for transactions already executed on fast-path
        for tx in &txs_to_execute {
            if fast_path_executed.contains(&tx.hash) {
                tracing::debug!(
                    "Follower checkpoint: skipping core execution for fast-path-executed tx {}",
                    &tx.hash[..16.min(tx.hash.len())]
                );
                self.execute_finalized_transaction_rewards(tx).await;
            } else {
                self.execute_finalized_transaction(tx).await;
            }
        }
        
        // FOLLOWER NODES DO NOT GENERATE PROOFS
        // Only the checkpoint LEADER can generate valid proofs because only they have the
        // exact simulated account set used to compute state_root. Followers receive checkpoints
        // with state_root but their account set may differ due to:
        // - Transaction propagation timing differences
        // - Activity bot transactions creating accounts on some nodes before others
        // - Different ordering of parallel transactions
        // 
        // Proof generation on followers would use the wrong account set and produce invalid proofs.
        // Users should query the leader node or wait for proofs to propagate via sync.
        if missing_tx_count > 0 {
            tracing::debug!(
                "Follower checkpoint {} - missing {} txs (proofs not generated, query leader)",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                missing_tx_count
            );
        } else {
            tracing::debug!(
                "Follower checkpoint {} applied successfully (proofs not generated, query leader)",
                &checkpoint.hash[..16.min(checkpoint.hash.len())]
            );
        }
        
        // Return missing_tx_count so caller can decide whether to store precomputed proofs
        // Proofs should ONLY be stored if missing_tx_count == 0, otherwise the proof values
        // (computed from leader's tx set) won't match local account state
        Ok(missing_tx_count)
    }

    pub async fn get_latest_checkpoint_id(&self) -> Option<String> {
        let state = self.inner.read().await;
        state.checkpoints.last().map(|c| c.hash.chars().take(16).collect())
    }

    pub async fn get_gas_price(&self) -> f64 {
        let state = self.inner.read().await;
        state.current_gas_price
    }

    pub async fn get_gas_stats(&self) -> (f64, f64, f64, f64) {
        let state = self.inner.read().await;
        (
            state.current_gas_price,
            state.total_burned,
            state.total_to_validators,
            state.current_gas_price,
        )
    }

    /// Get emission stats (total_emitted, total_burned) from the emission service
    pub async fn get_emission_stats(&self) -> (f64, f64) {
        let emission = self.emission.read().await;
        (emission.get_total_emitted(), emission.get_total_burned())
    }

    pub async fn get_total_supply(&self) -> f64 {
        let state = self.inner.read().await;
        state.total_supply
    }

    pub async fn get_validator_count(&self) -> usize {
        let state = self.inner.read().await;
        state.validators.len()
    }

    pub async fn get_total_stake(&self) -> f64 {
        let state = self.inner.read().await;
        state.validators.values().map(|v| v.stake).sum()
    }

    pub async fn get_faucet_balance(&self) -> f64 {
        let state = self.inner.read().await;
        state.accounts.get("faucet").map(|a| a.balance).unwrap_or(0.0)
    }

    /// Get staking info for a specific validator address (for TUI display)
    pub async fn get_validator_staking_info(&self, address: &str) -> (f64, f64, f64, bool) {
        let rewards = self.rewards.read().await;
        let stake_amount = rewards.get_stake(address).map(|p| p.amount).unwrap_or(0.0);
        let pending_rewards = rewards.get_pending_rewards(address);
        
        let state = self.inner.read().await;
        let is_validator = state.validators.contains_key(address);
        
        // Unbonding amount - check if in unbonding queue
        let unbonding = 0.0; // TODO: Track unbonding separately if needed
        
        (stake_amount, pending_rewards, unbonding, is_validator)
    }

    /// Get staking configuration for display (min stake, unbonding period)
    pub async fn get_staking_config(&self) -> (f64, u32) {
        let rewards = self.rewards.read().await;
        let min_stake = rewards.get_config().min_stake_amount;
        let unbonding_days = (crate::slashing::UNBONDING_PERIOD_MS / (24 * 60 * 60 * 1000)) as u32;
        (min_stake, unbonding_days)
    }

    pub async fn get_total_transactions(&self) -> u64 {
        let state = self.inner.read().await;
        state.total_transactions
    }

    pub fn get_elapsed_seconds(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    /// Record finalized transaction count at current timestamp for TPS calculation
    pub async fn record_finalized_batch(&self, tx_count: u64) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        
        let mut state = self.inner.write().await;
        state.finalized_tx_history.push_back((now_ms, tx_count));
        
        // Keep only last 5 minutes of history (300 seconds)
        const WINDOW_MS: u64 = 300_000;
        let cutoff = now_ms.saturating_sub(WINDOW_MS);
        while let Some(&(ts, _)) = state.finalized_tx_history.front() {
            if ts < cutoff {
                state.finalized_tx_history.pop_front();
            } else {
                break;
            }
        }
    }

    /// Calculate network TPS based on finalized transactions over a sliding window
    pub async fn get_finalized_tps(&self) -> f64 {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        
        let state = self.inner.read().await;
        
        if state.finalized_tx_history.is_empty() {
            return 0.0;
        }
        
        // Calculate TPS over the last 60 seconds
        const TPS_WINDOW_MS: u64 = 60_000;
        let cutoff = now_ms.saturating_sub(TPS_WINDOW_MS);
        
        let mut total_txs: u64 = 0;
        let mut earliest_ts = now_ms;
        
        for &(ts, count) in state.finalized_tx_history.iter() {
            if ts >= cutoff {
                total_txs += count;
                if ts < earliest_ts {
                    earliest_ts = ts;
                }
            }
        }
        
        let elapsed_ms = now_ms.saturating_sub(earliest_ts);
        if elapsed_ms > 0 && total_txs > 0 {
            (total_txs as f64) / (elapsed_ms as f64 / 1000.0)
        } else {
            0.0
        }
    }

    pub async fn get_all_accounts(&self) -> Vec<Account> {
        let state = self.inner.read().await;
        state.accounts.values().cloned().collect()
    }

    /// Get pending (unfinalized) transaction stats for a sender
    /// Returns (pending_outgoing_amount, pending_gas, pending_tx_count)
    /// Used for finality-first validation: effective_balance = confirmed - pending_outgoing - pending_gas
    fn get_pending_stats_for_sender(state: &StateInner, sender: &str) -> (f64, f64, u64) {
        let mut pending_amount = 0.0;
        let mut pending_gas = 0.0;
        let mut pending_count = 0u64;
        
        for node in state.dag.get_all_nodes() {
            if !node.finalized && node.tx.tx.from == sender {
                let gas = node.tx.tx.gas_price.unwrap_or(state.current_gas_price);
                pending_gas += gas;
                
                // Only count amount for transfers and stakes (not unstake/claim)
                let is_unstake = matches!(node.tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
                let is_claim = matches!(node.tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
                if !is_unstake && !is_claim {
                    pending_amount += node.tx.tx.amount;
                }
                pending_count += 1;
            }
        }
        
        (pending_amount, pending_gas, pending_count)
    }
    
    /// Get the expected nonce for a sender, accounting for pending (unfinalized) transactions
    /// effective_nonce = confirmed_nonce + pending_tx_count
    fn get_effective_nonce(state: &StateInner, sender: &str) -> u64 {
        let confirmed_nonce = state.accounts.get(sender).map(|a| a.nonce).unwrap_or(0);
        let (_, _, pending_count) = Self::get_pending_stats_for_sender(state, sender);
        confirmed_nonce + pending_count
    }
    
    /// Get effective balance for a sender, accounting for pending (unfinalized) transactions
    /// effective_balance = confirmed_balance - pending_outgoing - pending_gas
    fn get_effective_balance(state: &StateInner, sender: &str) -> f64 {
        let confirmed_balance = state.accounts.get(sender).map(|a| a.balance).unwrap_or(0.0);
        let (pending_amount, pending_gas, _) = Self::get_pending_stats_for_sender(state, sender);
        confirmed_balance - pending_amount - pending_gas
    }

    /// Compute state root from all account states
    /// This creates a deterministic merkle root from sorted account data
    /// Uses canonical format matching sync_verification: "account:address:balance:nonce:stake"
    /// Internal nodes: "node:left_hash:right_hash"
    pub async fn compute_state_root(&self) -> String {
        let state = self.inner.read().await;
        
        // Get sorted accounts for deterministic ordering (same as sync_verification)
        let mut account_entries: Vec<_> = state.accounts.iter().collect();
        account_entries.sort_by(|a, b| a.0.cmp(b.0));
        
        // Create leaf hashes using canonical format (matches sync_verification::hash_account_leaf)
        let leaves: Vec<String> = account_entries
            .iter()
            .map(|(address, account)| {
                Self::hash_account_leaf_for_proof(address, account.balance, account.nonce, account.staked)
            })
            .collect();
        
        if leaves.is_empty() {
            return "0".repeat(64);
        }
        
        if leaves.len() == 1 {
            return leaves[0].clone();
        }
        
        // Build merkle tree using canonical internal node format (matches sync_verification::hash_internal)
        let mut current_level = leaves;
        while current_level.len() > 1 {
            let mut next_level = Vec::new();
            for chunk in current_level.chunks(2) {
                let left = &chunk[0];
                let right = if chunk.len() > 1 { &chunk[1] } else { &chunk[0] };
                next_level.push(Self::hash_internal_for_proof(left, right));
            }
            current_level = next_level;
        }
        
        current_level[0].clone()
    }
    
    /// Compute state root with pending transactions applied (without modifying actual state)
    /// This is used by checkpoint creation to get the correct post-execution state root
    /// before actually executing the transactions
    pub async fn compute_state_root_with_pending_txs(&self, pending_txs: &[rinku_core::SignedTransaction], skip_hashes: &std::collections::HashSet<String>) -> String {
        self.compute_state_root_and_proofs(pending_txs, &[], None, "", skip_hashes).await.state_root
    }
    
    /// Compute state root AND precomputed proofs for affected addresses
    /// CRITICAL: Proofs must be computed from the same simulated account set used for state_root
    /// to ensure merkle proof verification will succeed
    pub async fn compute_state_root_and_proofs(
        &self,
        pending_txs: &[rinku_core::SignedTransaction],
        affected_addresses: &[String],
        checkpoint_template: Option<&rinku_core::types::Checkpoint>,
        tx_hash: &str,
        skip_hashes: &std::collections::HashSet<String>,
    ) -> StateRootWithProofs {
        use std::collections::HashMap;
        
        let state = self.inner.read().await;
        
        // Get the current gas price from state (used as fallback for tx without explicit gas_price)
        // CRITICAL: Must match execute_finalized_transaction which uses state.current_gas_price
        let current_gas_price = state.current_gas_price;
        
        // Clone accounts into a mutable HashMap for simulation
        let mut simulated_accounts: HashMap<String, (f64, u64, f64)> = state.accounts.iter()
            .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
            .collect();
        
        drop(state); // Release state lock before acquiring rewards lock
        
        // Get pending rewards and stake amounts snapshot for claim/unstake simulation
        // CRITICAL: Must use rewards service as source of truth to match execute_finalized_transaction
        let rewards = self.rewards.read().await;
        let pending_rewards_snapshot: HashMap<String, f64> = pending_txs.iter()
            .filter(|tx| matches!(tx.tx.kind, Some(rinku_core::TransactionKind::ClaimRewards)))
            .map(|tx| (tx.tx.from.clone(), rewards.get_pending_rewards(&tx.tx.from)))
            .collect();
        // Get stake amounts from rewards service for unstake simulation
        let stake_amounts_snapshot: HashMap<String, f64> = pending_txs.iter()
            .filter(|tx| matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Unstake)))
            .filter_map(|tx| {
                rewards.get_stake(&tx.tx.from).map(|p| (tx.tx.from.clone(), p.amount))
            })
            .collect();
        
        // Build simulated_reward_state for v3 proofs
        // Structure: (pending_rewards, staked_at, last_reward_at, claimed_rewards_total)
        let mut simulated_reward_state: HashMap<String, (f64, u64, Option<u64>, f64)> = HashMap::new();
        
        // Collect reward state for all affected addresses
        for address in affected_addresses {
            let pending = rewards.get_pending_rewards(address);
            let stake_info = rewards.get_stake(address);
            let (staked_at, last_reward_at) = stake_info
                .map(|p| (p.staked_at, p.last_reward_at))
                .unwrap_or((0, None));
            let claimed_total = rewards.get_claimed_total(address);
            simulated_reward_state.insert(
                address.clone(),
                (pending, staked_at, last_reward_at, claimed_total)
            );
        }
        drop(rewards);
        
        // Apply pending transactions to simulated state
        // This must match execute_finalized_transaction exactly!
        // CRITICAL: Skip transactions already executed on fast-path, since their
        // effects are already reflected in the current state we cloned above
        for tx in pending_txs {
            if skip_hashes.contains(&tx.hash) {
                continue;
            }
            let from = &tx.tx.from;
            let to = &tx.tx.to;
            let amount = tx.tx.amount;
            // CRITICAL: Use the same gas price fallback as execute_finalized_transaction
            let fee = tx.tx.gas_price.unwrap_or(current_gas_price);
            
            let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Stake));
            let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Unstake));
            let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::TransactionKind::ClaimRewards));
            let is_contract_tx = matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Contract));
            
            // Deduct from sender based on transaction type (matches execute_finalized_transaction)
            if let Some(sender) = simulated_accounts.get_mut(from) {
                if is_stake_tx {
                    // Stake: deduct amount + fee (amount goes to stake, not recipient)
                    sender.0 = (sender.0 - amount - fee).max(0.0);
                } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                    // Unstake/Claim/Contract: only deduct gas fee (contract execution fee charged separately)
                    sender.0 = (sender.0 - fee).max(0.0);
                } else {
                    // Regular transfer: deduct amount + fee
                    sender.0 = (sender.0 - amount - fee).max(0.0);
                }
                sender.1 += 1; // Increment nonce
            }
            
            // Credit recipient only for regular transfers (not stake/unstake/claim/contract)
            if !is_stake_tx && !is_unstake_tx && !is_claim_tx && !is_contract_tx {
                if let Some(receiver) = simulated_accounts.get_mut(to) {
                    receiver.0 += amount;
                } else {
                    // Create new account for receiver
                    simulated_accounts.insert(to.clone(), (amount, 0, 0.0));
                }
            }
            
            // Handle staking state changes
            if is_stake_tx {
                if let Some(staker) = simulated_accounts.get_mut(from) {
                    staker.2 += amount; // Increase stake
                }
                // Update simulated_reward_state with staked_at timestamp
                if let Some(reward_state) = simulated_reward_state.get_mut(from) {
                    if reward_state.1 == 0 { // staked_at was 0, set it now
                        reward_state.1 = tx.tx.timestamp;
                    }
                } else {
                    // Create new reward state entry for new stakers
                    simulated_reward_state.insert(from.clone(), (0.0, tx.tx.timestamp, None, 0.0));
                }
            } else if is_unstake_tx {
                // CRITICAL: Use rewards service stake amount (not account.staked) to match execution
                // execute_finalized_transaction uses rewards.unstake() which returns the rewards service value
                if let Some(rewards_stake) = stake_amounts_snapshot.get(from) {
                    if let Some(staker) = simulated_accounts.get_mut(from) {
                        staker.0 += rewards_stake; // Return stake to balance (from rewards service)
                        staker.2 = 0.0; // Clear stake
                    }
                } else {
                    // Fallback to account.staked if not in snapshot (shouldn't happen for unstake txs)
                    if let Some(staker) = simulated_accounts.get_mut(from) {
                        let unstaked = staker.2;
                        staker.0 += unstaked;
                        staker.2 = 0.0;
                    }
                }
            } else if is_claim_tx {
                // Claim adds pending rewards to balance (matches execute_finalized_transaction)
                if let Some(claimed) = pending_rewards_snapshot.get(from) {
                    if *claimed > 0.0 {
                        if let Some(claimer) = simulated_accounts.get_mut(from) {
                            let old_balance = claimer.0;
                            claimer.0 += claimed; // Add claimed rewards to balance
                            tracing::info!(
                                "[SIMULATION] Claim for {}: pending_rewards={:.8}, old_balance={:.8}, new_balance={:.8}",
                                &from[..16.min(from.len())],
                                claimed,
                                old_balance,
                                claimer.0
                            );
                            
                            // Update simulated_reward_state after claim
                            if let Some(reward_state) = simulated_reward_state.get_mut(from) {
                                reward_state.0 = 0.0; // pending_rewards = 0 after claim
                                reward_state.3 += claimed; // claimed_total += claimed amount
                            }
                        }
                    } else {
                        tracing::warn!(
                            "[SIMULATION] Claim for {}: pending_rewards is 0!",
                            &from[..16.min(from.len())]
                        );
                    }
                } else {
                    tracing::warn!(
                        "[SIMULATION] Claim for {}: NO pending_rewards in snapshot!",
                        &from[..16.min(from.len())]
                    );
                }
            }
        }
        
        // Get sorted accounts for deterministic ordering
        let mut account_entries: Vec<_> = simulated_accounts.iter().collect();
        account_entries.sort_by(|a, b| a.0.cmp(b.0));
        
        // Log simulated state for debugging proof generation issues
        for (addr, (balance, nonce, staked)) in account_entries.iter().take(5) {
            tracing::debug!(
                "Simulated state for {}: balance={:.8}, nonce={}, staked={:.8}",
                &addr[..16.min(addr.len())],
                balance,
                nonce,
                staked
            );
        }
        
        // Create leaf hashes using canonical format
        let leaves: Vec<String> = account_entries
            .iter()
            .map(|(address, (balance, nonce, staked))| {
                Self::hash_account_leaf_for_proof(address, *balance, *nonce, *staked)
            })
            .collect();
        
        if leaves.is_empty() {
            return StateRootWithProofs {
                state_root: "0".repeat(64),
                proofs: HashMap::new(),
            };
        }
        
        let state_root = if leaves.len() == 1 {
            leaves[0].clone()
        } else {
            // Build merkle tree using canonical internal node format
            let mut current_level = leaves.clone();
            while current_level.len() > 1 {
                let mut next_level = Vec::new();
                for chunk in current_level.chunks(2) {
                    let left = &chunk[0];
                    let right = if chunk.len() > 1 { &chunk[1] } else { &chunk[0] };
                    next_level.push(Self::hash_internal_for_proof(left, right));
                }
                current_level = next_level;
            }
            current_level[0].clone()
        };
        
        // Generate proofs for affected addresses using the SAME simulated account set
        // This is CRITICAL: proofs must be computed from identical data as state_root
        let mut proofs: HashMap<String, rinku_core::types::AccountStateProof> = HashMap::new();
        
        if let Some(checkpoint) = checkpoint_template {
            for address in affected_addresses {
                // Find the account in simulated_accounts (sorted by address)
                if let Some(idx) = account_entries.iter().position(|(addr, _)| *addr == address) {
                    let (_, (balance, nonce, staked)) = &account_entries[idx];
                    
                    // Compute merkle proof path from the simulated leaves
                    let merkle_proof = Self::compute_merkle_proof_path_canonical(&leaves, idx);
                    
                    tracing::info!(
                        "Generating proof for {}: balance={:.8}, nonce={}, staked={:.8} (checkpoint {}, state_root={})",
                        &address[..16.min(address.len())],
                        balance,
                        nonce,
                        staked,
                        checkpoint.height,
                        &state_root[..16.min(state_root.len())]
                    );
                    
                    // Get reward state from simulated_reward_state if available
                    let (pending_rewards, staked_at, last_reward_at, claimed_total) = 
                        simulated_reward_state.get(address)
                            .cloned()
                            .unwrap_or((0.0, 0, None, 0.0));
                    
                    let proof = rinku_core::types::AccountStateProof {
                        version: 3, // v3 includes reward state
                        address: address.clone(),
                        balance_micro: Self::to_micro_units(*balance),
                        balance: *balance,
                        nonce: *nonce,
                        staked_micro: Self::to_micro_units(*staked),
                        staked: *staked,
                        pending_rewards_micro: Self::to_micro_units(pending_rewards),
                        pending_rewards,
                        staked_at,
                        last_reward_at,
                        claimed_rewards_total_micro: Self::to_micro_units(claimed_total),
                        claimed_rewards_total: claimed_total,
                        checkpoint_height: checkpoint.height,
                        checkpoint_hash: checkpoint.hash.clone(),
                        checkpoint_timestamp: checkpoint.timestamp,
                        state_root: state_root.clone(),
                        merkle_proof,
                        merkle_index: idx,
                        is_on_demand: false,
                        bls_aggregated_sig: checkpoint.aggregated_signature.clone(),
                        bls_signer_bitmap: checkpoint.signer_bitmap.as_ref().map(|b| hex::encode(b)),
                        tx_hash: tx_hash.to_string(),
                    };
                    
                    proofs.insert(address.clone(), proof);
                }
            }
        }
        
        StateRootWithProofs { state_root, proofs }
    }

    /// Normalize f64 to 8 decimal places for consistent hashing (matches sync_verification)
    /// Convert f64 balance to u64 micro-units (1 RKU = 100,000,000 micro-RKU)
    fn to_micro_units(value: f64) -> u64 {
        rinku_core::types::to_micro_units(value)
    }
    
    /// Hash data using SHA256 and return hex string (matches sync_verification)
    fn sha256_hex_for_proof(data: &str) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(data.as_bytes());
        hex::encode(hasher.finalize())
    }
    
    /// Hash an account leaf using u64 micro-units for deterministic cross-language verification
    /// 
    /// Canonical format: "account:{address}:{balance_micro}:{nonce}:{staked_micro}"
    /// Where balance_micro and staked_micro are u64 values (1 RKU = 100,000,000 micro-RKU)
    fn hash_account_leaf_for_proof(addr: &str, balance: f64, nonce: u64, stake: f64) -> String {
        let balance_micro = Self::to_micro_units(balance);
        let staked_micro = Self::to_micro_units(stake);
        let data = format!(
            "account:{}:{}:{}:{}",
            addr,
            balance_micro,
            nonce,
            staked_micro
        );
        Self::sha256_hex_for_proof(&data)
    }
    
    /// Hash internal merkle node (matches sync_verification::hash_internal format)
    fn hash_internal_for_proof(left: &str, right: &str) -> String {
        let data = format!("node:{}:{}", left, right);
        Self::sha256_hex_for_proof(&data)
    }
    
    /// Generate a self-contained proof for an account's current state
    /// Returns the proof along with the merkle path for verification
    /// Uses the same leaf/node format as sync_verification for consistency
    pub async fn generate_account_state_proof(
        &self,
        address: &str,
        checkpoint: &rinku_core::types::Checkpoint,
        tx_hash: &str,
    ) -> Option<rinku_core::types::AccountStateProof> {
        let state = self.inner.read().await;
        
        // Get account data
        let account = state.accounts.get(address)?;
        
        tracing::info!(
            "Generating proof for {}: balance={:.8}, nonce={}, staked={:.8} (checkpoint {}, state_root={})",
            &address[..16.min(address.len())],
            account.balance,
            account.nonce,
            account.staked,
            checkpoint.height,
            &checkpoint.state_root[..16.min(checkpoint.state_root.len())]
        );
        
        // Get sorted accounts for deterministic ordering (same as sync_verification)
        let mut account_entries: Vec<_> = state.accounts.iter().collect();
        account_entries.sort_by(|a, b| a.0.cmp(b.0));
        
        // Find the index of the target account
        let merkle_index = account_entries
            .iter()
            .position(|(addr, _)| *addr == address)?;
        
        // Create leaf hashes using canonical format (matches sync_verification::hash_account_leaf)
        let leaves: Vec<String> = account_entries
            .iter()
            .map(|(addr, acc)| {
                Self::hash_account_leaf_for_proof(addr, acc.balance, acc.nonce, acc.staked)
            })
            .collect();
        
        if leaves.is_empty() {
            return None;
        }
        
        // Build merkle tree and collect proof path
        let merkle_proof = Self::compute_merkle_proof_path_canonical(&leaves, merkle_index);
        
        // Get reward state from existing proof if available
        let (pending_rewards, staked_at, last_reward_at, claimed_total) = 
            account.latest_balance_proof.as_ref()
                .map(|p| (p.pending_rewards, p.staked_at, p.last_reward_at, p.claimed_rewards_total))
                .unwrap_or((0.0, 0, None, 0.0));
        
        Some(rinku_core::types::AccountStateProof {
            version: 3, // v3 includes reward state
            address: address.to_string(),
            balance_micro: Self::to_micro_units(account.balance),
            balance: account.balance,
            nonce: account.nonce,
            staked_micro: Self::to_micro_units(account.staked),
            staked: account.staked,
            pending_rewards_micro: Self::to_micro_units(pending_rewards),
            pending_rewards,
            staked_at,
            last_reward_at,
            claimed_rewards_total_micro: Self::to_micro_units(claimed_total),
            claimed_rewards_total: claimed_total,
            checkpoint_height: checkpoint.height,
            checkpoint_hash: checkpoint.hash.clone(),
            checkpoint_timestamp: checkpoint.timestamp,
            state_root: checkpoint.state_root.clone(),
            merkle_proof,
            merkle_index,
            bls_aggregated_sig: checkpoint.aggregated_signature.clone(),
            bls_signer_bitmap: checkpoint.signer_bitmap.as_ref().map(|b| hex::encode(b)),
            tx_hash: tx_hash.to_string(),
            is_on_demand: false,
        })
    }
    
    /// Generate a fresh proof for an account at the latest checkpoint
    /// This is used when users request a proof via the explorer, regardless of recent activity
    /// The proof uses the checkpoint's actual BLS-signed state_root
    pub async fn generate_account_state_proof_on_demand(
        &self,
        address: &str,
    ) -> Option<rinku_core::types::AccountStateProof> {
        // Single read lock to ensure atomicity of checkpoint and account data
        let state = self.inner.read().await;
        
        // Get the latest checkpoint - this contains the BLS-signed state_root
        let checkpoint = state.checkpoints.last()?.clone();
        
        // Get account data (unchanged since last checkpoint finalization)
        let account = state.accounts.get(address)?.clone();
        
        // Get sorted accounts for deterministic ordering (same order used during checkpoint)
        let mut account_entries: Vec<_> = state.accounts.iter().collect();
        account_entries.sort_by(|a, b| a.0.cmp(b.0));
        
        // Find the index of the target account
        let merkle_index = account_entries
            .iter()
            .position(|(addr, _)| *addr == address)?;
        
        // Create leaf hashes using canonical format
        let leaves: Vec<String> = account_entries
            .iter()
            .map(|(addr, acc)| {
                Self::hash_account_leaf_for_proof(addr, acc.balance, acc.nonce, acc.staked)
            })
            .collect();
        
        if leaves.is_empty() {
            return None;
        }
        
        // Build merkle proof path against the current account set
        // Since account state only changes at checkpoint finalization,
        // this proof should verify against the checkpoint's state_root
        let merkle_proof = Self::compute_merkle_proof_path_canonical(&leaves, merkle_index);
        
        tracing::info!(
            "Generated proof for {} at checkpoint {}: balance={:.8}, nonce={}, staked={:.8}",
            &address[..16.min(address.len())],
            checkpoint.height,
            account.balance,
            account.nonce,
            account.staked
        );
        
        // Get reward state from existing proof if available
        let (pending_rewards, staked_at, last_reward_at, claimed_total) = 
            account.latest_balance_proof.as_ref()
                .map(|p| (p.pending_rewards, p.staked_at, p.last_reward_at, p.claimed_rewards_total))
                .unwrap_or((0.0, 0, None, 0.0));
        
        Some(rinku_core::types::AccountStateProof {
            version: 3, // v3 includes reward state
            address: address.to_string(),
            balance_micro: Self::to_micro_units(account.balance),
            balance: account.balance,
            nonce: account.nonce,
            staked_micro: Self::to_micro_units(account.staked),
            staked: account.staked,
            pending_rewards_micro: Self::to_micro_units(pending_rewards),
            pending_rewards,
            staked_at,
            last_reward_at,
            claimed_rewards_total_micro: Self::to_micro_units(claimed_total),
            claimed_rewards_total: claimed_total,
            checkpoint_height: checkpoint.height,
            checkpoint_hash: checkpoint.hash.clone(),
            checkpoint_timestamp: checkpoint.timestamp,
            state_root: checkpoint.state_root.clone(), // Use checkpoint's BLS-signed state_root
            merkle_proof,
            merkle_index,
            bls_aggregated_sig: checkpoint.aggregated_signature.clone(),
            bls_signer_bitmap: checkpoint.signer_bitmap.as_ref().map(|b| hex::encode(b)),
            tx_hash: "on-demand".to_string(), // No specific tx, generated on-demand
            is_on_demand: false, // Uses checkpoint's actual BLS-signed state_root
        })
    }
    
    /// Compute merkle proof path for a leaf at given index
    /// Uses canonical format matching sync_verification (hash_internal)
    fn compute_merkle_proof_path_canonical(leaves: &[String], target_index: usize) -> Vec<String> {
        if leaves.is_empty() || leaves.len() == 1 {
            return vec![];
        }
        
        let mut proof = Vec::new();
        let mut current_level: Vec<String> = leaves.to_vec();
        let mut current_index = target_index;
        
        while current_level.len() > 1 {
            // Get sibling
            let sibling_index = if current_index % 2 == 0 {
                current_index + 1
            } else {
                current_index - 1
            };
            
            if sibling_index < current_level.len() {
                proof.push(current_level[sibling_index].clone());
            } else {
                // Odd number of nodes, duplicate the last one
                proof.push(current_level[current_index].clone());
            }
            
            // Build next level using canonical hash_internal format
            let mut next_level = Vec::new();
            for chunk in current_level.chunks(2) {
                let left = &chunk[0];
                let right = if chunk.len() > 1 { &chunk[1] } else { &chunk[0] };
                next_level.push(Self::hash_internal_for_proof(left, right));
            }
            
            current_level = next_level;
            current_index /= 2;
        }
        
        proof
    }
    
    /// Update balance proofs for accounts affected by finalized transactions
    /// IMPORTANT: This must be called AFTER execute_finalized_transaction has completed
    /// for all transactions, so that state.accounts contains the post-execution values
    /// that match what was simulated in compute_state_root_with_pending_txs
    pub async fn update_account_balance_proofs(
        &self,
        addresses: &[String],
        checkpoint: &rinku_core::types::Checkpoint,
        tx_hash: &str,
    ) {
        for address in addresses {
            if let Some(proof) = self.generate_account_state_proof(address, checkpoint, tx_hash).await {
                let mut state = self.inner.write().await;
                if let Some(account) = state.accounts.get_mut(address) {
                    // Log before updating to help debug proof issues
                    tracing::info!(
                        "Updating balance proof for {} at checkpoint {}: balance={:.4}, nonce={}, staked={:.4}",
                        &address[..16.min(address.len())],
                        checkpoint.height,
                        proof.balance,
                        proof.nonce,
                        proof.staked
                    );
                    account.latest_balance_proof = Some(proof);
                }
            } else {
                tracing::warn!(
                    "Failed to generate balance proof for {} at checkpoint {}",
                    &address[..16.min(address.len())],
                    checkpoint.height
                );
            }
        }
    }
    
    /// Store precomputed proofs from checkpoint simulation
    /// CRITICAL: These proofs were computed from the same simulated account set used for state_root,
    /// guaranteeing that merkle proof verification will succeed against the checkpoint's state_root.
    /// This is used by the checkpoint LEADER to store proofs computed before transaction execution.
    /// 
    /// CONSENSUS FIX: For followers, this also SYNCHRONIZES local account state to match the leader's
    /// authoritative values. This is essential because non-deterministic operations (like ClaimRewards
    /// where pending_rewards can vary based on timing) could cause balance divergence if followers
    /// only execute locally without syncing to leader's computed state.
    pub async fn store_precomputed_proofs(
        &self,
        proofs: &std::collections::HashMap<String, rinku_core::types::AccountStateProof>,
    ) {
        // First pass: sync account state and collect addresses needing RewardsService sync
        // For v3 proofs, we now have authoritative reward state: (address, pending_rewards, staked_at, last_reward_at, claimed_total, staked_amount)
        let mut rewards_to_sync: Vec<(String, f64, u64, Option<u64>, f64, f64)> = Vec::new();
        
        {
            let mut state = self.inner.write().await;
            for (address, proof) in proofs {
                let is_v3_proof = proof.version >= 3;
                
                if let Some(account) = state.accounts.get_mut(address) {
                    // CONSENSUS FIX: Detect and fix balance divergence from leader's authoritative state
                    // This can happen when reward calculations differ between leader and follower
                    let balance_diff = (account.balance - proof.balance).abs();
                    let staked_diff = (account.staked - proof.staked).abs();
                    
                    if balance_diff > 0.000001 || staked_diff > 0.000001 || account.nonce != proof.nonce {
                        tracing::warn!(
                            "STATE SYNC for {} at checkpoint {}: local(bal={:.4}, nonce={}, stk={:.4}) -> leader(bal={:.4}, nonce={}, stk={:.4})",
                            &address[..16.min(address.len())],
                            proof.checkpoint_height,
                            account.balance, account.nonce, account.staked,
                            proof.balance, proof.nonce, proof.staked
                        );
                        // Synchronize to leader's authoritative state
                        account.balance = proof.balance;
                        account.nonce = proof.nonce;
                        account.staked = proof.staked;
                        // Mark for RewardsService sync with authoritative v3 values
                        if is_v3_proof {
                            rewards_to_sync.push((
                                address.clone(),
                                proof.pending_rewards,
                                proof.staked_at,
                                proof.last_reward_at,
                                proof.claimed_rewards_total,
                                proof.staked
                            ));
                        }
                    } else {
                        tracing::info!(
                            "Storing precomputed proof for {} at checkpoint {}: balance={:.4}, nonce={}, staked={:.4}",
                            &address[..16.min(address.len())],
                            proof.checkpoint_height,
                            proof.balance,
                            proof.nonce,
                            proof.staked
                        );
                    }
                    account.latest_balance_proof = Some(proof.clone());
                } else {
                    // Account doesn't exist locally - create it from leader's proof
                    tracing::info!(
                        "Creating account {} from leader proof at checkpoint {}: balance={:.4}, nonce={}, staked={:.4}",
                        &address[..16.min(address.len())],
                        proof.checkpoint_height,
                        proof.balance,
                        proof.nonce,
                        proof.staked
                    );
                    let mut new_account = Account::new(address.clone(), proof.checkpoint_height as u64);
                    new_account.balance = proof.balance;
                    new_account.nonce = proof.nonce;
                    new_account.staked = proof.staked;
                    new_account.latest_balance_proof = Some(proof.clone());
                    state.accounts.insert(address.clone(), new_account);
                    // Also mark for RewardsService sync with v3 values (new account)
                    if proof.version >= 3 && proof.staked > 0.0 {
                        rewards_to_sync.push((
                            address.clone(),
                            proof.pending_rewards,
                            proof.staked_at,
                            proof.last_reward_at,
                            proof.claimed_rewards_total,
                            proof.staked
                        ));
                    }
                }
            }
        } // Release state lock
        
        // Second pass: sync RewardsService for accounts with divergence using authoritative v3 values
        if !rewards_to_sync.is_empty() {
            let mut rewards = self.rewards.write().await;
            for (address, pending_rewards, staked_at, last_reward_at, claimed_total, staked_amount) in rewards_to_sync {
                rewards.sync_from_leader_v3(&address, pending_rewards, staked_at, last_reward_at, claimed_total, staked_amount);
            }
        }
    }

    pub async fn get_all_dag_nodes(&self) -> Vec<DagNodeInfo> {
        let state = self.inner.read().await;
        state
            .dag
            .get_all_nodes()
            .into_iter()
            .map(|n| DagNodeInfo {
                hash: n.hash.clone(),
                from: n.tx.tx.from.clone(),
                to: n.tx.tx.to.clone(),
                amount: n.tx.tx.amount,
                fee: n.tx.tx.gas_price.unwrap_or(0.001),
                nonce: n.tx.tx.nonce,
                ts: n.tx.tx.timestamp,
                parents: n.parents.clone(),
                finalized: n.finalized,
                weight: n.weight,
                kind: n.tx.tx.kind,
                sig: n.tx.signature.clone(),
            })
            .collect()
    }

    /// Get paginated DAG nodes - sorted by timestamp desc, with limit
    /// Much more efficient than fetching all nodes for large DAGs
    pub async fn get_dag_nodes_paginated(&self, page: usize, limit: usize) -> (Vec<DagNodeInfo>, usize, bool) {
        let state = self.inner.read().await;
        let all_nodes = state.dag.get_all_nodes();
        let total = all_nodes.len();
        
        // Sort by timestamp descending and paginate
        let mut sorted: Vec<_> = all_nodes.into_iter().collect();
        sorted.sort_by(|a, b| b.tx.tx.timestamp.cmp(&a.tx.tx.timestamp));
        
        let start = page * limit;
        let has_more = start + limit < total;
        
        let nodes: Vec<DagNodeInfo> = sorted
            .into_iter()
            .skip(start)
            .take(limit)
            .map(|n| DagNodeInfo {
                hash: n.hash.clone(),
                from: n.tx.tx.from.clone(),
                to: n.tx.tx.to.clone(),
                amount: n.tx.tx.amount,
                fee: n.tx.tx.gas_price.unwrap_or(0.001),
                nonce: n.tx.tx.nonce,
                ts: n.tx.tx.timestamp,
                parents: n.parents.clone(),
                finalized: n.finalized,
                weight: n.weight,
                kind: n.tx.tx.kind,
                sig: n.tx.signature.clone(),
            })
            .collect();
        
        (nodes, total, has_more)
    }

    /// Combined dashboard stats - single lock acquisition for all Explorer stats
    pub async fn get_dashboard_stats(&self) -> DashboardStats {
        let state = self.inner.read().await;
        
        // Use O(1) methods instead of O(n) get_all_nodes() iteration
        // This prevents lock starvation under high transaction load
        let dag_nodes = state.dag.node_count();
        let unfinalized_count = state.dag.unfinalized_count();
        let finalized_count = dag_nodes.saturating_sub(unfinalized_count);
        
        let latest_checkpoint_id = state.checkpoints.last().map(|cp| cp.hash.clone());
        
        DashboardStats {
            dag_nodes,
            tip_count: state.dag.tip_count(),
            account_count: state.accounts.len(),
            // CRITICAL: Use actual checkpoint height, NOT len() which breaks after pruning
            checkpoint_height: state.checkpoints.last().map(|cp| cp.height).unwrap_or(0),
            finalized_count,
            unfinalized_count,
            total_transactions: state.total_transactions,
            tips: state.dag.tips(),
            gas_price: state.current_gas_price,
            total_burned: state.total_burned,
            avg_gas: state.current_gas_price,
            latest_checkpoint_id,
        }
    }

    pub async fn add_transaction(&self, tx: SignedTransaction) -> Result<TransactionResult> {
        // PHASE 0: Validate transaction BEFORE any state mutations
        let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
        let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
        let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
        let is_consolidation_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Consolidation));
        let is_contract_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Contract));
        
        // System transactions (consolidation/anchor) bypass normal validation
        // They're created by validators to consolidate DAG tips
        let is_system_tx = is_consolidation_tx 
            || tx.signature.starts_with("anchor-")
            || tx.tx.from == "faucet"
            || tx.tx.from == "genesis";
        
        // Pre-check balance and stake minimum validation
        // System transactions (anchor/consolidation) skip this entirely
        if !is_system_tx {
            let state = self.inner.read().await;
            let gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);
            
            // Validate minimum stake amount BEFORE any state changes
            if is_stake_tx {
                let rewards = self.rewards.read().await;
                let min_stake = rewards.get_config().min_stake_amount;
                drop(rewards);
                
                if tx.tx.amount < min_stake {
                    tracing::warn!(
                        "Stake transaction rejected: amount {:.6} below minimum {:.6}",
                        tx.tx.amount, min_stake
                    );
                    return Err(anyhow::anyhow!(
                        "Minimum stake amount is {} RKU, you tried to stake {}",
                        min_stake, tx.tx.amount
                    ));
                }
            }
            
            // Validate memo size (max 1024 bytes for encrypted chat messages)
            const MAX_MEMO_SIZE: usize = 1024;
            if let Some(ref memo) = tx.tx.memo {
                if memo.len() > MAX_MEMO_SIZE {
                    tracing::warn!(
                        "Transaction rejected: memo too large ({} bytes, max {})",
                        memo.len(), MAX_MEMO_SIZE
                    );
                    return Err(anyhow::anyhow!(
                        "Memo too large: {} bytes (max {} bytes)",
                        memo.len(), MAX_MEMO_SIZE
                    ));
                }
            }
            
            // Validate references (max 4 references, must be valid tx hashes)
            const MAX_REFERENCES: usize = 4;
            if let Some(ref refs) = tx.tx.references {
                if refs.len() > MAX_REFERENCES {
                    tracing::warn!(
                        "Transaction rejected: too many references ({}, max {})",
                        refs.len(), MAX_REFERENCES
                    );
                    return Err(anyhow::anyhow!(
                        "Too many references: {} (max {})",
                        refs.len(), MAX_REFERENCES
                    ));
                }
            }
            
            // Validate contract transaction data
            if is_contract_tx {
                const MAX_CONTRACT_DATA_SIZE: usize = 3 * 1024 * 1024; // 3MB (allows ~2MB WASM base64-encoded)
                match &tx.tx.data {
                    None => {
                        tracing::warn!("Contract transaction rejected: missing data field");
                        return Err(anyhow::anyhow!(
                            "Contract transactions require a 'data' field with deploy or call payload"
                        ));
                    }
                    Some(data) => {
                        if data.len() > MAX_CONTRACT_DATA_SIZE {
                            tracing::warn!(
                                "Contract transaction rejected: data too large ({} bytes, max {})",
                                data.len(), MAX_CONTRACT_DATA_SIZE
                            );
                            return Err(anyhow::anyhow!(
                                "Contract data too large: {} bytes (max {} bytes)",
                                data.len(), MAX_CONTRACT_DATA_SIZE
                            ));
                        }
                        match rinku_core::types::ContractTransactionData::from_data_field(data) {
                            Ok(contract_data) => {
                                match &contract_data {
                                    rinku_core::types::ContractTransactionData::Deploy { wasm_base64, .. } => {
                                        let wasm_size_estimate = wasm_base64.len() * 3 / 4;
                                        const MAX_WASM_SIZE: usize = 2 * 1024 * 1024;
                                        if wasm_size_estimate > MAX_WASM_SIZE {
                                            return Err(anyhow::anyhow!(
                                                "WASM binary too large: ~{} bytes (max {})",
                                                wasm_size_estimate, MAX_WASM_SIZE
                                            ));
                                        }
                                        if wasm_base64.is_empty() {
                                            return Err(anyhow::anyhow!("WASM binary cannot be empty"));
                                        }
                                    }
                                    rinku_core::types::ContractTransactionData::Call { contract_id, entrypoint, .. } => {
                                        if contract_id.is_empty() {
                                            return Err(anyhow::anyhow!("Contract ID cannot be empty"));
                                        }
                                        if entrypoint.is_empty() {
                                            return Err(anyhow::anyhow!("Entrypoint cannot be empty"));
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Contract transaction rejected: invalid data: {}", e);
                                return Err(anyhow::anyhow!("{}", e));
                            }
                        }
                    }
                }
            }

            // Validate timestamp is not too far in the future
            // This prevents malicious actors from using far-future timestamps to delay finalization
            use crate::config::MAX_FUTURE_TIMESTAMP_MS;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            
            if tx.tx.timestamp > now_ms + MAX_FUTURE_TIMESTAMP_MS {
                tracing::warn!(
                    "Transaction rejected: timestamp {} too far in future (max {} ahead)",
                    tx.tx.timestamp, MAX_FUTURE_TIMESTAMP_MS
                );
                return Err(anyhow::anyhow!(
                    "Transaction timestamp is too far in the future"
                ));
            }
            
            // Calculate required balance based on transaction type
            let required_balance = if is_stake_tx {
                tx.tx.amount + gas_fee // Stake: need amount + gas
            } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                gas_fee // Unstake/Claim/Contract: only need gas
            } else {
                tx.tx.amount + gas_fee // Transfer: need amount + gas
            };
            
            // FINALITY-FIRST MODEL: Check using effective balance/nonce
            // Effective = confirmed - pending (unfinalized transactions)
            // This allows multiple pending txs while ensuring total doesn't exceed balance
            if tx.tx.from != "genesis" {
                if !state.accounts.contains_key(&tx.tx.from) {
                    tracing::warn!(
                        "Transaction rejected: account {} does not exist",
                        &tx.tx.from[..16.min(tx.tx.from.len())]
                    );
                    return Err(anyhow::anyhow!("Account does not exist"));
                }
                
                // Get effective balance (confirmed - pending outgoing)
                let effective_balance = Self::get_effective_balance(&state, &tx.tx.from);
                if effective_balance < required_balance {
                    tracing::warn!(
                        "Transaction rejected: insufficient effective balance. Have {:.6}, need {:.6} (amount: {:.6}, gas: {:.6})",
                        effective_balance, required_balance, tx.tx.amount, gas_fee
                    );
                    return Err(anyhow::anyhow!(
                        "Insufficient balance: have {:.6}, need {:.6}",
                        effective_balance, required_balance
                    ));
                }
                
                // Get effective nonce (confirmed + pending tx count)
                let effective_nonce = Self::get_effective_nonce(&state, &tx.tx.from);
                let confirmed_nonce = state.accounts.get(&tx.tx.from).map(|a| a.nonce).unwrap_or(0);
                
                // Nonce must be >= confirmed (not already finalized)
                if tx.tx.nonce < confirmed_nonce {
                    tracing::warn!(
                        "Transaction rejected: stale nonce. Confirmed nonce is {}, got {} (already finalized)",
                        confirmed_nonce, tx.tx.nonce
                    );
                    return Err(anyhow::anyhow!(
                        "Stale nonce: confirmed nonce is {}, got {} (already finalized)",
                        confirmed_nonce, tx.tx.nonce
                    ));
                }
                
                // Nonce must be == effective nonce (next in sequence including pending)
                if tx.tx.nonce != effective_nonce {
                    tracing::debug!(
                        "Nonce mismatch for {}: expected effective {}, got {}",
                        &tx.tx.from[..16.min(tx.tx.from.len())],
                        effective_nonce, tx.tx.nonce
                    );
                    return Err(anyhow::anyhow!(
                        "Invalid nonce: expected {}, got {}",
                        effective_nonce, tx.tx.nonce
                    ));
                }
            }
        }
        
        // PHASE 1: Pre-compute everything outside the lock
        // Normalize parent URLs to just hashes
        let client_parents: Vec<String> = tx
            .tx
            .parents
            .iter()
            .map(|p| {
                if p.starts_with("rinku://tx/h/") {
                    p.strip_prefix("rinku://tx/h/").unwrap_or(p).to_string()
                } else if p.starts_with("rinku://tx/") {
                    p.strip_prefix("rinku://tx/").unwrap_or(p).to_string()
                } else {
                    p.clone()
                }
            })
            .collect();

        // Calculate transaction weight based on sender's account
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        // CRITICAL FIX: Server-side tip injection
        // If client-provided parents don't exist in DAG, substitute with actual tips
        // This prevents tip explosion when clients reference pruned/missing transactions
        let (tx_weight, normalized_parents) = {
            let state = self.inner.read().await;
            
            let weight = if let Some(account) = state.accounts.get(&tx.tx.from) {
                calculate_account_weight(account, now_secs)
            } else {
                1.0 // New account, minimum weight
            };
            
            // Check which client parents exist in DAG
            let valid_parents: Vec<String> = client_parents
                .iter()
                .filter(|p| !p.is_empty() && state.dag.get_node(p).is_some())
                .cloned()
                .collect();
            
            // If no valid parents exist, inject current tips as parents
            let final_parents = if valid_parents.is_empty() {
                let current_tips = state.dag.tips();
                // Take up to 2 tips to reference (standard DAG behavior)
                let injected: Vec<String> = current_tips.into_iter().take(2).collect();
                if !injected.is_empty() {
                    tracing::debug!(
                        "Tip injection: tx {} had {} orphan parents, injecting {} tips",
                        &tx.hash[..16.min(tx.hash.len())],
                        client_parents.len(),
                        injected.len()
                    );
                }
                injected
            } else {
                valid_parents
            };
            
            (weight, final_parents)
        };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let node = rinku_core::types::DagNode {
            hash: tx.hash.clone(),
            tx: tx.clone(),
            parents: normalized_parents.clone(),
            children: Vec::new(),
            weight: tx_weight,
            finalized: false,
            checkpoint_height: None,
            received_at_ms: Some(now_ms),
        };

        // PHASE 2: Write lock with atomic re-validation
        // FINALITY-FIRST MODEL: Balance/nonce updates happen at checkpoint finalization
        // CRITICAL: Must re-check effective balance/nonce under write lock to prevent race conditions
        // Multiple concurrent transactions could pass read-lock validation before any is added to DAG
        let mut state = self.inner.write().await;
        
        // Re-validate under write lock (prevents race condition with concurrent txs from same sender)
        if !is_system_tx {
            let gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);
            let required_balance = if is_stake_tx {
                tx.tx.amount + gas_fee
            } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                gas_fee
            } else {
                tx.tx.amount + gas_fee
            };
            
            // ATOMIC CHECK: Re-check effective balance under write lock
            let effective_balance = Self::get_effective_balance(&state, &tx.tx.from);
            if effective_balance < required_balance {
                return Err(anyhow::anyhow!(
                    "Insufficient balance: have {:.6}, need {:.6}",
                    effective_balance, required_balance
                ));
            }
            
            // ATOMIC CHECK: Re-check effective nonce under write lock
            let effective_nonce = Self::get_effective_nonce(&state, &tx.tx.from);
            if tx.tx.nonce != effective_nonce {
                return Err(anyhow::anyhow!(
                    "Invalid nonce: expected {}, got {}",
                    effective_nonce, tx.tx.nonce
                ));
            }
        }

        state.dag.add_node(node)?;
        
        // Track transaction count for gas price adjustment
        state.txs_this_period += 1;
        // Note: total_transactions is incremented at checkpoint finalization (not here)
        // This ensures count reflects executed transactions with updated balances

        // Period-based gas adjustment
        const PERIOD_MS: u64 = 15000;
        const TARGET_TPS: f64 = 10.0;
        const MAX_CHANGE_PERCENT: f64 = 0.125;
        const ELASTICITY: f64 = 2.0;

        if now_ms - state.period_start_ms >= PERIOD_MS {
            let target_txs = TARGET_TPS * (PERIOD_MS as f64 / 1000.0);
            let utilization = state.txs_this_period as f64 / target_txs;
            let change_ratio = ((utilization - 1.0) / (ELASTICITY - 1.0)).clamp(-1.0, 1.0);
            let change_factor = 1.0 + change_ratio * MAX_CHANGE_PERCENT;
            state.current_gas_price = (state.current_gas_price * change_factor).clamp(
                state.config.gas.min_gas_price,
                state.config.gas.max_gas_price,
            );
            state.txs_this_period = 0;
            state.period_start_ms = now_ms;
        }
        
        // FINALITY-FIRST MODEL: No immediate execution
        // All balance changes, stake processing, and rewards happen at checkpoint finalization
        // We only add to DAG here - execution is deferred until tx is finalized
        drop(state);

        Ok(TransactionResult::Accepted)
    }
    
    pub async fn add_relay_transaction(
        &self,
        tx: SignedTransaction,
        relayer_address: &str,
        inner_kind: rinku_core::types::TransactionKind,
    ) -> Result<TransactionResult> {
        let is_stake_tx = matches!(inner_kind, rinku_core::types::TransactionKind::Stake);
        let is_unstake_tx = matches!(inner_kind, rinku_core::types::TransactionKind::Unstake);
        let is_claim_tx = matches!(inner_kind, rinku_core::types::TransactionKind::ClaimRewards);
        let is_contract_tx = matches!(inner_kind, rinku_core::types::TransactionKind::Contract);

        let intent_from_addr = tx.tx.data.as_ref().and_then(|d| {
            serde_json::from_str::<serde_json::Value>(d).ok()
                .and_then(|v| v.get("intentFrom")?.as_str().map(|s| s.to_string()))
        }).ok_or_else(|| anyhow::anyhow!("Relay transaction missing intentFrom in data"))?;

        {
            let state = self.inner.read().await;
            let gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);

            if let Some(ref memo) = tx.tx.memo {
                if memo.len() > 1024 {
                    return Err(anyhow::anyhow!("Memo too large: {} bytes (max 1024)", memo.len()));
                }
            }
            if let Some(ref refs) = tx.tx.references {
                if refs.len() > 4 {
                    return Err(anyhow::anyhow!("Too many references: {} (max 4)", refs.len()));
                }
            }

            if is_stake_tx {
                let rewards = self.rewards.read().await;
                let min_stake = rewards.get_config().min_stake_amount;
                drop(rewards);
                if tx.tx.amount < min_stake {
                    return Err(anyhow::anyhow!(
                        "Minimum stake amount is {} RKU, you tried to stake {}",
                        min_stake, tx.tx.amount
                    ));
                }
            }

            let required_balance_intent = if is_stake_tx {
                tx.tx.amount
            } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                0.0
            } else {
                tx.tx.amount
            };

            if intent_from_addr != "genesis" {
                if !state.accounts.contains_key(&intent_from_addr) {
                    return Err(anyhow::anyhow!("Intent signer account does not exist"));
                }

                let effective_balance = Self::get_effective_balance(&state, &intent_from_addr);
                if effective_balance < required_balance_intent {
                    return Err(anyhow::anyhow!(
                        "Insufficient balance for intent signer: have {:.6}, need {:.6}",
                        effective_balance, required_balance_intent
                    ));
                }

                let effective_nonce = Self::get_effective_nonce(&state, &intent_from_addr);
                let confirmed_nonce = state.accounts.get(&intent_from_addr).map(|a| a.nonce).unwrap_or(0);
                if tx.tx.nonce < confirmed_nonce {
                    return Err(anyhow::anyhow!(
                        "Stale nonce: confirmed nonce is {}, got {}",
                        confirmed_nonce, tx.tx.nonce
                    ));
                }
                if tx.tx.nonce != effective_nonce {
                    return Err(anyhow::anyhow!(
                        "Invalid nonce: expected {}, got {}",
                        effective_nonce, tx.tx.nonce
                    ));
                }
            }

            if !state.accounts.contains_key(relayer_address) {
                return Err(anyhow::anyhow!("Relayer account does not exist"));
            }
            let relayer_balance = state.accounts.get(relayer_address)
                .map(|a| a.balance)
                .unwrap_or(0.0);
            if relayer_balance < gas_fee {
                return Err(anyhow::anyhow!(
                    "Relayer insufficient gas balance: have {:.6}, need {:.6}",
                    relayer_balance, gas_fee
                ));
            }
        }

        let client_parents: Vec<String> = tx.tx.parents.iter()
            .map(|p| {
                if p.starts_with("rinku://tx/h/") {
                    p.strip_prefix("rinku://tx/h/").unwrap_or(p).to_string()
                } else if p.starts_with("rinku://tx/") {
                    p.strip_prefix("rinku://tx/").unwrap_or(p).to_string()
                } else {
                    p.clone()
                }
            })
            .collect();

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let now_ms = now_secs * 1000;

        let (tx_weight, normalized_parents) = {
            let state = self.inner.read().await;
            let weight = if let Some(account) = state.accounts.get(&tx.tx.from) {
                crate::state::calculate_account_weight(account, now_secs)
            } else {
                1.0
            };
            let valid_parents: Vec<String> = client_parents.iter()
                .filter(|p| !p.is_empty() && state.dag.get_node(p).is_some())
                .cloned()
                .collect();
            let final_parents = if valid_parents.is_empty() {
                state.dag.tips().into_iter().take(2).collect()
            } else {
                valid_parents
            };
            (weight, final_parents)
        };

        let node = rinku_core::types::DagNode {
            hash: tx.hash.clone(),
            tx: tx.clone(),
            parents: normalized_parents.clone(),
            children: Vec::new(),
            weight: tx_weight,
            finalized: false,
            checkpoint_height: None,
            received_at_ms: Some(now_ms),
        };

        let mut state = self.inner.write().await;

        let gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);
        let relayer_balance = state.accounts.get(relayer_address)
            .map(|a| a.balance)
            .unwrap_or(0.0);
        if relayer_balance < gas_fee {
            return Err(anyhow::anyhow!(
                "Relayer insufficient gas balance: have {:.6}, need {:.6}",
                relayer_balance, gas_fee
            ));
        }

        let effective_nonce = Self::get_effective_nonce(&state, &intent_from_addr);
        if tx.tx.nonce != effective_nonce {
            return Err(anyhow::anyhow!(
                "Invalid nonce: expected {}, got {}",
                effective_nonce, tx.tx.nonce
            ));
        }

        if state.dag.get_node(&tx.hash).is_some() {
            return Err(anyhow::anyhow!("Duplicate transaction hash"));
        }

        state.dag.add_node(node)?;
        state.txs_this_period += 1;

        tracing::info!(
            "RELAY TX accepted: hash={}, intentFrom={}, relayer={}, amount={:.4}",
            &tx.hash[..16.min(tx.hash.len())],
            &intent_from_addr[..16.min(intent_from_addr.len())],
            &relayer_address[..16.min(relayer_address.len())],
            tx.tx.amount
        );

        drop(state);
        Ok(TransactionResult::Accepted)
    }

    /// Execute a finalized transaction - apply all state changes
    /// Execute a transaction immediately upon fast-path confirmation (2/3 stake quorum).
    /// This applies balance/nonce/stake changes in real-time for sub-500ms finality.
    /// Rewards (tip/witness) are deferred to checkpoint time to maintain deterministic ordering.
    /// Returns true if execution succeeded, false if validation failed.
    pub async fn execute_fast_path_transaction(&self, tx: &SignedTransaction) -> bool {
        let gas_fee = {
            let state = self.inner.read().await;
            tx.tx.gas_price.unwrap_or(state.current_gas_price)
        };

        let is_relay_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Relay));
        let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
        let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
        let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
        let is_contract_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Contract));

        {
            let state = self.inner.read().await;

            if is_relay_tx {
                let relay_parsed = tx.tx.data.as_ref().and_then(|d| {
                    serde_json::from_str::<serde_json::Value>(d).ok()
                });
                let intent_from = relay_parsed.as_ref()
                    .and_then(|v| v.get("intentFrom")?.as_str().map(|s| s.to_string()));

                if let Some(ref intent_sender) = intent_from {
                    if let Some(from_account) = state.accounts.get(intent_sender) {
                        if from_account.balance < tx.tx.amount {
                            tracing::warn!(
                                "FastPath relay rejected: intent signer {} insufficient balance ({:.8} < {:.8})",
                                &intent_sender[..16.min(intent_sender.len())],
                                from_account.balance,
                                tx.tx.amount
                            );
                            return false;
                        }
                        if tx.tx.nonce != from_account.nonce {
                            tracing::warn!(
                                "FastPath relay rejected: intent signer {} nonce mismatch (tx={} vs account={})",
                                &intent_sender[..16.min(intent_sender.len())],
                                tx.tx.nonce,
                                from_account.nonce
                            );
                            return false;
                        }
                    } else {
                        tracing::warn!("FastPath relay rejected: intent signer account {} not found", &intent_sender[..16.min(intent_sender.len())]);
                        return false;
                    }
                } else {
                    tracing::warn!("FastPath relay rejected: missing intentFrom in relay data");
                    return false;
                }

                if let Some(relayer_account) = state.accounts.get(&tx.tx.from) {
                    if relayer_account.balance < gas_fee {
                        tracing::warn!(
                            "FastPath relay rejected: relayer {} insufficient gas ({:.8} < {:.8})",
                            &tx.tx.from[..16.min(tx.tx.from.len())],
                            relayer_account.balance,
                            gas_fee
                        );
                        return false;
                    }
                } else {
                    tracing::warn!("FastPath relay rejected: relayer account {} not found", &tx.tx.from[..16.min(tx.tx.from.len())]);
                    return false;
                }
            } else {
                if let Some(from_account) = state.accounts.get(&tx.tx.from) {
                    let required = if is_stake_tx {
                        tx.tx.amount + gas_fee
                    } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                        gas_fee
                    } else {
                        tx.tx.amount + gas_fee
                    };
                    if from_account.balance < required {
                        tracing::warn!(
                            "FastPath execution rejected: {} insufficient balance ({:.8} < {:.8})",
                            &tx.tx.from[..16.min(tx.tx.from.len())],
                            from_account.balance,
                            required
                        );
                        return false;
                    }
                    if tx.tx.nonce != from_account.nonce {
                        tracing::warn!(
                            "FastPath execution rejected: {} nonce mismatch (tx={} vs account={})",
                            &tx.tx.from[..16.min(tx.tx.from.len())],
                            tx.tx.nonce,
                            from_account.nonce
                        );
                        return false;
                    }
                } else {
                    tracing::warn!(
                        "FastPath execution rejected: account {} not found",
                        &tx.tx.from[..16.min(tx.tx.from.len())]
                    );
                    return false;
                }
            }
        }

        self.execute_finalized_transaction_core(tx).await;

        tracing::info!(
            "FastPath EXECUTED tx {} ({} -> {}, amount={:.8}, gas={:.8})",
            &tx.hash[..16.min(tx.hash.len())],
            &tx.tx.from[..16.min(tx.tx.from.len())],
            &tx.tx.to[..16.min(tx.tx.to.len())],
            tx.tx.amount,
            gas_fee
        );

        true
    }

    /// Called by checkpoint finalization after transactions are confirmed
    /// FINALITY-FIRST MODEL: This is where actual balance/nonce changes happen
    /// 
    /// NOTE: This method runs PHASES 1 & 2 (balance/stake/claim changes) and PHASE 3 (rewards).
    /// For proper simulation/execution parity, use the two-pass approach:
    /// 1. execute_finalized_transaction_core() for all txs (PHASES 1 & 2)
    /// 2. execute_finalized_transaction_rewards() for all txs (PHASE 3)
    pub async fn execute_finalized_transaction(&self, tx: &SignedTransaction) {
        self.execute_finalized_transaction_core(tx).await;
        self.execute_finalized_transaction_rewards(tx).await;
    }
    
    /// Execute PHASES 1 & 2: Balance/nonce changes and stake/unstake/claim processing
    /// This must be called for ALL transactions BEFORE any execute_finalized_transaction_rewards
    /// to ensure claim transactions see the correct pending_rewards (matching simulation)
    pub async fn execute_finalized_transaction_core(&self, tx: &SignedTransaction) {
        let gas_fee = {
            let state = self.inner.read().await;
            tx.tx.gas_price.unwrap_or(state.current_gas_price)
        };
        
        let is_relay_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Relay));
        let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
        let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
        let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
        let is_contract_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Contract));

        let relay_info = if is_relay_tx {
            tx.tx.data.as_ref().and_then(|d| {
                serde_json::from_str::<serde_json::Value>(d).ok().and_then(|v| {
                    let intent_from = v.get("intentFrom")?.as_str()?.to_string();
                    let inner_kind_str = v.get("innerKind").and_then(|k| k.as_str()).unwrap_or("transfer");
                    let inner_kind = match inner_kind_str {
                        "stake" | "Stake" => rinku_core::types::TransactionKind::Stake,
                        "unstake" | "Unstake" => rinku_core::types::TransactionKind::Unstake,
                        "claimRewards" | "ClaimRewards" => rinku_core::types::TransactionKind::ClaimRewards,
                        "contract" | "Contract" => rinku_core::types::TransactionKind::Contract,
                        _ => rinku_core::types::TransactionKind::Transfer,
                    };
                    Some((intent_from, inner_kind))
                })
            })
        } else {
            None
        };
        
        // PHASE 1: Apply balance and nonce changes
        {
            let mut state = self.inner.write().await;

            if is_relay_tx {
                if let Some((ref intent_from_addr, ref inner_kind)) = relay_info {
                    let is_inner_transfer = matches!(inner_kind, rinku_core::types::TransactionKind::Transfer);

                    if let Some(from_account) = state.accounts.get_mut(intent_from_addr) {
                        if is_inner_transfer {
                            from_account.balance -= tx.tx.amount;
                        } else if matches!(inner_kind, rinku_core::types::TransactionKind::Stake) {
                            from_account.balance -= tx.tx.amount;
                        }
                        from_account.nonce = tx.tx.nonce + 1;
                    }

                    if let Some(relayer_account) = state.accounts.get_mut(&tx.tx.from) {
                        relayer_account.balance -= gas_fee;
                    }

                    if is_inner_transfer {
                        let to_account = state
                            .accounts
                            .entry(tx.tx.to.clone())
                            .or_insert_with(|| Account::new(tx.tx.to.clone(), tx.tx.timestamp));
                        to_account.balance += tx.tx.amount;
                    }
                }
            } else {
                if let Some(from_account) = state.accounts.get_mut(&tx.tx.from) {
                    if is_stake_tx {
                        from_account.balance -= tx.tx.amount + gas_fee;
                    } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                        from_account.balance -= gas_fee;
                    } else {
                        from_account.balance -= tx.tx.amount + gas_fee;
                    }
                    from_account.nonce = tx.tx.nonce + 1;
                }
            
                if !is_stake_tx && !is_unstake_tx && !is_claim_tx && !is_contract_tx {
                    let to_account = state
                        .accounts
                        .entry(tx.tx.to.clone())
                        .or_insert_with(|| Account::new(tx.tx.to.clone(), tx.tx.timestamp));
                    to_account.balance += tx.tx.amount;
                }
            }
            
            // EIP-1559 gas tracking
            state.total_burned += gas_fee * 0.5;
            state.total_to_validators += gas_fee * 0.5;
        }
        
        // PHASE 2: Process stake/unstake/claim transactions
        if let Some(ref kind) = tx.tx.kind {
            use rinku_core::types::TransactionKind;
            let from_addr = &tx.tx.from;
            let stake_amount = tx.tx.amount;
            
            match kind {
                TransactionKind::Stake => {
                    let stake_update: Option<(f64, u64)> = {
                        let mut rewards = self.rewards.write().await;
                        if let Err(e) = rewards.stake(from_addr, stake_amount) {
                            tracing::warn!("Failed to process stake tx: {}", e);
                            None
                        } else {
                            tracing::debug!("Finalized stake: {} staked {} RKU", &from_addr[..16.min(from_addr.len())], stake_amount);
                            rewards.get_stake(from_addr).map(|p| (p.amount, p.staked_at))
                        }
                    };
                    if let Some((amount, staked_at)) = stake_update {
                        self.update_account_staked(from_addr, amount, Some(staked_at / 1000)).await;
                    }
                }
                TransactionKind::Unstake => {
                    let unstake_result: Option<f64> = {
                        let mut rewards = self.rewards.write().await;
                        match rewards.unstake(from_addr) {
                            Ok(amount) => {
                                tracing::debug!("Finalized unstake: {} unstaked {} RKU", &from_addr[..16.min(from_addr.len())], amount);
                                Some(amount)
                            }
                            Err(e) => {
                                tracing::warn!("Failed to process unstake tx: {}", e);
                                None
                            }
                        }
                    };
                    if let Some(unstaked_amount) = unstake_result {
                        let mut state = self.inner.write().await;
                        if let Some(account) = state.accounts.get_mut(from_addr) {
                            account.balance += unstaked_amount;
                            account.staked = 0.0;
                            tracing::info!(
                                "Unstake finalized: {} balance restored by {} RKU (new balance: {})",
                                &from_addr[..16.min(from_addr.len())],
                                unstaked_amount,
                                account.balance
                            );
                        }
                    }
                }
                TransactionKind::ClaimRewards => {
                    let claimed: f64 = {
                        let mut rewards = self.rewards.write().await;
                        rewards.claim_rewards(from_addr)
                    };
                    tracing::info!(
                        "[EXECUTION] Claim for {}: claimed_amount={:.8}",
                        &from_addr[..16.min(from_addr.len())],
                        claimed
                    );
                    if claimed > 0.0 {
                        let mut state = self.inner.write().await;
                        if let Some(account) = state.accounts.get_mut(from_addr) {
                            let old_balance = account.balance;
                            account.balance += claimed;
                            tracing::info!(
                                "[EXECUTION] Claim for {}: old_balance={:.8}, new_balance={:.8}",
                                &from_addr[..16.min(from_addr.len())],
                                old_balance,
                                account.balance
                            );
                        }
                    }
                }
                TransactionKind::Contract => {
                    if let Some(ref data) = tx.tx.data {
                        match rinku_core::types::ContractTransactionData::from_data_field(data) {
                            Ok(contract_data) => {
                                self.execute_contract_transaction(tx, contract_data).await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to parse contract tx data during finalization: {}", e);
                            }
                        }
                    }
                }
                TransactionKind::Relay => {
                    if let Some((ref intent_from_addr, ref inner_kind)) = relay_info {
                        match inner_kind {
                            TransactionKind::Stake => {
                                let stake_update: Option<(f64, u64)> = {
                                    let mut rewards = self.rewards.write().await;
                                    if let Err(e) = rewards.stake(intent_from_addr, stake_amount) {
                                        tracing::warn!("Failed to process relayed stake tx: {}", e);
                                        None
                                    } else {
                                        tracing::debug!("Finalized relayed stake: {} staked {} RKU", &intent_from_addr[..16.min(intent_from_addr.len())], stake_amount);
                                        rewards.get_stake(intent_from_addr).map(|p| (p.amount, p.staked_at))
                                    }
                                };
                                if let Some((amount, staked_at)) = stake_update {
                                    self.update_account_staked(intent_from_addr, amount, Some(staked_at / 1000)).await;
                                }
                            }
                            TransactionKind::Unstake => {
                                let unstake_result: Option<f64> = {
                                    let mut rewards = self.rewards.write().await;
                                    match rewards.unstake(intent_from_addr) {
                                        Ok(amount) => Some(amount),
                                        Err(e) => {
                                            tracing::warn!("Failed to process relayed unstake tx: {}", e);
                                            None
                                        }
                                    }
                                };
                                if let Some(unstaked_amount) = unstake_result {
                                    let mut state = self.inner.write().await;
                                    if let Some(account) = state.accounts.get_mut(intent_from_addr.as_str()) {
                                        account.balance += unstaked_amount;
                                        account.staked = 0.0;
                                    }
                                }
                            }
                            TransactionKind::ClaimRewards => {
                                let claimed: f64 = {
                                    let mut rewards = self.rewards.write().await;
                                    rewards.claim_rewards(intent_from_addr)
                                };
                                if claimed > 0.0 {
                                    let mut state = self.inner.write().await;
                                    if let Some(account) = state.accounts.get_mut(intent_from_addr.as_str()) {
                                        account.balance += claimed;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    async fn execute_contract_transaction(
        &self,
        tx: &SignedTransaction,
        contract_data: rinku_core::types::ContractTransactionData,
    ) {
        let runtime = crate::contracts::ContractRuntime::new();
        let gas_price = {
            let state = self.inner.read().await;
            tx.tx.gas_price.unwrap_or(state.current_gas_price)
        };

        match contract_data {
            rinku_core::types::ContractTransactionData::Deploy { wasm_base64, init_state } => {
                let contract_id = crate::contracts::create_contract_id(&tx.tx.from, tx.tx.nonce);
                let deploy_url = format!("rinku://contract/{}", contract_id);

                let mut final_state = init_state.clone();

                let init_input: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
                let init_result = runtime.execute_with_caller(
                    &contract_id,
                    &wasm_base64,
                    "init",
                    &init_input,
                    &init_state,
                    1,
                    Some(1_000_000),
                    &tx.tx.from,
                    tx.tx.timestamp / 1000,
                );

                let execution_gas = init_result.gas_used;
                self.charge_contract_execution_fee(&tx.tx.from, execution_gas, gas_price).await;

                if init_result.success {
                    if let Some(ref diff) = init_result.state_diff {
                        for change in &diff.changes {
                            if let Some(ref new_value) = change.new_value {
                                final_state.insert(change.key.clone(), new_value.clone());
                            } else {
                                final_state.remove(&change.key);
                            }
                        }
                    }
                    tracing::info!(
                        "Contract {} init executed successfully ({} state keys, gas: {})",
                        contract_id, final_state.len(), execution_gas
                    );
                } else {
                    tracing::warn!(
                        "Contract {} init failed (non-fatal, gas: {}): {:?}",
                        contract_id, execution_gas, init_result.error
                    );
                }

                let state_hash = crate::contracts::compute_state_hash(&final_state);

                let contract_state = crate::contracts::ContractState {
                    contract_id: contract_id.clone(),
                    creator: tx.tx.from.clone(),
                    wasm_base64,
                    deploy_url,
                    state: final_state,
                    state_hash,
                    height: 0,
                    created_at: tx.tx.timestamp / 1000,
                    schema: None,
                };

                match self.store_contract(contract_state).await {
                    Ok(()) => {
                        tracing::info!(
                            "Contract {} deployed via finalized tx {} by {}",
                            contract_id, &tx.hash[..16.min(tx.hash.len())],
                            &tx.tx.from[..16.min(tx.tx.from.len())]
                        );
                    }
                    Err(e) => {
                        tracing::error!("Failed to store contract {} from tx: {}", contract_id, e);
                    }
                }
            }
            rinku_core::types::ContractTransactionData::Call { contract_id, entrypoint, input } => {
                let contract = match self.get_contract(&contract_id).await {
                    Some(c) => c,
                    None => {
                        tracing::error!(
                            "Contract {} not found during finalization of tx {}",
                            contract_id, &tx.hash[..16.min(tx.hash.len())]
                        );
                        return;
                    }
                };

                let result = runtime.execute_with_caller(
                    &contract_id,
                    &contract.wasm_base64,
                    &entrypoint,
                    &input,
                    &contract.state,
                    contract.height + 1,
                    tx.tx.gas_limit,
                    &tx.tx.from,
                    tx.tx.timestamp / 1000,
                );

                let execution_gas = result.gas_used;
                self.charge_contract_execution_fee(&tx.tx.from, execution_gas, gas_price).await;

                if result.success {
                    let mut new_state = contract.state.clone();
                    let new_height = contract.height + 1;

                    if let Some(ref diff) = result.state_diff {
                        for change in &diff.changes {
                            if let Some(ref new_value) = change.new_value {
                                new_state.insert(change.key.clone(), new_value.clone());
                            } else {
                                new_state.remove(&change.key);
                            }
                        }
                    }

                    let new_state_hash = crate::contracts::compute_state_hash(&new_state);

                    if let Err(e) = self.update_contract_state(
                        &contract_id,
                        new_state,
                        new_state_hash,
                        new_height,
                    ).await {
                        tracing::error!("Failed to update contract {} state: {}", contract_id, e);
                    } else {
                        tracing::info!(
                            "Contract {} call '{}' executed via finalized tx {} (height: {}, gas: {})",
                            contract_id, entrypoint,
                            &tx.hash[..16.min(tx.hash.len())],
                            new_height, execution_gas
                        );
                    }
                } else {
                    tracing::warn!(
                        "Contract {} call '{}' failed during finalization of tx {} (gas: {}): {:?}",
                        contract_id, entrypoint,
                        &tx.hash[..16.min(tx.hash.len())],
                        execution_gas,
                        result.error
                    );
                }
            }
        }
    }

    async fn charge_contract_execution_fee(&self, from: &str, gas_used: u64, gas_price: f64) {
        use crate::wasm_runtime::BASE_TX_GAS;
        let additional_gas = gas_used.saturating_sub(BASE_TX_GAS);
        let execution_fee = (additional_gas as f64 / BASE_TX_GAS as f64) * gas_price;
        if execution_fee > 0.0 {
            let mut state = self.inner.write().await;
            if let Some(account) = state.accounts.get_mut(from) {
                account.balance -= execution_fee;
                if account.balance < 0.0 {
                    account.balance = 0.0;
                }
            }
            state.total_burned += execution_fee * 0.5;
            state.total_to_validators += execution_fee * 0.5;
            tracing::info!(
                "Contract execution fee: {} total gas ({} additional) = {:.6} RKU from {}",
                gas_used, additional_gas, execution_fee, &from[..16.min(from.len())]
            );
        }
    }
    
    /// Execute PHASE 3: Process tip and witness rewards for a transaction
    /// This must be called for ALL transactions AFTER all execute_finalized_transaction_core calls
    /// to ensure claim transactions don't see witness rewards from earlier txs in the same checkpoint
    pub async fn execute_finalized_transaction_rewards(&self, tx: &SignedTransaction) {
        let gas_fee = {
            let state = self.inner.read().await;
            tx.tx.gas_price.unwrap_or(state.current_gas_price)
        };
        
        // PHASE 3: Process tip and witness rewards
        let tx_hash = &tx.hash;
        let tx_url = format!("rinku://tx/h/{}", tx_hash);
        let tx_amount = tx.tx.amount;
        let from_addr = &tx.tx.from;
        
        // Get parent info for rewards
        let (parent_creators, validator_addr, normalized_parents) = {
            let state = self.inner.read().await;
            let parents: Vec<String> = tx.tx.parents.iter()
                .map(|p| {
                    if p.starts_with("rinku://tx/h/") {
                        p.strip_prefix("rinku://tx/h/").unwrap_or(p).to_string()
                    } else if p.starts_with("rinku://tx/") {
                        p.strip_prefix("rinku://tx/").unwrap_or(p).to_string()
                    } else {
                        p.clone()
                    }
                })
                .collect();
            
            let creators: Vec<(String, String)> = parents.iter()
                .filter_map(|parent_hash| {
                    state.dag.get_node(parent_hash).map(|node| {
                        let parent_url = format!("rinku://tx/h/{}", parent_hash);
                        (parent_url, node.tx.tx.from.clone())
                    })
                })
                .collect();
            
            (creators, state.node_validator_address.clone(), parents)
        };
        
        if tx_amount > 0.0 || gas_fee > 0.0 {
            let reward_base = tx_amount + gas_fee;
            let mut rewards = self.rewards.write().await;
            
            // Tip reward: validator who included this transaction gets rewarded
            if let Some(ref validator) = validator_addr {
                if let Some(first_parent) = normalized_parents.first() {
                    let tip_url = format!("rinku://tx/h/{}", first_parent);
                    rewards.process_tip_reward(&tx_url, &tip_url, validator, reward_base);
                }
            }
            
            // Witness rewards: reward creators of referenced parent transactions
            for (parent_url, parent_creator) in &parent_creators {
                if parent_creator != from_addr {
                    rewards.process_witness_reward(&tx_url, parent_url, parent_creator, reward_base);
                }
            }
        }
    }


    /// Add transaction to DAG only - for sync operations where state is already correct
    /// This skips nonce/balance validation since the transaction was already validated by the peer
    /// and accounts already have correct state from the snapshot.
    /// 
    /// TRUST MODEL: This method assumes the peer is trusted and transactions are valid.
    /// Parents must exist or be "genesis" - if parents are missing, the transaction is rejected.
    pub async fn add_transaction_dag_only(&self, tx: SignedTransaction) -> Result<()> {
        // Normalize parent URLs to just hashes
        let normalized_parents: Vec<String> = tx
            .tx
            .parents
            .iter()
            .map(|p| {
                if p.starts_with("rinku://tx/h/") {
                    p.strip_prefix("rinku://tx/h/").unwrap_or(p).to_string()
                } else if p.starts_with("rinku://tx/") {
                    p.strip_prefix("rinku://tx/").unwrap_or(p).to_string()
                } else {
                    p.clone()
                }
            })
            .collect();

        let mut state = self.inner.write().await;
        
        // Check if already exists
        if state.dag.get_node(&tx.hash).is_some() {
            return Ok(()); // Already have this transaction
        }
        
        // Verify all parents exist - reject if any parent is missing
        // This ensures DAG integrity is preserved
        for p in &normalized_parents {
            if p != "genesis" && state.dag.get_node(p).is_none() {
                anyhow::bail!("Parent {} not found for tx {}", p, &tx.hash);
            }
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let node = rinku_core::types::DagNode {
            hash: tx.hash.clone(),
            tx: tx.clone(),
            parents: normalized_parents,
            children: Vec::new(),
            weight: 1.0, // Default weight for synced transactions
            finalized: false,
            checkpoint_height: None,
            received_at_ms: Some(now_ms),
        };

        state.dag.add_node(node)?;
        Ok(())
    }
    
    /// Alias for add_transaction_dag_only - used by P2P sync
    pub async fn add_transaction_from_sync(&self, tx: SignedTransaction) -> Result<()> {
        self.add_transaction_dag_only(tx).await
    }
    
    /// Set the checkpoint height for a transaction (used by P2P sync)
    pub async fn set_tx_checkpoint_height(&self, hash: &str, height: u64) {
        let mut state = self.inner.write().await;
        if let Some(node) = state.dag.get_node_mut(hash) {
            node.checkpoint_height = Some(height);
            node.finalized = true;
        }
    }
    
    /// Apply a full P2P snapshot to local state
    /// Used for bootstrapping from a peer when significantly behind
    #[cfg(feature = "p2p")]
    pub async fn apply_p2p_snapshot(&self, snapshot: crate::network::SnapshotData) -> anyhow::Result<()> {
        use tracing::info;
        
        let mut state = self.inner.write().await;
        
        info!("Applying P2P snapshot: {} accounts, {} validators, {} checkpoints",
              snapshot.accounts.len(), snapshot.validators.len(), snapshot.checkpoints.len());
        
        // Apply accounts
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        
        for account_data in snapshot.accounts {
            // Get existing account or create new one
            let mut account = state.accounts.get(&account_data.address)
                .cloned()
                .unwrap_or_else(|| Account::new(account_data.address.clone(), now_ms));
            
            account.balance = account_data.balance;
            account.nonce = account_data.nonce;
            account.staked = account_data.stake;
            state.accounts.insert(account_data.address, account);
        }
        
        // Apply validators
        for validator_data in snapshot.validators {
            let validator = rinku_core::types::Validator {
                address: validator_data.address.clone(),
                stake: validator_data.stake,
                first_stake_time: 0,
                bls_public_key: Some(validator_data.bls_public_key),
                missed_checkpoints: 0,
            };
            state.validators.insert(validator_data.address, validator);
        }
        
        // Apply checkpoints
        for cp_data in snapshot.checkpoints {
            let checkpoint = rinku_core::types::Checkpoint {
                height: cp_data.height,
                tx_merkle_root: cp_data.merkle_root,
                state_root: String::new(),
                receipt_root: String::new(),
                timestamp: cp_data.timestamp,
                previous_hash: cp_data.previous_hash,
                tip_count: cp_data.tx_count as u32,
                hash: cp_data.hash.unwrap_or_default(),
                signer_bitmap: None,
                aggregated_signature: cp_data.signature,
                validator_signatures: Vec::new(),
                finalized_tx_hashes: Vec::new(),
                weight_trie_root: String::new(),
            };
            
            // Only add if we don't have this checkpoint yet
            if !state.checkpoints.iter().any(|c| c.height == checkpoint.height) {
                state.checkpoints.push(checkpoint);
            }
        }
        
        // Sort checkpoints by height
        state.checkpoints.sort_by_key(|c| c.height);
        
        // Apply recent transactions to DAG
        for tx_data in snapshot.recent_txs {
            let signed_tx = rinku_core::types::SignedTransaction {
                hash: tx_data.hash.clone(),
                tx: rinku_core::types::Transaction {
                    from: tx_data.from,
                    to: tx_data.to,
                    amount: tx_data.amount,
                    nonce: tx_data.nonce,
                    timestamp: tx_data.timestamp,
                    parents: tx_data.parents,
                    gas_price: Some(tx_data.gas_price),
                    gas_limit: None,
                    data: None,
                    signature: None,
                    kind: None,
                    memo: tx_data.memo,
                    references: tx_data.references,
                },
                signature: tx_data.signature,
            };
            
            // Add to DAG (skip if exists)
            if state.dag.get_node(&signed_tx.hash).is_none() {
                let now_dag = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                    
                let node = rinku_core::types::DagNode {
                    hash: signed_tx.hash.clone(),
                    tx: signed_tx,
                    parents: Vec::new(), // Parents handled separately
                    children: Vec::new(),
                    weight: 1.0,
                    finalized: false,
                    checkpoint_height: None,
                    received_at_ms: Some(now_dag),
                };
                let _ = state.dag.add_node(node);
            }
        }
        
        info!("P2P snapshot applied successfully");
        Ok(())
    }

    /// Force-add a transaction to the DAG without any validation.
    /// Used ONLY for checkpoint vote verification - allows syncing transactions
    /// that would otherwise be rejected (stale nonce, missing parents).
    /// Does NOT execute the transaction or update account balances.
    pub async fn force_add_transaction_for_vote(&self, tx: SignedTransaction) -> Result<()> {
        let normalized_parents: Vec<String> = tx
            .tx
            .parents
            .iter()
            .map(|p| {
                if p.starts_with("rinku://tx/h/") {
                    p.strip_prefix("rinku://tx/h/").unwrap_or(p).to_string()
                } else if p.starts_with("rinku://tx/") {
                    p.strip_prefix("rinku://tx/").unwrap_or(p).to_string()
                } else {
                    p.clone()
                }
            })
            .collect();

        let mut state = self.inner.write().await;
        
        // Check if already exists - if so, nothing to do
        if state.dag.get_node(&tx.hash).is_some() {
            return Ok(());
        }
        
        // Filter parents to only those that exist in our DAG
        // Missing parents are OK for vote verification - we just need the hash
        let existing_parents: Vec<String> = normalized_parents
            .into_iter()
            .filter(|p| p == "genesis" || state.dag.get_node(p).is_some())
            .collect();

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let node = rinku_core::types::DagNode {
            hash: tx.hash.clone(),
            tx: tx.clone(),
            parents: existing_parents, // Use only parents we have
            children: Vec::new(),
            weight: 1.0,
            finalized: false,
            checkpoint_height: None,
            received_at_ms: Some(now_ms),
        };

        state.dag.add_node(node)?;
        Ok(())
    }

    /// Batch add transactions - optimized for high throughput
    pub async fn add_transactions_batch(&self, txs: Vec<SignedTransaction>) -> Vec<Result<()>> {
        // PHASE 0: Pre-validate all transactions BEFORE any state mutations
        let mut validation_results: Vec<Option<anyhow::Error>> = Vec::with_capacity(txs.len());
        
        // Get min_stake from rewards config first
        let min_stake = {
            let rewards = self.rewards.read().await;
            rewards.get_config().min_stake_amount
        };
        
        {
            let state = self.inner.read().await;
            for tx in txs.iter() {
                let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
                let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
                let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
                let gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);
                
                // Validate minimum stake amount BEFORE any state changes
                if is_stake_tx && tx.tx.amount < min_stake {
                    validation_results.push(Some(anyhow::anyhow!(
                        "Minimum stake amount is {} RKU, you tried to stake {}",
                        min_stake, tx.tx.amount
                    )));
                    continue;
                }
                
                let required_balance = if is_stake_tx {
                    tx.tx.amount + gas_fee
                } else if is_unstake_tx || is_claim_tx {
                    gas_fee
                } else {
                    tx.tx.amount + gas_fee
                };
                
                if tx.tx.from != "genesis" {
                    // Use effective_balance which accounts for pending (unfinalized) transactions
                    let effective_balance = Self::get_effective_balance(&state, &tx.tx.from);
                    if state.accounts.get(&tx.tx.from).is_none() {
                        validation_results.push(Some(anyhow::anyhow!("Account does not exist")));
                        continue;
                    }
                    if effective_balance < required_balance {
                        validation_results.push(Some(anyhow::anyhow!(
                            "Insufficient balance: have {:.6}, need {:.6}",
                            effective_balance, required_balance
                        )));
                        continue;
                    }
                    // Note: nonce check in batch is complex due to ordering, skip for now
                }
                validation_results.push(None); // Valid
            }
        }
        
        // PHASE 1: Pre-compute outside lock
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let now_secs = now_ms / 1000;

        // Pre-normalize client parents for each tx
        let client_parents_list: Vec<Vec<String>> = txs
            .iter()
            .map(|tx| {
                tx.tx
                    .parents
                    .iter()
                    .map(|p| {
                        if p.starts_with("rinku://tx/h/") {
                            p.strip_prefix("rinku://tx/h/").unwrap_or(p).to_string()
                        } else if p.starts_with("rinku://tx/") {
                            p.strip_prefix("rinku://tx/").unwrap_or(p).to_string()
                        } else {
                            p.clone()
                        }
                    })
                    .collect()
            })
            .collect();

        // Get account weights with read lock first
        let account_weights: std::collections::HashMap<String, f64> = {
            let state = self.inner.read().await;
            txs.iter()
                .map(|tx| {
                    let weight = if let Some(account) = state.accounts.get(&tx.tx.from) {
                        calculate_account_weight(account, now_secs)
                    } else {
                        1.0
                    };
                    (tx.tx.from.clone(), weight)
                })
                .collect()
        };

        // PHASE 2: Single write lock for entire batch - with tip injection
        let mut state = self.inner.write().await;
        let mut results = Vec::with_capacity(txs.len());

        for (idx, tx) in txs.iter().enumerate() {
            // Check if this tx failed pre-validation
            if let Some(err) = validation_results.get(idx).and_then(|r| r.as_ref()) {
                results.push(Err(anyhow::anyhow!("{}", err)));
                continue;
            }
            
            let client_parents = &client_parents_list[idx];
            let tx_weight = account_weights.get(&tx.tx.from).copied().unwrap_or(1.0);
            
            // CRITICAL FIX: Server-side tip injection for batch
            // Check which client parents exist in DAG
            let valid_parents: Vec<String> = client_parents
                .iter()
                .filter(|p| !p.is_empty() && state.dag.get_node(p).is_some())
                .cloned()
                .collect();
            
            // If no valid parents exist, inject current tips as parents
            let normalized_parents = if valid_parents.is_empty() {
                let current_tips = state.dag.tips();
                // Take up to 2 tips to reference
                current_tips.into_iter().take(2).collect()
            } else {
                valid_parents
            };
            
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            let node = rinku_core::types::DagNode {
                hash: tx.hash.clone(),
                tx: tx.clone(),
                parents: normalized_parents.clone(),
                children: Vec::new(),
                weight: tx_weight,
                finalized: false,
                checkpoint_height: None,
                received_at_ms: Some(now_ms),
            };
            
            let result = state
                .dag
                .add_node(node)
                .map_err(|e| anyhow::anyhow!("{}", e));
            if result.is_ok() {
                // FINALITY-FIRST MODEL: Do NOT modify balances/nonces here!
                // All state changes (balance, nonce, gas burn, rewards) happen in
                // execute_finalized_transaction() when the checkpoint finalizes.
                // This prevents double-execution bugs.
                
                // Track transaction counts for gas price adjustment only
                // total_transactions is only incremented upon finalization
                state.txs_this_period += 1;
            }
            results.push(result);
        }

        // Gas adjustment once per batch
        const PERIOD_MS: u64 = 15000;
        const TARGET_TPS: f64 = 1000.0;
        const MAX_CHANGE_PERCENT: f64 = 0.125;
        const ELASTICITY: f64 = 2.0;

        if now_ms - state.period_start_ms >= PERIOD_MS {
            let target_txs = TARGET_TPS * (PERIOD_MS as f64 / 1000.0);
            let utilization = state.txs_this_period as f64 / target_txs;
            let change_ratio = ((utilization - 1.0) / (ELASTICITY - 1.0)).clamp(-1.0, 1.0);
            let change_factor = 1.0 + change_ratio * MAX_CHANGE_PERCENT;
            state.current_gas_price = (state.current_gas_price * change_factor).clamp(
                state.config.gas.min_gas_price,
                state.config.gas.max_gas_price,
            );
            state.txs_this_period = 0;
            state.period_start_ms = now_ms;
        }
        
        drop(state);
        
        // FINALITY-FIRST MODEL: No reward/stake processing here!
        // All rewards and stake operations are processed in execute_finalized_transaction()
        // when the checkpoint finalizes. This prevents double-execution bugs.

        results
    }

    pub async fn get_transaction(&self, hash: &str) -> Option<SignedTransaction> {
        let state = self.inner.read().await;
        state.dag.get_node(hash).map(|n| n.tx.clone())
    }
    
    /// Quick check if a transaction exists in the DAG (O(1) lookup)
    pub async fn has_transaction(&self, hash: &str) -> bool {
        let state = self.inner.read().await;
        state.dag.get_node(hash).is_some()
    }
    
    /// Get recent transactions from the DAG (up to limit)
    /// Used to flush local transactions to peers before snapshot sync
    pub async fn get_recent_transactions(&self, limit: usize) -> Vec<SignedTransaction> {
        let state = self.inner.read().await;
        state.dag
            .get_all_nodes()
            .into_iter()
            .take(limit)
            .map(|n| n.tx.clone())
            .collect()
    }
    
    /// Get transactions involving an address (as sender or recipient)
    /// Returns transactions sorted by timestamp (newest first)
    pub async fn get_transactions_by_address(&self, address: &str, limit: usize) -> Vec<(SignedTransaction, bool)> {
        let state = self.inner.read().await;
        let mut txs: Vec<_> = state.dag
            .get_all_nodes()
            .into_iter()
            .filter(|n| n.tx.tx.from == address || n.tx.tx.to == address)
            .map(|n| {
                let finalized = n.finalized;
                (n.tx.clone(), finalized)
            })
            .collect();
        
        // Sort by timestamp descending (newest first)
        txs.sort_by(|a, b| b.0.tx.timestamp.cmp(&a.0.tx.timestamp));
        txs.truncate(limit);
        txs
    }
    
    /// Get transaction with its weight from the DAG node
    pub async fn get_transaction_with_weight(&self, hash: &str) -> Option<(SignedTransaction, f64)> {
        let state = self.inner.read().await;
        let result = state.dag.get_node(hash).map(|n| (n.tx.clone(), n.weight));
        if result.is_none() {
            // Debug: check if hash is in all_nodes but not in index
            let all_hashes: Vec<_> = state.dag.get_all_nodes().iter().take(5).map(|n| &n.hash).collect();
            tracing::debug!("get_transaction_with_weight: hash '{}' not found. DAG has {} nodes. Sample hashes: {:?}", 
                hash, state.dag.node_count(), all_hashes);
        }
        result
    }

    pub async fn is_finalized(&self, hash: &str) -> bool {
        let state = self.inner.read().await;
        state
            .dag
            .get_node(hash)
            .map(|n| n.finalized)
            .unwrap_or(false)
    }

    pub async fn get_validators(&self) -> Vec<Validator> {
        let state = self.inner.read().await;
        state.validators.values().cloned().collect()
    }
    
    /// Check if an address is a registered validator
    pub async fn is_validator(&self, address: &str) -> bool {
        let state = self.inner.read().await;
        state.validators.contains_key(address)
    }
    
    /// Get the stake amount for a validator
    pub async fn get_validator_stake(&self, address: &str) -> Option<f64> {
        let state = self.inner.read().await;
        state.validators.get(address).map(|v| v.stake)
    }
    
    /// Get total validator stake for fast-path quorum calculation
    pub async fn get_total_validator_stake(&self) -> f64 {
        let state = self.inner.read().await;
        state.validators.values().map(|v| v.stake).sum()
    }
    
    /// Get the validators as a HashMap for syncing to the ValidatorIdentityService
    pub async fn get_validators_map(&self) -> std::collections::HashMap<String, Validator> {
        let state = self.inner.read().await;
        state.validators.clone()
    }
    
    /// Replace the entire validator set with genesis validators
    /// CRITICAL: This ensures state.validators and ValidatorIdentityService stay in sync
    /// when GENESIS_VALIDATORS env var is set. Prevents stale validators from persisting.
    pub async fn replace_validators_with_genesis(&self, genesis_validators: &[(String, Vec<u8>)]) {
        use crate::validator_identity::MIN_VALIDATOR_STAKE;
        
        let mut state = self.inner.write().await;
        let old_count = state.validators.len();
        
        // Build new validator map from genesis validators
        let mut new_validators = std::collections::HashMap::new();
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        for (address, bls_public_key) in genesis_validators {
            let validator = Validator {
                address: address.clone(),
                stake: MIN_VALIDATOR_STAKE,
                first_stake_time: now_secs * 1000,
                bls_public_key: Some(hex::encode(bls_public_key)),
                missed_checkpoints: 0,
            };
            new_validators.insert(address.clone(), validator);
        }
        
        let new_count = new_validators.len();
        state.validators = new_validators;
        
        info!(
            "state.validators: REPLACED with genesis validators ({} -> {})",
            old_count, new_count
        );
    }

    /// Merge validators from peer during delta sync
    /// This ensures all nodes converge to the same validator set for leader election
    /// Returns the number of validators added or updated
    pub async fn merge_validators_from_peer(
        &self,
        peer_validators: &std::collections::HashMap<String, Validator>,
    ) -> usize {
        let mut state = self.inner.write().await;
        let mut merged_count = 0;
        
        for (addr, peer_validator) in peer_validators {
            match state.validators.get_mut(addr) {
                Some(existing) => {
                    // Update BLS key if we don't have one but peer does
                    if existing.bls_public_key.is_none() && peer_validator.bls_public_key.is_some() {
                        existing.bls_public_key = peer_validator.bls_public_key.clone();
                        merged_count += 1;
                        info!("Updated BLS key for validator {} from peer", addr);
                    }
                    // Take higher stake value (peer might have more up-to-date stake info)
                    if peer_validator.stake > existing.stake {
                        existing.stake = peer_validator.stake;
                    }
                }
                None => {
                    // Add new validator from peer
                    state.validators.insert(addr.clone(), peer_validator.clone());
                    merged_count += 1;
                    info!("Added validator {} from peer (stake: {})", addr, peer_validator.stake);
                }
            }
        }
        
        merged_count
    }

    pub async fn get_finalization_info(&self, hash: &str) -> (bool, Option<u64>) {
        let state = self.inner.read().await;
        if let Some(node) = state.dag.get_node(hash) {
            (node.finalized, node.checkpoint_height)
        } else {
            (false, None)
        }
    }

    pub async fn get_merkle_proof(
        &self,
        tx_hash: &str,
        checkpoint_height: u64,
    ) -> Option<(Vec<String>, usize, Checkpoint)> {
        use rinku_core::merkle::MerkleTree;

        let state = self.inner.read().await;

        let checkpoint = state
            .checkpoints
            .iter()
            .find(|c| c.height == checkpoint_height)?
            .clone();

        // Use checkpoint's stored finalized_tx_hashes if available (new format)
        // Fall back to DAG query for backwards compatibility with old checkpoints
        let mut finalized_hashes: Vec<String> = if !checkpoint.finalized_tx_hashes.is_empty() {
            checkpoint.finalized_tx_hashes.clone()
        } else {
            state
                .dag
                .get_all_nodes()
                .into_iter()
                .filter(|n| n.finalized && n.checkpoint_height == Some(checkpoint_height))
                .map(|n| n.hash.clone())
                .collect()
        };

        if finalized_hashes.is_empty() {
            return None;
        }

        // CRITICAL: Sort hashes for deterministic merkle tree computation
        // This MUST match the order used in checkpoint creation
        finalized_hashes.sort();

        let index = finalized_hashes.iter().position(|h| h == tx_hash)?;

        let tree = MerkleTree::from_hex_leaves(&finalized_hashes).ok()?;
        let merkle_proof = tree.get_proof(index).ok()?;

        Some((merkle_proof.siblings, index, checkpoint))
    }

    pub async fn get_dag_merkle_root(&self) -> Option<String> {
        use rinku_core::merkle::MerkleTree;

        let state = self.inner.read().await;
        let tips = state.dag.tips();

        if tips.is_empty() {
            return None;
        }

        let tree = MerkleTree::from_hex_leaves(&tips).ok()?;
        Some(tree.root())
    }

    pub async fn get_txs_since_checkpoint(
        &self,
        from_checkpoint: u64,
        missing_hashes: &[String],
    ) -> Vec<SignedTransaction> {
        let state = self.inner.read().await;

        // Return current DAG transactions for sync
        // Include ALL transactions currently in the DAG
        // Preserve DAG topological order - don't sort, as that could break parent-child relationships
        state
            .dag
            .get_all_nodes()
            .into_iter()
            .filter(|n| {
                if !missing_hashes.is_empty() {
                    missing_hashes.contains(&n.hash)
                } else {
                    // Include unfinalized transactions OR from checkpoints at/after from_checkpoint
                    // Use >= to include transactions AT the requested checkpoint (not just after)
                    n.checkpoint_height
                        .map(|h| h >= from_checkpoint)
                        .unwrap_or(true)
                }
            })
            .map(|n| n.tx.clone())
            .collect()
    }

    /// Get a state snapshot for sync (accounts, checkpoints, recent DAG)
    /// This is the efficient way to sync - transfer state, not full tx history
    pub async fn get_sync_snapshot(&self) -> SyncSnapshot {
        let state = self.inner.read().await;

        // Get ALL current DAG transactions for sync (including finalized ones)
        // Peers need all transactions that are still in our DAG window
        // Preserve DAG topological order - don't sort, as that could break parent-child relationships
        let all_nodes = state.dag.get_all_nodes();
        let dag_txs: Vec<SignedTransaction> = all_nodes
            .iter()
            .map(|n| n.tx.clone())
            .collect();
        
        // Collect finalized transaction hashes so peers can restore finality status
        let finalized_tx_hashes: Vec<String> = all_nodes
            .iter()
            .filter(|n| n.finalized)
            .map(|n| n.hash.clone())
            .collect();
        
        // Map transaction hashes to their checkpoint heights for proper proof generation
        let tx_checkpoint_heights: HashMap<String, u64> = all_nodes
            .iter()
            .filter_map(|n| n.checkpoint_height.map(|h| (n.hash.clone(), h)))
            .collect();

        // Get contracts from state
        let contracts = state.contracts.clone();
        let total_burned = state.total_burned;
        let total_to_validators = state.total_to_validators;
        
        // Release state lock before acquiring service locks
        drop(state);

        // Get service snapshots
        let rewards_snapshot = {
            let rewards = self.rewards.read().await;
            Some(rewards.to_json())
        };
        
        let emission_snapshot = {
            let emission = self.emission.read().await;
            Some(emission.to_json())
        };
        
        let slashing_snapshot = {
            let slashing = self.slashing.read().await;
            Some(slashing.to_json())
        };

        // Re-acquire state lock to get the rest
        let state = self.inner.read().await;
        
        // Use persisted genesis hash if available, otherwise compute from DAG
        let genesis_hash = if let Some(ref hash) = state.genesis_hash {
            Some(hash.clone())
        } else {
            let mut found = None;
            for node in state.dag.get_all_nodes() {
                if node.tx.tx.from == "genesis" {
                    found = Some(node.hash.clone());
                    break;
                }
            }
            if found.is_none() {
                if let Some(first_checkpoint) = state.checkpoints.first() {
                    found = Some(first_checkpoint.tx_merkle_root.clone());
                }
            }
            found
        };

        SyncSnapshot {
            accounts: state.accounts.clone(),
            validators: state.validators.clone(),
            checkpoints: state.checkpoints.clone(),
            gas_price: state.current_gas_price,
            total_supply: state.total_supply,
            genesis_time: state.genesis_time,
            dag_transactions: dag_txs,
            total_transactions: state.total_transactions,
            contracts,
            rewards_snapshot,
            emission_snapshot,
            slashing_snapshot,
            total_burned,
            total_to_validators,
            genesis_hash,
            finalized_tx_hashes,
            tx_checkpoint_heights,
            weight_scores: {
                if let Some(ref wt) = state.weight_trie {
                    wt.all_weights().clone()
                } else {
                    HashMap::new()
                }
            },
        }
    }

    /// Apply a snapshot from a peer during sync
    /// This replaces local state with the peer's state (used for initial sync)
    /// Use force=true to apply even when checkpoint counts are equal (for recovery)
    pub async fn apply_sync_snapshot(&self, snapshot: SyncSnapshot) -> Result<usize> {
        self.apply_sync_snapshot_inner(snapshot, false).await
    }

    /// Force apply a snapshot (used for recovery when delta sync fails)
    pub async fn apply_sync_snapshot_force(&self, snapshot: SyncSnapshot) -> Result<usize> {
        self.apply_sync_snapshot_inner(snapshot, true).await
    }

    async fn apply_sync_snapshot_inner(&self, snapshot: SyncSnapshot, force: bool) -> Result<usize> {
        let mut state = self.inner.write().await;

        // CRITICAL: Compare checkpoint HEIGHT, not COUNT!
        // After pruning, both nodes may have 500 checkpoints but at different height ranges
        // e.g., local: 996-1496 (500), peer: 997-1497 (500) - same count, different height!
        let local_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
        let peer_height = snapshot.checkpoints.last().map(|cp| cp.height).unwrap_or(0);

        if !force && peer_height <= local_height && local_height > 0 {
            info!(
                "Skipping snapshot apply: local at height {}, peer at height {}",
                local_height, peer_height
            );
            return Ok(0);
        }
        
        if force {
            warn!(
                "RECOVERY MODE: Force applying snapshot ({} accounts, height {}) to fix state divergence",
                snapshot.accounts.len(), peer_height
            );
        }

        info!(
            "Applying sync snapshot: {} accounts, {} checkpoints, {} dag txs",
            snapshot.accounts.len(),
            snapshot.checkpoints.len(),
            snapshot.dag_transactions.len()
        );

        // AUTHORITATIVE MERGE: Peer with higher checkpoint is authoritative for account state
        // When syncing from a peer ahead in checkpoints, trust their account state completely
        // This ensures balance/stake/nonce all converge after checkpoint sync
        let mut merged_accounts = state.accounts.clone();
        let mut accounts_added = 0;
        let mut accounts_updated = 0;
        let mut accounts_balance_fixed = 0;
        
        // Track which local accounts are NOT in peer's snapshot
        // These need to be pushed back to the peer
        let peer_fingerprints: std::collections::HashSet<String> = 
            snapshot.accounts.keys().cloned().collect();
        let mut local_only_accounts: HashMap<String, Account> = HashMap::new();
        
        for (fingerprint, local_account) in state.accounts.iter() {
            if !peer_fingerprints.contains(fingerprint) {
                // This account exists locally but not on peer - need to push back
                local_only_accounts.insert(fingerprint.clone(), local_account.clone());
            }
        }
        
        for (fingerprint, peer_account) in snapshot.accounts.iter() {
            if let Some(local_account) = merged_accounts.get(fingerprint) {
                // Account exists in both - AUTHORITATIVE SYNC from peer
                // Peer with higher/equal checkpoint has authoritative state
                // Accept peer's full state (balance + stake + nonce) to fix divergence
                if peer_account.nonce > local_account.nonce {
                    // Peer has more transactions - take their state
                    merged_accounts.insert(fingerprint.clone(), peer_account.clone());
                    accounts_updated += 1;
                } else if peer_account.nonce == local_account.nonce {
                    // Same nonce but possibly different balance/stake
                    // Take peer's state since they're authoritative (higher checkpoint)
                    let balance_diff = (peer_account.balance - local_account.balance).abs();
                    let stake_diff = (peer_account.staked - local_account.staked).abs();
                    if balance_diff > 0.0001 || stake_diff > 0.0001 {
                        info!(
                            "Balance fix for {}: local={:.6} peer={:.6} (nonce={})",
                            &fingerprint[..fingerprint.len().min(12)], local_account.balance, peer_account.balance, peer_account.nonce
                        );
                        merged_accounts.insert(fingerprint.clone(), peer_account.clone());
                        accounts_balance_fixed += 1;
                    }
                }
                // If local has higher nonce, keep local (we have transactions peer doesn't know about)
            } else {
                // Account only exists on peer - add it
                merged_accounts.insert(fingerprint.clone(), peer_account.clone());
                accounts_added += 1;
            }
        }
        
        info!(
            "Account merge: {} added, {} updated (higher nonce), {} balance-fixed (same nonce), {} local-only, {} total",
            accounts_added, accounts_updated, accounts_balance_fixed, local_only_accounts.len(), merged_accounts.len()
        );
        
        state.accounts = merged_accounts;
        
        // VALIDATOR REPLACE: Replace validators entirely to ensure all nodes converge
        // This is critical for leader election - all nodes must have consistent validator sets
        let local_validator_addr = state.node_validator_address.clone();
        let local_validator_bls = state.node_bls_public_key.clone();
        let local_validator_backup = local_validator_addr.as_ref()
            .and_then(|addr| state.validators.get(addr).cloned());
        let old_validator_count = state.validators.len();
        
        // Start with peer's validators (REPLACE, not merge)
        let mut new_validators = snapshot.validators.clone();
        
        // CRITICAL: Re-register local validator to ensure we're always in the set
        if let Some(ref local_addr) = local_validator_addr {
            if !new_validators.contains_key(local_addr) {
                // Restore local validator from backup or create new
                if let Some(backup) = local_validator_backup {
                    info!("Re-adding local validator {} to synced set (from backup)", local_addr);
                    new_validators.insert(local_addr.clone(), backup);
                } else {
                    use crate::validator_identity::MIN_VALIDATOR_STAKE;
                    let now_secs = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let validator = Validator {
                        address: local_addr.clone(),
                        stake: MIN_VALIDATOR_STAKE,
                        first_stake_time: now_secs * 1000,
                        bls_public_key: local_validator_bls.clone(),
                        missed_checkpoints: 0,
                    };
                    info!("Re-registering local validator {} after snapshot sync", local_addr);
                    new_validators.insert(local_addr.clone(), validator);
                }
            } else if let Some(existing) = new_validators.get_mut(local_addr) {
                // Ensure our BLS key is set
                if existing.bls_public_key.is_none() && local_validator_bls.is_some() {
                    existing.bls_public_key = local_validator_bls.clone();
                }
            }
        }
        
        info!(
            "Validator sync: REPLACED validator set ({} -> {} validators)",
            old_validator_count, new_validators.len()
        );
        state.validators = new_validators;
        
        state.checkpoints = snapshot.checkpoints.clone();
        state.current_gas_price = snapshot.gas_price;
        state.total_supply = snapshot.total_supply;
        state.genesis_time = snapshot.genesis_time;
        state.total_transactions = snapshot.total_transactions;
        
        // Apply contracts state
        state.contracts = snapshot.contracts;
        state.total_burned = snapshot.total_burned;
        state.total_to_validators = snapshot.total_to_validators;
        
        // Apply weight scores (trust attestations)
        if !snapshot.weight_scores.is_empty() {
            let weight_count = snapshot.weight_scores.len();
            if let Some(ref mut wt) = state.weight_trie {
                wt.load_weights(snapshot.weight_scores);
            } else {
                let mut wt = WeightTrie::new();
                wt.load_weights(snapshot.weight_scores);
                state.weight_trie = Some(wt);
            }
            info!("Applied {} transaction weight scores from sync snapshot", weight_count);
        }
        
        // Store service snapshots for application after releasing state lock
        let rewards_to_apply = snapshot.rewards_snapshot;
        let emission_to_apply = snapshot.emission_snapshot;
        let slashing_to_apply = snapshot.slashing_snapshot;

        // Update checkpoint timestamp from latest checkpoint
        // Note: checkpoint.timestamp is in seconds, convert to milliseconds
        if let Some(latest_cp) = snapshot.checkpoints.last() {
            state.last_checkpoint_time_ms = latest_cp.timestamp * 1000;
        }

        // Reset DAG and rebuild with genesis + unfinalized transactions
        // Fresh DAG starts with genesis node as root (max 10000 nodes for synced state)
        state.dag = rinku_core::Dag::new(10000);

        // Create a synthetic genesis node that DAG transactions can reference
        let genesis_hash = "genesis".to_string();
        let genesis_tx = SignedTransaction {
            tx: rinku_core::types::Transaction {
                from: "genesis".to_string(),
                to: "genesis".to_string(),
                amount: 0.0,
                nonce: 0,
                timestamp: snapshot.genesis_time * 1000,
                parents: vec![],
                kind: None,
                gas_price: None,
                gas_limit: None,
                data: None,
                signature: None,
                memo: None,
                references: None,
            },
            hash: genesis_hash.clone(),
            signature: "genesis".to_string(),
        };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let genesis_node = rinku_core::types::DagNode {
            hash: genesis_hash.clone(),
            tx: genesis_tx,
            parents: vec![],
            children: Vec::new(),
            weight: 1.0,
            finalized: true,
            checkpoint_height: Some(0),
            received_at_ms: Some(now_ms),
        };
        let _ = state.dag.add_node(genesis_node);

        // Build a lookup of all transaction hashes in the snapshot
        // This allows us to preserve parent links even if insertion order differs
        let snapshot_hashes: std::collections::HashSet<String> = snapshot
            .dag_transactions
            .iter()
            .map(|tx| tx.hash.clone())
            .collect();
        
        // Build lookup of finalized transaction hashes from peer
        let finalized_hashes: std::collections::HashSet<String> = snapshot
            .finalized_tx_hashes
            .iter()
            .cloned()
            .collect();

        // Normalize parent references for all transactions
        let normalize_parent = |p: &str| -> String {
            if p.starts_with("rinku://tx/h/") {
                p.strip_prefix("rinku://tx/h/").unwrap_or(p).to_string()
            } else if p.starts_with("rinku://tx/") {
                p.strip_prefix("rinku://tx/").unwrap_or(p).to_string()
            } else {
                p.to_string()
            }
        };

        // Build normalized parent map for topological sorting
        let tx_parents: HashMap<String, Vec<String>> = snapshot
            .dag_transactions
            .iter()
            .map(|tx| {
                let parents: Vec<String> = tx
                    .tx
                    .parents
                    .iter()
                    .map(|p| normalize_parent(p))
                    .map(|p| {
                        if snapshot_hashes.contains(&p) {
                            p
                        } else {
                            genesis_hash.clone()
                        }
                    })
                    .collect();
                (tx.hash.clone(), parents)
            })
            .collect();

        // Topologically sort transactions (parents before children)
        // Uses Kahn's algorithm
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut children_map: HashMap<String, Vec<String>> = HashMap::new();

        for tx in &snapshot.dag_transactions {
            in_degree.entry(tx.hash.clone()).or_insert(0);
            for parent in tx_parents.get(&tx.hash).unwrap_or(&vec![]) {
                if snapshot_hashes.contains(parent) {
                    *in_degree.entry(tx.hash.clone()).or_insert(0) += 1;
                    children_map
                        .entry(parent.clone())
                        .or_default()
                        .push(tx.hash.clone());
                }
            }
        }

        // Start with nodes that have no parents in the snapshot (roots)
        let mut queue: Vec<String> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(hash, _)| hash.clone())
            .collect();

        let mut sorted_hashes: Vec<String> = Vec::new();
        while let Some(hash) = queue.pop() {
            sorted_hashes.push(hash.clone());
            if let Some(children) = children_map.get(&hash) {
                for child in children {
                    if let Some(deg) = in_degree.get_mut(child) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push(child.clone());
                        }
                    }
                }
            }
        }

        // Build hash-to-tx lookup for quick access
        let tx_lookup: HashMap<String, SignedTransaction> = snapshot
            .dag_transactions
            .into_iter()
            .map(|tx| (tx.hash.clone(), tx))
            .collect();

        // Add unfinalized DAG transactions in topological order
        // This ensures parents are inserted before children
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut added = 0;
        for hash in sorted_hashes {
            let tx = match tx_lookup.get(&hash) {
                Some(tx) => tx,
                None => continue,
            };

            let normalized_parents = tx_parents.get(&hash).cloned().unwrap_or_default();
            
            // Calculate weight from sender account
            let tx_weight = if let Some(account) = state.accounts.get(&tx.tx.from) {
                calculate_account_weight(account, now_secs)
            } else {
                1.0
            };

            // Check if this transaction was finalized on the peer
            let is_finalized = finalized_hashes.contains(&tx.hash);
            
            // Use the correct checkpoint height from the snapshot mapping
            let checkpoint_height = snapshot.tx_checkpoint_heights.get(&tx.hash).copied();
            
            let node = rinku_core::types::DagNode {
                hash: tx.hash.clone(),
                tx: tx.clone(),
                parents: normalized_parents,
                children: Vec::new(),
                weight: tx_weight,
                finalized: is_finalized,
                checkpoint_height,
                received_at_ms: Some(tx.tx.timestamp),
            };

            if state.dag.add_node(node).is_ok() {
                added += 1;
            }
        }

        let validator_count = state.validators.len();
        let checkpoint_count = state.checkpoints.len();
        let contract_count = state.contracts.len();
        
        // NOTE: We intentionally do NOT reconcile nonces after sync.
        // Account nonces from the peer are authoritative and represent finalized state.
        // Lowering nonces could allow replay attacks on already-finalized transactions.
        // 
        // The "stale nonce" rejections seen during sync are expected behavior:
        // - Old transactions in gossip that were already finalized on the peer
        // - The stale_nonce_cache will cache these and prevent log spam
        // 
        // The activity-bot should fetch current nonce from the node before submitting.
        
        // Release state lock before acquiring service locks
        drop(state);
        
        // Apply service snapshots if provided
        // Use merge_from for rewards to prevent double-claim exploit
        // (syncing from a peer that hasn't seen a claim yet would reset pending_rewards)
        if let Some(rewards_snap) = rewards_to_apply {
            let mut rewards = self.rewards.write().await;
            rewards.merge_from(rewards_snap);
            
            // CRITICAL: Reconcile account.staked with rewards stake positions
            // This fixes divergence where account.staked != rewards.stake_position.amount
            let stake_reconciliations: Vec<(String, f64, u64)> = rewards
                .get_all_stakes()
                .iter()
                .map(|pos| (pos.staker.clone(), pos.amount, pos.staked_at))
                .collect();
            drop(rewards);
            
            if !stake_reconciliations.is_empty() {
                let mut state = self.inner.write().await;
                let mut reconciled_count = 0;
                for (staker, amount, staked_at) in &stake_reconciliations {
                    if let Some(account) = state.accounts.get_mut(staker) {
                        let diff = (account.staked - amount).abs();
                        if diff > 0.0001 {
                            info!(
                                "Reconciling account.staked for {}: {} -> {} (diff: {:.4})",
                                &staker[..staker.len().min(12)],
                                account.staked,
                                amount,
                                diff
                            );
                            account.staked = *amount;
                            reconciled_count += 1;
                        }
                    }
                }
                if reconciled_count > 0 {
                    info!("Reconciled {} account stake values from rewards snapshot", reconciled_count);
                }
            }
        }
        
        // Use merge_from for emission to prevent double-emission exploits
        // (syncing from a peer with lower total_emitted would allow re-emission)
        if let Some(emission_snap) = emission_to_apply {
            let mut emission = self.emission.write().await;
            let (emitted_delta, burned_delta) = emission.merge_from(emission_snap);
            if emitted_delta > 0.0 || burned_delta > 0.0 {
                info!(
                    "Merged emission snapshot: +{:.6} emitted, +{:.6} burned",
                    emitted_delta, burned_delta
                );
            }
        }
        
        // Use merge_from for slashing to prevent unbonding/slashing rollback exploits
        // (syncing from a peer missing our unbonding request would lose it)
        if let Some(slashing_snap) = slashing_to_apply {
            let mut slashing = self.slashing.write().await;
            let result = slashing.merge_from(slashing_snap);
            if result.events_added > 0 || result.unbonding_added > 0 || result.liveness_updated > 0 {
                info!(
                    "Merged slashing snapshot: {} events added, {} unbonding added, {} liveness updated, +{:.6} total_slashed",
                    result.events_added, result.unbonding_added, result.liveness_updated, result.total_slashed_delta
                );
            }
        }

        info!(
            "Snapshot applied: {} DAG txs, {} validators, {} checkpoints, {} contracts",
            added, validator_count, checkpoint_count, contract_count
        );
        Ok(added)
    }

    pub async fn calculate_cumulative_weight(&self, hash: &str) -> f64 {
        let state = self.inner.read().await;
        state.dag.calculate_cumulative_weight(hash)
    }

    pub async fn prune_losing_branch(&self, loser_hash: &str) -> Result<usize> {
        let mut state = self.inner.write().await;

        if let Some(node) = state.dag.get_node(loser_hash) {
            if node.finalized {
                info!(
                    "Cannot prune finalized node {}",
                    &loser_hash[..16.min(loser_hash.len())]
                );
                return Ok(0);
            }
        }

        let removed_nodes = state.dag.prune_branch(loser_hash);
        let pruned_count = removed_nodes.len();

        for node in &removed_nodes {
            let tx = &node.tx.tx;

            if let Some(from_account) = state.accounts.get_mut(&tx.from) {
                from_account.balance += tx.amount;
                if let Some(gas_price) = tx.gas_price {
                    let gas_limit = tx.gas_limit.unwrap_or(21000);
                    from_account.balance += gas_price * gas_limit as f64;
                }
                if from_account.nonce > 0 {
                    from_account.nonce -= 1;
                }
            }

            if let Some(to_account) = state.accounts.get_mut(&tx.to) {
                to_account.balance -= tx.amount;
                if to_account.balance < 0.0 {
                    to_account.balance = 0.0;
                }
            }
        }

        info!(
            "Pruned {} transactions from losing branch starting at {}, reverted account balances",
            pruned_count,
            &loser_hash[..16.min(loser_hash.len())]
        );

        Ok(pruned_count)
    }

    pub async fn resolve_fork(
        &self,
        tip_a: &str,
        tip_b: &str,
    ) -> Option<(String, String, f64, f64)> {
        let state = self.inner.read().await;

        let weight_a = state.dag.calculate_cumulative_weight(tip_a);
        let weight_b = state.dag.calculate_cumulative_weight(tip_b);

        if (weight_a - weight_b).abs() < 0.001 {
            return None;
        }

        let (winner, loser) = if weight_a > weight_b {
            (tip_a.to_string(), tip_b.to_string())
        } else {
            (tip_b.to_string(), tip_a.to_string())
        };

        Some((winner, loser, weight_a, weight_b))
    }

    pub async fn store_contract(&self, contract: crate::contracts::ContractState) -> Result<()> {
        let mut state = self.inner.write().await;
        let contract_id = contract.contract_id.clone();
        state.contracts.insert(contract_id.clone(), contract);
        info!("Stored contract {}", contract_id);
        
        let contracts_data: Vec<_> = state.contracts.values().cloned().collect();
        drop(state);
        self.storage.save_contracts(&contracts_data)?;
        Ok(())
    }

    pub async fn get_contract(&self, contract_id: &str) -> Option<crate::contracts::ContractState> {
        let state = self.inner.read().await;
        state.contracts.get(contract_id).cloned()
    }

    pub async fn get_all_contracts(&self) -> Vec<crate::contracts::ContractState> {
        let state = self.inner.read().await;
        state.contracts.values().cloned().collect()
    }

    pub async fn update_contract_state(
        &self,
        contract_id: &str,
        new_state: std::collections::HashMap<String, serde_json::Value>,
        state_hash: String,
        new_height: u64,
    ) -> Result<()> {
        let mut state = self.inner.write().await;
        if let Some(contract) = state.contracts.get_mut(contract_id) {
            contract.state = new_state;
            contract.state_hash = state_hash;
            contract.height = new_height;
            info!("Updated contract {} state at height {}", contract_id, new_height);
            
            let contracts_data: Vec<_> = state.contracts.values().cloned().collect();
            drop(state);
            self.storage.save_contracts(&contracts_data)?;
            Ok(())
        } else {
            anyhow::bail!("Contract {} not found", contract_id)
        }
    }

    /// Reconcile account.staked values with rewards stake positions
    /// This fixes any divergence between the two data stores
    pub async fn reconcile_stakes(&self) -> (usize, Vec<(String, f64, f64)>) {
        let rewards = self.rewards.read().await;
        let stake_positions: Vec<(String, f64)> = rewards
            .get_all_stakes()
            .iter()
            .map(|pos| (pos.staker.clone(), pos.amount))
            .collect();
        drop(rewards);
        
        let mut state = self.inner.write().await;
        let mut reconciled_count = 0;
        let mut changes: Vec<(String, f64, f64)> = Vec::new();
        
        for (staker, rewards_amount) in &stake_positions {
            if let Some(account) = state.accounts.get_mut(staker) {
                let diff = (account.staked - rewards_amount).abs();
                if diff > 0.0001 {
                    info!(
                        "RECONCILE: account.staked for {}: {} -> {} (diff: {:.4})",
                        &staker[..staker.len().min(16)],
                        account.staked,
                        rewards_amount,
                        diff
                    );
                    changes.push((staker.clone(), account.staked, *rewards_amount));
                    account.staked = *rewards_amount;
                    reconciled_count += 1;
                }
            }
        }
        
        if reconciled_count > 0 {
            info!("Reconciled {} account stake values", reconciled_count);
        }
        
        (reconciled_count, changes)
    }

    /// Prune expired pending (unfinalized) transactions from the DAG
    /// Returns the count of transactions that were pruned
    /// This prevents indefinite mempool growth during checkpoint failures
    pub async fn prune_expired_pending_transactions(&self, cutoff_ms: u64) -> usize {
        let mut state = self.inner.write().await;
        
        // Collect hashes of expired unfinalized transactions
        let expired_hashes: Vec<String> = state
            .dag
            .get_all_nodes()
            .into_iter()
            .filter(|node| {
                // Only prune unfinalized transactions
                if node.finalized {
                    return false;
                }
                // Check if transaction has expired based on received_at_ms
                if let Some(received_at) = node.received_at_ms {
                    received_at < cutoff_ms
                } else {
                    // No timestamp - use transaction timestamp as fallback
                    // Convert to milliseconds if needed
                    let tx_ts = node.tx.tx.timestamp;
                    let ts_ms = if tx_ts < 4_000_000_000 {
                        tx_ts * 1000 // Seconds -> milliseconds
                    } else {
                        tx_ts // Already milliseconds
                    };
                    ts_ms < cutoff_ms
                }
            })
            .map(|node| node.hash.clone())
            .collect();

        let count = expired_hashes.len();
        
        // Remove each expired transaction from the DAG
        for hash in expired_hashes {
            if state.dag.remove_node(&hash).is_none() {
                warn!("Failed to remove expired tx {}: not found", &hash[..16.min(hash.len())]);
            }
        }
        
        count
    }
}
