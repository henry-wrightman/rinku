use anyhow::Result;
use redb::{Database, ReadableTable, TableDefinition, WriteTransaction};
use rinku_core::types::{Account, AggregatedWeight, Checkpoint, SignedTransaction, Validator};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, info, warn};

pub async fn blocking_io<F, R>(f: F) -> Result<R>
where
    F: FnOnce() -> Result<R> + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))?
}

pub async fn blocking_cpu<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .expect("spawn_blocking join error")
}

use crate::contracts::ContractState;
use crate::emission::EmissionSnapshot;
use crate::rewards::RewardsSnapshot;

/// DAG snapshot entry that stores a transaction along with its parent references.
/// This is necessary because SignedTransaction doesn't include DAG structure info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagSnapshotEntry {
    pub tx: SignedTransaction,
    pub parents: Vec<String>,
    #[serde(default)]
    pub finalized: bool,
    #[serde(default)]
    pub checkpoint_height: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_path_cert: Option<rinku_core::types::FastPathFinalizationCert>,
}

pub const TABLE_ACCOUNTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("accounts");
pub const TABLE_DAG: TableDefinition<&[u8], &[u8]> = TableDefinition::new("dag");
pub const TABLE_CHECKPOINTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("checkpoints");
pub const TABLE_VALIDATORS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("validators");
pub const TABLE_TRIE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("trie");
pub const TABLE_METADATA: TableDefinition<&[u8], &[u8]> = TableDefinition::new("metadata");
pub const TABLE_CONTRACTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("contracts");
pub const TABLE_REWARDS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("rewards");
pub const TABLE_EMISSION: TableDefinition<&[u8], &[u8]> = TableDefinition::new("emission");
pub const TABLE_WEIGHTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("weights");

pub const STORAGE_SCHEMA_VERSION: u32 = 1;

pub struct RedbStorage {
    db: Arc<RwLock<Database>>,
    #[allow(dead_code)]
    data_dir: String,
}

impl RedbStorage {
    pub fn open(data_dir: &str) -> Result<Self> {
        let db_path = format!("{}/redb.db", data_dir);
        std::fs::create_dir_all(data_dir)?;

        let db = Database::create(&db_path)?;

        {
            let write_txn = db.begin_write()?;
            let _ = write_txn.open_table(TABLE_ACCOUNTS);
            let _ = write_txn.open_table(TABLE_DAG);
            let _ = write_txn.open_table(TABLE_CHECKPOINTS);
            let _ = write_txn.open_table(TABLE_VALIDATORS);
            let _ = write_txn.open_table(TABLE_TRIE);
            let _ = write_txn.open_table(TABLE_METADATA);
            let _ = write_txn.open_table(TABLE_CONTRACTS);
            let _ = write_txn.open_table(TABLE_REWARDS);
            let _ = write_txn.open_table(TABLE_EMISSION);
            let _ = write_txn.open_table(TABLE_WEIGHTS);
            write_txn.commit()?;
        }

        info!("Opened redb database at {}", db_path);

        let storage = Self {
            db: Arc::new(RwLock::new(db)),
            data_dir: data_dir.to_string(),
        };

        storage.check_storage_version()?;

        Ok(storage)
    }

    fn check_storage_version(&self) -> Result<()> {
        let db = self.db.read().unwrap();
        let read_txn = db.begin_read()?;
        let stored_version: Option<u32> = {
            let table = read_txn.open_table(TABLE_METADATA)?;
            match table.get(b"storage_schema_version".as_slice())? {
                Some(data) => serde_json::from_slice(data.value()).ok(),
                None => None,
            }
        };

        match stored_version {
            None => {
                info!("No storage schema version found (fresh or pre-versioning database), setting to v{}", STORAGE_SCHEMA_VERSION);
                drop(read_txn);
                let write_txn = db.begin_write()?;
                {
                    let mut table = write_txn.open_table(TABLE_METADATA)?;
                    table.insert(
                        b"storage_schema_version".as_slice(),
                        serde_json::to_vec(&STORAGE_SCHEMA_VERSION)?.as_slice(),
                    )?;
                }
                write_txn.commit()?;
            }
            Some(v) if v > STORAGE_SCHEMA_VERSION => {
                anyhow::bail!(
                    "Storage schema version {} is newer than this binary supports (v{}). Refusing to start — upgrade the node binary or restore from a compatible backup.",
                    v, STORAGE_SCHEMA_VERSION
                );
            }
            Some(v) if v < STORAGE_SCHEMA_VERSION => {
                info!(
                    "Migrating storage schema from v{} to v{}",
                    v, STORAGE_SCHEMA_VERSION
                );
                drop(read_txn);
                let write_txn = db.begin_write()?;
                {
                    let mut table = write_txn.open_table(TABLE_METADATA)?;
                    table.insert(
                        b"storage_schema_version".as_slice(),
                        serde_json::to_vec(&STORAGE_SCHEMA_VERSION)?.as_slice(),
                    )?;
                }
                write_txn.commit()?;
                info!("Storage schema migration complete");
            }
            Some(v) => {
                info!("Storage schema version: v{}", v);
            }
        }

        Ok(())
    }

