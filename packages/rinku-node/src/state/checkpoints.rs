use super::*;
use base64::Engine;
use crate::bls::verify_aggregated_checkpoint_signature;

pub enum BlsVerifyResult {
    NoSignature,
    ValidWithQuorum,
    ValidNoQuorum { signer_stake: u64, total_stake: u64 },
    Invalid(String),
}

impl NodeState {
    /// Verify BLS aggregate signature on a checkpoint against the known validator set.
    /// Returns Ok(()) if valid, or Err with reason if invalid.
    /// Skips verification if the checkpoint has no aggregated signature (legacy/testnet).
    ///
    /// `sorted_validator_bls_keys_and_stakes` must be sorted by address (same order as
    /// the validator set used during signing) and provides (bls_public_key, effective_stake)
    /// pairs for stake-weighted quorum checking.
    pub fn verify_checkpoint_bls(
        checkpoint: &rinku_core::types::Checkpoint,
        sorted_validator_bls_keys_and_stakes: &[(Vec<u8>, u64)],
    ) -> anyhow::Result<()> {
        let agg_sig_b64 = match &checkpoint.aggregated_signature {
            Some(s) if !s.is_empty() => s.clone(),
            _ => {
                tracing::warn!(
                    "Checkpoint {} at height {} has no BLS aggregate signature — skipping verification",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    checkpoint.height,
                );
                return Ok(());
            }
        };
        let signer_bitmap = match &checkpoint.signer_bitmap {
            Some(bm) if !bm.is_empty() => bm.clone(),
            _ => {
                return Err(anyhow::anyhow!(
                    "Checkpoint {} has aggregated_signature but no signer_bitmap",
                    checkpoint.height
                ));
            }
        };

        let agg_sig_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&agg_sig_b64)
            .or_else(|_| base64::engine::general_purpose::STANDARD.decode(&agg_sig_b64))
            .map_err(|e| anyhow::anyhow!("Invalid base64 in aggregated_signature: {}", e))?;

        if sorted_validator_bls_keys_and_stakes.is_empty() {
            return Err(anyhow::anyhow!(
                "No validator BLS keys available for checkpoint {} verification — cannot verify aggregate signature",
                checkpoint.height
            ));
        }

        let signer_indices = crate::bls::parse_signer_bitmap(
            &signer_bitmap,
            sorted_validator_bls_keys_and_stakes.len(),
        );

        let total_stake: u64 = sorted_validator_bls_keys_and_stakes.iter().map(|(_, s)| *s).sum();
        let signer_stake: u64 = signer_indices.iter()
            .filter_map(|&i| sorted_validator_bls_keys_and_stakes.get(i).map(|(_, s)| *s))
            .sum();

        let quorum_met = if total_stake > 0 {
            (signer_stake as f64 / total_stake as f64) >= crate::consensus::QUORUM_THRESHOLD
        } else {
            false
        };

        if !quorum_met {
            return Err(anyhow::anyhow!(
                "Checkpoint {} stake quorum not met: signer_stake={} / total_stake={} ({:.2}%, need {:.2}%)",
                checkpoint.height, signer_stake, total_stake,
                if total_stake > 0 { signer_stake as f64 / total_stake as f64 * 100.0 } else { 0.0 },
                crate::consensus::QUORUM_THRESHOLD * 100.0,
            ));
        }

        let bls_keys_only: Vec<Vec<u8>> = sorted_validator_bls_keys_and_stakes
            .iter()
            .map(|(k, _)| k.clone())
            .collect();

