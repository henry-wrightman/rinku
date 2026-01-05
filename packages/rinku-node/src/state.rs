use anyhow::Result;
use rinku_core::{
    dag::Dag,
    types::{Account, Checkpoint, SignedTransaction, Validator},
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use crate::config::NodeConfig;
use crate::emission::EmissionService;
use crate::persistence::PersistenceService;
use crate::rewards::RewardsService;
use crate::slashing::SlashingService;

pub struct DagNodeInfo {
    pub hash: String,
    pub from: String,
    pub to: String,
    pub amount: f64,
    pub nonce: u64,
    pub ts: u64,
    pub parents: Vec<String>,
    pub finalized: bool,
    pub weight: f64,
}

#[derive(Debug)]
pub struct StateInner {
    pub dag: Dag,
    pub accounts: HashMap<String, Account>,
    pub validators: HashMap<String, Validator>,
    pub checkpoints: Vec<Checkpoint>,
    pub current_gas_price: f64,
    pub total_supply: f64,
    pub genesis_time: u64,
}

#[derive(Clone)]
pub struct NodeState {
    config: NodeConfig,
    pub inner: Arc<RwLock<StateInner>>,
    persistence: Arc<PersistenceService>,
    pub emission: Arc<RwLock<EmissionService>>,
    pub slashing: Arc<RwLock<SlashingService>>,
    pub rewards: Arc<RwLock<RewardsService>>,
}

impl NodeState {
    pub async fn new(config: NodeConfig) -> Result<Self> {
        let persistence = PersistenceService::new(&config.data_dir)?;
        let persistence = Arc::new(persistence);

        let inner = if let Some((accounts, validators, checkpoints, gas_price, supply, genesis, txs)) = 
            persistence.load_snapshot()? 
        {
            info!("Restored from snapshot: {} accounts, {} txs", accounts.len(), txs.len());
            let mut dag = Dag::new(config.max_dag_nodes);
            for tx in txs {
                let node = rinku_core::types::DagNode {
                    hash: tx.hash.clone(),
                    tx: tx.clone(),
                    parents: tx.tx.parents.clone(),
                    children: Vec::new(),
                    weight: 1.0,
                    finalized: false,
                    checkpoint_height: None,
                };
                let _ = dag.add_node(node);
            }
            StateInner {
                dag,
                accounts,
                validators,
                checkpoints,
                current_gas_price: gas_price,
                total_supply: supply,
                genesis_time: genesis,
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
            let genesis_hash = format!("genesis-{}", genesis_time);
            let genesis_tx = SignedTransaction {
                tx: rinku_core::types::Transaction {
                    from: "genesis".to_string(),
                    to: "faucet".to_string(),
                    amount: faucet_balance,
                    nonce: 0,
                    timestamp: genesis_time,
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
            info!("Genesis transaction created: {}", &genesis_hash[..16.min(genesis_hash.len())]);

            StateInner {
                dag,
                accounts,
                validators: HashMap::new(),
                checkpoints: Vec::new(),
                current_gas_price: config.gas.min_gas_price,
                total_supply: config.tokenomics.genesis_allocation,
                genesis_time,
            }
        };

        let emission = EmissionService::new();
        let slashing = SlashingService::new();
        let rewards = RewardsService::new(crate::rewards::RewardConfig::default());

        Ok(Self {
            config,
            inner: Arc::new(RwLock::new(inner)),
            persistence,
            emission: Arc::new(RwLock::new(emission)),
            slashing: Arc::new(RwLock::new(slashing)),
            rewards: Arc::new(RwLock::new(rewards)),
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
        Ok(())
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

    pub async fn get_checkpoint_height(&self) -> u64 {
        let state = self.inner.read().await;
        state.checkpoints.len() as u64
    }

    pub async fn get_gas_price(&self) -> f64 {
        let state = self.inner.read().await;
        state.current_gas_price
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
                nonce: n.tx.tx.nonce,
                ts: n.tx.tx.timestamp,
                parents: n.parents.clone(),
                finalized: n.finalized,
                weight: n.weight,
            })
            .collect()
    }

    pub async fn add_transaction(&self, tx: SignedTransaction) -> Result<()> {
        let mut state = self.inner.write().await;

        // Normalize parent URLs to just hashes
        // Parents can come as "rinku://tx/h/{hash}" or just "{hash}"
        let normalized_parents: Vec<String> = tx.tx.parents.iter()
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

        state.dag.add_node(node)?;

        if let Some(from_account) = state.accounts.get_mut(&tx.tx.from) {
            from_account.balance -= tx.tx.amount;
            from_account.nonce = tx.tx.nonce + 1;
        }

        let to_account = state
            .accounts
            .entry(tx.tx.to.clone())
            .or_insert_with(|| Account::new(tx.tx.to.clone(), tx.tx.timestamp));
        to_account.balance += tx.tx.amount;

        Ok(())
    }

    pub async fn get_transaction(&self, hash: &str) -> Option<SignedTransaction> {
        let state = self.inner.read().await;
        state.dag.get_node(hash).map(|n| n.tx.clone())
    }

    pub async fn is_finalized(&self, hash: &str) -> bool {
        let state = self.inner.read().await;
        state.dag.get_node(hash).map(|n| n.finalized).unwrap_or(false)
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

        let mut txs: Vec<SignedTransaction> = state
            .dag
            .get_all_nodes()
            .into_iter()
            .filter(|n| {
                if !missing_hashes.is_empty() {
                    missing_hashes.contains(&n.hash)
                } else {
                    n.checkpoint_height.map(|h| h > from_checkpoint).unwrap_or(true)
                }
            })
            .map(|n| n.tx.clone())
            .collect();

        txs.truncate(100);
        txs
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

    pub async fn resolve_fork(&self, tip_a: &str, tip_b: &str) -> Option<(String, String, f64, f64)> {
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
