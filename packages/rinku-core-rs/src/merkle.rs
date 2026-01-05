use std::collections::BTreeMap;

use crate::crypto::hash;
use crate::types::{AccountState, MerkleNode};

pub fn create_merkle_tree(accounts: &BTreeMap<String, AccountState>) -> Option<MerkleNode> {
    if accounts.is_empty() {
        return None;
    }

    let leaves: Vec<MerkleNode> = accounts
        .iter()
        .map(|(fingerprint, state)| {
            let leaf_data = format!("{}:{}:{}", fingerprint, state.balance, state.nonce);
            MerkleNode {
                hash: hash(&leaf_data),
                left: None,
                right: None,
                data: Some(fingerprint.clone()),
            }
        })
        .collect();

    Some(build_tree_from_leaves(leaves))
}

fn build_tree_from_leaves(leaves: Vec<MerkleNode>) -> MerkleNode {
    if leaves.len() == 1 {
        return leaves.into_iter().next().unwrap();
    }

    let mut next_level: Vec<MerkleNode> = Vec::new();
    let mut iter = leaves.into_iter().peekable();

    while let Some(left) = iter.next() {
        let right = iter.next();

        let combined_hash = match &right {
            Some(r) => hash(&format!("{}{}", left.hash, r.hash)),
            None => hash(&format!("{}{}", left.hash, left.hash)),
        };

        next_level.push(MerkleNode {
            hash: combined_hash,
            left: Some(Box::new(left.clone())),
            right: right.map(Box::new),
            data: None,
        });
    }

    build_tree_from_leaves(next_level)
}

pub fn get_merkle_root(accounts: &BTreeMap<String, AccountState>) -> String {
    match create_merkle_tree(accounts) {
        Some(tree) => tree.hash,
        None => hash("empty"),
    }
}

pub fn get_merkle_proof(
    accounts: &BTreeMap<String, AccountState>,
    fingerprint: &str,
) -> Option<(Vec<String>, usize)> {
    let entries: Vec<_> = accounts.iter().collect();
    let index = entries.iter().position(|(fp, _)| *fp == fingerprint)?;

    let leaves: Vec<MerkleNode> = entries
        .iter()
        .map(|(fp, state)| {
            let leaf_data = format!("{}:{}:{}", fp, state.balance, state.nonce);
            MerkleNode {
                hash: hash(&leaf_data),
                left: None,
                right: None,
                data: Some(fp.to_string()),
            }
        })
        .collect();

    let mut proof: Vec<String> = Vec::new();
    let mut current_level = leaves;
    let mut current_index = index;

    while current_level.len() > 1 {
        let sibling_index = if current_index % 2 == 0 {
            current_index + 1
        } else {
            current_index - 1
        };

        if sibling_index < current_level.len() {
            proof.push(current_level[sibling_index].hash.clone());
        }

        let mut next_level: Vec<MerkleNode> = Vec::new();
        let mut i = 0;
        while i < current_level.len() {
            let left = &current_level[i];
            let right = current_level.get(i + 1);

            let combined_hash = match right {
                Some(r) => hash(&format!("{}{}", left.hash, r.hash)),
                None => hash(&format!("{}{}", left.hash, left.hash)),
            };

            next_level.push(MerkleNode {
                hash: combined_hash,
                left: None,
                right: None,
                data: None,
            });

            i += 2;
        }

        current_level = next_level;
        current_index /= 2;
    }

    Some((proof, index))
}

pub fn verify_merkle_proof(leaf_hash: &str, proof: &[String], index: usize, root: &str) -> bool {
    let mut current_hash = leaf_hash.to_string();
    let mut current_index = index;

    for sibling_hash in proof {
        current_hash = if current_index % 2 == 0 {
            hash(&format!("{}{}", current_hash, sibling_hash))
        } else {
            hash(&format!("{}{}", sibling_hash, current_hash))
        };
        current_index /= 2;
    }

    current_hash == root
}

pub fn get_transaction_merkle_root(tx_hashes: &[String]) -> String {
    if tx_hashes.is_empty() {
        return hash("empty-tx-tree");
    }

    let mut sorted: Vec<_> = tx_hashes.to_vec();
    sorted.sort();

    let leaves: Vec<MerkleNode> = sorted
        .iter()
        .map(|h| MerkleNode {
            hash: h.clone(),
            left: None,
            right: None,
            data: None,
        })
        .collect();

    build_tree_from_leaves(leaves).hash
}

