use super::*;

impl NodeState {
    pub async fn get_merkle_proof(
        &self,
        tx_hash: &str,
        checkpoint_height: u64,
    ) -> Option<(Vec<String>, usize, Checkpoint)> {
        use rinku_core::merkle::MerkleTree;

        let state = self.inner.read().await;

        let checkpoint = state
            .checkpoints
            .iter()
            .find(|c| c.height == checkpoint_height)?
            .clone();

        let mut finalized_hashes: Vec<String> = if !checkpoint.finalized_tx_hashes.is_empty() {
            let mut hashes = checkpoint.finalized_tx_hashes.clone();
            if !hashes.contains(&tx_hash.to_string()) {
                let dag_hashes: Vec<String> = state
                    .dag
                    .get_all_nodes()
                    .into_iter()
                    .filter(|n| n.finalized && n.checkpoint_height == Some(checkpoint_height))
                    .map(|n| n.hash.clone())
                    .collect();
                for h in dag_hashes {
                    if !hashes.contains(&h) {
                        hashes.push(h);
                    }
                }
            }
            hashes
        } else {
            state
                .dag
                .get_all_nodes()
                .into_iter()
                .filter(|n| n.finalized && n.checkpoint_height == Some(checkpoint_height))
                .map(|n| n.hash.clone())
                .collect()
        };

        if finalized_hashes.is_empty() {
            tracing::warn!(
                "No finalized hashes found for checkpoint {} when generating proof for {}",
                checkpoint_height,
                &tx_hash[..16.min(tx_hash.len())]
            );
            return None;
        }

        finalized_hashes.sort();

        let index = match finalized_hashes.iter().position(|h| h == tx_hash) {
            Some(idx) => idx,
            None => {
                tracing::warn!(
                    "Transaction {} not found in {} finalized hashes at checkpoint {} (first: {}, last: {})",
                    &tx_hash[..16.min(tx_hash.len())],
                    finalized_hashes.len(),
                    checkpoint_height,
                    &finalized_hashes.first().map(|h| &h[..16.min(h.len())]).unwrap_or(""),
                    &finalized_hashes.last().map(|h| &h[..16.min(h.len())]).unwrap_or(""),
                );
                return None;
            }
        };

        let tree = MerkleTree::from_hex_leaves(&finalized_hashes).ok()?;
        let merkle_proof = tree.get_proof(index).ok()?;

        Some((merkle_proof.siblings, index, checkpoint))
    }

    pub async fn get_dag_merkle_root(&self) -> Option<String> {
        use rinku_core::merkle::MerkleTree;

        let state = self.inner.read().await;
        let tips = state.dag.tips();

        if tips.is_empty() {
            return None;
        }

        let tree = MerkleTree::from_hex_leaves(&tips).ok()?;
        Some(tree.root())
    }

    pub async fn get_txs_since_checkpoint(
        &self,
        from_checkpoint: u64,
        missing_hashes: &[String],
    ) -> Vec<SignedTransaction> {
        let state = self.inner.read().await;

        state
            .dag
            .get_all_nodes()
            .into_iter()
            .filter(|n| {
                if !missing_hashes.is_empty() {
                    missing_hashes.contains(&n.hash)
                } else {
                    n.checkpoint_height
                        .map(|h| h > from_checkpoint)
                        .unwrap_or(true)
                }
            })
            .map(|n| n.tx.clone())
            .collect()
    }

