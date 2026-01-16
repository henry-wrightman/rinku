use anyhow::Result;
use rinku_core::types::{Account, Checkpoint, SignedTransaction, Validator};
use sled::Db;
use std::collections::HashMap;
use tracing::{debug, info, warn};
use crate::rewards::RewardsSnapshot;

pub struct PersistenceService {
    db: Db,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Snapshot {
    accounts: HashMap<String, Account>,
    validators: HashMap<String, Validator>,
    checkpoints: Vec<Checkpoint>,
    gas_price: f64,
    total_supply: f64,
    genesis_time: u64,
    transactions: Vec<SignedTransaction>,
}

impl PersistenceService {
    pub fn new(data_dir: &str) -> Result<Self> {
        let db_path = format!("{}/sled-db", data_dir);
        std::fs::create_dir_all(data_dir)?;
        let db = sled::open(&db_path)?;
        info!("Opened sled database at {}", db_path);
        Ok(Self { db })
    }

    pub fn save_snapshot(
        &self,
        accounts: &HashMap<String, Account>,
        validators: &HashMap<String, Validator>,
        checkpoints: &[Checkpoint],
        gas_price: f64,
        total_supply: f64,
        genesis_time: u64,
        transactions: &[SignedTransaction],
    ) -> Result<()> {
        let snapshot = Snapshot {
            accounts: accounts.clone(),
            validators: validators.clone(),
            checkpoints: checkpoints.to_vec(),
            gas_price,
            total_supply,
            genesis_time,
            transactions: transactions.to_vec(),
        };

        let data = serde_json::to_vec(&snapshot)?;
        self.db.insert("snapshot", data)?;
        self.db.flush()?;
        debug!("Saved snapshot: {} accounts, {} validators, {} checkpoints",
            accounts.len(), validators.len(), checkpoints.len());
        Ok(())
    }

    pub fn load_snapshot(&self) -> Result<Option<(
        HashMap<String, Account>,
        HashMap<String, Validator>,
        Vec<Checkpoint>,
        f64,
        f64,
        u64,
        Vec<SignedTransaction>,
    )>> {
        match self.db.get("snapshot")? {
            Some(data) => {
                let snapshot: Snapshot = serde_json::from_slice(&data)?;
                info!("Loaded snapshot: {} accounts, {} validators, {} checkpoints, {} txs",
                    snapshot.accounts.len(),
                    snapshot.validators.len(),
                    snapshot.checkpoints.len(),
                    snapshot.transactions.len()
                );
                Ok(Some((
                    snapshot.accounts,
                    snapshot.validators,
                    snapshot.checkpoints,
                    snapshot.gas_price,
                    snapshot.total_supply,
                    snapshot.genesis_time,
                    snapshot.transactions,
                )))
            }
            None => {
                warn!("No snapshot found, starting fresh");
                Ok(None)
            }
        }
    }

    pub fn save_transaction(&self, tx: &SignedTransaction) -> Result<()> {
        let key = format!("tx:{}", tx.hash);
        let data = serde_json::to_vec(tx)?;
        self.db.insert(key.as_bytes(), data)?;
        Ok(())
    }

    pub fn get_transaction(&self, hash: &str) -> Result<Option<SignedTransaction>> {
        let key = format!("tx:{}", hash);
        match self.db.get(key.as_bytes())? {
            Some(data) => {
                let tx: SignedTransaction = serde_json::from_slice(&data)?;
                Ok(Some(tx))
            }
            None => Ok(None),
        }
    }

    pub fn save_checkpoint(&self, checkpoint: &Checkpoint) -> Result<()> {
        let key = format!("cp:{}", checkpoint.height);
        let data = serde_json::to_vec(checkpoint)?;
        self.db.insert(key.as_bytes(), data)?;
        self.db.flush()?;
        Ok(())
    }

    pub fn get_checkpoint(&self, height: u64) -> Result<Option<Checkpoint>> {
        let key = format!("cp:{}", height);
        match self.db.get(key.as_bytes())? {
            Some(data) => {
                let checkpoint: Checkpoint = serde_json::from_slice(&data)?;
                Ok(Some(checkpoint))
            }
            None => Ok(None),
        }
    }

    pub fn flush(&self) -> Result<()> {
        self.db.flush()?;
        Ok(())
    }

    pub fn save_rewards(&self, snapshot: &RewardsSnapshot) -> Result<()> {
        let data = serde_json::to_vec(snapshot)?;
        self.db.insert("rewards", data)?;
        self.db.flush()?;
        debug!("Saved rewards snapshot: {} stakes", snapshot.stakes.len());
        Ok(())
    }

    pub fn load_rewards(&self) -> Result<Option<RewardsSnapshot>> {
        match self.db.get("rewards")? {
            Some(data) => {
                let snapshot: RewardsSnapshot = serde_json::from_slice(&data)?;
                info!("Loaded rewards snapshot: {} stakes, {} pending rewards",
                    snapshot.stakes.len(),
                    snapshot.pending_rewards.len()
                );
                Ok(Some(snapshot))
            }
            None => {
                warn!("No rewards snapshot found, starting fresh");
                Ok(None)
            }
        }
    }

    pub fn save_contracts(&self, contracts: &[crate::contracts::ContractState]) -> Result<()> {
        let data = serde_json::to_vec(contracts)?;
        self.db.insert("contracts", data)?;
        self.db.flush()?;
        debug!("Saved {} contracts", contracts.len());
        Ok(())
    }

    pub fn load_contracts(&self) -> Result<Vec<crate::contracts::ContractState>> {
        match self.db.get("contracts")? {
            Some(data) => {
                let contracts: Vec<crate::contracts::ContractState> = serde_json::from_slice(&data)?;
                info!("Loaded {} contracts from persistence", contracts.len());
                Ok(contracts)
            }
            None => {
                debug!("No contracts found in persistence");
                Ok(Vec::new())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_persistence_roundtrip() {
        let dir = tempdir().unwrap();
        let service = PersistenceService::new(dir.path().to_str().unwrap()).unwrap();

        let mut accounts = HashMap::new();
        accounts.insert("addr1".to_string(), Account::new("addr1".to_string(), 1000));

        service.save_snapshot(
            &accounts,
            &HashMap::new(),
            &[],
            0.001,
            6_000_000.0,
            1000,
            &[],
        ).unwrap();

        let loaded = service.load_snapshot().unwrap().unwrap();
        assert_eq!(loaded.0.len(), 1);
        assert!(loaded.0.contains_key("addr1"));
    }
}
