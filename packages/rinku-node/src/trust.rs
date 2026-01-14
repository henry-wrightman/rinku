use crate::bls;
use crate::config::TrustConfig;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rinku_core::types::{Checkpoint, Validator};
use std::collections::HashMap;
use tracing::{debug, info, warn};

#[derive(Debug, Clone)]
pub struct CheckpointVerificationResult {
    pub valid: bool,
    pub verified_stake: f64,
    pub total_stake: f64,
    pub quorum_reached: bool,
    pub error: Option<String>,
}

pub struct TrustVerifier {
    config: TrustConfig,
}

impl TrustVerifier {
    pub fn new(config: TrustConfig) -> Self {
        Self { config }
    }

    pub fn get_validator_public_key(
        &self,
        address: &str,
        validators: &HashMap<String, Validator>,
    ) -> Option<Vec<u8>> {
        for genesis in &self.config.genesis_validators {
            if genesis.address == address {
                return Some(genesis.bls_public_key.clone());
            }
        }

        if let Some(validator) = validators.get(address) {
            if let Some(ref bls_key) = validator.bls_public_key {
                if let Ok(bytes) = hex::decode(bls_key) {
                    return Some(bytes);
                }
                if let Ok(bytes) = URL_SAFE_NO_PAD.decode(bls_key) {
                    return Some(bytes);
                }
            }
        }

        None
    }

    pub fn verify_checkpoint(
        &self,
        checkpoint: &Checkpoint,
        validators: &HashMap<String, Validator>,
    ) -> CheckpointVerificationResult {
        let mut verified_stake = 0.0;
        let mut total_stake: f64 = validators.values().map(|v| v.stake).sum();

        for genesis in &self.config.genesis_validators {
            if !validators.contains_key(&genesis.address) {
                total_stake += 1000.0;
            }
        }

        if total_stake == 0.0 {
            total_stake = 1.0;
        }

        if checkpoint.validator_signatures.is_empty() {
            if checkpoint.height == 0 || checkpoint.height == 1 {
                return CheckpointVerificationResult {
                    valid: true,
                    verified_stake: total_stake,
                    total_stake,
                    quorum_reached: true,
                    error: None,
                };
            }

            return CheckpointVerificationResult {
                valid: false,
                verified_stake: 0.0,
                total_stake,
                quorum_reached: false,
                error: Some("No signatures on checkpoint".to_string()),
            };
        }

        let checkpoint_hash = hex::decode(&checkpoint.hash).unwrap_or_default();

        let mut missing_keys = 0;
        let mut invalid_sigs = 0;

        for sig_info in &checkpoint.validator_signatures {
            let validator_addr = &sig_info.validator;

            let pubkey = match self.get_validator_public_key(validator_addr, validators) {
                Some(pk) => pk,
                None => {
                    debug!(
                        "No public key found for validator {} - skipping",
                        validator_addr
                    );
                    missing_keys += 1;
                    continue;
                }
            };

            let signature = match URL_SAFE_NO_PAD.decode(&sig_info.signature) {
                Ok(sig) => sig,
                Err(_) => {
                    debug!("Invalid base64 signature from validator {}", validator_addr);
                    continue;
                }
            };

            if bls::bls_verify(&checkpoint_hash, &signature, &pubkey) {
                let stake = validators
                    .get(validator_addr)
                    .map(|v| v.stake)
                    .unwrap_or_else(|| {
                        if self
                            .config
                            .genesis_validators
                            .iter()
                            .any(|g| g.address == *validator_addr)
                        {
                            1000.0
                        } else {
                            0.0
                        }
                    });

                verified_stake += stake;
                debug!(
                    "Verified signature from {} (stake: {})",
                    validator_addr, stake
                );
            } else {
                warn!(
                    "Invalid BLS signature from validator {} for checkpoint {}",
                    validator_addr, checkpoint.height
                );
                invalid_sigs += 1;
            }
        }

        if missing_keys > 0 || invalid_sigs > 0 {
            debug!(
                "Checkpoint {} verification: {} missing keys, {} invalid sigs, {:.2}% stake verified",
                checkpoint.height, missing_keys, invalid_sigs, (verified_stake / total_stake) * 100.0
            );
        }

        let quorum_reached = (verified_stake / total_stake) >= self.config.checkpoint_quorum_threshold;

        CheckpointVerificationResult {
            valid: quorum_reached,
            verified_stake,
            total_stake,
            quorum_reached,
            error: if quorum_reached {
                None
            } else {
                Some(format!(
                    "Quorum not reached: {:.2}% < {:.2}%",
                    (verified_stake / total_stake) * 100.0,
                    self.config.checkpoint_quorum_threshold * 100.0
                ))
            },
        }
    }

    pub fn verify_checkpoint_chain(
        &self,
        checkpoints: &[Checkpoint],
        validators: &HashMap<String, Validator>,
    ) -> Result<(), String> {
        if checkpoints.is_empty() {
            return Ok(());
        }

        for i in 1..checkpoints.len() {
            let expected_prev = &checkpoints[i - 1].hash;
            if checkpoints[i].previous_hash.as_deref() != Some(expected_prev) {
                return Err(format!(
                    "Broken chain at height {}: expected previous_hash {}, got {:?}",
                    checkpoints[i].height,
                    expected_prev,
                    checkpoints[i].previous_hash
                ));
            }
        }

        let mut verified_count = 0;
        for checkpoint in checkpoints {
            if checkpoint.height <= 1 && checkpoint.validator_signatures.is_empty() {
                verified_count += 1;
                continue;
            }

            let result = self.verify_checkpoint(checkpoint, validators);
            if !result.valid {
                return Err(format!(
                    "Checkpoint {} verification failed: {}",
                    checkpoint.height,
                    result.error.unwrap_or_else(|| "unknown error".to_string())
                ));
            }
            verified_count += 1;
        }

        info!(
            "Verified {} checkpoints with stake-weighted BLS signatures",
            verified_count
        );
        Ok(())
    }

    pub fn is_trusted_checkpoint(&self, checkpoint_hash: &str) -> bool {
        if let Some(ref trusted) = self.config.trust_checkpoint_hash {
            return checkpoint_hash == trusted;
        }
        false
    }

    pub fn has_genesis_validators(&self) -> bool {
        !self.config.genesis_validators.is_empty()
    }
}
