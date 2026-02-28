use serde::{Deserialize, Serialize};
use rinku_core::types::{MICRO_UNITS, micro_serde, from_micro_units};

pub const MAX_SUPPLY: u64 = 3_000_000_000_000_000;
pub const GENESIS_ALLOCATION: u64 = 600_000_000_000_000;
pub const INITIAL_CHECKPOINT_REWARD: u64 = 393_241_100;
pub const HALVING_INTERVAL: u64 = 3_150_000;
pub const MIN_CHECKPOINT_REWARD: u64 = 12_288_700;
pub const HALVINGS_COUNT: u32 = 5;

pub const VALIDATOR_FEE_FLOOR_PERCENT: f64 = 0.70;
pub const BURN_CEILING_PERCENT: f64 = 0.30;
pub const SUPPLY_TARGET_FOR_FULL_BURN: f64 = 0.50;

pub const STAKE_WEIGHT_PERCENT: f64 = 0.70;
pub const AGE_WEIGHT_PERCENT: f64 = 0.30;
pub const MIN_BOND_FOR_AGE_WEIGHT: u64 = 10_000_000_000;
pub const AGE_DECAY_PER_MISS: f64 = 0.10;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmissionStats {
    #[serde(with = "micro_serde")]
    pub current_reward: u64,
    pub halving_epoch: u32,
    pub next_halving_at: u64,
    #[serde(with = "micro_serde")]
    pub total_emitted: u64,
    #[serde(with = "micro_serde")]
    pub remaining_to_emit: u64,
    #[serde(with = "micro_serde")]
    pub circulating_supply: u64,
    #[serde(with = "micro_serde")]
    pub total_burned: u64,
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
    #[serde(with = "micro_serde")]
    pub total_emitted: u64,
    #[serde(with = "micro_serde")]
    pub total_burned: u64,
}

pub struct EmissionService {
    total_emitted: u64,
    total_burned: u64,
}

impl EmissionService {
    pub fn new() -> Self {
        Self {
            total_emitted: 0,
            total_burned: 0,
        }
    }

    pub fn with_initial(total_emitted: u64, total_burned: u64) -> Self {
        Self {
            total_emitted,
            total_burned,
        }
    }

    pub fn get_checkpoint_reward(&self, checkpoint_height: u64) -> u64 {
        let halvings = (checkpoint_height / HALVING_INTERVAL) as u32;
        let effective_halvings = halvings.min(HALVINGS_COUNT);
        
        let reward = INITIAL_CHECKPOINT_REWARD >> effective_halvings;
        reward.max(MIN_CHECKPOINT_REWARD)
    }

    pub fn get_halving_epoch(&self, checkpoint_height: u64) -> u32 {
        (checkpoint_height / HALVING_INTERVAL) as u32
    }

    pub fn get_next_halving_height(&self, checkpoint_height: u64) -> u64 {
        let current_epoch = self.get_halving_epoch(checkpoint_height);
        (current_epoch as u64 + 1) * HALVING_INTERVAL
    }

    pub fn record_emission(&mut self, amount: u64) {
        self.total_emitted += amount;
    }

    pub fn record_burn(&mut self, amount: u64) {
        self.total_burned += amount;
    }

    pub fn get_circulating_supply(&self) -> u64 {
        GENESIS_ALLOCATION + self.total_emitted - self.total_burned
    }

    pub fn get_remaining_to_emit(&self) -> u64 {
        let max_emittable = MAX_SUPPLY - GENESIS_ALLOCATION;
        max_emittable.saturating_sub(self.total_emitted)
    }

