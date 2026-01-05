use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MerkleProofPath {
    pub key: String,
    pub value: Value,
    pub proof: Vec<String>,
    pub index: usize,
}

#[derive(Debug, Clone)]
pub struct StateTrie {
    storage: HashMap<String, Value>,
    root_hash: String,
}

fn compute_hash(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

impl StateTrie {
    pub fn new() -> Self {
        StateTrie {
            storage: HashMap::new(),
            root_hash: compute_hash("empty"),
        }
    }

    pub fn set(&mut self, contract_id: &str, key: &str, value: Value) {
        let full_key = format!("{}:{}", contract_id, key);
        self.storage.insert(full_key, value);
        self.update_root();
    }

    pub fn get(&self, contract_id: &str, key: &str) -> Option<&Value> {
        let full_key = format!("{}:{}", contract_id, key);
        self.storage.get(&full_key)
    }

    pub fn delete(&mut self, contract_id: &str, key: &str) {
        let full_key = format!("{}:{}", contract_id, key);
        self.storage.remove(&full_key);
        self.update_root();
    }

    pub fn get_contract_state(&self, contract_id: &str) -> HashMap<String, Value> {
        let prefix = format!("{}:", contract_id);
        let mut state = HashMap::new();

        for (key, value) in &self.storage {
            if key.starts_with(&prefix) {
                let local_key = &key[prefix.len()..];
                state.insert(local_key.to_string(), value.clone());
            }
        }

        state
    }

    pub fn set_contract_state(&mut self, contract_id: &str, state: &HashMap<String, Value>) {
        let prefix = format!("{}:", contract_id);

        let keys_to_remove: Vec<String> = self
            .storage
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();

        for key in keys_to_remove {
            self.storage.remove(&key);
        }

        for (key, value) in state {
            self.storage
                .insert(format!("{}{}", prefix, key), value.clone());
        }

        self.update_root();
    }

    fn update_root(&mut self) {
        let mut sorted_keys: Vec<&String> = self.storage.keys().collect();
        sorted_keys.sort();

        if sorted_keys.is_empty() {
            self.root_hash = compute_hash("empty");
            return;
        }

        let leaves: Vec<String> = sorted_keys
            .iter()
            .map(|key| {
                let value = self.storage.get(*key).unwrap();
                compute_hash(&format!("{}:{}", key, value))
            })
            .collect();

        self.root_hash = compute_merkle_root(&leaves);
    }

    pub fn root_hash(&self) -> &str {
        &self.root_hash
    }

    pub fn get_proof(&self, contract_id: &str, key: &str) -> Option<MerkleProofPath> {
        let full_key = format!("{}:{}", contract_id, key);
        let value = self.storage.get(&full_key)?;

        let mut sorted_keys: Vec<&String> = self.storage.keys().collect();
        sorted_keys.sort();

        let index = sorted_keys.iter().position(|k| **k == full_key)?;

        let leaves: Vec<String> = sorted_keys
            .iter()
            .map(|k| {
                let v = self.storage.get(*k).unwrap();
                compute_hash(&format!("{}:{}", k, v))
            })
            .collect();

        let proof = compute_merkle_proof(&leaves, index);

        Some(MerkleProofPath {
            key: key.to_string(),
            value: value.clone(),
            proof,
            index,
        })
    }

    pub fn verify_proof(&self, proof: &MerkleProofPath, contract_id: &str) -> bool {
        let full_key = format!("{}:{}", contract_id, proof.key);
        let leaf_hash = compute_hash(&format!("{}:{}", full_key, proof.value));

        verify_merkle_proof(&leaf_hash, &proof.proof, proof.index, &self.root_hash)
    }

    pub fn size(&self) -> usize {
        self.storage.len()
    }

    pub fn to_json(&self) -> StateTrieSnapshot {
        StateTrieSnapshot {
            storage: self.storage.clone(),
            root_hash: self.root_hash.clone(),
        }
    }

    pub fn from_json(snapshot: StateTrieSnapshot) -> Self {
        StateTrie {
            storage: snapshot.storage,
            root_hash: snapshot.root_hash,
        }
    }
}

impl Default for StateTrie {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateTrieSnapshot {
    pub storage: HashMap<String, Value>,
    pub root_hash: String,
}

fn compute_merkle_root(leaves: &[String]) -> String {
    if leaves.is_empty() {
        return compute_hash("empty");
    }

    if leaves.len() == 1 {
        return leaves[0].clone();
    }

    let mut current_layer = leaves.to_vec();

    while current_layer.len() > 1 {
        let mut next_layer = Vec::new();

        let mut i = 0;
        while i < current_layer.len() {
            let left = &current_layer[i];
            let right = if i + 1 < current_layer.len() {
                &current_layer[i + 1]
            } else {
                left
            };

            next_layer.push(compute_hash(&format!("{}{}", left, right)));
            i += 2;
        }

        current_layer = next_layer;
    }

    current_layer[0].clone()
}

fn compute_merkle_proof(leaves: &[String], index: usize) -> Vec<String> {
    if leaves.len() <= 1 {
        return vec![];
    }

    let mut proof = Vec::new();
    let mut current_layer = leaves.to_vec();
    let mut idx = index;

    while current_layer.len() > 1 {
        let sibling_idx = if idx % 2 == 0 { idx + 1 } else { idx - 1 };

        if sibling_idx < current_layer.len() {
            proof.push(current_layer[sibling_idx].clone());
        } else {
            proof.push(current_layer[idx].clone());
        }

        let mut next_layer = Vec::new();
        let mut i = 0;
        while i < current_layer.len() {
            let left = &current_layer[i];
            let right = if i + 1 < current_layer.len() {
                &current_layer[i + 1]
            } else {
                left
            };

            next_layer.push(compute_hash(&format!("{}{}", left, right)));
            i += 2;
        }

        idx /= 2;
        current_layer = next_layer;
    }

    proof
}

fn verify_merkle_proof(leaf_hash: &str, proof: &[String], index: usize, expected_root: &str) -> bool {
    let mut current = leaf_hash.to_string();
    let mut idx = index;

    for sibling in proof {
        let (left, right) = if idx % 2 == 0 {
            (&current, sibling)
        } else {
            (sibling, &current)
        };

        current = compute_hash(&format!("{}{}", left, right));
        idx /= 2;
    }

    current == expected_root
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_set_and_get() {
        let mut trie = StateTrie::new();

        trie.set("contract1", "balance", json!(100));
        trie.set("contract1", "owner", json!("alice"));

        assert_eq!(trie.get("contract1", "balance"), Some(&json!(100)));
        assert_eq!(trie.get("contract1", "owner"), Some(&json!("alice")));
        assert_eq!(trie.get("contract1", "unknown"), None);
    }

    #[test]
    fn test_delete() {
        let mut trie = StateTrie::new();

        trie.set("contract1", "key", json!("value"));
        assert!(trie.get("contract1", "key").is_some());

        trie.delete("contract1", "key");
        assert!(trie.get("contract1", "key").is_none());
    }

    #[test]
    fn test_contract_state() {
        let mut trie = StateTrie::new();

        trie.set("contract1", "a", json!(1));
        trie.set("contract1", "b", json!(2));
        trie.set("contract2", "c", json!(3));

        let state = trie.get_contract_state("contract1");
        assert_eq!(state.len(), 2);
        assert_eq!(state.get("a"), Some(&json!(1)));
        assert_eq!(state.get("b"), Some(&json!(2)));
    }

    #[test]
    fn test_root_hash_changes() {
        let mut trie = StateTrie::new();
        let empty_root = trie.root_hash().to_string();

        trie.set("contract1", "key", json!("value"));
        let root1 = trie.root_hash().to_string();

        trie.set("contract1", "key2", json!("value2"));
        let root2 = trie.root_hash().to_string();

        assert_ne!(empty_root, root1);
        assert_ne!(root1, root2);
    }

    #[test]
    fn test_proof_generation_and_verification() {
        let mut trie = StateTrie::new();

        trie.set("contract1", "a", json!(1));
        trie.set("contract1", "b", json!(2));
        trie.set("contract1", "c", json!(3));

        let proof = trie.get_proof("contract1", "b").unwrap();

        assert!(trie.verify_proof(&proof, "contract1"));
        assert_eq!(proof.key, "b");
        assert_eq!(proof.value, json!(2));
    }

    #[test]
    fn test_snapshot() {
        let mut trie = StateTrie::new();
        trie.set("contract1", "key", json!("value"));

        let snapshot = trie.to_json();
        let restored = StateTrie::from_json(snapshot);

        assert_eq!(restored.root_hash(), trie.root_hash());
        assert_eq!(
            restored.get("contract1", "key"),
            Some(&json!("value"))
        );
    }
}
