use crate::crypto::{sha256, sha256_hex};
use crate::types::{
    from_micro_units, Account, AggregatedWeight, PendingWeightVote, WeightTrieLeaf, WeightVote,
};
use std::collections::HashMap;

/// Convert a public key (hex) to an address (first 40 chars of SHA256 hash of raw bytes)
/// This matches the frontend's fingerprint derivation: sha256(publicKeyBytes).slice(0, 40)
fn pubkey_to_address(pubkey_hex: &str) -> String {
    // Decode the pubkey hex string to raw bytes and hash those
    if let Ok(pubkey_bytes) = hex::decode(pubkey_hex) {
        let hash = sha256(&pubkey_bytes);
        hex::encode(&hash[..20])
    } else {
        String::new()
    }
}

const MIN_BOND_FOR_AGE_WEIGHT: f64 = 100.0;
const AGE_WEIGHT_DECAY_PER_MISSED: f64 = 0.10;
const MAX_AGE_WEIGHT: f64 = 10.0;
const BALANCE_WEIGHT_EXPONENT: f64 = 0.5;

pub fn calculate_account_weight(account: &Account, current_time: u64) -> f64 {
    let age_weight = calculate_age_weight(account, current_time);
    let balance_rku = from_micro_units(account.balance);
    let staked_rku = from_micro_units(account.staked);
    let balance_weight = calculate_balance_weight(balance_rku);
    let stake_weight = calculate_stake_weight(staked_rku);

    let base_weight = age_weight * balance_weight + stake_weight;
    base_weight * (1.0 - account.reputation_penalty.clamp(0.0, 1.0))
}

pub fn calculate_age_weight(account: &Account, current_time: u64) -> f64 {
    let staked_rku = from_micro_units(account.staked);
    if staked_rku < MIN_BOND_FOR_AGE_WEIGHT {
        return 1.0;
    }

    let age_seconds = current_time.saturating_sub(account.first_seen);
    let age_days = age_seconds as f64 / 86400.0;

    (1.0 + age_days.ln_1p()).min(MAX_AGE_WEIGHT)
}

pub fn calculate_balance_weight(balance: f64) -> f64 {
    if balance <= 0.0 {
        return 0.0;
    }
    balance.powf(BALANCE_WEIGHT_EXPONENT)
}

pub fn calculate_stake_weight(stake: f64) -> f64 {
    if stake <= 0.0 {
        return 0.0;
    }
    stake.powf(BALANCE_WEIGHT_EXPONENT) * 2.0
}

pub fn calculate_validator_weight(
    stake: u64,
    first_stake_time: u64,
    current_time: u64,
    missed_checkpoints: u32,
) -> f64 {
    let stake_rku = from_micro_units(stake);
    if stake_rku < MIN_BOND_FOR_AGE_WEIGHT {
        return stake_rku.powf(BALANCE_WEIGHT_EXPONENT);
    }

    let age_seconds = current_time.saturating_sub(first_stake_time);
    let age_days = age_seconds as f64 / 86400.0;
    let age_factor = (1.0 + age_days.ln_1p()).min(MAX_AGE_WEIGHT);

    let decay = (1.0 - AGE_WEIGHT_DECAY_PER_MISSED).powi(missed_checkpoints as i32);
    let decayed_age_factor = age_factor * decay;

    stake_rku.powf(BALANCE_WEIGHT_EXPONENT) * decayed_age_factor
}

pub fn calculate_transaction_weight(
    sender_weight: f64,
    gas_paid: u64,
    is_consolidation: bool,
) -> f64 {
    if is_consolidation {
        return sender_weight * 0.1;
    }

    let gas_rku = from_micro_units(gas_paid);
    sender_weight + (gas_rku * 10.0)
}

pub fn total_validator_weight(validators: &[(u64, u64, u32)], current_time: u64) -> f64 {
    validators
        .iter()
        .map(|(stake, first_stake, missed)| {
            calculate_validator_weight(*stake, *first_stake, current_time, *missed)
        })
        .sum()
}