    pub async fn get_sync_snapshot(&self) -> SyncSnapshot {
        let state = self.inner.read().await;

        let all_nodes = state.dag.get_all_nodes();
        let dag_txs: Vec<SignedTransaction> = all_nodes
            .iter()
            .map(|n| n.tx.clone())
            .collect();
        
        let finalized_tx_hashes: Vec<String> = all_nodes
            .iter()
            .filter(|n| n.finalized)
            .map(|n| n.hash.clone())
            .collect();
        
        let tx_checkpoint_heights: HashMap<String, u64> = all_nodes
            .iter()
            .filter_map(|n| n.checkpoint_height.map(|h| (n.hash.clone(), h)))
            .collect();

        let dag_tx_count = dag_txs.len() as u64;
        let contracts = state.contracts.clone();
        let total_burned = state.total_burned;
        let total_to_validators = state.total_to_validators;
        
        drop(state);

        let rewards_snapshot = {
            let rewards = self.rewards.read().await;
            Some(rewards.to_json())
        };
        
        let emission_snapshot = {
            let emission = self.emission.read().await;
            Some(emission.to_json())
        };
        
        let slashing_snapshot = {
            let slashing = self.slashing.read().await;
            Some(slashing.to_json())
        };

        let state = self.inner.read().await;
        
        let genesis_hash = if let Some(ref hash) = state.genesis_hash {
            Some(hash.clone())
        } else {
            let mut found = None;
            for node in state.dag.get_all_nodes() {
                if node.tx.tx.from == "genesis" {
                    found = Some(node.hash.clone());
                    break;
                }
            }
            if found.is_none() {
                if let Some(first_checkpoint) = state.checkpoints.first() {
                    found = Some(first_checkpoint.tx_merkle_root.clone());
                }
            }
            found
        };

        SyncSnapshot {
            accounts: state.accounts.clone(),
            validators: state.validators.clone(),
            checkpoints: state.checkpoints.clone(),
            gas_price: state.current_gas_price,
            total_supply: state.total_supply,
            genesis_time: state.genesis_time,
            dag_transactions: dag_txs,
            total_transactions: std::cmp::max(state.total_transactions, dag_tx_count),
            contracts,
            rewards_snapshot,
            emission_snapshot,
            slashing_snapshot,
            total_burned,
            total_to_validators,
            genesis_hash,
            finalized_tx_hashes,
            tx_checkpoint_heights,
            weight_scores: {
                if let Some(ref wt) = state.weight_trie {
                    wt.all_weights().clone()
                } else {
                    HashMap::new()
                }
            },
        }
    }

    pub async fn apply_sync_snapshot(&self, snapshot: SyncSnapshot) -> Result<usize> {
        self.apply_sync_snapshot_inner(snapshot, false).await
    }

    pub async fn apply_sync_snapshot_force(&self, snapshot: SyncSnapshot) -> Result<usize> {
        self.apply_sync_snapshot_inner(snapshot, true).await
    }

