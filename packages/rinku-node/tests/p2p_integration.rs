//! P2P Integration Tests for Rinku Node
//! 
//! Tests libp2p networking functionality including:
//! - DoS protection (rate limiting, peer banning, connection limits)
//! - Handshake validation (protocol version, chain/network IDs)
//! - Bloom filter operations
//! - Sync verification with merkle proofs

#[cfg(feature = "p2p")]
mod p2p_tests {
    use rinku_node::network::{
        DoSConfig, HandshakeConfig, NetworkConfig, 
        AccountData, SnapshotData, SyncRequest, SyncResponse,
        PeerHandshake,
    };
    use rinku_node::gossip::BloomFilter;
    use rinku_node::sync_verification::{
        SyncVerifier, VerificationResult,
        build_merkle_root, hash_account_leaf, verify_snapshot,
        generate_account_proofs, verify_account_proof,
    };

    // ============================================
    // DoS Protection Tests
    // ============================================

    #[test]
    fn test_dos_config_defaults() {
        let config = DoSConfig::default();
        
        assert_eq!(config.max_connections_per_peer, 50);
        assert_eq!(config.rate_limit_tokens_per_sec, 10);
        assert_eq!(config.rate_limit_burst, 20);
        assert_eq!(config.ban_duration_secs, 300);
    }

    #[test]
    fn test_dos_config_custom() {
        let config = DoSConfig {
            max_connections_per_peer: 100,
            rate_limit_tokens_per_sec: 20,
            rate_limit_burst: 40,
            ban_duration_secs: 600,
        };
        
        assert_eq!(config.max_connections_per_peer, 100);
        assert_eq!(config.ban_duration_secs, 600);
    }

    // ============================================
    // Handshake Configuration Tests
    // ============================================

    #[test]
    fn test_handshake_config_defaults() {
        let config = HandshakeConfig::default();
        
        assert_eq!(config.protocol_version, "1.0.0");
        assert_eq!(config.chain_id, "rinku-mainnet");
        assert_eq!(config.network_id, "mainnet");
        assert!(config.required_chain_id.is_none());
        assert!(config.required_network_id.is_none());
    }

    #[test]
    fn test_handshake_config_with_requirements() {
        let config = HandshakeConfig {
            protocol_version: "2.0.0".to_string(),
            chain_id: "rinku-testnet".to_string(),
            network_id: "testnet".to_string(),
            required_chain_id: Some("rinku-testnet".to_string()),
            required_network_id: Some("testnet".to_string()),
        };
        
        assert_eq!(config.required_chain_id, Some("rinku-testnet".to_string()));
        assert_eq!(config.required_network_id, Some("testnet".to_string()));
    }

    #[test]
    fn test_peer_handshake_serialization() {
        let handshake = PeerHandshake {
            protocol_version: "1.0.0".to_string(),
            chain_id: "rinku-mainnet".to_string(),
            network_id: "mainnet".to_string(),
            checkpoint_height: 100,
            validator_pubkey: Some("test_pubkey".to_string()),
            client_version: Some("rinku-node/0.1.0".to_string()),
        };
        
        let serialized = serde_json::to_string(&handshake).unwrap();
        let deserialized: PeerHandshake = serde_json::from_str(&serialized).unwrap();
        
        assert_eq!(deserialized.protocol_version, "1.0.0");
        assert_eq!(deserialized.checkpoint_height, 100);
    }

    // ============================================
    // Bloom Filter Tests
    // ============================================

    #[test]
    fn test_bloom_filter_in_mesh_context() {
        let mut node1_filter = BloomFilter::new();
        let mut node2_filter = BloomFilter::new();
        
        // Node 1 has transactions tx1, tx2, tx3
        node1_filter.insert("tx1");
        node1_filter.insert("tx2");
        node1_filter.insert("tx3");
        
        // Node 2 has transactions tx2, tx3, tx4
        node2_filter.insert("tx2");
        node2_filter.insert("tx3");
        node2_filter.insert("tx4");
        
        // Node 1 checks what Node 2 might have
        assert!(node2_filter.might_contain("tx2"), "Node 2 should have tx2");
        assert!(node2_filter.might_contain("tx3"), "Node 2 should have tx3");
        assert!(node2_filter.might_contain("tx4"), "Node 2 should have tx4");
        assert!(!node2_filter.might_contain("tx1"), "Node 2 should NOT have tx1 (definitively)");
        
        // Node 1 should send tx1 to Node 2 (it's not in Node 2's filter)
        let txs_to_send: Vec<&str> = vec!["tx1", "tx2", "tx3"]
            .into_iter()
            .filter(|tx| !node2_filter.might_contain(*tx))
            .collect();
        
        assert_eq!(txs_to_send.len(), 1);
        assert_eq!(txs_to_send[0], "tx1");
    }

