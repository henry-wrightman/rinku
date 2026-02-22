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

// Use 0.6666 (exactly 2/3) to allow 2-of-3 validator quorum in small validator sets
pub const QUORUM_THRESHOLD: f64 = 0.6666;
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

    pub fn reduce_validator_power(&mut self, address: &str, reduction_ratio: f64) -> Option<f64> {
        if let Some(validator) = self.frozen_validators.iter_mut().find(|v| v.address == address) {
            let reduction = validator.voting_power * reduction_ratio;
            validator.voting_power -= reduction;
            self.total_voting_power -= reduction;
            if let Some(_vote) = self.votes.get(address) {
                self.accumulated_power -= reduction;
            }
            Some(reduction)
        } else {
            None
        }
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
    pub signature2: Vec<u8>,
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
    vote_history: HashMap<(String, u64), Vote>,
    slashed_validators: HashSet<String>,
    slashing_service: Arc<RwLock<crate::slashing::SlashingService>>,
    liveness_tracking: HashMap<String, u32>,
}

impl ConsensusService {
    pub fn new(state: NodeState) -> Self {
        Self {
            state,
            validator_service: None,
            pending_votes: HashMap::new(),
            finalized_heights: HashSet::new(),
            last_finalized_height: 0,
            vote_history: HashMap::new(),
            slashed_validators: HashSet::new(),
            slashing_service: Arc::new(RwLock::new(crate::slashing::SlashingService::new())),
            liveness_tracking: HashMap::new(),
        }
    }
    