        let checkpoint_hash_bytes = match hex::decode(&checkpoint.hash) {
            Ok(b) => b,
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Checkpoint {} at height {} has non-hex hash — cannot verify BLS: {}",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    checkpoint.height,
                    e
                ));
            }
        };
        let valid = verify_aggregated_checkpoint_signature(
            &checkpoint_hash_bytes,
            &agg_sig_bytes,
            &signer_bitmap,
            &bls_keys_only,
        );

        if !valid {
            return Err(anyhow::anyhow!(
                "BLS aggregate signature verification FAILED for checkpoint {} at height {}",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                checkpoint.height
            ));
        }

        tracing::debug!(
            "BLS signature verified for checkpoint {} ({}/{} signers, {:.1}% stake)",
            checkpoint.height, signer_indices.len(), sorted_validator_bls_keys_and_stakes.len(),
            signer_stake as f64 / total_stake as f64 * 100.0,
        );
        Ok(())
    }

    pub fn verify_checkpoint_bls_signature_only(
        checkpoint: &rinku_core::types::Checkpoint,
        sorted_validator_bls_keys_and_stakes: &[(Vec<u8>, u64)],
    ) -> BlsVerifyResult {
        let agg_sig_b64 = match &checkpoint.aggregated_signature {
            Some(s) if !s.is_empty() => s.clone(),
            _ => {
                return BlsVerifyResult::NoSignature;
            }
        };
        let signer_bitmap = match &checkpoint.signer_bitmap {
            Some(bm) if !bm.is_empty() => bm.clone(),
            _ => {
                return BlsVerifyResult::Invalid(format!(
                    "Checkpoint {} has aggregated_signature but no signer_bitmap",
                    checkpoint.height
                ));
            }
        };

        let agg_sig_bytes = match base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&agg_sig_b64)
            .or_else(|_| base64::engine::general_purpose::STANDARD.decode(&agg_sig_b64))
        {
            Ok(b) => b,
            Err(e) => return BlsVerifyResult::Invalid(format!("Invalid base64: {}", e)),
        };

        if sorted_validator_bls_keys_and_stakes.is_empty() {
            return BlsVerifyResult::Invalid(format!(
                "No validator BLS keys available for checkpoint {}",
                checkpoint.height
            ));
        }

        let signer_indices = crate::bls::parse_signer_bitmap(
            &signer_bitmap,
            sorted_validator_bls_keys_and_stakes.len(),
        );

        if signer_indices.is_empty() {
            return BlsVerifyResult::Invalid(format!(
                "Checkpoint {} has no signers in bitmap",
                checkpoint.height
            ));
        }

        let bls_keys_only: Vec<Vec<u8>> = sorted_validator_bls_keys_and_stakes
            .iter()
            .map(|(k, _)| k.clone())
            .collect();

        let checkpoint_hash_bytes = match hex::decode(&checkpoint.hash) {
            Ok(b) => b,
            Err(e) => {
                return BlsVerifyResult::Invalid(format!(
                    "Checkpoint {} at height {} has non-hex hash — cannot verify BLS: {}",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    checkpoint.height,
                    e
                ));
            }
        };
        let valid = crate::bls::verify_aggregated_checkpoint_signature(
            &checkpoint_hash_bytes,
            &agg_sig_bytes,
            &signer_bitmap,
            &bls_keys_only,
        );

        if !valid {
            return BlsVerifyResult::Invalid(format!(
                "BLS aggregate signature verification FAILED for checkpoint {} at height {}",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                checkpoint.height
            ));
        }

        let total_stake: u64 = sorted_validator_bls_keys_and_stakes.iter().map(|(_, s)| *s).sum();
        let signer_stake: u64 = signer_indices.iter()
            .filter_map(|&i| sorted_validator_bls_keys_and_stakes.get(i).map(|(_, s)| *s))
            .sum();

        let quorum_met = total_stake > 0
            && (signer_stake as f64 / total_stake as f64) >= crate::consensus::QUORUM_THRESHOLD;

        if quorum_met {
            BlsVerifyResult::ValidWithQuorum
        } else {
            BlsVerifyResult::ValidNoQuorum {
                signer_stake,
                total_stake,
            }
        }
    }

    pub fn get_checkpoint_height(&self) -> u64 {
        self.checkpoint_height_cache.load(std::sync::atomic::Ordering::Relaxed)
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
    /// BLS signature verification: Callers MUST call `verify_checkpoint_bls()` before
    /// invoking this method when validator BLS keys are available. This method validates
    /// prev_hash chain linkage and merkle root consistency but does not verify BLS
    /// signatures itself (it lacks access to the validator identity service).
    /// Apply a checkpoint (legacy merkle-match path).
    /// LOCK-CONSOLIDATED: Same single-lock pattern as apply_checkpoint_with_finalized_hashes.
    pub async fn apply_checkpoint(&self, checkpoint: rinku_core::types::Checkpoint) -> anyhow::Result<()> {
        use rinku_core::merkle::MerkleTree;

        let batch_start = std::time::Instant::now();

        let unfinalized_hashes = {
            let state = self.inner.read().await;

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

            use crate::config::PROPAGATION_GRACE_MS;

            let now_ms_filter = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let cutoff_time = now_ms_filter.saturating_sub(PROPAGATION_GRACE_MS);

            let mut hashes: Vec<String> = state
                .dag
                .get_unfinalized_nodes()
                .iter()
                .filter(|n| n.tx.tx.timestamp <= cutoff_time)
                .map(|n| n.hash.clone())
                .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
                .collect();

            hashes.sort();
            hashes
        };

        let our_merkle_root = if unfinalized_hashes.is_empty() {
            "0".repeat(64)
        } else {
            let hashes_clone = unfinalized_hashes.clone();
            match tokio::task::spawn_blocking(move || MerkleTree::from_hex_leaves(&hashes_clone).map(|t| t.root())).await {
                Ok(Ok(root)) => root,
                _ => "0".repeat(64),
            }
        };

        let (batch_result, all_txs, finalized_count, convergence_skipped, from_deferred, retry_counts, height, convergence_already_executed, prev_deferred) = {
            let mut state = self.inner.write().await;

            let current_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
            if checkpoint.height != current_height + 1 {
                return Err(anyhow::anyhow!(
                    "Checkpoint height changed during merkle computation: expected {}, got {}",
                    current_height + 1,
                    checkpoint.height
                ));
            }

            if let Some(last_checkpoint) = state.checkpoints.last() {
                let expected_prev = &last_checkpoint.hash;
                let got_prev = checkpoint.previous_hash.as_deref().unwrap_or("");
                if got_prev != expected_prev {
                    return Err(anyhow::anyhow!(
                        "Checkpoint prev_hash changed during merkle computation: expected {}, got {}",
                        &expected_prev[..16.min(expected_prev.len())],
                        &got_prev[..16.min(got_prev.len())]
                    ));
                }
            }

            let pre_snapshot: std::collections::HashMap<String, (u64, u64, u64)> = state
                .accounts
                .iter()
                .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
                .collect();
            state.pre_checkpoint_accounts_snapshot = Some((checkpoint.height, pre_snapshot));

            let mut prev_deferred = {
                let mut deferred = self.deferred_batch_txs.lock().await;
                std::mem::take(&mut *deferred)
            };
            let mut retry_counts = {
                let counts = self.deferred_batch_retry_counts.lock().await;
                counts.clone()
            };

            const MAX_DEFERRED_RETRIES: u32 = 3;
            if !prev_deferred.is_empty() {
                prev_deferred.retain(|dtx| {
                    let count = retry_counts.get(&dtx.hash).copied().unwrap_or(0);
                    count < MAX_DEFERRED_RETRIES
                });
            }

            let height = checkpoint.height;
            let finalized_count;

            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            let mut txs_to_execute: Vec<SignedTransaction> = Vec::new();
            let mut convergence_already_executed: std::collections::HashSet<String> = std::collections::HashSet::new();

            if our_merkle_root == checkpoint.tx_merkle_root && !unfinalized_hashes.is_empty() {
                for hash in &unfinalized_hashes {
                    if state.convergence_executed_txs.contains_key(hash) {
                        convergence_already_executed.insert(hash.clone());
                    }

                    if let Some(node) = state.dag.get_node(hash) {
                        let tx_clone = node.tx.clone();
                        txs_to_execute.push(tx_clone);
                        if !node.finalized {
                            let _ = state.dag.mark_finalized_deferred_cleanup(hash, height);
                        }
                    }
                }
                finalized_count = unfinalized_hashes.len();
                tracing::info!(
                    "Applied checkpoint {} at height {} ({} txs finalized, merkle matched)",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    height,
                    finalized_count
                );
            } else if our_merkle_root != checkpoint.tx_merkle_root && !unfinalized_hashes.is_empty() {
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
                finalized_count = 0;
                tracing::info!(
                    "Applied checkpoint {} at height {} (no unfinalized txs)",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    height
                );
            }

            let finalized_hashes_for_cleanup: Vec<String> = txs_to_execute.iter().map(|tx| tx.hash.clone()).collect();

            state.checkpoints.push(checkpoint.clone());
            state.last_checkpoint_time_ms = now_ms;

            self.checkpoint_height_cache.store(height, std::sync::atomic::Ordering::Relaxed);

            prev_deferred.retain(|dtx| !convergence_already_executed.contains(&dtx.hash));

            let mut all_txs: Vec<SignedTransaction> = txs_to_execute;
            let convergence_skipped = convergence_already_executed.len();
            let from_deferred = prev_deferred.len();

            all_txs.sort_by(|a, b| {
                a.tx.from.cmp(&b.tx.from)
                    .then(a.tx.nonce.cmp(&b.tx.nonce))
                    .then(a.hash.cmp(&b.hash))
            });

            let available_nonces: std::collections::HashMap<String, std::collections::BTreeSet<u64>> = {
                let mut map: std::collections::HashMap<String, std::collections::BTreeSet<u64>> = std::collections::HashMap::new();
                for tx in &all_txs {
                    if !matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Consolidation)) {
                        map.entry(tx.tx.from.clone()).or_default().insert(tx.tx.nonce);
                    }
                }
                map
            };

            let batch_result = Self::execute_batch_inline(&mut state, &all_txs, &available_nonces);

            {
                let finalized_conv_hashes: std::collections::HashSet<String> = convergence_already_executed.iter().cloned().collect();
                let pre_snap: std::collections::HashMap<String, (u64, u64, u64)> = state.pre_checkpoint_accounts_snapshot
                    .as_ref()
                    .map(|(_, s)| s.clone())
                    .unwrap_or_default();
                let changed_accounts = state.compute_changed_accounts(&pre_snap);
                let (cleared, replayed) = state.selective_convergence_overlay_update(
                    &finalized_conv_hashes,
                    &changed_accounts,
                    height,
                    false,
                );
                if cleared > 0 || replayed > 0 {
                    tracing::info!(
                        "Checkpoint h={}: proposer selective convergence update — cleared {} overlay entries, replayed {} (changed_accounts={}, finalized_conv={})",
                        height, cleared, replayed, changed_accounts.len(), finalized_conv_hashes.len()
                    );
                }
            }

            if !finalized_hashes_for_cleanup.is_empty() {
                state.dag.cleanup_sender_unfinalized_batch(&finalized_hashes_for_cleanup);
            }

            let has_special_txs = !batch_result.special_txs.is_empty();

            if finalized_count > 0 && !has_special_txs {
                let snapshot: std::collections::HashMap<String, (u64, u64, u64)> = state
                    .accounts
                    .iter()
                    .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
                    .collect();
                state.checkpoint_accounts_snapshot = Some((height, snapshot));
            } else if finalized_count == 0 {
                state.checkpoint_accounts_snapshot = None;
            }

            (batch_result, all_txs, finalized_count, convergence_skipped, from_deferred, retry_counts, height, convergence_already_executed, prev_deferred)
        };

        if finalized_count > 0 {
            self.record_finalized_batch(finalized_count as u64).await;
        }

        {
            let mut combined_deferred = batch_result.new_deferred;
            let finalized_hash_set: std::collections::HashSet<&str> = all_txs.iter().map(|t| t.hash.as_str()).collect();
            for dtx in prev_deferred {
                if !finalized_hash_set.contains(dtx.hash.as_str()) && !convergence_already_executed.contains(&dtx.hash) {
                    combined_deferred.push(dtx);
                }
            }
            self.store_batch_deferred(combined_deferred, retry_counts).await;
        }

        let has_special = !batch_result.special_txs.is_empty();
        self.process_batch_special_txs_with_skip(&batch_result.special_txs, &convergence_already_executed).await;

        {
            let state = self.inner.read().await;
            let mut rewards = self.rewards.write().await;
            for (addr, account) in &state.accounts {
                if account.staked > 0 {
                    rewards.sync_stake_amount(addr, account.staked);
                }
            }
        }

        if has_special && finalized_count > 0 {
            let mut state = self.inner.write().await;
            let snapshot: std::collections::HashMap<String, (u64, u64, u64)> = state
                .accounts
                .iter()
                .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
                .collect();
            state.checkpoint_accounts_snapshot = Some((height, snapshot));
        }

        self.process_batch_reward_infos(&all_txs, &batch_result.executed_hashes).await;

        tracing::info!(
            "Checkpoint h={} batch executed {}/{} txs in {:?} ({} convergence-pre-executed, {} from deferred, {} gap-skipped senders)",
            height,
            batch_result.executed_count, all_txs.len(),
            batch_start.elapsed(),
            convergence_skipped, from_deferred, batch_result.gap_skipped_senders.len()
        );

        Ok(())
    }

    /// Apply a checkpoint received with its finalized transaction hashes.
    /// LOCK-CONSOLIDATED: All state mutations (mark finalized, execute batch, cleanup,
    /// snapshot) happen under a SINGLE write lock acquisition. This eliminates the
    /// "lock convoy" where 7-8 sequential lock acquire/release cycles let hundreds of
    /// queued gossip operations stampede in between each step.
    ///
    /// Returns the number of missing transactions that the leader finalized but we don't have.
    /// Proofs should ONLY be stored when missing_tx_count == 0.
    pub async fn apply_checkpoint_with_finalized_hashes(
        &self,
        checkpoint: Checkpoint,
        finalized_tx_hashes: Vec<String>,
    ) -> Result<usize> {
        self.apply_checkpoint_with_finalized_hashes_inner(checkpoint, finalized_tx_hashes, false).await
    }

    pub async fn apply_checkpoint_catching_up(
        &self,
        checkpoint: Checkpoint,
        finalized_tx_hashes: Vec<String>,
    ) -> Result<usize> {
        self.apply_checkpoint_with_finalized_hashes_inner(checkpoint, finalized_tx_hashes, true).await
    }

    async fn apply_checkpoint_with_finalized_hashes_inner(
        &self,
        checkpoint: Checkpoint,
        finalized_tx_hashes: Vec<String>,
        skip_convergence_reapply: bool,
    ) -> Result<usize> {
        let batch_start = std::time::Instant::now();

        let mut prev_deferred = {
            let mut deferred = self.deferred_batch_txs.lock().await;
            std::mem::take(&mut *deferred)
        };
        let mut retry_counts = {
            let counts = self.deferred_batch_retry_counts.lock().await;
            counts.clone()
        };

        const MAX_DEFERRED_RETRIES: u32 = 3;
        let mut expired_count = 0usize;
        if !prev_deferred.is_empty() {
            prev_deferred.retain(|dtx| {
                let count = retry_counts.get(&dtx.hash).copied().unwrap_or(0);
                if count >= MAX_DEFERRED_RETRIES {
                    expired_count += 1;
                    false
                } else {
                    true
                }
            });
            if expired_count > 0 {
                tracing::warn!(
                    "Batch expired {} permanently-stuck deferred txs (>{} retries, {} remaining)",
                    expired_count, MAX_DEFERRED_RETRIES, prev_deferred.len()
                );
            }
        }

        let t_phase1 = std::time::Instant::now();
        let (txs_to_execute, mut convergence_already_executed, finalized_count, missing_tx_count, height, finalized_hashes_for_cleanup, checkpoint_now_ms) = {
            let lock_start = std::time::Instant::now();
            let mut state = self.inner.write().await;
            let lock_ms = lock_start.elapsed().as_millis();
            if lock_ms > 5 {
                tracing::info!("RCC-LOCK: checkpoint apply write lock acquired in {}ms (h={})", lock_ms, checkpoint.height);
            }

            let local_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
            if checkpoint.height <= local_height {
                return Err(anyhow::anyhow!(
                    "Checkpoint height {} not greater than local height {}",
                    checkpoint.height,
                    local_height
                ));
            }

            let pre_snapshot: std::collections::HashMap<String, (u64, u64, u64)> = state
                .accounts
                .iter()
                .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
                .collect();
            state.pre_checkpoint_accounts_snapshot = Some((checkpoint.height, pre_snapshot));

            let height = checkpoint.height;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            let mut txs_to_execute: Vec<SignedTransaction> = Vec::new();
            let mut convergence_already_executed: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut missing_tx_count = 0usize;

            let finalized_count = if !finalized_tx_hashes.is_empty() {
                let mut count = 0;
                let mut missing = 0;

                for hash in &finalized_tx_hashes {
                    if state.convergence_executed_txs.contains_key(hash) {
                        convergence_already_executed.insert(hash.clone());
                    }

                    if let Some(node) = state.dag.get_node(hash) {
                        let tx_clone = node.tx.clone();
                        txs_to_execute.push(tx_clone);
                        if !node.finalized {
                            let _ = state.dag.mark_finalized_deferred_cleanup(hash, height);
                        }
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
                    let hashes_clone = unfinalized_hashes.clone();
                    match tokio::task::spawn_blocking(move || MerkleTree::from_hex_leaves(&hashes_clone).map(|t| t.root())).await {
                        Ok(Ok(root)) => root,
                        _ => "0".repeat(64),
                    }
                };

                if our_merkle_root == checkpoint.tx_merkle_root && !unfinalized_hashes.is_empty() {
                    for hash in &unfinalized_hashes {
                        if state.convergence_executed_txs.contains_key(hash) {
                            convergence_already_executed.insert(hash.clone());
                        }

                        if let Some(node) = state.dag.get_node(hash) {
                            let tx_clone = node.tx.clone();
                            txs_to_execute.push(tx_clone);
                            if !node.finalized {
                                let _ = state.dag.mark_finalized_deferred_cleanup(hash, height);
                            }
                        }
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

            let finalized_hashes_for_cleanup: Vec<String> = txs_to_execute.iter().map(|tx| tx.hash.clone()).collect();

            (txs_to_execute, convergence_already_executed, finalized_count, missing_tx_count, height, finalized_hashes_for_cleanup, now_ms)
        };
        let phase1_ms = t_phase1.elapsed().as_millis();

        prev_deferred.retain(|dtx| !convergence_already_executed.contains(&dtx.hash));

        let mut all_txs: Vec<SignedTransaction> = txs_to_execute;
        let convergence_skipped = convergence_already_executed.len();
        let from_deferred = prev_deferred.len();

        all_txs.sort_by(|a, b| {
            a.tx.from.cmp(&b.tx.from)
                .then(a.tx.nonce.cmp(&b.tx.nonce))
                .then(a.hash.cmp(&b.hash))
        });

        let available_nonces: std::collections::HashMap<String, std::collections::BTreeSet<u64>> = {
            let mut map: std::collections::HashMap<String, std::collections::BTreeSet<u64>> = std::collections::HashMap::new();
            for tx in &all_txs {
                if !matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Consolidation)) {
                    map.entry(tx.tx.from.clone()).or_default().insert(tx.tx.nonce);
                }
            }
            map
        };

        let t_phase2 = std::time::Instant::now();
        let (batch_result, needs_eviction) = {
            let lock_start2 = std::time::Instant::now();
            let mut state = self.inner.write().await;
            let lock_ms2 = lock_start2.elapsed().as_millis();
            if lock_ms2 > 5 {
                tracing::info!("RCC-LOCK: checkpoint phase2 write lock acquired in {}ms (h={})", lock_ms2, checkpoint.height);
            }

            let current_height_phase2 = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
            if checkpoint.height <= current_height_phase2 {
                tracing::warn!(
                    "PHASE2 RACE GUARD: Checkpoint h={} already applied during phase gap (current={}), aborting duplicate",
                    checkpoint.height, current_height_phase2
                );
                return Err(anyhow::anyhow!(
                    "Checkpoint height {} already applied during phase gap (current={})",
                    checkpoint.height, current_height_phase2
                ));
            }

            let mut checkpoint_with_hashes = checkpoint.clone();
            if checkpoint_with_hashes.finalized_tx_hashes.is_empty() && !finalized_tx_hashes.is_empty() {
                checkpoint_with_hashes.finalized_tx_hashes = finalized_tx_hashes;
            }
            state.checkpoints.push(checkpoint_with_hashes);
            state.last_checkpoint_time_ms = checkpoint_now_ms;
            self.checkpoint_height_cache.store(checkpoint.height, std::sync::atomic::Ordering::Relaxed);

            let result = Self::execute_batch_inline(&mut state, &all_txs, &available_nonces);

            {
                let finalized_conv_hashes: std::collections::HashSet<String> = convergence_already_executed.iter().cloned().collect();
                let pre_snap: std::collections::HashMap<String, (u64, u64, u64)> = state.pre_checkpoint_accounts_snapshot
                    .as_ref()
                    .map(|(_, s)| s.clone())
                    .unwrap_or_default();
                let changed_accounts = state.compute_changed_accounts(&pre_snap);
                let (cleared, replayed) = state.selective_convergence_overlay_update(
                    &finalized_conv_hashes,
                    &changed_accounts,
                    height,
                    skip_convergence_reapply,
                );
                if cleared > 0 || replayed > 0 {
                    tracing::info!(
                        "Checkpoint h={}: selective convergence update — cleared {} overlay entries, replayed {} (changed_accounts={}, finalized_conv={})",
                        height, cleared, replayed, changed_accounts.len(), finalized_conv_hashes.len()
                    );
                }
            }

            if !finalized_hashes_for_cleanup.is_empty() {
                state.dag.cleanup_sender_unfinalized_batch(&finalized_hashes_for_cleanup);
            }

            let has_special_txs = !result.special_txs.is_empty();

            if missing_tx_count == 0 && !has_special_txs {
                let snapshot: std::collections::HashMap<String, (u64, u64, u64)> = state
                    .accounts
                    .iter()
                    .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
                    .collect();
                state.checkpoint_accounts_snapshot = Some((height, snapshot));
            } else if missing_tx_count > 0 {
                state.checkpoint_accounts_snapshot = None;
            }

            const DAG_EVICTION_RETENTION: u64 = 50;
            let eviction_needed = height > DAG_EVICTION_RETENTION;

            (result, eviction_needed)
        };
        let phase2_ms = t_phase2.elapsed().as_millis();

        if needs_eviction {
            const DAG_EVICTION_RETENTION: u64 = 50;
            let eviction_boundary = height - DAG_EVICTION_RETENTION;
            let mut state = self.inner.write().await;
            let pre_count = state.dag.node_count();
            let evicted = state.dag.evict_finalized_before(eviction_boundary);
            if evicted > 0 {
                tracing::info!(
                    "In-memory DAG eviction: removed {} finalized nodes older than h={} ({} -> {} nodes)",
                    evicted, eviction_boundary, pre_count, state.dag.node_count()
                );
            }
        }

        if finalized_count > 0 {
            self.record_finalized_batch(finalized_count as u64).await;
        }

        {
            let mut combined_deferred = batch_result.new_deferred;
            let finalized_hash_set: std::collections::HashSet<&str> = all_txs.iter().map(|t| t.hash.as_str()).collect();
            for dtx in prev_deferred {
                if !finalized_hash_set.contains(dtx.hash.as_str()) && !convergence_already_executed.contains(&dtx.hash) {
                    combined_deferred.push(dtx);
                }
            }
            self.store_batch_deferred(combined_deferred, retry_counts).await;
        }

        let has_special = !batch_result.special_txs.is_empty();
        self.process_batch_special_txs_with_skip(&batch_result.special_txs, &convergence_already_executed).await;

        {
            let state = self.inner.read().await;
            let mut rewards = self.rewards.write().await;
            for (addr, account) in &state.accounts {
                if account.staked > 0 {
                    rewards.sync_stake_amount(addr, account.staked);
                }
            }
        }

        if has_special && missing_tx_count == 0 {
            let mut state = self.inner.write().await;
            let snapshot: std::collections::HashMap<String, (u64, u64, u64)> = state
                .accounts
                .iter()
                .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
                .collect();
            state.checkpoint_accounts_snapshot = Some((height, snapshot));
        }

        self.process_batch_reward_infos(&all_txs, &batch_result.executed_hashes).await;

        let total_finalized = all_txs.len();
        let newly_failed = all_txs.len().saturating_sub(batch_result.executed_count);
        if newly_failed > 0 && convergence_skipped == 0 {
            tracing::warn!(
                "Batch UNDERCOUNT: {} of {} finalized txs actually executed (skipped {})",
                batch_result.executed_count, all_txs.len(), newly_failed
            );
        }
        tracing::info!(
            "Checkpoint h={} batch executed {}/{} txs in {:?} (phase1={}ms phase2={}ms, {} convergence-pre-executed, {} from deferred, {} expired, {} gap-skipped senders)",
            height,
            batch_result.executed_count, total_finalized,
            batch_start.elapsed(),
            phase1_ms, phase2_ms,
            convergence_skipped, from_deferred, expired_count, batch_result.gap_skipped_senders.len()
        );

        Ok(missing_tx_count)
    }

    pub async fn rollback_last_checkpoint(&self) -> Result<(Checkpoint, Vec<String>), anyhow::Error> {
        let mut state = self.inner.write().await;

        let checkpoint = state.checkpoints.last()
            .ok_or_else(|| anyhow::anyhow!("No checkpoint to rollback"))?;
        let height = checkpoint.height;
        let rolled_back_hash = checkpoint.hash.clone();
        let finalized_hashes = checkpoint.finalized_tx_hashes.clone();

        let snapshot = match &state.pre_checkpoint_accounts_snapshot {
            Some((snap_height, snap)) if *snap_height == height => snap.clone(),
            Some((snap_height, _)) => {
                tracing::error!(
                    "FORK ROLLBACK: pre_checkpoint snapshot height mismatch (expected {}, got {})",
                    height, snap_height
                );
                return Err(anyhow::anyhow!(
                    "Pre-checkpoint snapshot height mismatch: expected {}, got {}",
                    height, snap_height
                ));
            }
            None => {
                tracing::error!(
                    "FORK ROLLBACK: No pre-checkpoint snapshot available for height {}",
                    height
                );
                return Err(anyhow::anyhow!(
                    "No pre-checkpoint snapshot available for height {}",
                    height
                ));
            }
        };

        let checkpoint = state.checkpoints.pop().unwrap();

        let snapshot_addrs: std::collections::HashSet<String> = snapshot.keys().cloned().collect();
        let current_addrs: Vec<String> = state.accounts.keys().cloned().collect();
        for addr in &current_addrs {
            if !snapshot_addrs.contains(addr) {
                state.accounts.remove(addr);
            }
        }
        for (addr, (balance, nonce, staked)) in &snapshot {
            if let Some(acc) = state.accounts.get_mut(addr) {
                acc.balance = *balance;
                acc.nonce = *nonce;
                acc.staked = *staked;
            }
        }
        let unmarked = state.dag.unmark_finalized_batch(&finalized_hashes);
        tracing::warn!(
            "FORK ROLLBACK: Rolled back checkpoint {} at height {} — restored {} accounts, unmarked {} finalized TXs",
            &rolled_back_hash[..16.min(rolled_back_hash.len())],
            height,
            snapshot.len(),
            unmarked
        );

        state.pre_checkpoint_accounts_snapshot = None;
        state.checkpoint_accounts_snapshot = None;
        state.convergence_executed_txs.clear();
        state.convergence_executed_order.clear();
        state.convergence_overlay.clear();

        let new_height = state.checkpoints.last().map(|c| c.height).unwrap_or(0);
        drop(state);

        self.checkpoint_height_cache.store(new_height, std::sync::atomic::Ordering::SeqCst);

        Ok((checkpoint, finalized_hashes))
    }

    pub async fn get_latest_checkpoint_id(&self) -> Option<String> {
        let state = self.inner.read().await;
        state.checkpoints.last().map(|c| c.hash.chars().take(16).collect())
    }
}