pub fn required_weight_for_finality(total_weight: f64) -> f64 {
    total_weight * 2.0 / 3.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_balance_weight() {
        assert_eq!(calculate_balance_weight(0.0), 0.0);
        assert_eq!(calculate_balance_weight(100.0), 10.0);
        assert_eq!(calculate_balance_weight(10000.0), 100.0);
    }

    #[test]
    fn test_stake_weight() {
        assert_eq!(calculate_stake_weight(0.0), 0.0);
        let stake_weight = calculate_stake_weight(100.0);
        assert!(stake_weight > calculate_balance_weight(100.0));
    }

    #[test]
    fn test_age_weight_requires_min_bond() {
        use crate::types::to_micro_units;
        let account = Account {
            address: "test".to_string(),
            balance: to_micro_units(1000.0),
            nonce: 0,
            first_seen: 0,
            staked: to_micro_units(50.0),
            unbonding: 0,
            unbonding_release: None,
            latest_balance_proof: None,
            partition_violations: 0,
            reputation_penalty: 0.0,
            penalty_decay_checkpoint: None,
            partition_budget: None,
            partition_budget_spent: 0,
            ecdsa_public_key: None,
        };

        let weight = calculate_age_weight(&account, 86400 * 30);
        assert_eq!(weight, 1.0);
    }

    #[test]
    fn test_age_weight_with_stake() {
        use crate::types::to_micro_units;
        let account = Account {
            address: "test".to_string(),
            balance: to_micro_units(1000.0),
            nonce: 0,
            first_seen: 0,
            staked: to_micro_units(100.0),
            unbonding: 0,
            unbonding_release: None,
            latest_balance_proof: None,
            partition_violations: 0,
            reputation_penalty: 0.0,
            penalty_decay_checkpoint: None,
            partition_budget: None,
            partition_budget_spent: 0,
            ecdsa_public_key: None,
        };

        let weight_30_days = calculate_age_weight(&account, 86400 * 30);
        assert!(weight_30_days > 1.0);
        assert!(weight_30_days <= MAX_AGE_WEIGHT);
    }

    #[test]
    fn test_validator_weight_decay() {
        use crate::types::to_micro_units;
        let stake = to_micro_units(1000.0);
        let first_stake = 0;
        let current = 86400 * 30;

        let weight_0_missed = calculate_validator_weight(stake, first_stake, current, 0);
        let weight_5_missed = calculate_validator_weight(stake, first_stake, current, 5);

        assert!(weight_5_missed < weight_0_missed);
    }

    #[test]
    fn test_finality_threshold() {
        let total = 1000.0;
        let required = required_weight_for_finality(total);
        assert!((required - 666.67).abs() < 1.0);
    }
}

// ============================================================================
// WEIGHT TRIE - Merkle structure for transaction weight attestations
// ============================================================================

/// Hash a weight trie leaf for Merkle tree inclusion
/// Format: SHA256("weight:{tx_hash}:{boost_micro}:{suppress_micro}:{neutral_micro}:{total_stake}:{count}")
/// Includes all fields for deterministic AggregatedWeight reconstruction
pub fn hash_weight_leaf(leaf: &WeightTrieLeaf) -> String {
    let data = format!(
        "weight:{}:{}:{}:{}:{}:{}",
        leaf.tx_hash,
        leaf.boost_stake_micro,
        leaf.suppress_stake_micro,
        leaf.neutral_stake_micro,
        leaf.total_network_stake_micro,
        leaf.attestation_count
    );
    sha256_hex(&data)
}

/// Reconstruct AggregatedWeight from a WeightTrieLeaf (for offline verification)
pub fn leaf_to_aggregated_weight(leaf: &WeightTrieLeaf) -> AggregatedWeight {
    let net_weight = leaf.boost_stake_micro as i64 - leaf.suppress_stake_micro as i64;
    AggregatedWeight {
        boost_stake_micro: leaf.boost_stake_micro,
        suppress_stake_micro: leaf.suppress_stake_micro,
        neutral_stake_micro: leaf.neutral_stake_micro,
        net_weight,
        attestation_count: leaf.attestation_count,
        total_network_stake_micro: leaf.total_network_stake_micro,
    }
}

/// Weight Trie for aggregating and proving transaction weights
#[derive(Debug, Clone, Default)]
pub struct WeightTrie {
    /// Map of tx_hash -> aggregated weights
    weights: HashMap<String, AggregatedWeight>,
    /// Pending votes before checkpoint aggregation
    pending_votes: Vec<PendingWeightVote>,
    /// Cached Merkle root (invalidated on changes)
    cached_root: Option<String>,
}

