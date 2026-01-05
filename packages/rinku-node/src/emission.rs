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

#[derive(Debug, Clone)]
pub struct FeeSplit {
    pub validator_share: f64,
    pub burn_share: f64,
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

    pub fn get_checkpoint_reward(&self, checkpoint_height: u64) -> f64 {
        let halvings = checkpoint_height / HALVING_INTERVAL;
        let effective_halvings = std::cmp::min(halvings as u32, HALVINGS_COUNT);
        let divisor = 2_u64.pow(effective_halvings);
        let reward = INITIAL_CHECKPOINT_REWARD / divisor as f64;
        reward.max(MIN_CHECKPOINT_REWARD)
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
        let validator_percent = VALIDATOR_FEE_FLOOR_PERCENT.max(1.0 - burn_percent);

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
        Self {
            total_emitted: snapshot.total_emitted,
            total_burned: snapshot.total_burned,
        }
    }
}

impl Default for EmissionService {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmissionSnapshot {
    pub total_emitted: f64,
    pub total_burned: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_rewards() {
        let service = EmissionService::new();
        
        let reward_0 = service.get_checkpoint_reward(0);
        assert!((reward_0 - INITIAL_CHECKPOINT_REWARD).abs() < 0.0001);

        let reward_halving = service.get_checkpoint_reward(HALVING_INTERVAL);
        assert!((reward_halving - INITIAL_CHECKPOINT_REWARD / 2.0).abs() < 0.0001);

        let reward_2halving = service.get_checkpoint_reward(HALVING_INTERVAL * 2);
        assert!((reward_2halving - INITIAL_CHECKPOINT_REWARD / 4.0).abs() < 0.0001);
    }

    #[test]
    fn test_circulating_supply() {
        let mut service = EmissionService::new();
        assert!((service.get_circulating_supply() - GENESIS_ALLOCATION).abs() < 0.0001);

        service.record_emission(1000.0);
        assert!((service.get_circulating_supply() - (GENESIS_ALLOCATION + 1000.0)).abs() < 0.0001);

        service.record_burn(500.0);
        assert!((service.get_circulating_supply() - (GENESIS_ALLOCATION + 500.0)).abs() < 0.0001);
    }

    #[test]
    fn test_fee_split() {
        let service = EmissionService::new();
        let split = service.get_adaptive_fee_split();
        
        assert!(split.validator_share >= VALIDATOR_FEE_FLOOR_PERCENT);
        assert!(split.burn_share <= BURN_CEILING_PERCENT);
        assert!((split.validator_share + split.burn_share - 1.0).abs() < 0.0001);
    }
}
