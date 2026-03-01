use proptest::prelude::*;
use std::collections::{HashMap, HashSet};
use rinku_core::types::Account;
use super::{
    MergeReport, MergePhase, PartitionTxSummary, RollbackReport,
    conflict_detection, resolution, cascade,
};

fn make_account(address: &str, balance_micro: u64, nonce: u64) -> Account {
    Account {
        address: address.to_string(),
        balance: balance_micro,
        nonce,
        first_seen: 1700000000,
        staked: 0,
        unbonding: 0,
        unbonding_release: None,
        latest_balance_proof: None,
        partition_violations: 0,
        reputation_penalty: 0.0,
        penalty_decay_checkpoint: None,
        partition_budget: None,
        partition_budget_spent: 0,
    }
}

fn run_full_merge_pipeline(
    local_summaries: &[PartitionTxSummary],
    remote_summaries: &[PartitionTxSummary],
    fork_point_accounts: &HashMap<String, Account>,
) -> (MergeReport, RollbackReport) {
    let mut report = MergeReport::new(1, 0);

    report.phase = MergePhase::ConflictDetection;
    let (direct_conflicts, economic_conflicts) = conflict_detection::detect_all_conflicts(
        local_summaries,
        remote_summaries,
        fork_point_accounts,
    );
    report.direct_conflicts = direct_conflicts.clone();
    report.economic_conflicts = economic_conflicts.clone();

    report.phase = MergePhase::WeightResolution;
    let resolutions = resolution::resolve_all_conflicts(
        &direct_conflicts,
        &economic_conflicts,
        local_summaries,
        remote_summaries,
        fork_point_accounts,
    );

    let mut conflict_losers: HashSet<String> = HashSet::new();
    for res in &resolutions {
        conflict_losers.extend(res.loser_tx_hashes.clone());
    }
    report.resolutions = resolutions;

    report.phase = MergePhase::CascadeRollback;
    let all_txs: Vec<PartitionTxSummary> = local_summaries.iter()
        .chain(remote_summaries.iter())
        .cloned()
        .collect();

    let fork_balances_micro = cascade::build_fork_point_balances_micro(fork_point_accounts);
    let fork_nonces = cascade::build_fork_point_nonces(fork_point_accounts);

    let rollback_report = cascade::cascade_rollback(
        &conflict_losers,
        &all_txs,
        &fork_balances_micro,
        &fork_nonces,
    );

    let mut all_rejected: Vec<String> = conflict_losers.iter().cloned().collect();
    for cr in &rollback_report.cascade_rollbacks {
        all_rejected.push(cr.tx_hash.clone());
    }
    all_rejected.sort();
    all_rejected.dedup();

    report.cascade_rollbacks = rollback_report.cascade_rollbacks.clone();
    report.cascade_rejected_count = report.cascade_rollbacks.len();
    report.final_balances_micro = Some(rollback_report.final_balances_micro.clone());
    report.transactions_kept = rollback_report.surviving_tx_hashes.clone();
    report.transactions_rejected = all_rejected;
    report.local_tx_count = local_summaries.len();
    report.remote_tx_count = remote_summaries.len();

    report.complete();
    (report, rollback_report)
}

const ADDR_POOL: &[&str] = &["alice", "bob", "carol", "dave", "eve", "frank", "grace", "heidi"];
const WEIGHT_PROXIMITY_THRESHOLD: f64 = 1.5;

fn build_nonce_chain_txs(
    side: &str,
    from_idx: usize,
    to_idx: usize,
    count: usize,
    amount_per_tx: u64,
    gas_per_tx: u64,
    start_nonce: u64,
    weight: f64,
) -> Vec<PartitionTxSummary> {
    (0..count)
        .map(|i| {
            let n = start_nonce + i as u64;
            PartitionTxSummary {
                tx_hash: format!("{}_{}_n{}_{}", side, ADDR_POOL[from_idx], n, i),
                from: ADDR_POOL[from_idx].to_string(),
                to: ADDR_POOL[to_idx].to_string(),
                amount_micro: amount_per_tx,
                gas_micro: gas_per_tx,
                nonce: n,
                weight,
                partition_epoch: Some(1),
                visible_stake_pct: 0.5,
            }
        })
        .collect()
}

