use rinku_core::types::{Account, WriteSet, WriteSetEntry};
use rinku_core::crypto::sha256_hex;
use std::collections::HashMap;

pub fn compute_transfer_write_set(
    from_addr: &str,
    _to_addr: &str,
    from_account: &Account,
    _to_account: &Account,
) -> WriteSet {
    let from_value = format!(
        "account:{}:{}:{}:{}",
        from_addr, from_account.balance, from_account.nonce, from_account.staked
    );

    let entries = vec![
        WriteSetEntry {
            key: from_addr.to_string(),
            value_hash: sha256_hex(&from_value),
        },
    ];

    WriteSet::from_entries(entries)
}

pub fn compute_stake_write_set(
    staker_addr: &str,
    staker_account: &Account,
) -> WriteSet {
    let value = format!(
        "account:{}:{}:{}:{}",
        staker_addr, staker_account.balance, staker_account.nonce, staker_account.staked
    );

    let entries = vec![WriteSetEntry {
        key: staker_addr.to_string(),
        value_hash: sha256_hex(&value),
    }];

    WriteSet::from_entries(entries)
}

pub fn compute_accounts_write_set(
    changed: &[(String, Account)],
) -> WriteSet {
    let entries: Vec<WriteSetEntry> = changed
        .iter()
        .map(|(addr, acc)| {
            let value = format!(
                "account:{}:{}:{}:{}",
                addr, acc.balance, acc.nonce, acc.staked
            );
            WriteSetEntry {
                key: addr.clone(),
                value_hash: sha256_hex(&value),
            }
        })
        .collect();

    WriteSet::from_entries(entries)
}

pub fn compute_contract_write_set(
    contract_id: &str,
    changes: &[(String, Option<String>)],
    from_addr: &str,
    from_account: &Account,
) -> WriteSet {
    let mut entries: Vec<WriteSetEntry> = Vec::new();

    for (key, new_value) in changes {
        let state_key = format!("contract:{}:{}", contract_id, key);
        let value_repr = match new_value {
            Some(v) => format!("contract_state:{}:{}:{}", contract_id, key, v),
            None => format!("contract_state:{}:{}:DELETED", contract_id, key),
        };
        entries.push(WriteSetEntry {
            key: state_key,
            value_hash: sha256_hex(&value_repr),
        });
    }

    let from_value = format!(
        "account:{}:{}:{}:{}",
        from_addr, from_account.balance, from_account.nonce, from_account.staked
    );
    entries.push(WriteSetEntry {
        key: from_addr.to_string(),
        value_hash: sha256_hex(&from_value),
    });

    WriteSet::from_entries(entries)
}

pub struct WriteSetConflictTracker {
    in_flight: HashMap<String, Vec<(String, String)>>,
}

impl WriteSetConflictTracker {
    pub fn new() -> Self {
        Self {
            in_flight: HashMap::new(),
        }
    }

    pub fn check_and_register(&mut self, tx_hash: &str, sender: &str, write_set: &WriteSet) -> bool {
        let mut has_cross_sender_conflict = false;
        for entry in &write_set.entries {
            if let Some(existing) = self.in_flight.get(&entry.key) {
                if existing.iter().any(|(_, s)| s != sender) {
                    has_cross_sender_conflict = true;
                    break;
                }
            }
        }
        if has_cross_sender_conflict {
            return true;
        }
        for entry in &write_set.entries {
            self.in_flight
                .entry(entry.key.clone())
                .or_default()
                .push((tx_hash.to_string(), sender.to_string()));
        }
        false
    }

    pub fn remove(&mut self, tx_hash: &str) {
        self.in_flight.retain(|_key, txs| {
            txs.retain(|(h, _)| h != tx_hash);
            !txs.is_empty()
        });
    }

    pub fn clear_all(&mut self) {
        self.in_flight.clear();
    }

    pub fn in_flight_key_count(&self) -> usize {
        self.in_flight.len()
    }

