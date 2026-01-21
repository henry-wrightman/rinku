use anyhow::Result;
use rinku_core::{
    dag::Dag,
    types::{Account, Checkpoint, SignedTransaction, Validator},
    weight::calculate_account_weight,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::config::NodeConfig;
use crate::emission::EmissionService;
use crate::storage::RedbStorage;
use crate::rewards::RewardsService;
use crate::slashing::SlashingService;

#[cfg(feature = "p2p")]
use crate::network::{SyncRequest, SyncResponse, SnapshotData};

/// Try to fetch a snapshot from configured P2P bootstrap peers before creating genesis
/// This ensures non-genesis nodes sync from the network instead of creating their own chain
/// Uses exponential backoff retry for robustness - genesis node may still be starting
#[cfg(feature = "p2p")]
async fn try_presync_from_peers(bootstrap_peers: &[String], is_genesis_node: bool) -> Option<SyncSnapshot> {
    use libp2p::{
        identity::Keypair,
        noise, tcp, yamux,
        request_response::{self, cbor, ProtocolSupport},
        swarm::SwarmEvent,
        Multiaddr, PeerId, StreamProtocol,
    };
    use libp2p::futures::StreamExt;
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
        request_response::{self, cbor, ProtocolSupport},
        swarm::SwarmEvent,
        Multiaddr, PeerId, StreamProtocol,
    };
    use libp2p::futures::StreamExt;
    use std::time::Duration;
    
    // Parse bootstrap peers to extract peer IDs and addresses
    let mut targets: Vec<(PeerId, Multiaddr)> = Vec::new();
    for peer_str in bootstrap_peers {
        match peer_str.parse::<Multiaddr>() {
            Ok(addr) => {
                // Extract peer ID from multiaddr (last component should be /p2p/<peer_id>)
                let peer_id = addr.iter().find_map(|p| {
                    if let libp2p::multiaddr::Protocol::P2p(peer_id) = p {
                        Some(peer_id)
                    } else {
                        None
                    }
                });
                
                if let Some(peer_id) = peer_id {
                    // Remove /p2p/ component from address for dialing
                    let dial_addr: Multiaddr = addr.iter()
                        .filter(|p| !matches!(p, libp2p::multiaddr::Protocol::P2p(_)))
                        .collect();
                    info!("PRE-SYNC: Parsed peer {} at {}", peer_id, dial_addr);
                    targets.push((peer_id, dial_addr));
                } else {
                    warn!("PRE-SYNC: No peer ID in multiaddr: {}", peer_str);
                }
            }
            Err(e) => {
                warn!("PRE-SYNC: Failed to parse multiaddr '{}': {}", peer_str, e);
            }
        }
    }
    
    if targets.is_empty() {
        info!("PRE-SYNC: No valid bootstrap peers, will create genesis locally");
        return None;
    }
    
    // Create a temporary keypair for this sync connection
    let local_key = Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    info!("PRE-SYNC: Temporary peer ID: {}", local_peer_id);
    
    // Build minimal swarm for sync only
    let swarm_result = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .map(|builder| {
            builder.with_behaviour(|_| {
                cbor::Behaviour::<SyncRequest, SyncResponse>::new(
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
    for (peer_id, dial_addr) in targets {
        info!("PRE-SYNC: Dialing peer {} at {}...", peer_id, dial_addr);
        
        // Dial the peer
        if let Err(e) = swarm.dial(dial_addr.clone()) {
            warn!("PRE-SYNC: Failed to dial {}: {}", peer_id, e);
            continue;
        }
        
        // Wait for connection and send request
        let timeout = tokio::time::sleep(Duration::from_secs(15));
        tokio::pin!(timeout);
        
        let mut connected = false;
        let mut request_sent = false;
        
        loop {
            tokio::select! {
                _ = &mut timeout => {
                    warn!("PRE-SYNC: Timeout waiting for peer {}", peer_id);
                    break;
                }
                event = swarm.select_next_some() => {
                    match event {
                        SwarmEvent::ConnectionEstablished { peer_id: connected_peer, .. } => {
                            if connected_peer == peer_id {
                                info!("PRE-SYNC: Connected to {}", peer_id);
                                connected = true;
                                // Send snapshot request
                                swarm.behaviour_mut().send_request(&peer_id, SyncRequest::Snapshot);
                                request_sent = true;
                                info!("PRE-SYNC: Sent snapshot request to {}", peer_id);
                            }
                        }
                        SwarmEvent::Behaviour(request_response::Event::Message {
                            message: request_response::Message::Response { response, .. },
                            ..
                        }) => {
                            if let SyncResponse::Snapshot(snapshot_data) = response {
                                info!(
                                    "PRE-SYNC: Received snapshot: {} accounts, {} checkpoints, {} txs",
                                    snapshot_data.accounts.len(),
                                    snapshot_data.checkpoints.len(),
                                    snapshot_data.recent_txs.len()
                                );
                                
                                // Convert SnapshotData to SyncSnapshot
                                let sync_snapshot = convert_snapshot_data_to_sync_snapshot(snapshot_data);
                                return Some(sync_snapshot);
                            } else {
                                warn!("PRE-SYNC: Unexpected response type from peer");
                            }
                        }
                        SwarmEvent::OutgoingConnectionError { peer_id: Some(failed_peer), error, .. } => {
                            if failed_peer == peer_id {
                                warn!("PRE-SYNC: Connection failed to {}: {}", peer_id, error);
                                break;
                            }
                        }
                        SwarmEvent::ConnectionClosed { peer_id: closed_peer, .. } => {
                            if closed_peer == peer_id && request_sent {
                                warn!("PRE-SYNC: Connection closed before response from {}", peer_id);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    
    info!("PRE-SYNC: All bootstrap peers failed, will create genesis locally");
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

impl NodeState {
    pub fn storage(&self) -> &Arc<RedbStorage> {
        &self.storage
    }
    
    pub async fn new(config: NodeConfig) -> Result<Self> {
        let storage = RedbStorage::open(&config.data_dir)?;
        let storage = Arc::new(storage);

        let inner =
            if let Some((accounts, validators, checkpoints, gas_price, supply, genesis, txs)) =
                storage.load_snapshot()?
            {
                let tx_count = txs.len() as u64;
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
                for tx in txs {
                    // Genesis transaction and txs from before checkpoints should be considered finalized
                    let is_genesis = tx.tx.from == "genesis";
                    let is_finalized = is_genesis || checkpoint_count > 0;
                    // Calculate weight from sender account
                    let tx_weight = if let Some(account) = accounts.get(&tx.tx.from) {
                        calculate_account_weight(account, now_secs)
                    } else {
                        1.0
                    };
                    let node = rinku_core::types::DagNode {
                        hash: tx.hash.clone(),
                        tx: tx.clone(),
                        parents: tx.tx.parents.clone(),
                        children: Vec::new(),
                        weight: tx_weight,
                        finalized: is_finalized,
                        checkpoint_height: if is_genesis {
                            Some(0)
                        } else if is_finalized {
                            Some(checkpoint_count)
                        } else {
                            None
                        },
                    };
                    let _ = dag.add_node(node);
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
                }
            };

        let emission = EmissionService::new();
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
        let transactions: Vec<SignedTransaction> = state.dag.all_transactions();
        self.storage.save_snapshot(
            &state.accounts,
            &state.validators,
            &state.checkpoints,
            state.current_gas_price,
            state.total_supply,
            state.genesis_time,
            &transactions,
        )?;

        // Also save rewards/staking state
        let rewards = self.rewards.read().await;
        let rewards_snapshot = rewards.to_json();
        drop(rewards);
        self.storage.save_rewards(&rewards_snapshot)?;

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

    pub async fn set_validator_info(&self, address: Option<String>, bls_public_key: Option<String>) {
        let mut state = self.inner.write().await;
        state.node_validator_address = address;
        state.node_bls_public_key = bls_public_key;
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
    /// Adds new accounts and updates existing ones if peer has higher nonce.
    /// Returns (accounts_added, accounts_updated)
    pub async fn merge_accounts_from_peer(&self, accounts: HashMap<String, Account>) -> (usize, usize) {
        let mut state = self.inner.write().await;
        let mut added = 0;
        let mut updated = 0;
        
        for (fingerprint, peer_account) in accounts {
            if let Some(local_account) = state.accounts.get_mut(&fingerprint) {
                // Account exists locally - update if peer has higher nonce
                if peer_account.nonce > local_account.nonce {
                    *local_account = peer_account;
                    updated += 1;
                }
            } else {
                // Account doesn't exist locally - add it
                state.accounts.insert(fingerprint, peer_account);
                added += 1;
            }
        }
        
        if added > 0 || updated > 0 {
            info!(
                "Merged accounts from peer: {} added, {} updated, {} total accounts",
                added, updated, state.accounts.len()
            );
        }
        
        (added, updated)
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
        let all_nodes = state.dag.get_all_nodes();
        let finalized = all_nodes.iter().filter(|n| n.finalized).count();
        let unfinalized = all_nodes.len() - finalized;
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

        // Use aggregate stats for accurate average (not biased by rolling window)
        let avg = if state.finality_count > 0 {
            state.finality_sum_ms as f64 / state.finality_count as f64
        } else {
            0.0
        };

        if state.finality_times_ms.is_empty() {
            return (avg, avg, avg, last_checkpoint_age, checkpoints_per_minute);
        }

        // Use rolling window for percentile calculations
        let mut times: Vec<u64> = state.finality_times_ms.iter().copied().collect();
        times.sort();

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
        state.checkpoints.len() as u64
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
            })
            .collect();
        
        (nodes, total, has_more)
    }

    /// Combined dashboard stats - single lock acquisition for all Explorer stats
    pub async fn get_dashboard_stats(&self) -> DashboardStats {
        let state = self.inner.read().await;
        let all_nodes = state.dag.get_all_nodes();
        let finalized_count = all_nodes.iter().filter(|n| n.finalized).count();
        let unfinalized_count = all_nodes.len() - finalized_count;
        
        let latest_checkpoint_id = state.checkpoints.last().map(|cp| cp.hash.clone());
        
        DashboardStats {
            dag_nodes: all_nodes.len(),
            tip_count: state.dag.tip_count(),
            account_count: state.accounts.len(),
            checkpoint_height: state.checkpoints.len() as u64,
            finalized_count,
            unfinalized_count,
            total_transactions: state.total_transactions,
            tips: state.dag.tips(),
            gas_price: state.current_gas_price,
            total_burned: state.total_burned,
            avg_gas: state.current_gas_price, // Could compute from history if needed
            latest_checkpoint_id,
        }
    }

    pub async fn add_transaction(&self, tx: SignedTransaction) -> Result<()> {
        // PHASE 0: Validate transaction BEFORE any state mutations
        let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
        let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
        let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
        
        // Pre-check balance and stake minimum validation
        {
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
            
            // Calculate required balance based on transaction type
            let required_balance = if is_stake_tx {
                tx.tx.amount + gas_fee // Stake: need amount + gas
            } else if is_unstake_tx || is_claim_tx {
                gas_fee // Unstake/Claim: only need gas
            } else {
                tx.tx.amount + gas_fee // Transfer: need amount + gas
            };
            
            // Check sender account
            if tx.tx.from != "genesis" {
                match state.accounts.get(&tx.tx.from) {
                    Some(account) => {
                        if account.balance < required_balance {
                            tracing::warn!(
                                "Transaction rejected: insufficient balance. Have {:.6}, need {:.6} (amount: {:.6}, gas: {:.6})",
                                account.balance, required_balance, tx.tx.amount, gas_fee
                            );
                            return Err(anyhow::anyhow!(
                                "Insufficient balance: have {:.6}, need {:.6}",
                                account.balance, required_balance
                            ));
                        }
                        if tx.tx.nonce != account.nonce {
                            tracing::warn!(
                                "Transaction rejected: invalid nonce. Expected {}, got {}",
                                account.nonce, tx.tx.nonce
                            );
                            return Err(anyhow::anyhow!(
                                "Invalid nonce: expected {}, got {}",
                                account.nonce, tx.tx.nonce
                            ));
                        }
                    }
                    None => {
                        tracing::warn!(
                            "Transaction rejected: account {} does not exist",
                            &tx.tx.from[..16.min(tx.tx.from.len())]
                        );
                        return Err(anyhow::anyhow!("Account does not exist"));
                    }
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

        let node = rinku_core::types::DagNode {
            hash: tx.hash.clone(),
            tx: tx.clone(),
            parents: normalized_parents.clone(),
            children: Vec::new(),
            weight: tx_weight,
            finalized: false,
            checkpoint_height: None,
        };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // PHASE 2: Minimal write lock - just state mutation
        let mut state = self.inner.write().await;

        state.dag.add_node(node)?;

        let gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);

        // Check transaction kind to determine balance handling
        let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
        let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
        let is_claim_tx_inner = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
        
        if let Some(from_account) = state.accounts.get_mut(&tx.tx.from) {
            // For stake: deduct amount (locks it in stake)
            // For unstake/claim: don't deduct amount (only gas)
            // For transfer: deduct amount (sends to recipient)
            if is_stake_tx {
                from_account.balance -= tx.tx.amount + gas_fee;
            } else if is_unstake_tx || is_claim_tx_inner {
                from_account.balance -= gas_fee; // Only gas for unstake/claim
            } else {
                from_account.balance -= tx.tx.amount + gas_fee;
            }
            from_account.nonce = tx.tx.nonce + 1;
        }

        // For stake/unstake/claim: don't transfer to recipient (amount is handled by rewards/staking)
        // For transfer: add to recipient balance
        if !is_stake_tx && !is_unstake_tx && !is_claim_tx_inner {
            let to_account = state
                .accounts
                .entry(tx.tx.to.clone())
                .or_insert_with(|| Account::new(tx.tx.to.clone(), tx.tx.timestamp));
            to_account.balance += tx.tx.amount;
        }

        // EIP-1559 tracking
        state.total_burned += gas_fee * 0.5;
        state.total_to_validators += gas_fee * 0.5;
        state.txs_this_period += 1;
        state.total_transactions += 1;

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
        
        // Save transaction kind for stake processing after lock release
        let tx_kind = tx.tx.kind.clone();
        let from_addr = tx.tx.from.clone();
        let stake_amount = tx.tx.amount;
        let tx_amount = tx.tx.amount;
        let tx_hash = tx.hash.clone();
        let tx_url = format!("rinku://tx/h/{}", tx_hash);
        
        // Collect parent tx creators for witness rewards (while we have the lock)
        let parent_creators: Vec<(String, String)> = normalized_parents
            .iter()
            .filter_map(|parent_hash| {
                state.dag.get_node(parent_hash).map(|node| {
                    let parent_url = format!("rinku://tx/h/{}", parent_hash);
                    (parent_url, node.tx.tx.from.clone())
                })
            })
            .collect();
        
        // Get validator address for tip rewards
        let validator_addr = state.node_validator_address.clone();
        
        drop(state);
        
        // Process tip and witness rewards (separate lock for rewards service)
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
                // Don't reward yourself for referencing your own transactions
                if parent_creator != &from_addr {
                    rewards.process_witness_reward(&tx_url, parent_url, parent_creator, reward_base);
                }
            }
            
            drop(rewards);
        }
        
        // Process stake/unstake transactions (separate lock for rewards)
        if let Some(kind) = tx_kind {
            use rinku_core::types::TransactionKind;
            match kind {
                TransactionKind::Stake => {
                    let stake_update: Option<(f64, u64)> = {
                        let mut rewards = self.rewards.write().await;
                        if let Err(e) = rewards.stake(&from_addr, stake_amount) {
                            tracing::warn!("Failed to process stake tx: {}", e);
                            None
                        } else {
                            tracing::debug!("Processed stake: {} staked {} RKU", &from_addr[..16.min(from_addr.len())], stake_amount);
                            // Get stake position data before releasing lock
                            rewards.get_stake(&from_addr).map(|p| (p.amount, p.staked_at))
                        }
                    };
                    // Sync staked amount to account state (after releasing rewards lock)
                    if let Some((amount, staked_at)) = stake_update {
                        self.update_account_staked(&from_addr, amount, Some(staked_at / 1000)).await;
                    }
                }
                TransactionKind::Unstake => {
                    let unstake_result: Option<f64> = {
                        let mut rewards = self.rewards.write().await;
                        match rewards.unstake(&from_addr) {
                            Ok(amount) => {
                                tracing::debug!("Processed unstake: {} unstaked {} RKU", &from_addr[..16.min(from_addr.len())], amount);
                                Some(amount)
                            }
                            Err(e) => {
                                tracing::warn!("Failed to process unstake tx: {}", e);
                                None
                            }
                        }
                    };
                    // Sync staked amount (now 0) and return unstaked amount to balance
                    if let Some(unstaked_amount) = unstake_result {
                        // Return unstaked amount to user's balance
                        {
                            let mut state = self.inner.write().await;
                            if let Some(account) = state.accounts.get_mut(&from_addr) {
                                account.balance += unstaked_amount;
                                account.staked = 0.0;
                                tracing::info!(
                                    "Unstake completed: {} balance restored by {} RKU (new balance: {})",
                                    &from_addr[..16.min(from_addr.len())],
                                    unstaked_amount,
                                    account.balance
                                );
                            }
                        }
                    }
                }
                TransactionKind::ClaimRewards => {
                    let claimed: f64 = {
                        let mut rewards = self.rewards.write().await;
                        rewards.claim_rewards(&from_addr)
                    };
                    if claimed > 0.0 {
                        let mut state = self.inner.write().await;
                        if let Some(account) = state.accounts.get_mut(&from_addr) {
                            account.balance += claimed;
                            tracing::info!(
                                "Rewards claimed: {} received {} RKU (new balance: {})",
                                &from_addr[..16.min(from_addr.len())],
                                claimed,
                                account.balance
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(())
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

        let node = rinku_core::types::DagNode {
            hash: tx.hash.clone(),
            tx: tx.clone(),
            parents: normalized_parents,
            children: Vec::new(),
            weight: 1.0, // Default weight for synced transactions
            finalized: false,
            checkpoint_height: None,
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
                    match state.accounts.get(&tx.tx.from) {
                        Some(account) => {
                            if account.balance < required_balance {
                                validation_results.push(Some(anyhow::anyhow!(
                                    "Insufficient balance: have {:.6}, need {:.6}",
                                    account.balance, required_balance
                                )));
                                continue;
                            }
                            // Note: nonce check in batch is complex due to ordering, skip for now
                        }
                        None => {
                            validation_results.push(Some(anyhow::anyhow!("Account does not exist")));
                            continue;
                        }
                    }
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
        let mut stake_txs: Vec<(rinku_core::types::TransactionKind, String, f64)> = Vec::new();
        
        // Collect reward info for processing after lock release
        // (tx_url, from_addr, reward_base, normalized_parents, parent_creators)
        let mut reward_infos: Vec<(String, String, f64, Vec<String>, Vec<(String, String)>)> = Vec::new();
        let validator_addr = state.node_validator_address.clone();

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
            
            let node = rinku_core::types::DagNode {
                hash: tx.hash.clone(),
                tx: tx.clone(),
                parents: normalized_parents.clone(),
                children: Vec::new(),
                weight: tx_weight,
                finalized: false,
                checkpoint_height: None,
            };
            
            let result = state
                .dag
                .add_node(node)
                .map_err(|e| anyhow::anyhow!("{}", e));
            if result.is_ok() {
                let gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);
                
                // Check transaction kind to determine balance handling
                let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
                let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
                let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));

                if let Some(from_account) = state.accounts.get_mut(&tx.tx.from) {
                    // For stake: deduct amount (locks it in stake)
                    // For unstake/claim: don't deduct amount (only gas)
                    // For transfer: deduct amount (sends to recipient)
                    if is_stake_tx {
                        from_account.balance -= tx.tx.amount + gas_fee;
                    } else if is_unstake_tx || is_claim_tx {
                        from_account.balance -= gas_fee; // Only gas for unstake/claim
                    } else {
                        from_account.balance -= tx.tx.amount + gas_fee;
                    }
                    from_account.nonce = tx.tx.nonce + 1;
                }

                // For stake/unstake/claim: don't transfer to recipient (amount is handled by rewards/staking)
                // For transfer: add to recipient balance
                if !is_stake_tx && !is_unstake_tx && !is_claim_tx {
                    let to_account = state
                        .accounts
                        .entry(tx.tx.to.clone())
                        .or_insert_with(|| Account::new(tx.tx.to.clone(), tx.tx.timestamp));
                    to_account.balance += tx.tx.amount;
                }

                state.total_burned += gas_fee * 0.5;
                state.total_to_validators += gas_fee * 0.5;
                state.txs_this_period += 1;
                state.total_transactions += 1;
                
                // Track stake/unstake transactions for processing after lock release
                if let Some(kind) = &tx.tx.kind {
                    stake_txs.push((kind.clone(), tx.tx.from.clone(), tx.tx.amount));
                }
                
                // Collect reward info for this transaction
                let reward_base = tx.tx.amount + gas_fee;
                if reward_base > 0.0 {
                    let tx_url = format!("rinku://tx/h/{}", tx.hash);
                    let parent_creators: Vec<(String, String)> = normalized_parents
                        .iter()
                        .filter_map(|parent_hash| {
                            state.dag.get_node(parent_hash).map(|pnode| {
                                let parent_url = format!("rinku://tx/h/{}", parent_hash);
                                (parent_url, pnode.tx.tx.from.clone())
                            })
                        })
                        .collect();
                    reward_infos.push((tx_url, tx.tx.from.clone(), reward_base, normalized_parents.clone(), parent_creators));
                }
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
        
        // Process tip and witness rewards for all transactions in batch
        if !reward_infos.is_empty() {
            let mut rewards = self.rewards.write().await;
            for (tx_url, from_addr, reward_base, normalized_parents, parent_creators) in &reward_infos {
                // Tip reward: validator who included this transaction gets rewarded
                if let Some(ref validator) = validator_addr {
                    if let Some(first_parent) = normalized_parents.first() {
                        let tip_url = format!("rinku://tx/h/{}", first_parent);
                        rewards.process_tip_reward(tx_url, &tip_url, validator, *reward_base);
                    }
                }
                
                // Witness rewards: reward creators of referenced parent transactions
                for (parent_url, parent_creator) in parent_creators {
                    // Don't reward yourself for referencing your own transactions
                    if parent_creator != from_addr {
                        rewards.process_witness_reward(tx_url, parent_url, parent_creator, *reward_base);
                    }
                }
            }
            drop(rewards);
        }
        
        // Process stake/unstake transactions (separate lock for rewards)
        // Collect account updates to apply after releasing rewards lock
        // (address, staked_amount, staked_at, unstaked_amount_to_return)
        let mut account_stake_updates: Vec<(String, f64, Option<u64>, f64)> = Vec::new();
        
        if !stake_txs.is_empty() {
            use rinku_core::types::TransactionKind;
            let mut rewards = self.rewards.write().await;
            for (kind, from_addr, amount) in stake_txs {
                match kind {
                    TransactionKind::Stake => {
                        if let Err(e) = rewards.stake(&from_addr, amount) {
                            tracing::warn!("Failed to process batch stake tx: {}", e);
                        } else {
                            tracing::debug!("Batch stake: {} staked {} RKU", &from_addr[..16.min(from_addr.len())], amount);
                            // Track account update for after lock release
                            if let Some(position) = rewards.get_stake(&from_addr) {
                                account_stake_updates.push((from_addr.clone(), position.amount, Some(position.staked_at / 1000), 0.0));
                            }
                        }
                    }
                    TransactionKind::Unstake => {
                        match rewards.unstake(&from_addr) {
                            Ok(unstaked) => {
                                tracing::debug!("Batch unstake: {} unstaked {} RKU", &from_addr[..16.min(from_addr.len())], unstaked);
                                // Track account update for after lock release - return unstaked amount to balance
                                account_stake_updates.push((from_addr.clone(), 0.0, None, unstaked));
                            }
                            Err(e) => {
                                tracing::warn!("Failed to process batch unstake tx: {}", e);
                            }
                        }
                    }
                    TransactionKind::ClaimRewards => {
                        let claimed = rewards.claim_rewards(&from_addr);
                        if claimed > 0.0 {
                            tracing::debug!("Batch claim rewards: {} claimed {} RKU", &from_addr[..16.min(from_addr.len())], claimed);
                            // Track account update to add claimed rewards to balance
                            account_stake_updates.push((from_addr.clone(), -1.0, None, claimed)); // -1.0 signals "don't update staked"
                        }
                    }
                    _ => {}
                }
            }
        }
        
        // Sync staked amounts and return unstaked/claimed balance (after releasing rewards lock)
        for (addr, staked_amount, staked_at, amount_to_add) in account_stake_updates {
            let mut state = self.inner.write().await;
            if let Some(account) = state.accounts.get_mut(&addr) {
                // Only update staked amount if >= 0 (negative signals "don't update staked", used for claim rewards)
                if staked_amount >= 0.0 {
                    account.staked = staked_amount;
                    if let Some(ts) = staked_at {
                        if account.first_seen == 0 {
                            account.first_seen = ts;
                        }
                    }
                }
                // Add claimed rewards or return unstaked amount to balance
                if amount_to_add > 0.0 {
                    account.balance += amount_to_add;
                    tracing::info!(
                        "Batch balance update: {} received {} RKU (new balance: {})",
                        &addr[..16.min(addr.len())],
                        amount_to_add,
                        account.balance
                    );
                }
            }
        }

        results
    }

    pub async fn get_transaction(&self, hash: &str) -> Option<SignedTransaction> {
        let state = self.inner.read().await;
        state.dag.get_node(hash).map(|n| n.tx.clone())
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
    
    /// Get transaction with its weight from the DAG node
    pub async fn get_transaction_with_weight(&self, hash: &str) -> Option<(SignedTransaction, f64)> {
        let state = self.inner.read().await;
        state.dag.get_node(hash).map(|n| (n.tx.clone(), n.weight))
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

        let mut finalized_hashes: Vec<String> = state
            .dag
            .get_all_nodes()
            .into_iter()
            .filter(|n| n.finalized && n.checkpoint_height == Some(checkpoint_height))
            .map(|n| n.hash.clone())
            .collect();

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

        // Only apply if peer has more checkpoints (more finalized history)
        // OR if force=true (recovery mode - delta sync failed, need fresh state)
        let local_checkpoint_count = state.checkpoints.len();
        let peer_checkpoint_count = snapshot.checkpoints.len();

        if !force && peer_checkpoint_count <= local_checkpoint_count && local_checkpoint_count > 0 {
            info!(
                "Skipping snapshot apply: local has {} checkpoints, peer has {}",
                local_checkpoint_count, peer_checkpoint_count
            );
            return Ok(0);
        }
        
        if force {
            warn!(
                "RECOVERY MODE: Force applying snapshot ({} accounts, {} checkpoints) to fix state divergence",
                snapshot.accounts.len(), peer_checkpoint_count
            );
        }

        info!(
            "Applying sync snapshot: {} accounts, {} checkpoints, {} dag txs",
            snapshot.accounts.len(),
            snapshot.checkpoints.len(),
            snapshot.dag_transactions.len()
        );

        // MERGE accounts from peer instead of replacing
        // This ensures accounts created on different nodes are preserved during sync
        // For conflicts, keep the account with higher nonce (more transactions processed)
        let mut merged_accounts = state.accounts.clone();
        let mut accounts_added = 0;
        let mut accounts_updated = 0;
        
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
                // Account exists in both - keep the one with higher nonce
                // Higher nonce means more transactions processed = more authoritative
                if peer_account.nonce > local_account.nonce {
                    merged_accounts.insert(fingerprint.clone(), peer_account.clone());
                    accounts_updated += 1;
                }
            } else {
                // Account only exists on peer - add it
                merged_accounts.insert(fingerprint.clone(), peer_account.clone());
                accounts_added += 1;
            }
        }
        
        info!(
            "Account merge: {} added from peer, {} updated (higher nonce), {} local-only to push back, {} total",
            accounts_added, accounts_updated, local_only_accounts.len(), merged_accounts.len()
        );
        
        state.accounts = merged_accounts;
        state.validators = snapshot.validators;
        state.checkpoints = snapshot.checkpoints.clone();
        state.current_gas_price = snapshot.gas_price;
        state.total_supply = snapshot.total_supply;
        state.genesis_time = snapshot.genesis_time;
        state.total_transactions = snapshot.total_transactions;
        
        // Apply contracts state
        state.contracts = snapshot.contracts;
        state.total_burned = snapshot.total_burned;
        state.total_to_validators = snapshot.total_to_validators;
        
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
            },
            hash: genesis_hash.clone(),
            signature: "genesis".to_string(),
        };

        let genesis_node = rinku_core::types::DagNode {
            hash: genesis_hash.clone(),
            tx: genesis_tx,
            parents: vec![],
            children: Vec::new(),
            weight: 1.0,
            finalized: true,
            checkpoint_height: Some(0),
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
            };

            if state.dag.add_node(node).is_ok() {
                added += 1;
            }
        }

        let validator_count = state.validators.len();
        let checkpoint_count = state.checkpoints.len();
        let contract_count = state.contracts.len();
        
        // Release state lock before acquiring service locks
        drop(state);
        
        // Apply service snapshots if provided
        // Use merge_from for rewards to prevent double-claim exploit
        // (syncing from a peer that hasn't seen a claim yet would reset pending_rewards)
        if let Some(rewards_snap) = rewards_to_apply {
            let mut rewards = self.rewards.write().await;
            rewards.merge_from(rewards_snap);
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
}
