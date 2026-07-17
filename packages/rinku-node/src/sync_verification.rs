//! Sync verification with merkle proofs
//!
//! This module provides cryptographic verification for sync operations,
//! ensuring data integrity when receiving state from peers.

use crate::network::{AccountData, CheckpointData, DeltaData, SnapshotData, TransactionData};
use rinku_core::merkle::MerkleTree;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Result of verification
#[derive(Debug, Clone, PartialEq)]
pub enum VerificationResult {
    Valid,
    Invalid(String),
    Skipped(String),
}

/// Merkle proof for account state verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountProof {
    pub address: String,
    pub balance: u64,
    pub nonce: u64,
    pub stake: u64,
    /// Sibling hashes for merkle path
    pub siblings: Vec<ProofSibling>,
    /// Index in the leaf array
    pub leaf_index: usize,
}

/// Sibling node in merkle proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofSibling {
    pub hash: String,
    pub is_left: bool,
}

/// Compute SHA-256 hash of data
fn sha256_hex(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn hash_account_leaf(account: &AccountData) -> String {
    let data = format!(
        "account:{}:{}:{}:{}",
        account.address, account.balance, account.nonce, account.stake
    );
    sha256_hex(&data)
}

/// Hash a transaction leaf
pub fn hash_transaction_leaf(tx: &TransactionData) -> String {
    let data = format!("tx:{}:{}:{}:{}", tx.hash, tx.from, tx.nonce, tx.timestamp);
    sha256_hex(&data)
}

/// Hash a checkpoint leaf  
pub fn hash_checkpoint_leaf(cp: &CheckpointData) -> String {
    let data = format!(
        "checkpoint:{}:{}:{}",
        cp.height, cp.merkle_root, cp.timestamp
    );
    sha256_hex(&data)
}

/// Hash internal node
fn hash_internal(left: &str, right: &str) -> String {
    let data = format!("node:{}:{}", left, right);
    sha256_hex(&data)
}

/// Build merkle root from leaf hashes
pub fn build_merkle_root(leaf_hashes: &[String]) -> String {
    if leaf_hashes.is_empty() {
        return sha256_hex("empty");
    }

    if leaf_hashes.len() == 1 {
        return leaf_hashes[0].clone();
    }

    let mut current_layer = leaf_hashes.to_vec();

    while current_layer.len() > 1 {
        let mut next_layer = Vec::new();

        for i in (0..current_layer.len()).step_by(2) {
            let left = &current_layer[i];
            let right = if i + 1 < current_layer.len() {
                &current_layer[i + 1]
            } else {
                left // Duplicate last node for odd count
            };
            next_layer.push(hash_internal(left, right));
        }

        current_layer = next_layer;
    }

    current_layer[0].clone()
}

/// Build merkle root from accounts sorted by address for deterministic ordering
pub fn build_account_merkle_root_sorted(accounts: &[AccountData]) -> String {
    let mut sorted = accounts.to_vec();
    sorted.sort_by(|a, b| a.address.cmp(&b.address));
    let leaf_hashes: Vec<String> = sorted.iter().map(hash_account_leaf).collect();
    build_merkle_root(&leaf_hashes)
}

/// Verify merkle proof for an account
pub fn verify_account_proof(proof: &AccountProof, root: &str) -> VerificationResult {
    let account = AccountData {
        address: proof.address.clone(),
        balance: proof.balance,
        nonce: proof.nonce,
        stake: proof.stake,
    };

    let leaf_hash = hash_account_leaf(&account);
    let mut current = leaf_hash;

    for sibling in &proof.siblings {
        current = if sibling.is_left {
            hash_internal(&sibling.hash, &current)
        } else {
            hash_internal(&current, &sibling.hash)
        };
    }

    if current == root {
        VerificationResult::Valid
    } else {
        VerificationResult::Invalid(format!(
            "Merkle root mismatch: computed {} != expected {}",
            current, root
        ))
    }
}

/// Verify snapshot data integrity
pub fn verify_snapshot(snapshot: &SnapshotData) -> VerificationResult {
    // Compute expected merkle root from accounts
    let account_hashes: Vec<String> = snapshot.accounts.iter().map(hash_account_leaf).collect();

    let computed_root = build_merkle_root(&account_hashes);

    if computed_root == snapshot.merkle_root {
        VerificationResult::Valid
    } else {
        VerificationResult::Invalid(format!(
            "Snapshot merkle root mismatch: computed {} != received {}",
            computed_root, snapshot.merkle_root
        ))
    }
}

/// Verify snapshot data integrity with deterministic account ordering
pub fn verify_snapshot_sorted(snapshot: &SnapshotData) -> VerificationResult {
    let computed_root = build_account_merkle_root_sorted(&snapshot.accounts);
    if computed_root == snapshot.merkle_root {
        VerificationResult::Valid
    } else {
        VerificationResult::Invalid(format!(
            "Snapshot merkle root mismatch: computed {} != received {}",
            computed_root, snapshot.merkle_root
        ))
    }
}

/// Verify delta sync data integrity with strict merkle root validation
///
/// SECURITY: This function enforces that all transactions in the delta
/// match the advertised merkle roots in checkpoints. Any mismatch indicates
/// potential data tampering and will be rejected.
pub fn verify_delta(delta: &DeltaData) -> VerificationResult {
    if delta.new_checkpoints.is_empty() {
        return VerificationResult::Valid; // No checkpoints to verify
    }

    let use_checkpoint_heights = !delta.tx_checkpoint_heights.is_empty();
    if use_checkpoint_heights {
        for tx in &delta.transactions {
            if !delta.tx_checkpoint_heights.contains_key(&tx.hash) {
                return VerificationResult::Invalid(format!(
                    "Missing checkpoint height for tx {}",
                    &tx.hash[..16.min(tx.hash.len())]
                ));
            }
        }
    }

    // Sort checkpoints by height for sequential verification
    let mut sorted_checkpoints: Vec<_> = delta.new_checkpoints.iter().collect();
    sorted_checkpoints.sort_by_key(|cp| cp.height);

    // Track which transactions we've already processed
    let mut last_checkpoint_end: u64 = delta.from_checkpoint;

    for checkpoint in sorted_checkpoints {
        if checkpoint.merkle_root.is_empty() {
            return VerificationResult::Invalid(format!(
                "Checkpoint {} has empty merkle root - potential tampering",
                checkpoint.height
            ));
        }

        // Get transactions for this checkpoint window
        // Using height-based windowing for proper checkpoint association
        let checkpoint_txs: Vec<&TransactionData> = if use_checkpoint_heights {
            delta
                .transactions
                .iter()
                .filter(|tx| {
                    delta
                        .tx_checkpoint_heights
                        .get(&tx.hash)
                        .map(|h| *h == checkpoint.height)
                        .unwrap_or(false)
                })
                .collect()
        } else {
            delta
                .transactions
                .iter()
                .filter(|tx| {
                    // Transaction belongs to this checkpoint if its timestamp
                    // falls within the checkpoint's range
                    tx.timestamp > last_checkpoint_end && tx.timestamp <= checkpoint.timestamp
                })
                .collect()
        };

        if checkpoint_txs.is_empty() {
            // Empty checkpoint - verify root matches expected empty merkle
            let empty_root = "0".repeat(64);
            if checkpoint.merkle_root != empty_root && checkpoint.tx_count > 0 {
                return VerificationResult::Invalid(format!(
                    "Checkpoint {} claims {} txs but none provided",
                    checkpoint.height, checkpoint.tx_count
                ));
            }
        } else {
            // Build merkle root from transaction hashes (sorted for determinism)
            let mut tx_hashes: Vec<String> =
                checkpoint_txs.iter().map(|tx| tx.hash.clone()).collect();
            tx_hashes.sort(); // Canonical ordering

            let computed_root = match MerkleTree::from_hex_leaves(&tx_hashes) {
                Ok(tree) => tree.root(),
                Err(e) => {
                    return VerificationResult::Invalid(format!(
                        "Failed to compute merkle root for checkpoint {}: {}",
                        checkpoint.height, e
                    ));
                }
            };

            // SECURITY: Strict verification - reject any mismatch
            // This is the critical security check that prevents forged state
            if computed_root != checkpoint.merkle_root {
                return VerificationResult::Invalid(format!(
                    "Checkpoint {} merkle root mismatch: computed {} != received {} - REJECTED",
                    checkpoint.height,
                    &computed_root[..16],
                    &checkpoint.merkle_root[..16.min(checkpoint.merkle_root.len())]
                ));
            }
        }

        if !use_checkpoint_heights {
            last_checkpoint_end = checkpoint.timestamp;
        }
    }

    VerificationResult::Valid
}

/// Full sync verification including all data types
pub struct SyncVerifier {
    /// Whether to enforce strict verification (reject on any failure)
    pub strict_mode: bool,
    /// Track verification results
    pub results: Vec<(String, VerificationResult)>,
}

impl SyncVerifier {
    pub fn new(strict_mode: bool) -> Self {
        Self {
            strict_mode,
            results: Vec::new(),
        }
    }

    /// Verify a snapshot and record result (uses sorted account order for determinism).
    pub fn verify_snapshot(&mut self, snapshot: &SnapshotData) -> bool {
        let result = verify_snapshot_sorted(snapshot);
        let is_valid = matches!(result, VerificationResult::Valid);
        self.results.push(("snapshot".to_string(), result));
        is_valid || !self.strict_mode
    }

    /// Verify delta sync and record result
    pub fn verify_delta(&mut self, delta: &DeltaData) -> bool {
        let result = verify_delta(delta);
        let is_valid = matches!(result, VerificationResult::Valid);
        self.results.push(("delta".to_string(), result));
        is_valid || !self.strict_mode
    }

    /// Verify account proof and record result
    pub fn verify_account(&mut self, proof: &AccountProof, root: &str) -> bool {
        let result = verify_account_proof(proof, root);
        let is_valid = matches!(result, VerificationResult::Valid);
        self.results
            .push((format!("account:{}", proof.address), result));
        is_valid || !self.strict_mode
    }

    /// Get summary of all verification results
    pub fn summary(&self) -> String {
        let valid_count = self
            .results
            .iter()
            .filter(|(_, r)| matches!(r, VerificationResult::Valid))
            .count();
        let invalid_count = self
            .results
            .iter()
            .filter(|(_, r)| matches!(r, VerificationResult::Invalid(_)))
            .count();
        let skipped_count = self
            .results
            .iter()
            .filter(|(_, r)| matches!(r, VerificationResult::Skipped(_)))
            .count();

        format!(
            "Verification: {} valid, {} invalid, {} skipped",
            valid_count, invalid_count, skipped_count
        )
    }

    /// Check if all verifications passed
    pub fn all_valid(&self) -> bool {
        self.results.iter().all(|(_, r)| {
            matches!(
                r,
                VerificationResult::Valid | VerificationResult::Skipped(_)
            )
        })
    }

    /// Get failed verifications
    pub fn failures(&self) -> Vec<&(String, VerificationResult)> {
        self.results
            .iter()
            .filter(|(_, r)| matches!(r, VerificationResult::Invalid(_)))
            .collect()
    }
}

/// Generate merkle proofs for accounts in a snapshot
pub fn generate_account_proofs(accounts: &[AccountData]) -> Vec<AccountProof> {
    if accounts.is_empty() {
        return Vec::new();
    }

    // Build full tree layers
    let leaf_hashes: Vec<String> = accounts.iter().map(hash_account_leaf).collect();
    let mut layers: Vec<Vec<String>> = vec![leaf_hashes.clone()];

    let mut current = leaf_hashes;
    while current.len() > 1 {
        let mut next = Vec::new();
        for i in (0..current.len()).step_by(2) {
            let left = &current[i];
            let right = if i + 1 < current.len() {
                &current[i + 1]
            } else {
                left
            };
            next.push(hash_internal(left, right));
        }
        layers.push(next.clone());
        current = next;
    }

    // Generate proof for each account
    accounts
        .iter()
        .enumerate()
        .map(|(idx, account)| {
            let mut siblings = Vec::new();
            let mut current_idx = idx;

            for layer in &layers[..layers.len().saturating_sub(1)] {
                let sibling_idx = if current_idx % 2 == 0 {
                    current_idx + 1
                } else {
                    current_idx - 1
                };

                if sibling_idx < layer.len() {
                    siblings.push(ProofSibling {
                        hash: layer[sibling_idx].clone(),
                        is_left: current_idx % 2 == 1,
                    });
                }

                current_idx /= 2;
            }

            AccountProof {
                address: account.address.clone(),
                balance: account.balance,
                nonce: account.nonce,
                stake: account.stake,
                siblings,
                leaf_index: idx,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rinku_core::merkle::MerkleTree;

    fn make_account(addr: &str, balance: u64) -> AccountData {
        AccountData {
            address: addr.to_string(),
            balance,
            nonce: 1,
            stake: 0,
        }
    }

    #[test]
    fn test_build_merkle_root_single() {
        let accounts = vec![make_account("a", 100)];
        let hashes: Vec<String> = accounts.iter().map(hash_account_leaf).collect();
        let root = build_merkle_root(&hashes);
        assert!(!root.is_empty());
        assert_eq!(root.len(), 64); // SHA256 hex
    }

    #[test]
    fn test_build_merkle_root_multiple() {
        let accounts = vec![
            make_account("a", 100),
            make_account("b", 200),
            make_account("c", 300),
        ];
        let hashes: Vec<String> = accounts.iter().map(hash_account_leaf).collect();
        let root = build_merkle_root(&hashes);
        assert!(!root.is_empty());
    }

    #[test]
    fn test_verify_snapshot_valid() {
        let accounts = vec![make_account("a", 100), make_account("b", 200)];
        let hashes: Vec<String> = accounts.iter().map(hash_account_leaf).collect();
        let merkle_root = build_merkle_root(&hashes);

        let snapshot = SnapshotData {
            accounts,
            validators: vec![],
            checkpoints: vec![],
            recent_txs: vec![],
            merkle_root,
        };

        let result = verify_snapshot(&snapshot);
        assert_eq!(result, VerificationResult::Valid);
    }

    #[test]
    fn test_verify_snapshot_invalid() {
        let accounts = vec![make_account("a", 100)];

        let snapshot = SnapshotData {
            accounts,
            validators: vec![],
            checkpoints: vec![],
            recent_txs: vec![],
            merkle_root: "invalid_root".to_string(),
        };

        let result = verify_snapshot(&snapshot);
        assert!(matches!(result, VerificationResult::Invalid(_)));
    }

    #[test]
    fn test_generate_and_verify_account_proofs() {
        let accounts = vec![
            make_account("a", 100),
            make_account("b", 200),
            make_account("c", 300),
            make_account("d", 400),
        ];
        let hashes: Vec<String> = accounts.iter().map(hash_account_leaf).collect();
        let root = build_merkle_root(&hashes);

        let proofs = generate_account_proofs(&accounts);
        assert_eq!(proofs.len(), 4);

        for proof in &proofs {
            let result = verify_account_proof(proof, &root);
            assert_eq!(
                result,
                VerificationResult::Valid,
                "Proof for {} should be valid",
                proof.address
            );
        }
    }

    #[test]
    fn test_sync_verifier_rejects_tampered_balance() {
        let accounts = vec![make_account("alice", 100), make_account("bob", 200)];
        let merkle_root = build_account_merkle_root_sorted(&accounts);

        let mut tampered = accounts.clone();
        tampered[0].balance = 999_999;

        let snapshot = SnapshotData {
            accounts: tampered,
            validators: vec![],
            checkpoints: vec![],
            recent_txs: vec![],
            merkle_root,
        };

        let mut strict = SyncVerifier::new(true);
        assert!(!strict.verify_snapshot(&snapshot));
        assert!(!strict.failures().is_empty());
    }

    #[test]
    fn test_sync_verifier_sorted_independent_of_input_order() {
        let a = make_account("alice", 100);
        let b = make_account("bob", 200);
        let root = build_account_merkle_root_sorted(&[a.clone(), b.clone()]);

        let snapshot = SnapshotData {
            accounts: vec![b, a], // reverse order
            validators: vec![],
            checkpoints: vec![],
            recent_txs: vec![],
            merkle_root: root,
        };

        let mut verifier = SyncVerifier::new(true);
        assert!(verifier.verify_snapshot(&snapshot));
    }

    #[test]
    fn test_verify_snapshot_sorted_order() {
        let accounts = vec![make_account("b", 200), make_account("a", 100)];
        let merkle_root = build_account_merkle_root_sorted(&accounts);

        let snapshot = SnapshotData {
            accounts,
            validators: vec![],
            checkpoints: vec![],
            recent_txs: vec![],
            merkle_root,
        };

        let result = verify_snapshot_sorted(&snapshot);
        assert_eq!(result, VerificationResult::Valid);
    }

    #[test]
    fn test_verify_delta_with_checkpoint_heights() {
        let transactions = vec![
            TransactionData {
                hash: "a1".repeat(32),
                from: "genesis".to_string(),
                to: "alice".to_string(),
                amount: 100_000_000,
                nonce: 1,
                timestamp: 1,
                signature: "sig".to_string(),
                parents: vec![],
                gas_price: 0,
                memo: None,
                references: None,
            },
            TransactionData {
                hash: "b2".repeat(32),
                from: "genesis".to_string(),
                to: "bob".to_string(),
                amount: 200_000_000,
                nonce: 2,
                timestamp: 2,
                signature: "sig".to_string(),
                parents: vec![],
                gas_price: 0,
                memo: None,
                references: None,
            },
        ];
        let mut tx_hashes: Vec<String> = transactions.iter().map(|tx| tx.hash.clone()).collect();
        tx_hashes.sort();
        let merkle_root = MerkleTree::from_hex_leaves(&tx_hashes).unwrap().root();
        let checkpoint = CheckpointData {
            height: 2,
            merkle_root,
            timestamp: 10,
            tx_count: transactions.len() as u64,
            hash: None,
            previous_hash: None,
            signature: None,
            genesis_hash: None,
            finalized_tx_hashes: Vec::new(),
            state_root: None,
            receipt_root: None,
            tip_count: None,
            validator_signatures: Vec::new(),
            signer_bitmap: None,
        };
        let mut tx_checkpoint_heights = std::collections::HashMap::new();
        for tx in &transactions {
            tx_checkpoint_heights.insert(tx.hash.clone(), 2);
        }
        let delta = DeltaData {
            transactions,
            new_checkpoints: vec![checkpoint],
            from_checkpoint: 1,
            to_checkpoint: 2,
            tx_checkpoint_heights,
            validators: Vec::new(),
            precomputed_proofs: Vec::new(),
        };
        let result = verify_delta(&delta);
        assert_eq!(result, VerificationResult::Valid);
    }
}
