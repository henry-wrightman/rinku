use anyhow::Result;
use rinku_core::{
    dag::Dag,
    types::{Account, Checkpoint, SignedTransaction, Validator},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use crate::config::NodeConfig;
use crate::emission::EmissionService;
use crate::persistence::PersistenceService;
use crate::rewards::RewardsService;
use crate::slashing::SlashingService;

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
}

use std::collections::VecDeque;

#[derive(Debug)]
pub struct StateInner {
    pub dag: Dag,
    pub accounts: HashMap<String, Account>,
    pub validators: HashMap<String, Validator>,
    pub checkpoints: Vec<Checkpoint>,
    pub current_gas_price: f64,
    pub total_supply: f64,
    pub genesis_time: u64,
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
}

#[derive(Clone)]
pub struct NodeState {
    config: NodeConfig,
    pub inner: Arc<RwLock<StateInner>>,
    persistence: Arc<PersistenceService>,
    pub emission: Arc<RwLock<EmissionService>>,
    pub slashing: Arc<RwLock<SlashingService>>,
    pub rewards: Arc<RwLock<RewardsService>>,
    start_time: std::time::Instant,
}

impl NodeState {
    pub async fn new(config: NodeConfig) -> Result<Self> {
        let persistence = PersistenceService::new(&config.data_dir)?;
        let persistence = Arc::new(persistence);

        let inner =
            if let Some((accounts, validators, checkpoints, gas_price, supply, genesis, txs)) =
                persistence.load_snapshot()?
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
                for tx in txs {
                    // Genesis transaction and txs from before checkpoints should be considered finalized
                    let is_genesis = tx.tx.from == "genesis";
                    let is_finalized = is_genesis || checkpoint_count > 0;
                    let node = rinku_core::types::DagNode {
                        hash: tx.hash.clone(),
                        tx: tx.clone(),
                        parents: tx.tx.parents.clone(),
                        children: Vec::new(),
                        weight: 1.0,
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
                StateInner {
                    dag,
                    accounts,
                    validators,
                    checkpoints,
                    current_gas_price: gas_price,
                    total_supply: supply,
                    genesis_time: genesis,
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
                }
            } else {
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
                let genesis_checkpoint = rinku_core::types::Checkpoint {
                    height: 0,
                    hash: rinku_core::sha256_hex(&format!("genesis-checkpoint:{}", genesis_time)),
                    previous_hash: None,
                    tx_merkle_root: genesis_hash.clone(),
                    state_root: "0".repeat(64),
                    receipt_root: "0".repeat(64),
                    tip_count: 1,
                    timestamp: genesis_time,
                    validator_signatures: vec![], // Will be updated when checkpoint service starts
                    aggregated_signature: None,
                    signer_bitmap: None,
                };
                
                StateInner {
                    dag,
                    accounts,
                    validators: HashMap::new(),
                    checkpoints: vec![genesis_checkpoint],
                    current_gas_price: config.gas.min_gas_price,
                    total_supply: config.tokenomics.genesis_allocation,
                    genesis_time,
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
                }
            };

        let emission = EmissionService::new();
        let slashing = SlashingService::new();

        // Load rewards from persistence or create fresh
        let rewards = if let Some(snapshot) = persistence.load_rewards()? {
            info!(
                "Restored rewards: {} stakes, {} pending",
                snapshot.stakes.len(),
                snapshot.pending_rewards.len()
            );
            RewardsService::from_json(snapshot)
        } else {
            RewardsService::new(crate::rewards::RewardConfig::default())
        };

        Ok(Self {
            config,
            inner: Arc::new(RwLock::new(inner)),
            persistence,
            emission: Arc::new(RwLock::new(emission)),
            slashing: Arc::new(RwLock::new(slashing)),
            rewards: Arc::new(RwLock::new(rewards)),
            start_time: std::time::Instant::now(),
        })
    }

    pub async fn save_snapshot(&self) -> Result<()> {
        let state = self.inner.read().await;
        let transactions: Vec<SignedTransaction> = state.dag.all_transactions();
        self.persistence.save_snapshot(
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
        self.persistence.save_rewards(&rewards_snapshot)?;

        Ok(())
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

    pub async fn get_account(&self, address: &str) -> Option<Account> {
        let state = self.inner.read().await;
        state.accounts.get(address).cloned()
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

    pub async fn get_total_transactions(&self) -> u64 {
        let state = self.inner.read().await;
        state.total_transactions
    }

    pub fn get_elapsed_seconds(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
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
        }
    }

    pub async fn add_transaction(&self, tx: SignedTransaction) -> Result<()> {
        // PHASE 1: Pre-compute everything outside the lock
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

        let node = rinku_core::types::DagNode {
            hash: tx.hash.clone(),
            tx: tx.clone(),
            parents: normalized_parents,
            children: Vec::new(),
            weight: 1.0,
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

        if let Some(from_account) = state.accounts.get_mut(&tx.tx.from) {
            from_account.balance -= tx.tx.amount + gas_fee;
            from_account.nonce = tx.tx.nonce + 1;
        }

        let to_account = state
            .accounts
            .entry(tx.tx.to.clone())
            .or_insert_with(|| Account::new(tx.tx.to.clone(), tx.tx.timestamp));
        to_account.balance += tx.tx.amount;

        // EIP-1559 tracking
        state.total_burned += gas_fee * 0.5;
        state.total_to_validators += gas_fee * 0.5;
        state.txs_this_period += 1;
        state.total_transactions += 1;

        // Period-based gas adjustment
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
        
        // Save transaction kind for stake processing after lock release
        let tx_kind = tx.tx.kind.clone();
        let from_addr = tx.tx.from.clone();
        let stake_amount = tx.tx.amount;
        
        drop(state);
        
        // Process stake/unstake transactions (separate lock for rewards)
        if let Some(kind) = tx_kind {
            use rinku_core::types::TransactionKind;
            match kind {
                TransactionKind::Stake => {
                    let mut rewards = self.rewards.write().await;
                    if let Err(e) = rewards.stake(&from_addr, stake_amount) {
                        tracing::warn!("Failed to process stake tx: {}", e);
                    } else {
                        tracing::debug!("Processed stake: {} staked {} RKU", &from_addr[..16.min(from_addr.len())], stake_amount);
                    }
                }
                TransactionKind::Unstake => {
                    let mut rewards = self.rewards.write().await;
                    match rewards.unstake(&from_addr) {
                        Ok(amount) => {
                            tracing::debug!("Processed unstake: {} unstaked {} RKU", &from_addr[..16.min(from_addr.len())], amount);
                        }
                        Err(e) => {
                            tracing::warn!("Failed to process unstake tx: {}", e);
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Batch add transactions - optimized for high throughput
    pub async fn add_transactions_batch(&self, txs: Vec<SignedTransaction>) -> Vec<Result<()>> {
        // PHASE 1: Pre-compute all nodes outside lock
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let prepared: Vec<_> = txs
            .iter()
            .map(|tx| {
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

                rinku_core::types::DagNode {
                    hash: tx.hash.clone(),
                    tx: tx.clone(),
                    parents: normalized_parents,
                    children: Vec::new(),
                    weight: 1.0,
                    finalized: false,
                    checkpoint_height: None,
                }
            })
            .collect();

        // PHASE 2: Single write lock for entire batch
        let mut state = self.inner.write().await;
        let mut results = Vec::with_capacity(txs.len());
        let mut stake_txs: Vec<(rinku_core::types::TransactionKind, String, f64)> = Vec::new();

        for (node, tx) in prepared.into_iter().zip(txs.iter()) {
            let result = state
                .dag
                .add_node(node)
                .map_err(|e| anyhow::anyhow!("{}", e));
            if result.is_ok() {
                let gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);

                if let Some(from_account) = state.accounts.get_mut(&tx.tx.from) {
                    from_account.balance -= tx.tx.amount + gas_fee;
                    from_account.nonce = tx.tx.nonce + 1;
                }

                let to_account = state
                    .accounts
                    .entry(tx.tx.to.clone())
                    .or_insert_with(|| Account::new(tx.tx.to.clone(), tx.tx.timestamp));
                to_account.balance += tx.tx.amount;

                state.total_burned += gas_fee * 0.5;
                state.total_to_validators += gas_fee * 0.5;
                state.txs_this_period += 1;
                state.total_transactions += 1;
                
                // Track stake/unstake transactions for processing after lock release
                if let Some(kind) = &tx.tx.kind {
                    stake_txs.push((kind.clone(), tx.tx.from.clone(), tx.tx.amount));
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
        
        // Process stake/unstake transactions (separate lock for rewards)
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
                        }
                    }
                    TransactionKind::Unstake => {
                        match rewards.unstake(&from_addr) {
                            Ok(unstaked) => {
                                tracing::debug!("Batch unstake: {} unstaked {} RKU", &from_addr[..16.min(from_addr.len())], unstaked);
                            }
                            Err(e) => {
                                tracing::warn!("Failed to process batch unstake tx: {}", e);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        results
    }

    pub async fn get_transaction(&self, hash: &str) -> Option<SignedTransaction> {
        let state = self.inner.read().await;
        state.dag.get_node(hash).map(|n| n.tx.clone())
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

        let finalized_hashes: Vec<String> = state
            .dag
            .get_all_nodes()
            .into_iter()
            .filter(|n| n.finalized && n.checkpoint_height == Some(checkpoint_height))
            .map(|n| n.hash.clone())
            .collect();

        if finalized_hashes.is_empty() {
            return None;
        }

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

        // Return only current DAG transactions (not full history)
        state
            .dag
            .get_all_nodes()
            .into_iter()
            .filter(|n| {
                if !missing_hashes.is_empty() {
                    missing_hashes.contains(&n.hash)
                } else {
                    // Include unfinalized transactions OR from checkpoints after from_checkpoint
                    n.checkpoint_height
                        .map(|h| h > from_checkpoint)
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

        // Get recent DAG transactions (only unfinalized ones for sync)
        let dag_txs: Vec<SignedTransaction> = state
            .dag
            .get_all_nodes()
            .into_iter()
            .filter(|n| !n.finalized)
            .map(|n| n.tx.clone())
            .collect();

        SyncSnapshot {
            accounts: state.accounts.clone(),
            validators: state.validators.clone(),
            checkpoints: state.checkpoints.clone(),
            gas_price: state.current_gas_price,
            total_supply: state.total_supply,
            genesis_time: state.genesis_time,
            dag_transactions: dag_txs,
            total_transactions: state.total_transactions,
        }
    }

    /// Apply a snapshot from a peer during sync
    /// This replaces local state with the peer's state (used for initial sync)
    pub async fn apply_sync_snapshot(&self, snapshot: SyncSnapshot) -> Result<usize> {
        let mut state = self.inner.write().await;

        // Only apply if peer has more checkpoints (more finalized history)
        let local_checkpoint_count = state.checkpoints.len();
        let peer_checkpoint_count = snapshot.checkpoints.len();

        if peer_checkpoint_count <= local_checkpoint_count && local_checkpoint_count > 0 {
            info!(
                "Skipping snapshot apply: local has {} checkpoints, peer has {}",
                local_checkpoint_count, peer_checkpoint_count
            );
            return Ok(0);
        }

        info!(
            "Applying sync snapshot: {} accounts, {} checkpoints, {} dag txs",
            snapshot.accounts.len(),
            snapshot.checkpoints.len(),
            snapshot.dag_transactions.len()
        );

        // Apply derived state from peer
        state.accounts = snapshot.accounts;
        state.validators = snapshot.validators;
        state.checkpoints = snapshot.checkpoints.clone();
        state.current_gas_price = snapshot.gas_price;
        state.total_supply = snapshot.total_supply;
        state.genesis_time = snapshot.genesis_time;
        state.total_transactions = snapshot.total_transactions;

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
        let mut added = 0;
        for hash in sorted_hashes {
            let tx = match tx_lookup.get(&hash) {
                Some(tx) => tx,
                None => continue,
            };

            let normalized_parents = tx_parents.get(&hash).cloned().unwrap_or_default();

            let node = rinku_core::types::DagNode {
                hash: tx.hash.clone(),
                tx: tx.clone(),
                parents: normalized_parents,
                children: Vec::new(),
                weight: 1.0,
                finalized: false,
                checkpoint_height: None,
            };

            if state.dag.add_node(node).is_ok() {
                added += 1;
            }
        }

        info!(
            "Snapshot applied: {} DAG transactions, {} validators, {} checkpoints",
            added,
            state.validators.len(),
            state.checkpoints.len()
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
}