impl WeightTrie {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the aggregated weight for a transaction hash
    pub fn get_weight(&self, tx_hash: &str) -> Option<&AggregatedWeight> {
        self.weights.get(tx_hash)
    }

    /// Add a pending weight vote from a validator
    pub fn add_vote(&mut self, vote: PendingWeightVote) {
        self.pending_votes.push(vote);
        self.cached_root = None;
    }

    /// Process all pending votes and aggregate into final weights
    /// Called during checkpoint finalization
    ///
    /// Parameters:
    /// - validator_stakes: map of validator_pubkey -> stake_micro at this checkpoint
    /// - total_network_stake: sum of all validator stakes
    pub fn finalize_votes(
        &mut self,
        validator_stakes: &HashMap<String, u64>,
        total_network_stake: u64,
    ) -> Vec<(String, AggregatedWeight)> {
        // Group pending votes by tx_hash
        let mut tx_votes: HashMap<String, Vec<&PendingWeightVote>> = HashMap::new();
        for vote in &self.pending_votes {
            tx_votes.entry(vote.tx_hash.clone()).or_default().push(vote);
        }

        let mut updated = Vec::new();

        for (tx_hash, votes) in tx_votes {
            // Deduplicate by validator (last vote wins)
            let mut validator_votes: HashMap<String, &PendingWeightVote> = HashMap::new();
            for vote in votes {
                validator_votes.insert(vote.validator_pubkey.clone(), vote);
            }

            // Aggregate weights
            let mut boost_stake: u64 = 0;
            let mut suppress_stake: u64 = 0;
            let mut neutral_stake: u64 = 0;
            let mut attestation_count: u32 = 0;

            for (validator, vote) in validator_votes {
                // Look up stake: first try validator key as-is (might be address),
                // then derive address from pubkey if it looks like a full public key
                let (stake, derived_addr_for_log) =
                    if let Some(&s) = validator_stakes.get(&validator) {
                        (s, None)
                    } else if validator.len() >= 128 {
                        // Derive address from pubkey (SHA256 of raw bytes, first 40 hex chars)
                        let derived_addr = pubkey_to_address(&validator);
                        // Try exact match first
                        if let Some(&s) = validator_stakes.get(&derived_addr) {
                            (s, Some(derived_addr))
                        } else {
                            // Try prefix matching in case addresses have different lengths
                            let found = validator_stakes
                                .iter()
                                .find(|(k, _)| {
                                    derived_addr.starts_with(*k) || k.starts_with(&derived_addr)
                                })
                                .map(|(_, v)| *v);
                            (found.unwrap_or(0), Some(derived_addr))
                        }
                    } else {
                        (0, None)
                    };

                // Store debug info for logging by caller
                if let Some(addr) = derived_addr_for_log {
                    // We can't log from rinku-core easily, but the stake value will show in aggregation
                    let _ = addr; // Suppress unused warning
                }

                attestation_count += 1;

                match vote.vote {
                    WeightVote::Boost => boost_stake += stake,
                    WeightVote::Suppress => suppress_stake += stake,
                    WeightVote::Neutral => neutral_stake += stake,
                }
            }

            // Merge with existing weights (cumulative)
            let existing = self.weights.entry(tx_hash.clone()).or_default();
            existing.boost_stake_micro = existing.boost_stake_micro.saturating_add(boost_stake);
            existing.suppress_stake_micro =
                existing.suppress_stake_micro.saturating_add(suppress_stake);
            existing.neutral_stake_micro =
                existing.neutral_stake_micro.saturating_add(neutral_stake);
            existing.net_weight =
                existing.boost_stake_micro as i64 - existing.suppress_stake_micro as i64;
            existing.attestation_count =
                existing.attestation_count.saturating_add(attestation_count);
            existing.total_network_stake_micro = total_network_stake;

            updated.push((tx_hash, existing.clone()));
        }

        // Clear pending votes
        self.pending_votes.clear();
        self.cached_root = None;

        updated
    }

    /// Get all weights (for serialization)
    pub fn all_weights(&self) -> &HashMap<String, AggregatedWeight> {
        &self.weights
    }

    /// Load weights from storage
    pub fn load_weights(&mut self, weights: HashMap<String, AggregatedWeight>) {
        self.weights = weights;
        self.cached_root = None;
    }

