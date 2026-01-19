use crate::storage::RedbStorage;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use tracing::{debug, info};

pub const TREE_DEPTH: usize = 256;
const EMPTY_HASH: [u8; 32] = [0u8; 32];

fn hash_bytes(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

fn key_to_path(key: &[u8; 32]) -> Vec<bool> {
    let mut path = Vec::with_capacity(TREE_DEPTH);
    for byte in key.iter() {
        for bit in (0..8).rev() {
            path.push((byte >> bit) & 1 == 1);
        }
    }
    path
}

fn compute_default_hashes() -> Vec<[u8; 32]> {
    let mut defaults = vec![EMPTY_HASH; TREE_DEPTH + 1];
    for i in (0..TREE_DEPTH).rev() {
        defaults[i] = hash_pair(&defaults[i + 1], &defaults[i + 1]);
    }
    defaults
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrieNode {
    pub left: Option<[u8; 32]>,
    pub right: Option<[u8; 32]>,
    pub value: Option<Vec<u8>>,
}

impl TrieNode {
    pub fn empty() -> Self {
        Self {
            left: None,
            right: None,
            value: None,
        }
    }

    pub fn leaf(value: Vec<u8>) -> Self {
        Self {
            left: None,
            right: None,
            value: Some(value),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleProof {
    pub key: [u8; 32],
    pub value: Option<Vec<u8>>,
    pub siblings: Vec<[u8; 32]>,
    pub root: [u8; 32],
}

impl MerkleProof {
    pub fn verify(&self) -> bool {
        let default_hashes = compute_default_hashes();
        let path = key_to_path(&self.key);
        
        let mut current_hash = match &self.value {
            Some(v) => hash_bytes(v),
            None => EMPTY_HASH,
        };

        for (i, &go_right) in path.iter().enumerate().rev() {
            let sibling = if i < self.siblings.len() {
                self.siblings[TREE_DEPTH - 1 - i]
            } else {
                default_hashes[i + 1]
            };

            current_hash = if go_right {
                hash_pair(&sibling, &current_hash)
            } else {
                hash_pair(&current_hash, &sibling)
            };
        }

        current_hash == self.root
    }

    pub fn encoded_size(&self) -> usize {
        32 + self.value.as_ref().map_or(0, |v| v.len()) + 
        self.siblings.len() * 32 + 32
    }
}

pub struct SparseMerkleTrie {
    root_hash: [u8; 32],
    cache: HashMap<[u8; 32], TrieNode>,
    dirty_nodes: HashMap<[u8; 32], TrieNode>,
    default_hashes: Vec<[u8; 32]>,
}

impl SparseMerkleTrie {
    pub fn new() -> Self {
        let default_hashes = compute_default_hashes();
        Self {
            root_hash: default_hashes[0],
            cache: HashMap::new(),
            dirty_nodes: HashMap::new(),
            default_hashes,
        }
    }

    pub fn with_root(root_hash: [u8; 32]) -> Self {
        let default_hashes = compute_default_hashes();
        Self {
            root_hash,
            cache: HashMap::new(),
            dirty_nodes: HashMap::new(),
            default_hashes,
        }
    }

    pub fn root(&self) -> [u8; 32] {
        self.root_hash
    }

    pub fn root_hex(&self) -> String {
        hex::encode(self.root_hash)
    }

    pub fn get(&self, key: &[u8; 32], storage: Option<&RedbStorage>) -> Result<Option<Vec<u8>>> {
        let path = key_to_path(key);
        let mut current_hash = self.root_hash;

        for (depth, &go_right) in path.iter().enumerate() {
            if current_hash == self.default_hashes[depth] {
                return Ok(None);
            }

            let node = self.get_node(&current_hash, storage)?;
            match node {
                Some(n) => {
                    if n.value.is_some() && depth == TREE_DEPTH - 1 {
                        return Ok(n.value);
                    }
                    current_hash = if go_right {
                        n.right.unwrap_or(self.default_hashes[depth + 1])
                    } else {
                        n.left.unwrap_or(self.default_hashes[depth + 1])
                    };
                }
                None => return Ok(None),
            }
        }

        Ok(None)
    }

    pub fn set(&mut self, key: &[u8; 32], value: Vec<u8>, storage: Option<&RedbStorage>) -> Result<[u8; 32]> {
        let path = key_to_path(key);
        let leaf_hash = hash_bytes(&value);
        
        let new_root = self.set_recursive(
            self.root_hash,
            &path,
            0,
            leaf_hash,
            Some(value),
            storage,
        )?;

        self.root_hash = new_root;
        Ok(new_root)
    }

    fn set_recursive(
        &mut self,
        current_hash: [u8; 32],
        path: &[bool],
        depth: usize,
        leaf_hash: [u8; 32],
        value: Option<Vec<u8>>,
        storage: Option<&RedbStorage>,
    ) -> Result<[u8; 32]> {
        if depth == TREE_DEPTH {
            let node = TrieNode::leaf(value.unwrap_or_default());
            self.dirty_nodes.insert(leaf_hash, node);
            return Ok(leaf_hash);
        }

        let (left, right) = if current_hash == self.default_hashes[depth] {
            (self.default_hashes[depth + 1], self.default_hashes[depth + 1])
        } else {
            let node = self.get_node(&current_hash, storage)?.unwrap_or_else(TrieNode::empty);
            (
                node.left.unwrap_or(self.default_hashes[depth + 1]),
                node.right.unwrap_or(self.default_hashes[depth + 1]),
            )
        };

        let (new_left, new_right) = if path[depth] {
            let new_right = self.set_recursive(right, path, depth + 1, leaf_hash, value, storage)?;
            (left, new_right)
        } else {
            let new_left = self.set_recursive(left, path, depth + 1, leaf_hash, value, storage)?;
            (new_left, right)
        };

        let new_hash = hash_pair(&new_left, &new_right);
        let new_node = TrieNode {
            left: Some(new_left),
            right: Some(new_right),
            value: None,
        };
        self.dirty_nodes.insert(new_hash, new_node);

        Ok(new_hash)
    }

    pub fn delete(&mut self, key: &[u8; 32], storage: Option<&RedbStorage>) -> Result<[u8; 32]> {
        let path = key_to_path(key);
        let new_root = self.delete_recursive(self.root_hash, &path, 0, storage)?;
        self.root_hash = new_root;
        Ok(new_root)
    }

    fn delete_recursive(
        &mut self,
        current_hash: [u8; 32],
        path: &[bool],
        depth: usize,
        storage: Option<&RedbStorage>,
    ) -> Result<[u8; 32]> {
        if current_hash == self.default_hashes[depth] {
            return Ok(self.default_hashes[depth]);
        }

        if depth == TREE_DEPTH {
            return Ok(self.default_hashes[depth]);
        }

        let node = self.get_node(&current_hash, storage)?.unwrap_or_else(TrieNode::empty);
        let left = node.left.unwrap_or(self.default_hashes[depth + 1]);
        let right = node.right.unwrap_or(self.default_hashes[depth + 1]);

        let (new_left, new_right) = if path[depth] {
            let new_right = self.delete_recursive(right, path, depth + 1, storage)?;
            (left, new_right)
        } else {
            let new_left = self.delete_recursive(left, path, depth + 1, storage)?;
            (new_left, right)
        };

        if new_left == self.default_hashes[depth + 1] && new_right == self.default_hashes[depth + 1] {
            return Ok(self.default_hashes[depth]);
        }

        let new_hash = hash_pair(&new_left, &new_right);
        let new_node = TrieNode {
            left: Some(new_left),
            right: Some(new_right),
            value: None,
        };
        self.dirty_nodes.insert(new_hash, new_node);

        Ok(new_hash)
    }

    pub fn prove(&self, key: &[u8; 32], storage: Option<&RedbStorage>) -> Result<MerkleProof> {
        let path = key_to_path(key);
        let mut siblings = Vec::with_capacity(TREE_DEPTH);
        let mut current_hash = self.root_hash;
        let mut value = None;

        for (depth, &go_right) in path.iter().enumerate() {
            if current_hash == self.default_hashes[depth] {
                for remaining in depth..TREE_DEPTH {
                    siblings.push(self.default_hashes[remaining + 1]);
                }
                break;
            }

            let node = self.get_node(&current_hash, storage)?;
            match node {
                Some(n) => {
                    let (left, right) = (
                        n.left.unwrap_or(self.default_hashes[depth + 1]),
                        n.right.unwrap_or(self.default_hashes[depth + 1]),
                    );

                    if go_right {
                        siblings.push(left);
                        current_hash = right;
                    } else {
                        siblings.push(right);
                        current_hash = left;
                    }

                    if depth == TREE_DEPTH - 1 {
                        value = n.value.clone();
                    }
                }
                None => {
                    for remaining in depth..TREE_DEPTH {
                        siblings.push(self.default_hashes[remaining + 1]);
                    }
                    break;
                }
            }
        }

        while siblings.len() < TREE_DEPTH {
            siblings.push(self.default_hashes[siblings.len() + 1]);
        }

        Ok(MerkleProof {
            key: *key,
            value,
            siblings,
            root: self.root_hash,
        })
    }

    fn get_node(&self, hash: &[u8; 32], storage: Option<&RedbStorage>) -> Result<Option<TrieNode>> {
        if let Some(node) = self.dirty_nodes.get(hash) {
            return Ok(Some(node.clone()));
        }

        if let Some(node) = self.cache.get(hash) {
            return Ok(Some(node.clone()));
        }

        if let Some(store) = storage {
            return store.get_trie::<TrieNode>(hash);
        }

        Ok(None)
    }

    pub fn flush_to_storage(&mut self, storage: &RedbStorage) -> Result<usize> {
        let count = self.dirty_nodes.len();

        for (hash, node) in self.dirty_nodes.drain() {
            storage.put_trie(&hash, &node)?;
            self.cache.insert(hash, node);
        }

        debug!("Flushed {} trie nodes to storage", count);
        Ok(count)
    }

    pub fn dirty_node_count(&self) -> usize {
        self.dirty_nodes.len()
    }

    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }
}

impl Default for SparseMerkleTrie {
    fn default() -> Self {
        Self::new()
    }
}

pub fn hash_account_key(address: &str) -> [u8; 32] {
    hash_bytes(address.as_bytes())
}

pub fn hash_contract_key(contract_id: &str, key: &str) -> [u8; 32] {
    let mut data = contract_id.as_bytes().to_vec();
    data.push(b':');
    data.extend_from_slice(key.as_bytes());
    hash_bytes(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_trie() {
        let trie = SparseMerkleTrie::new();
        let default_hashes = compute_default_hashes();
        assert_eq!(trie.root(), default_hashes[0]);
    }

    #[test]
    fn test_set_and_get() {
        let mut trie = SparseMerkleTrie::new();
        let key = hash_bytes(b"test_key");
        let value = b"test_value".to_vec();

        let initial_root = trie.root();
        trie.set(&key, value.clone(), None).unwrap();
        
        assert_ne!(trie.root(), initial_root);
        
        let retrieved = trie.get(&key, None).unwrap();
        assert_eq!(retrieved, Some(value));
    }

    #[test]
    fn test_multiple_keys() {
        let mut trie = SparseMerkleTrie::new();

        let key1 = hash_bytes(b"key1");
        let key2 = hash_bytes(b"key2");
        let key3 = hash_bytes(b"key3");

        trie.set(&key1, b"value1".to_vec(), None).unwrap();
        trie.set(&key2, b"value2".to_vec(), None).unwrap();
        trie.set(&key3, b"value3".to_vec(), None).unwrap();

        assert_eq!(trie.get(&key1, None).unwrap(), Some(b"value1".to_vec()));
        assert_eq!(trie.get(&key2, None).unwrap(), Some(b"value2".to_vec()));
        assert_eq!(trie.get(&key3, None).unwrap(), Some(b"value3".to_vec()));
    }

    #[test]
    fn test_update_value() {
        let mut trie = SparseMerkleTrie::new();
        let key = hash_bytes(b"test_key");

        trie.set(&key, b"initial".to_vec(), None).unwrap();
        let root1 = trie.root();

        trie.set(&key, b"updated".to_vec(), None).unwrap();
        let root2 = trie.root();

        assert_ne!(root1, root2);
        assert_eq!(trie.get(&key, None).unwrap(), Some(b"updated".to_vec()));
    }

    #[test]
    fn test_delete() {
        let mut trie = SparseMerkleTrie::new();
        let key = hash_bytes(b"test_key");

        trie.set(&key, b"value".to_vec(), None).unwrap();
        assert!(trie.get(&key, None).unwrap().is_some());

        trie.delete(&key, None).unwrap();
        assert!(trie.get(&key, None).unwrap().is_none());
    }

    #[test]
    fn test_proof_generation_and_verification() {
        let mut trie = SparseMerkleTrie::new();
        let key = hash_bytes(b"test_key");
        let value = b"test_value".to_vec();

        trie.set(&key, value.clone(), None).unwrap();

        let proof = trie.prove(&key, None).unwrap();
        assert_eq!(proof.key, key);
        assert_eq!(proof.value, Some(value));
        assert_eq!(proof.root, trie.root());
        assert!(proof.verify());
    }

    #[test]
    fn test_non_membership_proof() {
        let mut trie = SparseMerkleTrie::new();
        let key1 = hash_bytes(b"exists");
        let key2 = hash_bytes(b"not_exists");

        trie.set(&key1, b"value".to_vec(), None).unwrap();

        let proof = trie.prove(&key2, None).unwrap();
        assert_eq!(proof.value, None);
        assert!(proof.verify());
    }

    #[test]
    fn test_deterministic_roots() {
        let key1 = hash_bytes(b"key1");
        let key2 = hash_bytes(b"key2");

        let mut trie1 = SparseMerkleTrie::new();
        trie1.set(&key1, b"v1".to_vec(), None).unwrap();
        trie1.set(&key2, b"v2".to_vec(), None).unwrap();

        let mut trie2 = SparseMerkleTrie::new();
        trie2.set(&key1, b"v1".to_vec(), None).unwrap();
        trie2.set(&key2, b"v2".to_vec(), None).unwrap();

        assert_eq!(trie1.root(), trie2.root());
    }

    #[test]
    fn test_order_independence() {
        let key1 = hash_bytes(b"key1");
        let key2 = hash_bytes(b"key2");

        let mut trie1 = SparseMerkleTrie::new();
        trie1.set(&key1, b"v1".to_vec(), None).unwrap();
        trie1.set(&key2, b"v2".to_vec(), None).unwrap();

        let mut trie2 = SparseMerkleTrie::new();
        trie2.set(&key2, b"v2".to_vec(), None).unwrap();
        trie2.set(&key1, b"v1".to_vec(), None).unwrap();

        assert_eq!(trie1.root(), trie2.root());
    }

    #[test]
    fn test_account_key_hashing() {
        let addr1 = "0x1234567890abcdef";
        let addr2 = "0xfedcba0987654321";

        let key1 = hash_account_key(addr1);
        let key2 = hash_account_key(addr2);

        assert_ne!(key1, key2);
        assert_eq!(key1, hash_account_key(addr1));
    }
}
