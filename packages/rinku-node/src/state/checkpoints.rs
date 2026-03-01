use super::*;

impl NodeState {
    pub async fn get_checkpoint_height(&self) -> u64 {
        let state = self.inner.read().await;
        // CRITICAL: Return the actual height of the last checkpoint, NOT the count!
        // After pruning, len() can be 500 while actual height is 516+
        state.checkpoints.last()
            .map(|cp| cp.height)
            .unwrap_or(0)
    }
    
    /// Get the checkpoint height at which a transaction was finalized
    pub async fn get_tx_checkpoint_height(&self, tx_hash: &str) -> Option<u64> {
        let state = self.inner.read().await;
        // Check DAG node checkpoint_height
        if let Some(node) = state.dag.get_node(tx_hash) {
            return node.checkpoint_height;
        }
        None
    }
    
    /// Get a checkpoint by height
    pub async fn get_checkpoint_by_height(&self, height: u64) -> Option<rinku_core::types::Checkpoint> {
        let state = self.inner.read().await;
        state.checkpoints.iter().find(|cp| cp.height == height).cloned()
    }
    
    /// Get the latest checkpoint
    pub async fn get_latest_checkpoint(&self) -> Option<rinku_core::types::Checkpoint> {
        let state = self.inner.read().await;
        state.checkpoints.last().cloned()
    }
    
    /// Get the node's unique identifier
    pub async fn get_node_id(&self) -> String {
        // Use a hash of the genesis hash as a simple node identifier
        let state = self.inner.read().await;
        state.genesis_hash.clone().unwrap_or_else(|| "unknown".to_string())
    }
    
    /// Get accounts by a list of addresses (for P2P sync)
    pub async fn get_accounts_by_addresses(&self, addresses: &[String]) -> Vec<crate::network::AccountData> {
        let state = self.inner.read().await;
        addresses.iter()
            .filter_map(|addr| {
                state.accounts.get(addr).map(|a| crate::network::AccountData {
                    address: a.address.clone(),
                    balance: a.balance,
                    nonce: a.nonce,
                    stake: a.staked,
                })
            })
            .collect()
    }
    
