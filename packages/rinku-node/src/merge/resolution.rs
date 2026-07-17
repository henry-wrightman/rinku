use super::{
    ConflictResolution, ConflictType, DirectConflict, EconomicConflict, PartitionTxSummary,
    ResolutionReason,
};
use rinku_core::types::Account;
use std::collections::HashMap;

const WEIGHT_PROXIMITY_THRESHOLD: f64 = 1.5;

pub fn resolve_direct_conflicts(
    conflicts: &[DirectConflict],
    local_txs: &[PartitionTxSummary],
    remote_txs: &[PartitionTxSummary],
) -> Vec<ConflictResolution> {
    let local_map: HashMap<&str, &PartitionTxSummary> = local_txs
        .iter()
        .map(|tx| (tx.tx_hash.as_str(), tx))
        .collect();
    let remote_map: HashMap<&str, &PartitionTxSummary> = remote_txs
        .iter()
        .map(|tx| (tx.tx_hash.as_str(), tx))
        .collect();

    conflicts
        .iter()
        .map(|conflict| {
            let local_tx = local_map.get(conflict.local_tx_hash.as_str());
            let remote_tx = remote_map.get(conflict.remote_tx_hash.as_str());

            let local_weight = local_tx.map(|t| t.weight).unwrap_or(0.0);
            let remote_weight = remote_tx.map(|t| t.weight).unwrap_or(0.0);

            let (winner_hash, loser_hash, winner_w, loser_w, reason) = {
                let weights_differ = (local_weight - remote_weight).abs() > f64::EPSILON;

                if weights_differ {
                    let (higher_w, lower_w) = if local_weight > remote_weight {
                        (local_weight, remote_weight)
                    } else {
                        (remote_weight, local_weight)
                    };

                    let within_threshold =
                        lower_w > 0.0 && higher_w / lower_w <= WEIGHT_PROXIMITY_THRESHOLD;

                    if within_threshold {
                        let local_stake = local_tx.map(|t| t.visible_stake_pct).unwrap_or(0.0);
                        let remote_stake = remote_tx.map(|t| t.visible_stake_pct).unwrap_or(0.0);

                        if (local_stake - remote_stake).abs() > f64::EPSILON {
                            if local_stake > remote_stake {
                                (
                                    &conflict.local_tx_hash,
                                    &conflict.remote_tx_hash,
                                    local_weight,
                                    remote_weight,
                                    ResolutionReason::HigherStake,
                                )
                            } else {
                                (
                                    &conflict.remote_tx_hash,
                                    &conflict.local_tx_hash,
                                    remote_weight,
                                    local_weight,
                                    ResolutionReason::HigherStake,
                                )
                            }
                        } else if conflict.local_tx_hash < conflict.remote_tx_hash {
                            (
                                &conflict.local_tx_hash,
                                &conflict.remote_tx_hash,
                                local_weight,
                                remote_weight,
                                ResolutionReason::LowerHashTiebreak,
                            )
                        } else {
                            (
                                &conflict.remote_tx_hash,
                                &conflict.local_tx_hash,
                                remote_weight,
                                local_weight,
                                ResolutionReason::LowerHashTiebreak,
                            )
                        }
                    } else if local_weight > remote_weight {
                        (
                            &conflict.local_tx_hash,
                            &conflict.remote_tx_hash,
                            local_weight,
                            remote_weight,
                            ResolutionReason::HigherWeight,
                        )
                    } else {
                        (
                            &conflict.remote_tx_hash,
                            &conflict.local_tx_hash,
                            remote_weight,
                            local_weight,
                            ResolutionReason::HigherWeight,
                        )
                    }
                } else if conflict.local_tx_hash < conflict.remote_tx_hash {
                    (
                        &conflict.local_tx_hash,
                        &conflict.remote_tx_hash,
                        local_weight,
                        remote_weight,
                        ResolutionReason::LowerHashTiebreak,
                    )
                } else {
                    (
                        &conflict.remote_tx_hash,
                        &conflict.local_tx_hash,
                        remote_weight,
                        local_weight,
                        ResolutionReason::LowerHashTiebreak,
                    )
                }
            };

            ConflictResolution {
                conflict_type: ConflictType::DirectDoubleSpend,
                account: conflict.account.clone(),
                winner_tx_hashes: vec![winner_hash.clone()],
                loser_tx_hashes: vec![loser_hash.clone()],
                reason,
                winner_weight: winner_w,
                loser_weight: loser_w,
            }
        })
        .collect()
}

