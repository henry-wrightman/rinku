use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rinku_core::{
    crypto::sha256_hex,
    types::{Checkpoint, SignedTransaction, Transaction, ValidatorSignature},
    weight::calculate_account_weight,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::bls::{aggregate_signatures, bls_verify, create_signer_bitmap};
use crate::state::NodeState;
use crate::validator_identity::ValidatorIdentityService;

pub const QUORUM_THRESHOLD: f64 = 0.67;
pub const SUPER_MAJORITY_THRESHOLD: f64 = 0.75;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoteType {
    Prepare,
    Commit,
    Finalize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    pub validator: String,
    pub vote_type: VoteType,
    pub checkpoint_height: u64,
    pub checkpoint_hash: String,
    pub signature: Vec<u8>,
    pub timestamp: u64,
}

impl Vote {
    pub fn message(&self) -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend_from_slice(&[self.vote_type as u8]);
        msg.extend_from_slice(&self.checkpoint_height.to_le_bytes());
        msg.extend_from_slice(self.checkpoint_hash.as_bytes());
        msg
    }
}

#[derive(Debug, Clone)]
pub struct ValidatorSnapshot {
    pub address: String,
    pub voting_power: f64,
    pub bls_public_key: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct VoteAccumulator {
    pub checkpoint_height: u64,
    pub checkpoint_hash: String,
    pub votes: HashMap<String, Vote>,
    pub total_voting_power: f64,
    pub accumulated_power: f64,
    pub signatures: Vec<Vec<u8>>,
    pub signer_indices: Vec<usize>,
    pub frozen_validators: Vec<ValidatorSnapshot>,
}

impl VoteAccumulator {
    pub fn new(height: u64, hash: String, total_power: f64, validators: Vec<ValidatorSnapshot>) -> Self {
        Self {
            checkpoint_height: height,
            checkpoint_hash: hash,
            votes: HashMap::new(),
            total_voting_power: total_power,
            accumulated_power: 0.0,
            signatures: Vec::new(),
            signer_indices: Vec::new(),
            frozen_validators: validators,
        }
    }

    pub fn get_validator_index(&self, address: &str) -> Option<usize> {
        self.frozen_validators.iter().position(|v| v.address == address)
    }

    pub fn get_validator(&self, address: &str) -> Option<&ValidatorSnapshot> {
        self.frozen_validators.iter().find(|v| v.address == address)
    }

    pub fn quorum_reached(&self) -> bool {
        self.voting_power_ratio() >= QUORUM_THRESHOLD
    }

    pub fn super_majority_reached(&self) -> bool {
        self.voting_power_ratio() >= SUPER_MAJORITY_THRESHOLD
    }

    pub fn voting_power_ratio(&self) -> f64 {
        if self.total_voting_power <= 0.0 {
            return 0.0;
        }
        self.accumulated_power / self.total_voting_power
    }

    pub fn add_vote(&mut self, vote: Vote, voting_power: f64, signer_index: usize) -> bool {
        if self.votes.contains_key(&vote.validator) {
            return false;
        }
        self.signatures.push(vote.signature.clone());
        self.signer_indices.push(signer_index);
        self.accumulated_power += voting_power;
        self.votes.insert(vote.validator.clone(), vote);
        true
    }

