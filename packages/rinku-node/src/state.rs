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

        let node = rinku_core::types::DagNode {
            hash: tx.hash.clone(),
            tx: tx.clone(),
            parents: tx.tx.parents.clone(),
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
}