    #[test]
    fn test_bloom_filter_false_positive_behavior() {
        let mut filter = BloomFilter::new();
        
        // Add many items
        for i in 0..10000 {
            filter.insert(&format!("tx_{}", i));
        }
        
        // Check false positive rate
        let fpr = filter.false_positive_rate();
        println!("False positive rate for 10K items: {:.4}%", fpr * 100.0);
        
        // Should be under 5% for default filter size
        assert!(fpr < 0.05, "FPR should be under 5%");
    }

    // ============================================
    // Sync Verification Tests
    // ============================================

    fn make_test_account(addr: &str, balance: f64, nonce: u64) -> AccountData {
        AccountData {
            address: addr.to_string(),
            balance,
            nonce,
            stake: 0.0,
        }
    }

    #[test]
    fn test_sync_verification_valid_snapshot() {
        let accounts = vec![
            make_test_account("alice", 1000.0, 5),
            make_test_account("bob", 500.0, 3),
            make_test_account("charlie", 250.0, 1),
        ];
        
        let hashes: Vec<String> = accounts.iter().map(hash_account_leaf).collect();
        let merkle_root = build_merkle_root(&hashes);
        
        let snapshot = SnapshotData {
            accounts,
            validators: vec![],
            checkpoints: vec![],
            recent_txs: vec![],
            merkle_root,
        };
        
        let result = verify_snapshot(&snapshot);
        assert_eq!(result, VerificationResult::Valid);
    }

    #[test]
    fn test_sync_verification_tampered_snapshot() {
        let accounts = vec![
            make_test_account("alice", 1000.0, 5),
            make_test_account("bob", 500.0, 3),
        ];
        
        let hashes: Vec<String> = accounts.iter().map(hash_account_leaf).collect();
        let merkle_root = build_merkle_root(&hashes);
        
        // Tamper with an account after computing root
        let mut tampered_accounts = accounts.clone();
        tampered_accounts[0].balance = 9999.0; // Changed!
        
        let snapshot = SnapshotData {
            accounts: tampered_accounts,
            validators: vec![],
            checkpoints: vec![],
            recent_txs: vec![],
            merkle_root, // Original root
        };
        
        let result = verify_snapshot(&snapshot);
        assert!(matches!(result, VerificationResult::Invalid(_)));
    }

    #[test]
    fn test_sync_verifier_strict_mode() {
        let accounts = vec![make_test_account("alice", 1000.0, 5)];
        let hashes: Vec<String> = accounts.iter().map(hash_account_leaf).collect();
        let merkle_root = build_merkle_root(&hashes);
        
        let valid_snapshot = SnapshotData {
            accounts: accounts.clone(),
            validators: vec![],
            checkpoints: vec![],
            recent_txs: vec![],
            merkle_root: merkle_root.clone(),
        };
        
        let invalid_snapshot = SnapshotData {
            accounts,
            validators: vec![],
            checkpoints: vec![],
            recent_txs: vec![],
            merkle_root: "wrong_root".to_string(),
        };
        
        // Strict mode - should fail on invalid
        let mut strict_verifier = SyncVerifier::new(true);
        assert!(strict_verifier.verify_snapshot(&valid_snapshot));
        assert!(!strict_verifier.verify_snapshot(&invalid_snapshot));
        
        // Non-strict mode - should pass even on invalid
        let mut lenient_verifier = SyncVerifier::new(false);
        assert!(lenient_verifier.verify_snapshot(&invalid_snapshot));
    }

    #[test]
    fn test_account_merkle_proofs_full_cycle() {
        let accounts = vec![
            make_test_account("alice", 1000.0, 5),
            make_test_account("bob", 500.0, 3),
            make_test_account("charlie", 250.0, 1),
            make_test_account("dave", 100.0, 0),
        ];
        
        let hashes: Vec<String> = accounts.iter().map(hash_account_leaf).collect();
        let root = build_merkle_root(&hashes);
        
        // Generate proofs for all accounts
        let proofs = generate_account_proofs(&accounts);
        
        assert_eq!(proofs.len(), 4);
        
        // Verify each proof
        for (i, proof) in proofs.iter().enumerate() {
            let result = verify_account_proof(proof, &root);
            assert_eq!(
                result, VerificationResult::Valid,
                "Proof for account {} should verify", 
                accounts[i].address
            );
        }
    }

    #[test]
    fn test_merkle_proof_tamper_detection() {
        let accounts = vec![
            make_test_account("alice", 1000.0, 5),
            make_test_account("bob", 500.0, 3),
        ];
        
        let hashes: Vec<String> = accounts.iter().map(hash_account_leaf).collect();
        let root = build_merkle_root(&hashes);
        
        let mut proofs = generate_account_proofs(&accounts);
        
        // Tamper with the proof
        proofs[0].balance = 9999.0;
        
        let result = verify_account_proof(&proofs[0], &root);
        assert!(matches!(result, VerificationResult::Invalid(_)));
    }

