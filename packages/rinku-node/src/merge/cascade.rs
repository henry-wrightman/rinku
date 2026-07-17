use super::{CascadeRollback, PartitionTxSummary, RollbackReason, RollbackReport};
use std::collections::{HashMap, HashSet};

/// Assign topological DAG depth from fork-point for each tx in `txs`.
/// Parents outside the set (pre-fork / unknown) are treated as depth roots (depth 0).
pub fn assign_dag_depths(txs: &mut [PartitionTxSummary]) {
    let in_set: HashSet<String> = txs.iter().map(|t| t.tx_hash.clone()).collect();
    let parents_of: HashMap<String, Vec<String>> = txs
        .iter()
        .map(|t| (t.tx_hash.clone(), t.parents.clone()))
        .collect();

    let mut depths: HashMap<String, u32> = HashMap::new();
    let mut visiting: HashSet<String> = HashSet::new();

    fn resolve(
        hash: &str,
        in_set: &HashSet<String>,
        parents_of: &HashMap<String, Vec<String>>,
        depths: &mut HashMap<String, u32>,
        visiting: &mut HashSet<String>,
    ) -> u32 {
        if let Some(&d) = depths.get(hash) {
            return d;
        }
        if !visiting.insert(hash.to_string()) {
            return 0;
        }
        let depth = match parents_of.get(hash) {
            Some(parents) => {
                let in_partition: Vec<&String> =
                    parents.iter().filter(|p| in_set.contains(*p)).collect();
                if in_partition.is_empty() {
                    0
                } else {
                    1 + in_partition
                        .iter()
                        .map(|p| resolve(p, in_set, parents_of, depths, visiting))
                        .max()
                        .unwrap_or(0)
                }
            }
            None => 0,
        };
        visiting.remove(hash);
        depths.insert(hash.to_string(), depth);
        depth
    }

    for tx in txs.iter() {
        resolve(
            &tx.tx_hash,
            &in_set,
            &parents_of,
            &mut depths,
            &mut visiting,
        );
    }
    for tx in txs.iter_mut() {
        tx.dag_depth = depths.get(&tx.tx_hash).copied().unwrap_or(0);
    }
}