    async fn apply_sync_snapshot_inner(&self, snapshot: SyncSnapshot, force: bool) -> Result<usize> {
        let mut state = self.inner.write().await;

        let local_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
        let peer_height = snapshot.checkpoints.last().map(|cp| cp.height).unwrap_or(0);

        if !force && peer_height <= local_height && local_height > 0 {
            info!(
                "Skipping snapshot apply: local at height {}, peer at height {}",
                local_height, peer_height
            );
            return Ok(0);
        }
        
        if force {
            warn!(
                "RECOVERY MODE: Force applying snapshot ({} accounts, height {}) to fix state divergence",
                snapshot.accounts.len(), peer_height
            );
        }

        info!(
            "Applying sync snapshot: {} accounts, {} checkpoints, {} dag txs",
            snapshot.accounts.len(),
            snapshot.checkpoints.len(),
            snapshot.dag_transactions.len()
        );

        let mut merged_accounts = state.accounts.clone();
        let mut accounts_added = 0;
        let mut accounts_updated = 0;
        let mut accounts_balance_fixed = 0;
        
        let peer_fingerprints: std::collections::HashSet<String> = 
            snapshot.accounts.keys().cloned().collect();
        let mut local_only_accounts: HashMap<String, Account> = HashMap::new();
        
        for (fingerprint, local_account) in state.accounts.iter() {
            if !peer_fingerprints.contains(fingerprint) {
                local_only_accounts.insert(fingerprint.clone(), local_account.clone());
            }
        }
        
        for (fingerprint, peer_account) in snapshot.accounts.iter() {
            if let Some(local_account) = merged_accounts.get(fingerprint) {
                if peer_account.nonce > local_account.nonce {
                    merged_accounts.insert(fingerprint.clone(), peer_account.clone());
                    accounts_updated += 1;
                } else if peer_account.nonce == local_account.nonce {
                    let balance_diff = peer_account.balance.abs_diff(local_account.balance);
                    let stake_diff = peer_account.staked.abs_diff(local_account.staked);
                    if balance_diff > 0 || stake_diff > 0 {
                        info!(
                            "Balance fix for {}: local={} peer={} (nonce={})",
                            &fingerprint[..fingerprint.len().min(12)], local_account.balance, peer_account.balance, peer_account.nonce
                        );
                        merged_accounts.insert(fingerprint.clone(), peer_account.clone());
                        accounts_balance_fixed += 1;
                    }
                }
            } else {
                merged_accounts.insert(fingerprint.clone(), peer_account.clone());
                accounts_added += 1;
            }
        }
        
        info!(
            "Account merge: {} added, {} updated (higher nonce), {} balance-fixed (same nonce), {} local-only, {} total",
            accounts_added, accounts_updated, accounts_balance_fixed, local_only_accounts.len(), merged_accounts.len()
        );
        
        state.accounts = merged_accounts;
        state.convergence_overlay.clear();
        state.convergence_executed_txs.clear();
        state.convergence_executed_order.clear();
        
        let old_validator_count = state.validators.len();
        let genesis_validators = &self.config.trust.genesis_validators;
        
        if !genesis_validators.is_empty() && self.config.is_genesis_node {
            use crate::validator_identity::GENESIS_VALIDATOR_STAKE;
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            
            let mut preserved_validators = HashMap::new();
            for gv in genesis_validators {
                let existing = state.validators.get(&gv.address)
                    .or_else(|| snapshot.validators.get(&gv.address));
                
                let validator = if let Some(existing) = existing {
                    let mut v = existing.clone();
                    if v.bls_public_key.is_none() {
                        v.bls_public_key = Some(hex::encode(&gv.bls_public_key));
                    }
                    v
                } else {
                    Validator {
                        address: gv.address.clone(),
                        stake: GENESIS_VALIDATOR_STAKE,
                        first_stake_time: now_secs * 1000,
                        bls_public_key: Some(hex::encode(&gv.bls_public_key)),
                        missed_checkpoints: 0,
                    }
                };
                preserved_validators.insert(gv.address.clone(), validator);
            }
            
            info!(
                "Validator sync: PRESERVED genesis validator set ({} -> {} validators, config-authoritative, genesis-only)",
                old_validator_count, preserved_validators.len()
            );
            state.validators = preserved_validators;
        } else {
            let local_validator_addr = state.node_validator_address.clone();
            let local_validator_bls = state.node_bls_public_key.clone();
            let local_validator_backup = local_validator_addr.as_ref()
                .and_then(|addr| state.validators.get(addr).cloned());
            
            let mut new_validators = snapshot.validators.clone();
            
            if !genesis_validators.is_empty() {
                use crate::validator_identity::GENESIS_VALIDATOR_STAKE;
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let mut config_added = 0;
                for gv in genesis_validators {
                    if !new_validators.contains_key(&gv.address) {
                        let existing = state.validators.get(&gv.address);
                        let validator = if let Some(existing) = existing {
                            let mut v = existing.clone();
                            if v.bls_public_key.is_none() {
                                v.bls_public_key = Some(hex::encode(&gv.bls_public_key));
                            }
                            v
                        } else {
                            Validator {
                                address: gv.address.clone(),
                                stake: GENESIS_VALIDATOR_STAKE,
                                first_stake_time: now_secs * 1000,
                                bls_public_key: Some(hex::encode(&gv.bls_public_key)),
                                missed_checkpoints: 0,
                            }
                        };
                        new_validators.insert(gv.address.clone(), validator);
                        config_added += 1;
                    } else if let Some(existing) = new_validators.get_mut(&gv.address) {
                        if existing.bls_public_key.is_none() {
                            existing.bls_public_key = Some(hex::encode(&gv.bls_public_key));
                        }
                    }
                }
                if config_added > 0 {
                    info!(
                        "Validator sync: Added {} missing genesis validators from config to peer set",
                        config_added
                    );
                }
            }
            
            if let Some(ref local_addr) = local_validator_addr {
                let is_genesis_validator = genesis_validators.is_empty() 
                    || genesis_validators.iter().any(|gv| gv.address == *local_addr);
                
                if !new_validators.contains_key(local_addr) {
                    if !is_genesis_validator {
                        info!(
                            "Local validator {} not in GENESIS_VALIDATORS - NOT re-adding to synced set (non-validator node)",
                            &local_addr[..local_addr.len().min(16)]
                        );
                    } else if let Some(backup) = local_validator_backup {
                        info!("Re-adding local validator {} to synced set (from backup)", local_addr);
                        new_validators.insert(local_addr.clone(), backup);
                    } else {
                        use crate::validator_identity::MIN_VALIDATOR_STAKE;
                        let now_secs = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        let validator = Validator {
                            address: local_addr.clone(),
                            stake: MIN_VALIDATOR_STAKE,
                            first_stake_time: now_secs * 1000,
                            bls_public_key: local_validator_bls.clone(),
                            missed_checkpoints: 0,
                        };
                        info!("Re-registering local validator {} after snapshot sync", local_addr);
                        new_validators.insert(local_addr.clone(), validator);
                    }
                } else if let Some(existing) = new_validators.get_mut(local_addr) {
                    if existing.bls_public_key.is_none() && local_validator_bls.is_some() {
                        existing.bls_public_key = local_validator_bls.clone();
                    }
                }
            }
            
            info!(
                "Validator sync: ADOPTED peer validator set ({} -> {} validators, non-genesis)",
                old_validator_count, new_validators.len()
            );
            state.validators = new_validators;
        }
        
        let mut ghost_removed = 0;
        let ghost_candidates: Vec<String> = state.accounts
            .iter()
            .filter(|(addr, account)| {
                *addr != "faucet"
                    && *addr != "genesis"
                    && account.staked > 0
                    && account.nonce == 0
                    && !state.validators.contains_key(*addr)
            })
            .map(|(addr, _)| addr.clone())
            .collect();
        
        for addr in &ghost_candidates {
            if let Some(account) = state.accounts.get_mut(addr) {
                warn!(
                    "Ghost validator cleanup: zeroing stale stake on {} (staked={}, balance={}, nonce=0, not in validator set)",
                    &addr[..addr.len().min(16)], account.staked, account.balance
                );
                account.staked = 0;
                ghost_removed += 1;
            }
        }
        
        let stale_to_remove: Vec<String> = state.accounts
            .iter()
            .filter(|(addr, account)| {
                *addr != "faucet"
                    && *addr != "genesis"
                    && account.balance == 0
                    && account.staked == 0
                    && account.nonce == 0
                    && !state.validators.contains_key(*addr)
            })
            .map(|(addr, _)| addr.clone())
            .collect();
        
        for addr in &stale_to_remove {
            state.accounts.remove(addr);
        }
        
        if ghost_removed > 0 || !stale_to_remove.is_empty() {
            info!(
                "Post-sync cleanup: {} ghost stakes zeroed, {} empty accounts removed, {} accounts remaining",
                ghost_removed, stale_to_remove.len(), state.accounts.len()
            );
        }
        
        state.checkpoints = snapshot.checkpoints.clone();
        let sync_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
        self.checkpoint_height_cache.store(sync_height, std::sync::atomic::Ordering::Relaxed);
        state.current_gas_price = snapshot.gas_price;
        state.total_supply = snapshot.total_supply;
        state.genesis_time = snapshot.genesis_time;
        state.total_transactions = snapshot.total_transactions;
        
        state.contracts = snapshot.contracts;
        state.total_burned = snapshot.total_burned;
        state.total_to_validators = snapshot.total_to_validators;
        
        if !snapshot.weight_scores.is_empty() {
            let weight_count = snapshot.weight_scores.len();
            if let Some(ref mut wt) = state.weight_trie {
                wt.load_weights(snapshot.weight_scores);
            } else {
                let mut wt = WeightTrie::new();
                wt.load_weights(snapshot.weight_scores);
                state.weight_trie = Some(wt);
            }
            info!("Applied {} transaction weight scores from sync snapshot", weight_count);
        }
        
        let rewards_to_apply = snapshot.rewards_snapshot;
        let emission_to_apply = snapshot.emission_snapshot;
        let slashing_to_apply = snapshot.slashing_snapshot;

        if let Some(latest_cp) = snapshot.checkpoints.last() {
            state.last_checkpoint_time_ms = latest_cp.timestamp * 1000;
        }

        state.dag = rinku_core::Dag::new(10000);

        let genesis_hash = "genesis".to_string();
        let genesis_tx = SignedTransaction {
            tx: rinku_core::types::Transaction {
                from: "genesis".to_string(),
                to: "genesis".to_string(),
                amount: 0,
                nonce: 0,
                timestamp: snapshot.genesis_time * 1000,
                parents: vec![],
                kind: None,
                gas_price: None,
                gas_limit: None,
                data: None,
                signature: None,
                memo: None,
                references: None,
            },
            hash: genesis_hash.clone(),
            signature: "genesis".to_string(),
        };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let genesis_node = rinku_core::types::DagNode {
            hash: genesis_hash.clone(),
            tx: genesis_tx,
            parents: vec![],
            children: Vec::new(),
            weight: 1.0,
            finalized: true,
            checkpoint_height: Some(0),
            received_at_ms: Some(now_ms),
            partition_epoch: None,
            rolled_back: false,
            convergence_certificate: None,
        };
        let _ = state.dag.add_node(genesis_node);

        let snapshot_hashes: std::collections::HashSet<String> = snapshot
            .dag_transactions
            .iter()
            .map(|tx| tx.hash.clone())
            .collect();

        let normalize_parent = |p: &str| -> String {
            if p.starts_with("rinku://tx/h/") {
                p.strip_prefix("rinku://tx/h/").unwrap_or(p).to_string()
            } else if p.starts_with("rinku://tx/") {
                p.strip_prefix("rinku://tx/").unwrap_or(p).to_string()
            } else {
                p.to_string()
            }
        };

        let tx_parents: HashMap<String, Vec<String>> = snapshot
            .dag_transactions
            .iter()
            .map(|tx| {
                let parents: Vec<String> = tx
                    .tx
                    .parents
                    .iter()
                    .map(|p| normalize_parent(p))
                    .map(|p| {
                        if snapshot_hashes.contains(&p) {
                            p
                        } else {
                            genesis_hash.clone()
                        }
                    })
                    .collect();
                (tx.hash.clone(), parents)
            })
            .collect();

        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut children_map: HashMap<String, Vec<String>> = HashMap::new();

        for tx in &snapshot.dag_transactions {
            in_degree.entry(tx.hash.clone()).or_insert(0);
            for parent in tx_parents.get(&tx.hash).unwrap_or(&vec![]) {
                if snapshot_hashes.contains(parent) {
                    *in_degree.entry(tx.hash.clone()).or_insert(0) += 1;
                    children_map
                        .entry(parent.clone())
                        .or_default()
                        .push(tx.hash.clone());
                }
            }
        }

        let mut queue: Vec<String> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(hash, _)| hash.clone())
            .collect();

