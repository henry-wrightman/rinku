use rinku_core::dag::Dag;
use rinku_core::types::{
    Account, Checkpoint, DagNode, SignedTransaction, Transaction, Validator, ValidatorSignature,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

fn compute_test_checkpoint_hash(
    height: u64,
    tx_merkle_root: &str,
    state_root: &str,
    receipt_root: &str,
    tip_count: u32,
    timestamp: u64,
) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(height.to_be_bytes());
    hasher.update(tx_merkle_root.as_bytes());
    hasher.update(state_root.as_bytes());
    hasher.update(receipt_root.as_bytes());
    hasher.update((tip_count as u64).to_be_bytes());
    hasher.update(timestamp.to_be_bytes());
    hasher.finalize().to_vec()
}

fn create_test_transaction(
    from: &str,
    to: &str,
    amount: u64,
    nonce: u64,
    parents: Vec<String>,
) -> SignedTransaction {
    let tx = Transaction {
        from: from.to_string(),
        to: to.to_string(),
        amount,
        nonce,
        timestamp: 1700000000000 + nonce,
        parents,
        kind: None,
        gas_limit: Some(21000),
        gas_price: Some(0),
        data: None,
        signature: None,
        memo: None,
        references: None,
    };

    let hash = format!("tx_{}_{}_{}_{}", from, to, nonce, amount);

    SignedTransaction {
        tx,
        hash,
        signature: "test_sig".to_string(),
    }
}

fn create_dag_node(tx: SignedTransaction, weight: f64) -> DagNode {
    DagNode {
        hash: tx.hash.clone(),
        tx: tx.clone(),
        parents: tx.tx.parents.clone(),
        children: Vec::new(),
        finalized: false,
        checkpoint_height: None,
        weight,
        received_at_ms: None,
        partition_epoch: None,
        provisional_finality: false,
        rolled_back: false,
    }
}

fn create_test_checkpoint(
    height: u64,
    previous_hash: Option<String>,
    tx_hashes: &[&str],
) -> Checkpoint {
    let tx_merkle_root = if tx_hashes.is_empty() {
        "empty".to_string()
    } else {
        format!("merkle_{}", tx_hashes.join("_"))
    };

    let hash_bytes = compute_test_checkpoint_hash(
        height,
        &tx_merkle_root,
        "state_root",
        "receipt_root",
        1,
        1700000000 + height,
    );

    Checkpoint {
        height,
        hash: hex::encode(&hash_bytes),
        previous_hash,
        tx_merkle_root,
        state_root: "state_root".to_string(),
        receipt_root: "receipt_root".to_string(),
        tip_count: 1,
        validator_signatures: vec![],
        timestamp: 1700000000 + height,
        aggregated_signature: None,
        signer_bitmap: None,
        finalized_tx_hashes: vec![],
        weight_trie_root: "weight_trie_root".to_string(),
        provisional: false,
        partition_epoch: None,
        visible_stake_pct: None,
        merge_report_hash: None,
    }
}

