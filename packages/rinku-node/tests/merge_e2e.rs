use std::collections::HashMap;
use rinku_core::types::{
    Account, TransactionKind, to_micro_units, from_micro_units,
};
use rinku_node::merge::{
    MergeReport, MergePhase, PartitionTxSummary, RollbackReport,
    conflict_detection, resolution, cascade,
};

fn make_account(address: &str, balance_rku: f64, nonce: u64) -> Account {
    Account {
        address: address.to_string(),
        balance: to_micro_units(balance_rku),
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
        ecdsa_public_key: None,
    }
}

fn make_staked_account(address: &str, balance_rku: f64, nonce: u64, staked_rku: f64) -> Account {
    let mut acct = make_account(address, balance_rku, nonce);
    acct.staked = to_micro_units(staked_rku);
    acct
}

fn make_summary(hash: &str, from: &str, to: &str, amount: f64, nonce: u64, weight: f64) -> PartitionTxSummary {
    PartitionTxSummary {
        tx_hash: hash.to_string(),
        from: from.to_string(),
        to: to.to_string(),
        amount_micro: to_micro_units(amount),
        gas_micro: 0,
        nonce,
        weight,
        partition_epoch: Some(1),
        visible_stake_pct: 0.5,
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

    let mut conflict_losers: std::collections::HashSet<String> = std::collections::HashSet::new();
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

#[test]
fn test_clean_merge_no_conflicts() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 100.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 100.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));
    accounts.insert("dave".to_string(), make_account("dave", 0.0, 0));

    let local = vec![
        make_summary("tx_a1", "alice", "carol", 10.0, 0, 1.0),
    ];
    let remote = vec![
        make_summary("tx_b1", "bob", "dave", 20.0, 0, 1.0),
    ];

    let (report, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert_eq!(report.phase, MergePhase::Complete);
    assert!(report.direct_conflicts.is_empty());
    assert!(report.economic_conflicts.is_empty());
    assert!(report.resolutions.is_empty());
    assert!(report.transactions_rejected.is_empty());
    assert_eq!(report.transactions_kept.len(), 2);
    assert!(report.transactions_kept.contains(&"tx_a1".to_string()));
    assert!(report.transactions_kept.contains(&"tx_b1".to_string()));

    let balances = rollback.final_balances_micro;
    assert_eq!(from_micro_units(*balances.get("alice").unwrap()), 90.0);
    assert_eq!(from_micro_units(*balances.get("bob").unwrap()), 80.0);
    assert_eq!(from_micro_units(*balances.get("carol").unwrap()), 10.0);
    assert_eq!(from_micro_units(*balances.get("dave").unwrap()), 20.0);
}

#[test]
fn test_direct_double_spend_higher_weight_wins() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 100.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 0.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));

    let local = vec![
        make_summary("tx_local", "alice", "bob", 50.0, 0, 5.0),
    ];
    let remote = vec![
        make_summary("tx_remote", "alice", "carol", 50.0, 0, 1.0),
    ];

    let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert_eq!(report.direct_conflicts.len(), 1);
    assert_eq!(report.direct_conflicts[0].account, "alice");
    assert_eq!(report.direct_conflicts[0].nonce, 0);

    assert_eq!(report.resolutions.len(), 1);
    assert!(report.resolutions[0].winner_tx_hashes.contains(&"tx_local".to_string()));
    assert!(report.resolutions[0].loser_tx_hashes.contains(&"tx_remote".to_string()));

    assert!(report.transactions_kept.contains(&"tx_local".to_string()));
    assert!(report.transactions_rejected.contains(&"tx_remote".to_string()));
}

#[test]
fn test_direct_double_spend_hash_tiebreak() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 100.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 0.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));

    let local = vec![
        make_summary("zzz_local", "alice", "bob", 50.0, 0, 1.0),
    ];
    let remote = vec![
        make_summary("aaa_remote", "alice", "carol", 50.0, 0, 1.0),
    ];

    let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert_eq!(report.direct_conflicts.len(), 1);
    assert!(report.transactions_kept.contains(&"aaa_remote".to_string()));
    assert!(report.transactions_rejected.contains(&"zzz_local".to_string()));
}