        let mut sorted_hashes: Vec<String> = Vec::new();
        while let Some(hash) = queue.pop() {
            sorted_hashes.push(hash.clone());
            if let Some(children) = children_map.get(&hash) {
                for child in children {
                    if let Some(deg) = in_degree.get_mut(child) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push(child.clone());
                        }
                    }
                }
            }
        }

        let tx_lookup: HashMap<String, SignedTransaction> = snapshot
            .dag_transactions
            .into_iter()
            .map(|tx| (tx.hash.clone(), tx))
            .collect();

        let actually_finalized: std::collections::HashSet<String> = if !snapshot.finalized_tx_hashes.is_empty() {
            snapshot.finalized_tx_hashes.into_iter().collect()
        } else {
            snapshot.checkpoints.iter()
                .flat_map(|cp| cp.finalized_tx_hashes.iter().cloned())
                .collect()
        };

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut added = 0;
        let mut unfinalized_carried = 0;
        for hash in sorted_hashes {
            let tx = match tx_lookup.get(&hash) {
                Some(tx) => tx,
                None => continue,
            };

            let normalized_parents = tx_parents.get(&hash).cloned().unwrap_or_default();
            
            let tx_weight = if let Some(account) = state.accounts.get(&tx.tx.from) {
                calculate_account_weight(account, now_secs)
            } else {
                1.0
            };

            let is_finalized = actually_finalized.contains(&hash);
            let checkpoint_height = if is_finalized {
                snapshot.tx_checkpoint_heights.get(&tx.hash).copied()
                    .or(Some(peer_height))
            } else {
                None
            };

            if !is_finalized {
                unfinalized_carried += 1;
            }
            
            let node = rinku_core::types::DagNode {
                hash: tx.hash.clone(),
                tx: tx.clone(),
                parents: normalized_parents,
                children: Vec::new(),
                weight: tx_weight,
                finalized: is_finalized,
                checkpoint_height,
                received_at_ms: Some(tx.tx.timestamp),
                partition_epoch: None,
                rolled_back: false,
                convergence_certificate: None,
            };

            if state.dag.add_node(node).is_ok() {
                added += 1;
            }
        }

        if unfinalized_carried > 0 {
            info!(
                "Sync carried {} unfinalized transactions (not yet checkpointed) for future finalization",
                unfinalized_carried
            );
        }

        let validator_count = state.validators.len();
        let checkpoint_count = state.checkpoints.len();
        let contract_count = state.contracts.len();
        
        drop(state);
        
        if let Some(rewards_snap) = rewards_to_apply {
            let mut rewards = self.rewards.write().await;
            rewards.merge_from(rewards_snap);
            
            let stake_reconciliations: Vec<(String, u64, u64)> = rewards
                .get_all_stakes()
                .iter()
                .map(|pos| (pos.staker.clone(), pos.amount, pos.staked_at))
                .collect();
            drop(rewards);
            
            if !stake_reconciliations.is_empty() {
                let mut state = self.inner.write().await;
                let mut reconciled_count = 0;
                for (staker, amount, staked_at) in &stake_reconciliations {
                    if let Some(account) = state.accounts.get_mut(staker) {
                        let diff = account.staked.abs_diff(*amount);
                        if diff > 0 {
                            info!(
                                "Reconciling account.staked for {}: {} -> {} (diff: {})",
                                &staker[..staker.len().min(12)],
                                account.staked,
                                amount,
                                diff
                            );
                            account.staked = *amount;
                            reconciled_count += 1;
                        }
                    }
                }
                if reconciled_count > 0 {
                    info!("Reconciled {} account stake values from rewards snapshot", reconciled_count);
                }
            }
        }
        
        if let Some(emission_snap) = emission_to_apply {
            let mut emission = self.emission.write().await;
            let (emitted_delta, burned_delta) = emission.merge_from(emission_snap);
            if emitted_delta > 0 || burned_delta > 0 {
                info!(
                    "Merged emission snapshot: +{} emitted, +{} burned",
                    emitted_delta, burned_delta
                );
            }
        }
        
        if let Some(slashing_snap) = slashing_to_apply {
            let mut slashing = self.slashing.write().await;
            let result = slashing.merge_from(slashing_snap);
            if result.events_added > 0 || result.unbonding_added > 0 || result.liveness_updated > 0 {
                info!(
                    "Merged slashing snapshot: {} events added, {} unbonding added, {} liveness updated, +{:.6} total_slashed",
                    result.events_added, result.unbonding_added, result.liveness_updated, result.total_slashed_delta
                );
            }
        }

        info!(
            "Snapshot applied: {} DAG txs, {} validators, {} checkpoints, {} contracts",
            added, validator_count, checkpoint_count, contract_count
        );
        Ok(added)
    }
}