fn arb_valid_nonce_chain(
    side: &'static str,
    from_idx: usize,
    to_idx: usize,
    start_nonce: u64,
    max_balance: u64,
) -> impl Strategy<Value = Vec<PartitionTxSummary>> {
    let count = 0usize..=4;
    let weight = 1u32..=100u32;
    let gas = 0u64..=100u64;

    (count, weight, gas).prop_flat_map(move |(c, w, g)| {
        if c == 0 {
            return Just(vec![]).boxed();
        }
        let max_per_tx = if c > 0 { max_balance / (c as u64) } else { max_balance };
        let amt_range = if max_per_tx > 1 { 1u64..=max_per_tx } else { 1u64..=1u64 };
        prop::collection::vec(amt_range, c).prop_map(move |amounts| {
            amounts.iter().enumerate().map(|(i, &amt)| {
                let n = start_nonce + i as u64;
                PartitionTxSummary {
                    tx_hash: format!("{}_{}_n{}_{}", side, ADDR_POOL[from_idx], n, amt),
                    from: ADDR_POOL[from_idx].to_string(),
                    to: ADDR_POOL[to_idx].to_string(),
                    amount_micro: amt,
                    gas_micro: g,
                    nonce: n,
                    weight: w as f64,
                    partition_epoch: Some(1),
                    visible_stake_pct: 0.5,
                }
            }).collect::<Vec<_>>()
        }).boxed()
    })
}

fn arb_scenario_with_nonce_chains()
    -> impl Strategy<Value = (HashMap<String, Account>, Vec<PartitionTxSummary>, Vec<PartitionTxSummary>)>
{
    let balances = prop::collection::vec(500_000u64..=5_000_000u64, 5usize);

    balances.prop_flat_map(|bals| {
        let b0 = bals[0]; let b1 = bals[1];

        let local_a = arb_valid_nonce_chain("L", 0, 2, 0, b0 / 2);
        let local_b = arb_valid_nonce_chain("L", 1, 3, 0, b1 / 2);
        let remote_a = arb_valid_nonce_chain("R", 0, 3, 0, b0 / 2);
        let remote_b = arb_valid_nonce_chain("R", 1, 4, 0, b1 / 2);

        (Just(bals), local_a, local_b, remote_a, remote_b)
    }).prop_map(|(bals, la, lb, ra, rb)| {
        let mut accounts = HashMap::new();
        for (i, &b) in bals.iter().enumerate() {
            let addr = ADDR_POOL[i].to_string();
            accounts.insert(addr.clone(), make_account(&addr, b, 0));
        }
        let mut local = la; local.extend(lb);
        let mut remote = ra; remote.extend(rb);

        let mut seen = HashSet::new();
        local.retain(|tx| seen.insert(tx.tx_hash.clone()));
        remote.retain(|tx| seen.insert(tx.tx_hash.clone()));

        (accounts, local, remote)
    })
}

