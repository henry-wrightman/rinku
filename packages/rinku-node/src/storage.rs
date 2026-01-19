use anyhow::Result;
use redb::{Database, ReadableTable, TableDefinition, WriteTransaction};
use serde::{de::DeserializeOwned, Serialize};
use std::sync::{Arc, RwLock};
use tracing::{debug, info};

pub const TABLE_ACCOUNTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("accounts");
pub const TABLE_DAG: TableDefinition<&[u8], &[u8]> = TableDefinition::new("dag");
pub const TABLE_CHECKPOINTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("checkpoints");
pub const TABLE_VALIDATORS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("validators");
pub const TABLE_TRIE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("trie");
pub const TABLE_METADATA: TableDefinition<&[u8], &[u8]> = TableDefinition::new("metadata");
pub const TABLE_CONTRACTS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("contracts");
pub const TABLE_REWARDS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("rewards");

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
            write_txn.commit()?;
        }

        info!("Opened redb database at {}", db_path);

        Ok(Self {
            db: Arc::new(RwLock::new(db)),
            data_dir: data_dir.to_string(),
        })
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
        let data = bincode::serialize(value)?;
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
                let value: T = bincode::deserialize(data.value())?;
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
        let data = bincode::serialize(value)?;
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
                let value: T = bincode::deserialize(data.value())?;
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
            if let Ok(v) = bincode::deserialize::<T>(value.value()) {
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
            if let Ok(v) = bincode::deserialize::<T>(value.value()) {
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

        storage.put_metadata(b"test_key", &"test_value".to_string()).unwrap();
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

        storage.put_dag(b"tx1", &"transaction1".to_string()).unwrap();
        storage.put_dag(b"tx2", &"transaction2".to_string()).unwrap();

        assert_eq!(storage.get_dag::<String>(b"tx1").unwrap(), Some("transaction1".to_string()));
        
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
        storage.iter_accounts::<u64, _>(|_key, _value| {
            count += 1;
        }).unwrap();

        assert_eq!(count, 10);
    }

    #[test]
    fn test_redb_checkpoints() {
        let dir = tempdir().unwrap();
        let storage = RedbStorage::open(dir.path().to_str().unwrap()).unwrap();

        for i in 0..5u64 {
            let key = i.to_be_bytes();
            storage.put_checkpoint(&key, &format!("checkpoint_{}", i)).unwrap();
        }

        assert_eq!(storage.count_checkpoints().unwrap(), 5);

        let key = 2u64.to_be_bytes();
        storage.delete_checkpoint(&key).unwrap();
        assert_eq!(storage.count_checkpoints().unwrap(), 4);
    }
}