pub fn resolve_economic_conflicts(
    conflicts: &[EconomicConflict],
    local_txs: &[PartitionTxSummary],
    remote_txs: &[PartitionTxSummary],
    fork_point_accounts: &HashMap<String, Account>,
) -> Vec<ConflictResolution> {
    conflicts
        .iter()
        .map(|conflict| {
            let pre_balance_micro = fork_point_accounts
                .get(&conflict.account)
                .map(|a| a.balance)
                .unwrap_or(conflict.pre_partition_balance_micro);

            let mut account_txs: Vec<&PartitionTxSummary> = local_txs
                .iter()
                .chain(remote_txs.iter())
                .filter(|tx| tx.from == conflict.account)
                .collect();

            account_txs.sort_by(|a, b| {
                a.nonce
                    .cmp(&b.nonce)
                    .then_with(|| {
                        b.weight
                            .partial_cmp(&a.weight)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| a.tx_hash.cmp(&b.tx_hash))
            });

            let mut balance = pre_balance_micro;
            let mut received: u64 = 0;
            for tx in local_txs.iter().chain(remote_txs.iter()) {
                if tx.to == conflict.account && tx.from != conflict.account {
                    received = received.saturating_add(tx.amount_micro);
                }
            }
            balance = balance.saturating_add(received);

            let mut winners: Vec<String> = Vec::new();
            let mut losers: Vec<String> = Vec::new();
            let mut overdraft_hit = false;

            for tx in &account_txs {
                if overdraft_hit {
                    losers.push(tx.tx_hash.clone());
                    continue;
                }

                let total_cost = tx.amount_micro.saturating_add(tx.gas_micro);
                if balance < total_cost {
                    overdraft_hit = true;
                    losers.push(tx.tx_hash.clone());
                } else {
                    balance = balance.saturating_sub(total_cost);
                    winners.push(tx.tx_hash.clone());
                }
            }

            let winner_weight: f64 = account_txs
                .iter()
                .filter(|tx| winners.contains(&tx.tx_hash))
                .map(|tx| tx.weight)
                .sum();
            let loser_weight: f64 = account_txs
                .iter()
                .filter(|tx| losers.contains(&tx.tx_hash))
                .map(|tx| tx.weight)
                .sum();

            let reason = if losers.is_empty() {
                ResolutionReason::HigherWeight
            } else {
                ResolutionReason::HigherWeight
            };

            ConflictResolution {
                conflict_type: ConflictType::EconomicOverdraft,
                account: conflict.account.clone(),
                winner_tx_hashes: winners,
                loser_tx_hashes: losers,
                reason,
                winner_weight,
                loser_weight,
            }
        })
        .collect()
}

pub fn resolve_all_conflicts(
    direct_conflicts: &[DirectConflict],
    economic_conflicts: &[EconomicConflict],
    local_txs: &[PartitionTxSummary],
    remote_txs: &[PartitionTxSummary],
    fork_point_accounts: &HashMap<String, Account>,
) -> Vec<ConflictResolution> {
    let mut resolutions = resolve_direct_conflicts(direct_conflicts, local_txs, remote_txs);
    resolutions.extend(resolve_economic_conflicts(
        economic_conflicts,
        local_txs,
        remote_txs,
        fork_point_accounts,
    ));
    resolutions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::PartitionTxSummary;
    use rinku_core::types::to_micro_units;

    fn make_tx(
        hash: &str,
        from: &str,
        to: &str,
        amount: f64,
        nonce: u64,
        weight: f64,
    ) -> PartitionTxSummary {
        PartitionTxSummary {
            tx_hash: hash.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            amount_micro: to_micro_units(amount),
            gas_micro: to_micro_units(0.001),
            nonce,
            weight,
            dag_depth: 0,
            parents: vec![],
            partition_epoch: Some(1),
            visible_stake_pct: 0.5,
        }
    }

    fn make_tx_with_stake(
        hash: &str,
        from: &str,
        to: &str,
        amount: f64,
        nonce: u64,
        weight: f64,
        stake_pct: f64,
    ) -> PartitionTxSummary {
        PartitionTxSummary {
            tx_hash: hash.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            amount_micro: to_micro_units(amount),
            gas_micro: to_micro_units(0.001),
            nonce,
            weight,
            dag_depth: 0,
            parents: vec![],
            partition_epoch: Some(1),
            visible_stake_pct: stake_pct,
        }
    }

    fn make_account(addr: &str, balance: f64) -> Account {
        Account {
            address: addr.to_string(),
            balance: to_micro_units(balance),
            nonce: 5,
            first_seen: 0,
            staked: 0,
            unbonding: 0,
            unbonding_release: None,
            latest_balance_proof: None,
            partition_violations: 0,
            reputation_penalty: 0.0,
            penalty_decay_checkpoint: None,
            partition_budget: None,
            partition_budget_spent: 0,
            ecdsa_public_key: None,
        }
    }

    #[test]
    fn test_higher_weight_wins_direct_beyond_threshold() {
        let conflict = DirectConflict {
            account: "alice".to_string(),
            nonce: 5,
            local_tx_hash: "tx_local".to_string(),
            remote_tx_hash: "tx_remote".to_string(),
            local_partition_epoch: Some(1),
            remote_partition_epoch: Some(1),
        };

        let local = vec![make_tx("tx_local", "alice", "bob", 10.0, 5, 5.0)];
        let remote = vec![make_tx("tx_remote", "alice", "carol", 10.0, 5, 3.0)];

        let resolutions = resolve_direct_conflicts(&[conflict], &local, &remote);
        assert_eq!(resolutions.len(), 1);
        assert_eq!(resolutions[0].winner_tx_hashes, vec!["tx_local"]);
        assert_eq!(resolutions[0].loser_tx_hashes, vec!["tx_remote"]);
        assert_eq!(resolutions[0].reason, ResolutionReason::HigherWeight);
    }

    #[test]
    fn test_stake_tiebreak_within_threshold() {
        let conflict = DirectConflict {
            account: "alice".to_string(),
            nonce: 5,
            local_tx_hash: "tx_local".to_string(),
            remote_tx_hash: "tx_remote".to_string(),
            local_partition_epoch: Some(1),
            remote_partition_epoch: Some(1),
        };

        let local = vec![make_tx_with_stake(
            "tx_local", "alice", "bob", 10.0, 5, 4.0, 0.7,
        )];
        let remote = vec![make_tx_with_stake(
            "tx_remote",
            "alice",
            "carol",
            10.0,
            5,
            3.0,
            0.4,
        )];

        let resolutions = resolve_direct_conflicts(&[conflict], &local, &remote);
        assert_eq!(resolutions.len(), 1);
        assert_eq!(resolutions[0].winner_tx_hashes, vec!["tx_local"]);
        assert_eq!(resolutions[0].reason, ResolutionReason::HigherStake);
    }

    #[test]
    fn test_hash_tiebreak_when_equal_weight() {
        let conflict = DirectConflict {
            account: "alice".to_string(),
            nonce: 5,
            local_tx_hash: "tx_zzz".to_string(),
            remote_tx_hash: "tx_aaa".to_string(),
            local_partition_epoch: Some(1),
            remote_partition_epoch: Some(1),
        };

        let local = vec![make_tx("tx_zzz", "alice", "bob", 10.0, 5, 1.0)];
        let remote = vec![make_tx("tx_aaa", "alice", "carol", 10.0, 5, 1.0)];

        let resolutions = resolve_direct_conflicts(&[conflict], &local, &remote);
        assert_eq!(resolutions[0].winner_tx_hashes, vec!["tx_aaa"]);
        assert_eq!(resolutions[0].reason, ResolutionReason::LowerHashTiebreak);
    }

    #[test]
    fn test_economic_nonce_ordered_replay() {
        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), make_account("alice", 100.0));

        let conflict = EconomicConflict {
            account: "alice".to_string(),
            pre_partition_balance_micro: to_micro_units(100.0),
            total_sent_local_micro: to_micro_units(60.0),
            total_sent_remote_micro: to_micro_units(60.0),
            total_received_local_micro: 0,
            total_received_remote_micro: 0,
            deficit_micro: to_micro_units(20.0),
            local_tx_hashes: vec!["tx_l1".to_string()],
            remote_tx_hashes: vec!["tx_r1".to_string()],
        };

        let local = vec![make_tx("tx_l1", "alice", "bob", 60.0, 5, 5.0)];
        let remote = vec![make_tx("tx_r1", "alice", "dave", 60.0, 6, 2.0)];

        let resolutions = resolve_economic_conflicts(&[conflict], &local, &remote, &accounts);
        assert_eq!(resolutions.len(), 1);
        assert_eq!(resolutions[0].winner_tx_hashes, vec!["tx_l1"]);
        assert_eq!(resolutions[0].loser_tx_hashes, vec!["tx_r1"]);
    }

    #[test]
    fn test_economic_higher_weight_survives_at_same_nonce() {
        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), make_account("alice", 100.0));

        let conflict = EconomicConflict {
            account: "alice".to_string(),
            pre_partition_balance_micro: to_micro_units(100.0),
            total_sent_local_micro: to_micro_units(80.0),
            total_sent_remote_micro: to_micro_units(80.0),
            total_received_local_micro: 0,
            total_received_remote_micro: 0,
            deficit_micro: to_micro_units(60.0),
            local_tx_hashes: vec!["tx_l1".to_string()],
            remote_tx_hashes: vec!["tx_r1".to_string()],
        };

        let local = vec![make_tx("tx_l1", "alice", "bob", 80.0, 5, 3.0)];
        let remote = vec![make_tx("tx_r1", "alice", "carol", 80.0, 5, 7.0)];

        let resolutions = resolve_economic_conflicts(&[conflict], &local, &remote, &accounts);
        assert_eq!(resolutions.len(), 1);
        assert_eq!(resolutions[0].winner_tx_hashes, vec!["tx_r1"]);
        assert_eq!(resolutions[0].loser_tx_hashes, vec!["tx_l1"]);
    }

    #[test]
    fn test_economic_cascade_after_overdraft() {
        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), make_account("alice", 100.0));

        let conflict = EconomicConflict {
            account: "alice".to_string(),
            pre_partition_balance_micro: to_micro_units(100.0),
            total_sent_local_micro: to_micro_units(70.0),
            total_sent_remote_micro: to_micro_units(70.0),
            total_received_local_micro: 0,
            total_received_remote_micro: 0,
            deficit_micro: to_micro_units(40.0),
            local_tx_hashes: vec!["tx_l1".to_string(), "tx_l2".to_string()],
            remote_tx_hashes: vec!["tx_r1".to_string()],
        };

        let local = vec![
            make_tx("tx_l1", "alice", "bob", 30.0, 5, 3.0),
            make_tx("tx_l2", "alice", "carol", 40.0, 6, 4.0),
        ];
        let remote = vec![make_tx("tx_r1", "alice", "dave", 70.0, 7, 2.0)];

        let resolutions = resolve_economic_conflicts(&[conflict], &local, &remote, &accounts);
        assert_eq!(resolutions.len(), 1);
        assert_eq!(resolutions[0].winner_tx_hashes, vec!["tx_l1", "tx_l2"]);
        assert_eq!(resolutions[0].loser_tx_hashes, vec!["tx_r1"]);
    }

    #[test]
    fn test_deterministic_ordering_same_nonce_hash_tiebreak() {
        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), make_account("alice", 50.0));

        let conflict = EconomicConflict {
            account: "alice".to_string(),
            pre_partition_balance_micro: to_micro_units(50.0),
            total_sent_local_micro: to_micro_units(40.0),
            total_sent_remote_micro: to_micro_units(40.0),
            total_received_local_micro: 0,
            total_received_remote_micro: 0,
            deficit_micro: to_micro_units(30.0),
            local_tx_hashes: vec!["tx_zzz".to_string()],
            remote_tx_hashes: vec!["tx_aaa".to_string()],
        };

        let local = vec![make_tx("tx_zzz", "alice", "bob", 40.0, 5, 1.0)];
        let remote = vec![make_tx("tx_aaa", "alice", "carol", 40.0, 5, 1.0)];

        let resolutions = resolve_economic_conflicts(&[conflict], &local, &remote, &accounts);
        assert_eq!(resolutions[0].winner_tx_hashes, vec!["tx_aaa"]);
        assert_eq!(resolutions[0].loser_tx_hashes, vec!["tx_zzz"]);
    }
}
