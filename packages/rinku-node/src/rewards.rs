use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use rinku_core::types::{micro_serde, from_micro_units};

pub const WITNESS_TTL_MS: u64 = 3_600_000;
pub const QUEUE_COMPACT_THRESHOLD: usize = 10_000;
pub const MAX_WITNESSED_ENTRIES: usize = 20_000;
pub const MAX_LIFETIME_ENTRIES: usize = 10_000;
pub const MAX_PENDING_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardConfig {
    pub tip_reward_percent: f64,
    pub stake_reward_percent: f64,
    pub witness_reward_percent: f64,
    #[serde(with = "micro_serde")]
    pub min_stake_amount: u64,
    #[serde(with = "micro_serde")]
    pub min_stake_for_rewards: u64,
    pub unstake_cooldown_ms: u64,
}

impl Default for RewardConfig {
    fn default() -> Self {
        Self {
            tip_reward_percent: 0.01,
            stake_reward_percent: 0.005,
            witness_reward_percent: 0.002,
            min_stake_amount: 10_000_000_000,
            min_stake_for_rewards: 10_000_000_000,
            unstake_cooldown_ms: 24 * 60 * 60 * 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StakePosition {
    pub staker: String,
    #[serde(with = "micro_serde")]
    pub amount: u64,
    pub staked_at: u64,
    pub last_reward_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StakingStatus {
    pub address: String,
    pub position: Option<StakePosition>,
    #[serde(with = "micro_serde")]
    pub stake_rewards_total: u64,
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
    #[serde(with = "micro_serde")]
    pub amount: u64,
    pub tx_url: String,
    pub tip_url: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StakeReward {
    pub recipient: String,
    #[serde(with = "micro_serde")]
    pub amount: u64,
    pub tx_url: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WitnessReward {
    pub recipient: String,
    #[serde(with = "micro_serde")]
    pub amount: u64,
    pub referenced_tx_url: String,
    pub referencing_tx_url: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardsSummary {
    pub address: String,
    #[serde(with = "micro_serde")]
    pub tip_rewards: u64,
    #[serde(with = "micro_serde")]
    pub stake_rewards: u64,
    #[serde(with = "micro_serde")]
    pub witness_rewards: u64,
    #[serde(with = "micro_serde")]
    pub total_rewards: u64,
    #[serde(with = "micro_serde")]
    pub pending_rewards: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WitnessedEntry {
    key: String,
    ts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LifetimeRewards {
    #[serde(with = "micro_serde")]
    pub tip_rewards: u64,
    #[serde(with = "micro_serde")]
    pub stake_rewards: u64,
    #[serde(with = "micro_serde")]
    pub witness_rewards: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardsSnapshot {
    pub rewards: Vec<(String, Vec<Reward>)>,
    pub stakes: Vec<(String, StakePosition)>,
    #[serde(default)]
    pub pending_rewards: Vec<(String, u64)>,
    pub witnessed_txs: Vec<(String, u64)>,
    pub witnessed_queue: Vec<WitnessedEntry>,
    pub config: RewardConfig,
    #[serde(default)]
    pub lifetime_rewards: Vec<(String, LifetimeRewards)>,
    #[serde(default)]
    pub claimed_rewards: Vec<(String, u64)>,
}

pub struct RewardsService {
    config: RewardConfig,
    rewards: HashMap<String, Vec<Reward>>,
    stakes: HashMap<String, StakePosition>,
    pending_rewards: HashMap<String, u64>,
    lifetime_rewards: HashMap<String, LifetimeRewards>,
    claimed_rewards: HashMap<String, u64>,
    witnessed_txs: HashMap<String, u64>,
    witnessed_queue: Vec<WitnessedEntry>,
    witnessed_queue_head: usize,
    processed_stake_tx_hashes: std::collections::HashSet<String>,
    processed_stake_tx_order: std::collections::VecDeque<String>,
}

impl RewardsService {
    pub fn new(config: RewardConfig) -> Self {
        Self {
            config,
            rewards: HashMap::new(),
            stakes: HashMap::new(),
            pending_rewards: HashMap::new(),
            lifetime_rewards: HashMap::new(),
            claimed_rewards: HashMap::new(),
            witnessed_txs: HashMap::new(),
            witnessed_queue: Vec::new(),
            witnessed_queue_head: 0,
            processed_stake_tx_hashes: std::collections::HashSet::new(),
            processed_stake_tx_order: std::collections::VecDeque::new(),
        }
    }

    pub fn get_config(&self) -> &RewardConfig {
        &self.config
    }

    pub fn calculate_tip_reward(&self, tx_amount: u64) -> u64 {
        (tx_amount as f64 * self.config.tip_reward_percent).round() as u64
    }

    pub fn calculate_stake_reward(&self, stake_amount: u64, tx_amount: u64) -> u64 {
        let total_staked = self.get_total_staked();
        if total_staked == 0 {
            return 0;
        }
        let stake_share = stake_amount as f64 / total_staked as f64;
        (tx_amount as f64 * self.config.stake_reward_percent * stake_share).round() as u64
    }

    pub fn calculate_witness_reward(&self, tx_amount: u64) -> u64 {
        (tx_amount as f64 * self.config.witness_reward_percent).round() as u64
    }

    pub fn process_tip_reward(
        &mut self,
        tx_url: &str,
        tip_url: &str,
        recipient: &str,
        tx_amount: u64,
    ) -> Option<TipReward> {
        let amount = self.calculate_tip_reward(tx_amount);
        if amount == 0 {
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

    pub fn process_stake_rewards(&mut self, tx_url: &str, tx_amount: u64) -> Vec<StakeReward> {
        let mut rewards = Vec::new();
        let validators = self.get_active_validators_data();

        if validators.is_empty() {
            return rewards;
        }

        let now = current_time_ms();

        for (staker, stake_amount) in validators {
            let amount = self.calculate_stake_reward(stake_amount, tx_amount);
            if amount == 0 {
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
        tx_amount: u64,
    ) -> Option<WitnessReward> {
        let ref_short = referencing_tx_url.chars().rev().take(16).collect::<String>();
        let parent_short = referenced_tx_url.chars().rev().take(16).collect::<String>();
        let key = format!("{}:{}", ref_short, parent_short);
        
        if self.witnessed_txs.contains_key(&key) {
            return None;
        }

        let amount = self.calculate_witness_reward(tx_amount);
        if amount == 0 {
            return None;
        }

        if self.witnessed_txs.len() >= MAX_WITNESSED_ENTRIES {
            self.force_prune_witnessed(MAX_WITNESSED_ENTRIES / 4);
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
    
    fn force_prune_witnessed(&mut self, count: usize) {
        let mut pruned = 0;
        while pruned < count && self.witnessed_queue_head < self.witnessed_queue.len() {
            let oldest = &self.witnessed_queue[self.witnessed_queue_head];
            self.witnessed_txs.remove(&oldest.key);
            self.witnessed_queue_head += 1;
            pruned += 1;
        }
        
        if self.witnessed_queue_head >= QUEUE_COMPACT_THRESHOLD {
            self.witnessed_queue = self.witnessed_queue[self.witnessed_queue_head..].to_vec();
            self.witnessed_queue_head = 0;
        }
    }

    pub fn stake(&mut self, staker: &str, amount: u64, tx_hash: &str) -> Result<StakePosition, String> {
        if amount < self.config.min_stake_amount {
            return Err(format!(
                "Minimum stake amount is {} RKU",
                from_micro_units(self.config.min_stake_amount)
            ));
        }

        if !tx_hash.is_empty() && self.processed_stake_tx_hashes.contains(tx_hash) {
            tracing::warn!(
                "STAKE DEDUP: skipping duplicate rewards.stake() for tx {} staker {} amount {}",
                &tx_hash[..16.min(tx_hash.len())],
                &staker[..16.min(staker.len())],
                amount
            );
            if let Some(existing) = self.stakes.get(staker) {
                return Ok(existing.clone());
            }
        }

        let now = current_time_ms();

        if let Some(existing) = self.stakes.get_mut(staker) {
            tracing::info!(
                "STAKE ADD: {} adding {} to existing {} (total will be {}) tx={}",
                &staker[..16.min(staker.len())],
                amount,
                existing.amount,
                existing.amount + amount,
                if tx_hash.is_empty() { "genesis" } else { &tx_hash[..16.min(tx_hash.len())] }
            );
            existing.amount += amount;
            existing.staked_at = now;
            if !tx_hash.is_empty() {
                self.processed_stake_tx_hashes.insert(tx_hash.to_string());
                self.processed_stake_tx_order.push_back(tx_hash.to_string());
            }
            return Ok(existing.clone());
        }

        tracing::info!(
            "STAKE NEW: {} staking {} tx={}",
            &staker[..16.min(staker.len())],
            amount,
            if tx_hash.is_empty() { "genesis" } else { &tx_hash[..16.min(tx_hash.len())] }
        );
        let position = StakePosition {
            staker: staker.to_string(),
            amount,
            staked_at: now,
            last_reward_at: None,
        };
        self.stakes.insert(staker.to_string(), position.clone());
        if !tx_hash.is_empty() {
            self.processed_stake_tx_hashes.insert(tx_hash.to_string());
            self.processed_stake_tx_order.push_back(tx_hash.to_string());
        }

        const MAX_PROCESSED_HASHES: usize = 50_000;
        while self.processed_stake_tx_hashes.len() > MAX_PROCESSED_HASHES {
            if let Some(oldest) = self.processed_stake_tx_order.pop_front() {
                self.processed_stake_tx_hashes.remove(&oldest);
            } else {
                break;
            }
        }

        Ok(position)
    }

    pub fn register_stake_dedup(&mut self, tx_hash: &str) {
        if tx_hash.is_empty() || self.processed_stake_tx_hashes.contains(tx_hash) {
            return;
        }
        self.processed_stake_tx_hashes.insert(tx_hash.to_string());
        self.processed_stake_tx_order.push_back(tx_hash.to_string());
        const MAX_PROCESSED_HASHES: usize = 50_000;
        while self.processed_stake_tx_hashes.len() > MAX_PROCESSED_HASHES {
            if let Some(oldest) = self.processed_stake_tx_order.pop_front() {
                self.processed_stake_tx_hashes.remove(&oldest);
            } else {
                break;
            }
        }
    }

    pub fn sync_stake_amount(&mut self, staker: &str, canonical_amount: u64) {
        if let Some(existing) = self.stakes.get_mut(staker) {
            if existing.amount != canonical_amount {
                tracing::info!(
                    "STAKE SYNC: {} rewards.stakes {} -> {} (canonical account.staked)",
                    &staker[..16.min(staker.len())],
                    existing.amount,
                    canonical_amount
                );
                existing.amount = canonical_amount;
            }
        } else if canonical_amount > 0 {
            tracing::info!(
                "STAKE SYNC CREATE: {} creating rewards.stakes entry with amount {} (canonical account.staked, no prior entry)",
                &staker[..16.min(staker.len())],
                canonical_amount
            );
            let position = StakePosition {
                staker: staker.to_string(),
                amount: canonical_amount,
                staked_at: current_time_ms(),
                last_reward_at: None,
            };
            self.stakes.insert(staker.to_string(), position);
        }
    }

    pub fn unstake(&mut self, staker: &str) -> Result<u64, String> {
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

    pub fn get_stake_mut(&mut self, staker: &str) -> Option<&mut StakePosition> {
        self.stakes.get_mut(staker)
    }

    pub fn update_stake(&mut self, staker: &str, new_amount: u64) {
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
        let stake_rewards_total: u64 = rewards
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
        let pending = self.pending_rewards.get(address).copied().unwrap_or(0);
        let lifetime = self.lifetime_rewards.get(address).cloned().unwrap_or_default();

        RewardsSummary {
            address: address.to_string(),
            tip_rewards: lifetime.tip_rewards,
            stake_rewards: lifetime.stake_rewards,
            witness_rewards: lifetime.witness_rewards,
            total_rewards: lifetime.tip_rewards + lifetime.stake_rewards + lifetime.witness_rewards,
            pending_rewards: pending,
        }
    }

    pub fn get_active_validators(&self) -> Vec<&StakePosition> {
        self.stakes
            .values()
            .filter(|s| s.amount >= self.config.min_stake_amount)
            .collect()
    }

    fn get_active_validators_data(&self) -> Vec<(String, u64)> {
        self.stakes
            .values()
            .filter(|s| s.amount >= self.config.min_stake_amount)
            .map(|s| (s.staker.clone(), s.amount))
            .collect()
    }

    pub fn get_total_staked(&self) -> u64 {
        self.stakes.values().map(|s| s.amount).sum()
    }
    
    pub fn get_all_stakes(&self) -> Vec<&StakePosition> {
        self.stakes.values().collect()
    }

    pub fn get_pending_rewards(&self, address: &str) -> u64 {
        self.pending_rewards.get(address).copied().unwrap_or(0)
    }

    pub fn rollback_rewards_above_height(&mut self, target_height: u64, emission: &crate::emission::EmissionService) {
        let last_height = emission.get_last_reward_height();
        if target_height >= last_height {
            return;
        }

        let mut total_to_revert = 0u64;
        for h in (target_height + 1)..=last_height {
            total_to_revert += emission.get_checkpoint_reward(h);
        }

        if total_to_revert == 0 {
            return;
        }

        let eligible: Vec<(String, u64)> = self.stakes
            .values()
            .filter(|s| s.amount >= self.config.min_stake_amount)
            .map(|s| (s.staker.clone(), s.amount))
            .collect();
        let total_stake: u64 = eligible.iter().map(|(_, a)| *a).sum();
        if total_stake == 0 {
            return;
        }

        let mut reverted_count = 0u64;
        for (staker, stake_amount) in &eligible {
            let share = ((total_to_revert as u128 * *stake_amount as u128) / total_stake as u128) as u64;
            if share == 0 {
                continue;
            }

            if let Some(pending) = self.pending_rewards.get_mut(staker) {
                *pending = pending.saturating_sub(share);
            }
            if let Some(lifetime) = self.lifetime_rewards.get_mut(staker) {
                lifetime.stake_rewards = lifetime.stake_rewards.saturating_sub(share);
            }
            reverted_count += 1;
        }

        tracing::info!(
            "Rewards rollback: reverted {} micro-units across {} validators for heights {}..={}",
            total_to_revert, reverted_count, target_height + 1, last_height
        );
    }
    
    pub fn get_claimed_total(&self, address: &str) -> u64 {
        self.claimed_rewards.get(address).copied().unwrap_or(0)
    }

    pub fn claim_rewards(&mut self, address: &str) -> u64 {
        let pending = self.pending_rewards.get(address).copied().unwrap_or(0);
        if pending > 0 {
            self.pending_rewards.insert(address.to_string(), 0);
            let prev_claimed = self.claimed_rewards.get(address).copied().unwrap_or(0);
            self.claimed_rewards.insert(address.to_string(), prev_claimed + pending);
        }
        pending
    }
    
    pub fn sync_from_leader_v3(
        &mut self,
        address: &str,
        pending_rewards: u64,
        staked_at: u64,
        last_reward_at: Option<u64>,
        claimed_total: u64,
        staked_amount: u64,
    ) {
        let addr = address.to_string();
        
        let local_pending = self.pending_rewards.get(&addr).copied().unwrap_or(0);
        if local_pending != pending_rewards {
            tracing::info!(
                "REWARD SYNC for {}: pending_rewards {} -> {}",
                &address[..16.min(address.len())],
                local_pending, pending_rewards
            );
            self.pending_rewards.insert(addr.clone(), pending_rewards);
        }
        
        let local_claimed = self.claimed_rewards.get(&addr).copied().unwrap_or(0);
        if local_claimed != claimed_total {
            tracing::info!(
                "REWARD SYNC for {}: claimed_total {} -> {}",
                &address[..16.min(address.len())],
                local_claimed, claimed_total
            );
            self.claimed_rewards.insert(addr.clone(), claimed_total);
        }
        
        if let Some(stake) = self.stakes.get_mut(&addr) {
            if stake.amount != staked_amount 
                || stake.staked_at != staked_at 
                || stake.last_reward_at != last_reward_at 
            {
                tracing::info!(
                    "REWARD SYNC for {}: stake(amount={}, staked_at={}, last_reward={:?}) -> (amount={}, staked_at={}, last_reward={:?})",
                    &address[..16.min(address.len())],
                    stake.amount, stake.staked_at, stake.last_reward_at,
                    staked_amount, staked_at, last_reward_at
                );
                stake.amount = staked_amount;
                stake.staked_at = staked_at;
                stake.last_reward_at = last_reward_at;
            }
        } else if staked_amount > 0 {
            tracing::info!(
                "REWARD SYNC: Creating stake for {} from leader: amount={}, staked_at={}",
                &address[..16.min(address.len())],
                staked_amount, staked_at
            );
            self.stakes.insert(addr.clone(), StakePosition {
                staker: addr.clone(),
                amount: staked_amount,
                staked_at,
                last_reward_at,
            });
        }
    }
    
    #[allow(dead_code)]
    pub fn sync_stake_from_leader(&mut self, address: &str, staked_amount: u64, balance_increased: bool) {
        if let Some(stake) = self.stakes.get_mut(address) {
            if stake.amount != staked_amount {
                tracing::info!(
                    "RewardsService sync for {}: staked {} -> {}",
                    &address[..16.min(address.len())],
                    stake.amount,
                    staked_amount
                );
                stake.amount = staked_amount;
            }
        } else if staked_amount > 0 {
            let now = current_time_ms();
            tracing::info!(
                "RewardsService: Creating stake from leader for {}: {} micro-RKU",
                &address[..16.min(address.len())],
                staked_amount
            );
            self.stakes.insert(address.to_string(), StakePosition {
                staker: address.to_string(),
                amount: staked_amount,
                staked_at: now,
                last_reward_at: None,
            });
        }
        
        if balance_increased && self.pending_rewards.get(address).copied().unwrap_or(0) > 0 {
            tracing::info!(
                "RewardsService: Resetting pending_rewards for {} (claim processed by leader)",
                &address[..16.min(address.len())]
            );
            self.pending_rewards.insert(address.to_string(), 0);
        }
    }

    pub fn distribute_checkpoint_rewards(&mut self, reward_amount: u64) -> Vec<(String, u64)> {
        let now = current_time_ms();
        let min_stake_age_ms: u64 = 15_000;
        
        let eligible_validators: Vec<(String, u64)> = self.stakes
            .values()
            .filter(|s| {
                s.amount >= self.config.min_stake_amount
                    && now.saturating_sub(s.staked_at) >= min_stake_age_ms
            })
            .map(|s| (s.staker.clone(), s.amount))
            .collect();
        
        let total_eligible_stake: u64 = eligible_validators.iter().map(|(_, amt)| amt).sum();
        if total_eligible_stake == 0 {
            return vec![];
        }

        let mut distributions = Vec::new();
        let now = current_time_ms();

        for (staker, stake_amount) in eligible_validators {
            let reward_amt = ((reward_amount as u128 * stake_amount as u128) / total_eligible_stake as u128) as u64;
            
            let stake_reward = StakeReward {
                recipient: staker.clone(),
                amount: reward_amt,
                tx_url: format!("checkpoint:{}", now),
                timestamp: now,
            };
            self.add_reward(&staker, Reward::Stake(stake_reward));
            distributions.push((staker, reward_amt));
        }

        distributions
    }

    pub fn distribute_fee_to_validators(&mut self, fee_amount: u64) -> Vec<(String, u64)> {
        self.distribute_checkpoint_rewards(fee_amount)
    }

    pub fn get_top_stakers(&self, limit: usize) -> Vec<&StakePosition> {
        let mut stakers: Vec<_> = self.stakes.values().collect();
        stakers.sort_by(|a, b| b.amount.cmp(&a.amount));
        stakers.truncate(limit);
        stakers
    }

    fn add_reward(&mut self, address: &str, reward: Reward) {
        let amount = match &reward {
            Reward::Tip(r) => r.amount,
            Reward::Stake(r) => r.amount,
            Reward::Witness(r) => r.amount,
        };

        let lifetime = self.lifetime_rewards.entry(address.to_string()).or_default();
        match &reward {
            Reward::Tip(_) => lifetime.tip_rewards += amount,
            Reward::Stake(_) => lifetime.stake_rewards += amount,
            Reward::Witness(_) => lifetime.witness_rewards += amount,
        }

        let rewards = self.rewards.entry(address.to_string()).or_default();
        rewards.push(reward);
        if rewards.len() > 100 {
            rewards.drain(0..rewards.len() - 100);
        }

        self.add_pending_reward(address, amount);
    }

    pub fn reverse_reward(&mut self, address: &str, amount: u64) {
        if let Some(pending) = self.pending_rewards.get_mut(address) {
            *pending = pending.saturating_sub(amount);
        }
        if let Some(lifetime) = self.lifetime_rewards.get_mut(address) {
            lifetime.stake_rewards = lifetime.stake_rewards.saturating_sub(amount);
        }
        if let Some(rewards) = self.rewards.get_mut(address) {
            if let Some(pos) = rewards.iter().rposition(|r| {
                matches!(r, Reward::Stake(sr) if sr.amount == amount)
            }) {
                rewards.remove(pos);
            }
        }
    }

    fn add_pending_reward(&mut self, address: &str, amount: u64) {
        let pending = self.pending_rewards.entry(address.to_string()).or_insert(0);
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
        
        let zero_pending: Vec<String> = self.pending_rewards
            .iter()
            .filter(|(_, &v)| v == 0)
            .map(|(k, _)| k.clone())
            .collect();
        for key in zero_pending {
            self.pending_rewards.remove(&key);
            pruned += 1;
        }
        
        let empty_rewards: Vec<String> = self.rewards
            .iter()
            .filter(|(_, v)| v.is_empty())
            .map(|(k, _)| k.clone())
            .collect();
        for key in empty_rewards {
            self.rewards.remove(&key);
            pruned += 1;
        }
        
        if self.lifetime_rewards.len() > MAX_LIFETIME_ENTRIES {
            let mut sorted: Vec<_> = self.lifetime_rewards.iter()
                .map(|(k, v)| (k.clone(), v.tip_rewards + v.stake_rewards + v.witness_rewards))
                .collect();
            sorted.sort_by(|a, b| b.1.cmp(&a.1));
            let keep: std::collections::HashSet<String> = sorted.into_iter()
                .take(MAX_LIFETIME_ENTRIES)
                .map(|(k, _)| k)
                .collect();
            let to_remove: Vec<String> = self.lifetime_rewards.keys()
                .filter(|k| !keep.contains(*k))
                .cloned()
                .collect();
            for key in &to_remove {
                self.lifetime_rewards.remove(key);
            }
            pruned += to_remove.len();
        }
        
        if self.pending_rewards.len() > MAX_PENDING_ENTRIES {
            let mut sorted: Vec<_> = self.pending_rewards.iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            sorted.sort_by(|a, b| b.1.cmp(&a.1));
            let keep: std::collections::HashSet<String> = sorted.into_iter()
                .take(MAX_PENDING_ENTRIES)
                .map(|(k, _)| k)
                .collect();
            let to_remove: Vec<String> = self.pending_rewards.keys()
                .filter(|k| !keep.contains(*k))
                .cloned()
                .collect();
            for key in &to_remove {
                self.pending_rewards.remove(key);
            }
            pruned += to_remove.len();
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
            lifetime_rewards: self.lifetime_rewards.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            claimed_rewards: self.claimed_rewards.iter().map(|(k, v)| (k.clone(), *v)).collect(),
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

        for (addr, lifetime) in snapshot.lifetime_rewards {
            service.lifetime_rewards.insert(addr, lifetime);
        }

        for (addr, claimed) in snapshot.claimed_rewards {
            service.claimed_rewards.insert(addr, claimed);
        }

        service.witnessed_queue = snapshot.witnessed_queue;
        service.witnessed_queue_head = 0;

        service
    }

    pub fn merge_from(&mut self, snapshot: RewardsSnapshot) {
        use tracing::info;
        
        let mut lifetime_raised = 0;
        let mut claimed_raised = 0;
        let mut pending_recomputed = 0;
        
        for (addr, peer_lifetime) in &snapshot.lifetime_rewards {
            let local_lifetime = self.lifetime_rewards.get(addr).cloned().unwrap_or_default();
            let peer_total = peer_lifetime.tip_rewards + peer_lifetime.stake_rewards + peer_lifetime.witness_rewards;
            let local_total = local_lifetime.tip_rewards + local_lifetime.stake_rewards + local_lifetime.witness_rewards;
            if peer_total > local_total {
                self.lifetime_rewards.insert(addr.clone(), peer_lifetime.clone());
                lifetime_raised += 1;
            }
        }
        
        for (addr, peer_claimed) in &snapshot.claimed_rewards {
            let local_claimed = self.claimed_rewards.get(addr).copied().unwrap_or(0);
            if *peer_claimed > local_claimed {
                self.claimed_rewards.insert(addr.clone(), *peer_claimed);
                claimed_raised += 1;
            }
        }
        
        let all_addresses: std::collections::HashSet<String> = self.lifetime_rewards.keys()
            .chain(self.claimed_rewards.keys())
            .chain(self.pending_rewards.keys())
            .cloned()
            .collect();
        
        for addr in &all_addresses {
            let lifetime = self.lifetime_rewards.get(addr).cloned().unwrap_or_default();
            let lifetime_total = lifetime.tip_rewards + lifetime.stake_rewards + lifetime.witness_rewards;
            let claimed = self.claimed_rewards.get(addr).copied().unwrap_or(0);
            let correct_pending = lifetime_total.saturating_sub(claimed);
            
            let current_pending = self.pending_rewards.get(addr).copied().unwrap_or(0);
            if correct_pending != current_pending {
                self.pending_rewards.insert(addr.clone(), correct_pending);
                pending_recomputed += 1;
            }
        }
        
        for (addr, peer_rewards) in snapshot.rewards {
            self.rewards.entry(addr).or_insert(peer_rewards);
        }
        
        for (addr, peer_stake) in snapshot.stakes {
            self.stakes.entry(addr).or_insert(peer_stake);
        }
        
        for (key, peer_ts) in snapshot.witnessed_txs {
            let local_ts = self.witnessed_txs.get(&key).copied().unwrap_or(0);
            if peer_ts > local_ts {
                self.witnessed_txs.insert(key, peer_ts);
            }
        }
        
        info!(
            "Merged rewards snapshot: {} lifetime raised, {} claimed raised, {} pending recomputed",
            lifetime_raised, claimed_raised, pending_recomputed
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardsStats {
    #[serde(with = "micro_serde")]
    pub total_staked: u64,
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
    use rinku_core::types::to_micro_units;

    #[test]
    fn test_stake_and_unstake() {
        let mut service = RewardsService::new(RewardConfig::default());

        let result = service.stake("validator1", to_micro_units(1000.0), "test_tx_hash_1");
        assert!(result.is_ok());

        let stake = service.get_stake("validator1");
        assert!(stake.is_some());
        assert_eq!(stake.unwrap().amount, to_micro_units(1000.0));

        let unstake_result = service.unstake("validator1");
        assert!(unstake_result.is_err());
    }

    #[test]
    fn test_tip_rewards() {
        let mut service = RewardsService::new(RewardConfig::default());

        let reward = service.process_tip_reward("tx1", "tip1", "recipient1", to_micro_units(100.0));
        assert!(reward.is_some());
        assert_eq!(reward.unwrap().amount, to_micro_units(1.0));
    }

    #[test]
    fn test_witness_dedup() {
        let mut service = RewardsService::new(RewardConfig::default());

        let first = service.process_witness_reward("tx1", "ref1", "recipient1", to_micro_units(100.0));
        assert!(first.is_some());

        let second = service.process_witness_reward("tx1", "ref1", "recipient1", to_micro_units(100.0));
        assert!(second.is_none());
    }
}