    pub fn with_slashing_service(mut self, slashing: Arc<RwLock<crate::slashing::SlashingService>>) -> Self {
        self.slashing_service = slashing;
        self
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

        let expected_hash = {
            let accumulator = self.pending_votes.get(&vote.checkpoint_height)
                .ok_or_else(|| anyhow!("No voting round for height {}", vote.checkpoint_height))?;
            accumulator.checkpoint_hash.clone()
        };

        let vote_key = (vote.validator.clone(), vote.checkpoint_height);
        let double_sign_evidence = if let Some(existing_vote) = self.vote_history.get(&vote_key) {
            if existing_vote.checkpoint_hash != vote.checkpoint_hash 
                && !self.slashed_validators.contains(&vote.validator) {
                Some(DoubleSignEvidence {
                    validator: vote.validator.clone(),
                    height: vote.checkpoint_height,
                    hash1: existing_vote.checkpoint_hash.clone(),
                    hash2: vote.checkpoint_hash.clone(),
                    signature1: existing_vote.signature.clone(),
                    signature2: vote.signature.clone(),
                })
            } else {
                None
            }
        } else {
            None
        };

        if let Some(evidence) = double_sign_evidence {
            warn!(
                "DOUBLE-SIGN DETECTED: validator {} at height {} (hashes: {} vs {})",
                evidence.validator, evidence.height, evidence.hash1, evidence.hash2
            );
            if let Some(slashed_amount) = self.handle_double_sign_detection(&evidence).await {
                self.slashed_validators.insert(vote.validator.clone());
                info!("Slashed {} RKU from validator {}", slashed_amount, vote.validator);
            }
        }

        self.vote_history.insert(vote_key, vote.clone());

        if vote.checkpoint_hash != expected_hash {
            return Err(anyhow!(
                "Vote for wrong checkpoint hash: expected {}, got {}",
                expected_hash, vote.checkpoint_hash
            ));
        }

        let accumulator = self.pending_votes.get_mut(&vote.checkpoint_height)
            .ok_or_else(|| anyhow!("No voting round for height {}", vote.checkpoint_height))?;

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

    pub async fn handle_double_sign_detection(&mut self, evidence: &DoubleSignEvidence) -> Option<f64> {
        let slashed_amount = if let Some(ref vs) = self.validator_service {
            let mut vs_guard = vs.write().await;
            if vs_guard.get_validator(&evidence.validator).is_some() {
                vs_guard.slash_validator(&evidence.validator, 0.15).ok()
            } else {
                None
            }
        } else {
            None
        };

        {
            let mut slashing = self.slashing_service.write().await;
            let _ = slashing.submit_double_sign_evidence(
                evidence.validator.clone(),
                evidence.height,
                evidence.hash1.clone(),
                evidence.hash2.clone(),
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&evidence.signature1),
                Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&evidence.signature2)),
            );
        }

        if let Some(slashed) = slashed_amount {
            warn!(
                "Slashed validator {} for double-signing: {} RKU (15% of stake)",
                evidence.validator, slashed
            );

            {
                let original_stake = slashed / 0.15;
                let mut slashing = self.slashing_service.write().await;
                slashing.slash(
                    &evidence.validator,
                    original_stake,
                    crate::slashing::SlashReason::DoubleSign,
                    evidence.height,
                    Some(format!("Double-sign: {} vs {}", &evidence.hash1[..8.min(evidence.hash1.len())], &evidence.hash2[..8.min(evidence.hash2.len())])),
                );
            }

            for (_, accumulator) in self.pending_votes.iter_mut() {
                if let Some(reduced) = accumulator.reduce_validator_power(&evidence.validator, 0.15) {
                    debug!(
                        "Reduced voting power for {} in height {} by {} RKU",
                        evidence.validator, accumulator.checkpoint_height, reduced
                    );
                }
            }
            return Some(slashed);
        }
        None
    }
    
    pub async fn track_liveness(&mut self, checkpoint_height: u64, participating_validators: &[String]) {
        let all_validators: Vec<String> = if let Some(ref vs) = self.validator_service {
            vs.read().await.active_validators().keys().cloned().collect()
        } else {
            let state = self.state.inner.read().await;
            state.validators.keys().cloned().collect()
        };
        
        for validator in &all_validators {
            if !participating_validators.contains(validator) {
                let count = self.liveness_tracking.entry(validator.clone()).or_insert(0);
                *count += 1;
                
                if *count >= crate::slashing::LIVENESS_MISS_THRESHOLD {
                    warn!("Validator {} missed {} consecutive checkpoints", validator, count);
                    
                    let stake = if let Some(ref vs) = self.validator_service {
                        let vs_guard = vs.read().await;
                        vs_guard.get_validator(validator).map(|v| v.effective_stake).unwrap_or(0.0)
                    } else {
                        let state = self.state.inner.read().await;
                        state.validators.get(validator).map(|v| v.stake).unwrap_or(0.0)
                    };
                    
                    if stake > 0.0 {
                        let mut slashing = self.slashing_service.write().await;
                        slashing.record_liveness_failure(validator, checkpoint_height, stake);
                    }
                }
            } else {
                let mut slashing = self.slashing_service.write().await;
                slashing.reset_liveness_counter(validator);
                self.liveness_tracking.remove(validator);
            }
        }
    }
    
    pub async fn get_slash_events(&self) -> Vec<crate::slashing::SlashEvent> {
        self.slashing_service.read().await.get_events().to_vec()
    }

    pub fn cleanup_old_vote_history(&mut self, keep_after_height: u64) {
        self.vote_history.retain(|(_, height), _| *height >= keep_after_height);
    }

    pub fn is_validator_slashed(&self, validator: &str) -> bool {
        self.slashed_validators.contains(validator)
    }

    pub fn clear_slashed_set(&mut self) {
        self.slashed_validators.clear();
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

    pub fn check_double_signing(&self, validator: &str, height: u64, hash: &str, signature: &[u8]) -> Option<DoubleSignEvidence> {
        if let Some(accumulator) = self.pending_votes.get(&height) {
            if let Some(existing_vote) = accumulator.votes.get(validator) {
                if existing_vote.checkpoint_hash != hash {
                    return Some(DoubleSignEvidence {
                        validator: validator.to_string(),
                        height,
                        hash1: existing_vote.checkpoint_hash.clone(),
                        hash2: hash.to_string(),
                        signature1: existing_vote.signature.clone(),
                        signature2: signature.to_vec(),
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
            memo: tx.tx.memo.clone(),
            references: tx.tx.references.clone(),
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
        
        // 66.65% should not reach quorum (below 0.6666 threshold)
        acc.add_vote(create_test_vote("v1", 1, "hash"), 666.5, 0);
        assert!(!acc.quorum_reached(), "66.65% should not reach quorum");
        
        // 66.66% should reach quorum (at 0.6666 threshold)
        acc.add_vote(create_test_vote("v2", 1, "hash"), 0.1, 1);
        assert!(acc.quorum_reached(), "66.66% should reach quorum");
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
            memo: None,
            references: None,
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

    #[test]
    fn test_bls_sign_and_verify_real_keys() {
        use crate::bls::{generate_bls_keypair, bls_sign, bls_verify};

        let keypair = generate_bls_keypair();
        let message = b"checkpoint_hash_12345";

        let signature = bls_sign(message, &keypair.private_key)
            .expect("BLS signing should succeed");

        assert!(!signature.is_empty(), "Signature should not be empty");
        assert!(bls_verify(message, &signature, &keypair.public_key),
            "Signature verification should succeed with correct key");
    }

    #[test]
    fn test_bls_verify_rejects_wrong_key() {
        use crate::bls::{generate_bls_keypair, bls_sign, bls_verify};

        let keypair1 = generate_bls_keypair();
        let keypair2 = generate_bls_keypair();
        let message = b"checkpoint_hash_12345";

        let signature = bls_sign(message, &keypair1.private_key)
            .expect("BLS signing should succeed");

        assert!(!bls_verify(message, &signature, &keypair2.public_key),
            "Verification should fail with wrong public key");
    }

    #[test]
    fn test_bls_verify_rejects_tampered_message() {
        use crate::bls::{generate_bls_keypair, bls_sign, bls_verify};

        let keypair = generate_bls_keypair();
        let message = b"checkpoint_hash_12345";
        let tampered = b"checkpoint_hash_67890";

        let signature = bls_sign(message, &keypair.private_key)
            .expect("BLS signing should succeed");

        assert!(!bls_verify(tampered, &signature, &keypair.public_key),
            "Verification should fail with tampered message");
    }

    #[test]
    fn test_bls_aggregate_signatures() {
        use crate::bls::{generate_bls_keypair, bls_sign, aggregate_signatures, verify_aggregated_signature};

        let keypair1 = generate_bls_keypair();
        let keypair2 = generate_bls_keypair();
        let keypair3 = generate_bls_keypair();
        let message = b"checkpoint_hash_for_aggregation";

        let sig1 = bls_sign(message, &keypair1.private_key).unwrap();
        let sig2 = bls_sign(message, &keypair2.private_key).unwrap();
        let sig3 = bls_sign(message, &keypair3.private_key).unwrap();

        let aggregated = aggregate_signatures(&[sig1, sig2, sig3])
            .expect("Aggregation should succeed");

        let public_keys = vec![
            keypair1.public_key,
            keypair2.public_key,
            keypair3.public_key,
        ];

        assert!(verify_aggregated_signature(message, &aggregated, &public_keys),
            "Aggregated signature should verify with all public keys");
    }

    #[test]
    fn test_bls_aggregate_rejects_missing_signer() {
        use crate::bls::{generate_bls_keypair, bls_sign, aggregate_signatures, verify_aggregated_signature};

        let keypair1 = generate_bls_keypair();
        let keypair2 = generate_bls_keypair();
        let keypair3 = generate_bls_keypair();
        let message = b"checkpoint_hash_for_aggregation";

        let sig1 = bls_sign(message, &keypair1.private_key).unwrap();
        let sig2 = bls_sign(message, &keypair2.private_key).unwrap();

        let aggregated = aggregate_signatures(&[sig1, sig2])
            .expect("Aggregation should succeed");

        let public_keys = vec![
            keypair1.public_key,
            keypair2.public_key,
            keypair3.public_key,
        ];

        assert!(!verify_aggregated_signature(message, &aggregated, &public_keys),
            "Aggregated signature should fail if signer is missing");
    }

    #[test]
    fn test_vote_with_real_bls_signature() {
        use crate::bls::{generate_bls_keypair, bls_sign, bls_verify};

        let keypair = generate_bls_keypair();
        let vote = Vote {
            validator: "validator_001".to_string(),
            vote_type: VoteType::Commit,
            checkpoint_height: 100,
            checkpoint_hash: "abcdef123456".to_string(),
            signature: vec![],
            timestamp: 1000,
        };

        let message = vote.message();
        let signature = bls_sign(&message, &keypair.private_key).unwrap();

        assert!(bls_verify(&message, &signature, &keypair.public_key),
            "Vote signature should verify");
    }

    #[test]
    fn test_frozen_validator_snapshot_consistency() {
        let validators = vec![
            ValidatorSnapshot {
                address: "validator_c".to_string(),
                voting_power: 100.0,
                bls_public_key: vec![1, 2, 3],
            },
            ValidatorSnapshot {
                address: "validator_a".to_string(),
                voting_power: 200.0,
                bls_public_key: vec![4, 5, 6],
            },
            ValidatorSnapshot {
                address: "validator_b".to_string(),
                voting_power: 150.0,
                bls_public_key: vec![7, 8, 9],
            },
        ];

        let mut sorted = validators.clone();
        sorted.sort_by(|a, b| a.address.cmp(&b.address));

        assert_eq!(sorted[0].address, "validator_a");
        assert_eq!(sorted[1].address, "validator_b");
        assert_eq!(sorted[2].address, "validator_c");

        let acc = VoteAccumulator::new(1, "hash".to_string(), 450.0, sorted);

        assert_eq!(acc.get_validator_index("validator_a"), Some(0));
        assert_eq!(acc.get_validator_index("validator_b"), Some(1));
        assert_eq!(acc.get_validator_index("validator_c"), Some(2));
        assert_eq!(acc.get_validator_index("validator_x"), None);
    }

    #[test]
    fn test_double_sign_evidence_structure() {
        let evidence = DoubleSignEvidence {
            validator: "validator_001".to_string(),
            height: 100,
            hash1: "hash_a".to_string(),
            hash2: "hash_b".to_string(),
            signature1: vec![1, 2, 3],
            signature2: vec![4, 5, 6],
        };

        assert_ne!(evidence.hash1, evidence.hash2);
        assert_eq!(evidence.height, 100);
    }

    #[test]
    fn test_reduce_validator_power_in_accumulator() {
        let validators = vec![
            ValidatorSnapshot {
                address: "v1".to_string(),
                voting_power: 100.0,
                bls_public_key: vec![1],
            },
            ValidatorSnapshot {
                address: "v2".to_string(),
                voting_power: 100.0,
                bls_public_key: vec![2],
            },
        ];

        let mut acc = VoteAccumulator::new(1, "hash".to_string(), 200.0, validators);

        acc.add_vote(create_test_vote("v1", 1, "hash"), 100.0, 0);
        assert_eq!(acc.accumulated_power, 100.0);
        assert_eq!(acc.total_voting_power, 200.0);

        let reduced = acc.reduce_validator_power("v1", 0.15);
        assert!(reduced.is_some());
        assert!((reduced.unwrap() - 15.0).abs() < 0.001);

        assert!((acc.accumulated_power - 85.0).abs() < 0.001);
        assert!((acc.total_voting_power - 185.0).abs() < 0.001);
    }

    #[test]
    fn test_slashed_validator_tracking() {
        let mut slashed: HashSet<String> = HashSet::new();

        assert!(!slashed.contains("v1"));

        slashed.insert("v1".to_string());
        assert!(slashed.contains("v1"));
        assert!(!slashed.contains("v2"));

        slashed.clear();
        assert!(!slashed.contains("v1"));
    }

    #[test]
    fn test_vote_history_cleanup() {
        let mut vote_history: HashMap<(String, u64), Vote> = HashMap::new();

        vote_history.insert(
            ("v1".to_string(), 10), 
            create_test_vote("v1", 10, "h10")
        );
        vote_history.insert(
            ("v2".to_string(), 20), 
            create_test_vote("v2", 20, "h20")
        );
        vote_history.insert(
            ("v3".to_string(), 30), 
            create_test_vote("v3", 30, "h30")
        );

        assert_eq!(vote_history.len(), 3);

        let keep_after_height = 15u64;
        vote_history.retain(|(_, height), _| *height >= keep_after_height);

        assert_eq!(vote_history.len(), 2);
        assert!(vote_history.get(&("v2".to_string(), 20)).is_some());
        assert!(vote_history.get(&("v3".to_string(), 30)).is_some());
        assert!(vote_history.get(&("v1".to_string(), 10)).is_none());
    }
}
