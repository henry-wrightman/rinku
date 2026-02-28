use std::collections::{HashMap, HashSet};
use rinku_core::types::Account;
use super::{DirectConflict, EconomicConflict, PartitionTxSummary};

pub fn detect_direct_conflicts(
    local_txs: &[PartitionTxSummary],
    remote_txs: &[PartitionTxSummary],
) -> Vec<DirectConflict> {
    let mut local_nonce_index: HashMap<(String, u64), String> = HashMap::new();
    for tx in local_txs {
        if tx.from == "genesis" {
            continue;
        }
        local_nonce_index.insert((tx.from.clone(), tx.nonce), tx.tx_hash.clone());
    }

    let mut conflicts = Vec::new();
    for tx in remote_txs {
        if tx.from == "genesis" {
            continue;
        }
        let key = (tx.from.clone(), tx.nonce);
        if let Some(local_hash) = local_nonce_index.get(&key) {
            if local_hash != &tx.tx_hash {
                conflicts.push(DirectConflict {
                    account: tx.from.clone(),
                    nonce: tx.nonce,
                    local_tx_hash: local_hash.clone(),
                    remote_tx_hash: tx.tx_hash.clone(),
                    local_partition_epoch: local_txs.iter()
                        .find(|lt| lt.tx_hash == *local_hash)
                        .and_then(|lt| lt.partition_epoch),
                    remote_partition_epoch: tx.partition_epoch,
                });
            }
        }
    }

    conflicts.sort_by(|a, b| {
        a.account.cmp(&b.account)
            .then(a.nonce.cmp(&b.nonce))
    });

    conflicts
}

pub fn detect_economic_conflicts(
    local_txs: &[PartitionTxSummary],
    remote_txs: &[PartitionTxSummary],
    fork_point_accounts: &HashMap<String, Account>,
) -> Vec<EconomicConflict> {
    let mut local_accounts: HashSet<String> = HashSet::new();
    let mut remote_accounts: HashSet<String> = HashSet::new();

    for tx in local_txs {
        if tx.from != "genesis" {
            local_accounts.insert(tx.from.clone());
        }
    }
    for tx in remote_txs {
        if tx.from != "genesis" {
            remote_accounts.insert(tx.from.clone());
        }
    }

    let cross_partition_accounts: HashSet<&String> = local_accounts.intersection(&remote_accounts).collect();
    if cross_partition_accounts.is_empty() {
        return Vec::new();
    }

    let mut conflicts = Vec::new();

    for account in &cross_partition_accounts {
        let pre_balance_micro = fork_point_accounts
            .get(*account)
            .map(|a| a.balance)
            .unwrap_or(0);

        let (total_sent_local, total_received_local, local_hashes) =
            compute_account_flows(local_txs, account);
        let (total_sent_remote, total_received_remote, remote_hashes) =
            compute_account_flows(remote_txs, account);

        let total_inflow = pre_balance_micro
            .saturating_add(total_received_local)
            .saturating_add(total_received_remote);
        let total_outflow = total_sent_local.saturating_add(total_sent_remote);

        if total_outflow > total_inflow {
            let deficit = total_outflow - total_inflow;
            conflicts.push(EconomicConflict {
                account: (*account).clone(),
                pre_partition_balance_micro: pre_balance_micro,
                total_sent_local_micro: total_sent_local,
                total_sent_remote_micro: total_sent_remote,
                total_received_local_micro: total_received_local,
                total_received_remote_micro: total_received_remote,
                deficit_micro: deficit,
                local_tx_hashes: local_hashes,
                remote_tx_hashes: remote_hashes,
            });
        }
    }

    conflicts.sort_by(|a, b| a.account.cmp(&b.account));

    conflicts
}

fn compute_account_flows(
    txs: &[PartitionTxSummary],
    account: &str,
) -> (u64, u64, Vec<String>) {
    let mut total_sent: u64 = 0;
    let mut total_received: u64 = 0;
    let mut hashes = Vec::new();

    for tx in txs {
        if tx.from == account {
            total_sent = total_sent.saturating_add(tx.amount_micro).saturating_add(tx.gas_micro);
            hashes.push(tx.tx_hash.clone());
        }
        if tx.to == account && tx.from != account {
            total_received = total_received.saturating_add(tx.amount_micro);
        }
    }

    (total_sent, total_received, hashes)
}

