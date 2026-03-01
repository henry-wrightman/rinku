pub use rinku_core::types::GasConfig;
use rinku_core::types::{MICRO_UNITS, from_micro_units, to_micro_units};
use std::collections::VecDeque;

pub struct GasService {
    config: GasConfig,
    current_price: u64,
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

    pub fn current_price(&self) -> u64 {
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

        let price_f64 = from_micro_units(self.current_price);
        let new_price_f64 = if recent_tx_count > target {
            price_f64 * (1.0 + adjustment)
        } else if recent_tx_count < target {
            price_f64 * (1.0 - adjustment)
        } else {
            price_f64
        };

        let new_price = to_micro_units(new_price_f64);
        self.current_price = new_price
            .max(self.config.min_gas_price)
            .min(self.config.max_gas_price);
    }

    pub fn calculate_fee(&self, gas_limit: u64) -> u64 {
        (self.current_price as u128 * gas_limit as u128 / MICRO_UNITS as u128) as u64
    }

    pub fn calculate_burn_amount(&self, fee: u64, supply_ratio: f64) -> u64 {
        let min_price_ratio = from_micro_units(self.config.min_gas_price);
        let burn_ratio = (1.0 - min_price_ratio) * supply_ratio;
        let capped_burn = burn_ratio.min(0.3);
        (fee as f64 * capped_burn).round() as u64
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
            min_gas_price: to_micro_units(1.0),
            ..Default::default()
        });

        service.record_transaction(1000);
        service.update_price();

        assert!(service.current_price() <= to_micro_units(1.0));
    }
}