#[test]
fn test_economic_overdraft_detection() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 100.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 0.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));

    let local = vec![
        make_summary("tx_l1", "alice", "bob", 70.0, 0, 1.0),
    ];
    let remote = vec![
        make_summary("tx_r1", "alice", "carol", 70.0, 1, 1.0),
    ];

    let (report, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert!(report.direct_conflicts.is_empty());
    assert_eq!(report.economic_conflicts.len(), 1);
    assert_eq!(report.economic_conflicts[0].account, "alice");

    assert!(!report.transactions_rejected.is_empty());

    let alice_balance = from_micro_units(*rollback.final_balances_micro.get("alice").unwrap_or(&0));
    assert!(alice_balance >= 0.0, "Alice balance must not go negative");
}

#[test]
fn test_cascade_rollback_fund_dependency() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 100.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 5.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));
    accounts.insert("dave".to_string(), make_account("dave", 0.0, 0));

    let local = vec![
        make_summary("tx_a_to_b", "alice", "bob", 50.0, 0, 5.0),
        make_summary("tx_b_to_carol", "bob", "carol", 40.0, 0, 1.0),
    ];
    let remote = vec![
        make_summary("tx_a_to_dave_dup", "alice", "dave", 50.0, 0, 1.0),
    ];

    let (report, _rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert!(report.transactions_kept.contains(&"tx_a_to_b".to_string()),
        "tx_a_to_b should win (higher weight)");
    assert!(report.transactions_rejected.contains(&"tx_a_to_dave_dup".to_string()),
        "tx_a_to_dave_dup should lose (lower weight, same nonce as tx_a_to_b)");

    assert!(report.transactions_kept.contains(&"tx_b_to_carol".to_string()),
        "tx_b_to_carol should survive since tx_a_to_b survived and bob has 5+50=55 >= 40");
}

#[test]
fn test_cascade_rollback_propagates_when_funds_lost() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 100.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 0.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));

    let local = vec![
        make_summary("tx_a_bob_local", "alice", "bob", 90.0, 0, 1.0),
        make_summary("tx_b_carol", "bob", "carol", 80.0, 0, 1.0),
    ];
    let remote = vec![
        make_summary("tx_a_carol_remote", "alice", "carol", 90.0, 0, 2.0),
    ];

    let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert!(report.transactions_rejected.contains(&"tx_a_bob_local".to_string()),
        "Local alice->bob should lose (lower weight)");
    assert!(report.transactions_rejected.contains(&"tx_b_carol".to_string()),
        "bob->carol should cascade-rollback (bob has 0 balance without alice's funds)");
    assert!(report.transactions_kept.contains(&"tx_a_carol_remote".to_string()),
        "Remote alice->carol should win (higher weight)");
}

#[test]
fn test_nonce_gap_cascade() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 1000.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 0.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));

    let local = vec![
        make_summary("tx_n0", "alice", "bob", 10.0, 0, 1.0),
        make_summary("tx_n1", "alice", "bob", 10.0, 1, 1.0),
        make_summary("tx_n2", "alice", "bob", 10.0, 2, 1.0),
    ];
    let remote = vec![
        make_summary("tx_n1_dup", "alice", "carol", 10.0, 1, 5.0),
    ];

    let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert!(report.transactions_kept.contains(&"tx_n0".to_string()),
        "nonce 0 has no conflict, should survive");

    assert!(report.transactions_rejected.contains(&"tx_n1".to_string()),
        "nonce 1 local should lose (lower weight)");

    assert!(report.transactions_kept.contains(&"tx_n1_dup".to_string()),
        "nonce 1 remote should win (higher weight)");

    assert!(report.transactions_kept.contains(&"tx_n2".to_string()),
        "nonce 2 should survive (nonce 1 slot filled by winning tx_n1_dup)");
}

#[test]
fn test_nonce_gap_cascade_balance_insufficient() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 100.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 0.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));

    let local = vec![
        make_summary("tx_n0", "alice", "bob", 90.0, 0, 1.0),
        make_summary("tx_n1", "alice", "bob", 90.0, 1, 1.0),
        make_summary("tx_n2", "alice", "bob", 5.0, 2, 1.0),
    ];
    let remote = vec![
        make_summary("tx_n0_dup", "alice", "carol", 90.0, 0, 5.0),
    ];

    let (report, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert!(report.transactions_rejected.contains(&"tx_n0".to_string()),
        "nonce 0 local should lose (lower weight)");

    assert!(report.transactions_kept.contains(&"tx_n0_dup".to_string()),
        "nonce 0 remote should win (higher weight)");

    assert!(report.transactions_rejected.contains(&"tx_n1".to_string()),
        "nonce 1 should be rejected (alice has only 10 left, needs 90)");

    assert!(report.transactions_rejected.contains(&"tx_n2".to_string()),
        "nonce 2 should cascade-rollback (nonce 1 rejected creates gap)");

    let alice_balance = from_micro_units(*rollback.final_balances_micro.get("alice").unwrap());
    assert_eq!(alice_balance, 10.0, "Alice should have 100 - 90 = 10 left");
}