pub fn get_transaction_merkle_proof(
    tx_hashes: &[String],
    target_hash: &str,
) -> Option<(Vec<String>, usize)> {
    let mut sorted: Vec<_> = tx_hashes.to_vec();
    sorted.sort();

    let index = sorted.iter().position(|h| h == target_hash)?;

    let leaves: Vec<MerkleNode> = sorted
        .iter()
        .map(|h| MerkleNode {
            hash: h.clone(),
            left: None,
            right: None,
            data: None,
        })
        .collect();

    let mut proof: Vec<String> = Vec::new();
    let mut current_level = leaves;
    let mut current_index = index;

    while current_level.len() > 1 {
        let sibling_index = if current_index % 2 == 0 {
            current_index + 1
        } else {
            current_index - 1
        };

        if sibling_index < current_level.len() {
            proof.push(current_level[sibling_index].hash.clone());
        }

        let mut next_level: Vec<MerkleNode> = Vec::new();
        let mut i = 0;
        while i < current_level.len() {
            let left = &current_level[i];
            let right = current_level.get(i + 1);

            let combined_hash = match right {
                Some(r) => hash(&format!("{}{}", left.hash, r.hash)),
                None => hash(&format!("{}{}", left.hash, left.hash)),
            };

            next_level.push(MerkleNode {
                hash: combined_hash,
                left: None,
                right: None,
                data: None,
            });

            i += 2;
        }

        current_level = next_level;
        current_index /= 2;
    }

    Some((proof, index))
}

#[cfg(test)]
mod merkle_tests {
    use super::*;

    fn create_account(fingerprint: &str, balance: f64, nonce: u64) -> AccountState {
        AccountState {
            fingerprint: fingerprint.to_string(),
            balance,
            nonce,
            first_tx_timestamp: 0,
        }
    }

    #[test]
    fn test_empty_tree() {
        let accounts: BTreeMap<String, AccountState> = BTreeMap::new();
        let tree = create_merkle_tree(&accounts);
        assert!(tree.is_none());
    }

    #[test]
    fn test_single_account() {
        let mut accounts = BTreeMap::new();
        accounts.insert("account1".to_string(), create_account("account1", 1000.0, 1));
        let tree = create_merkle_tree(&accounts);
        assert!(tree.is_some());
    }

    #[test]
    fn test_deterministic_root() {
        let mut accounts = BTreeMap::new();
        accounts.insert("a".to_string(), create_account("a", 100.0, 1));
        accounts.insert("b".to_string(), create_account("b", 200.0, 2));

        let root1 = get_merkle_root(&accounts);
        let root2 = get_merkle_root(&accounts);
        assert_eq!(root1, root2);
    }

    #[test]
    fn test_root_changes_with_balance() {
        let mut accounts1 = BTreeMap::new();
        accounts1.insert("a".to_string(), create_account("a", 100.0, 1));

        let mut accounts2 = BTreeMap::new();
        accounts2.insert("a".to_string(), create_account("a", 200.0, 1));

        let root1 = get_merkle_root(&accounts1);
        let root2 = get_merkle_root(&accounts2);
        assert_ne!(root1, root2);
    }

    #[test]
    fn test_proof_generation() {
        let mut accounts = BTreeMap::new();
        accounts.insert("a".to_string(), create_account("a", 100.0, 1));
        accounts.insert("b".to_string(), create_account("b", 200.0, 2));
        accounts.insert("c".to_string(), create_account("c", 300.0, 3));

        let result = get_merkle_proof(&accounts, "b");
        assert!(result.is_some());
    }

    #[test]
    fn test_proof_verification() {
        let mut accounts = BTreeMap::new();
        accounts.insert("testaccount".to_string(), create_account("testaccount", 100.0, 1));
        accounts.insert("other".to_string(), create_account("other", 200.0, 2));

        let root = get_merkle_root(&accounts);
        let (proof, index) = get_merkle_proof(&accounts, "testaccount").unwrap();

        let leaf_hash = hash("testaccount:100:1");
        let valid = verify_merkle_proof(&leaf_hash, &proof, index, &root);
        assert!(valid);
    }

    #[test]
    fn test_invalid_proof() {
        let mut accounts = BTreeMap::new();
        accounts.insert("a".to_string(), create_account("a", 100.0, 1));
        accounts.insert("b".to_string(), create_account("b", 200.0, 2));

        let root = get_merkle_root(&accounts);
        let (proof, index) = get_merkle_proof(&accounts, "a").unwrap();

        let valid = verify_merkle_proof("fake_leaf_hash", &proof, index, &root);
        assert!(!valid);
    }

    #[test]
    fn test_transaction_merkle_root() {
        let tx_hashes = vec!["hash1".to_string(), "hash2".to_string(), "hash3".to_string()];
        let root = get_transaction_merkle_root(&tx_hashes);
        assert!(!root.is_empty());
    }

    #[test]
    fn test_transaction_proof() {
        let tx_hashes = vec![
            "aaa".to_string(),
            "bbb".to_string(),
            "ccc".to_string(),
            "ddd".to_string(),
        ];
        let target = "bbb";
        let root = get_transaction_merkle_root(&tx_hashes);
        let (proof, index) = get_transaction_merkle_proof(&tx_hashes, target).unwrap();

        let valid = verify_merkle_proof(target, &proof, index, &root);
        assert!(valid);
    }
}