    pub fn progressive_burn(&self) -> f64 {
        let circulating_supply = self.get_circulating_supply();
        let supply_ratio = circulating_supply as f64 / MAX_SUPPLY as f64;
        
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

    pub fn calculate_fee_split(&self, fee_amount: u64) -> (u64, u64) {
        let split = self.get_adaptive_fee_split();
        let validator_share = (fee_amount as f64 * split.validator_share).round() as u64;
        let burn_share = fee_amount.saturating_sub(validator_share);
        (validator_share, burn_share)
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

    pub fn get_total_emitted(&self) -> u64 {
        self.total_emitted
    }

    pub fn get_total_burned(&self) -> u64 {
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

    pub fn merge_from(&mut self, snapshot: EmissionSnapshot) -> (u64, u64) {
        let old_emitted = self.total_emitted;
        let old_burned = self.total_burned;
        
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
    #[serde(with = "micro_serde")]
    pub stake_amount: u64,
    pub age_weight: f64,
    pub missed_checkpoints: u32,
}

pub fn calculate_effective_age_weight(
    age_weight: f64,
    stake_amount: u64,
    missed_checkpoints: u32,
) -> f64 {
    if stake_amount < MIN_BOND_FOR_AGE_WEIGHT {
        return 0.0;
    }
    
    let decay_factor = (1.0 - AGE_DECAY_PER_MISS).powi(missed_checkpoints as i32);
    age_weight * decay_factor
}

pub fn distribute_checkpoint_reward(
    reward: u64,
    validators: &[ValidatorWeightInfo],
) -> Vec<(String, u64)> {
    if validators.is_empty() || reward == 0 {
        return vec![];
    }

    let total_stake: u64 = validators.iter().map(|v| v.stake_amount).sum();
    
    let effective_age_weights: Vec<f64> = validators
        .iter()
        .map(|v| calculate_effective_age_weight(v.age_weight, v.stake_amount, v.missed_checkpoints))
        .collect();
    let total_effective_age: f64 = effective_age_weights.iter().sum();

    let stake_pool = (reward as f64 * STAKE_WEIGHT_PERCENT).round() as u64;
    let age_pool = reward.saturating_sub(stake_pool);

    let mut distribution = Vec::new();
    let mut distributed = 0u64;

    for (i, validator) in validators.iter().enumerate() {
        let mut share = 0u64;
        
        if total_stake > 0 {
            share += ((stake_pool as u128 * validator.stake_amount as u128) / total_stake as u128) as u64;
        }
        
        if total_effective_age > 0.0 {
            share += (age_pool as f64 * effective_age_weights[i] / total_effective_age).round() as u64;
        }
        
        if share > 0 {
            distributed += share;
            distribution.push((validator.address.clone(), share));
        }
    }

    distribution
}

#[cfg(test)]
mod tests {
    use super::*;
    use rinku_core::types::to_micro_units;

    #[test]
    fn test_initial_checkpoint_reward() {
        let service = EmissionService::new();
        let reward = service.get_checkpoint_reward(0);
        assert_eq!(reward, INITIAL_CHECKPOINT_REWARD);
    }

    #[test]
    fn test_halving_reduces_reward() {
        let service = EmissionService::new();
        let initial = service.get_checkpoint_reward(0);
        let after_halving = service.get_checkpoint_reward(HALVING_INTERVAL);
        assert!(after_halving < initial);
        assert_eq!(after_halving, initial >> 1);
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
        assert_eq!(reward, INITIAL_CHECKPOINT_REWARD >> 2);
    }

    #[test]
    fn test_effective_age_weight() {
        let full_weight = calculate_effective_age_weight(1.0, to_micro_units(500.0), 0);
        assert_eq!(full_weight, 1.0);

        let decayed = calculate_effective_age_weight(1.0, to_micro_units(500.0), 2);
        assert!((decayed - 0.81).abs() < 0.01);

        let insufficient_stake = calculate_effective_age_weight(1.0, to_micro_units(50.0), 0);
        assert_eq!(insufficient_stake, 0.0);
    }

    #[test]
    fn test_reward_distribution() {
        let validators = vec![
            ValidatorWeightInfo {
                address: "v1".to_string(),
                stake_amount: to_micro_units(1000.0),
                age_weight: 1.0,
                missed_checkpoints: 0,
            },
            ValidatorWeightInfo {
                address: "v2".to_string(),
                stake_amount: to_micro_units(1000.0),
                age_weight: 1.0,
                missed_checkpoints: 0,
            },
        ];

        let reward = to_micro_units(100.0);
        let dist = distribute_checkpoint_reward(reward, &validators);
        assert_eq!(dist.len(), 2);
        
        let total: u64 = dist.iter().map(|(_, amount)| *amount).sum();
        let diff = if total > reward { total - reward } else { reward - total };
        assert!(diff <= 2, "Rounding error should be minimal");
    }
}