    fn db_read(&self) -> std::sync::RwLockReadGuard<'_, Database> {
        self.db.read().unwrap()
    }

    fn db_write(&self) -> std::sync::RwLockWriteGuard<'_, Database> {
        self.db.write().unwrap()
    }

    pub fn put_accounts<T: Serialize>(&self, key: &[u8], value: &T) -> Result<()> {
        let data = bincode::serialize(value)?;
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_ACCOUNTS)?;
            table.insert(key, data.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_accounts<T: DeserializeOwned>(&self, key: &[u8]) -> Result<Option<T>> {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_ACCOUNTS)?;
        match table.get(key)? {
            Some(data) => {
                let value: T = bincode::deserialize(data.value())?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    pub fn put_dag<T: Serialize>(&self, key: &[u8], value: &T) -> Result<()> {
        let data = serde_json::to_vec(value)?;
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_DAG)?;
            table.insert(key, data.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_dag<T: DeserializeOwned>(&self, key: &[u8]) -> Result<Option<T>> {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_DAG)?;
        match table.get(key)? {
            Some(data) => {
                let value: T = serde_json::from_slice(data.value())?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    pub fn delete_dag(&self, key: &[u8]) -> Result<()> {
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_DAG)?;
            table.remove(key)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn put_checkpoint<T: Serialize>(&self, key: &[u8], value: &T) -> Result<()> {
        let data = serde_json::to_vec(value)?;
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_CHECKPOINTS)?;
            table.insert(key, data.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_checkpoint<T: DeserializeOwned>(&self, key: &[u8]) -> Result<Option<T>> {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_CHECKPOINTS)?;
        match table.get(key)? {
            Some(data) => {
                let value: T = serde_json::from_slice(data.value())?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    pub fn delete_checkpoint(&self, key: &[u8]) -> Result<()> {
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_CHECKPOINTS)?;
            table.remove(key)?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn batch_delete_dag(&self, keys: &[Vec<u8>]) -> Result<usize> {
        if keys.is_empty() {
            return Ok(0);
        }
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        let mut deleted = 0;
        {
            let mut table = write_txn.open_table(TABLE_DAG)?;
            for key in keys {
                if table.remove(key.as_slice())?.is_some() {
                    deleted += 1;
                }
            }
        }
        write_txn.commit()?;
        Ok(deleted)
    }

    pub fn batch_delete_checkpoints(&self, heights: &[u64]) -> Result<usize> {
        if heights.is_empty() {
            return Ok(0);
        }
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        let mut deleted = 0;
        {
            let mut table = write_txn.open_table(TABLE_CHECKPOINTS)?;
            for height in heights {
                let key = height.to_be_bytes();
                if table.remove(key.as_slice())?.is_some() {
                    deleted += 1;
                }
            }
        }
        write_txn.commit()?;
        Ok(deleted)
    }

    pub fn put_trie<T: Serialize>(&self, key: &[u8], value: &T) -> Result<()> {
        let data = bincode::serialize(value)?;
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_TRIE)?;
            table.insert(key, data.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_trie<T: DeserializeOwned>(&self, key: &[u8]) -> Result<Option<T>> {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_TRIE)?;
        match table.get(key)? {
            Some(data) => {
                let value: T = bincode::deserialize(data.value())?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    pub fn put_metadata<T: Serialize>(&self, key: &[u8], value: &T) -> Result<()> {
        let data = bincode::serialize(value)?;
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_METADATA)?;
            table.insert(key, data.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_metadata<T: DeserializeOwned>(&self, key: &[u8]) -> Result<Option<T>> {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_METADATA)?;
        match table.get(key)? {
            Some(data) => {
                let value: T = bincode::deserialize(data.value())?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    #[allow(dead_code)]
    pub fn batch_write<F>(&self, operations: F) -> Result<()>
    where
        F: FnOnce(&WriteTransaction) -> Result<()>,
    {
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        operations(&write_txn)?;
        write_txn.commit()?;
        Ok(())
    }

    pub fn count_accounts(&self) -> Result<usize> {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_ACCOUNTS)?;
        let mut count = 0;
        for _ in table.iter()? {
            count += 1;
        }
        Ok(count)
    }

    pub fn count_dag(&self) -> Result<usize> {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_DAG)?;
        let mut count = 0;
        for _ in table.iter()? {
            count += 1;
        }
        Ok(count)
    }

    pub fn count_checkpoints(&self) -> Result<usize> {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_CHECKPOINTS)?;
        let mut count = 0;
        for _ in table.iter()? {
            count += 1;
        }
        Ok(count)
    }

    pub fn iter_accounts<T, F>(&self, mut callback: F) -> Result<()>
    where
        T: DeserializeOwned,
        F: FnMut(Vec<u8>, T),
    {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_ACCOUNTS)?;
        for entry in table.iter()? {
            let (key, value) = entry?;
            if let Ok(v) = bincode::deserialize::<T>(value.value()) {
                callback(key.value().to_vec(), v);
            }
        }
        Ok(())
    }

    pub fn iter_dag<T, F>(&self, mut callback: F) -> Result<()>
    where
        T: DeserializeOwned,
        F: FnMut(Vec<u8>, T),
    {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_DAG)?;
        for entry in table.iter()? {
            let (key, value) = entry?;
            if let Ok(v) = serde_json::from_slice::<T>(value.value()) {
                callback(key.value().to_vec(), v);
            }
        }
        Ok(())
    }

    pub fn iter_checkpoints<T, F>(&self, mut callback: F) -> Result<()>
    where
        T: DeserializeOwned,
        F: FnMut(Vec<u8>, T),
    {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_CHECKPOINTS)?;
        for entry in table.iter()? {
            let (key, value) = entry?;
            if let Ok(v) = serde_json::from_slice::<T>(value.value()) {
                callback(key.value().to_vec(), v);
            }
        }
        Ok(())
    }

    pub fn compact(&self) -> Result<()> {
        let mut db = self.db_write();
        db.compact()?;
        debug!("Database compacted");
        Ok(())
    }

    pub fn get_stats(&self) -> StorageStats {
        StorageStats {
            accounts_count: self.count_accounts().unwrap_or(0),
            dag_count: self.count_dag().unwrap_or(0),
            checkpoints_count: self.count_checkpoints().unwrap_or(0),
        }
    }

    pub fn save_snapshot(
        &self,
        accounts: &HashMap<String, Account>,
        validators: &HashMap<String, Validator>,
        checkpoints: &[Checkpoint],
        gas_price: u64,
        total_supply: u64,
        genesis_time: u64,
        dag_entries: &[DagSnapshotEntry],
        total_transactions: u64,
    ) -> Result<()> {
        let db = self.db_read();
        let write_txn = db.begin_write()?;

        {
            let mut table = write_txn.open_table(TABLE_ACCOUNTS)?;
            for (addr, account) in accounts {
                let data = serde_json::to_vec(account)?;
                table.insert(addr.as_bytes(), data.as_slice())?;
            }
        }

        {
            let mut table = write_txn.open_table(TABLE_VALIDATORS)?;
            for (addr, validator) in validators {
                let data = serde_json::to_vec(validator)?;
                table.insert(addr.as_bytes(), data.as_slice())?;
            }
        }

        {
            let mut table = write_txn.open_table(TABLE_CHECKPOINTS)?;
            for checkpoint in checkpoints {
                let key = checkpoint.height.to_be_bytes();
                let data = serde_json::to_vec(checkpoint)?;
                table.insert(key.as_slice(), data.as_slice())?;
            }
        }

        {
            let mut table = write_txn.open_table(TABLE_DAG)?;
            let current_hashes: std::collections::HashSet<&[u8]> =
                dag_entries.iter().map(|e| e.tx.hash.as_bytes()).collect();
            let stale_keys: Vec<Vec<u8>> = {
                let mut keys = Vec::new();
                let iter = table.iter()?;
                for result in iter {
                    if let Ok((key, _)) = result {
                        let k = key.value().to_vec();
                        if !current_hashes.contains(k.as_slice()) {
                            keys.push(k);
                        }
                    }
                }
                keys
            };
            if !stale_keys.is_empty() {
                debug!(
                    "Cleaning {} stale TABLE_DAG entries (keeping {})",
                    stale_keys.len(),
                    dag_entries.len()
                );
                for key in &stale_keys {
                    table.remove(key.as_slice())?;
                }
            }
            for entry in dag_entries {
                let data = serde_json::to_vec(entry)?;
                table.insert(entry.tx.hash.as_bytes(), data.as_slice())?;
            }
        }

        {
            let mut table = write_txn.open_table(TABLE_METADATA)?;
            table.insert(
                b"gas_price".as_slice(),
                serde_json::to_vec(&gas_price)?.as_slice(),
            )?;
            table.insert(
                b"total_supply".as_slice(),
                serde_json::to_vec(&total_supply)?.as_slice(),
            )?;
            table.insert(
                b"genesis_time".as_slice(),
                serde_json::to_vec(&genesis_time)?.as_slice(),
            )?;
            table.insert(
                b"total_transactions".as_slice(),
                serde_json::to_vec(&total_transactions)?.as_slice(),
            )?;
            table.insert(
                b"storage_schema_version".as_slice(),
                serde_json::to_vec(&STORAGE_SCHEMA_VERSION)?.as_slice(),
            )?;
            table.insert(
                b"last_node_version".as_slice(),
                serde_json::to_vec(&crate::versioning::NODE_VERSION)?.as_slice(),
            )?;
            table.insert(
                b"last_protocol_version".as_slice(),
                serde_json::to_vec(&crate::versioning::PROTOCOL_VERSION)?.as_slice(),
            )?;
        }

        write_txn.commit()?;
        debug!(
            "Saved snapshot: {} accounts, {} validators, {} checkpoints, {} txs",
            accounts.len(),
            validators.len(),
            checkpoints.len(),
            dag_entries.len()
        );
        Ok(())
    }

    pub fn load_snapshot(
        &self,
    ) -> Result<
        Option<(
            HashMap<String, Account>,
            HashMap<String, Validator>,
            Vec<Checkpoint>,
            u64,
            u64,
            u64,
            Vec<DagSnapshotEntry>,
            Option<u64>,
        )>,
    > {
        let db = self.db_read();
        let read_txn = db.begin_read()?;

        let gas_price: u64 = {
            let table = read_txn.open_table(TABLE_METADATA)?;
            match table.get(b"gas_price".as_slice())? {
                Some(data) => serde_json::from_slice(data.value())?,
                None => {
                    warn!("No snapshot found in redb, starting fresh");
                    return Ok(None);
                }
            }
        };

        let total_supply: u64 = {
            let table = read_txn.open_table(TABLE_METADATA)?;
            match table.get(b"total_supply".as_slice())? {
                Some(data) => serde_json::from_slice(data.value())?,
                None => return Ok(None),
            }
        };

        let genesis_time: u64 = {
            let table = read_txn.open_table(TABLE_METADATA)?;
            match table.get(b"genesis_time".as_slice())? {
                Some(data) => serde_json::from_slice(data.value())?,
                None => return Ok(None),
            }
        };

        let persisted_total_transactions: Option<u64> = {
            let table = read_txn.open_table(TABLE_METADATA)?;
            match table.get(b"total_transactions".as_slice())? {
                Some(data) => serde_json::from_slice(data.value()).ok(),
                None => None,
            }
        };

        let mut accounts = HashMap::new();
        {
            let table = read_txn.open_table(TABLE_ACCOUNTS)?;
            for entry in table.iter()? {
                let (key, value) = entry?;
                let addr = String::from_utf8_lossy(key.value()).to_string();
                let account: Account = serde_json::from_slice(value.value())?;
                accounts.insert(addr, account);
            }
        }

        let mut validators = HashMap::new();
        {
            let table = read_txn.open_table(TABLE_VALIDATORS)?;
            for entry in table.iter()? {
                let (key, value) = entry?;
                let addr = String::from_utf8_lossy(key.value()).to_string();
                let validator: Validator = serde_json::from_slice(value.value())?;
                validators.insert(addr, validator);
            }
        }

        let mut checkpoints = Vec::new();
        {
            let table = read_txn.open_table(TABLE_CHECKPOINTS)?;
            for entry in table.iter()? {
                let (_, value) = entry?;
                let checkpoint: Checkpoint = serde_json::from_slice(value.value())?;
                checkpoints.push(checkpoint);
            }
        }
        checkpoints.sort_by_key(|c| c.height);

        let mut dag_entries = Vec::new();
        {
            let table = read_txn.open_table(TABLE_DAG)?;
            for entry in table.iter()? {
                let (_, value) = entry?;
                // Try to deserialize as new DagSnapshotEntry format first
                match serde_json::from_slice::<DagSnapshotEntry>(value.value()) {
                    Ok(entry) => dag_entries.push(entry),
                    Err(_) => {
                        // Fall back to old SignedTransaction format (backward compatibility)
                        if let Ok(tx) = serde_json::from_slice::<SignedTransaction>(value.value()) {
                            dag_entries.push(DagSnapshotEntry {
                                tx,
                                parents: Vec::new(),
                                finalized: true,
                                checkpoint_height: None,
                                fast_path_cert: None,
                            });
                        }
                    }
                }
            }
        }

        info!(
            "Loaded snapshot: {} accounts, {} validators, {} checkpoints, {} txs",
            accounts.len(),
            validators.len(),
            checkpoints.len(),
            dag_entries.len()
        );

        Ok(Some((
            accounts,
            validators,
            checkpoints,
            gas_price,
            total_supply,
            genesis_time,
            dag_entries,
            persisted_total_transactions,
        )))
    }

    pub fn save_genesis_hash(&self, genesis_hash: &str) -> Result<()> {
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_METADATA)?;
            table.insert(b"genesis_hash".as_slice(), genesis_hash.as_bytes())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn load_genesis_hash(&self) -> Result<Option<String>> {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_METADATA)?;
        match table.get(b"genesis_hash".as_slice())? {
            Some(data) => Ok(Some(String::from_utf8_lossy(data.value()).to_string())),
            None => Ok(None),
        }
    }

    pub fn save_rewards(&self, snapshot: &RewardsSnapshot) -> Result<()> {
        let data = serde_json::to_vec(snapshot)?;
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_REWARDS)?;
            table.insert(b"rewards_snapshot".as_slice(), data.as_slice())?;
        }
        write_txn.commit()?;
        debug!("Saved rewards snapshot: {} stakes", snapshot.stakes.len());
        Ok(())
    }

    pub fn load_rewards(&self) -> Result<Option<RewardsSnapshot>> {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_REWARDS)?;
        match table.get(b"rewards_snapshot".as_slice())? {
            Some(data) => {
                let snapshot: RewardsSnapshot = serde_json::from_slice(data.value())?;
                info!(
                    "Loaded rewards snapshot: {} stakes, {} pending rewards",
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

    pub fn save_emission(&self, snapshot: &EmissionSnapshot) -> Result<()> {
        let data = serde_json::to_vec(snapshot)?;
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_EMISSION)?;
            table.insert(b"emission_snapshot".as_slice(), data.as_slice())?;
        }
        write_txn.commit()?;
        debug!(
            "Saved emission snapshot: emitted={:.2}, burned={:.2}",
            snapshot.total_emitted, snapshot.total_burned
        );
        Ok(())
    }

    pub fn load_emission(&self) -> Result<Option<EmissionSnapshot>> {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_EMISSION)?;
        match table.get(b"emission_snapshot".as_slice())? {
            Some(data) => {
                let snapshot: EmissionSnapshot = serde_json::from_slice(data.value())?;
                info!(
                    "Loaded emission snapshot: emitted={:.2} RKU, burned={:.2} RKU",
                    snapshot.total_emitted, snapshot.total_burned
                );
                Ok(Some(snapshot))
            }
            None => {
                warn!("No emission snapshot found, starting fresh");
                Ok(None)
            }
        }
    }

    pub fn save_weights(&self, weights: &HashMap<String, AggregatedWeight>) -> Result<()> {
        let data = serde_json::to_vec(weights)?;
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_WEIGHTS)?;
            table.insert(b"weights_snapshot".as_slice(), data.as_slice())?;
        }
        write_txn.commit()?;
        debug!(
            "Saved weights snapshot: {} transaction weights",
            weights.len()
        );
        Ok(())
    }

    pub fn load_weights(&self) -> Result<Option<HashMap<String, AggregatedWeight>>> {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_WEIGHTS)?;
        match table.get(b"weights_snapshot".as_slice())? {
            Some(data) => {
                let weights: HashMap<String, AggregatedWeight> =
                    serde_json::from_slice(data.value())?;
                info!(
                    "Loaded weights snapshot: {} transaction weights",
                    weights.len()
                );
                Ok(Some(weights))
            }
            None => {
                debug!("No weights snapshot found, starting with empty weights");
                Ok(None)
            }
        }
    }

    pub fn save_contracts(&self, contracts: &[ContractState]) -> Result<()> {
        let data = serde_json::to_vec(contracts)?;
        let db = self.db_read();
        let write_txn = db.begin_write()?;
        {
            let mut table = write_txn.open_table(TABLE_CONTRACTS)?;
            table.insert(b"contracts_snapshot".as_slice(), data.as_slice())?;
        }
        write_txn.commit()?;
        debug!("Saved {} contracts", contracts.len());
        Ok(())
    }

    pub fn load_contracts(&self) -> Result<Vec<ContractState>> {
        let db = self.db_read();
        let read_txn = db.begin_read()?;
        let table = read_txn.open_table(TABLE_CONTRACTS)?;
        match table.get(b"contracts_snapshot".as_slice())? {
            Some(data) => {
                let contracts: Vec<ContractState> = serde_json::from_slice(data.value())?;
                info!("Loaded {} contracts from redb", contracts.len());
                Ok(contracts)
            }
            None => {
                debug!("No contracts found in redb");
                Ok(Vec::new())
            }
        }
    }

    pub fn flush(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Default, Clone)]
pub struct StorageStats {
    pub accounts_count: usize,
    pub dag_count: usize,
    pub checkpoints_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_redb_basic_operations() {
        let dir = tempdir().unwrap();
        let storage = RedbStorage::open(dir.path().to_str().unwrap()).unwrap();

        storage
            .put_metadata(b"test_key", &"test_value".to_string())
            .unwrap();
        let value: Option<String> = storage.get_metadata(b"test_key").unwrap();
        assert_eq!(value, Some("test_value".to_string()));
    }

    #[test]
    fn test_redb_accounts() {
        let dir = tempdir().unwrap();
        let storage = RedbStorage::open(dir.path().to_str().unwrap()).unwrap();

        storage.put_accounts(b"acc1", &1000u64).unwrap();
        storage.put_accounts(b"acc2", &2000u64).unwrap();

        assert_eq!(storage.get_accounts::<u64>(b"acc1").unwrap(), Some(1000));
        assert_eq!(storage.get_accounts::<u64>(b"acc2").unwrap(), Some(2000));
        assert_eq!(storage.count_accounts().unwrap(), 2);
    }

    #[test]
    fn test_redb_dag() {
        let dir = tempdir().unwrap();
        let storage = RedbStorage::open(dir.path().to_str().unwrap()).unwrap();

        storage
            .put_dag(b"tx1", &"transaction1".to_string())
            .unwrap();
        storage
            .put_dag(b"tx2", &"transaction2".to_string())
            .unwrap();

        assert_eq!(
            storage.get_dag::<String>(b"tx1").unwrap(),
            Some("transaction1".to_string())
        );

        storage.delete_dag(b"tx1").unwrap();
        assert_eq!(storage.get_dag::<String>(b"tx1").unwrap(), None);
    }

    #[test]
    fn test_redb_iteration() {
        let dir = tempdir().unwrap();
        let storage = RedbStorage::open(dir.path().to_str().unwrap()).unwrap();

        for i in 0..10u64 {
            let key = format!("key_{:04}", i);
            storage.put_accounts(key.as_bytes(), &i).unwrap();
        }

        let mut count = 0;
        storage
            .iter_accounts::<u64, _>(|_key, _value| {
                count += 1;
            })
            .unwrap();

        assert_eq!(count, 10);
    }

    #[test]
    fn test_redb_checkpoints() {
        let dir = tempdir().unwrap();
        let storage = RedbStorage::open(dir.path().to_str().unwrap()).unwrap();

        for i in 0..5u64 {
            let key = i.to_be_bytes();
            storage
                .put_checkpoint(&key, &format!("checkpoint_{}", i))
                .unwrap();
        }

        assert_eq!(storage.count_checkpoints().unwrap(), 5);

        let key = 2u64.to_be_bytes();
        storage.delete_checkpoint(&key).unwrap();
        assert_eq!(storage.count_checkpoints().unwrap(), 4);
    }
}
