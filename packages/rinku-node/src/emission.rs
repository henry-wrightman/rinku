use serde::{Deserialize, Serialize};

pub const MAX_SUPPLY: f64 = 30_000_000.0;
pub const GENESIS_ALLOCATION: f64 = 6_000_000.0;
pub const INITIAL_CHECKPOINT_REWARD: f64 = 3.932411;
pub const HALVING_INTERVAL: u64 = 3_150_000;
pub const MIN_CHECKPOINT_REWARD: f64 = 0.122887;
pub const HALVINGS_COUNT: u32 = 5;

pub const VALIDATOR_FEE_FLOOR_PERCENT: f64 = 0.70;
pub const BURN_CEILING_PERCENT: f64 = 0.30;
pub const SUPPLY_TARGET_FOR_FULL_BURN: f64 = 0.50;

pub const STAKE_WEIGHT_PERCENT: f64 = 0.70;
pub const AGE_WEIGHT_PERCENT: f64 = 0.30;
pub const MIN_BOND_FOR_AGE_WEIGHT: f64 = 100.0;
pub const AGE_DECAY_PER_MISS: f64 = 0.10;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmissionStats {
    pub current_reward: f64,
    pub halving_epoch: u32,
    pub next_halving_at: u64,
    pub total_emitted: f64,
    pub remaining_to_emit: f64,
    pub circulating_supply: f64,
    pub total_burned: f64,
    pub validator_fee_percent: f64,
    pub burn_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeSplit {
    pub validator_share: f64,
    pub burn_share: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmissionSnapshot {
    pub total_emitted: f64,
    pub total_burned: f64,
}

pub struct EmissionService {
    total_emitted: f64,
    total_burned: f64,
}

impl EmissionService {
    pub fn new() -> Self {
        Self {
            total_emitted: 0.0,
            total_burned: 0.0,
        }
    }

    pub fn with_initial(total_emitted: f64, total_burned: f64) -> Self {
        Self {
            total_emitted,
            total_burned,
        }
    }

    pub fn get_checkpoint_reward(&self, checkpoint_height: u64) -> f64 {
        let halvings = (checkpoint_height / HALVING_INTERVAL) as u32;
        let effective_halvings = halvings.min(HALVINGS_COUNT);
        
        let reward_micro = ((INITIAL_CHECKPOINT_REWARD * 1_000_000.0) as i64) 
            / (1i64 << effective_halvings);
        let min_reward_micro = (MIN_CHECKPOINT_REWARD * 1_000_000.0) as i64;
        
        (reward_micro.max(min_reward_micro) as f64) / 1_000_000.0
    }

    pub fn get_halving_epoch(&self, checkpoint_height: u64) -> u32 {
        (checkpoint_height / HALVING_INTERVAL) as u32
    }

    pub fn get_next_halving_height(&self, checkpoint_height: u64) -> u64 {
        let current_epoch = self.get_halving_epoch(checkpoint_height);
        (current_epoch as u64 + 1) * HALVING_INTERVAL
    }

    pub fn record_emission(&mut self, amount: f64) {
        self.total_emitted += amount;
    }

    pub fn record_burn(&mut self, amount: f64) {
        self.total_burned += amount;
    }

    pub fn get_circulating_supply(&self) -> f64 {
        GENESIS_ALLOCATION + self.total_emitted - self.total_burned
    }

    pub fn get_remaining_to_emit(&self) -> f64 {
        let max_emittable = MAX_SUPPLY - GENESIS_ALLOCATION;
        (max_emittable - self.total_emitted).max(0.0)
    }

    pub fn progressive_burn(&self) -> f64 {
        let circulating_supply = self.get_circulating_supply();
        let supply_ratio = circulating_supply / MAX_SUPPLY;
        
        if supply_ratio >= SUPPLY_TARGET_FOR_FULL_BURN {
            return BURN_CEILING_PERCENT;
        }
        
        let burn_progress = supply_ratio / SUPPLY_TARGET_FOR_FULL_BURN;
        burn_progress * BURN_CEILING_PERCENT
    }

    pub fn get_adaptive_fee_split(&self) -> FeeSplit {
        let burn_percent = self.progressive_burn();
        let validator_percent = (1.0 - burn_percent).max(VALIDATOR_FEE_FLOOR_PERCENT);
        
        FeeSplit {
            validator_share: validator_percent,
            burn_share: 1.0 - validator_percent,
        }
    }

    pub fn calculate_fee_split(&self, fee_amount: f64) -> (f64, f64) {
        let split = self.get_adaptive_fee_split();
        (fee_amount * split.validator_share, fee_amount * split.burn_share)
    }

    pub fn get_stats(&self, checkpoint_height: u64) -> EmissionStats {
        let fee_split = self.get_adaptive_fee_split();
        EmissionStats {
            current_reward: self.get_checkpoint_reward(checkpoint_height),
            halving_epoch: self.get_halving_epoch(checkpoint_height),
            next_halving_at: self.get_next_halving_height(checkpoint_height),
            total_emitted: self.total_emitted,
            remaining_to_emit: self.get_remaining_to_emit(),
            circulating_supply: self.get_circulating_supply(),
            total_burned: self.total_burned,
            validator_fee_percent: fee_split.validator_share * 100.0,
            burn_percent: fee_split.burn_share * 100.0,
        }
    }

    pub fn get_total_emitted(&self) -> f64 {
        self.total_emitted
    }

    pub fn get_total_burned(&self) -> f64 {
        self.total_burned
    }

    pub fn to_json(&self) -> EmissionSnapshot {
        EmissionSnapshot {
            total_emitted: self.total_emitted,
            total_burned: self.total_burned,
        }
    }

    pub fn from_json(snapshot: EmissionSnapshot) -> Self {
        Self::with_initial(snapshot.total_emitted, snapshot.total_burned)
    }

    /// Merge emission state from peer snapshot.
    /// Takes maximum values to prevent emission rollback exploits:
    /// - total_emitted: higher = more tokens emitted (prevents double-emission)
    /// - total_burned: higher = more tokens burned
    /// Returns (emitted_delta, burned_delta) for logging.
    pub fn merge_from(&mut self, snapshot: EmissionSnapshot) -> (f64, f64) {
        let old_emitted = self.total_emitted;
        let old_burned = self.total_burned;
        
        // Take maximum values - can't un-emit or un-burn tokens
        self.total_emitted = self.total_emitted.max(snapshot.total_emitted);
        self.total_burned = self.total_burned.max(snapshot.total_burned);
        
        let emitted_delta = self.total_emitted - old_emitted;
        let burned_delta = self.total_burned - old_burned;
        
        (emitted_delta, burned_delta)
    }
}

impl Default for EmissionService {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorWeightInfo {
    pub address: String,
    pub stake_amount: f64,
    pub age_weight: f64,
    pub missed_checkpoints: u32,
}

pub fn calculate_effective_age_weight(
    age_weight: f64,
    stake_amount: f64,
    missed_checkpoints: u32,
) -> f64 {
    if stake_amount < MIN_BOND_FOR_AGE_WEIGHT {
        return 0.0;
    }
    
    let decay_factor = (1.0 - AGE_DECAY_PER_MISS).powi(missed_checkpoints as i32);
    age_weight * decay_factor
}

pub fn distribute_checkpoint_reward(
    reward: f64,
    validators: &[ValidatorWeightInfo],
) -> Vec<(String, f64)> {
    if validators.is_empty() || reward <= 0.0 {
        return vec![];
    }

    let total_stake: f64 = validators.iter().map(|v| v.stake_amount).sum();
    
    let effective_age_weights: Vec<f64> = validators
        .iter()
        .map(|v| calculate_effective_age_weight(v.age_weight, v.stake_amount, v.missed_checkpoints))
        .collect();
    let total_effective_age: f64 = effective_age_weights.iter().sum();

    let stake_pool = reward * STAKE_WEIGHT_PERCENT;
    let age_pool = reward * AGE_WEIGHT_PERCENT;

    let mut distribution = Vec::new();

    for (i, validator) in validators.iter().enumerate() {
        let mut share = 0.0;
        
        if total_stake > 0.0 {
            share += (validator.stake_amount / total_stake) * stake_pool;
        }
        
        if total_effective_age > 0.0 {
            share += (effective_age_weights[i] / total_effective_age) * age_pool;
        }
        
        if share > 0.0 {
            distribution.push((validator.address.clone(), share));
        }
    }

    distribution
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_checkpoint_reward() {
        let service = EmissionService::new();
        let reward = service.get_checkpoint_reward(0);
        assert!((reward - INITIAL_CHECKPOINT_REWARD).abs() < 0.000001);
    }

    #[test]
    fn test_halving_reduces_reward() {
        let service = EmissionService::new();
        let initial = service.get_checkpoint_reward(0);
        let after_halving = service.get_checkpoint_reward(HALVING_INTERVAL);
        assert!(after_halving < initial);
        assert!((after_halving - initial / 2.0).abs() < 0.000001);
    }

    #[test]
    fn test_fee_split() {
        let service = EmissionService::new();
        let split = service.get_adaptive_fee_split();
        assert!(split.validator_share >= VALIDATOR_FEE_FLOOR_PERCENT);
        assert!((split.validator_share + split.burn_share - 1.0).abs() < 0.000001);
    }

    #[test]
    fn test_circulating_supply() {
        let service = EmissionService::new();
        assert_eq!(service.get_circulating_supply(), GENESIS_ALLOCATION);
    }

    #[test]
    fn test_micro_units_precision() {
        let service = EmissionService::new();
        let reward = service.get_checkpoint_reward(HALVING_INTERVAL * 2);
        assert!((reward - INITIAL_CHECKPOINT_REWARD / 4.0).abs() < 0.000001);
    }

    #[test]
    fn test_effective_age_weight() {
        let full_weight = calculate_effective_age_weight(1.0, 500.0, 0);
        assert_eq!(full_weight, 1.0);

        let decayed = calculate_effective_age_weight(1.0, 500.0, 2);
        assert!((decayed - 0.81).abs() < 0.01);

        let insufficient_stake = calculate_effective_age_weight(1.0, 50.0, 0);
        assert_eq!(insufficient_stake, 0.0);
    }

    #[test]
    fn test_reward_distribution() {
        let validators = vec![
            ValidatorWeightInfo {
                address: "v1".to_string(),
                stake_amount: 1000.0,
                age_weight: 1.0,
                missed_checkpoints: 0,
            },
            ValidatorWeightInfo {
                address: "v2".to_string(),
                stake_amount: 1000.0,
                age_weight: 1.0,
                missed_checkpoints: 0,
            },
        ];

        let dist = distribute_checkpoint_reward(100.0, &validators);
        assert_eq!(dist.len(), 2);
        
        let total: f64 = dist.iter().map(|(_, amount)| *amount).sum();
        assert!((total - 100.0).abs() < 0.001);
    }
}
