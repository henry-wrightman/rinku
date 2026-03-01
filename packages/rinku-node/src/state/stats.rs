use super::*;

const TPS_RING_SIZE: usize = 300;
const TPS_SHORT_WINDOW_SECS: u64 = 10;
const TPS_LONG_WINDOW_SECS: u64 = 60;

impl NodeState {
    pub async fn get_gas_price(&self) -> u64 {
        let state = self.inner.read().await;
        state.current_gas_price
    }

    pub async fn get_gas_stats(&self) -> (u64, u64, u64, u64) {
        let state = self.inner.read().await;
        (
            state.current_gas_price,
            state.total_burned,
            state.total_to_validators,
            state.current_gas_price,
        )
    }

    pub async fn get_emission_stats(&self) -> (u64, u64) {
        let emission = self.emission.read().await;
        (emission.get_total_emitted(), emission.get_total_burned())
    }

    pub async fn get_total_supply(&self) -> u64 {
        let state = self.inner.read().await;
        state.total_supply
    }

    pub async fn get_validator_count(&self) -> usize {
        let state = self.inner.read().await;
        state.validators.len()
    }

    pub async fn get_total_stake(&self) -> u64 {
        let state = self.inner.read().await;
        state.validators.values().map(|v| v.stake).sum()
    }

    pub async fn get_faucet_balance(&self) -> u64 {
        let state = self.inner.read().await;
        state.accounts.get("faucet").map(|a| a.balance).unwrap_or(0)
    }

    pub async fn get_validator_staking_info(&self, address: &str) -> (u64, u64, u64, bool) {
        let rewards = self.rewards.read().await;
        let stake_amount = rewards.get_stake(address).map(|p| p.amount).unwrap_or(0);
        let pending_rewards = rewards.get_pending_rewards(address);
        
        let state = self.inner.read().await;
        let is_validator = state.validators.contains_key(address);
        
        let unbonding = 0;
        
        (stake_amount, pending_rewards, unbonding, is_validator)
    }

    pub async fn get_staking_config(&self) -> (u64, u32) {
        let rewards = self.rewards.read().await;
        let min_stake = rewards.get_config().min_stake_amount;
        let unbonding_days = (crate::slashing::UNBONDING_PERIOD_MS / (24 * 60 * 60 * 1000)) as u32;
        (min_stake, unbonding_days)
    }

    pub async fn get_total_transactions(&self) -> u64 {
        let state = self.inner.read().await;
        state.total_transactions
    }

    pub fn get_elapsed_seconds(&self) -> f64 {
        self.start_time.elapsed().as_secs_f64()
    }

    pub async fn record_tx_timestamp(&self) {
        self.record_tx_timestamps(1).await;
    }

    pub async fn record_tx_timestamps(&self, count: u64) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut state = self.inner.write().await;
        state.finalized_tx_history.push_back((now_ms, count));

        while state.finalized_tx_history.len() > TPS_RING_SIZE {
            state.finalized_tx_history.pop_front();
        }
    }

    pub async fn record_finalized_batch(&self, tx_count: u64) {
        self.record_tx_timestamps(tx_count).await;
    }

    pub async fn get_dynamic_tps(&self) -> (f64, f64, f64) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let state = self.inner.read().await;

        if state.finalized_tx_history.is_empty() {
            return (0.0, 0.0, 0.0);
        }

        let short_cutoff = now_ms.saturating_sub(TPS_SHORT_WINDOW_SECS * 1000);
        let long_cutoff = now_ms.saturating_sub(TPS_LONG_WINDOW_SECS * 1000);

        let mut short_txs: u64 = 0;
        let mut long_txs: u64 = 0;
        let mut short_earliest = now_ms;
        let mut long_earliest = now_ms;

        for &(ts, count) in state.finalized_tx_history.iter() {
            if ts >= long_cutoff {
                long_txs += count;
                if ts < long_earliest {
                    long_earliest = ts;
                }
            }
            if ts >= short_cutoff {
                short_txs += count;
                if ts < short_earliest {
                    short_earliest = ts;
                }
            }
        }

        let short_elapsed = now_ms.saturating_sub(short_earliest);
        let short_tps = if short_elapsed > 500 && short_txs > 0 {
            (short_txs as f64) / (short_elapsed as f64 / 1000.0)
        } else if short_txs > 0 {
            short_txs as f64
        } else {
            0.0
        };

        let long_elapsed = now_ms.saturating_sub(long_earliest);
        let long_tps = if long_elapsed > 1000 && long_txs > 0 {
            (long_txs as f64) / (long_elapsed as f64 / 1000.0)
        } else if long_txs > 0 {
            long_txs as f64
        } else {
            0.0
        };

        let peak = if short_tps > long_tps { short_tps } else { long_tps };

        (peak, short_tps, long_tps)
    }

    pub async fn get_finalized_tps(&self) -> f64 {
        let (peak, _, _) = self.get_dynamic_tps().await;
        peak
    }
}