    /// Apply a checkpoint received from the network (via CheckpointAnnouncement)
    /// 
    /// SAFETY: We finalize transactions ONLY if our unfinalized set's merkle root
    /// matches the checkpoint's tx_merkle_root. This ensures we have the same
    /// transaction set as the leader before finalizing.
    /// 
    /// In production, this should verify:
    /// 1. Validator signatures meet quorum threshold
    /// 2. The checkpoint merkle roots match expected state
    /// For now in testnet mode, we trust the checkpoint if prev_hash links correctly.
    pub async fn apply_checkpoint(&self, checkpoint: rinku_core::types::Checkpoint, _fast_path_executed: Option<&std::collections::HashSet<String>>) -> anyhow::Result<()> {
        use rinku_core::merkle::MerkleTree;
        
        let mut state = self.inner.write().await;
        
        // Validate this is the next expected checkpoint
        // CRITICAL: Use actual last checkpoint height, NOT len() which breaks after pruning
        let current_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
        let expected_height = current_height + 1;
        if checkpoint.height != expected_height {
            return Err(anyhow::anyhow!(
                "Checkpoint height mismatch: expected {}, got {} (current: {})",
                expected_height,
                checkpoint.height,
                current_height
            ));
        }
        
        // Validate prev_hash linkage (critical for chain integrity)
        if let Some(last_checkpoint) = state.checkpoints.last() {
            let expected_prev = &last_checkpoint.hash;
            let got_prev = checkpoint.previous_hash.as_deref().unwrap_or("");
            if got_prev != expected_prev {
                return Err(anyhow::anyhow!(
                    "Checkpoint prev_hash mismatch: expected {}, got {}",
                    &expected_prev[..16.min(expected_prev.len())],
                    &got_prev[..16.min(got_prev.len())]
                ));
            }
        }
        
        // Get our unfinalized transactions and compute merkle root
        // CRITICAL: Apply same propagation grace period as checkpoint creation
        // Only include transactions older than 5 seconds for merkle root calculation
        // This ensures leader and followers compute the same merkle root
        use crate::config::PROPAGATION_GRACE_MS;
        
        let now_ms_filter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let cutoff_time = now_ms_filter.saturating_sub(PROPAGATION_GRACE_MS);
        
        let mut unfinalized_hashes: Vec<String> = state
            .dag
            .get_unfinalized_nodes()
            .iter()
            .filter(|n| n.tx.tx.timestamp <= cutoff_time)
            .map(|n| n.hash.clone())
            .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
            .collect();
        
        // Sort for deterministic merkle root (must match leader's computation)
        unfinalized_hashes.sort();
        
        let our_merkle_root = if unfinalized_hashes.is_empty() {
            "0".repeat(64)
        } else {
            match MerkleTree::from_hex_leaves(&unfinalized_hashes) {
                Ok(tree) => tree.root(),
                Err(_) => "0".repeat(64),
            }
        };
        
        let height = checkpoint.height;
        let finalized_count;
        
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        
        // SAFE FINALIZATION: Only finalize if our transaction set matches the leader's
        // FINALITY-FIRST MODEL: Collect transactions for execution after marking finalized
        let mut txs_to_execute: Vec<SignedTransaction> = Vec::new();
        
        if our_merkle_root == checkpoint.tx_merkle_root && !unfinalized_hashes.is_empty() {
            // Our unfinalized set matches the checkpoint - safe to finalize
            for hash in &unfinalized_hashes {
                // Get transaction timestamp for finality time calculation
                if let Some(node) = state.dag.get_node(hash) {
                    // DOUBLE-EXECUTION GUARD: Skip already-finalized transactions
                    if node.finalized {
                        continue;
                    }
                    
                    let tx_timestamp = node.tx.tx.timestamp;
                    // FINALITY-FIRST: Clone transaction for execution (before mutable borrow)
                    let tx_clone = node.tx.clone();
                    
                    // Handle both seconds and milliseconds timestamp formats
                    let tx_time_ms = if tx_timestamp < 4_000_000_000 {
                        tx_timestamp * 1000  // Convert seconds to milliseconds
                    } else {
                        tx_timestamp
                    };
                    let finality_time_ms = now_ms.saturating_sub(tx_time_ms);
                    
                    // Cap finality time to 5 minutes (300s) to prevent sync/restore txs from polluting stats
                    const MAX_FINALITY_MS: u64 = 300_000;
                    if finality_time_ms <= MAX_FINALITY_MS {
                        state.finality_sum_ms += finality_time_ms;
                        state.finality_count += 1;
                        if finality_time_ms > state.finality_max_ms {
                            state.finality_max_ms = finality_time_ms;
                        }
                        if state.finality_times_ms.len() >= 1000 {
                            state.finality_times_ms.pop_front();
                        }
                        state.finality_times_ms.push_back(finality_time_ms);
                    }
                    // FINALITY-FIRST: Collect transaction for execution
                    txs_to_execute.push(tx_clone);
                }
                let _ = state.dag.mark_finalized(hash, height);
            }
            finalized_count = unfinalized_hashes.len();
            tracing::info!(
                "Applied checkpoint {} at height {} ({} txs finalized, merkle matched)",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                height,
                finalized_count
            );
        } else if our_merkle_root != checkpoint.tx_merkle_root && !unfinalized_hashes.is_empty() {
            // Merkle root mismatch - we have different transactions than the leader
            // Sync mechanism will reconcile the difference
            finalized_count = 0;
            tracing::info!(
                "Applied checkpoint {} at height {} (merkle mismatch: ours={} theirs={}, {} unfinalized)",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                height,
                &our_merkle_root[..16],
                &checkpoint.tx_merkle_root[..16.min(checkpoint.tx_merkle_root.len())],
                unfinalized_hashes.len()
            );
        } else {
            // No unfinalized transactions
            finalized_count = 0;
            tracing::info!(
                "Applied checkpoint {} at height {} (no unfinalized txs)",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                height
            );
        }
        
        // Add the checkpoint
        state.checkpoints.push(checkpoint.clone());
        state.last_checkpoint_time_ms = now_ms;
        
        // Release state lock before executing transactions
        drop(state);
        
        // DETERMINISTIC ORDERING: Sort transactions by hash before execution
        // to ensure all nodes apply balance changes in identical order,
        // preventing floating-point rounding divergence across nodes.
        txs_to_execute.sort_by(|a, b| a.hash.cmp(&b.hash));
        
        for tx in &txs_to_execute {
            self.execute_finalized_transaction(tx).await;
        }
        
        Ok(())
    }

