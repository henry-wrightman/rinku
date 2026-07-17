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

        let (batch_result, all_txs, finalized_count, fast_path_skipped, from_deferred, retry_counts, height, fast_path_already_finalized, prev_deferred, fp_special_txs) = {
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
            let mut fast_path_already_finalized: std::collections::HashSet<String> = std::collections::HashSet::new();

            if our_merkle_root == checkpoint.tx_merkle_root && !unfinalized_hashes.is_empty() {
                for hash in &unfinalized_hashes {
                    if state.fast_path_finalized_txs.contains_key(hash) {
                        fast_path_already_finalized.insert(hash.clone());
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

            prev_deferred.retain(|dtx| !fast_path_already_finalized.contains(&dtx.hash));

            let fp_special_txs: Vec<SignedTransaction> = txs_to_execute
                .iter()
                .filter(|tx| {
                    fast_path_already_finalized.contains(&tx.hash)
                        && matches!(
                            tx.tx.kind,
                            Some(rinku_core::types::TransactionKind::Stake)
                                | Some(rinku_core::types::TransactionKind::Unstake)
                                | Some(rinku_core::types::TransactionKind::ClaimRewards)
                        )
                })
                .cloned()
                .collect();

            let mut contract_lane_txs: Vec<SignedTransaction> = txs_to_execute
                .into_iter()
                .filter(|tx| !fast_path_already_finalized.contains(&tx.hash))
                .collect();
            let fast_path_skipped = fast_path_already_finalized.len();
            let from_deferred = prev_deferred.len();

            if fast_path_skipped > 0 {
                tracing::info!(
                    "Non-proposer checkpoint h={}: skipping {} fast-path TXs (already applied), {} contract-lane TXs to execute",
                    height, fast_path_skipped, contract_lane_txs.len()
                );
            }

            contract_lane_txs.sort_by(|a, b| {
                a.tx.from.cmp(&b.tx.from)
                    .then(a.tx.nonce.cmp(&b.tx.nonce))
                    .then(a.hash.cmp(&b.hash))
            });

            let available_nonces: std::collections::HashMap<String, std::collections::BTreeSet<u64>> = {
                let mut map: std::collections::HashMap<String, std::collections::BTreeSet<u64>> = std::collections::HashMap::new();
                for tx in &contract_lane_txs {
                    if !matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Consolidation)) {
                        map.entry(tx.tx.from.clone()).or_default().insert(tx.tx.nonce);
                    }
                }
                map
            };

            let all_txs = contract_lane_txs;
            let batch_result = Self::execute_batch_inline(&mut state, &all_txs, &available_nonces);

            {
                let finalized_fp_hashes: std::collections::HashSet<String> = fast_path_already_finalized.iter().cloned().collect();
                let cleared = state.clear_checkpoint_finalized_txs(&finalized_fp_hashes);
                if cleared > 0 {
                    tracing::info!(
                        "Checkpoint h={}: cleared {} fast-path finalized entries",
                        height, cleared
                    );
                }
            }

            if !finalized_hashes_for_cleanup.is_empty() {
                state.dag.cleanup_sender_unfinalized_batch(&finalized_hashes_for_cleanup);
            }

            let has_special_txs = !batch_result.special_txs.is_empty() || !fp_special_txs.is_empty();

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

            (batch_result, all_txs, finalized_count, fast_path_skipped, from_deferred, retry_counts, height, fast_path_already_finalized, prev_deferred, fp_special_txs)
        };

        if finalized_count > 0 {
            self.record_finalized_batch(finalized_count as u64).await;
        }

        {
            let mut combined_deferred = batch_result.new_deferred;
            let finalized_hash_set: std::collections::HashSet<&str> = all_txs.iter().map(|t| t.hash.as_str()).collect();
            for dtx in prev_deferred {
                if !finalized_hash_set.contains(dtx.hash.as_str()) && !fast_path_already_finalized.contains(&dtx.hash) {
                    combined_deferred.push(dtx);
                }
            }
            self.store_batch_deferred(combined_deferred, retry_counts).await;
        }

        let mut all_special_txs = batch_result.special_txs;
        all_special_txs.extend(fp_special_txs);
        let has_special = !all_special_txs.is_empty();
        self.process_batch_special_txs_with_skip(&all_special_txs, &fast_path_already_finalized).await;

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
            "Checkpoint h={} batch executed {}/{} txs in {:?} ({} fast-path-pre-finalized, {} from deferred, {} gap-skipped senders)",
            height,
            batch_result.executed_count, all_txs.len(),
            batch_start.elapsed(),
            fast_path_skipped, from_deferred, batch_result.gap_skipped_senders.len()
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

    pub async fn apply_checkpoint_proof_verified(
        &self,
        checkpoint: Checkpoint,
        finalized_tx_hashes: Vec<String>,
        proofs: &[rinku_core::types::AccountStateProof],
    ) -> Result<usize> {
        let apply_start = std::time::Instant::now();

        let expected_state_root = checkpoint.state_root.clone();
        let height = checkpoint.height;

        let verified_accounts: Vec<(String, u64, u64, u64)> = {
            let mut verified = Vec::with_capacity(proofs.len());
            for proof in proofs {
                if proof.state_root != expected_state_root {
                    tracing::warn!(
                        "PROOF-VERIFY REJECT: proof for {} has state_root {} but checkpoint has {} at h={}",
                        &proof.address[..16.min(proof.address.len())],
                        &proof.state_root[..16.min(proof.state_root.len())],
                        &expected_state_root[..16.min(expected_state_root.len())],
                        height
                    );
                    return Err(anyhow::anyhow!(
                        "Proof state_root mismatch for {} at height {}",
                        proof.address, height
                    ));
                }
                let detail = crate::proofs::verify_account_state_proof_detailed(proof);
                if !detail.valid {
                    tracing::warn!(
                        "PROOF-VERIFY REJECT: invalid proof for {} at h={} (computed={}, expected={})",
                        &proof.address[..16.min(proof.address.len())],
                        height,
                        &detail.computed_root[..16.min(detail.computed_root.len())],
                        &detail.expected_root[..16.min(detail.expected_root.len())]
                    );
                    return Err(anyhow::anyhow!(
                        "Invalid SMT proof for {} at height {}",
                        proof.address, height
                    ));
                }
                verified.push((
                    proof.address.clone(),
                    proof.balance_micro,
                    proof.nonce,
                    proof.staked_micro,
                ));
            }
            verified
        };
        let verify_ms = apply_start.elapsed().as_millis();

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        {
            let mut wal = self.wal.lock().await;
            if wal.is_open() {
                if let Err(e) = wal.begin_checkpoint(height, &checkpoint.hash) {
                    tracing::warn!("WAL: failed to write BeginCheckpoint: {}", e);
                }
                for (addr, balance, nonce, staked) in &verified_accounts {
                    if let Err(e) = wal.log_account_update(addr, *balance, *nonce, *staked) {
                        tracing::warn!("WAL: failed to write AccountUpdate: {}", e);
                    }
                }
            }
        }

        let lock_start = std::time::Instant::now();
        let mut incremental_changed: usize = 0;
        let mut incremental_pruned: usize = 0;
        {
            let mut state = self.inner.write().await;
            let lock_ms = lock_start.elapsed().as_millis();
            if lock_ms > 5 {
                tracing::info!(
                    "RCC-LOCK: proof-verified apply write lock acquired in {}ms (h={})",
                    lock_ms, height
                );
            }

            let local_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
            if height <= local_height {
                return Err(anyhow::anyhow!(
                    "Checkpoint height {} not greater than local height {}",
                    height, local_height
                ));
            }

            let pre_snapshot: std::collections::HashMap<String, (u64, u64, u64)> = state
                .accounts
                .iter()
                .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
                .collect();
            state.pre_checkpoint_accounts_snapshot = Some((height, pre_snapshot));

            let accounts_backup: std::collections::HashMap<String, rinku_core::types::Account> =
                state.accounts.clone();

            let mut proof_addrs: std::collections::HashSet<String> = std::collections::HashSet::with_capacity(verified_accounts.len());

            for (addr, balance, nonce, staked) in &verified_accounts {
                proof_addrs.insert(addr.clone());
                if let Some(acc) = state.accounts.get_mut(addr) {
                    acc.balance = *balance;
                    acc.nonce = *nonce;
                    acc.staked = *staked;
                } else {
                    state.accounts.insert(
                        addr.clone(),
                        rinku_core::types::Account {
                            address: addr.clone(),
                            balance: *balance,
                            nonce: *nonce,
                            first_seen: now_ms,
                            staked: *staked,
                            unbonding: 0,
                            unbonding_release: None,
                            latest_balance_proof: None,
                            partition_violations: 0,
                            reputation_penalty: 0.0,
                            penalty_decay_checkpoint: None,
                            partition_budget: None,
                            partition_budget_spent: 0,
                            ecdsa_public_key: None,
                        },
                    );
                }
            }

            let mut pruned_addrs: Vec<String> = Vec::new();
            let pre_prune_count = state.accounts.len();
            {
                let proof_ref = &proof_addrs;
                pruned_addrs = state.accounts.keys()
                    .filter(|addr| !proof_ref.contains(*addr))
                    .cloned()
                    .collect();
            }
            for addr in &pruned_addrs {
                state.accounts.remove(addr);
            }
            let pruned = pruned_addrs.len();
            if pruned > 0 {
                tracing::warn!(
                    "PROOF-VERIFIED PRUNE: removed {} local-only accounts not in proof set at h={} ({} -> {} accounts)",
                    pruned, height, pre_prune_count, state.accounts.len()
                );
            }

            use crate::sparse_merkle_trie::hash_account_key;

            for addr in &pruned_addrs {
                let key = hash_account_key(addr);
                let _ = state.state_trie.delete(&key, None);
            }

            let mut changed_addrs: Vec<String> = Vec::new();
            for (addr, balance, nonce, staked) in &verified_accounts {
                let changed = match accounts_backup.get(addr) {
                    Some(old) => old.balance != *balance || old.nonce != *nonce || old.staked != *staked,
                    None => true,
                };
                if changed {
                    changed_addrs.push(addr.clone());
                }
            }

            incremental_changed = changed_addrs.len();
            incremental_pruned = pruned;

            state.update_state_trie_accounts(&changed_addrs);

            let local_root = state.state_trie.root_hex();
            if local_root != expected_state_root {
                tracing::warn!(
                    "PROOF-VERIFIED INCREMENTAL MISMATCH at h={}: local {} != expected {} ({} changed, {} pruned) — falling back to full rebuild",
                    height,
                    &local_root[..16.min(local_root.len())],
                    &expected_state_root[..16.min(expected_state_root.len())],
                    changed_addrs.len(),
                    pruned
                );

                state.state_trie = StateInner::build_state_trie_from_accounts(&state.accounts);
                let rebuild_root = state.state_trie.root_hex();
                if rebuild_root != expected_state_root {
                    tracing::error!(
                        "PROOF-VERIFIED ROOT MISMATCH at h={}: rebuilt trie root {} != checkpoint state_root {} ({} accounts, {} pruned) — rolling back",
                        height,
                        &rebuild_root[..16.min(rebuild_root.len())],
                        &expected_state_root[..16.min(expected_state_root.len())],
                        state.accounts.len(),
                        pruned
                    );
                    state.accounts = accounts_backup;
                    state.state_trie = StateInner::build_state_trie_from_accounts(&state.accounts);
                    state.pre_checkpoint_accounts_snapshot = None;
                    return Err(anyhow::anyhow!(
                        "Proof-verified root mismatch at h={}: local {} != expected {}",
                        height, &rebuild_root[..16.min(rebuild_root.len())],
                        &expected_state_root[..16.min(expected_state_root.len())]
                    ));
                }
                tracing::info!(
                    "PROOF-VERIFIED FULL REBUILD OK at h={}: root matched after rebuild ({} accounts)",
                    height, state.accounts.len()
                );
            } else {
                tracing::info!(
                    "PROOF-VERIFIED INCREMENTAL OK at h={}: root matched ({} changed, {} pruned, {} total accounts)",
                    height, changed_addrs.len(), pruned, state.accounts.len()
                );
            }

            for hash in &finalized_tx_hashes {
                if state.dag.get_node(hash).is_some() {
                    let _ = state.dag.mark_finalized_deferred_cleanup(hash, height);
                }
            }
            if !finalized_tx_hashes.is_empty() {
                state.dag.cleanup_sender_unfinalized_batch(&finalized_tx_hashes);
            }

            let total_finalized_in_checkpoint = finalized_tx_hashes.len() as u64;
            let already_counted_by_fast_path = finalized_tx_hashes.iter()
                .filter(|h| state.fast_path_finalized_txs.contains_key(h.as_str()))
                .count() as u64;
            let new_tx_count = total_finalized_in_checkpoint.saturating_sub(already_counted_by_fast_path);
            if new_tx_count > 0 {
                state.total_transactions += new_tx_count;
            }

            let mut checkpoint_with_hashes = checkpoint.clone();
            if checkpoint_with_hashes.finalized_tx_hashes.is_empty() && !finalized_tx_hashes.is_empty() {
                checkpoint_with_hashes.finalized_tx_hashes = finalized_tx_hashes;
            }
            state.checkpoints.push(checkpoint_with_hashes);
            state.last_checkpoint_time_ms = now_ms;
            self.checkpoint_height_cache.store(height, std::sync::atomic::Ordering::Relaxed);

            state.fast_path_finalized_txs.clear();
            state.fast_path_finalized_order.clear();

            let snapshot: std::collections::HashMap<String, (u64, u64, u64)> = state
                .accounts
                .iter()
                .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
                .collect();
            state.checkpoint_accounts_snapshot = Some((height, snapshot));

            const DAG_EVICTION_RETENTION: u64 = 50;
            if height > DAG_EVICTION_RETENTION {
                let eviction_boundary = height - DAG_EVICTION_RETENTION;
                let evicted = state.dag.evict_finalized_before(eviction_boundary);
                if evicted > 0 {
                    tracing::info!(
                        "Proof-verified DAG eviction: removed {} nodes older than h={}",
                        evicted, eviction_boundary
                    );
                }
            }
        };
        let write_ms = lock_start.elapsed().as_millis();

        let finalized_count = proofs.len();
        if finalized_count > 0 {
            self.record_finalized_batch(finalized_count as u64).await;
        }

        {
            let state = self.inner.read().await;
            let mut rewards = self.rewards.write().await;
            for (addr, account) in &state.accounts {
                if account.staked > 0 {
                    rewards.sync_stake_amount(addr, account.staked);
                }
            }
        }

        {
            let mut wal = self.wal.lock().await;
            if wal.is_open() {
                if let Err(e) = wal.commit_checkpoint(height, &checkpoint.hash) {
                    tracing::warn!("WAL: failed to write CommitCheckpoint: {}", e);
                }
            }
        }

        tracing::info!(
            "PROOF-VERIFIED checkpoint h={}: {} accounts updated in {}ms (verify={}ms, write={}ms, changed={}, pruned={}) — no re-execution",
            height, verified_accounts.len(), apply_start.elapsed().as_millis(), verify_ms, write_ms,
            incremental_changed, incremental_pruned
        );

        Ok(0)
    }

    async fn apply_checkpoint_with_finalized_hashes_inner(
        &self,
        checkpoint: Checkpoint,
        finalized_tx_hashes: Vec<String>,
        _skip_fast_path_reapply: bool,
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

        let pre_merkle_hashes: Option<(Vec<String>, String)> = if finalized_tx_hashes.is_empty() {
            use crate::config::PROPAGATION_GRACE_MS;
            use rinku_core::merkle::MerkleTree;

            let unfinalized_hashes = {
                let state = self.inner.read().await;
                let local_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
                if checkpoint.height <= local_height {
                    return Err(anyhow::anyhow!(
                        "Checkpoint height {} not greater than local height {}",
                        checkpoint.height,
                        local_height
                    ));
                }
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

            let merkle_root = if unfinalized_hashes.is_empty() {
                "0".repeat(64)
            } else {
                let hashes_clone = unfinalized_hashes.clone();
                match tokio::task::spawn_blocking(move || rinku_core::merkle::MerkleTree::from_hex_leaves(&hashes_clone).map(|t| t.root())).await {
                    Ok(Ok(root)) => root,
                    _ => "0".repeat(64),
                }
            };

            Some((unfinalized_hashes, merkle_root))
        } else {
            None
        };

        let t_phase1 = std::time::Instant::now();
        let (txs_to_execute, mut fast_path_already_finalized, finalized_count, missing_tx_count, height, finalized_hashes_for_cleanup, checkpoint_now_ms) = {
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
            let mut fast_path_already_finalized: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut missing_tx_count = 0usize;

            let finalized_count = if !finalized_tx_hashes.is_empty() {
                let mut count = 0;
                let mut missing = 0;

                for hash in &finalized_tx_hashes {
                    if state.fast_path_finalized_txs.contains_key(hash) {
                        fast_path_already_finalized.insert(hash.clone());
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
            } else if let Some((ref unfinalized_hashes, ref our_merkle_root)) = pre_merkle_hashes {
                if *our_merkle_root == checkpoint.tx_merkle_root && !unfinalized_hashes.is_empty() {
                    for hash in unfinalized_hashes {
                        if state.fast_path_finalized_txs.contains_key(hash.as_str()) {
                            fast_path_already_finalized.insert(hash.clone());
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
            } else {
                0
            };

            let finalized_hashes_for_cleanup: Vec<String> = txs_to_execute.iter().map(|tx| tx.hash.clone()).collect();

            (txs_to_execute, fast_path_already_finalized, finalized_count, missing_tx_count, height, finalized_hashes_for_cleanup, now_ms)
        };
        let phase1_ms = t_phase1.elapsed().as_millis();

        prev_deferred.retain(|dtx| !fast_path_already_finalized.contains(&dtx.hash));

        let fp_special_txs: Vec<SignedTransaction> = txs_to_execute
            .iter()
            .filter(|tx| {
                fast_path_already_finalized.contains(&tx.hash)
                    && matches!(
                        tx.tx.kind,
                        Some(rinku_core::types::TransactionKind::Stake)
                            | Some(rinku_core::types::TransactionKind::Unstake)
                            | Some(rinku_core::types::TransactionKind::ClaimRewards)
                    )
            })
            .cloned()
            .collect();

        let mut contract_lane_txs: Vec<SignedTransaction> = txs_to_execute
            .into_iter()
            .filter(|tx| !fast_path_already_finalized.contains(&tx.hash))
            .collect();
        let fast_path_skipped = fast_path_already_finalized.len();
        let from_deferred = prev_deferred.len();

        if fast_path_skipped > 0 {
            tracing::info!(
                "Non-proposer checkpoint (proof-sync) h={}: skipping {} fast-path TXs (already applied), {} contract-lane TXs to execute",
                height, fast_path_skipped, contract_lane_txs.len()
            );
        }

        contract_lane_txs.sort_by(|a, b| {
            a.tx.from.cmp(&b.tx.from)
                .then(a.tx.nonce.cmp(&b.tx.nonce))
                .then(a.hash.cmp(&b.hash))
        });

        let available_nonces: std::collections::HashMap<String, std::collections::BTreeSet<u64>> = {
            let mut map: std::collections::HashMap<String, std::collections::BTreeSet<u64>> = std::collections::HashMap::new();
            for tx in &contract_lane_txs {
                if !matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Consolidation)) {
                    map.entry(tx.tx.from.clone()).or_default().insert(tx.tx.nonce);
                }
            }
            map
        };

        let all_txs = contract_lane_txs;

        let t_phase2 = std::time::Instant::now();
        let batch_result = {
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
                let finalized_fp_hashes: std::collections::HashSet<String> = fast_path_already_finalized.iter().cloned().collect();
                let cleared = state.clear_checkpoint_finalized_txs(&finalized_fp_hashes);
                if cleared > 0 {
                    tracing::info!(
                        "Checkpoint h={}: cleared {} fast-path finalized entries",
                        height, cleared
                    );
                }
            }

            if !finalized_hashes_for_cleanup.is_empty() {
                state.dag.cleanup_sender_unfinalized_batch(&finalized_hashes_for_cleanup);
            }

            let has_special_txs = !result.special_txs.is_empty() || !fp_special_txs.is_empty();

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
            if height > DAG_EVICTION_RETENTION {
                let eviction_boundary = height - DAG_EVICTION_RETENTION;
                let pre_count = state.dag.node_count();
                let evicted = state.dag.evict_finalized_before(eviction_boundary);
                if evicted > 0 {
                    tracing::info!(
                        "In-memory DAG eviction: removed {} finalized nodes older than h={} ({} -> {} nodes)",
                        evicted, eviction_boundary, pre_count, state.dag.node_count()
                    );
                }
            }

            result
        };
        let phase2_ms = t_phase2.elapsed().as_millis();

        if finalized_count > 0 {
            self.record_finalized_batch(finalized_count as u64).await;
        }

        {
            let mut combined_deferred = batch_result.new_deferred;
            let finalized_hash_set: std::collections::HashSet<&str> = all_txs.iter().map(|t| t.hash.as_str()).collect();
            for dtx in prev_deferred {
                if !finalized_hash_set.contains(dtx.hash.as_str()) && !fast_path_already_finalized.contains(&dtx.hash) {
                    combined_deferred.push(dtx);
                }
            }
            self.store_batch_deferred(combined_deferred, retry_counts).await;
        }

        let mut all_special_txs = batch_result.special_txs;
        all_special_txs.extend(fp_special_txs);
        let has_special = !all_special_txs.is_empty();
        self.process_batch_special_txs_with_skip(&all_special_txs, &fast_path_already_finalized).await;

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
        if newly_failed > 0 && fast_path_skipped == 0 {
            tracing::warn!(
                "Batch UNDERCOUNT: {} of {} finalized txs actually executed (skipped {})",
                batch_result.executed_count, all_txs.len(), newly_failed
            );
        }
        tracing::info!(
            "Checkpoint h={} batch executed {}/{} txs in {:?} (phase1={}ms phase2={}ms, {} fast-path-pre-finalized, {} from deferred, {} expired, {} gap-skipped senders)",
            height,
            batch_result.executed_count, total_finalized,
            batch_start.elapsed(),
            phase1_ms, phase2_ms,
            fast_path_skipped, from_deferred, expired_count, batch_result.gap_skipped_senders.len()
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
        state.fast_path_finalized_txs.clear();
        state.fast_path_finalized_order.clear();

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
