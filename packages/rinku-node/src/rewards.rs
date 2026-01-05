use std::collections::HashMap;
use tracing::info;

#[derive(Debug, Clone)]
pub struct StakePosition {
    pub staker: String,
    pub amount: f64,
    pub staked_at: u64,
    pub last_reward_at: u64,
}

#[derive(Debug, Clone)]
pub struct RewardConfig {
    pub tip_reward_percent: f64,
    pub stake_reward_percent: f64,
    pub witness_reward_percent: f64,
    pub min_stake_for_rewards: f64,
}

impl Default for RewardConfig {
    fn default() -> Self {
        Self {
            tip_reward_percent: 0.30,
            stake_reward_percent: 0.50,
            witness_reward_percent: 0.20,
            min_stake_for_rewards: 100.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Reward {
    pub recipient: String,
    pub amount: f64,
    pub reward_type: RewardType,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub enum RewardType {
    Tip,
    Stake,
    Witness,
}

pub struct RewardsService {
    config: RewardConfig,
    stakes: HashMap<String, StakePosition>,
    pending_rewards: HashMap<String, f64>,
    total_distributed: f64,
    witnessed_count: u64,
}

impl RewardsService {
    pub fn new(config: RewardConfig) -> Self {
        Self {
            config,
            stakes: HashMap::new(),
            pending_rewards: HashMap::new(),
            total_distributed: 0.0,
            witnessed_count: 0,
        }
    }

    pub fn get_config(&self) -> &RewardConfig {
        &self.config
    }

    pub fn stake(&mut self, staker: String, amount: f64) -> bool {
        if amount < self.config.min_stake_for_rewards {
            return false;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let position = self.stakes.entry(staker.clone()).or_insert(StakePosition {
            staker: staker.clone(),
            amount: 0.0,
            staked_at: now,
            last_reward_at: now,
        });
        position.amount += amount;
        
        info!("Staked {} RKU for {}", amount, staker);
        true
    }

    pub fn unstake(&mut self, staker: &str, amount: f64) -> Option<f64> {
        if let Some(position) = self.stakes.get_mut(staker) {
            if position.amount >= amount {
                position.amount -= amount;
                if position.amount < self.config.min_stake_for_rewards {
                    self.stakes.remove(staker);
                }
                info!("Unstaked {} RKU for {}", amount, staker);
                return Some(amount);
            }
        }
        None
    }

    pub fn get_stake(&self, staker: &str) -> Option<&StakePosition> {
        self.stakes.get(staker)
    }

    pub fn get_active_validators(&self) -> Vec<&StakePosition> {
        self.stakes
            .values()
            .filter(|s| s.amount >= self.config.min_stake_for_rewards)
            .collect()
    }

    pub fn get_total_staked(&self) -> f64 {
        self.stakes.values().map(|s| s.amount).sum()
    }

    pub fn add_pending_reward(&mut self, address: &str, amount: f64) {
        let current = self.pending_rewards.entry(address.to_string()).or_insert(0.0);
        *current += amount;
    }

    pub fn get_pending_rewards(&self, address: &str) -> f64 {
        self.pending_rewards.get(address).copied().unwrap_or(0.0)
    }

    pub fn claim_rewards(&mut self, address: &str) -> f64 {
        let amount = self.pending_rewards.remove(address).unwrap_or(0.0);
        if amount > 0.0 {
            self.total_distributed += amount;
        }
        amount
    }

    pub fn distribute_checkpoint_rewards(&mut self, reward_amount: f64) -> Vec<(String, f64)> {
        let total_stake = self.get_total_staked();
        if total_stake <= 0.0 {
            return vec![];
        }

        let validator_data: Vec<(String, f64)> = self
            .stakes
            .values()
            .filter(|s| s.amount >= self.config.min_stake_for_rewards)
            .map(|s| (s.staker.clone(), s.amount))
            .collect();

        let mut distributions = Vec::new();

        for (staker, amount) in validator_data {
            let share = amount / total_stake;
            let reward = reward_amount * share;
            self.add_pending_reward(&staker, reward);
            distributions.push((staker, reward));
        }

        distributions
    }

    pub fn get_stats(&self) -> RewardsStats {
        RewardsStats {
            total_staked: self.get_total_staked(),
            validator_count: self.stakes.len(),
            total_distributed: self.total_distributed,
            witnessed_count: self.witnessed_count,
        }
    }

    pub fn increment_witnessed(&mut self) {
        self.witnessed_count += 1;
    }

    pub fn to_json(&self) -> RewardsSnapshot {
        RewardsSnapshot {
            stakes: self.stakes.clone(),
            pending_rewards: self.pending_rewards.clone(),
            total_distributed: self.total_distributed,
            witnessed_count: self.witnessed_count,
        }
    }

    pub fn from_json(snapshot: RewardsSnapshot, config: RewardConfig) -> Self {
        Self {
            config,
            stakes: snapshot.stakes,
            pending_rewards: snapshot.pending_rewards,
            total_distributed: snapshot.total_distributed,
            witnessed_count: snapshot.witnessed_count,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RewardsStats {
    pub total_staked: f64,
    pub validator_count: usize,
    pub total_distributed: f64,
    pub witnessed_count: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RewardsSnapshot {
    pub stakes: HashMap<String, StakePosition>,
    pub pending_rewards: HashMap<String, f64>,
    pub total_distributed: f64,
    pub witnessed_count: u64,
}

impl serde::Serialize for StakePosition {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("StakePosition", 4)?;
        state.serialize_field("staker", &self.staker)?;
        state.serialize_field("amount", &self.amount)?;
        state.serialize_field("stakedAt", &self.staked_at)?;
        state.serialize_field("lastRewardAt", &self.last_reward_at)?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for StakePosition {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Helper {
            staker: String,
            amount: f64,
            staked_at: u64,
            last_reward_at: u64,
        }
        let helper = Helper::deserialize(deserializer)?;
        Ok(StakePosition {
            staker: helper.staker,
            amount: helper.amount,
            staked_at: helper.staked_at,
            last_reward_at: helper.last_reward_at,
        })
    }
}
