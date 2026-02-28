use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::{info, warn};

use crate::bls::generate_bls_keypair;

pub const MIN_VALIDATOR_STAKE: u64 = 10_000_000_000;
pub const ACTIVATION_DELAY_EPOCHS: u64 = 2;
pub const EXIT_DELAY_EPOCHS: u64 = 4;
pub const EPOCH_LENGTH_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidatorStatus {
    PendingActivation,
    Active,
    PendingExit,
    Exited,
    Slashed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorIdentity {
    pub address: String,
    pub bls_public_key: Vec<u8>,
    pub bls_public_key_hex: String,
    pub stake: u64,
    pub status: ValidatorStatus,
    pub activation_epoch: u64,
    pub exit_epoch: Option<u64>,
    pub slashed: bool,
    pub effective_stake: u64,
    pub missed_checkpoints: u32,
    pub last_checkpoint_signed: u64,
}

impl ValidatorIdentity {
    pub fn voting_power(&self, total_stake: u64) -> f64 {
        if total_stake == 0 || self.effective_stake == 0 {
            return 0.0;
        }
        self.effective_stake as f64 / total_stake as f64
    }

    pub fn is_active(&self) -> bool {
        self.status == ValidatorStatus::Active && !self.slashed
    }

    pub fn bls_public_key_base64(&self) -> String {
        URL_SAFE_NO_PAD.encode(&self.bls_public_key)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalValidatorKeys {
    pub address: String,
    pub bls_private_key: Vec<u8>,
    pub bls_public_key: Vec<u8>,
    pub created_at: u64,
}

impl LocalValidatorKeys {
    pub fn generate() -> Self {
        let keypair = generate_bls_keypair();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        Self {
            address: keypair.fingerprint,
            bls_private_key: keypair.private_key,
            bls_public_key: keypair.public_key,
            created_at: now,
        }
    }

    pub fn bls_public_key_hex(&self) -> String {
        hex::encode(&self.bls_public_key)
    }

    pub fn bls_public_key_base64(&self) -> String {
        URL_SAFE_NO_PAD.encode(&self.bls_public_key)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorSetState {
    pub current_epoch: u64,
    pub epoch_start_time: u64,
    pub active_validators: HashMap<String, ValidatorIdentity>,
    pub pending_validators: HashMap<String, ValidatorIdentity>,
    pub exiting_validators: HashMap<String, ValidatorIdentity>,
    pub exited_validators: HashMap<String, ValidatorIdentity>,
    pub total_active_stake: u64,
    pub finalized_checkpoint_height: u64,
}

impl Default for ValidatorSetState {
    fn default() -> Self {
        Self {
            current_epoch: 0,
            epoch_start_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() * 1000,
            active_validators: HashMap::new(),
            pending_validators: HashMap::new(),
            exiting_validators: HashMap::new(),
            exited_validators: HashMap::new(),
            total_active_stake: 0,
            finalized_checkpoint_height: 0,
        }
    }
}

pub struct ValidatorIdentityService {
    data_dir: String,
    local_keys: Option<LocalValidatorKeys>,
    state: ValidatorSetState,
}

impl ValidatorIdentityService {
    pub fn new(data_dir: &str) -> Result<Self> {
        let keys_path = Path::new(data_dir).join("validator_keys.json");
        let state_path = Path::new(data_dir).join("validator_set.json");

        fs::create_dir_all(data_dir)?;

        let local_keys = if keys_path.exists() {
            let data = fs::read_to_string(&keys_path)?;
            match serde_json::from_str::<LocalValidatorKeys>(&data) {
                Ok(keys) => {
                    info!("Loaded persistent validator keys: {}", keys.address);
                    Some(keys)
                }
                Err(e) => {
                    warn!("Failed to parse validator keys, generating new: {}", e);
                    let keys = LocalValidatorKeys::generate();
                    Self::save_keys_to_file(&keys_path, &keys)?;
                    info!("Generated and saved new validator keys: {}", keys.address);
                    Some(keys)
                }
            }
        } else {
            let keys = LocalValidatorKeys::generate();
            Self::save_keys_to_file(&keys_path, &keys)?;
            info!("Generated and saved new validator keys: {}", keys.address);
            Some(keys)
        };

        let state = if state_path.exists() {
            let data = fs::read_to_string(&state_path)?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            ValidatorSetState::default()
        };

        Ok(Self {
            data_dir: data_dir.to_string(),
            local_keys,
            state,
        })
    }

    fn save_keys_to_file(path: &Path, keys: &LocalValidatorKeys) -> Result<()> {
        let data = serde_json::to_string_pretty(keys)?;
        fs::write(path, data)?;
        Ok(())
    }

    pub fn save_state(&self) -> Result<()> {
        let path = Path::new(&self.data_dir).join("validator_set.json");
        let data = serde_json::to_string_pretty(&self.state)?;
        fs::write(path, data)?;
        Ok(())
    }

    pub fn local_keys(&self) -> Option<&LocalValidatorKeys> {
        self.local_keys.as_ref()
    }

    pub fn local_address(&self) -> Option<&str> {
        self.local_keys.as_ref().map(|k| k.address.as_str())
    }

    pub fn local_bls_public_key(&self) -> Option<&[u8]> {
        self.local_keys.as_ref().map(|k| k.bls_public_key.as_slice())
    }

    pub fn local_bls_private_key(&self) -> Option<&[u8]> {
        self.local_keys.as_ref().map(|k| k.bls_private_key.as_slice())
    }

    pub fn current_epoch(&self) -> u64 {
        self.state.current_epoch
    }

    pub fn total_active_stake(&self) -> u64 {
        self.state.total_active_stake
    }

    pub fn active_validators(&self) -> &HashMap<String, ValidatorIdentity> {
        &self.state.active_validators
    }

    pub fn is_active_validator(&self, address: &str) -> bool {
        self.state.active_validators.get(address)
            .map(|v| v.is_active())
            .unwrap_or(false)
    }
    
    pub fn get_validator_bls_key(&self, address: &str) -> Option<Vec<u8>> {
        self.state.active_validators.get(address)
            .map(|v| v.bls_public_key.clone())
            .or_else(|| {
                self.state.pending_validators.get(address)
                    .map(|v| v.bls_public_key.clone())
            })
    }
    
    pub fn get_validator_stake(&self, address: &str) -> Option<u64> {
        self.state.active_validators.get(address)
            .map(|v| v.effective_stake)
            .or_else(|| {
                self.state.pending_validators.get(address)
                    .map(|v| v.stake)
            })
    }

    pub fn get_validator(&self, address: &str) -> Option<&ValidatorIdentity> {
        self.state.active_validators.get(address)
            .or_else(|| self.state.pending_validators.get(address))
            .or_else(|| self.state.exiting_validators.get(address))
    }

    pub fn register_validator(
        &mut self,
        address: String,
        bls_public_key: Vec<u8>,
        stake: u64,
    ) -> Result<ValidatorIdentity> {
        if stake < MIN_VALIDATOR_STAKE {
            return Err(anyhow!(
                "Minimum stake is {} micro-units, got {}",
                MIN_VALIDATOR_STAKE,
                stake
            ));
        }

        if self.state.active_validators.contains_key(&address)
            || self.state.pending_validators.contains_key(&address)
        {
            return Err(anyhow!("Validator {} is already registered", address));
        }

        let activation_epoch = self.state.current_epoch + ACTIVATION_DELAY_EPOCHS;

        let identity = ValidatorIdentity {
            address: address.clone(),
            bls_public_key_hex: hex::encode(&bls_public_key),
            bls_public_key,
            stake,
            status: ValidatorStatus::PendingActivation,
            activation_epoch,
            exit_epoch: None,
            slashed: false,
            effective_stake: stake,
            missed_checkpoints: 0,
            last_checkpoint_signed: 0,
        };

        info!(
            "Registered validator {} with {} micro-units stake, activation at epoch {}",
            address, stake, activation_epoch
        );

        self.state.pending_validators.insert(address, identity.clone());
        self.save_state()?;

        Ok(identity)
    }

    pub fn add_stake(&mut self, address: &str, amount: u64) -> Result<u64> {
        let new_stake = if let Some(validator) = self.state.active_validators.get_mut(address) {
            validator.stake += amount;
            validator.effective_stake = validator.stake;
            Some((validator.stake, true))
        } else if let Some(validator) = self.state.pending_validators.get_mut(address) {
            validator.stake += amount;
            validator.effective_stake = validator.stake;
            Some((validator.stake, false))
        } else {
            None
        };

        match new_stake {
            Some((stake, recalc)) => {
                if recalc {
                    self.recalculate_total_stake();
                }
                self.save_state()?;
                Ok(stake)
            }
            None => Err(anyhow!("Validator {} not found", address)),
        }
    }

    pub fn initiate_exit(&mut self, address: &str) -> Result<u64> {
        let validator = self.state.active_validators.remove(address)
            .ok_or_else(|| anyhow!("Validator {} is not active", address))?;

        let exit_epoch = self.state.current_epoch + EXIT_DELAY_EPOCHS;
        let mut exiting = validator;
        exiting.status = ValidatorStatus::PendingExit;
        exiting.exit_epoch = Some(exit_epoch);

        info!(
            "Validator {} initiated exit, effective at epoch {}",
            address, exit_epoch
        );

        self.state.exiting_validators.insert(address.to_string(), exiting);
        self.recalculate_total_stake();
        self.save_state()?;

        Ok(exit_epoch)
    }

    pub fn slash_validator(&mut self, address: &str, slash_percent: f64) -> Result<u64> {
        let validator = self.state.active_validators.get_mut(address)
            .or_else(|| self.state.exiting_validators.get_mut(address))
            .ok_or_else(|| anyhow!("Validator {} not found for slashing", address))?;

        let slash_amount = ((validator.stake as f64) * slash_percent) as u64;
        validator.stake -= slash_amount;
        validator.effective_stake = validator.stake;
        validator.slashed = true;
        validator.status = ValidatorStatus::Slashed;

        warn!(
            "Slashed validator {} by {:.2}% ({} micro-units)",
            address, slash_percent * 100.0, slash_amount
        );

        self.recalculate_total_stake();
        self.save_state()?;

        Ok(slash_amount)
    }

    pub fn process_epoch_transition(&mut self) -> EpochTransitionResult {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let elapsed = now - self.state.epoch_start_time;
        if elapsed < EPOCH_LENGTH_MS {
            return EpochTransitionResult::default();
        }

        let epochs_passed = elapsed / EPOCH_LENGTH_MS;
        let new_epoch = self.state.current_epoch + epochs_passed;

        info!("Epoch transition: {} -> {}", self.state.current_epoch, new_epoch);

        let mut activated = Vec::new();
        let mut exited = Vec::new();

        let pending: Vec<_> = self.state.pending_validators.drain().collect();
        for (addr, mut validator) in pending {
            if validator.activation_epoch <= new_epoch {
                validator.status = ValidatorStatus::Active;
                activated.push(addr.clone());
                self.state.active_validators.insert(addr, validator);
            } else {
                self.state.pending_validators.insert(addr, validator);
            }
        }

        let exiting: Vec<_> = self.state.exiting_validators.drain().collect();
        for (addr, mut validator) in exiting {
            if validator.exit_epoch.map(|e| e <= new_epoch).unwrap_or(false) {
                validator.status = ValidatorStatus::Exited;
                exited.push(addr.clone());
                self.state.exited_validators.insert(addr, validator);
            } else {
                self.state.exiting_validators.insert(addr, validator);
            }
        }

        self.state.current_epoch = new_epoch;
        self.state.epoch_start_time = now - (elapsed % EPOCH_LENGTH_MS);
        self.recalculate_total_stake();

        if let Err(e) = self.save_state() {
            warn!("Failed to save validator state: {}", e);
        }

        EpochTransitionResult {
            old_epoch: new_epoch - epochs_passed,
            new_epoch,
            activated,
            exited,
        }
    }

    pub fn record_checkpoint_participation(&mut self, signers: &[String], checkpoint_height: u64) {
        for (addr, validator) in self.state.active_validators.iter_mut() {
            if signers.contains(addr) {
                validator.last_checkpoint_signed = checkpoint_height;
            } else {
                validator.missed_checkpoints += 1;
            }
        }
        let _ = self.save_state();
    }

    fn recalculate_total_stake(&mut self) {
        self.state.total_active_stake = self.state.active_validators
            .values()
            .filter(|v| v.is_active())
            .map(|v| v.effective_stake)
            .sum();
    }

    pub fn get_state_snapshot(&self) -> ValidatorSetState {
        self.state.clone()
    }

    pub fn restore_state(&mut self, state: ValidatorSetState) {
        self.state = state;
        self.recalculate_total_stake();
        let _ = self.save_state();
    }

    pub fn sync_from_legacy_validators(
        &mut self,
        validators: &HashMap<String, rinku_core::types::Validator>,
    ) {
        let old_count = self.state.active_validators.len();
        
        let mut new_validators = HashMap::new();
        
        for (addr, legacy) in validators {
            let bls_key = legacy.bls_public_key.as_ref()
                .and_then(|k| hex::decode(k).ok())
                .or_else(|| {
                    legacy.bls_public_key.as_ref()
                        .and_then(|k| URL_SAFE_NO_PAD.decode(k).ok())
                })
                .unwrap_or_default();

            let identity = ValidatorIdentity {
                address: addr.clone(),
                bls_public_key_hex: hex::encode(&bls_key),
                bls_public_key: bls_key,
                stake: legacy.stake,
                status: ValidatorStatus::Active,
                activation_epoch: 0,
                exit_epoch: None,
                slashed: false,
                effective_stake: legacy.stake,
                missed_checkpoints: legacy.missed_checkpoints,
                last_checkpoint_signed: 0,
            };

            new_validators.insert(addr.clone(), identity);
        }
        
        let new_count = new_validators.len();
        self.state.active_validators = new_validators;
        
        self.state.pending_validators.clear();
        self.state.exiting_validators.clear();
        
        info!(
            "ValidatorIdentityService sync: REPLACED validator set ({} -> {} validators)",
            old_count, new_count
        );
        
        self.recalculate_total_stake();
        
        if let Err(e) = self.save_state() {
            warn!("Failed to persist validator set after sync: {}", e);
        }
    }

    pub fn seed_genesis_validators(&mut self, validators: &[(String, Vec<u8>)]) {
        let old_count = self.state.active_validators.len();
        
        let mut new_validators = HashMap::new();
        
        for (address, bls_public_key) in validators {
            let identity = ValidatorIdentity {
                address: address.clone(),
                bls_public_key_hex: hex::encode(bls_public_key),
                bls_public_key: bls_public_key.clone(),
                stake: MIN_VALIDATOR_STAKE,
                status: ValidatorStatus::Active,
                activation_epoch: self.state.current_epoch,
                exit_epoch: None,
                slashed: false,
                effective_stake: MIN_VALIDATOR_STAKE,
                missed_checkpoints: 0,
                last_checkpoint_signed: 0,
            };

            new_validators.insert(address.clone(), identity);
        }
        
        let new_count = new_validators.len();
        self.state.active_validators = new_validators;
        
        self.state.pending_validators.clear();
        self.state.exiting_validators.clear();

        info!(
            "Genesis validators seeded: REPLACED validator set ({} -> {} validators)",
            old_count, new_count
        );
        
        self.recalculate_total_stake();
        let _ = self.save_state();
    }
}

#[derive(Debug, Clone, Default)]
pub struct EpochTransitionResult {
    pub old_epoch: u64,
    pub new_epoch: u64,
    pub activated: Vec<String>,
    pub exited: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_service() -> (ValidatorIdentityService, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let service = ValidatorIdentityService::new(temp_dir.path().to_str().unwrap()).unwrap();
        (service, temp_dir)
    }

    #[test]
    fn test_key_generation_and_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().to_str().unwrap();

        let service1 = ValidatorIdentityService::new(path).unwrap();
        let addr1 = service1.local_address().unwrap().to_string();

        let service2 = ValidatorIdentityService::new(path).unwrap();
        let addr2 = service2.local_address().unwrap().to_string();

        assert_eq!(addr1, addr2, "Keys should persist across restarts");
    }

    #[test]
    fn test_register_validator_min_stake() {
        let (mut service, _temp) = create_test_service();
        let pubkey = vec![1u8; 48];

        let result = service.register_validator(
            "validator1".to_string(),
            pubkey.clone(),
            5_000_000_000,
        );
        assert!(result.is_err(), "Should reject below min stake");

        let result = service.register_validator(
            "validator1".to_string(),
            pubkey,
            MIN_VALIDATOR_STAKE,
        );
        assert!(result.is_ok(), "Should accept min stake");
    }

    #[test]
    fn test_validator_activation_delay() {
        let (mut service, _temp) = create_test_service();
        let pubkey = vec![1u8; 48];

        let validator = service.register_validator(
            "validator1".to_string(),
            pubkey,
            15_000_000_000,
        ).unwrap();

        assert_eq!(validator.status, ValidatorStatus::PendingActivation);
        assert_eq!(
            validator.activation_epoch,
            service.current_epoch() + ACTIVATION_DELAY_EPOCHS
        );
    }

    #[test]
    fn test_add_stake() {
        let (mut service, _temp) = create_test_service();
        let pubkey = vec![1u8; 48];

        service.register_validator(
            "validator1".to_string(),
            pubkey,
            MIN_VALIDATOR_STAKE,
        ).unwrap();

        let new_stake = service.add_stake("validator1", 5_000_000_000).unwrap();
        assert_eq!(new_stake, 15_000_000_000);
    }

    #[test]
    fn test_slash_validator() {
        let (mut service, _temp) = create_test_service();
        let pubkey = vec![1u8; 48];

        let identity = ValidatorIdentity {
            address: "validator1".to_string(),
            bls_public_key: pubkey,
            bls_public_key_hex: "".to_string(),
            stake: MIN_VALIDATOR_STAKE,
            status: ValidatorStatus::Active,
            activation_epoch: 0,
            exit_epoch: None,
            slashed: false,
            effective_stake: MIN_VALIDATOR_STAKE,
            missed_checkpoints: 0,
            last_checkpoint_signed: 0,
        };
        service.state.active_validators.insert("validator1".to_string(), identity);

        let slashed = service.slash_validator("validator1", 0.15).unwrap();
        assert_eq!(slashed, 1_500_000_000);

        let validator = service.get_validator("validator1").unwrap();
        assert_eq!(validator.stake, 8_500_000_000);
        assert!(validator.slashed);
    }

    #[test]
    fn test_voting_power_calculation() {
        let validator = ValidatorIdentity {
            address: "test".to_string(),
            bls_public_key: vec![],
            bls_public_key_hex: String::new(),
            stake: MIN_VALIDATOR_STAKE,
            status: ValidatorStatus::Active,
            activation_epoch: 0,
            exit_epoch: None,
            slashed: false,
            effective_stake: MIN_VALIDATOR_STAKE,
            missed_checkpoints: 0,
            last_checkpoint_signed: 0,
        };

        assert!((validator.voting_power(MIN_VALIDATOR_STAKE * 10) - 0.1).abs() < 0.0001);
        assert!((validator.voting_power(MIN_VALIDATOR_STAKE) - 1.0).abs() < 0.0001);
        assert_eq!(validator.voting_power(0), 0.0);
    }

    #[test]
    fn test_duplicate_registration_rejected() {
        let (mut service, _temp) = create_test_service();
        let pubkey = vec![1u8; 48];

        service.register_validator(
            "validator1".to_string(),
            pubkey.clone(),
            MIN_VALIDATOR_STAKE,
        ).unwrap();

        let result = service.register_validator(
            "validator1".to_string(),
            pubkey,
            MIN_VALIDATOR_STAKE * 2,
        );
        assert!(result.is_err(), "Duplicate registration should fail");
    }
}
