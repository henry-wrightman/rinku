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
use std::sync::atomic::{AtomicBool, AtomicU64};
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

pub(crate) mod presync;
mod accounts;
mod metadata;
mod dag;
pub(crate) mod checkpoints;
mod stats;
mod proofs;
mod transactions;
pub use transactions::{FastPathExecResult, FastPathExecEntry, FastPathBatchExecEntry};
mod validators;
mod sync;
mod fork;
mod contracts;
pub mod partition;
pub mod wal;

#[derive(Debug, Clone, PartialEq)]
pub enum TransactionResult {
    Accepted,
    Buffered,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSnapshot {
    pub accounts: HashMap<String, Account>,
    pub validators: HashMap<String, Validator>,
    pub checkpoints: Vec<Checkpoint>,
    pub gas_price: u64,
    pub total_supply: u64,
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
    pub total_burned: u64,
    #[serde(default)]
    pub total_to_validators: u64,
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
    pub amount: u64,
    pub fee: u64,
    pub nonce: u64,
    pub ts: u64,
    pub parents: Vec<String>,
    pub finalized: bool,
    pub weight: f64,
    pub kind: Option<rinku_core::types::TransactionKind>,
    pub sig: String,
    pub effective_amount: Option<u64>,
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
    pub gas_price: u64,
    pub total_burned: u64,
    pub avg_gas: u64,
    pub latest_checkpoint_id: Option<String>,
}

#[derive(Debug)]
pub struct StateInner {
    pub dag: Dag,
    pub accounts: HashMap<String, Account>,
    pub validators: HashMap<String, Validator>,
    pub checkpoints: Vec<Checkpoint>,
    pub contracts: HashMap<String, crate::contracts::ContractState>,
    pub current_gas_price: u64,
    pub total_supply: u64,
    pub genesis_time: u64,
    pub genesis_hash: Option<String>,
    pub total_burned: u64,
    pub total_to_validators: u64,
    pub _txs_this_period: u64,
    pub _period_start_ms: u64,
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
    pub partition_state: partition::PartitionState,
    pub checkpoint_accounts_snapshot: Option<(u64, HashMap<String, (u64, u64, u64)>)>,
    pub pre_checkpoint_accounts_snapshot: Option<(u64, HashMap<String, (u64, u64, u64)>)>,
    pub fast_path_finalized_txs: HashMap<String, FastPathFinalizedEntry>,
    pub fast_path_finalized_order: VecDeque<String>,
}

#[derive(Clone, Debug)]
pub struct FastPathFinalizedEntry {
    pub from: String,
    pub to: String,
    pub amount: u64,
    pub nonce: u64,
    pub gas_price: Option<u64>,
    pub kind: Option<rinku_core::types::TransactionKind>,
    pub hash: String,
    pub finalized_at_ms: u64,
}

impl StateInner {
    pub fn build_state_trie_from_accounts(accounts: &HashMap<String, Account>) -> crate::sparse_merkle_trie::SparseMerkleTrie {
        use crate::sparse_merkle_trie::{SparseMerkleTrie, hash_account_key};
        let mut trie = SparseMerkleTrie::new();
        for (addr, acc) in accounts {
            let key = hash_account_key(addr);
            let value = format!("account:{}:{}:{}:{}", addr, acc.balance, acc.nonce, acc.staked);
            let _ = trie.set(&key, value.into_bytes(), None);
        }
        trie
    }

    pub fn collect_trie_updates_for_addresses(&self, changed_addresses: &[String]) -> Vec<(String, u64, u64, u64)> {
        let mut updates = Vec::with_capacity(changed_addresses.len());
        for addr in changed_addresses {
            if let Some(acc) = self.accounts.get(addr) {
                updates.push((addr.clone(), acc.balance, acc.nonce, acc.staked));
            }
        }
        updates
    }

    pub fn clear_checkpoint_finalized_txs(
        &mut self,
        finalized_hashes: &std::collections::HashSet<String>,
    ) -> usize {
        let mut removed = 0usize;
        for hash in finalized_hashes {
            if self.fast_path_finalized_txs.remove(hash).is_some() {
                removed += 1;
            }
        }
        if !finalized_hashes.is_empty() {
            self.fast_path_finalized_order.retain(|h| !finalized_hashes.contains(h));
        }
        const MAX_FAST_PATH_POOL: usize = 5000;
        while self.fast_path_finalized_txs.len() > MAX_FAST_PATH_POOL {
            if let Some(oldest) = self.fast_path_finalized_order.pop_front() {
                self.fast_path_finalized_txs.remove(&oldest);
            } else {
                break;
            }
        }
        removed
    }