pub fn cascade_rollback(
    losing_tx_hashes: &HashSet<String>,
    all_txs: &[PartitionTxSummary],
    fork_point_balances_micro: &HashMap<String, u64>,
    fork_point_nonces: &HashMap<String, u64>,
) -> RollbackReport {
    let mut annotated = all_txs.to_vec();
    assign_dag_depths(&mut annotated);

    let mut ordered_txs: Vec<PartitionTxSummary> = annotated
        .into_iter()
        .filter(|tx| !losing_tx_hashes.contains(&tx.tx_hash))
        .collect();

    // Canonical order (partition-tolerance.md §1.1 / Phase 4):
    // nonce ascending → DAG depth from fork-point → tx hash tiebreak
    ordered_txs.sort_by(|a, b| {
        a.nonce
            .cmp(&b.nonce)
            .then_with(|| a.dag_depth.cmp(&b.dag_depth))
            .then_with(|| a.tx_hash.cmp(&b.tx_hash))
    });

    let mut rolled_back: HashSet<String> = HashSet::new();
    let mut rollback_reasons: HashMap<String, RollbackReason> = HashMap::new();
    let mut iteration_count: u32 = 0;

    loop {
        let mut newly_rolled_back: HashSet<String> = HashSet::new();
        iteration_count += 1;

        let mut balances = fork_point_balances_micro.clone();
        let mut expected_nonces: HashMap<String, u64> = fork_point_nonces.clone();

        for tx in &ordered_txs {
            if rolled_back.contains(&tx.tx_hash) {
                continue;
            }

            if !expected_nonces.contains_key(&tx.from) {
                expected_nonces.insert(tx.from.clone(), 0);
            }

            let expected = expected_nonces.get(&tx.from).copied().unwrap_or(0);
            if tx.nonce != expected {
                newly_rolled_back.insert(tx.tx_hash.clone());
                rollback_reasons.insert(tx.tx_hash.clone(), RollbackReason::NonceContinuityGap);
                continue;
            }

            let sender_balance = balances.get(&tx.from).copied().unwrap_or(0);
            let total_cost = tx.amount_micro.saturating_add(tx.gas_micro);

            if sender_balance < total_cost {
                newly_rolled_back.insert(tx.tx_hash.clone());
                rollback_reasons.insert(
                    tx.tx_hash.clone(),
                    RollbackReason::InsufficientBalanceAfterConflictResolution,
                );
                continue;
            }

            *balances.entry(tx.from.clone()).or_insert(0) -= total_cost;
            *balances.entry(tx.to.clone()).or_insert(0) += tx.amount_micro;
            *expected_nonces.entry(tx.from.clone()).or_insert(0) += 1;
        }

        if newly_rolled_back.is_empty() {
            let cascade_details: Vec<CascadeRollback> = rolled_back
                .iter()
                .filter(|h| !losing_tx_hashes.contains(*h))
                .filter_map(|h| {
                    let tx = all_txs.iter().find(|t| t.tx_hash == *h)?;
                    let reason = rollback_reasons
                        .get(h)
                        .cloned()
                        .unwrap_or(RollbackReason::InsufficientBalanceAfterConflictResolution);
                    Some(CascadeRollback {
                        tx_hash: h.clone(),
                        reason,
                        affected_account: tx.from.clone(),
                        amount_reverted_micro: tx.amount_micro.saturating_add(tx.gas_micro),
                    })
                })
                .collect();

            let surviving_hashes: Vec<String> = ordered_txs
                .iter()
                .filter(|tx| !rolled_back.contains(&tx.tx_hash))
                .map(|tx| tx.tx_hash.clone())
                .collect();

            return RollbackReport {
                direct_conflict_losers: losing_tx_hashes.clone(),
                cascade_rollbacks: cascade_details,
                final_balances_micro: balances,
                surviving_tx_hashes: surviving_hashes,
                iterations: iteration_count,
            };
        }

        rolled_back.extend(newly_rolled_back);
    }
}

pub fn build_fork_point_balances_micro(
    fork_point_accounts: &HashMap<String, rinku_core::types::Account>,
) -> HashMap<String, u64> {
    fork_point_accounts
        .iter()
        .map(|(addr, acct)| (addr.clone(), acct.balance))
        .collect()
}