    /// Apply a checkpoint received with its finalized transaction hashes
    /// This is the preferred method when receiving CheckpointAnnouncement from the leader
    /// because it allows finalizing transactions even if merkle roots don't match
    /// 
    /// Returns the number of missing transactions that the leader finalized but we don't have.
    /// This is CRITICAL for proof storage decisions - proofs should ONLY be stored when
    /// missing_tx_count == 0, otherwise the proof values won't match local account state.
    pub async fn apply_checkpoint_with_finalized_hashes(
        &self, 
        checkpoint: Checkpoint,
        finalized_tx_hashes: Vec<String>,
        _fast_path_executed: &std::collections::HashSet<String>,
    ) -> Result<usize> {
        let mut state = self.inner.write().await;
        
        // Validate checkpoint height
        // CRITICAL: Use actual checkpoint height, NOT len() which breaks after pruning
        let local_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
        if checkpoint.height <= local_height {
            return Err(anyhow::anyhow!(
                "Checkpoint height {} not greater than local height {}",
                checkpoint.height,
                local_height
            ));
        }
        
        let height = checkpoint.height;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        
        // FINALITY-FIRST MODEL: Collect transactions for execution after marking finalized
        let mut txs_to_execute: Vec<SignedTransaction> = Vec::new();
        
        // Track missing transactions - if we're missing any, our state differs from leader's
        let mut missing_tx_count = 0usize;
        
        // If leader provided finalized hashes, use them to finalize transactions
        // This solves the "merkle mismatch" problem where transactions stay pending
        let finalized_count = if !finalized_tx_hashes.is_empty() {
            let mut count = 0;
            let mut missing = 0;
            
            for hash in &finalized_tx_hashes {
                // Only finalize transactions we have in our DAG
                if let Some(node) = state.dag.get_node(hash) {
                    // DOUBLE-EXECUTION GUARD: Skip already-finalized transactions
                    if node.finalized {
                        continue;
                    }
                    
                    let tx_timestamp = node.tx.tx.timestamp;
                    // FINALITY-FIRST: Clone transaction for execution (before mutable borrow)
                    let tx_clone = node.tx.clone();
                    
                    // Handle both seconds and milliseconds timestamp formats
                    let tx_time_ms = if tx_timestamp < 4_000_000_000 {
                        tx_timestamp * 1000  // Convert seconds to milliseconds
                    } else {
                        tx_timestamp
                    };
                    let finality_time_ms = now_ms.saturating_sub(tx_time_ms);
                    
                    // Cap finality time to 5 minutes (300s) to prevent sync/restore txs from polluting stats
                    const MAX_FINALITY_MS: u64 = 300_000;
                    if finality_time_ms <= MAX_FINALITY_MS {
                        state.finality_sum_ms += finality_time_ms;
                        state.finality_count += 1;
                        if finality_time_ms > state.finality_max_ms {
                            state.finality_max_ms = finality_time_ms;
                        }
                        if state.finality_times_ms.len() >= 1000 {
                            state.finality_times_ms.pop_front();
                        }
                        state.finality_times_ms.push_back(finality_time_ms);
                    }
                    
                    // FINALITY-FIRST: Collect transaction for execution
                    txs_to_execute.push(tx_clone);
                    let _ = state.dag.mark_finalized(hash, height);
                    // Increment total_transactions at checkpoint finalization
                    state.total_transactions += 1;
                    count += 1;
                } else {
                    missing += 1;
                }
            }
            
            if missing > 0 {
                tracing::debug!(
                    "Checkpoint {} finalized {} txs, {} missing locally (will sync)",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    count, missing
                );
            }
            
            // Propagate missing count to function scope for proof generation decision
            missing_tx_count = missing;
            
            tracing::info!(
                "Applied checkpoint {} at height {} ({} of {} txs finalized from leader list)",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                height,
                count,
                finalized_tx_hashes.len()
            );
            
            count
        } else {
            // Fallback to old behavior if no hashes provided
            // (for backwards compatibility with older nodes)
            use crate::config::PROPAGATION_GRACE_MS;
            use rinku_core::merkle::MerkleTree;
            
            let now_ms_filter = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let cutoff_time = now_ms_filter.saturating_sub(PROPAGATION_GRACE_MS);
            
            let mut unfinalized_hashes: Vec<String> = state
                .dag
                .get_unfinalized_nodes()
                .iter()
                .filter(|n| n.tx.tx.timestamp <= cutoff_time)
                .map(|n| n.hash.clone())
                .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
                .collect();
            
            unfinalized_hashes.sort();
            
            let our_merkle_root = if unfinalized_hashes.is_empty() {
                "0".repeat(64)
            } else {
                match MerkleTree::from_hex_leaves(&unfinalized_hashes) {
                    Ok(tree) => tree.root(),
                    Err(_) => "0".repeat(64),
                }
            };
            
            if our_merkle_root == checkpoint.tx_merkle_root && !unfinalized_hashes.is_empty() {
                for hash in &unfinalized_hashes {
                    if let Some(node) = state.dag.get_node(hash) {
                        // DOUBLE-EXECUTION GUARD: Skip already-finalized transactions
                        if node.finalized {
                            continue;
                        }
                        
                        let tx_timestamp = node.tx.tx.timestamp;
                        // FINALITY-FIRST: Clone transaction for execution (before mutable borrow)
                        let tx_clone = node.tx.clone();
                        
                        // Handle both seconds and milliseconds timestamp formats
                        let tx_time_ms = if tx_timestamp < 4_000_000_000 {
                            tx_timestamp * 1000  // Convert seconds to milliseconds
                        } else {
                            tx_timestamp
                        };
                        let finality_time_ms = now_ms.saturating_sub(tx_time_ms);
                        
                        // Cap finality time to 5 minutes (300s) to prevent sync/restore txs from polluting stats
                        const MAX_FINALITY_MS: u64 = 300_000;
                        if finality_time_ms <= MAX_FINALITY_MS {
                            state.finality_sum_ms += finality_time_ms;
                            state.finality_count += 1;
                            if finality_time_ms > state.finality_max_ms {
                                state.finality_max_ms = finality_time_ms;
                            }
                            if state.finality_times_ms.len() >= 1000 {
                                state.finality_times_ms.pop_front();
                            }
                            state.finality_times_ms.push_back(finality_time_ms);
                        }
                        // FINALITY-FIRST: Collect transaction for execution
                        txs_to_execute.push(tx_clone);
                        // Increment total_transactions only upon finalization
                        state.total_transactions += 1;
                    }
                    let _ = state.dag.mark_finalized(hash, height);
                }
                tracing::info!(
                    "Applied checkpoint {} at height {} ({} txs finalized, merkle matched, no leader list)",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    height,
                    unfinalized_hashes.len()
                );
                unfinalized_hashes.len()
            } else {
                tracing::warn!(
                    "Applied checkpoint {} at height {} (no finalized txs - merkle mismatch and no leader list)",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    height
                );
                0
            }
        };
        
        // Add the checkpoint with finalized hashes
        let mut checkpoint_with_hashes = checkpoint.clone();
        if checkpoint_with_hashes.finalized_tx_hashes.is_empty() && !finalized_tx_hashes.is_empty() {
            checkpoint_with_hashes.finalized_tx_hashes = finalized_tx_hashes;
        }
        state.checkpoints.push(checkpoint_with_hashes);
        state.last_checkpoint_time_ms = now_ms;
        
        // Suppress unused variable warning
        let _ = finalized_count;
        
        // Release state lock before executing transactions
        drop(state);
        
        // DETERMINISTIC ORDERING: Sort transactions by hash before execution
        // to ensure all nodes apply balance changes in identical order,
        // preventing floating-point rounding divergence across nodes.
        txs_to_execute.sort_by(|a, b| a.hash.cmp(&b.hash));
        
        for tx in &txs_to_execute {
            self.execute_finalized_transaction(tx).await;
        }
        
        // FOLLOWER NODES DO NOT GENERATE PROOFS
        // Only the checkpoint LEADER can generate valid proofs because only they have the
        // exact simulated account set used to compute state_root. Followers receive checkpoints
        // with state_root but their account set may differ due to:
        // - Transaction propagation timing differences
        // - Activity bot transactions creating accounts on some nodes before others
        // - Different ordering of parallel transactions
        // 
        // Proof generation on followers would use the wrong account set and produce invalid proofs.
        // Users should query the leader node or wait for proofs to propagate via sync.
        if missing_tx_count > 0 {
            tracing::debug!(
                "Follower checkpoint {} - missing {} txs (proofs not generated, query leader)",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                missing_tx_count
            );
        } else {
            tracing::debug!(
                "Follower checkpoint {} applied successfully (proofs not generated, query leader)",
                &checkpoint.hash[..16.min(checkpoint.hash.len())]
            );
        }
        
        // Return missing_tx_count so caller can decide whether to store precomputed proofs
        // Proofs should ONLY be stored if missing_tx_count == 0, otherwise the proof values
        // (computed from leader's tx set) won't match local account state
        Ok(missing_tx_count)
    }

    pub async fn get_latest_checkpoint_id(&self) -> Option<String> {
        let state = self.inner.read().await;
        state.checkpoints.last().map(|c| c.hash.chars().take(16).collect())
    }
}