    pub fn compute_changed_accounts(
        &self,
        pre_snapshot: &HashMap<String, (u64, u64, u64)>,
    ) -> std::collections::HashSet<String> {
        let mut changed = std::collections::HashSet::new();
        for (addr, (old_bal, old_nonce, old_staked)) in pre_snapshot {
            if let Some(acc) = self.accounts.get(addr) {
                if acc.balance != *old_bal || acc.nonce != *old_nonce || acc.staked != *old_staked {
                    changed.insert(addr.clone());
                }
            } else {
                changed.insert(addr.clone());
            }
        }
        for addr in self.accounts.keys() {
            if !pre_snapshot.contains_key(addr) {
                changed.insert(addr.clone());
            }
        }
        changed
    }
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
    pub checkpoint_height_cache: Arc<AtomicU64>,
    pub dag_write_semaphore: Arc<tokio::sync::Semaphore>,
    deferred_batch_txs: Arc<tokio::sync::Mutex<Vec<rinku_core::types::SignedTransaction>>>,
    deferred_batch_retry_counts: Arc<tokio::sync::Mutex<std::collections::HashMap<String, u32>>>,
    pub wal: Arc<tokio::sync::Mutex<wal::WriteAheadLog>>,
    pub checkpoint_in_progress: Arc<std::sync::atomic::AtomicUsize>,
    pub checkpoint_complete_notify: Arc<tokio::sync::Notify>,
    pub state_trie: Arc<tokio::sync::Mutex<crate::sparse_merkle_trie::SparseMerkleTrie>>,
    pub compaction_in_progress: Arc<AtomicBool>,
    pub compaction_overlay: Arc<tokio::sync::Mutex<Vec<(String, u64, u64, u64)>>>,
}

pub struct StateRootWithProofs {
    pub state_root: String,
    pub proofs: std::collections::HashMap<String, rinku_core::types::AccountStateProof>,
    pub executed_tx_hashes: std::collections::HashSet<String>,
}

impl NodeState {
    pub fn storage(&self) -> &Arc<RedbStorage> {
        &self.storage
    }

    pub async fn update_trie_for_addrs(&self, changed_addresses: &[String]) {
        use crate::sparse_merkle_trie::hash_account_key;
        let updates = {
            let state = self.inner.read().await;
            state.collect_trie_updates_for_addresses(changed_addresses)
        };
        let record_overlay = {
            let mut trie = self.state_trie.lock().await;
            for (addr, balance, nonce, staked) in &updates {
                let key = hash_account_key(addr);
                let value = format!("account:{}:{}:{}:{}", addr, balance, nonce, staked);
                let _ = trie.set(&key, value.into_bytes(), None);
            }
            self.compaction_in_progress.load(std::sync::atomic::Ordering::Acquire)
        };
        if record_overlay && !updates.is_empty() {
            self.compaction_overlay.lock().await.extend(updates);
        }
    }

    pub async fn update_trie_with_tuples(&self, updates: &[(String, u64, u64, u64)]) {
        let record_overlay = {
            let mut trie = self.state_trie.lock().await;
            Self::apply_trie_updates_inline(&mut trie, updates);
            self.compaction_in_progress.load(std::sync::atomic::Ordering::Acquire)
        };
        if record_overlay && !updates.is_empty() {
            self.compaction_overlay.lock().await.extend(updates.iter().cloned());
        }
    }

    pub fn apply_trie_updates_inline(trie: &mut crate::sparse_merkle_trie::SparseMerkleTrie, updates: &[(String, u64, u64, u64)]) {
        use crate::sparse_merkle_trie::hash_account_key;
        for (addr, balance, nonce, staked) in updates {
            let key = hash_account_key(addr);
            let value = format!("account:{}:{}:{}:{}", addr, balance, nonce, staked);
            let _ = trie.set(&key, value.into_bytes(), None);
        }
    }

    pub async fn trie_root_hex(&self) -> String {
        self.state_trie.lock().await.root_hex()
    }