pub fn build_fork_point_nonces(
    fork_point_accounts: &HashMap<String, rinku_core::types::Account>,
) -> HashMap<String, u64> {
    fork_point_accounts
        .iter()
        .map(|(addr, acct)| (addr.clone(), acct.nonce))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rinku_core::types::{from_micro_units, to_micro_units};

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

    fn nonces(pairs: &[(&str, u64)]) -> HashMap<String, u64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn test_no_cascade_when_no_dependencies() {
        let mut balances = HashMap::new();
        balances.insert("alice".to_string(), to_micro_units(100.0));
        balances.insert("bob".to_string(), to_micro_units(100.0));
        let nonces = nonces(&[("alice", 0), ("bob", 0)]);

        let txs = vec![
            make_tx("tx1", "alice", "carol", 50.0, 0, 5.0),
            make_tx("tx2", "bob", "dave", 50.0, 0, 3.0),
        ];

        let losers: HashSet<String> = ["tx2".to_string()].into();
        let report = cascade_rollback(&losers, &txs, &balances, &nonces);

        assert!(report.cascade_rollbacks.is_empty());
        assert_eq!(report.surviving_tx_hashes, vec!["tx1"]);
    }

    #[test]
    fn test_cascade_when_recipient_spent_rolled_back_funds() {
        let mut balances = HashMap::new();
        balances.insert("alice".to_string(), to_micro_units(100.0));
        balances.insert("bob".to_string(), to_micro_units(10.0));
        let nonces = nonces(&[("alice", 0), ("bob", 0)]);

        let txs = vec![
            make_tx("tx1", "alice", "bob", 90.0, 0, 5.0),
            make_tx("tx2", "bob", "carol", 95.0, 0, 3.0),
        ];

        let losers: HashSet<String> = ["tx1".to_string()].into();
        let report = cascade_rollback(&losers, &txs, &balances, &nonces);

        assert_eq!(report.cascade_rollbacks.len(), 1);
        assert_eq!(report.cascade_rollbacks[0].tx_hash, "tx2");
        assert_eq!(report.cascade_rollbacks[0].affected_account, "bob");
        assert!(report.surviving_tx_hashes.is_empty());
    }

    #[test]
    fn test_nonce_gap_cascade() {
        let mut balances = HashMap::new();
        balances.insert("alice".to_string(), to_micro_units(200.0));
        let nonces = nonces(&[("alice", 0)]);

        let txs = vec![
            make_tx("tx1", "alice", "bob", 10.0, 0, 5.0),
            make_tx("tx2", "alice", "carol", 10.0, 1, 5.0),
            make_tx("tx3", "alice", "dave", 10.0, 2, 5.0),
        ];

        let losers: HashSet<String> = ["tx2".to_string()].into();
        let report = cascade_rollback(&losers, &txs, &balances, &nonces);

        assert_eq!(report.cascade_rollbacks.len(), 1);
        assert_eq!(report.cascade_rollbacks[0].tx_hash, "tx3");
        assert!(matches!(
            report.cascade_rollbacks[0].reason,
            RollbackReason::NonceContinuityGap
        ));
        assert_eq!(report.surviving_tx_hashes, vec!["tx1"]);
    }

    #[test]
    fn test_deep_cascade_chain() {
        let mut balances = HashMap::new();
        balances.insert("alice".to_string(), to_micro_units(100.0));
        balances.insert("bob".to_string(), to_micro_units(1.0));
        balances.insert("carol".to_string(), to_micro_units(1.0));
        balances.insert("dave".to_string(), to_micro_units(1.0));
        let nonces = nonces(&[("alice", 0), ("bob", 0), ("carol", 0), ("dave", 0)]);

        let txs = vec![
            make_tx("tx1", "alice", "bob", 50.0, 0, 5.0),
            make_tx("tx2", "bob", "carol", 45.0, 0, 3.0),
            make_tx("tx3", "carol", "dave", 40.0, 0, 2.0),
        ];

        let losers: HashSet<String> = ["tx1".to_string()].into();
        let report = cascade_rollback(&losers, &txs, &balances, &nonces);

        assert_eq!(report.cascade_rollbacks.len(), 2);
        let cascade_hashes: HashSet<&str> = report
            .cascade_rollbacks
            .iter()
            .map(|r| r.tx_hash.as_str())
            .collect();
        assert!(cascade_hashes.contains("tx2"));
        assert!(cascade_hashes.contains("tx3"));
        assert!(report.surviving_tx_hashes.is_empty());
    }

    #[test]
    fn test_no_rollback_when_balance_sufficient() {
        let mut balances = HashMap::new();
        balances.insert("alice".to_string(), to_micro_units(100.0));
        balances.insert("bob".to_string(), to_micro_units(100.0));
        let nonces = nonces(&[("alice", 0), ("bob", 0)]);

        let txs = vec![
            make_tx("tx1", "alice", "bob", 50.0, 0, 5.0),
            make_tx("tx2", "bob", "carol", 50.0, 0, 3.0),
        ];

        let losers: HashSet<String> = ["tx1".to_string()].into();
        let report = cascade_rollback(&losers, &txs, &balances, &nonces);

        assert!(report.cascade_rollbacks.is_empty());
        assert_eq!(report.surviving_tx_hashes, vec!["tx2"]);
    }

    #[test]
    fn test_convergence_with_multiple_iterations() {
        let mut balances = HashMap::new();
        balances.insert("alice".to_string(), to_micro_units(100.0));
        balances.insert("bob".to_string(), to_micro_units(5.0));
        balances.insert("carol".to_string(), to_micro_units(5.0));
        balances.insert("dave".to_string(), to_micro_units(5.0));
        let nonces = nonces(&[("alice", 0), ("bob", 0), ("carol", 0), ("dave", 0)]);

        let txs = vec![
            make_tx("tx_a1", "alice", "bob", 50.0, 0, 5.0),
            make_tx("tx_b1", "bob", "carol", 50.0, 0, 3.0),
            make_tx("tx_c1", "carol", "dave", 50.0, 0, 2.0),
            make_tx("tx_d1", "dave", "alice", 50.0, 0, 1.0),
        ];

        let losers: HashSet<String> = ["tx_a1".to_string()].into();
        let report = cascade_rollback(&losers, &txs, &balances, &nonces);

        assert_eq!(report.cascade_rollbacks.len(), 3);
        assert!(report.surviving_tx_hashes.is_empty());
    }

    #[test]
    fn test_fork_point_nonce_respected() {
        let mut balances = HashMap::new();
        balances.insert("alice".to_string(), to_micro_units(200.0));
        let nonces = nonces(&[("alice", 5)]);

        let txs = vec![
            make_tx("tx1", "alice", "bob", 10.0, 3, 5.0),
            make_tx("tx2", "alice", "carol", 10.0, 5, 5.0),
        ];

        let losers: HashSet<String> = HashSet::new();
        let report = cascade_rollback(&losers, &txs, &balances, &nonces);

        assert_eq!(report.cascade_rollbacks.len(), 1);
        assert_eq!(report.cascade_rollbacks[0].tx_hash, "tx1");
        assert!(matches!(
            report.cascade_rollbacks[0].reason,
            RollbackReason::NonceContinuityGap
        ));
        assert_eq!(report.surviving_tx_hashes, vec!["tx2"]);
    }

    #[test]
    fn test_assign_dag_depths_from_parents() {
        let mut txs = vec![
            make_tx("root", "alice", "bob", 10.0, 0, 1.0),
            {
                let mut t = make_tx("child", "bob", "carol", 5.0, 0, 1.0);
                t.parents = vec!["root".to_string()];
                t
            },
            {
                let mut t = make_tx("grandchild", "carol", "dave", 3.0, 0, 1.0);
                t.parents = vec!["child".to_string()];
                t
            },
        ];
        assign_dag_depths(&mut txs);
        assert_eq!(txs[0].dag_depth, 0);
        assert_eq!(txs[1].dag_depth, 1);
        assert_eq!(txs[2].dag_depth, 2);
    }

    #[test]
    fn test_canonical_order_nonce_then_depth_then_hash() {
        // Same nonce across accounts: shallower DAG depth must replay first.
        let mut balances = HashMap::new();
        balances.insert("alice".to_string(), to_micro_units(100.0));
        balances.insert("bob".to_string(), to_micro_units(0.0));
        balances.insert("carol".to_string(), to_micro_units(0.0));
        let nonces = nonces(&[("alice", 0), ("bob", 0)]);

        let mut funding = make_tx("funding", "alice", "bob", 80.0, 0, 5.0);
        funding.parents = vec![];
        let mut spend = make_tx("spend", "bob", "carol", 70.0, 0, 3.0);
        spend.parents = vec!["funding".to_string()];
        // Intentionally reverse list order; depth should still place funding before spend.
        let txs = vec![spend, funding];

        let losers: HashSet<String> = HashSet::new();
        let report = cascade_rollback(&losers, &txs, &balances, &nonces);

        assert_eq!(
            report.surviving_tx_hashes,
            vec!["funding".to_string(), "spend".to_string()],
            "Ordering must be nonce → dag_depth → hash so fund flow replays correctly"
        );
        assert_eq!(
            from_micro_units(*report.final_balances_micro.get("carol").unwrap()),
            70.0
        );
    }
}