#[test]
fn test_multiple_accounts_mixed_conflicts() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 200.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 200.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));
    accounts.insert("dave".to_string(), make_account("dave", 0.0, 0));

    let local = vec![
        make_summary("tx_alice_carol_l", "alice", "carol", 50.0, 0, 3.0),
        make_summary("tx_bob_dave_l", "bob", "dave", 30.0, 0, 1.0),
    ];
    let remote = vec![
        make_summary("tx_alice_dave_r", "alice", "dave", 50.0, 0, 1.0),
        make_summary("tx_bob_carol_r", "bob", "carol", 30.0, 0, 4.0),
    ];

    let (report, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert_eq!(report.direct_conflicts.len(), 2);

    assert!(report.transactions_kept.contains(&"tx_alice_carol_l".to_string()),
        "Alice local (weight 3.0) beats remote (weight 1.0)");
    assert!(report.transactions_rejected.contains(&"tx_alice_dave_r".to_string()));

    assert!(report.transactions_kept.contains(&"tx_bob_carol_r".to_string()),
        "Bob remote (weight 4.0) beats local (weight 1.0)");
    assert!(report.transactions_rejected.contains(&"tx_bob_dave_l".to_string()));

    let balances = &rollback.final_balances_micro;
    assert_eq!(from_micro_units(*balances.get("alice").unwrap()), 150.0);
    assert_eq!(from_micro_units(*balances.get("bob").unwrap()), 170.0);
    assert_eq!(from_micro_units(*balances.get("carol").unwrap()), 80.0);
    assert_eq!(from_micro_units(*balances.get("dave").unwrap()), 0.0);
}

#[test]
fn test_deep_cascade_chain_three_hops() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 100.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 0.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));
    accounts.insert("dave".to_string(), make_account("dave", 0.0, 0));
    accounts.insert("eve".to_string(), make_account("eve", 0.0, 0));

    let local = vec![
        make_summary("tx_a_b", "alice", "bob", 90.0, 0, 1.0),
        make_summary("tx_b_c", "bob", "carol", 80.0, 0, 1.0),
        make_summary("tx_c_d", "carol", "dave", 70.0, 0, 1.0),
    ];
    let remote = vec![
        make_summary("tx_a_e", "alice", "eve", 90.0, 0, 2.0),
    ];

    let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert!(report.transactions_rejected.contains(&"tx_a_b".to_string()),
        "alice->bob loses (lower weight)");
    assert!(report.transactions_rejected.contains(&"tx_b_c".to_string()),
        "bob->carol cascades (bob has no funds)");
    assert!(report.transactions_rejected.contains(&"tx_c_d".to_string()),
        "carol->dave cascades (carol has no funds)");
    assert!(report.transactions_kept.contains(&"tx_a_e".to_string()),
        "alice->eve wins");
}

#[test]
fn test_balance_conservation() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 500.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 500.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));
    accounts.insert("dave".to_string(), make_account("dave", 0.0, 0));

    let local = vec![
        make_summary("tx1", "alice", "carol", 100.0, 0, 3.0),
        make_summary("tx2", "alice", "dave", 100.0, 1, 1.0),
        make_summary("tx3", "bob", "carol", 200.0, 0, 1.0),
    ];
    let remote = vec![
        make_summary("tx4", "alice", "dave", 100.0, 0, 1.0),
        make_summary("tx5", "bob", "dave", 200.0, 0, 3.0),
    ];

    let (_, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

    let total_initial: u64 = accounts.values().map(|a| a.balance).sum();
    let total_final: u64 = rollback.final_balances_micro.values().sum();

    assert_eq!(
        total_initial, total_final,
        "Balance must be conserved: initial={}, final={}",
        from_micro_units(total_initial), from_micro_units(total_final)
    );
}

#[test]
fn test_no_negative_balances_after_merge() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 50.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 50.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));

    let local = vec![
        make_summary("tx_a1", "alice", "carol", 40.0, 0, 1.0),
        make_summary("tx_b1", "bob", "carol", 40.0, 0, 1.0),
    ];
    let remote = vec![
        make_summary("tx_a2", "alice", "carol", 40.0, 1, 1.0),
        make_summary("tx_b2", "bob", "carol", 40.0, 1, 1.0),
    ];

    let (_, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

    for (addr, &balance_micro) in &rollback.final_balances_micro {
        let balance = from_micro_units(balance_micro);
        assert!(
            balance >= 0.0,
            "Account {} has negative balance: {}",
            addr, balance
        );
    }
}