pub fn detect_all_conflicts(
    local_txs: &[PartitionTxSummary],
    remote_txs: &[PartitionTxSummary],
    fork_point_accounts: &HashMap<String, Account>,
) -> (Vec<DirectConflict>, Vec<EconomicConflict>) {
    let direct = detect_direct_conflicts(local_txs, remote_txs);
    let economic = detect_economic_conflicts(local_txs, remote_txs, fork_point_accounts);
    (direct, economic)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::merge::PartitionTxSummary;
    use rinku_core::types::to_micro_units;

    fn make_tx(hash: &str, from: &str, to: &str, amount: f64, nonce: u64, weight: f64) -> PartitionTxSummary {
        PartitionTxSummary {
            tx_hash: hash.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            amount_micro: to_micro_units(amount),
            gas_micro: to_micro_units(0.001),
            nonce,
            weight,
            partition_epoch: Some(1),
            visible_stake_pct: 0.5,
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
        }
    }

    #[test]
    fn test_direct_conflict_same_nonce() {
        let local = vec![make_tx("tx_a", "alice", "bob", 10.0, 5, 1.0)];
        let remote = vec![make_tx("tx_b", "alice", "carol", 10.0, 5, 1.0)];

        let conflicts = detect_direct_conflicts(&local, &remote);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].account, "alice");
        assert_eq!(conflicts[0].nonce, 5);
        assert_eq!(conflicts[0].local_tx_hash, "tx_a");
        assert_eq!(conflicts[0].remote_tx_hash, "tx_b");
    }

    #[test]
    fn test_no_conflict_different_nonces() {
        let local = vec![make_tx("tx_a", "alice", "bob", 10.0, 5, 1.0)];
        let remote = vec![make_tx("tx_b", "alice", "carol", 10.0, 6, 1.0)];

        let conflicts = detect_direct_conflicts(&local, &remote);
        assert_eq!(conflicts.len(), 0);
    }

    #[test]
    fn test_same_tx_hash_not_flagged() {
        let local = vec![make_tx("tx_same", "alice", "bob", 10.0, 5, 1.0)];
        let remote = vec![make_tx("tx_same", "alice", "bob", 10.0, 5, 1.0)];

        let conflicts = detect_direct_conflicts(&local, &remote);
        assert_eq!(conflicts.len(), 0);
    }

    #[test]
    fn test_economic_overdraft() {
        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), make_account("alice", 100.0));

        let local = vec![make_tx("tx_a", "alice", "bob", 60.0, 5, 1.0)];
        let remote = vec![make_tx("tx_b", "alice", "carol", 60.0, 6, 1.0)];

        let conflicts = detect_economic_conflicts(&local, &remote, &accounts);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].account, "alice");
        assert!(conflicts[0].deficit_micro > 0);
    }

    #[test]
    fn test_no_economic_overdraft_when_sufficient() {
        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), make_account("alice", 200.0));

        let local = vec![make_tx("tx_a", "alice", "bob", 60.0, 5, 1.0)];
        let remote = vec![make_tx("tx_b", "alice", "carol", 60.0, 6, 1.0)];

        let conflicts = detect_economic_conflicts(&local, &remote, &accounts);
        assert_eq!(conflicts.len(), 0);
    }

    #[test]
    fn test_economic_conflict_accounts_received_funds() {
        let mut accounts = HashMap::new();
        accounts.insert("alice".to_string(), make_account("alice", 50.0));

        let local = vec![
            make_tx("tx_fund", "bob", "alice", 30.0, 1, 1.0),
            make_tx("tx_a", "alice", "carol", 60.0, 5, 1.0),
        ];
        let remote = vec![
            make_tx("tx_b", "alice", "dave", 60.0, 6, 1.0),
        ];

        let conflicts = detect_economic_conflicts(&local, &remote, &accounts);
        assert_eq!(conflicts.len(), 1);
        let c = &conflicts[0];
        assert_eq!(c.account, "alice");
        assert!(c.total_received_local_micro > 0);
    }
}
