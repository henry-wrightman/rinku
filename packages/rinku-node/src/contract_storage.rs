use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use tracing::{debug, info};

use crate::sparse_merkle_trie::{hash_contract_key, SparseMerkleTrie, MerkleProof};
use crate::storage::RedbStorage;

pub struct ContractStorageManager {
    trie: SparseMerkleTrie,
}

impl ContractStorageManager {
    pub fn new() -> Self {
        Self {
            trie: SparseMerkleTrie::new(),
        }
    }

    pub fn with_root(root: [u8; 32]) -> Self {
        Self {
            trie: SparseMerkleTrie::with_root(root),
        }
    }

    pub fn root(&self) -> [u8; 32] {
        self.trie.root()
    }

    pub fn root_hex(&self) -> String {
        self.trie.root_hex()
    }

    pub fn read_key(
        &self,
        contract_id: &str,
        key: &str,
        storage: Option<&RedbStorage>,
    ) -> Result<Option<Value>> {
        let trie_key = hash_contract_key(contract_id, key);
        match self.trie.get(&trie_key, storage)? {
            Some(bytes) => {
                let value: Value = serde_json::from_slice(&bytes)?;
                Ok(Some(value))
            }
            None => Ok(None),
        }
    }

    pub fn write_key(
        &mut self,
        contract_id: &str,
        key: &str,
        value: &Value,
        storage: Option<&RedbStorage>,
    ) -> Result<[u8; 32]> {
        let trie_key = hash_contract_key(contract_id, key);
        let bytes = serde_json::to_vec(value)?;
        self.trie.set(&trie_key, bytes, storage)
    }

    pub fn delete_key(
        &mut self,
        contract_id: &str,
        key: &str,
        storage: Option<&RedbStorage>,
    ) -> Result<[u8; 32]> {
        let trie_key = hash_contract_key(contract_id, key);
        self.trie.delete(&trie_key, storage)
    }

    pub fn prove_key(
        &self,
        contract_id: &str,
        key: &str,
        storage: Option<&RedbStorage>,
    ) -> Result<MerkleProof> {
        let trie_key = hash_contract_key(contract_id, key);
        self.trie.prove(&trie_key, storage)
    }

    pub fn load_contract_state(
        &self,
        contract_id: &str,
        keys: &[String],
        storage: Option<&RedbStorage>,
    ) -> Result<HashMap<String, Value>> {
        let mut state = HashMap::new();
        for key in keys {
            if let Some(value) = self.read_key(contract_id, key, storage)? {
                state.insert(key.clone(), value);
            }
        }
        Ok(state)
    }

    pub fn apply_state_diff(
        &mut self,
        contract_id: &str,
        changes: &[(String, Option<Value>)],
        storage: Option<&RedbStorage>,
    ) -> Result<[u8; 32]> {
        let mut new_root = self.trie.root();
        for (key, value) in changes {
            match value {
                Some(v) => {
                    new_root = self.write_key(contract_id, key, v, storage)?;
                    debug!("Contract {} state: set key '{}' -> {}", contract_id, key, v);
                }
                None => {
                    new_root = self.delete_key(contract_id, key, storage)?;
                    debug!("Contract {} state: deleted key '{}'", contract_id, key);
                }
            }
        }
        info!(
            "Contract {} state updated: {} changes, new root: {}",
            contract_id,
            changes.len(),
            hex::encode(new_root)
        );
        Ok(new_root)
    }

    pub fn flush(&mut self, storage: &RedbStorage) -> Result<usize> {
        self.trie.flush_to_storage(storage)
    }

    pub fn prove_multiple_keys(
        &self,
        contract_id: &str,
        keys: &[String],
        storage: Option<&RedbStorage>,
    ) -> Result<Vec<MerkleProof>> {
        let mut proofs = Vec::with_capacity(keys.len());
        for key in keys {
            proofs.push(self.prove_key(contract_id, key, storage)?);
        }
        Ok(proofs)
    }