#[test]
fn test_surviving_nonce_continuity() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 1000.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 0.0, 0));

    let local = vec![
        make_summary("tx_n0", "alice", "bob", 10.0, 0, 1.0),
        make_summary("tx_n1", "alice", "bob", 10.0, 1, 1.0),
        make_summary("tx_n2", "alice", "bob", 10.0, 2, 1.0),
        make_summary("tx_n3", "alice", "bob", 10.0, 3, 1.0),
    ];
    let remote: Vec<PartitionTxSummary> = vec![];

    let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert_eq!(report.transactions_kept.len(), 4, "All txs should survive with no conflicts");
    assert!(report.transactions_rejected.is_empty());
}

#[test]
fn test_empty_partitions() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 100.0, 0));

    let local: Vec<PartitionTxSummary> = vec![];
    let remote: Vec<PartitionTxSummary> = vec![];

    let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert_eq!(report.phase, MergePhase::Complete);
    assert!(report.direct_conflicts.is_empty());
    assert!(report.economic_conflicts.is_empty());
    assert!(report.transactions_kept.is_empty());
    assert!(report.transactions_rejected.is_empty());
}

#[test]
fn test_one_sided_merge_local_only() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 100.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 0.0, 0));

    let local = vec![
        make_summary("tx1", "alice", "bob", 30.0, 0, 1.0),
        make_summary("tx2", "alice", "bob", 20.0, 1, 1.0),
    ];
    let remote: Vec<PartitionTxSummary> = vec![];

    let (report, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert_eq!(report.transactions_kept.len(), 2);
    assert!(report.transactions_rejected.is_empty());
    assert_eq!(from_micro_units(*rollback.final_balances_micro.get("alice").unwrap()), 50.0);
    assert_eq!(from_micro_units(*rollback.final_balances_micro.get("bob").unwrap()), 50.0);
}

#[test]
fn test_one_sided_merge_remote_only() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 100.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 0.0, 0));

    let local: Vec<PartitionTxSummary> = vec![];
    let remote = vec![
        make_summary("tx_r1", "alice", "bob", 25.0, 0, 1.0),
    ];

    let (report, rollback) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert_eq!(report.transactions_kept.len(), 1);
    assert!(report.transactions_rejected.is_empty());
    assert_eq!(from_micro_units(*rollback.final_balances_micro.get("alice").unwrap()), 75.0);
}

#[test]
fn test_partition_safety_classification() {
    use rinku_core::types::PartitionSafety;

    assert_eq!(TransactionKind::DataOnly.partition_safety(), PartitionSafety::Safe);
    assert_eq!(TransactionKind::Consolidation.partition_safety(), PartitionSafety::Safe);
    assert_eq!(TransactionKind::Reward.partition_safety(), PartitionSafety::Safe);

    assert_eq!(TransactionKind::Transfer.partition_safety(), PartitionSafety::BoundedSpend);
    assert_eq!(TransactionKind::Contract.partition_safety(), PartitionSafety::BoundedSpend);

    assert_eq!(TransactionKind::Stake.partition_safety(), PartitionSafety::CpOnly);
    assert_eq!(TransactionKind::Unstake.partition_safety(), PartitionSafety::CpOnly);
    assert_eq!(TransactionKind::ClaimRewards.partition_safety(), PartitionSafety::CpOnly);
}

#[test]
fn test_partition_budget_enforcement() {
    assert!(TransactionKind::Transfer.allowed_during_partition(None, to_micro_units(100.0)));
    assert!(TransactionKind::Transfer.allowed_during_partition(Some(to_micro_units(200.0)), to_micro_units(100.0)));
    assert!(!TransactionKind::Transfer.allowed_during_partition(Some(to_micro_units(50.0)), to_micro_units(100.0)));
    assert!(TransactionKind::Transfer.allowed_during_partition(Some(to_micro_units(100.0)), to_micro_units(100.0)));

    assert!(!TransactionKind::Stake.allowed_during_partition(None, 0));
    assert!(!TransactionKind::Stake.allowed_during_partition(Some(to_micro_units(1000.0)), 0));

    assert!(TransactionKind::DataOnly.allowed_during_partition(None, 0));
    assert!(TransactionKind::DataOnly.allowed_during_partition(Some(0), 0));
}

