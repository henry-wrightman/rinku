use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const WITNESS_TTL_MS: u64 = 3_600_000;
pub const QUEUE_COMPACT_THRESHOLD: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardConfig {
    pub tip_reward_percent: f64,
    pub stake_reward_percent: f64,
    pub witness_reward_percent: f64,
    pub min_stake_amount: f64,
    pub min_stake_for_rewards: f64,
    pub unstake_cooldown_ms: u64,
}

impl Default for RewardConfig {
    fn default() -> Self {
        Self {
            tip_reward_percent: 0.01,
            stake_reward_percent: 0.005,
            witness_reward_percent: 0.002,
            min_stake_amount: 100.0,
            min_stake_for_rewards: 100.0,
            unstake_cooldown_ms: 24 * 60 * 60 * 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StakePosition {
    pub staker: String,
    pub amount: f64,
    pub staked_at: u64,
    pub last_reward_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StakingStatus {
    pub address: String,
    pub position: Option<StakePosition>,
    pub stake_rewards_total: f64,
    pub can_unstake: bool,
    pub cooldown_remaining_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Reward {
    #[serde(rename = "tip")]
    Tip(TipReward),
    #[serde(rename = "stake")]
    Stake(StakeReward),
    #[serde(rename = "witness")]
    Witness(WitnessReward),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TipReward {
    pub recipient: String,
    pub amount: f64,
    pub tx_url: String,
    pub tip_url: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StakeReward {
    pub recipient: String,
    pub amount: f64,
    pub tx_url: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WitnessReward {
    pub recipient: String,
    pub amount: f64,
    pub referenced_tx_url: String,
    pub referencing_tx_url: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardsSummary {
    pub address: String,
    pub tip_rewards: f64,
    pub stake_rewards: f64,
    pub witness_rewards: f64,
    pub total_rewards: f64,
    pub pending_rewards: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WitnessedEntry {
    key: String,
    ts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardsSnapshot {
    pub rewards: Vec<(String, Vec<Reward>)>,
    pub stakes: Vec<(String, StakePosition)>,
    pub pending_rewards: Vec<(String, f64)>,
    pub witnessed_txs: Vec<(String, u64)>,
    pub witnessed_queue: Vec<WitnessedEntry>,
    pub config: RewardConfig,
}

pub struct RewardsService {
    config: RewardConfig,
    rewards: HashMap<String, Vec<Reward>>,
    stakes: HashMap<String, StakePosition>,
    pending_rewards: HashMap<String, f64>,
    witnessed_txs: HashMap<String, u64>,
    witnessed_queue: Vec<WitnessedEntry>,
    witnessed_queue_head: usize,
}

impl RewardsService {
    pub fn new(config: RewardConfig) -> Self {
        Self {
            config,
            rewards: HashMap::new(),
            stakes: HashMap::new(),
            pending_rewards: HashMap::new(),
            witnessed_txs: HashMap::new(),
            witnessed_queue: Vec::new(),
            witnessed_queue_head: 0,
        }
    }

    pub fn get_config(&self) -> &RewardConfig {
        &self.config
    }

    pub fn calculate_tip_reward(&self, tx_amount: f64) -> f64 {
        tx_amount * self.config.tip_reward_percent
    }

    pub fn calculate_stake_reward(&self, stake_amount: f64, tx_amount: f64) -> f64 {
        let total_staked = self.get_total_staked();
        if total_staked <= 0.0 {
            return 0.0;
        }
        let stake_share = stake_amount / total_staked;
        tx_amount * self.config.stake_reward_percent * stake_share
    }

    pub fn calculate_witness_reward(&self, tx_amount: f64) -> f64 {
        tx_amount * self.config.witness_reward_percent
    }

    pub fn process_tip_reward(
        &mut self,
        tx_url: &str,
        tip_url: &str,
        recipient: &str,
        tx_amount: f64,
    ) -> Option<TipReward> {
        let amount = self.calculate_tip_reward(tx_amount);
        if amount <= 0.0 {
            return None;
        }

        let reward = TipReward {
            recipient: recipient.to_string(),
            amount,
            tx_url: tx_url.to_string(),
            tip_url: tip_url.to_string(),
            timestamp: current_time_ms(),
        };

        self.add_reward(recipient, Reward::Tip(reward.clone()));
        Some(reward)
    }

    pub fn process_stake_rewards(&mut self, tx_url: &str, tx_amount: f64) -> Vec<StakeReward> {
        let mut rewards = Vec::new();
        let validators = self.get_active_validators_data();

        if validators.is_empty() {
            return rewards;
        }

        let now = current_time_ms();

        for (staker, stake_amount) in validators {
            let amount = self.calculate_stake_reward(stake_amount, tx_amount);
            if amount <= 0.0 {
                continue;
            }

            let reward = StakeReward {
                recipient: staker.clone(),
                amount,
                tx_url: tx_url.to_string(),
                timestamp: now,
            };

            self.add_reward(&staker, Reward::Stake(reward.clone()));
            if let Some(stake) = self.stakes.get_mut(&staker) {
                stake.last_reward_at = Some(now);
            }
            rewards.push(reward);
        }

        rewards
    }

    pub fn process_witness_reward(
        &mut self,
        referencing_tx_url: &str,
        referenced_tx_url: &str,
        recipient: &str,
        tx_amount: f64,
    ) -> Option<WitnessReward> {
        let key = format!("{}:{}", referencing_tx_url, referenced_tx_url);
        if self.witnessed_txs.contains_key(&key) {
            return None;
        }

        let amount = self.calculate_witness_reward(tx_amount);
        if amount <= 0.0 {
            return None;
        }

        let now = current_time_ms();
        let reward = WitnessReward {
            recipient: recipient.to_string(),
            amount,
            referenced_tx_url: referenced_tx_url.to_string(),
            referencing_tx_url: referencing_tx_url.to_string(),
            timestamp: now,
        };

        self.add_reward(recipient, Reward::Witness(reward.clone()));
        self.witnessed_txs.insert(key.clone(), now);
        self.witnessed_queue.push(WitnessedEntry { key, ts: now });

        Some(reward)
    }

    pub fn stake(&mut self, staker: &str, amount: f64) -> Result<StakePosition, String> {
        if amount < self.config.min_stake_amount {
            return Err(format!(
                "Minimum stake amount is {}",
                self.config.min_stake_amount
            ));
        }

        let now = current_time_ms();

        if let Some(existing) = self.stakes.get_mut(staker) {
            existing.amount += amount;
            existing.staked_at = now;
            return Ok(existing.clone());
        }

        let position = StakePosition {
            staker: staker.to_string(),
            amount,
            staked_at: now,
            last_reward_at: None,
        };
        self.stakes.insert(staker.to_string(), position.clone());

        Ok(position)
    }

    pub fn unstake(&mut self, staker: &str) -> Result<f64, String> {
        let position = self
            .stakes
            .get(staker)
            .ok_or("No stake found")?;

        let now = current_time_ms();
        let can_unstake_at = position.staked_at + self.config.unstake_cooldown_ms;

        if now < can_unstake_at {
            let remaining_ms = can_unstake_at - now;
            let remaining_hours = (remaining_ms as f64 / (60.0 * 60.0 * 1000.0)).ceil() as u64;
            return Err(format!(
                "Cooldown not complete. {} hours remaining.",
                remaining_hours
            ));
        }

        let amount = position.amount;
        self.stakes.remove(staker);

        Ok(amount)
    }

    pub fn get_stake(&self, staker: &str) -> Option<&StakePosition> {
        self.stakes.get(staker)
    }

    pub fn update_stake(&mut self, staker: &str, new_amount: f64) {
        if let Some(stake) = self.stakes.get_mut(staker) {
            stake.amount = new_amount;
        }
    }

    pub fn remove_stake(&mut self, staker: &str) {
        self.stakes.remove(staker);
    }

    pub fn get_staking_status(&self, address: &str) -> StakingStatus {
        let position = self.stakes.get(address).cloned();
        let rewards = self.rewards.get(address).map(|r| r.as_slice()).unwrap_or(&[]);
        let stake_rewards_total: f64 = rewards
            .iter()
            .filter_map(|r| match r {
                Reward::Stake(sr) => Some(sr.amount),
                _ => None,
            })
            .sum();

        let now = current_time_ms();
        let (can_unstake, cooldown_remaining_ms) = if let Some(ref pos) = position {
            let can_unstake_at = pos.staked_at + self.config.unstake_cooldown_ms;
            if now >= can_unstake_at {
                (true, 0)
            } else {
                (false, can_unstake_at - now)
            }
        } else {
            (false, 0)
        };

        StakingStatus {
            address: address.to_string(),
            position,
            stake_rewards_total,
            can_unstake,
            cooldown_remaining_ms,
        }
    }

    pub fn get_rewards_summary(&self, address: &str) -> RewardsSummary {
        let rewards = self.rewards.get(address).map(|r| r.as_slice()).unwrap_or(&[]);
        let pending = self.pending_rewards.get(address).copied().unwrap_or(0.0);

        let mut tip_rewards = 0.0;
        let mut stake_rewards = 0.0;
        let mut witness_rewards = 0.0;

        for reward in rewards {
            match reward {
                Reward::Tip(r) => tip_rewards += r.amount,
                Reward::Stake(r) => stake_rewards += r.amount,
                Reward::Witness(r) => witness_rewards += r.amount,
            }
        }

        RewardsSummary {
            address: address.to_string(),
            tip_rewards,
            stake_rewards,
            witness_rewards,
            total_rewards: tip_rewards + stake_rewards + witness_rewards,
            pending_rewards: pending,
        }
    }

    pub fn get_active_validators(&self) -> Vec<&StakePosition> {
        self.stakes
            .values()
            .filter(|s| s.amount >= self.config.min_stake_amount)
            .collect()
    }

    fn get_active_validators_data(&self) -> Vec<(String, f64)> {
        self.stakes
            .values()
            .filter(|s| s.amount >= self.config.min_stake_amount)
            .map(|s| (s.staker.clone(), s.amount))
            .collect()
    }

    pub fn get_total_staked(&self) -> f64 {
        self.stakes.values().map(|s| s.amount).sum()
    }
    
    pub fn get_all_stakes(&self) -> Vec<&StakePosition> {
        self.stakes.values().collect()
    }

    pub fn get_pending_rewards(&self, address: &str) -> f64 {
        self.pending_rewards.get(address).copied().unwrap_or(0.0)
    }

    pub fn claim_rewards(&mut self, address: &str) -> f64 {
        let pending = self.pending_rewards.get(address).copied().unwrap_or(0.0);
        if pending > 0.0 {
            self.pending_rewards.insert(address.to_string(), 0.0);
        }
        pending
    }

    pub fn distribute_checkpoint_rewards(&mut self, reward_amount: f64) -> Vec<(String, f64)> {
        let total_stake = self.get_total_staked();
        if total_stake <= 0.0 {
            return vec![];
        }

        let validator_data = self.get_active_validators_data();
        let mut distributions = Vec::new();

        for (staker, amount) in validator_data {
            let share = amount / total_stake;
            let reward = reward_amount * share;
            self.add_pending_reward(&staker, reward);
            distributions.push((staker, reward));
        }

        distributions
    }

    pub fn distribute_fee_to_validators(&mut self, fee_amount: f64) -> Vec<(String, f64)> {
        self.distribute_checkpoint_rewards(fee_amount)
    }

    pub fn get_top_stakers(&self, limit: usize) -> Vec<&StakePosition> {
        let mut stakers: Vec<_> = self.stakes.values().collect();
        stakers.sort_by(|a, b| b.amount.partial_cmp(&a.amount).unwrap());
        stakers.truncate(limit);
        stakers
    }

    fn add_reward(&mut self, address: &str, reward: Reward) {
        let amount = match &reward {
            Reward::Tip(r) => r.amount,
            Reward::Stake(r) => r.amount,
            Reward::Witness(r) => r.amount,
        };

        let rewards = self.rewards.entry(address.to_string()).or_default();
        rewards.push(reward);
        if rewards.len() > 100 {
            rewards.drain(0..rewards.len() - 100);
        }

        self.add_pending_reward(address, amount);
    }

    fn add_pending_reward(&mut self, address: &str, amount: f64) {
        let pending = self.pending_rewards.entry(address.to_string()).or_insert(0.0);
        *pending += amount;
    }

    pub fn prune_old_data(&mut self) -> usize {
        let now = current_time_ms();
        let cutoff = now.saturating_sub(WITNESS_TTL_MS);
        let mut pruned = 0;

        while self.witnessed_queue_head < self.witnessed_queue.len() {
            let oldest = &self.witnessed_queue[self.witnessed_queue_head];
            if oldest.ts >= cutoff {
                break;
            }
            self.witnessed_queue_head += 1;
            if self.witnessed_txs.get(&oldest.key) == Some(&oldest.ts) {
                self.witnessed_txs.remove(&oldest.key);
                pruned += 1;
            }
        }

        if self.witnessed_queue_head >= QUEUE_COMPACT_THRESHOLD {
            self.witnessed_queue = self.witnessed_queue[self.witnessed_queue_head..].to_vec();
            self.witnessed_queue_head = 0;
        }

        pruned
    }

    pub fn get_stats(&self) -> RewardsStats {
        RewardsStats {
            total_staked: self.get_total_staked(),
            rewards_count: self.rewards.values().map(|v| v.len()).sum(),
            stakes_count: self.stakes.len(),
            witnessed_count: self.witnessed_txs.len(),
            pending_count: self.pending_rewards.len(),
        }
    }

    pub fn to_json(&self) -> RewardsSnapshot {
        let active_queue = self.witnessed_queue[self.witnessed_queue_head..].to_vec();
        RewardsSnapshot {
            rewards: self.rewards.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            stakes: self.stakes.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            pending_rewards: self.pending_rewards.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            witnessed_txs: self.witnessed_txs.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            witnessed_queue: active_queue,
            config: self.config.clone(),
        }
    }

    pub fn from_json(snapshot: RewardsSnapshot) -> Self {
        let mut service = Self::new(snapshot.config);

        for (addr, rewards) in snapshot.rewards {
            service.rewards.insert(addr, rewards);
        }

        for (addr, stake) in snapshot.stakes {
            service.stakes.insert(addr, stake);
        }

        for (addr, pending) in snapshot.pending_rewards {
            service.pending_rewards.insert(addr, pending);
        }

        for (key, ts) in snapshot.witnessed_txs {
            service.witnessed_txs.insert(key, ts);
        }

        service.witnessed_queue = snapshot.witnessed_queue;
        service.witnessed_queue_head = 0;

        service
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardsStats {
    pub total_staked: f64,
    pub rewards_count: usize,
    pub stakes_count: usize,
    pub witnessed_count: usize,
    pub pending_count: usize,
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stake_and_unstake() {
        let mut service = RewardsService::new(RewardConfig::default());

        let result = service.stake("validator1", 1000.0);
        assert!(result.is_ok());

        let stake = service.get_stake("validator1");
        assert!(stake.is_some());
        assert_eq!(stake.unwrap().amount, 1000.0);

        let unstake_result = service.unstake("validator1");
        assert!(unstake_result.is_err());
    }

    #[test]
    fn test_tip_rewards() {
        let mut service = RewardsService::new(RewardConfig::default());

        let reward = service.process_tip_reward("tx1", "tip1", "recipient1", 100.0);
        assert!(reward.is_some());
        assert_eq!(reward.unwrap().amount, 1.0);
    }

    #[test]
    fn test_witness_dedup() {
        let mut service = RewardsService::new(RewardConfig::default());

        let first = service.process_witness_reward("tx1", "ref1", "recipient1", 100.0);
        assert!(first.is_some());

        let second = service.process_witness_reward("tx1", "ref1", "recipient1", 100.0);
        assert!(second.is_none());
    }
}
