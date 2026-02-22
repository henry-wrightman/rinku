use super::*;

impl NodeState {
    pub async fn get_gas_price(&self) -> f64 {
        let state = self.inner.read().await;
        state.current_gas_price
    }

    pub async fn get_gas_stats(&self) -> (f64, f64, f64, f64) {
        let state = self.inner.read().await;
        (
            state.current_gas_price,
            state.total_burned,
            state.total_to_validators,
            state.current_gas_price,
        )
    }

    /// Get emission stats (total_emitted, total_burned) from the emission service
    pub async fn get_emission_stats(&self) -> (f64, f64) {
        let emission = self.emission.read().await;
        (emission.get_total_emitted(), emission.get_total_burned())
    }

    pub async fn get_total_supply(&self) -> f64 {
        let state = self.inner.read().await;
        state.total_supply
    }

    pub async fn get_validator_count(&self) -> usize {
        let state = self.inner.read().await;
        state.validators.len()
    }

    pub async fn get_total_stake(&self) -> f64 {
        let state = self.inner.read().await;
        state.validators.values().map(|v| v.stake).sum()
    }

    pub async fn get_faucet_balance(&self) -> f64 {
        let state = self.inner.read().await;
        state.accounts.get("faucet").map(|a| a.balance).unwrap_or(0.0)
    }

    /// Get staking info for a specific validator address (for TUI display)
    pub async fn get_validator_staking_info(&self, address: &str) -> (f64, f64, f64, bool) {
        let rewards = self.rewards.read().await;
        let stake_amount = rewards.get_stake(address).map(|p| p.amount).unwrap_or(0.0);
        let pending_rewards = rewards.get_pending_rewards(address);
        
        let state = self.inner.read().await;
        let is_validator = state.validators.contains_key(address);
        
        // Unbonding amount - check if in unbonding queue
        let unbonding = 0.0; // TODO: Track unbonding separately if needed
        
        (stake_amount, pending_rewards, unbonding, is_validator)
    }

    /// Get staking configuration for display (min stake, unbonding period)
    pub async fn get_staking_config(&self) -> (f64, u32) {
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

    /// Record finalized transaction count at current timestamp for TPS calculation
    pub async fn record_finalized_batch(&self, tx_count: u64) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        
        let mut state = self.inner.write().await;
        state.finalized_tx_history.push_back((now_ms, tx_count));
        
        // Keep only last 5 minutes of history (300 seconds)
        const WINDOW_MS: u64 = 300_000;
        let cutoff = now_ms.saturating_sub(WINDOW_MS);
        while let Some(&(ts, _)) = state.finalized_tx_history.front() {
            if ts < cutoff {
                state.finalized_tx_history.pop_front();
            } else {
                break;
            }
        }
    }

    /// Calculate network TPS based on finalized transactions over a sliding window
    pub async fn get_finalized_tps(&self) -> f64 {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        
        let state = self.inner.read().await;
        
        if state.finalized_tx_history.is_empty() {
            return 0.0;
        }
        
        // Calculate TPS over the last 60 seconds
        const TPS_WINDOW_MS: u64 = 60_000;
        let cutoff = now_ms.saturating_sub(TPS_WINDOW_MS);
        
        let mut total_txs: u64 = 0;
        let mut earliest_ts = now_ms;
        
        for &(ts, count) in state.finalized_tx_history.iter() {
            if ts >= cutoff {
                total_txs += count;
                if ts < earliest_ts {
                    earliest_ts = ts;
                }
            }
        }
        
        let elapsed_ms = now_ms.saturating_sub(earliest_ts);
        if elapsed_ms > 0 && total_txs > 0 {
            (total_txs as f64) / (elapsed_ms as f64 / 1000.0)
        } else {
            0.0
        }
    }
}