    /// Compute Merkle root of the weight trie
    pub fn compute_root(&mut self) -> String {
        if let Some(ref root) = self.cached_root {
            return root.clone();
        }

        if self.weights.is_empty() {
            let empty_root = sha256_hex("weight_trie:empty");
            self.cached_root = Some(empty_root.clone());
            return empty_root;
        }

        // Sort by tx_hash for deterministic ordering
        let mut sorted_keys: Vec<_> = self.weights.keys().cloned().collect();
        sorted_keys.sort();

        // Build leaves with all fields for deterministic reconstruction
        let leaves: Vec<String> = sorted_keys
            .iter()
            .map(|tx_hash| {
                let w = &self.weights[tx_hash];
                let leaf = WeightTrieLeaf {
                    tx_hash: tx_hash.clone(),
                    boost_stake_micro: w.boost_stake_micro,
                    suppress_stake_micro: w.suppress_stake_micro,
                    neutral_stake_micro: w.neutral_stake_micro,
                    total_network_stake_micro: w.total_network_stake_micro,
                    attestation_count: w.attestation_count,
                };
                hash_weight_leaf(&leaf)
            })
            .collect();

        // Build Merkle tree
        let root = build_merkle_root(&leaves);
        self.cached_root = Some(root.clone());
        root
    }

    /// Generate a Merkle proof for a specific transaction's weight
    /// Returns (proof, index, leaf) for offline verification
    pub fn generate_proof(
        &mut self,
        tx_hash: &str,
    ) -> Option<(Vec<String>, usize, WeightTrieLeaf)> {
        let weight = self.weights.get(tx_hash)?;

        // Sort keys for deterministic ordering
        let mut sorted_keys: Vec<_> = self.weights.keys().cloned().collect();
        sorted_keys.sort();

        // Find index
        let index = sorted_keys.iter().position(|k| k == tx_hash)?;

        // Create the leaf for this tx
        let target_leaf = WeightTrieLeaf {
            tx_hash: tx_hash.to_string(),
            boost_stake_micro: weight.boost_stake_micro,
            suppress_stake_micro: weight.suppress_stake_micro,
            neutral_stake_micro: weight.neutral_stake_micro,
            total_network_stake_micro: weight.total_network_stake_micro,
            attestation_count: weight.attestation_count,
        };

        // Build all leaves
        let leaves: Vec<String> = sorted_keys
            .iter()
            .map(|key| {
                let w = &self.weights[key];
                let leaf = WeightTrieLeaf {
                    tx_hash: key.clone(),
                    boost_stake_micro: w.boost_stake_micro,
                    suppress_stake_micro: w.suppress_stake_micro,
                    neutral_stake_micro: w.neutral_stake_micro,
                    total_network_stake_micro: w.total_network_stake_micro,
                    attestation_count: w.attestation_count,
                };
                hash_weight_leaf(&leaf)
            })
            .collect();

        // Generate proof
        let proof = build_merkle_proof(&leaves, index);
        Some((proof, index, target_leaf))
    }

    /// Prune weights for transactions older than a checkpoint height
    pub fn prune_before_checkpoint(
        &mut self,
        min_checkpoint: u64,
        tx_checkpoints: &HashMap<String, u64>,
    ) {
        self.weights.retain(|tx_hash, _| {
            tx_checkpoints
                .get(tx_hash)
                .map(|cp| *cp >= min_checkpoint)
                .unwrap_or(false)
        });
        self.cached_root = None;
    }

    /// Get pending vote count
    pub fn pending_vote_count(&self) -> usize {
        self.pending_votes.len()
    }
}

/// Build Merkle root from leaves
fn build_merkle_root(leaves: &[String]) -> String {
    if leaves.is_empty() {
        return sha256_hex("merkle:empty");
    }
    if leaves.len() == 1 {
        return leaves[0].clone();
    }

    let mut current_level = leaves.to_vec();

    while current_level.len() > 1 {
        let mut next_level = Vec::new();

        for chunk in current_level.chunks(2) {
            let combined = if chunk.len() == 2 {
                format!("{}{}", chunk[0], chunk[1])
            } else {
                format!("{}{}", chunk[0], chunk[0]) // Duplicate odd node
            };
            next_level.push(sha256_hex(&combined));
        }

        current_level = next_level;
    }

    current_level[0].clone()
}