fn arb_direct_conflict_scenario()
    -> impl Strategy<Value = (HashMap<String, Account>, Vec<PartitionTxSummary>, Vec<PartitionTxSummary>)>
{
    let balance = 1_000_000u64..=5_000_000u64;
    let amount = 1_000u64..=100_000u64;
    let local_weight = 1u32..=100u32;
    let remote_weight = 1u32..=100u32;
    let nonce = 0u64..=3u64;

    (balance, amount, local_weight, remote_weight, nonce).prop_map(|(bal, amt, lw, rw, n)| {
        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), make_account("alice", bal, 0));
        accounts.insert("bob".to_string(), make_account("bob", 0, 0));
        accounts.insert("carol".to_string(), make_account("carol", 0, 0));

        let mut local = Vec::new();
        for i in 0..=n {
            local.push(PartitionTxSummary {
                tx_hash: format!("L_alice_n{}", i),
                from: "alice".to_string(),
                to: "bob".to_string(),
                amount_micro: amt.min(bal / (n + 1).max(1)),
                gas_micro: 0,
                nonce: i,
                weight: lw as f64,
                partition_epoch: Some(1),
                visible_stake_pct: 0.5,
            });
        }

        let mut remote = Vec::new();
        for i in 0..=n {
            remote.push(PartitionTxSummary {
                tx_hash: format!("R_alice_n{}", i),
                from: "alice".to_string(),
                to: "carol".to_string(),
                amount_micro: amt.min(bal / (n + 1).max(1)),
                gas_micro: 0,
                nonce: i,
                weight: rw as f64,
                partition_epoch: Some(1),
                visible_stake_pct: 0.5,
            });
        }

        (accounts, local, remote)
    })
}

