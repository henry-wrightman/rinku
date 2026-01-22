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

        if checkpoint.height == 0 || checkpoint.height == 1 {
            return CheckpointVerificationResult {
                valid: true,
                verified_stake: total_stake,
                total_stake,
                quorum_reached: true,
                error: None,
            };
        }

        if checkpoint.validator_signatures.is_empty() {
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
            if checkpoint.height <= 1 {
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

    pub fn genesis_validator_addresses(&self) -> Vec<String> {
        self.config
            .genesis_validators
            .iter()
            .map(|v| v.address.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GenesisValidator;
    use rinku_core::types::ValidatorSignature;

    fn make_test_checkpoint(height: u64, hash: &str) -> Checkpoint {
        Checkpoint {
            height,
            hash: hash.to_string(),
            tx_merkle_root: "merkle_root".to_string(),
            state_root: "state_root".to_string(),
            receipt_root: "receipt_root".to_string(),
            previous_hash: None,
            tip_count: 10,
            timestamp: 1700000000,
            validator_signatures: vec![],
            aggregated_signature: None,
            signer_bitmap: None,
        }
    }

    #[test]
    fn test_trust_verifier_no_genesis_validators() {
        let config = TrustConfig::default();
        let verifier = TrustVerifier::new(config);
        
        assert!(!verifier.has_genesis_validators());
    }

    #[test]
    fn test_trust_verifier_with_genesis_validators() {
        let config = TrustConfig {
            genesis_validators: vec![GenesisValidator {
                address: "test_validator".to_string(),
                bls_public_key: vec![0u8; 48],
            }],
            ..Default::default()
        };
        let verifier = TrustVerifier::new(config);
        
        assert!(verifier.has_genesis_validators());
    }

    #[test]
    fn test_verify_checkpoint_genesis_allowed_unsigned() {
        let config = TrustConfig::default();
        let verifier = TrustVerifier::new(config);
        let validators = HashMap::new();
        
        let checkpoint = make_test_checkpoint(0, "genesis_hash");
        let result = verifier.verify_checkpoint(&checkpoint, &validators);
        
        assert!(result.valid);
        assert!(result.quorum_reached);
    }

    #[test]
    fn test_verify_checkpoint_height_1_allowed_unsigned() {
        let config = TrustConfig::default();
        let verifier = TrustVerifier::new(config);
        let validators = HashMap::new();
        
        let checkpoint = make_test_checkpoint(1, "first_hash");
        let result = verifier.verify_checkpoint(&checkpoint, &validators);
        
        assert!(result.valid);
        assert!(result.quorum_reached);
    }

    #[test]
    fn test_verify_checkpoint_requires_signatures_after_height_1() {
        let config = TrustConfig::default();
        let verifier = TrustVerifier::new(config);
        let validators = HashMap::new();
        
        let checkpoint = make_test_checkpoint(2, "second_hash");
        let result = verifier.verify_checkpoint(&checkpoint, &validators);
        
        assert!(!result.valid);
        assert!(!result.quorum_reached);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_get_validator_public_key_from_genesis() {
        let pubkey = vec![1u8, 2, 3, 4, 5];
        let config = TrustConfig {
            genesis_validators: vec![GenesisValidator {
                address: "genesis_val".to_string(),
                bls_public_key: pubkey.clone(),
            }],
            ..Default::default()
        };
        let verifier = TrustVerifier::new(config);
        let validators = HashMap::new();
        
        let result = verifier.get_validator_public_key("genesis_val", &validators);
        
        assert!(result.is_some());
        assert_eq!(result.unwrap(), pubkey);
    }

    #[test]
    fn test_get_validator_public_key_from_on_chain() {
        let config = TrustConfig::default();
        let verifier = TrustVerifier::new(config);
        
        let mut validators = HashMap::new();
        validators.insert("onchain_val".to_string(), Validator {
            address: "onchain_val".to_string(),
            stake: 1000.0,
            first_stake_time: 0,
            bls_public_key: Some("0102030405".to_string()),
            missed_checkpoints: 0,
        });
        
        let result = verifier.get_validator_public_key("onchain_val", &validators);
        
        assert!(result.is_some());
        assert_eq!(result.unwrap(), vec![1u8, 2, 3, 4, 5]);
    }

    #[test]
    fn test_get_validator_public_key_not_found() {
        let config = TrustConfig::default();
        let verifier = TrustVerifier::new(config);
        let validators = HashMap::new();
        
        let result = verifier.get_validator_public_key("unknown_val", &validators);
        
        assert!(result.is_none());
    }

    #[test]
    fn test_is_trusted_checkpoint() {
        let config = TrustConfig {
            trust_checkpoint_hash: Some("trusted_hash".to_string()),
            ..Default::default()
        };
        let verifier = TrustVerifier::new(config);
        
        assert!(verifier.is_trusted_checkpoint("trusted_hash"));
        assert!(!verifier.is_trusted_checkpoint("untrusted_hash"));
    }

    #[test]
    fn test_is_trusted_checkpoint_none_configured() {
        let config = TrustConfig::default();
        let verifier = TrustVerifier::new(config);
        
        assert!(!verifier.is_trusted_checkpoint("any_hash"));
    }

    #[test]
    fn test_verify_checkpoint_chain_empty() {
        let config = TrustConfig::default();
        let verifier = TrustVerifier::new(config);
        let validators = HashMap::new();
        
        let result = verifier.verify_checkpoint_chain(&[], &validators);
        
        assert!(result.is_ok());
    }
}