    // ============================================
    // Network Config Tests
    // ============================================

    #[test]
    fn test_network_config_creation() {
        let config = NetworkConfig {
            listen_addr: "/ip4/0.0.0.0/tcp/4001".to_string(),
            bootstrap_peers: vec![
                "/ip4/192.168.1.1/tcp/4001/p2p/QmPeer1".to_string(),
                "/ip4/192.168.1.2/tcp/4001/p2p/QmPeer2".to_string(),
            ],
            enable_mdns: true,
        };
        
        assert_eq!(config.bootstrap_peers.len(), 2);
        assert!(config.enable_mdns);
    }

    // ============================================
    // Sync Request/Response Tests
    // ============================================

    #[test]
    fn test_sync_request_serialization() {
        let requests = vec![
            SyncRequest::Snapshot,
            SyncRequest::Delta { from_checkpoint: 100 },
            SyncRequest::Transaction { hash: "tx123".to_string() },
            SyncRequest::Proof { tx_hash: "tx456".to_string() },
            SyncRequest::AccountsState { addresses: vec!["alice".to_string(), "bob".to_string()] },
        ];
        
        for req in requests {
            let serialized = serde_json::to_string(&req).unwrap();
            let deserialized: SyncRequest = serde_json::from_str(&serialized).unwrap();
            
            // Verify round-trip
            let re_serialized = serde_json::to_string(&deserialized).unwrap();
            assert_eq!(serialized, re_serialized);
        }
    }

    #[test]
    fn test_sync_response_error() {
        let error_response = SyncResponse::Error {
            message: "Peer is rate limited".to_string(),
        };
        
        let serialized = serde_json::to_string(&error_response).unwrap();
        let deserialized: SyncResponse = serde_json::from_str(&serialized).unwrap();
        
        match deserialized {
            SyncResponse::Error { message } => {
                assert_eq!(message, "Peer is rate limited");
            }
            _ => panic!("Expected Error response"),
        }
    }

    // ============================================
    // Multi-Node Mesh Simulation Tests
    // ============================================

    #[test]
    fn test_three_node_bloom_filter_propagation() {
        // Simulate 3 nodes in a mesh
        let mut node_filters: Vec<BloomFilter> = vec![
            BloomFilter::new(),
            BloomFilter::new(),
            BloomFilter::new(),
        ];
        
        // Node 0 has tx1, tx2
        node_filters[0].insert("tx1");
        node_filters[0].insert("tx2");
        
        // Node 1 has tx2, tx3
        node_filters[1].insert("tx2");
        node_filters[1].insert("tx3");
        
        // Node 2 has tx3, tx4
        node_filters[2].insert("tx3");
        node_filters[2].insert("tx4");
        
        // Simulate gossip: each node determines what to send to others
        fn determine_missing_txs(sender: &BloomFilter, receiver: &BloomFilter, sender_txs: &[&str]) -> Vec<String> {
            sender_txs.iter()
                .filter(|tx| sender.might_contain(*tx) && !receiver.might_contain(*tx))
                .map(|s| s.to_string())
                .collect()
        }
        
        // Node 0 -> Node 2: tx1 should be sent (tx2 might already be there via Node 1)
        let node0_txs = vec!["tx1", "tx2"];
        let to_send_0_to_2 = determine_missing_txs(&node_filters[0], &node_filters[2], &node0_txs);
        assert!(to_send_0_to_2.contains(&"tx1".to_string()));
        
        // After propagation, update Node 2's filter
        for tx in &to_send_0_to_2 {
            node_filters[2].insert(tx);
        }
        
        // Now Node 2 should have tx1
        assert!(node_filters[2].might_contain("tx1"));
    }

    #[test]
    fn test_sync_verification_summary() {
        let accounts = vec![
            make_test_account("alice", 1000.0, 5),
        ];
        let hashes: Vec<String> = accounts.iter().map(hash_account_leaf).collect();
        let merkle_root = build_merkle_root(&hashes);
        
        let snapshot = SnapshotData {
            accounts,
            validators: vec![],
            checkpoints: vec![],
            recent_txs: vec![],
            merkle_root,
        };
        
        let mut verifier = SyncVerifier::new(true);
        verifier.verify_snapshot(&snapshot);
        
        let summary = verifier.summary();
        assert!(summary.contains("1 valid"));
        assert!(summary.contains("0 invalid"));
        assert!(verifier.all_valid());
        assert!(verifier.failures().is_empty());
    }
}