fn create_test_account(address: &str, balance: u64, nonce: u64) -> Account {
    Account {
        address: address.to_string(),
        balance,
        nonce,
        first_seen: 1700000000000,
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

#[derive(Clone)]
struct SimulatedNode {
    id: String,
    accounts: HashMap<String, Account>,
    checkpoints: Vec<Checkpoint>,
    dag: Vec<SignedTransaction>,
    total_supply: u64,
}

impl SimulatedNode {
    fn new(id: &str) -> Self {
        let mut accounts = HashMap::new();
        accounts.insert(
            "genesis".to_string(),
            create_test_account("genesis", 1_000_000, 0),
        );

        Self {
            id: id.to_string(),
            accounts,
            checkpoints: vec![],
            dag: vec![],
            total_supply: 1_000_000,
        }
    }

    fn add_transaction(&mut self, tx: SignedTransaction) {
        let fee = tx.tx.gas_price.unwrap_or(100_000) * tx.tx.gas_limit.unwrap_or(21000);
        let from_balance = self
            .accounts
            .get(&tx.tx.from)
            .map(|a| a.balance)
            .unwrap_or(0);

        if from_balance >= tx.tx.amount + fee {
            let from_acc = self
                .accounts
                .entry(tx.tx.from.clone())
                .or_insert(create_test_account(&tx.tx.from, 0, 0));
            from_acc.balance -= tx.tx.amount + fee;
            from_acc.nonce = tx.tx.nonce;

            let to_acc = self
                .accounts
                .entry(tx.tx.to.clone())
                .or_insert(create_test_account(&tx.tx.to, 0, 0));
            to_acc.balance += tx.tx.amount;

            self.dag.push(tx);
        }
    }

    fn create_checkpoint(&mut self) -> Checkpoint {
        let height = self.checkpoints.len() as u64 + 1;
        let previous_hash = self.checkpoints.last().map(|c| c.hash.clone());

        let tx_hashes: Vec<&str> = self.dag.iter().map(|t| t.hash.as_str()).collect();
        let checkpoint = create_test_checkpoint(height, previous_hash, &tx_hashes);
        self.checkpoints.push(checkpoint.clone());
        checkpoint
    }

    fn get_checkpoint_merkle_root(&self) -> Option<String> {
        self.checkpoints.last().map(|c| c.tx_merkle_root.clone())
    }

    fn get_account_balance(&self, address: &str) -> u64 {
        self.accounts.get(address).map(|a| a.balance).unwrap_or(0)
    }
}

mod fork_detection_tests {
    use super::*;

    #[test]
    fn test_identical_nodes_no_fork() {
        let mut node1 = SimulatedNode::new("node1");
        let mut node2 = SimulatedNode::new("node2");

        let tx = create_test_transaction("genesis", "alice", 100, 1, vec![]);
        node1.add_transaction(tx.clone());
        node2.add_transaction(tx);

        node1.create_checkpoint();
        node2.create_checkpoint();

        assert_eq!(
            node1.get_checkpoint_merkle_root(),
            node2.get_checkpoint_merkle_root(),
            "Nodes with same transactions should have matching merkle roots"
        );

        assert_eq!(
            node1.get_account_balance("alice"),
            node2.get_account_balance("alice"),
            "Account balances should match"
        );
    }

    #[test]
    fn test_different_transactions_causes_fork() {
        let mut node1 = SimulatedNode::new("node1");
        let mut node2 = SimulatedNode::new("node2");

        let tx1 = create_test_transaction("genesis", "alice", 100, 1, vec![]);
        let tx2 = create_test_transaction("genesis", "bob", 100, 1, vec![]);

        node1.add_transaction(tx1);
        node2.add_transaction(tx2);

        node1.create_checkpoint();
        node2.create_checkpoint();

        assert_ne!(
            node1.get_checkpoint_merkle_root(),
            node2.get_checkpoint_merkle_root(),
            "Nodes with different transactions should have different merkle roots"
        );
    }

    #[test]
    fn test_fork_detection_via_previous_hash_mismatch() {
        let mut node1 = SimulatedNode::new("node1");
        let mut node2 = SimulatedNode::new("node2");

        let tx1 = create_test_transaction("genesis", "alice", 100, 1, vec![]);
        node1.add_transaction(tx1.clone());
        node2.add_transaction(tx1);
        node1.create_checkpoint();
        node2.create_checkpoint();

        let tx2_node1 = create_test_transaction("genesis", "bob", 50, 2, vec![]);
        let tx2_node2 = create_test_transaction("genesis", "charlie", 50, 2, vec![]);

        node1.add_transaction(tx2_node1);
        node2.add_transaction(tx2_node2);

        let cp1 = node1.create_checkpoint();
        let cp2 = node2.create_checkpoint();

        assert_eq!(cp1.height, cp2.height, "Heights should match");
        assert_ne!(
            cp1.hash, cp2.hash,
            "Checkpoint hashes should differ (fork detected)"
        );
        assert_eq!(
            cp1.previous_hash, cp2.previous_hash,
            "Previous hashes should match (common ancestor)"
        );
    }

    #[test]
    fn test_consecutive_previous_hash_mismatches_triggers_recovery() {
        const FORK_RECOVERY_THRESHOLD: usize = 3;

        let mut node1 = SimulatedNode::new("node1");
        let mut node2 = SimulatedNode::new("node2");

        let tx = create_test_transaction("genesis", "alice", 100, 1, vec![]);
        node1.add_transaction(tx.clone());
        node2.add_transaction(tx);

        node1.create_checkpoint();
        node2.create_checkpoint();

        let mut mismatch_count = 0usize;

        for i in 2..=4 {
            let tx_node1 =
                create_test_transaction("genesis", "bob", 10 * i as u64, i as u64, vec![]);
            let tx_node2 =
                create_test_transaction("genesis", "charlie", 10 * i as u64, i as u64, vec![]);

            node1.add_transaction(tx_node1);
            node2.add_transaction(tx_node2);

            let cp1 = node1.create_checkpoint();
            let cp2 = node2.create_checkpoint();

            if cp1.previous_hash != cp2.previous_hash || cp1.hash != cp2.hash {
                mismatch_count += 1;
            }

            if mismatch_count >= FORK_RECOVERY_THRESHOLD {
                break;
            }
        }

        assert!(
            mismatch_count >= FORK_RECOVERY_THRESHOLD,
            "Should reach fork recovery threshold after {} consecutive mismatches",
            FORK_RECOVERY_THRESHOLD
        );
    }
}

mod snapshot_sync_tests {
    use super::*;

    #[test]
    fn test_snapshot_contains_all_accounts() {
        let mut node = SimulatedNode::new("node1");

        node.add_transaction(create_test_transaction(
            "genesis",
            "alice",
            100,
            1,
            vec![],
        ));
        node.add_transaction(create_test_transaction("genesis", "bob", 200, 2, vec![]));
        node.add_transaction(create_test_transaction(
            "genesis",
            "charlie",
            300,
            3,
            vec![],
        ));

        node.create_checkpoint();

        let snapshot_accounts = node.accounts.clone();

        assert!(snapshot_accounts.contains_key("genesis"));
        assert!(snapshot_accounts.contains_key("alice"));
        assert!(snapshot_accounts.contains_key("bob"));
        assert!(snapshot_accounts.contains_key("charlie"));
        assert_eq!(snapshot_accounts.len(), 4);
    }

    #[test]
    fn test_snapshot_sync_restores_balances() {
        let mut source_node = SimulatedNode::new("source");

        source_node.add_transaction(create_test_transaction(
            "genesis",
            "alice",
            100,
            1,
            vec![],
        ));
        source_node.add_transaction(create_test_transaction("genesis", "bob", 200, 2, vec![]));
        source_node.create_checkpoint();

        let mut new_node = SimulatedNode::new("new");
        new_node.accounts = source_node.accounts.clone();
        new_node.checkpoints = source_node.checkpoints.clone();

        assert_eq!(
            new_node.get_account_balance("alice"),
            source_node.get_account_balance("alice")
        );
        assert_eq!(
            new_node.get_account_balance("bob"),
            source_node.get_account_balance("bob")
        );
        assert_eq!(
            new_node.get_checkpoint_merkle_root(),
            source_node.get_checkpoint_merkle_root()
        );
    }

    #[test]
    fn test_snapshot_sync_without_full_transaction_history() {
        let mut source_node = SimulatedNode::new("source");

        for i in 1..=100 {
            source_node.add_transaction(create_test_transaction(
                "genesis",
                &format!("user{}", i % 10),
                1,
                i,
                vec![],
            ));
        }
        source_node.create_checkpoint();

        let mut new_node = SimulatedNode::new("new");
        new_node.accounts = source_node.accounts.clone();
        new_node.checkpoints = source_node.checkpoints.clone();
        new_node.dag = vec![];

        assert_eq!(
            new_node.accounts.len(),
            source_node.accounts.len(),
            "Account count should match without transaction history"
        );

        assert!(
            new_node.dag.is_empty(),
            "New node doesn't need full transaction history"
        );

        for i in 0..10 {
            let addr = format!("user{}", i);
            assert_eq!(
                new_node.get_account_balance(&addr),
                source_node.get_account_balance(&addr),
                "Balance for {} should match",
                addr
            );
        }
    }

    #[test]
    fn test_snapshot_preserves_checkpoint_chain() {
        let mut source_node = SimulatedNode::new("source");

        for round in 1..=5 {
            source_node.add_transaction(create_test_transaction(
                "genesis",
                "alice",
                10,
                round,
                vec![],
            ));
            source_node.create_checkpoint();
        }

        let mut new_node = SimulatedNode::new("new");
        new_node.checkpoints = source_node.checkpoints.clone();

        assert_eq!(new_node.checkpoints.len(), 5);

        for i in 1..new_node.checkpoints.len() {
            assert_eq!(
                new_node.checkpoints[i].previous_hash,
                Some(new_node.checkpoints[i - 1].hash.clone()),
                "Checkpoint chain should be properly linked"
            );
        }
    }
}

mod checkpoint_adoption_tests {
    use super::*;

    #[test]
    fn test_adopt_peer_checkpoint_with_matching_transactions() {
        let mut node1 = SimulatedNode::new("node1");
        let mut node2 = SimulatedNode::new("node2");

        let tx = create_test_transaction("genesis", "alice", 100, 1, vec![]);
        node1.add_transaction(tx.clone());
        node2.add_transaction(tx);

        let peer_checkpoint = node1.create_checkpoint();

        let local_pending_txs: Vec<&str> = node2.dag.iter().map(|t| t.hash.as_str()).collect();
        let local_merkle = format!("merkle_{}", local_pending_txs.join("_"));

        let can_adopt = peer_checkpoint.tx_merkle_root == local_merkle;

        assert!(
            can_adopt,
            "Should be able to adopt checkpoint with matching transactions"
        );

        if can_adopt {
            node2.checkpoints.push(peer_checkpoint);
        }

        assert_eq!(
            node1.get_checkpoint_merkle_root(),
            node2.get_checkpoint_merkle_root()
        );
    }

    #[test]
    fn test_reject_peer_checkpoint_with_different_transactions() {
        let mut node1 = SimulatedNode::new("node1");
        let mut node2 = SimulatedNode::new("node2");

        node1.add_transaction(create_test_transaction(
            "genesis",
            "alice",
            100,
            1,
            vec![],
        ));
        node2.add_transaction(create_test_transaction("genesis", "bob", 100, 1, vec![]));

        let peer_checkpoint = node1.create_checkpoint();

        let local_pending_txs: Vec<&str> = node2.dag.iter().map(|t| t.hash.as_str()).collect();
        let local_merkle = format!("merkle_{}", local_pending_txs.join("_"));

        let can_adopt = peer_checkpoint.tx_merkle_root == local_merkle;

        assert!(
            !can_adopt,
            "Should NOT adopt checkpoint with different transactions"
        );
    }

    #[test]
    fn test_checkpoint_chain_linkage_validation() {
        let mut node = SimulatedNode::new("node1");

        node.add_transaction(create_test_transaction(
            "genesis",
            "alice",
            100,
            1,
            vec![],
        ));
        let cp1 = node.create_checkpoint();

        node.add_transaction(create_test_transaction("genesis", "bob", 50, 2, vec![]));
        let cp2 = node.create_checkpoint();

        let valid_chain = cp2.previous_hash == Some(cp1.hash.clone());
        assert!(valid_chain, "Checkpoint 2 should link to checkpoint 1");

        let orphan_checkpoint = Checkpoint {
            height: 2,
            hash: "orphan_hash".to_string(),
            previous_hash: Some("nonexistent_parent".to_string()),
            tx_merkle_root: "orphan_merkle".to_string(),
            state_root: "state".to_string(),
            receipt_root: "receipt".to_string(),
            tip_count: 1,
            validator_signatures: vec![],
            timestamp: 1700000002,
            aggregated_signature: None,
            signer_bitmap: None,
            finalized_tx_hashes: vec![],
            weight_trie_root: "weight_trie_root".to_string(),
            provisional: false,
            partition_epoch: None,
            visible_stake_pct: None,
            merge_report_hash: None,
        };

        let can_adopt_orphan = orphan_checkpoint.previous_hash == Some(cp1.hash.clone());
        assert!(
            !can_adopt_orphan,
            "Should reject checkpoint with wrong previous_hash"
        );
    }

    #[test]
    fn test_checkpoint_hash_recomputation_validation() {
        let checkpoint = create_test_checkpoint(1, None, &["tx1", "tx2"]);

        let recomputed = compute_test_checkpoint_hash(
            checkpoint.height,
            &checkpoint.tx_merkle_root,
            &checkpoint.state_root,
            &checkpoint.receipt_root,
            checkpoint.tip_count,
            checkpoint.timestamp,
        );

        assert_eq!(
            hex::encode(&recomputed),
            checkpoint.hash,
            "Recomputed hash should match checkpoint hash"
        );

        let tampered_recompute = compute_test_checkpoint_hash(
            checkpoint.height,
            "tampered_merkle_root",
            &checkpoint.state_root,
            &checkpoint.receipt_root,
            checkpoint.tip_count,
            checkpoint.timestamp,
        );

        assert_ne!(
            hex::encode(&tampered_recompute),
            checkpoint.hash,
            "Tampered merkle root should produce different hash"
        );
    }
}

mod fork_resolution_tests {
    use super::*;

    #[test]
    fn test_dag_structure_and_tips() {
        let mut dag = Dag::new(1000);

        let genesis = create_test_transaction("genesis", "alice", 100, 0, vec![]);
        let genesis_node = create_dag_node(genesis.clone(), 1.0);
        dag.add_node(genesis_node).unwrap();

        assert_eq!(dag.tip_count(), 1, "Should have 1 tip after genesis");

        let branch_a = create_test_transaction("alice", "bob", 50, 1, vec![genesis.hash.clone()]);
        let branch_a_node = create_dag_node(branch_a.clone(), 1.0);
        dag.add_node(branch_a_node).unwrap();

        let branch_b =
            create_test_transaction("alice", "charlie", 50, 1, vec![genesis.hash.clone()]);
        let branch_b_node = create_dag_node(branch_b.clone(), 1.0);
        dag.add_node(branch_b_node).unwrap();

        assert_eq!(
            dag.tip_count(),
            2,
            "Should have 2 tips (two branches from genesis)"
        );
        assert!(dag.contains(&branch_a.hash), "DAG should contain branch A");
        assert!(dag.contains(&branch_b.hash), "DAG should contain branch B");
    }

    #[test]
    fn test_conflicting_nonces_detected() {
        let tx1 = create_test_transaction("alice", "bob", 50, 5, vec![]);
        let tx2 = create_test_transaction("alice", "charlie", 50, 5, vec![]);

        let has_conflict = tx1.tx.from == tx2.tx.from && tx1.tx.nonce == tx2.tx.nonce;

        assert!(
            has_conflict,
            "Same sender + same nonce = double-spend attempt"
        );
    }

    #[test]
    fn test_higher_weight_branch_wins() {
        let branch_a_weight = 5.0;
        let branch_b_weight = 3.0;

        let winner = if branch_a_weight > branch_b_weight {
            "A"
        } else {
            "B"
        };

        assert_eq!(winner, "A", "Higher weight branch should win");
    }

    #[test]
    fn test_weight_comparison_logic() {
        struct Branch {
            confirmations: usize,
            stake_weight: f64,
        }

        fn calculate_weight(branch: &Branch) -> f64 {
            branch.confirmations as f64 * branch.stake_weight
        }

        let branch_a = Branch {
            confirmations: 5,
            stake_weight: 1.0,
        };
        let branch_b = Branch {
            confirmations: 2,
            stake_weight: 1.0,
        };

        assert!(
            calculate_weight(&branch_a) > calculate_weight(&branch_b),
            "Branch with more confirmations should have higher weight"
        );

        let branch_c = Branch {
            confirmations: 2,
            stake_weight: 3.0,
        };
        let branch_d = Branch {
            confirmations: 5,
            stake_weight: 1.0,
        };

        assert!(
            calculate_weight(&branch_c) > calculate_weight(&branch_d),
            "Branch with higher stake weight can outweigh more confirmations"
        );
    }
}

mod delta_sync_tests {
    use super::*;

    #[test]
    fn test_delta_sync_fetches_missing_transactions() {
        let mut source_node = SimulatedNode::new("source");
        let mut stale_node = SimulatedNode::new("stale");

        let common_tx = create_test_transaction("genesis", "alice", 100, 1, vec![]);
        source_node.add_transaction(common_tx.clone());
        stale_node.add_transaction(common_tx);
        source_node.create_checkpoint();
        stale_node.create_checkpoint();

        let new_tx1 = create_test_transaction("genesis", "bob", 50, 2, vec![]);
        let new_tx2 = create_test_transaction("genesis", "charlie", 75, 3, vec![]);
        source_node.add_transaction(new_tx1.clone());
        source_node.add_transaction(new_tx2.clone());
        source_node.create_checkpoint();

        let source_height = source_node.checkpoints.len() as u64;
        let stale_height = stale_node.checkpoints.len() as u64;
        let needs_sync = stale_height < source_height;

        assert!(needs_sync, "Stale node should detect it's behind");

        let delta_txs: Vec<SignedTransaction> = source_node
            .dag
            .iter()
            .filter(|tx| !stale_node.dag.iter().any(|t| t.hash == tx.hash))
            .cloned()
            .collect();

        assert_eq!(delta_txs.len(), 2, "Should fetch 2 missing transactions");

        for tx in delta_txs {
            stale_node.add_transaction(tx);
        }
        stale_node.create_checkpoint();

        assert_eq!(
            stale_node.get_account_balance("bob"),
            source_node.get_account_balance("bob"),
            "After sync, balances should match"
        );
    }

    #[test]
    fn test_paginated_delta_sync() {
        let mut source_node = SimulatedNode::new("source");

        for i in 1..=100 {
            source_node.add_transaction(create_test_transaction(
                "genesis",
                &format!("user{}", i),
                1,
                i,
                vec![],
            ));
        }
        source_node.create_checkpoint();

        let page_size = 25;
        let total_txs = source_node.dag.len();
        let pages_needed = (total_txs + page_size - 1) / page_size;

        assert_eq!(pages_needed, 4, "Should need 4 pages for 100 transactions");

        let mut fetched_txs = Vec::new();
        for page in 0..pages_needed {
            let offset = page * page_size;
            let batch: Vec<SignedTransaction> = source_node
                .dag
                .iter()
                .skip(offset)
                .take(page_size)
                .cloned()
                .collect();

            let has_more = offset + page_size < total_txs;

            if page < pages_needed - 1 {
                assert!(has_more, "Should have more pages");
            } else {
                assert!(!has_more, "Last page should not have more");
            }

            fetched_txs.extend(batch);
        }

        assert_eq!(
            fetched_txs.len(),
            100,
            "Should have fetched all transactions"
        );
    }
}

mod network_partition_tests {
    use super::*;

    #[test]
    fn test_partition_creates_divergent_chains() {
        let mut node_partition_a = SimulatedNode::new("partition_a");
        let mut node_partition_b = SimulatedNode::new("partition_b");

        let common_tx = create_test_transaction("genesis", "alice", 100, 1, vec![]);
        node_partition_a.add_transaction(common_tx.clone());
        node_partition_b.add_transaction(common_tx);
        node_partition_a.create_checkpoint();
        node_partition_b.create_checkpoint();

        assert_eq!(
            node_partition_a.get_checkpoint_merkle_root(),
            node_partition_b.get_checkpoint_merkle_root(),
            "Pre-partition: nodes should be in sync"
        );

        for i in 2..=5 {
            let tx_a =
                create_test_transaction("genesis", &format!("user_a_{}", i), 10, i, vec![]);
            let tx_b =
                create_test_transaction("genesis", &format!("user_b_{}", i), 10, i, vec![]);

            node_partition_a.add_transaction(tx_a);
            node_partition_b.add_transaction(tx_b);

            node_partition_a.create_checkpoint();
            node_partition_b.create_checkpoint();
        }

        assert_ne!(
            node_partition_a.get_checkpoint_merkle_root(),
            node_partition_b.get_checkpoint_merkle_root(),
            "After partition: nodes should have diverged"
        );

        assert_ne!(
            node_partition_a.checkpoints.last().unwrap().hash,
            node_partition_b.checkpoints.last().unwrap().hash,
            "Checkpoint hashes should differ"
        );
    }

    #[test]
    fn test_partition_heal_requires_fork_resolution() {
        let mut nodes = vec![SimulatedNode::new("node1"), SimulatedNode::new("node2")];

        let common_tx = create_test_transaction("genesis", "alice", 100, 1, vec![]);
        for node in &mut nodes {
            node.add_transaction(common_tx.clone());
            node.create_checkpoint();
        }

        let partition_tx_0 = create_test_transaction("genesis", "bob", 50, 2, vec![]);
        let partition_tx_1 = create_test_transaction("genesis", "charlie", 75, 2, vec![]);

        nodes[0].add_transaction(partition_tx_0);
        nodes[1].add_transaction(partition_tx_1);

        nodes[0].create_checkpoint();
        nodes[1].create_checkpoint();

        let cp0 = nodes[0].checkpoints.last().unwrap();
        let cp1 = nodes[1].checkpoints.last().unwrap();

        let fork_detected = cp0.hash != cp1.hash;
        assert!(
            fork_detected,
            "Fork should be detected when partitions heal"
        );

        let winner_idx = 0;
        nodes[1].accounts = nodes[winner_idx].accounts.clone();
        nodes[1].checkpoints = nodes[winner_idx].checkpoints.clone();
        nodes[1].dag = nodes[winner_idx].dag.clone();

        assert_eq!(
            nodes[0].get_checkpoint_merkle_root(),
            nodes[1].get_checkpoint_merkle_root(),
            "After resolution: nodes should be in sync"
        );
    }
}
