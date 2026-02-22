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

use crate::bls::bls_verify;
use crate::config::NodeConfig;
use crate::consensus::VoteType;
use crate::emission::EmissionService;
use crate::storage::RedbStorage;
use crate::rewards::RewardsService;
use crate::slashing::SlashingService;

use std::collections::VecDeque;

mod presync;
mod accounts;
mod metadata;
mod dag;
mod checkpoints;
mod stats;
mod proofs;
mod transactions;
mod validators;
mod sync;
mod fork;
mod contracts;

#[derive(Debug, Clone, PartialEq)]
pub enum TransactionResult {
    Accepted,
    Buffered,
}

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
    
    let delays = [5, 10, 20, 40, 80, 80, 80, 80];
    
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
    #[serde(default)]
    pub tx_checkpoint_heights: HashMap<String, u64>,
    #[serde(default)]
    pub weight_scores: HashMap<String, AggregatedWeight>,
}

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
    pub finality_times_ms: VecDeque<u64>,
    pub finality_sum_ms: u64,
    pub finality_count: u64,
    pub finality_max_ms: u64,
    pub node_validator_address: Option<String>,
    pub node_bls_public_key: Option<String>,
    pub node_peer_id: Option<String>,
    pub node_listen_addr: Option<String>,
    pub finalized_tx_history: VecDeque<(u64, u64)>,
    pub has_synced_from_network: bool,
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
    pub loaded_from_persistence: bool,
}

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

        let persisted_snapshot = storage.load_snapshot()?;
        let loaded_from_persistence = persisted_snapshot.is_some();

        let inner =
            if let Some((accounts, validators, checkpoints, gas_price, supply, genesis, dag_entries)) =
                persisted_snapshot
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
                    
                let loaded_hashes: std::collections::HashSet<String> = dag_entries
                    .iter()
                    .map(|e| e.tx.hash.clone())
                    .collect();
                    
                for entry in dag_entries {
                    let is_genesis = entry.tx.tx.from == "genesis";
                    let is_finalized = entry.finalized || is_genesis || checkpoint_count > 0;
                    let tx_weight = if let Some(account) = accounts.get(&entry.tx.tx.from) {
                        calculate_account_weight(account, now_secs)
                    } else {
                        1.0
                    };
                    
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
                    has_synced_from_network: true,
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
                if let Some(snapshot) = presync::try_presync_from_peers(&config.p2p.bootstrap_peers, config.is_genesis_node).await {
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
                    
                    if let Some(ref genesis_hash) = snapshot.genesis_hash {
                        let _ = storage.save_genesis_hash(genesis_hash);
                        info!("PRE-SYNC: Saved peer genesis hash: {}", &genesis_hash[..16.min(genesis_hash.len())]);
                    }
                    
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
                        loaded_from_persistence: false,
                    };
                    
                    node_state.recalculate_dag_weights().await;
                    
                    return Ok(node_state);
                }
                
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
                
                let genesis_state_root = "0".repeat(64);
                let genesis_receipt_root = "0".repeat(64);
                let genesis_tip_count = 1u32;
                let genesis_checkpoint_hash = rinku_core::sha256_hex(&format!(
                    "{}:{}:{}:{}:{}:{}",
                    0,
                    genesis_hash,
                    genesis_state_root,
                    genesis_receipt_root,
                    genesis_tip_count,
                    genesis_time
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
                    validator_signatures: vec![],
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
                    has_synced_from_network: false,
                    weight_trie: Some(WeightTrie::new()),
                }
            };

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
            loaded_from_persistence,
        };
        
        node_state.recalculate_dag_weights().await;
        
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
}