    pub fn in_flight_tx_count(&self) -> usize {
        let mut txs: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for v in self.in_flight.values() {
            for (h, _) in v {
                txs.insert(h.as_str());
            }
        }
        txs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_account(balance: u64, nonce: u64, staked: u64) -> Account {
        Account {
            address: String::new(),
            balance,
            nonce,
            first_seen: 0,
            staked,
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
    fn test_transfer_write_set_deterministic() {
        let from = test_account(1000, 5, 0);
        let to = test_account(500, 0, 0);

        let ws1 = compute_transfer_write_set("alice", "bob", &from, &to);
        let ws2 = compute_transfer_write_set("alice", "bob", &from, &to);
        assert_eq!(ws1.hash, ws2.hash);
        assert_eq!(ws1.entries.len(), 1);
    }

    #[test]
    fn test_write_set_conflict_detection() {
        let ws_ab = WriteSet::from_entries(vec![
            WriteSetEntry { key: "alice".to_string(), value_hash: "h1".to_string() },
            WriteSetEntry { key: "bob".to_string(), value_hash: "h2".to_string() },
        ]);
        let ws_bc = WriteSet::from_entries(vec![
            WriteSetEntry { key: "bob".to_string(), value_hash: "h3".to_string() },
            WriteSetEntry { key: "charlie".to_string(), value_hash: "h4".to_string() },
        ]);
        let ws_cd = WriteSet::from_entries(vec![
            WriteSetEntry { key: "charlie".to_string(), value_hash: "h5".to_string() },
            WriteSetEntry { key: "dave".to_string(), value_hash: "h6".to_string() },
        ]);

        assert!(ws_ab.conflicts_with(&ws_bc));
        assert!(!ws_ab.conflicts_with(&ws_cd));
    }

    #[test]
    fn test_conflict_tracker_cross_sender() {
        let mut tracker = WriteSetConflictTracker::new();

        let ws1 = WriteSet::from_entries(vec![
            WriteSetEntry { key: "alice".to_string(), value_hash: "h1".to_string() },
            WriteSetEntry { key: "bob".to_string(), value_hash: "h2".to_string() },
        ]);
        let ws2 = WriteSet::from_entries(vec![
            WriteSetEntry { key: "bob".to_string(), value_hash: "h3".to_string() },
            WriteSetEntry { key: "charlie".to_string(), value_hash: "h4".to_string() },
        ]);
        let ws3 = WriteSet::from_entries(vec![
            WriteSetEntry { key: "dave".to_string(), value_hash: "h5".to_string() },
        ]);

        assert!(!tracker.check_and_register("tx1", "sender_a", &ws1));
        assert!(tracker.check_and_register("tx2", "sender_b", &ws2));
        assert!(!tracker.check_and_register("tx3", "sender_c", &ws3));

        tracker.remove("tx1");
        assert!(!tracker.check_and_register("tx2_retry", "sender_b", &ws2));
    }

    #[test]
    fn test_conflict_tracker_same_sender_no_conflict() {
        let mut tracker = WriteSetConflictTracker::new();

        let ws1 = WriteSet::from_entries(vec![
            WriteSetEntry { key: "faucet".to_string(), value_hash: "h1".to_string() },
            WriteSetEntry { key: "recipient_1".to_string(), value_hash: "h2".to_string() },
        ]);
        let ws2 = WriteSet::from_entries(vec![
            WriteSetEntry { key: "faucet".to_string(), value_hash: "h3".to_string() },
            WriteSetEntry { key: "recipient_2".to_string(), value_hash: "h4".to_string() },
        ]);
        let ws3 = WriteSet::from_entries(vec![
            WriteSetEntry { key: "faucet".to_string(), value_hash: "h5".to_string() },
            WriteSetEntry { key: "recipient_3".to_string(), value_hash: "h6".to_string() },
        ]);

        assert!(!tracker.check_and_register("tx1", "faucet", &ws1));
        assert!(!tracker.check_and_register("tx2", "faucet", &ws2));
        assert!(!tracker.check_and_register("tx3", "faucet", &ws3));
    }

    #[test]
    fn test_conflict_tracker_cross_sender_same_recipient() {
        let mut tracker = WriteSetConflictTracker::new();

        let ws1 = WriteSet::from_entries(vec![
            WriteSetEntry { key: "alice".to_string(), value_hash: "h1".to_string() },
            WriteSetEntry { key: "shared_recipient".to_string(), value_hash: "h2".to_string() },
        ]);
        let ws2 = WriteSet::from_entries(vec![
            WriteSetEntry { key: "bob".to_string(), value_hash: "h3".to_string() },
            WriteSetEntry { key: "shared_recipient".to_string(), value_hash: "h4".to_string() },
        ]);

        assert!(!tracker.check_and_register("tx1", "alice", &ws1));
        assert!(tracker.check_and_register("tx2", "bob", &ws2));
    }
}