fn arb_economic_overdraft_scenario()
    -> impl Strategy<Value = (HashMap<String, Account>, Vec<PartitionTxSummary>, Vec<PartitionTxSummary>)>
{
    let balance = 100_000u64..=1_000_000u64;
    let local_spend_pct = 60u64..=90u64;
    let remote_spend_pct = 60u64..=90u64;
    let weight = 1u32..=50u32;

    (balance, local_spend_pct, remote_spend_pct, weight).prop_map(|(bal, lp, rp, w)| {
        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), make_account("alice", bal, 0));
        accounts.insert("bob".to_string(), make_account("bob", 0, 0));
        accounts.insert("carol".to_string(), make_account("carol", 0, 0));

        let local_amount = bal * lp / 100;
        let remote_amount = bal * rp / 100;

        let local = vec![PartitionTxSummary {
            tx_hash: "L_alice_n0".to_string(),
            from: "alice".to_string(),
            to: "bob".to_string(),
            amount_micro: local_amount,
            gas_micro: 0,
            nonce: 0,
            weight: w as f64,
            partition_epoch: Some(1),
            visible_stake_pct: 0.5,
        }];
        let remote = vec![PartitionTxSummary {
            tx_hash: "R_alice_n1".to_string(),
            from: "alice".to_string(),
            to: "carol".to_string(),
            amount_micro: remote_amount,
            gas_micro: 0,
            nonce: 1,
            weight: w as f64,
            partition_epoch: Some(1),
            visible_stake_pct: 0.5,
        }];

        (accounts, local, remote)
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn prop_balance_conservation(
        (accounts, local, remote) in arb_scenario_with_nonce_chains()
    ) {
        let (_, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

        let total_initial: u64 = accounts.values()
            .map(|a| a.balance)
            .sum();

        let total_gas_burned: u64 = local.iter().chain(remote.iter())
            .filter(|tx| rollback.surviving_tx_hashes.contains(&tx.tx_hash))
            .map(|tx| tx.gas_micro)
            .sum();

        let total_final: u64 = rollback.final_balances_micro.values().sum();

        prop_assert_eq!(
            total_initial,
            total_final + total_gas_burned,
            "Balance must be conserved. initial={}, final={}, gas_burned={}",
            total_initial, total_final, total_gas_burned
        );
    }

    #[test]
    fn prop_balance_conservation_with_conflicts(
        (accounts, local, remote) in arb_direct_conflict_scenario()
    ) {
        let (_, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

        let total_initial: u64 = accounts.values()
            .map(|a| a.balance)
            .sum();
        let total_gas_burned: u64 = local.iter().chain(remote.iter())
            .filter(|tx| rollback.surviving_tx_hashes.contains(&tx.tx_hash))
            .map(|tx| tx.gas_micro)
            .sum();
        let total_final: u64 = rollback.final_balances_micro.values().sum();

        prop_assert_eq!(
            total_initial,
            total_final + total_gas_burned,
            "Balance must be conserved even with direct conflicts"
        );
    }

    #[test]
    fn prop_no_negative_balances(
        (accounts, local, remote) in arb_scenario_with_nonce_chains()
    ) {
        let (_, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

        let total_initial: u64 = accounts.values()
            .map(|a| a.balance)
            .sum();
        let total_final: u64 = rollback.final_balances_micro.values().sum();

        prop_assert!(
            total_final <= total_initial,
            "Total final balance {} exceeds total initial {} (indicates underflow)",
            total_final, total_initial
        );

        for (addr, &balance_micro) in &rollback.final_balances_micro {
            let initial = accounts.get(addr)
                .map(|a| a.balance)
                .unwrap_or(0);
            let total_received: u64 = local.iter().chain(remote.iter())
                .filter(|tx| rollback.surviving_tx_hashes.contains(&tx.tx_hash))
                .filter(|tx| tx.to == *addr)
                .map(|tx| tx.amount_micro)
                .sum();
            let max_possible = initial.saturating_add(total_received);
            prop_assert!(
                balance_micro <= max_possible,
                "Account {} balance {} exceeds max possible {} (initial={} + received={})",
                addr, balance_micro, max_possible, initial, total_received
            );
        }
    }

    #[test]
    fn prop_no_negative_balances_overdraft(
        (accounts, local, remote) in arb_economic_overdraft_scenario()
    ) {
        let (_, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

        let total_initial: u64 = accounts.values()
            .map(|a| a.balance)
            .sum();
        let total_final: u64 = rollback.final_balances_micro.values().sum();

        prop_assert!(
            total_final <= total_initial,
            "Total final {} exceeds initial {} during overdraft scenario",
            total_final, total_initial
        );
    }

    #[test]
    fn prop_determinism(
        (accounts, local, remote) in arb_scenario_with_nonce_chains()
    ) {
        let (r1, rb1) = run_full_merge_pipeline(&local, &remote, &accounts);
        let (r2, rb2) = run_full_merge_pipeline(&local, &remote, &accounts);

        prop_assert_eq!(
            &r1.transactions_kept, &r2.transactions_kept,
            "Kept transactions must be deterministic"
        );
        prop_assert_eq!(
            &r1.transactions_rejected, &r2.transactions_rejected,
            "Rejected transactions must be deterministic"
        );
        prop_assert_eq!(
            &rb1.final_balances_micro, &rb2.final_balances_micro,
            "Final balances must be deterministic"
        );
    }

    #[test]
    fn prop_determinism_with_conflicts(
        (accounts, local, remote) in arb_direct_conflict_scenario()
    ) {
        let (r1, rb1) = run_full_merge_pipeline(&local, &remote, &accounts);
        let (r2, rb2) = run_full_merge_pipeline(&local, &remote, &accounts);

        prop_assert_eq!(
            &r1.transactions_kept, &r2.transactions_kept,
            "Conflict resolution must be deterministic"
        );
        prop_assert_eq!(
            &rb1.final_balances_micro, &rb2.final_balances_micro,
            "Final balances after conflicts must be deterministic"
        );
    }

    #[test]
    fn prop_commutativity(
        (accounts, local, remote) in arb_scenario_with_nonce_chains()
    ) {
        let (r_lr, rb_lr) = run_full_merge_pipeline(&local, &remote, &accounts);
        let (r_rl, rb_rl) = run_full_merge_pipeline(&remote, &local, &accounts);

        let mut kept_lr = r_lr.transactions_kept.clone();
        kept_lr.sort();
        let mut kept_rl = r_rl.transactions_kept.clone();
        kept_rl.sort();

        prop_assert_eq!(
            &kept_lr, &kept_rl,
            "Merge must be commutative: swapping local/remote must yield same kept set"
        );

        let mut rejected_lr = r_lr.transactions_rejected.clone();
        rejected_lr.sort();
        let mut rejected_rl = r_rl.transactions_rejected.clone();
        rejected_rl.sort();

        prop_assert_eq!(
            &rejected_lr, &rejected_rl,
            "Merge must be commutative: rejected sets must match"
        );

        prop_assert_eq!(
            &rb_lr.final_balances_micro, &rb_rl.final_balances_micro,
            "Merge must be commutative: final balances must match"
        );
    }

    #[test]
    fn prop_commutativity_with_conflicts(
        (accounts, local, remote) in arb_direct_conflict_scenario()
    ) {
        let (r_lr, rb_lr) = run_full_merge_pipeline(&local, &remote, &accounts);
        let (r_rl, rb_rl) = run_full_merge_pipeline(&remote, &local, &accounts);

        let mut kept_lr = r_lr.transactions_kept.clone();
        kept_lr.sort();
        let mut kept_rl = r_rl.transactions_kept.clone();
        kept_rl.sort();

        prop_assert_eq!(
            &kept_lr, &kept_rl,
            "Direct conflict merge must be commutative"
        );

        prop_assert_eq!(
            &rb_lr.final_balances_micro, &rb_rl.final_balances_micro,
            "Direct conflict merge balances must be commutative"
        );
    }

    #[test]
    fn prop_partition_completeness(
        (accounts, local, remote) in arb_scenario_with_nonce_chains()
    ) {
        let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

        let all_hashes: HashSet<String> = local.iter()
            .chain(remote.iter())
            .map(|tx| tx.tx_hash.clone())
            .collect();

        let kept_set: HashSet<String> = report.transactions_kept.iter().cloned().collect();
        let rejected_set: HashSet<String> = report.transactions_rejected.iter().cloned().collect();

        for h in &kept_set {
            prop_assert!(
                !rejected_set.contains(h),
                "Transaction {} is in both kept and rejected sets",
                h
            );
        }

        let accounted: HashSet<String> = kept_set.union(&rejected_set).cloned().collect();
        for h in &all_hashes {
            prop_assert!(
                accounted.contains(h),
                "Transaction {} is neither kept nor rejected",
                h
            );
        }
    }

    #[test]
    fn prop_surviving_nonce_continuity(
        (accounts, local, remote) in arb_scenario_with_nonce_chains()
    ) {
        let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

        let all_txs: Vec<&PartitionTxSummary> = local.iter()
            .chain(remote.iter())
            .collect();

        let surviving: Vec<&&PartitionTxSummary> = all_txs.iter()
            .filter(|tx| report.transactions_kept.contains(&tx.tx_hash))
            .collect();

        let mut per_account: HashMap<String, Vec<u64>> = HashMap::new();
        for tx in &surviving {
            per_account.entry(tx.from.clone()).or_default().push(tx.nonce);
        }

        for (addr, nonces) in &per_account {
            let mut sorted = nonces.clone();
            sorted.sort();
            sorted.dedup();

            let fork_nonce = accounts.get(addr).map(|a| a.nonce).unwrap_or(0);

            for (i, &n) in sorted.iter().enumerate() {
                let expected = fork_nonce + i as u64;
                prop_assert_eq!(
                    n, expected,
                    "Account {} has nonce gap: expected {}, got {} (surviving nonces: {:?})",
                    addr, expected, n, sorted
                );
            }
        }
    }

    #[test]
    fn prop_no_double_spend_survives(
        (accounts, local, remote) in arb_direct_conflict_scenario()
    ) {
        let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

        let all_txs: Vec<&PartitionTxSummary> = local.iter()
            .chain(remote.iter())
            .collect();

        let surviving: Vec<&&PartitionTxSummary> = all_txs.iter()
            .filter(|tx| report.transactions_kept.contains(&tx.tx_hash))
            .collect();

        let mut nonce_usage: HashMap<(String, u64), Vec<String>> = HashMap::new();
        for tx in &surviving {
            nonce_usage
                .entry((tx.from.clone(), tx.nonce))
                .or_default()
                .push(tx.tx_hash.clone());
        }

        for ((addr, nonce), hashes) in &nonce_usage {
            let unique: HashSet<&String> = hashes.iter().collect();
            prop_assert!(
                unique.len() <= 1,
                "Account {} nonce {} has multiple surviving txs: {:?}",
                addr, nonce, hashes
            );
        }
    }

    #[test]
    fn prop_higher_weight_wins_beyond_threshold(
        winner_weight in 20.0f64..100.0,
        balance in 1_000_000u64..=5_000_000u64,
    ) {
        let loser_weight = winner_weight / (WEIGHT_PROXIMITY_THRESHOLD + 0.5);

        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), make_account("alice", balance, 0));
        accounts.insert("bob".to_string(), make_account("bob", 0, 0));
        accounts.insert("carol".to_string(), make_account("carol", 0, 0));

        let local = vec![PartitionTxSummary {
            tx_hash: "L_alice_n0".to_string(),
            from: "alice".to_string(),
            to: "bob".to_string(),
            amount_micro: 1000,
            gas_micro: 0,
            nonce: 0,
            weight: winner_weight,
            partition_epoch: Some(1),
            visible_stake_pct: 0.5,
        }];
        let remote = vec![PartitionTxSummary {
            tx_hash: "R_alice_n0".to_string(),
            from: "alice".to_string(),
            to: "carol".to_string(),
            amount_micro: 1000,
            gas_micro: 0,
            nonce: 0,
            weight: loser_weight,
            partition_epoch: Some(1),
            visible_stake_pct: 0.5,
        }];

        let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

        prop_assert!(
            report.transactions_kept.contains(&"L_alice_n0".to_string()),
            "Higher weight tx should win when beyond proximity threshold (winner={}, loser={})",
            winner_weight, loser_weight
        );
        prop_assert!(
            report.transactions_rejected.contains(&"R_alice_n0".to_string()),
            "Lower weight tx should be rejected"
        );
    }

    #[test]
    fn prop_hash_tiebreak_deterministic(
        weight in 1.0f64..50.0,
        balance in 1_000_000u64..=5_000_000u64,
    ) {
        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), make_account("alice", balance, 0));
        accounts.insert("bob".to_string(), make_account("bob", 0, 0));
        accounts.insert("carol".to_string(), make_account("carol", 0, 0));

        let local = vec![PartitionTxSummary {
            tx_hash: "zzz_hash".to_string(),
            from: "alice".to_string(),
            to: "bob".to_string(),
            amount_micro: 1000,
            gas_micro: 0,
            nonce: 0,
            weight,
            partition_epoch: Some(1),
            visible_stake_pct: 0.5,
        }];
        let remote = vec![PartitionTxSummary {
            tx_hash: "aaa_hash".to_string(),
            from: "alice".to_string(),
            to: "carol".to_string(),
            amount_micro: 1000,
            gas_micro: 0,
            nonce: 0,
            weight,
            partition_epoch: Some(1),
            visible_stake_pct: 0.5,
        }];

        let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

        prop_assert!(
            report.transactions_kept.contains(&"aaa_hash".to_string()),
            "Lower hash should win tiebreak (equal weight={})",
            weight
        );
        prop_assert!(
            report.transactions_rejected.contains(&"zzz_hash".to_string()),
            "Higher hash should lose tiebreak"
        );
    }

    #[test]
    fn prop_cascade_convergence(
        (accounts, local, remote) in arb_scenario_with_nonce_chains()
    ) {
        let (_, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

        let total_txs = local.len() + remote.len();
        prop_assert!(
            rollback.iterations <= (total_txs + 1) as u32,
            "Cascade should converge in at most N+1 iterations, got {} for {} txs",
            rollback.iterations, total_txs
        );
    }

    #[test]
    fn prop_empty_partitions_are_noop(
        balance in prop::collection::vec(100_000u64..=5_000_000u64, 3usize),
    ) {
        let mut accounts = HashMap::new();
        for (i, &b) in balance.iter().enumerate() {
            let addr = ADDR_POOL[i].to_string();
            accounts.insert(addr.clone(), make_account(&addr, b, 0));
        }

        let (report, rollback) = run_full_merge_pipeline(&[], &[], &accounts);

        prop_assert!(report.direct_conflicts.is_empty());
        prop_assert!(report.economic_conflicts.is_empty());
        prop_assert!(report.transactions_kept.is_empty());
        prop_assert!(report.transactions_rejected.is_empty());
        prop_assert_eq!(report.phase, MergePhase::Complete);

        for (addr, acct) in &accounts {
            let final_bal = rollback.final_balances_micro.get(addr).copied().unwrap_or(0);
            prop_assert_eq!(
                final_bal, acct.balance,
                "Empty merge must not change balance for {}",
                addr
            );
        }
    }

    #[test]
    fn prop_conflict_free_merge_keeps_everything(
        (accounts, local, remote) in arb_scenario_with_nonce_chains()
    ) {
        let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

        if report.direct_conflicts.is_empty() && report.economic_conflicts.is_empty() {
            let expected_count = {
                let all: HashSet<String> = local.iter()
                    .chain(remote.iter())
                    .map(|tx| tx.tx_hash.clone())
                    .collect();
                all.len()
            };

            let kept_count = report.transactions_kept.len();
            let cascade_count = report.cascade_rollbacks.len();

            prop_assert_eq!(
                kept_count + cascade_count, expected_count,
                "Without conflicts, kept + cascade-rejected should equal total unique txs"
            );
        }
    }

    #[test]
    fn prop_direct_conflict_always_detected(
        (accounts, local, remote) in arb_direct_conflict_scenario()
    ) {
        let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

        let local_nonces: HashSet<(String, u64)> = local.iter()
            .map(|tx| (tx.from.clone(), tx.nonce))
            .collect();
        let remote_nonces: HashSet<(String, u64)> = remote.iter()
            .map(|tx| (tx.from.clone(), tx.nonce))
            .collect();

        let overlapping: HashSet<&(String, u64)> = local_nonces.intersection(&remote_nonces).collect();

        let actual_conflicts: HashSet<(String, u64)> = report.direct_conflicts.iter()
            .map(|c| (c.account.clone(), c.nonce))
            .collect();

        for (addr, nonce) in overlapping {
            let local_hash = local.iter()
                .find(|tx| tx.from == *addr && tx.nonce == *nonce)
                .map(|tx| &tx.tx_hash);
            let remote_hash = remote.iter()
                .find(|tx| tx.from == *addr && tx.nonce == *nonce)
                .map(|tx| &tx.tx_hash);

            if local_hash != remote_hash {
                prop_assert!(
                    actual_conflicts.contains(&(addr.clone(), *nonce)),
                    "Direct conflict at {}:nonce={} not detected",
                    addr, nonce
                );
            }
        }
    }

    #[test]
    fn prop_economic_overdraft_rejected(
        (accounts, local, remote) in arb_economic_overdraft_scenario()
    ) {
        let (report, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

        let alice_initial = accounts.get("alice").unwrap().balance;
        let alice_final = rollback.final_balances_micro.get("alice").copied().unwrap_or(0);

        prop_assert!(
            alice_final <= alice_initial,
            "Alice balance {} exceeds initial {} after overdraft resolution",
            alice_final, alice_initial
        );

        let total_spent: u64 = local.iter().chain(remote.iter())
            .filter(|tx| tx.from == "alice" && rollback.surviving_tx_hashes.contains(&tx.tx_hash))
            .map(|tx| tx.amount_micro + tx.gas_micro)
            .sum();

        prop_assert!(
            total_spent <= alice_initial,
            "Surviving txs spend {} but alice only had {}",
            total_spent, alice_initial
        );
    }
}
