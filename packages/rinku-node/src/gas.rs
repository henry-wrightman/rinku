pub use rinku_core::types::GasConfig;
use std::collections::VecDeque;

pub struct GasService {
    config: GasConfig,
    current_price: f64,
    tx_counts: VecDeque<(u64, u32)>,
}

impl GasService {
    pub fn new(config: GasConfig) -> Self {
        Self {
            current_price: config.min_gas_price,
            config,
            tx_counts: VecDeque::new(),
        }
    }

    pub fn current_price(&self) -> f64 {
        self.current_price
    }

    pub fn record_transaction(&mut self, timestamp: u64) {
        let period = timestamp / (self.config.period_duration_ms / 1000);

        if let Some((last_period, count)) = self.tx_counts.back_mut() {
            if *last_period == period {
                *count += 1;
                return;
            }
        }

        self.tx_counts.push_back((period, 1));

        while self.tx_counts.len() > 10 {
            self.tx_counts.pop_front();
        }
    }

    pub fn update_price(&mut self) {
        let recent_tx_count: u32 = self
            .tx_counts
            .iter()
            .rev()
            .take(1)
            .map(|(_, c)| *c)
            .sum();

        let target = self.config.target_txs_per_period;
        let adjustment = self.config.adjustment_factor;

        let new_price = if recent_tx_count > target {
            self.current_price * (1.0 + adjustment)
        } else if recent_tx_count < target {
            self.current_price * (1.0 - adjustment)
        } else {
            self.current_price
        };

        self.current_price = new_price
            .max(self.config.min_gas_price)
            .min(self.config.max_gas_price);
    }

    pub fn calculate_fee(&self, gas_limit: u64) -> f64 {
        self.current_price * gas_limit as f64
    }

    pub fn calculate_burn_amount(&self, fee: f64, supply_ratio: f64) -> f64 {
        let burn_ratio = (1.0 - self.config.min_gas_price) * supply_ratio;
        let capped_burn = burn_ratio.min(0.3);
        fee * capped_burn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gas_price_increase() {
        let config = GasConfig {
            target_txs_per_period: 5,
            ..Default::default()
        };
        let mut service = GasService::new(config);
        let initial = service.current_price();

        for _ in 0..20 {
            service.record_transaction(1000);
        }
        service.update_price();

        assert!(service.current_price() > initial);
    }

    #[test]
    fn test_gas_price_decrease() {
        let mut service = GasService::new(GasConfig {
            min_gas_price: 1.0,
            ..Default::default()
        });

        service.record_transaction(1000);
        service.update_price();

        assert!(service.current_price() <= 1.0);
    }
}