#[test]
fn test_penalty_constants_for_nonce_reuse() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_staked_account("alice", 1000.0, 0, 500.0));
    accounts.insert("bob".to_string(), make_account("bob", 0.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));

    let local = vec![
        make_summary("tx_local", "alice", "bob", 50.0, 0, 1.0),
    ];
    let remote = vec![
        make_summary("tx_remote", "alice", "carol", 50.0, 0, 2.0),
    ];

    let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert_eq!(report.direct_conflicts.len(), 1);

    let nonce_reuse_conflicts: Vec<_> = report.direct_conflicts.iter()
        .filter(|c| c.account == "alice")
        .collect();
    assert_eq!(nonce_reuse_conflicts.len(), 1);
}

#[test]
fn test_report_timing_metadata() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 100.0, 0));

    let local: Vec<PartitionTxSummary> = vec![];
    let remote: Vec<PartitionTxSummary> = vec![];

    let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert_eq!(report.phase, MergePhase::Complete);
    assert!(report.started_at_ms > 0);
    assert!(report.completed_at_ms.is_some());
    assert!(report.completed_at_ms.unwrap() >= report.started_at_ms);
    assert!(report.error.is_none());
}

#[test]
fn test_high_volume_merge_determinism() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 10000.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 10000.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));

    let mut local = Vec::new();
    for i in 0..50 {
        local.push(make_summary(
            &format!("tx_a_{}", i), "alice", "carol", 10.0, i, 1.0,
        ));
    }

    let mut remote = Vec::new();
    for i in 0..50 {
        remote.push(make_summary(
            &format!("tx_b_{}", i), "bob", "carol", 10.0, i, 1.0,
        ));
    }

    let (report1, rollback1) = run_full_merge_pipeline(&local, &remote, &accounts);
    let (report2, rollback2) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert_eq!(report1.transactions_kept, report2.transactions_kept);
    assert_eq!(report1.transactions_rejected, report2.transactions_rejected);
    assert_eq!(rollback1.final_balances_micro, rollback2.final_balances_micro);
}

#[test]
fn test_high_volume_with_conflicts_determinism() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 500.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 0.0, 0));
    accounts.insert("carol".to_string(), make_account("carol", 0.0, 0));

    let mut local = Vec::new();
    let mut remote = Vec::new();

    for i in 0..20 {
        local.push(make_summary(
            &format!("tx_l_{}", i), "alice", "bob", 10.0, i, 1.0 + (i % 3) as f64,
        ));
        remote.push(make_summary(
            &format!("tx_r_{}", i), "alice", "carol", 10.0, i, 2.0 + (i % 2) as f64,
        ));
    }

    let (r1, rb1) = run_full_merge_pipeline(&local, &remote, &accounts);
    let (r2, rb2) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert_eq!(r1.transactions_kept, r2.transactions_kept,
        "Merge results must be deterministic");
    assert_eq!(r1.transactions_rejected, r2.transactions_rejected);
    assert_eq!(rb1.final_balances_micro, rb2.final_balances_micro);

    for (addr, &balance_micro) in &rb1.final_balances_micro {
        assert!(balance_micro <= to_micro_units(500.0) || addr != "alice",
            "Alice cannot have more than starting balance");
    }
}

#[test]
fn test_micro_unit_conversion_roundtrip() {
    let values = [0.0, 1.0, 0.000001, 100.5, 29999999.999999, 0.1 + 0.2];
    for &v in &values {
        let micro = to_micro_units(v);
        let back = from_micro_units(micro);
        assert!(
            (back - v).abs() < 0.000002,
            "Roundtrip failed for {}: got {} (micro={})",
            v, back, micro
        );
    }
}

#[test]
fn test_identical_tx_in_both_partitions_not_conflict() {
    let mut accounts = HashMap::new();
    accounts.insert("alice".to_string(), make_account("alice", 100.0, 0));
    accounts.insert("bob".to_string(), make_account("bob", 0.0, 0));

    let tx = make_summary("same_hash", "alice", "bob", 10.0, 0, 1.0);
    let local = vec![tx.clone()];
    let remote = vec![tx];

    let (report, _) = run_full_merge_pipeline(&local, &remote, &accounts);

    assert!(report.direct_conflicts.is_empty(),
        "Same tx hash in both partitions should not be flagged as conflict");
}
