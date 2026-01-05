use anyhow::Result;
use rinku_core::{
    dag::Dag,
    types::{Account, Checkpoint, SignedTransaction, Validator},
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::NodeConfig;

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
    pub config: NodeConfig,
    pub inner: Arc<RwLock<StateInner>>,
}

impl NodeState {
    pub async fn new(config: NodeConfig) -> Result<Self> {
        let genesis_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();

        let inner = StateInner {
            dag: Dag::new(config.max_dag_nodes),
            accounts: HashMap::new(),
            validators: HashMap::new(),
            checkpoints: Vec::new(),
            current_gas_price: config.gas.min_gas_price,
            total_supply: config.tokenomics.genesis_allocation,
            genesis_time,
        };

        Ok(Self {
            config,
            inner: Arc::new(RwLock::new(inner)),
        })
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
}