    pub fn verify_key_proof(proof: &MerkleProof) -> bool {
        proof.verify()
    }
}

impl Default for ContractStorageManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_write_key() {
        let mut mgr = ContractStorageManager::new();
        let empty_root = mgr.root();

        mgr.write_key("contract_1", "balance", &Value::from(100), None).unwrap();
        assert_ne!(mgr.root(), empty_root);

        let val = mgr.read_key("contract_1", "balance", None).unwrap();
        assert_eq!(val, Some(Value::from(100)));
    }

    #[test]
    fn test_contract_isolation() {
        let mut mgr = ContractStorageManager::new();

        mgr.write_key("contract_a", "key", &Value::from("a_value"), None).unwrap();
        mgr.write_key("contract_b", "key", &Value::from("b_value"), None).unwrap();

        let a_val = mgr.read_key("contract_a", "key", None).unwrap();
        let b_val = mgr.read_key("contract_b", "key", None).unwrap();

        assert_eq!(a_val, Some(Value::from("a_value")));
        assert_eq!(b_val, Some(Value::from("b_value")));
    }

    #[test]
    fn test_delete_key() {
        let mut mgr = ContractStorageManager::new();

        mgr.write_key("contract_1", "temp", &Value::from(42), None).unwrap();
        assert!(mgr.read_key("contract_1", "temp", None).unwrap().is_some());

        mgr.delete_key("contract_1", "temp", None).unwrap();
        assert!(mgr.read_key("contract_1", "temp", None).unwrap().is_none());
    }

    #[test]
    fn test_prove_key() {
        let mut mgr = ContractStorageManager::new();
        mgr.write_key("contract_1", "proven_key", &Value::from("data"), None).unwrap();

        let proof = mgr.prove_key("contract_1", "proven_key", None).unwrap();
        assert!(ContractStorageManager::verify_key_proof(&proof));
        assert_eq!(proof.root, mgr.root());
    }

    #[test]
    fn test_apply_state_diff() {
        let mut mgr = ContractStorageManager::new();

        mgr.write_key("contract_1", "existing", &Value::from("old"), None).unwrap();

        let changes = vec![
            ("existing".to_string(), Some(Value::from("new"))),
            ("added".to_string(), Some(Value::from(42))),
        ];

        mgr.apply_state_diff("contract_1", &changes, None).unwrap();

        assert_eq!(mgr.read_key("contract_1", "existing", None).unwrap(), Some(Value::from("new")));
        assert_eq!(mgr.read_key("contract_1", "added", None).unwrap(), Some(Value::from(42)));
    }

    #[test]
    fn test_load_contract_state() {
        let mut mgr = ContractStorageManager::new();

        mgr.write_key("contract_1", "a", &Value::from(1), None).unwrap();
        mgr.write_key("contract_1", "b", &Value::from(2), None).unwrap();
        mgr.write_key("contract_1", "c", &Value::from(3), None).unwrap();

        let keys = vec!["a".to_string(), "b".to_string(), "c".to_string(), "missing".to_string()];
        let state = mgr.load_contract_state("contract_1", &keys, None).unwrap();

        assert_eq!(state.len(), 3);
        assert_eq!(state.get("a"), Some(&Value::from(1)));
        assert_eq!(state.get("missing"), None);
    }

    #[test]
    fn test_deterministic_roots() {
        let mut mgr1 = ContractStorageManager::new();
        let mut mgr2 = ContractStorageManager::new();

        mgr1.write_key("c", "x", &Value::from(1), None).unwrap();
        mgr1.write_key("c", "y", &Value::from(2), None).unwrap();

        mgr2.write_key("c", "x", &Value::from(1), None).unwrap();
        mgr2.write_key("c", "y", &Value::from(2), None).unwrap();

        assert_eq!(mgr1.root(), mgr2.root());
    }
}
