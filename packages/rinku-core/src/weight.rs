use crate::types::Account;

const MIN_BOND_FOR_AGE_WEIGHT: f64 = 100.0;
const AGE_WEIGHT_DECAY_PER_MISSED: f64 = 0.10;
const MAX_AGE_WEIGHT: f64 = 10.0;
const BALANCE_WEIGHT_EXPONENT: f64 = 0.5;

pub fn calculate_account_weight(account: &Account, current_time: u64) -> f64 {
    let age_weight = calculate_age_weight(account, current_time);
    let balance_weight = calculate_balance_weight(account.balance);
    let stake_weight = calculate_stake_weight(account.staked);

    age_weight * balance_weight + stake_weight
}

pub fn calculate_age_weight(account: &Account, current_time: u64) -> f64 {
    if account.staked < MIN_BOND_FOR_AGE_WEIGHT {
        return 1.0;
    }

    let age_seconds = current_time.saturating_sub(account.first_seen);
    let age_days = age_seconds as f64 / 86400.0;

    (1.0 + age_days.ln_1p()).min(MAX_AGE_WEIGHT)
}

pub fn calculate_balance_weight(balance: f64) -> f64 {
    if balance <= 0.0 {
        return 0.0;
    }
    balance.powf(BALANCE_WEIGHT_EXPONENT)
}

pub fn calculate_stake_weight(stake: f64) -> f64 {
    if stake <= 0.0 {
        return 0.0;
    }
    stake.powf(BALANCE_WEIGHT_EXPONENT) * 2.0
}

pub fn calculate_validator_weight(
    stake: f64,
    first_stake_time: u64,
    current_time: u64,
    missed_checkpoints: u32,
) -> f64 {
    if stake < MIN_BOND_FOR_AGE_WEIGHT {
        return stake.powf(BALANCE_WEIGHT_EXPONENT);
    }

    let age_seconds = current_time.saturating_sub(first_stake_time);
    let age_days = age_seconds as f64 / 86400.0;
    let age_factor = (1.0 + age_days.ln_1p()).min(MAX_AGE_WEIGHT);

    let decay = (1.0 - AGE_WEIGHT_DECAY_PER_MISSED).powi(missed_checkpoints as i32);
    let decayed_age_factor = age_factor * decay;

    stake.powf(BALANCE_WEIGHT_EXPONENT) * decayed_age_factor
}

pub fn calculate_transaction_weight(
    sender_weight: f64,
    gas_paid: f64,
    is_consolidation: bool,
) -> f64 {
    if is_consolidation {
        return sender_weight * 0.1;
    }

    sender_weight + (gas_paid * 10.0)
}

pub fn total_validator_weight(validators: &[(f64, u64, u32)], current_time: u64) -> f64 {
    validators
        .iter()
        .map(|(stake, first_stake, missed)| {
            calculate_validator_weight(*stake, *first_stake, current_time, *missed)
        })
        .sum()
}

pub fn required_weight_for_finality(total_weight: f64) -> f64 {
    total_weight * 2.0 / 3.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_balance_weight() {
        assert_eq!(calculate_balance_weight(0.0), 0.0);
        assert_eq!(calculate_balance_weight(100.0), 10.0);
        assert_eq!(calculate_balance_weight(10000.0), 100.0);
    }

    #[test]
    fn test_stake_weight() {
        assert_eq!(calculate_stake_weight(0.0), 0.0);
        let stake_weight = calculate_stake_weight(100.0);
        assert!(stake_weight > calculate_balance_weight(100.0));
    }

    #[test]
    fn test_age_weight_requires_min_bond() {
        let account = Account {
            address: "test".to_string(),
            balance: 1000.0,
            nonce: 0,
            first_seen: 0,
            staked: 50.0,
            unbonding: 0.0,
            unbonding_release: None,
            latest_balance_proof: None,
        };

        let weight = calculate_age_weight(&account, 86400 * 30);
        assert_eq!(weight, 1.0);
    }

    #[test]
    fn test_age_weight_with_stake() {
        let account = Account {
            address: "test".to_string(),
            balance: 1000.0,
            nonce: 0,
            first_seen: 0,
            staked: 100.0,
            unbonding: 0.0,
            unbonding_release: None,
            latest_balance_proof: None,
        };

        let weight_30_days = calculate_age_weight(&account, 86400 * 30);
        assert!(weight_30_days > 1.0);
        assert!(weight_30_days <= MAX_AGE_WEIGHT);
    }

    #[test]
    fn test_validator_weight_decay() {
        let stake = 1000.0;
        let first_stake = 0;
        let current = 86400 * 30;

        let weight_0_missed = calculate_validator_weight(stake, first_stake, current, 0);
        let weight_5_missed = calculate_validator_weight(stake, first_stake, current, 5);

        assert!(weight_5_missed < weight_0_missed);
    }

    #[test]
    fn test_finality_threshold() {
        let total = 1000.0;
        let required = required_weight_for_finality(total);
        assert!((required - 666.67).abs() < 1.0);
    }
}