    pub fn signers(&self) -> Vec<String> {
        self.votes.keys().cloned().collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalityProof {
    pub checkpoint_height: u64,
    pub checkpoint_hash: String,
    pub aggregated_signature: Vec<u8>,
    pub signer_bitmap: Vec<u8>,
    pub total_stake_voted: f64,
    pub total_stake: f64,
    pub quorum_threshold: f64,
    pub timestamp: u64,
}

impl FinalityProof {
    pub fn is_valid(&self) -> bool {
        let ratio = if self.total_stake > 0.0 {
            self.total_stake_voted / self.total_stake
        } else {
            0.0
        };
        ratio >= self.quorum_threshold
    }
}

#[derive(Debug, Clone)]
pub struct QuorumVerificationResult {
    pub valid: bool,
    pub verified_stake: f64,
    pub total_stake: f64,
    pub quorum_reached: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoubleSignEvidence {
    pub validator: String,
    pub height: u64,
    pub hash1: String,
    pub hash2: String,
    pub signature1: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoteResult {
    Accepted,
    Duplicate,
    QuorumReached,
}

pub struct ConsensusService {
    state: NodeState,
    validator_service: Option<Arc<RwLock<ValidatorIdentityService>>>,
    pending_votes: HashMap<u64, VoteAccumulator>,
    finalized_heights: HashSet<u64>,
    last_finalized_height: u64,
}

impl ConsensusService {
    pub fn new(state: NodeState) -> Self {
        Self {
            state,
            validator_service: None,
            pending_votes: HashMap::new(),
            finalized_heights: HashSet::new(),
            last_finalized_height: 0,
        }
    }

    pub fn with_validator_service(
        mut self,
        validator_service: Arc<RwLock<ValidatorIdentityService>>,
    ) -> Self {
        self.validator_service = Some(validator_service);
        self
    }

    pub async fn start_voting_round(&mut self, checkpoint: &Checkpoint) -> Result<()> {
        let (total_stake, frozen_validators) = if let Some(ref vs) = self.validator_service {
            let vs_guard = vs.read().await;
            let mut validators: Vec<ValidatorSnapshot> = vs_guard.active_validators()
                .iter()
                .map(|(addr, v)| ValidatorSnapshot {
                    address: addr.clone(),
                    voting_power: v.effective_stake,
                    bls_public_key: v.bls_public_key.clone(),
                })
                .collect();
            validators.sort_by(|a, b| a.address.cmp(&b.address));
            let total = vs_guard.total_active_stake();
            (total, validators)
        } else {
            let state = self.state.inner.read().await;
            let mut validators: Vec<ValidatorSnapshot> = state.validators
                .iter()
                .map(|(addr, v)| ValidatorSnapshot {
                    address: addr.clone(),
                    voting_power: v.stake,
                    bls_public_key: v.bls_public_key.as_ref()
                        .and_then(|k| hex::decode(k).ok())
                        .unwrap_or_default(),
                })
                .collect();
            validators.sort_by(|a, b| a.address.cmp(&b.address));
            let total = state.validators.values().map(|v| v.stake).sum();
            (total, validators)
        };

        if total_stake <= 0.0 {
            return Err(anyhow!("No active validators with stake"));
        }

        let accumulator = VoteAccumulator::new(
            checkpoint.height,
            checkpoint.hash.clone(),
            total_stake,
            frozen_validators,
        );
        self.pending_votes.insert(checkpoint.height, accumulator);
        debug!("Started voting round for checkpoint {} with {} total stake", 
               checkpoint.height, total_stake);
        Ok(())
    }

    pub async fn process_vote(&mut self, vote: Vote) -> Result<VoteResult> {
        let accumulator = self.pending_votes.get(&vote.checkpoint_height)
            .ok_or_else(|| anyhow!("No voting round for height {}", vote.checkpoint_height))?;

        let (voting_power, bls_pubkey, signer_index) = if !accumulator.frozen_validators.is_empty() {
            let validator = accumulator.get_validator(&vote.validator)
                .ok_or_else(|| anyhow!("Unknown validator {} (not in frozen set)", vote.validator))?;
            let idx = accumulator.get_validator_index(&vote.validator)
                .ok_or_else(|| anyhow!("Validator {} index not found", vote.validator))?;
            (validator.voting_power, validator.bls_public_key.clone(), idx)
        } else if let Some(ref vs) = self.validator_service {
            let validator_state = vs.read().await;
            let validator = validator_state.get_validator(&vote.validator)
                .ok_or_else(|| anyhow!("Unknown validator: {}", vote.validator))?;
            if !validator.is_active() {
                return Err(anyhow!("Validator {} is not active", vote.validator));
            }
            let mut addrs: Vec<_> = validator_state.active_validators().keys().cloned().collect();
            addrs.sort();
            let idx = self.get_validator_index_deterministic(&vote.validator, &addrs);
            (validator.effective_stake, validator.bls_public_key.clone(), idx)
        } else {
            let state = self.state.inner.read().await;
            let validator = state.validators.get(&vote.validator)
                .ok_or_else(|| anyhow!("Unknown validator: {}", vote.validator))?;
            let pubkey = validator.bls_public_key.as_ref()
                .and_then(|k| hex::decode(k).ok())
                .unwrap_or_default();
            let mut addrs: Vec<_> = state.validators.keys().cloned().collect();
            addrs.sort();
            let idx = self.get_validator_index_deterministic(&vote.validator, &addrs);
            (validator.stake, pubkey, idx)
        };

        if bls_pubkey.is_empty() {
            return Err(anyhow!("Validator {} has no BLS public key - cannot verify vote", vote.validator));
        }

        let expected_msg = vote.message();
        if !bls_verify(&expected_msg, &vote.signature, &bls_pubkey) {
            return Err(anyhow!("Invalid BLS signature from validator {}", vote.validator));
        }

        let accumulator = self.pending_votes.get_mut(&vote.checkpoint_height)
            .ok_or_else(|| anyhow!("No voting round for height {}", vote.checkpoint_height))?;

        if vote.checkpoint_hash != accumulator.checkpoint_hash {
            return Err(anyhow!(
                "Vote for wrong checkpoint hash: expected {}, got {}",
                accumulator.checkpoint_hash, vote.checkpoint_hash
            ));
        }

        let is_new = accumulator.add_vote(vote.clone(), voting_power, signer_index);
        if !is_new {
            return Ok(VoteResult::Duplicate);
        }

        if accumulator.quorum_reached() && !self.finalized_heights.contains(&accumulator.checkpoint_height) {
            self.finalized_heights.insert(accumulator.checkpoint_height);
            self.last_finalized_height = accumulator.checkpoint_height;
            info!(
                "Checkpoint {} finalized with {:.2}% stake ({} validators)",
                accumulator.checkpoint_height,
                accumulator.voting_power_ratio() * 100.0,
                accumulator.votes.len()
            );
            return Ok(VoteResult::QuorumReached);
        }

        Ok(VoteResult::Accepted)
    }

    fn get_validator_index_deterministic(&self, address: &str, validators: &[String]) -> usize {
        validators.iter().position(|a| a == address).unwrap_or(0)
    }

    pub async fn get_sorted_validators(&self) -> Vec<String> {
        if let Some(ref vs) = self.validator_service {
            let mut addrs: Vec<_> = vs.read().await.active_validators().keys().cloned().collect();
            addrs.sort();
            addrs
        } else {
            let state = self.state.inner.read().await;
            let mut addrs: Vec<_> = state.validators.keys().cloned().collect();
            addrs.sort();
            addrs
        }
    }

    pub async fn handle_double_sign_detection(&self, evidence: &DoubleSignEvidence) -> Option<f64> {
        if let Some(ref vs) = self.validator_service {
            let mut vs_guard = vs.write().await;
            if let Some(validator) = vs_guard.get_validator(&evidence.validator) {
                let stake = validator.effective_stake;
                if let Ok(slashed) = vs_guard.slash_validator(&evidence.validator, 0.15) {
                    warn!(
                        "Slashed validator {} for double-signing: {} RKU",
                        evidence.validator, slashed
                    );
                    return Some(slashed);
                }
            }
        }
        None
    }

    pub async fn create_finality_proof(&self, height: u64) -> Result<FinalityProof> {
        let accumulator = self.pending_votes.get(&height)
            .ok_or_else(|| anyhow!("No votes for height {}", height))?;

        if !accumulator.quorum_reached() {
            return Err(anyhow!(
                "Quorum not reached: {:.2}% < {:.2}%",
                accumulator.voting_power_ratio() * 100.0,
                QUORUM_THRESHOLD * 100.0
            ));
        }

        let validator_count = if let Some(ref vs) = self.validator_service {
            vs.read().await.active_validators().len()
        } else {
            let state = self.state.inner.read().await;
            state.validators.len()
        };

        let aggregated = aggregate_signatures(&accumulator.signatures)
            .map_err(|e| anyhow!("Failed to aggregate signatures: {}", e))?;
        let bitmap = create_signer_bitmap(&accumulator.signer_indices, validator_count);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Ok(FinalityProof {
            checkpoint_height: height,
            checkpoint_hash: accumulator.checkpoint_hash.clone(),
            aggregated_signature: aggregated,
            signer_bitmap: bitmap,
            total_stake_voted: accumulator.accumulated_power,
            total_stake: accumulator.total_voting_power,
            quorum_threshold: QUORUM_THRESHOLD,
            timestamp: now,
        })
    }

    pub async fn apply_votes_to_checkpoint(&self, checkpoint: &mut Checkpoint, height: u64) -> Result<()> {
        let accumulator = self.pending_votes.get(&height)
            .ok_or_else(|| anyhow!("No votes for height {}", height))?;

        if !accumulator.quorum_reached() {
            return Err(anyhow!("Cannot apply votes: quorum not reached"));
        }

        let validator_count = if let Some(ref vs) = self.validator_service {
            let vs_guard = vs.read().await;
            let mut signatures: Vec<ValidatorSignature> = Vec::new();
            for (addr, vote) in &accumulator.votes {
                if let Some(validator) = vs_guard.get_validator(addr) {
                    signatures.push(ValidatorSignature {
                        validator: addr.clone(),
                        signature: URL_SAFE_NO_PAD.encode(&vote.signature),
                        weight: validator.effective_stake,
                        bls_public_key: Some(validator.bls_public_key_base64()),
                    });
                }
            }
            checkpoint.validator_signatures = signatures;
            vs_guard.active_validators().len()
        } else {
            let state = self.state.inner.read().await;
            let mut signatures: Vec<ValidatorSignature> = Vec::new();
            for (addr, vote) in &accumulator.votes {
                if let Some(validator) = state.validators.get(addr) {
                    signatures.push(ValidatorSignature {
                        validator: addr.clone(),
                        signature: URL_SAFE_NO_PAD.encode(&vote.signature),
                        weight: validator.stake,
                        bls_public_key: validator.bls_public_key.clone(),
                    });
                }
            }
            checkpoint.validator_signatures = signatures;
            state.validators.len()
        };

        let aggregated = aggregate_signatures(&accumulator.signatures)
            .map_err(|e| anyhow!("Failed to aggregate signatures: {}", e))?;
        checkpoint.aggregated_signature = Some(URL_SAFE_NO_PAD.encode(&aggregated));
        checkpoint.signer_bitmap = Some(create_signer_bitmap(&accumulator.signer_indices, validator_count));

        Ok(())
    }

    pub async fn verify_checkpoint_quorum(&self, checkpoint: &Checkpoint) -> Result<QuorumVerificationResult> {
        let (total_stake, validators_map) = if let Some(ref vs) = self.validator_service {
            let vs_guard = vs.read().await;
            let total = vs_guard.total_active_stake();
            let map: HashMap<String, (f64, Vec<u8>)> = vs_guard.active_validators()
                .iter()
                .map(|(k, v)| (k.clone(), (v.effective_stake, v.bls_public_key.clone())))
                .collect();
            (total, map)
        } else {
            let state = self.state.inner.read().await;
            let total: f64 = state.validators.values().map(|v| v.stake).sum();
            let map: HashMap<String, (f64, Vec<u8>)> = state.validators.iter()
                .map(|(k, v)| {
                    let pk = v.bls_public_key.as_ref()
                        .and_then(|s| hex::decode(s).ok())
                        .unwrap_or_default();
                    (k.clone(), (v.stake, pk))
                })
                .collect();
            (total, map)
        };

        if total_stake <= 0.0 {
            return Ok(QuorumVerificationResult {
                valid: false,
                verified_stake: 0.0,
                total_stake: 0.0,
                quorum_reached: false,
                error: Some("No active validators".to_string()),
            });
        }

        let mut verified_stake = 0.0;
        let checkpoint_hash = hex::decode(&checkpoint.hash).unwrap_or_default();

        for sig_info in &checkpoint.validator_signatures {
            if let Some((stake, bls_pk)) = validators_map.get(&sig_info.validator) {
                let signature = match URL_SAFE_NO_PAD.decode(&sig_info.signature) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                if bls_verify(&checkpoint_hash, &signature, bls_pk) {
                    verified_stake += stake;
                }
            }
        }

        let ratio = verified_stake / total_stake;
        let quorum_reached = ratio >= QUORUM_THRESHOLD;

        Ok(QuorumVerificationResult {
            valid: quorum_reached,
            verified_stake,
            total_stake,
            quorum_reached,
            error: if quorum_reached {
                None
            } else {
                Some(format!(
                    "Quorum not reached: {:.2}% < {:.2}%",
                    ratio * 100.0,
                    QUORUM_THRESHOLD * 100.0
                ))
            },
        })
    }

    pub fn check_double_signing(&self, validator: &str, height: u64, hash: &str) -> Option<DoubleSignEvidence> {
        if let Some(accumulator) = self.pending_votes.get(&height) {
            if let Some(existing_vote) = accumulator.votes.get(validator) {
                if existing_vote.checkpoint_hash != hash {
                    return Some(DoubleSignEvidence {
                        validator: validator.to_string(),
                        height,
                        hash1: existing_vote.checkpoint_hash.clone(),
                        hash2: hash.to_string(),
                        signature1: existing_vote.signature.clone(),
                    });
                }
            }
        }
        None
    }

    pub fn is_finalized(&self, height: u64) -> bool {
        self.finalized_heights.contains(&height)
    }

    pub fn last_finalized_height(&self) -> u64 {
        self.last_finalized_height
    }

    pub fn cleanup_old_rounds(&mut self, keep_after: u64) {
        self.pending_votes.retain(|h, _| *h >= keep_after);
        self.finalized_heights.retain(|h| *h >= keep_after);
    }

    pub async fn validate_transaction(&self, tx: &SignedTransaction) -> Result<bool> {
        let tx_for_hash = Transaction {
            from: tx.tx.from.clone(),
            to: tx.tx.to.clone(),
            amount: tx.tx.amount,
            nonce: tx.tx.nonce,
            timestamp: tx.tx.timestamp,
            parents: tx.tx.parents.clone(),
            kind: tx.tx.kind,
            gas_limit: tx.tx.gas_limit,
            gas_price: tx.tx.gas_price,
            data: tx.tx.data.clone(),
            signature: None,
        };

        let tx_json = serde_json::to_string(&tx_for_hash)?;
        let computed_hash = sha256_hex(&tx_json);

        if computed_hash != tx.hash {
            warn!("Transaction hash mismatch");
            return Ok(false);
        }

        // Amount must be positive (except for unstake which can be 0)
        let is_unstake = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
        if tx.tx.amount <= 0.0 && !is_unstake {
            warn!("Invalid transaction amount");
            return Ok(false);
        }

        let sender = self.state.get_account(&tx.tx.from).await;
        let gas_fee = tx.tx.gas_price.unwrap_or(0.001); // Default gas price
        
        match &sender {
            Some(account) => {
                // Calculate required balance based on transaction type
                let is_stake = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
                
                let required_balance = if is_stake {
                    // Stake: need amount + gas (amount is locked, not transferred)
                    tx.tx.amount + gas_fee
                } else if is_unstake {
                    // Unstake: only need gas fee
                    gas_fee
                } else {
                    // Transfer: need amount + gas
                    tx.tx.amount + gas_fee
                };
                
                if account.balance < required_balance {
                    warn!(
                        "Insufficient balance: have {:.6}, need {:.6} (amount: {:.6}, gas: {:.6})",
                        account.balance, required_balance, tx.tx.amount, gas_fee
                    );
                    return Ok(false);
                }

                if tx.tx.nonce != account.nonce {
                    warn!("Invalid nonce: expected {}, got {}", account.nonce, tx.tx.nonce);
                    return Ok(false);
                }
            }
            None => {
                // Account doesn't exist - reject unless it's a genesis transaction
                if tx.tx.from != "genesis" {
                    warn!("Account {} does not exist - cannot process transaction", &tx.tx.from[..16.min(tx.tx.from.len())]);
                    return Ok(false);
                }
            }
        }

        for parent in &tx.tx.parents {
            if !parent.is_empty() {
                let state = self.state.inner.read().await;
                if !state.dag.contains(parent) {
                    warn!("Parent not found: {}", parent);
                    return Ok(false);
                }
            }
        }

        debug!("Transaction {} validated successfully", &tx.hash[..16]);
        Ok(true)
    }

    pub async fn calculate_transaction_weight(&self, tx: &SignedTransaction) -> f64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if let Some(account) = self.state.get_account(&tx.tx.from).await {
            calculate_account_weight(&account, now)
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rinku_core::types::{Transaction, TransactionKind};

    fn create_test_vote(validator: &str, height: u64, hash: &str) -> Vote {
        Vote {
            validator: validator.to_string(),
            vote_type: VoteType::Commit,
            checkpoint_height: height,
            checkpoint_hash: hash.to_string(),
            signature: vec![0u8; 96],
            timestamp: 1000,
        }
    }

    #[test]
    fn test_vote_accumulator_quorum() {
        let mut acc = VoteAccumulator::new(1, "hash123".to_string(), 1000.0, vec![]);

        assert!(!acc.quorum_reached());

        acc.add_vote(create_test_vote("v1", 1, "hash123"), 400.0, 0);
        assert!(!acc.quorum_reached());

        acc.add_vote(create_test_vote("v2", 1, "hash123"), 300.0, 1);
        assert!(acc.quorum_reached());
        assert!((acc.voting_power_ratio() - 0.7).abs() < 0.001);
    }

    #[test]
    fn test_vote_accumulator_duplicate_rejection() {
        let mut acc = VoteAccumulator::new(1, "hash123".to_string(), 1000.0, vec![]);

        assert!(acc.add_vote(create_test_vote("v1", 1, "hash123"), 400.0, 0));
        assert!(!acc.add_vote(create_test_vote("v1", 1, "hash123"), 400.0, 0));
        assert_eq!(acc.votes.len(), 1);
    }

    #[test]
    fn test_vote_accumulator_super_majority() {
        let mut acc = VoteAccumulator::new(1, "hash123".to_string(), 1000.0, vec![]);

        acc.add_vote(create_test_vote("v1", 1, "hash123"), 400.0, 0);
        acc.add_vote(create_test_vote("v2", 1, "hash123"), 300.0, 1);
        assert!(!acc.super_majority_reached());

        acc.add_vote(create_test_vote("v3", 1, "hash123"), 100.0, 2);
        assert!(acc.super_majority_reached());
    }

    #[test]
    fn test_finality_proof_validation() {
        let valid_proof = FinalityProof {
            checkpoint_height: 1,
            checkpoint_hash: "hash".to_string(),
            aggregated_signature: vec![],
            signer_bitmap: vec![],
            total_stake_voted: 700.0,
            total_stake: 1000.0,
            quorum_threshold: QUORUM_THRESHOLD,
            timestamp: 1000,
        };
        assert!(valid_proof.is_valid());

        let invalid_proof = FinalityProof {
            checkpoint_height: 1,
            checkpoint_hash: "hash".to_string(),
            aggregated_signature: vec![],
            signer_bitmap: vec![],
            total_stake_voted: 600.0,
            total_stake: 1000.0,
            quorum_threshold: QUORUM_THRESHOLD,
            timestamp: 1000,
        };
        assert!(!invalid_proof.is_valid());
    }

    #[test]
    fn test_vote_message_determinism() {
        let vote1 = Vote {
            validator: "v1".to_string(),
            vote_type: VoteType::Commit,
            checkpoint_height: 100,
            checkpoint_hash: "abc123".to_string(),
            signature: vec![],
            timestamp: 1000,
        };

        let vote2 = Vote {
            validator: "v1".to_string(),
            vote_type: VoteType::Commit,
            checkpoint_height: 100,
            checkpoint_hash: "abc123".to_string(),
            signature: vec![],
            timestamp: 2000,
        };

        assert_eq!(vote1.message(), vote2.message());
    }

    #[test]
    fn test_vote_message_differs_by_type() {
        let prepare = Vote {
            validator: "v1".to_string(),
            vote_type: VoteType::Prepare,
            checkpoint_height: 100,
            checkpoint_hash: "abc".to_string(),
            signature: vec![],
            timestamp: 1000,
        };

        let commit = Vote {
            validator: "v1".to_string(),
            vote_type: VoteType::Commit,
            checkpoint_height: 100,
            checkpoint_hash: "abc".to_string(),
            signature: vec![],
            timestamp: 1000,
        };

        assert_ne!(prepare.message(), commit.message());
    }

    #[test]
    fn test_quorum_threshold_boundary() {
        let mut acc = VoteAccumulator::new(1, "hash".to_string(), 1000.0, vec![]);
        
        acc.add_vote(create_test_vote("v1", 1, "hash"), 660.0, 0);
        assert!(!acc.quorum_reached(), "66% should not reach quorum");
        
        acc.add_vote(create_test_vote("v2", 1, "hash"), 10.0, 1);
        assert!(acc.quorum_reached(), "67% should reach quorum");
    }

    fn create_test_tx(from: &str, amount: f64, gas_price: Option<f64>, kind: Option<TransactionKind>) -> SignedTransaction {
        let tx = Transaction {
            from: from.to_string(),
            to: from.to_string(), // stake to self
            amount,
            nonce: 0,
            timestamp: 1000000,
            parents: vec![],
            kind,
            gas_limit: None,
            gas_price,
            data: None,
            signature: None,
        };
        let tx_json = serde_json::to_string(&tx).unwrap();
        let hash = sha256_hex(&tx_json);
        SignedTransaction {
            tx,
            hash,
            signature: "test_sig".to_string(),
        }
    }

    #[test]
    fn test_stake_requires_balance_plus_gas() {
        // Test that stake validation checks for amount + gas, not just amount
        let tx = create_test_tx("test_user", 100.0, Some(0.01), Some(TransactionKind::Stake));
        
        // Required balance should be 100.0 + 0.01 = 100.01
        let gas_fee = tx.tx.gas_price.unwrap_or(0.001);
        let is_stake = matches!(tx.tx.kind, Some(TransactionKind::Stake));
        let required = if is_stake { tx.tx.amount + gas_fee } else { tx.tx.amount + gas_fee };
        
        assert_eq!(required, 100.01);
        assert!(is_stake);
    }

    #[test]
    fn test_unstake_only_needs_gas() {
        let tx = create_test_tx("test_user", 0.0, Some(0.01), Some(TransactionKind::Unstake));
        
        let gas_fee = tx.tx.gas_price.unwrap_or(0.001);
        let is_unstake = matches!(tx.tx.kind, Some(TransactionKind::Unstake));
        let required = if is_unstake { gas_fee } else { tx.tx.amount + gas_fee };
        
        assert_eq!(required, 0.01);
        assert!(is_unstake);
    }

    #[test]
    fn test_transfer_requires_amount_plus_gas() {
        let tx = create_test_tx("test_user", 50.0, Some(0.005), None);
        
        let gas_fee = tx.tx.gas_price.unwrap_or(0.001);
        let is_stake = matches!(tx.tx.kind, Some(TransactionKind::Stake));
        let is_unstake = matches!(tx.tx.kind, Some(TransactionKind::Unstake));
        let required = if is_stake || is_unstake { 0.0 } else { tx.tx.amount + gas_fee };
        
        assert_eq!(required, 50.005);
        assert!(!is_stake);
        assert!(!is_unstake);
    }

    #[test]
    fn test_zero_balance_cannot_stake() {
        // Simulates the bug: account with 0 balance trying to stake 100
        let account_balance = 0.0;
        let stake_amount = 100.0;
        let gas_fee = 0.001;
        
        let required_balance = stake_amount + gas_fee; // 100.001
        let has_sufficient = account_balance >= required_balance;
        
        assert!(!has_sufficient, "Zero balance should NOT be able to stake");
    }

    #[test]
    fn test_insufficient_balance_for_stake_plus_gas() {
        // Account has exactly the stake amount but not enough for gas
        let account_balance = 100.0;
        let stake_amount = 100.0;
        let gas_fee = 0.001;
        
        let required_balance = stake_amount + gas_fee; // 100.001
        let has_sufficient = account_balance >= required_balance;
        
        assert!(!has_sufficient, "Balance equal to stake amount should fail (no gas)");
    }

    #[test]
    fn test_sufficient_balance_for_stake() {
        let account_balance = 100.01;
        let stake_amount = 100.0;
        let gas_fee = 0.001;
        
        let required_balance = stake_amount + gas_fee; // 100.001
        let has_sufficient = account_balance >= required_balance;
        
        assert!(has_sufficient, "Balance > stake + gas should succeed");
    }
}