/// Build Merkle proof for a specific index
fn build_merkle_proof(leaves: &[String], index: usize) -> Vec<String> {
    if leaves.len() <= 1 {
        return vec![];
    }

    let mut proof = Vec::new();
    let mut current_level = leaves.to_vec();
    let mut current_index = index;

    while current_level.len() > 1 {
        let sibling_index = if current_index.is_multiple_of(2) {
            current_index + 1
        } else {
            current_index - 1
        };

        let sibling = if sibling_index < current_level.len() {
            current_level[sibling_index].clone()
        } else {
            current_level[current_index].clone() // Duplicate for odd count
        };

        proof.push(sibling);

        // Move to next level
        let mut next_level = Vec::new();
        for chunk in current_level.chunks(2) {
            let combined = if chunk.len() == 2 {
                format!("{}{}", chunk[0], chunk[1])
            } else {
                format!("{}{}", chunk[0], chunk[0])
            };
            next_level.push(sha256_hex(&combined));
        }

        current_level = next_level;
        current_index /= 2;
    }

    proof
}

/// Verify a weight proof against a known root
pub fn verify_weight_proof(
    leaf: &WeightTrieLeaf,
    proof: &[String],
    index: usize,
    expected_root: &str,
) -> bool {
    let mut current_hash = hash_weight_leaf(leaf);
    let mut current_index = index;

    for sibling in proof {
        current_hash = if current_index.is_multiple_of(2) {
            sha256_hex(&format!("{}{}", current_hash, sibling))
        } else {
            sha256_hex(&format!("{}{}", sibling, current_hash))
        };
        current_index /= 2;
    }

    current_hash == expected_root
}

#[cfg(test)]
mod weight_trie_tests {
    use super::*;

    #[test]
    fn test_weight_trie_basic() {
        let mut trie = WeightTrie::new();

        // Add some votes
        trie.add_vote(PendingWeightVote {
            tx_hash: "tx1".to_string(),
            validator_pubkey: "val1".to_string(),
            vote: WeightVote::Boost,
            timestamp_ms: 1000,
            bls_signature: None,
        });

        trie.add_vote(PendingWeightVote {
            tx_hash: "tx1".to_string(),
            validator_pubkey: "val2".to_string(),
            vote: WeightVote::Boost,
            timestamp_ms: 1001,
            bls_signature: None,
        });

        // Finalize with validator stakes
        let mut stakes = HashMap::new();
        stakes.insert("val1".to_string(), 1000_u64);
        stakes.insert("val2".to_string(), 2000_u64);

        let updated = trie.finalize_votes(&stakes, 3000);

        assert_eq!(updated.len(), 1);
        let (tx_hash, weight) = &updated[0];
        assert_eq!(tx_hash, "tx1");
        assert_eq!(weight.boost_stake_micro, 3000);
        assert_eq!(weight.suppress_stake_micro, 0);
        assert_eq!(weight.attestation_count, 2);
    }

    #[test]
    fn test_weight_proof_verification() {
        let mut trie = WeightTrie::new();

        // Manually add weights
        let mut weights = HashMap::new();
        weights.insert(
            "tx1".to_string(),
            AggregatedWeight {
                boost_stake_micro: 1000,
                suppress_stake_micro: 0,
                neutral_stake_micro: 0,
                net_weight: 1000,
                attestation_count: 1,
                total_network_stake_micro: 1000,
            },
        );
        weights.insert(
            "tx2".to_string(),
            AggregatedWeight {
                boost_stake_micro: 500,
                suppress_stake_micro: 500,
                neutral_stake_micro: 0,
                net_weight: 0,
                attestation_count: 2,
                total_network_stake_micro: 1000,
            },
        );
        trie.load_weights(weights);

        // Compute root
        let root = trie.compute_root();

        // Generate proof for tx1 - now returns leaf as well
        let (proof, index, leaf) = trie.generate_proof("tx1").unwrap();

        // Verify - leaf returned by generate_proof has all fields
        assert!(verify_weight_proof(&leaf, &proof, index, &root));

        // Verify leaf can reconstruct AggregatedWeight
        let reconstructed = leaf_to_aggregated_weight(&leaf);
        assert_eq!(reconstructed.trust_score(), 100); // All boost, no suppress
    }
}