    /// Async SMT compaction: rebuild the trie off the checkpoint critical path.
    ///
    /// CAS-claims `compaction_in_progress`. Spawns a task that snapshots accounts
    /// under a brief read lock, rebuilds the trie on a blocking thread, then swaps
    /// it in under the trie mutex. Concurrent trie updates during the rebuild are
    /// recorded in `compaction_overlay` and replayed onto the new trie prior to
    /// the swap, guaranteeing no lost updates.
    pub fn maybe_spawn_compaction(&self, height: u64, threshold: usize) {
        use std::sync::atomic::Ordering;
        let inner = self.inner.clone();
        let state_trie = self.state_trie.clone();
        let flag = self.compaction_in_progress.clone();
        let overlay = self.compaction_overlay.clone();
        tokio::spawn(async move {
            let old_dirty = {
                let trie = state_trie.lock().await;
                trie.dirty_node_count()
            };
            if old_dirty < threshold {
                return;
            }
            if flag
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }
            let t_overall = std::time::Instant::now();

            let accounts_snapshot = {
                let guard = inner.read().await;
                guard.accounts.clone()
            };
            let snapshot_account_count = accounts_snapshot.len();

            let t_build = std::time::Instant::now();
            let new_trie = match tokio::task::spawn_blocking(move || {
                StateInner::build_state_trie_from_accounts(&accounts_snapshot)
            })
            .await
            {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("SMT compaction (async) at h={}: rebuild task failed: {}", height, e);
                    flag.store(false, Ordering::Release);
                    return;
                }
            };
            let build_ms = t_build.elapsed().as_millis();

            let t_swap = std::time::Instant::now();
            let mut new_trie = new_trie;
            let mut trie_guard = state_trie.lock().await;
            let overlay_entries: Vec<(String, u64, u64, u64)> = {
                let mut overlay_guard = overlay.lock().await;
                std::mem::take(&mut *overlay_guard)
            };
            let overlay_count = overlay_entries.len();
            Self::apply_trie_updates_inline(&mut new_trie, &overlay_entries);
            *trie_guard = new_trie;
            drop(trie_guard);
            flag.store(false, Ordering::Release);
            let swap_ms = t_swap.elapsed().as_millis();
            let total_ms = t_overall.elapsed().as_millis();
            tracing::info!(
                "SMT compaction (async) at h={}: {} accounts ({} dirty → overlay={} replayed), build={}ms swap={}ms total={}ms",
                height, snapshot_account_count, old_dirty, overlay_count, build_ms, swap_ms, total_ms
            );
        });
    }

    pub async fn rebuild_trie_from_accounts(&self) {
        let accounts = {
            let state = self.inner.read().await;
            state.accounts.clone()
        };
        let new_trie = StateInner::build_state_trie_from_accounts(&accounts);
        let mut trie = self.state_trie.lock().await;
        *trie = new_trie;
    }

    pub async fn get_chain_info(&self) -> (String, String) {
        let state = self.inner.read().await;
        (state.config.chain_id.clone(), state.config.network_id.clone())
    }
    
    pub async fn new(config: NodeConfig) -> Result<Self> {
        let wal_data_dir = config.data_dir.clone();
        let storage = RedbStorage::open(&config.data_dir)?;
        let storage = Arc::new(storage);

        let persisted_snapshot = storage.load_snapshot()?;
        let loaded_from_persistence = persisted_snapshot.is_some();

        let inner =
            if let Some((accounts, validators, checkpoints, gas_price, supply, genesis, dag_entries, persisted_total_tx)) =
                persisted_snapshot
            {
                let tx_count = persisted_total_tx.unwrap_or(dag_entries.len() as u64);
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
                        partition_epoch: None,
                        rolled_back: false,
                        fast_path_cert: entry.fast_path_cert.clone(),
                        effective_amount: None,
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
                
                {
                    let start = std::time::Instant::now();
                    let t = StateInner::build_state_trie_from_accounts(&accounts);
                    info!("Built state trie from {} accounts in {}ms (root: {})", accounts.len(), start.elapsed().as_millis(), t.root_hex());
                }
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
                    total_burned: 0,
                    total_to_validators: 0,
                    _txs_this_period: 0,
                    _period_start_ms: now_ms,
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
                    partition_state: partition::PartitionState::default(),
                    checkpoint_accounts_snapshot: None,
                    pre_checkpoint_accounts_snapshot: None,
                    fast_path_finalized_txs: HashMap::new(),
                    fast_path_finalized_order: VecDeque::new(),
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
                            partition_epoch: None,
                            rolled_back: false,
                            fast_path_cert: None,
                            effective_amount: None,
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
                    
                    {
                        let t = StateInner::build_state_trie_from_accounts(&snapshot.accounts);
                        info!("PRE-SYNC: Built state trie from {} accounts (root: {})", snapshot.accounts.len(), t.root_hex());
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
                        _txs_this_period: 0,
                        _period_start_ms: now_ms,
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
                        partition_state: partition::PartitionState::default(),
                        checkpoint_accounts_snapshot: None,
                        pre_checkpoint_accounts_snapshot: None,
                        fast_path_finalized_txs: HashMap::new(),
                        fast_path_finalized_order: VecDeque::new(),
                    };
                    
                    let mut emission = if let Some(em_snapshot) = snapshot.emission_snapshot {
                        EmissionService::from_json(em_snapshot)
                    } else {
                        EmissionService::new()
                    };
                    let restored_height = snapshot.checkpoints.last().map(|c| c.height).unwrap_or(0);
                    if restored_height > 0 {
                        emission.set_last_reward_height(restored_height);
                    }
                    
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
                    
                    let initial_height = inner.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
                    let node_state = Self {
                        config,
                        state_trie: Arc::new(tokio::sync::Mutex::new(
                            StateInner::build_state_trie_from_accounts(&inner.accounts)
                        )),
                        inner: Arc::new(RwLock::new(inner)),
                        storage,
                        emission: Arc::new(RwLock::new(emission)),
                        slashing: Arc::new(RwLock::new(slashing)),
                        rewards: Arc::new(RwLock::new(rewards)),
                        start_time: std::time::Instant::now(),
                        loaded_from_persistence: false,
                        checkpoint_height_cache: Arc::new(AtomicU64::new(initial_height)),
                        dag_write_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
                        deferred_batch_txs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                        deferred_batch_retry_counts: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
                        wal: {
                            let mut w = wal::WriteAheadLog::new(&wal_data_dir);
                            if let Err(e) = w.open() {
                                warn!("WAL: failed to open: {}", e);
                            }
                            Arc::new(tokio::sync::Mutex::new(w))
                        },
                        checkpoint_in_progress: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                        checkpoint_complete_notify: Arc::new(tokio::sync::Notify::new()),
                        compaction_in_progress: Arc::new(AtomicBool::new(false)),
                        compaction_overlay: Arc::new(tokio::sync::Mutex::new(Vec::new())),
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
                let faucet_balance: u64 = 100_000_000_000_000;
                accounts.insert(
                    "faucet".to_string(),
                    Account {
                        address: "faucet".to_string(),
                        balance: faucet_balance,
                        nonce: 0,
                        first_seen: genesis_time,
                        staked: 0,
                        unbonding: 0,
                        unbonding_release: None,
                        latest_balance_proof: None,
                        partition_violations: 0,
                        reputation_penalty: 0.0,
                        penalty_decay_checkpoint: None,
                        partition_budget: None,
                        partition_budget_spent: 0,
                        total_claimed: 0,
                    },
                );
                info!("Faucet account initialized with {} micro-RKU ({} RKU)", faucet_balance, faucet_balance / 100_000_000);

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
                        gas_price: Some(0),
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
                    partition_epoch: None,
                    rolled_back: false,
                    fast_path_cert: None,
                    effective_amount: None,
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
            provisional: false,
            partition_epoch: None,
            visible_stake_pct: None,
                    merge_report_hash: None,
                    view_change_certificate: None,
                    view: 0,
                };
                
                info!("Genesis hash created: {}", &genesis_hash[..16.min(genesis_hash.len())]);
                
                let _ = storage.save_genesis_hash(&genesis_hash);
                
                {
                    let t = StateInner::build_state_trie_from_accounts(&accounts);
                    info!("Genesis: Built state trie from {} accounts (root: {})", accounts.len(), t.root_hex());
                }
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
                    total_burned: 0,
                    total_to_validators: 0,
                    _txs_this_period: 0,
                    _period_start_ms: now_ms,
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
                    partition_state: partition::PartitionState::default(),
                    checkpoint_accounts_snapshot: None,
                    pre_checkpoint_accounts_snapshot: None,
                    fast_path_finalized_txs: HashMap::new(),
                    fast_path_finalized_order: VecDeque::new(),
                }
            };

        let mut emission = if let Some(snapshot) = storage.load_emission()? {
            info!(
                "Restored emission: {:.2} RKU emitted, {:.2} RKU burned",
                snapshot.total_emitted,
                snapshot.total_burned
            );
            EmissionService::from_json(snapshot)
        } else {
            EmissionService::new()
        };
        {
            let local_height = inner.checkpoints.last().map(|c| c.height).unwrap_or(0);
            emission.set_last_reward_height(local_height);
        }
        
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

        let initial_height = inner.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
        let node_state = Self {
            config,
            state_trie: Arc::new(tokio::sync::Mutex::new(
                StateInner::build_state_trie_from_accounts(&inner.accounts)
            )),
            inner: Arc::new(RwLock::new(inner)),
            storage,
            emission: Arc::new(RwLock::new(emission)),
            slashing: Arc::new(RwLock::new(slashing)),
            rewards: Arc::new(RwLock::new(rewards)),
            start_time: std::time::Instant::now(),
            loaded_from_persistence,
            checkpoint_height_cache: Arc::new(AtomicU64::new(initial_height)),
            dag_write_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
            deferred_batch_txs: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            deferred_batch_retry_counts: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            wal: {
                let mut w = wal::WriteAheadLog::new(&wal_data_dir);
                if let Err(e) = w.open() {
                    warn!("WAL: failed to open: {}", e);
                }
                Arc::new(tokio::sync::Mutex::new(w))
            },
            checkpoint_in_progress: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            checkpoint_complete_notify: Arc::new(tokio::sync::Notify::new()),
            compaction_in_progress: Arc::new(AtomicBool::new(false)),
            compaction_overlay: Arc::new(tokio::sync::Mutex::new(Vec::new())),
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
        
        {
            let mut wal_guard = node_state.wal.lock().await;
            match wal_guard.recover() {
                Ok(Some(recovery)) => {
                    match recovery.action {
                        wal::WalRecoveryAction::Replay => {
                            info!(
                                "WAL RECOVERY: replaying {} account updates from uncommitted checkpoint h={}",
                                recovery.account_updates.len(), recovery.height
                            );
                            let mut state = node_state.inner.write().await;
                            let local_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
                            if recovery.height > local_height {
                                let mut changed: Vec<String> = Vec::new();
                                for (addr, balance, nonce, staked) in &recovery.account_updates {
                                    changed.push(addr.clone());
                                    if let Some(acc) = state.accounts.get_mut(addr) {
                                        acc.balance = *balance;
                                        acc.nonce = *nonce;
                                        acc.staked = *staked;
                                    } else {
                                        state.accounts.insert(addr.clone(), rinku_core::types::Account {
                                            address: addr.clone(),
                                            balance: *balance,
                                            nonce: *nonce,
                                            first_seen: 0,
                                            staked: *staked,
                                            unbonding: 0,
                                            unbonding_release: None,
                                            latest_balance_proof: None,
                                            partition_violations: 0,
                                            reputation_penalty: 0.0,
                                            penalty_decay_checkpoint: None,
                                            partition_budget: None,
                                            partition_budget_spent: 0,
                                            total_claimed: 0,
                                        });
                                    }
                                }
                                let trie_updates = state.collect_trie_updates_for_addresses(&changed);
                                drop(state);
                                node_state.update_trie_with_tuples(&trie_updates).await;
                                info!(
                                    "WAL RECOVERY: applied {} account updates for h={}, committing WAL",
                                    changed.len(), recovery.height
                                );
                            } else {
                                info!(
                                    "WAL RECOVERY: h={} already applied (local={}), skipping replay",
                                    recovery.height, local_height
                                );
                                drop(state);
                            }
                            let _ = wal_guard.truncate();
                        }
                        wal::WalRecoveryAction::Rollback => {
                            info!(
                                "WAL RECOVERY: rolling back incomplete checkpoint h={} (no account updates)",
                                recovery.height
                            );
                            let _ = wal_guard.truncate();
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    warn!("WAL RECOVERY: failed: {} — starting clean", e);
                    let _ = wal_guard.truncate();
                }
            }
        }

        Ok(node_state)
    }
}
