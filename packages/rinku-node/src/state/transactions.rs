use super::*;

#[derive(Debug, Clone, PartialEq)]
pub enum ConvergenceExecResult {
    Executed,
    AlreadyApplied,
    Rejected,
    Deferred,
}

impl NodeState {
    pub async fn add_transaction(&self, tx: SignedTransaction) -> Result<TransactionResult> {
        let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
        let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
        let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
        let is_consolidation_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Consolidation));
        let is_contract_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Contract));
        
        let is_system_tx = is_consolidation_tx 
            || tx.signature.starts_with("anchor-")
            || tx.tx.from == "faucet"
            || tx.tx.from == "genesis";
        
        if !is_system_tx {
            let state = self.inner.read().await;
            let current_gas_price = state.current_gas_price;
            
            if let Some(offered_gas) = tx.tx.gas_price {
                if offered_gas < current_gas_price {
                    let offered_rku = rinku_core::types::from_micro_units(offered_gas);
                    let required_rku = rinku_core::types::from_micro_units(current_gas_price);
                    tracing::debug!(
                        "Transaction rejected: gas price too low ({:.6} < {:.6} RKU)",
                        offered_rku, required_rku
                    );
                    return Err(anyhow::anyhow!(
                        "Gas price too low: offered {:.6} RKU, current minimum is {:.6} RKU",
                        offered_rku, required_rku
                    ));
                }
            }
            
            let gas_fee = tx.tx.gas_price.unwrap_or(current_gas_price);
            
            if state.partition_state.status == crate::state::partition::PartitionStatus::Partitioned {
                let tx_kind = tx.tx.kind.unwrap_or(rinku_core::types::TransactionKind::Transfer);
                let safety = tx_kind.partition_safety();
                
                match safety {
                    rinku_core::types::PartitionSafety::CpOnly => {
                        tracing::info!(
                            "Transaction rejected during partition: {:?} transactions require full quorum",
                            tx_kind
                        );
                        return Err(anyhow::anyhow!(
                            "Transaction type '{:?}' is not allowed during network partition. \
                             Stake, unstake, and reward claim operations require full network quorum. \
                             The network is currently partitioned (epoch {}). \
                             Please wait for the partition to heal.",
                            tx_kind,
                            state.partition_state.current_epoch.unwrap_or(0)
                        ));
                    }
                    rinku_core::types::PartitionSafety::BoundedSpend => {
                        let tx_cost = tx.tx.amount + gas_fee;
                        if let Some(account) = state.accounts.get(&tx.tx.from) {
                            if let Some(budget) = account.partition_budget {
                                let remaining = budget - account.partition_budget_spent;
                                if tx_cost > remaining {
                                    tracing::info!(
                                        "Transaction rejected: exceeds partition budget. Cost: {:.6}, remaining budget: {:.6}",
                                        tx_cost, remaining
                                    );
                                    return Err(anyhow::anyhow!(
                                        "Transaction cost ({:.6} RKU) exceeds remaining partition budget ({:.6} RKU). \
                                         You set a partition spending limit of {:.6} RKU and have spent {:.6} RKU \
                                         during this partition epoch.",
                                        tx_cost, remaining, budget, account.partition_budget_spent
                                    ));
                                }
                            }
                        }
                    }
                    rinku_core::types::PartitionSafety::Safe => {}
                }
            }
            
            if is_stake_tx {
                let rewards = self.rewards.read().await;
                let min_stake = rewards.get_config().min_stake_amount;
                drop(rewards);
                
                if tx.tx.amount < min_stake {
                    tracing::warn!(
                        "Stake transaction rejected: amount {:.6} below minimum {:.6}",
                        tx.tx.amount, min_stake
                    );
                    return Err(anyhow::anyhow!(
                        "Minimum stake amount is {} RKU, you tried to stake {}",
                        min_stake, tx.tx.amount
                    ));
                }
            }
            
            const MAX_MEMO_SIZE: usize = 1024;
            if let Some(ref memo) = tx.tx.memo {
                if memo.len() > MAX_MEMO_SIZE {
                    tracing::warn!(
                        "Transaction rejected: memo too large ({} bytes, max {})",
                        memo.len(), MAX_MEMO_SIZE
                    );
                    return Err(anyhow::anyhow!(
                        "Memo too large: {} bytes (max {} bytes)",
                        memo.len(), MAX_MEMO_SIZE
                    ));
                }
            }
            
            const MAX_REFERENCES: usize = 4;
            if let Some(ref refs) = tx.tx.references {
                if refs.len() > MAX_REFERENCES {
                    tracing::warn!(
                        "Transaction rejected: too many references ({}, max {})",
                        refs.len(), MAX_REFERENCES
                    );
                    return Err(anyhow::anyhow!(
                        "Too many references: {} (max {})",
                        refs.len(), MAX_REFERENCES
                    ));
                }
            }
            
            if is_contract_tx {
                const MAX_CONTRACT_DATA_SIZE: usize = 3 * 1024 * 1024;
                match &tx.tx.data {
                    None => {
                        tracing::warn!("Contract transaction rejected: missing data field");
                        return Err(anyhow::anyhow!(
                            "Contract transactions require a 'data' field with deploy or call payload"
                        ));
                    }
                    Some(data) => {
                        if data.len() > MAX_CONTRACT_DATA_SIZE {
                            tracing::warn!(
                                "Contract transaction rejected: data too large ({} bytes, max {})",
                                data.len(), MAX_CONTRACT_DATA_SIZE
                            );
                            return Err(anyhow::anyhow!(
                                "Contract data too large: {} bytes (max {} bytes)",
                                data.len(), MAX_CONTRACT_DATA_SIZE
                            ));
                        }
                        match rinku_core::types::ContractTransactionData::from_data_field(data) {
                            Ok(contract_data) => {
                                match &contract_data {
                                    rinku_core::types::ContractTransactionData::Deploy { wasm_base64, .. } => {
                                        let wasm_size_estimate = wasm_base64.len() * 3 / 4;
                                        const MAX_WASM_SIZE: usize = 2 * 1024 * 1024;
                                        if wasm_size_estimate > MAX_WASM_SIZE {
                                            return Err(anyhow::anyhow!(
                                                "WASM binary too large: ~{} bytes (max {})",
                                                wasm_size_estimate, MAX_WASM_SIZE
                                            ));
                                        }
                                        if wasm_base64.is_empty() {
                                            return Err(anyhow::anyhow!("WASM binary cannot be empty"));
                                        }
                                    }
                                    rinku_core::types::ContractTransactionData::Call { contract_id, entrypoint, .. } => {
                                        if contract_id.is_empty() {
                                            return Err(anyhow::anyhow!("Contract ID cannot be empty"));
                                        }
                                        if entrypoint.is_empty() {
                                            return Err(anyhow::anyhow!("Entrypoint cannot be empty"));
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Contract transaction rejected: invalid data: {}", e);
                                return Err(anyhow::anyhow!("{}", e));
                            }
                        }
                    }
                }
            }

            use crate::config::MAX_FUTURE_TIMESTAMP_MS;
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            
            if tx.tx.timestamp > now_ms + MAX_FUTURE_TIMESTAMP_MS {
                tracing::warn!(
                    "Transaction rejected: timestamp {} too far in future (max {} ahead)",
                    tx.tx.timestamp, MAX_FUTURE_TIMESTAMP_MS
                );
                return Err(anyhow::anyhow!(
                    "Transaction timestamp is too far in the future"
                ));
            }
            
            let required_balance = if is_stake_tx {
                tx.tx.amount + gas_fee
            } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                gas_fee
            } else {
                tx.tx.amount + gas_fee
            };
            
            if tx.tx.from != "genesis" {
                if !state.accounts.contains_key(&tx.tx.from) {
                    tracing::warn!(
                        "Transaction rejected: account {} does not exist",
                        &tx.tx.from[..16.min(tx.tx.from.len())]
                    );
                    return Err(anyhow::anyhow!("Account does not exist"));
                }
                
                let (effective_balance, effective_nonce) =
                    Self::get_effective_balance_and_nonce(&state, &tx.tx.from);
                if effective_balance < required_balance {
                    tracing::warn!(
                        "Transaction rejected: insufficient effective balance. Have {:.6}, need {:.6} (amount: {:.6}, gas: {:.6})",
                        effective_balance, required_balance, tx.tx.amount, gas_fee
                    );
                    return Err(anyhow::anyhow!(
                        "Insufficient balance: have {:.6}, need {:.6}",
                        effective_balance, required_balance
                    ));
                }

                let confirmed_nonce = state.accounts.get(&tx.tx.from).map(|a| a.nonce).unwrap_or(0);

                if tx.tx.nonce < confirmed_nonce {
                    tracing::warn!(
                        "Transaction rejected: stale nonce. Confirmed nonce is {}, got {} (already finalized)",
                        confirmed_nonce, tx.tx.nonce
                    );
                    return Err(anyhow::anyhow!(
                        "Stale nonce: confirmed nonce is {}, got {} (already finalized)",
                        confirmed_nonce, tx.tx.nonce
                    ));
                }

                if tx.tx.nonce != effective_nonce {
                    tracing::debug!(
                        "Nonce mismatch for {}: expected effective {}, got {}",
                        &tx.tx.from[..16.min(tx.tx.from.len())],
                        effective_nonce, tx.tx.nonce
                    );
                    return Err(anyhow::anyhow!(
                        "Invalid nonce: expected {}, got {}",
                        effective_nonce, tx.tx.nonce
                    ));
                }
            }
        }
        
        let client_parents: Vec<String> = tx
            .tx
            .parents
            .iter()
            .map(|p| {
                if p.starts_with("rinku://tx/h/") {
                    p.strip_prefix("rinku://tx/h/").unwrap_or(p).to_string()
                } else if p.starts_with("rinku://tx/") {
                    p.strip_prefix("rinku://tx/").unwrap_or(p).to_string()
                } else {
                    p.clone()
                }
            })
            .collect();

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let (tx_weight, normalized_parents) = {
            let state = self.inner.read().await;
            
            let weight = if let Some(account) = state.accounts.get(&tx.tx.from) {
                calculate_account_weight(account, now_secs)
            } else {
                1.0
            };
            
            let valid_parents: Vec<String> = client_parents
                .iter()
                .filter(|p| !p.is_empty() && state.dag.get_node(p).is_some())
                .cloned()
                .collect();
            
            let final_parents = if valid_parents.is_empty() {
                let current_tips = state.dag.tips();
                let injected: Vec<String> = current_tips.into_iter().take(2).collect();
                if !injected.is_empty() {
                    tracing::debug!(
                        "Tip injection: tx {} had {} orphan parents, injecting {} tips",
                        &tx.hash[..16.min(tx.hash.len())],
                        client_parents.len(),
                        injected.len()
                    );
                }
                injected
            } else {
                valid_parents
            };
            
            (weight, final_parents)
        };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let node = rinku_core::types::DagNode {
            hash: tx.hash.clone(),
            tx: tx.clone(),
            parents: normalized_parents.clone(),
            children: Vec::new(),
            weight: tx_weight,
            finalized: false,
            checkpoint_height: None,
            received_at_ms: Some(now_ms),
            partition_epoch: None,
            rolled_back: false,
            convergence_certificate: None,
        };

        let mut state = self.inner.write().await;

        if !is_system_tx {
            let gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);
            let required_balance = if is_stake_tx {
                tx.tx.amount + gas_fee
            } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                gas_fee
            } else {
                tx.tx.amount + gas_fee
            };

            let (effective_balance, effective_nonce) =
                Self::get_effective_balance_and_nonce(&state, &tx.tx.from);
            if effective_balance < required_balance {
                return Err(anyhow::anyhow!(
                    "Insufficient balance: have {:.6}, need {:.6}",
                    effective_balance, required_balance
                ));
            }
            if tx.tx.nonce != effective_nonce {
                return Err(anyhow::anyhow!(
                    "Invalid nonce: expected {}, got {}",
                    effective_nonce, tx.tx.nonce
                ));
            }
        }

        state.dag.add_node(node)?;

        drop(state);

        Ok(TransactionResult::Accepted)
    }
    
    pub async fn execute_confirmed_transaction_state(&self, tx: &SignedTransaction) -> ConvergenceExecResult {
        if matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Consolidation)) {
            return ConvergenceExecResult::AlreadyApplied;
        }

        let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
        let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
        let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
        let is_contract_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Contract));

        let gas_fee;

        {
            let mut state = self.inner.write().await;
            gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);

            {
                let from_account = match state.accounts.get(&tx.tx.from) {
                    Some(acc) => acc,
                    None => return ConvergenceExecResult::Rejected,
                };
                let required = if is_stake_tx {
                    tx.tx.amount + gas_fee
                } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                    gas_fee
                } else {
                    tx.tx.amount + gas_fee
                };
                if from_account.balance < required {
                    tracing::warn!(
                        "FastPath execution rejected: {} insufficient balance ({} < {})",
                        &tx.tx.from[..16.min(tx.tx.from.len())],
                        from_account.balance,
                        required
                    );
                    return ConvergenceExecResult::Rejected;
                }
                if tx.tx.nonce < from_account.nonce {
                    return ConvergenceExecResult::AlreadyApplied;
                }
                if tx.tx.nonce > from_account.nonce {
                    tracing::info!(
                        "FastPath DEFERRED: {} nonce {} > expected {} (will cascade when nonce {} executes)",
                        &tx.tx.from[..16.min(tx.tx.from.len())],
                        tx.tx.nonce,
                        from_account.nonce,
                        from_account.nonce
                    );
                    return ConvergenceExecResult::Deferred;
                }
            }

            let is_in_partition = state.partition_state.status == crate::state::partition::PartitionStatus::Partitioned;

            if let Some(from_account) = state.accounts.get_mut(&tx.tx.from) {
                let tx_cost = if is_stake_tx {
                    tx.tx.amount + gas_fee
                } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                    gas_fee
                } else {
                    tx.tx.amount + gas_fee
                };
                from_account.balance -= tx_cost;
                from_account.nonce = tx.tx.nonce + 1;

                if is_in_partition && from_account.partition_budget.is_some() {
                    from_account.partition_budget_spent += tx_cost;
                }
            }

            if !is_stake_tx && !is_unstake_tx && !is_claim_tx && !is_contract_tx {
                let to_account = state
                    .accounts
                    .entry(tx.tx.to.clone())
                    .or_insert_with(|| Account::new(tx.tx.to.clone(), tx.tx.timestamp));
                to_account.balance += tx.tx.amount;
            }

            state.total_burned += gas_fee / 2;
            state.total_to_validators += gas_fee / 2;

            state.total_transactions += 1;

            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let tx_time_ms = if tx.tx.timestamp < 4_000_000_000 {
                tx.tx.timestamp * 1000
            } else {
                tx.tx.timestamp
            };
            let finality_time_ms = now_ms.saturating_sub(tx_time_ms);
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
            state.convergence_executed_hashes.insert(tx.hash.clone());
        }

        self.execute_transaction_side_effects(tx).await;

        tracing::info!(
            "FastPath EXECUTED tx {} ({} -> {}, amount={}, gas={})",
            &tx.hash[..16.min(tx.hash.len())],
            &tx.tx.from[..16.min(tx.tx.from.len())],
            &tx.tx.to[..16.min(tx.tx.to.len())],
            tx.tx.amount,
            gas_fee
        );

        ConvergenceExecResult::Executed
    }

    pub async fn execute_finalized_transaction(&self, tx: &SignedTransaction) {
        let applied = self.execute_finalized_transaction_core(tx).await;
        if applied {
            self.execute_finalized_transaction_rewards(tx).await;
        }
    }

    pub async fn execute_finalized_transactions_batch(
        &self,
        txs: &[SignedTransaction],
        convergence_already_executed: &std::collections::HashSet<String>,
    ) {
        let mut prev_deferred = {
            let mut deferred = self.deferred_batch_txs.lock().await;
            std::mem::take(&mut *deferred)
        };

        if txs.is_empty() && prev_deferred.is_empty() {
            return;
        }

        const MAX_DEFERRED_RETRIES: u32 = 3;

        let mut retry_counts = {
            let counts = self.deferred_batch_retry_counts.lock().await;
            counts.clone()
        };

        let mut expired_count = 0usize;
        if !prev_deferred.is_empty() {
            prev_deferred.retain(|dtx| {
                if convergence_already_executed.contains(&dtx.hash) {
                    return false;
                }
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

        let mut all_txs: Vec<SignedTransaction> = Vec::new();
        let mut convergence_skipped = 0usize;
        for tx in txs {
            if convergence_already_executed.contains(&tx.hash) {
                convergence_skipped += 1;
            } else {
                all_txs.push(tx.clone());
            }
        }
        let from_deferred = prev_deferred.len();
        if !prev_deferred.is_empty() {
            let existing: std::collections::HashSet<String> = all_txs.iter().map(|t| t.hash.clone()).collect();
            for dtx in prev_deferred.drain(..) {
                if !existing.contains(&dtx.hash) {
                    all_txs.push(dtx);
                }
            }
        }

        all_txs.sort_by(|a, b| {
            a.tx.from.cmp(&b.tx.from)
                .then(a.tx.nonce.cmp(&b.tx.nonce))
                .then(a.hash.cmp(&b.hash))
        });

        let available_nonces: std::collections::HashMap<String, std::collections::BTreeSet<u64>> = {
            let mut map: std::collections::HashMap<String, std::collections::BTreeSet<u64>> = std::collections::HashMap::new();
            for tx in &all_txs {
                map.entry(tx.tx.from.clone()).or_default().insert(tx.tx.nonce);
            }
            map
        };

        let batch_start = std::time::Instant::now();
        let total_finalized = all_txs.len() + convergence_skipped;
        let mut executed_count = 0usize;
        let mut new_deferred: Vec<SignedTransaction> = Vec::new();
        let mut special_txs: Vec<SignedTransaction> = Vec::new();
        let mut executed_hashes: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut gap_skipped_senders: std::collections::HashSet<String> = std::collections::HashSet::new();

        {
            let mut state = self.inner.write().await;
            let is_in_partition = state.partition_state.status == crate::state::partition::PartitionStatus::Partitioned;
            let current_gas_price = state.current_gas_price;

            for tx in &all_txs {
                if matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Consolidation)) {
                    continue;
                }

                if gap_skipped_senders.contains(&tx.tx.from) {
                    new_deferred.push(tx.clone());
                    continue;
                }

                let gas_fee = tx.tx.gas_price.unwrap_or(current_gas_price);

                let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
                let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
                let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
                let is_contract_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Contract));

                let sender_exists = state.accounts.contains_key(&tx.tx.from);
                let mut tx_applied = false;

                if let Some(from_account) = state.accounts.get_mut(&tx.tx.from) {
                    if tx.tx.nonce != from_account.nonce {
                        if tx.tx.nonce > from_account.nonce {
                            let needed_nonce = from_account.nonce;
                            if let Some(sender_nonces) = available_nonces.get(&tx.tx.from) {
                                if !sender_nonces.contains(&needed_nonce) {
                                    gap_skipped_senders.insert(tx.tx.from.clone());
                                    tracing::info!(
                                        "Batch GAP-SKIP sender {} — needs nonce {} but not in batch (have {:?}), deferring all",
                                        &tx.tx.from[..16.min(tx.tx.from.len())],
                                        needed_nonce,
                                        sender_nonces.iter().take(5).collect::<Vec<_>>()
                                    );
                                    new_deferred.push(tx.clone());
                                    continue;
                                }
                            }
                            new_deferred.push(tx.clone());
                        }
                        continue;
                    }
                    let tx_cost = if is_stake_tx {
                        tx.tx.amount + gas_fee
                    } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                        gas_fee
                    } else {
                        tx.tx.amount + gas_fee
                    };
                    if from_account.balance < tx_cost {
                        tracing::warn!(
                            "Batch SKIP insufficient-balance tx {} from {} (bal={} < cost={})",
                            &tx.hash[..16.min(tx.hash.len())],
                            &tx.tx.from[..16.min(tx.tx.from.len())],
                            from_account.balance,
                            tx_cost
                        );
                        continue;
                    }
                    from_account.balance -= tx_cost;
                    from_account.nonce = tx.tx.nonce + 1;
                    tx_applied = true;

                    if is_in_partition && from_account.partition_budget.is_some() {
                        from_account.partition_budget_spent += tx_cost;
                    }
                }

                if tx_applied || !sender_exists {
                    if !is_stake_tx && !is_unstake_tx && !is_claim_tx && !is_contract_tx {
                        let to_account = state
                            .accounts
                            .entry(tx.tx.to.clone())
                            .or_insert_with(|| Account::new(tx.tx.to.clone(), tx.tx.timestamp));
                        to_account.balance += tx.tx.amount;
                    }

                    state.total_burned += gas_fee / 2;
                    state.total_to_validators += gas_fee / 2;
                    state.total_transactions += 1;
                    executed_count += 1;
                    executed_hashes.insert(tx.hash.clone());
                }

                if (tx_applied || !sender_exists) && tx.tx.kind.is_some() && !matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Consolidation)) {
                    special_txs.push(tx.clone());
                }
            }
        }

        if !new_deferred.is_empty() {
            const MAX_DEFERRED: usize = 500;
            if new_deferred.len() > MAX_DEFERRED {
                tracing::warn!(
                    "Batch deferred queue overflow: {} txs exceeds cap {}, dropping oldest",
                    new_deferred.len(), MAX_DEFERRED
                );
                new_deferred.sort_by(|a, b| a.tx.nonce.cmp(&b.tx.nonce));
                new_deferred.truncate(MAX_DEFERRED);
            }
            tracing::warn!(
                "Batch deferred {} txs with future nonces (will retry on next checkpoint)",
                new_deferred.len()
            );
        }

        {
            let final_hashes: std::collections::HashSet<&str> = new_deferred.iter().map(|t| t.hash.as_str()).collect();
            for dtx in &new_deferred {
                *retry_counts.entry(dtx.hash.clone()).or_insert(0) += 1;
            }
            retry_counts.retain(|hash, _| final_hashes.contains(hash.as_str()));
        }

        {
            let mut deferred = self.deferred_batch_txs.lock().await;
            *deferred = new_deferred;
            let mut counts = self.deferred_batch_retry_counts.lock().await;
            *counts = retry_counts;
        }

        if !special_txs.is_empty() {
            let mut unstake_credits: Vec<(String, u64)> = Vec::new();
            let mut claim_credits: Vec<(String, u64)> = Vec::new();
            let mut stake_updates: Vec<(String, u64, u64)> = Vec::new();

            {
                let mut rewards = self.rewards.write().await;
                for tx in &special_txs {
                    use rinku_core::types::TransactionKind;
                    let from_addr = &tx.tx.from;

                    match tx.tx.kind.as_ref().unwrap() {
                        TransactionKind::Stake => {
                            if let Err(e) = rewards.stake(from_addr, tx.tx.amount) {
                                tracing::warn!("Failed to process stake tx: {}", e);
                            } else {
                                if let Some(p) = rewards.get_stake(from_addr) {
                                    stake_updates.push((from_addr.clone(), p.amount, p.staked_at));
                                }
                            }
                        }
                        TransactionKind::Unstake => {
                            match rewards.unstake(from_addr) {
                                Ok(amount) => {
                                    unstake_credits.push((from_addr.clone(), amount));
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to process unstake tx: {}", e);
                                }
                            }
                        }
                        TransactionKind::ClaimRewards => {
                            let claimed = rewards.claim_rewards(from_addr);
                            if claimed > 0 {
                                claim_credits.push((from_addr.clone(), claimed));
                            }
                        }
                        _ => {}
                    }
                }
            }

            if !unstake_credits.is_empty() || !claim_credits.is_empty() || !stake_updates.is_empty() {
                let mut state = self.inner.write().await;
                for (addr, amount) in &unstake_credits {
                    if let Some(account) = state.accounts.get_mut(addr) {
                        account.balance += amount;
                        account.staked = 0;
                    }
                }
                for (addr, claimed) in &claim_credits {
                    if let Some(account) = state.accounts.get_mut(addr) {
                        account.balance += claimed;
                    }
                }
                for (addr, amount, staked_at) in &stake_updates {
                    if let Some(account) = state.accounts.get_mut(addr) {
                        account.staked = *amount;
                        account.first_seen = *staked_at / 1000;
                    }
                }
            }

            for tx in &special_txs {
                if matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Contract)) {
                    if let Some(ref data) = tx.tx.data {
                        match rinku_core::types::ContractTransactionData::from_data_field(data) {
                            Ok(contract_data) => {
                                self.execute_contract_transaction(tx, contract_data).await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to parse contract tx data during finalization: {}", e);
                            }
                        }
                    }
                }
            }
        }

        struct TxRewardInfo {
            tx_url: String,
            reward_base: u64,
            first_parent_hash: Option<String>,
            witness_parents: Vec<(String, String)>,
        }

        let reward_infos: Vec<TxRewardInfo> = {
            let state = self.inner.read().await;
            let current_gas_price = state.current_gas_price;
            all_txs.iter()
                .filter(|tx| executed_hashes.contains(&tx.hash))
                .filter(|tx| !matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Consolidation)))
                .filter_map(|tx| {
                    let gas_fee = tx.tx.gas_price.unwrap_or(current_gas_price);
                    let tx_amount = tx.tx.amount;
                    if tx_amount == 0 && gas_fee == 0 {
                        return None;
                    }
                    let reward_base = tx_amount + gas_fee;
                    let tx_url = format!("rinku://tx/h/{}", tx.hash);
                    let from_addr = tx.tx.from.clone();

                    fn normalize_parent(p: &str) -> &str {
                        if p.starts_with("rinku://tx/h/") {
                            p.strip_prefix("rinku://tx/h/").unwrap_or(p)
                        } else if p.starts_with("rinku://tx/") {
                            p.strip_prefix("rinku://tx/").unwrap_or(p)
                        } else {
                            p
                        }
                    }

                    let first_parent_hash = tx.tx.parents.first().map(|p| normalize_parent(p).to_string());

                    let witness_parents: Vec<(String, String)> = tx.tx.parents.iter()
                        .filter_map(|parent_ref| {
                            let ph = normalize_parent(parent_ref);
                            state.dag.get_node(ph).and_then(|node| {
                                let creator = node.tx.tx.from.clone();
                                if creator != from_addr {
                                    Some((format!("rinku://tx/h/{}", ph), creator))
                                } else {
                                    None
                                }
                            })
                        })
                        .collect();

                    Some(TxRewardInfo { tx_url, reward_base, first_parent_hash, witness_parents })
                })
                .collect()
        };

        if !reward_infos.is_empty() {
            let validator_addr = {
                let state = self.inner.read().await;
                state.node_validator_address.clone()
            };

            let mut rewards = self.rewards.write().await;
            for info in &reward_infos {
                if let Some(ref validator) = validator_addr {
                    if let Some(ref parent_hash) = info.first_parent_hash {
                        let tip_url = format!("rinku://tx/h/{}", parent_hash);
                        rewards.process_tip_reward(&info.tx_url, &tip_url, validator, info.reward_base);
                    }
                }
                for (parent_url, parent_creator) in &info.witness_parents {
                    rewards.process_witness_reward(&info.tx_url, parent_url, parent_creator, info.reward_base);
                }
            }
        }

        let newly_failed = all_txs.len().saturating_sub(executed_count);
        if newly_failed > 0 && convergence_skipped == 0 {
            tracing::warn!(
                "Batch UNDERCOUNT: {} of {} finalized txs actually executed (skipped {})",
                executed_count, all_txs.len(), newly_failed
            );
        }
        tracing::info!(
            "Batch executed {}/{} finalized txs in {:?} ({} convergence-pre-executed, {} from deferred, {} expired, {} gap-skipped senders)",
            executed_count, total_finalized,
            batch_start.elapsed(),
            convergence_skipped, from_deferred, expired_count, gap_skipped_senders.len()
        );
    }

    pub async fn purge_stale_deferred_txs(&self) {
        let mut deferred = self.deferred_batch_txs.lock().await;
        if deferred.is_empty() {
            return;
        }
        let before = deferred.len();
        {
            let state = self.inner.read().await;
            deferred.retain(|tx| {
                if let Some(account) = state.accounts.get(&tx.tx.from) {
                    tx.tx.nonce >= account.nonce
                } else {
                    true
                }
            });
        }
        let purged = before - deferred.len();
        if purged > 0 {
            let mut counts = self.deferred_batch_retry_counts.lock().await;
            let remaining_hashes: std::collections::HashSet<&str> = deferred.iter().map(|t| t.hash.as_str()).collect();
            counts.retain(|hash, _| remaining_hashes.contains(hash.as_str()));
            tracing::info!(
                "Purged {} stale deferred txs after proof sync ({} remaining)",
                purged, deferred.len()
            );
        }
    }

    pub async fn execute_finalized_transaction_core(&self, tx: &SignedTransaction) -> bool {
        if matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Consolidation)) {
            return false;
        }

        let gas_fee = {
            let state = self.inner.read().await;
            tx.tx.gas_price.unwrap_or(state.current_gas_price)
        };
        
        let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
        let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
        let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
        let is_contract_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Contract));

        {
            let mut state = self.inner.write().await;
            let is_in_partition = state.partition_state.status == crate::state::partition::PartitionStatus::Partitioned;

            {
                if let Some(from_account) = state.accounts.get_mut(&tx.tx.from) {
                    if tx.tx.nonce < from_account.nonce {
                        tracing::debug!(
                            "Skipping already-executed tx {} (tx_nonce={} < account_nonce={})",
                            &tx.hash[..16.min(tx.hash.len())],
                            tx.tx.nonce,
                            from_account.nonce
                        );
                        return false;
                    }
                    let tx_cost = if is_stake_tx {
                        tx.tx.amount + gas_fee
                    } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                        gas_fee
                    } else {
                        tx.tx.amount + gas_fee
                    };
                    if from_account.balance < tx_cost {
                        tracing::warn!(
                            "Skipping insufficient-balance tx {} (bal={} < cost={})",
                            &tx.hash[..16.min(tx.hash.len())],
                            from_account.balance,
                            tx_cost
                        );
                        return false;
                    }
                    from_account.balance -= tx_cost;
                    from_account.nonce = tx.tx.nonce + 1;

                    if is_in_partition && from_account.partition_budget.is_some() {
                        from_account.partition_budget_spent += tx_cost;
                    }
                }
            
                if !is_stake_tx && !is_unstake_tx && !is_claim_tx && !is_contract_tx {
                    let to_account = state
                        .accounts
                        .entry(tx.tx.to.clone())
                        .or_insert_with(|| Account::new(tx.tx.to.clone(), tx.tx.timestamp));
                    to_account.balance += tx.tx.amount;
                }
            }
            
            state.total_burned += gas_fee / 2;
            state.total_to_validators += gas_fee / 2;
            state.total_transactions += 1;
        }
        
        self.execute_transaction_side_effects(tx).await;
        true
    }

    async fn execute_transaction_side_effects(&self, tx: &SignedTransaction) {
        if let Some(ref kind) = tx.tx.kind {
            use rinku_core::types::TransactionKind;
            let from_addr = &tx.tx.from;
            let stake_amount = tx.tx.amount;
            
            match kind {
                TransactionKind::Stake => {
                    let stake_update: Option<(u64, u64)> = {
                        let mut rewards = self.rewards.write().await;
                        if let Err(e) = rewards.stake(from_addr, stake_amount) {
                            tracing::warn!("Failed to process stake tx: {}", e);
                            None
                        } else {
                            tracing::debug!("Finalized stake: {} staked {} RKU", &from_addr[..16.min(from_addr.len())], stake_amount);
                            rewards.get_stake(from_addr).map(|p| (p.amount, p.staked_at))
                        }
                    };
                    if let Some((amount, staked_at)) = stake_update {
                        self.update_account_staked(from_addr, amount, Some(staked_at / 1000)).await;
                    }
                }
                TransactionKind::Unstake => {
                    let unstake_result: Option<u64> = {
                        let mut rewards = self.rewards.write().await;
                        match rewards.unstake(from_addr) {
                            Ok(amount) => {
                                tracing::debug!("Finalized unstake: {} unstaked {} RKU", &from_addr[..16.min(from_addr.len())], amount);
                                Some(amount)
                            }
                            Err(e) => {
                                tracing::warn!("Failed to process unstake tx: {}", e);
                                None
                            }
                        }
                    };
                    if let Some(unstaked_amount) = unstake_result {
                        let mut state = self.inner.write().await;
                        if let Some(account) = state.accounts.get_mut(from_addr) {
                            account.balance += unstaked_amount;
                            account.staked = 0;
                            tracing::info!(
                                "Unstake finalized: {} balance restored by {} RKU (new balance: {})",
                                &from_addr[..16.min(from_addr.len())],
                                unstaked_amount,
                                account.balance
                            );
                        }
                    }
                }
                TransactionKind::ClaimRewards => {
                    let claimed: u64 = {
                        let mut rewards = self.rewards.write().await;
                        rewards.claim_rewards(from_addr)
                    };
                    tracing::info!(
                        "[EXECUTION] Claim for {}: claimed_amount={:.8}",
                        &from_addr[..16.min(from_addr.len())],
                        claimed
                    );
                    if claimed > 0 {
                        let mut state = self.inner.write().await;
                        if let Some(account) = state.accounts.get_mut(from_addr) {
                            let old_balance = account.balance;
                            account.balance += claimed;
                            tracing::info!(
                                "[EXECUTION] Claim for {}: old_balance={:.8}, new_balance={:.8}",
                                &from_addr[..16.min(from_addr.len())],
                                old_balance,
                                account.balance
                            );
                        }
                    }
                }
                TransactionKind::Contract => {
                    if let Some(ref data) = tx.tx.data {
                        match rinku_core::types::ContractTransactionData::from_data_field(data) {
                            Ok(contract_data) => {
                                self.execute_contract_transaction(tx, contract_data).await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to parse contract tx data during finalization: {}", e);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    async fn execute_contract_transaction(
        &self,
        tx: &SignedTransaction,
        contract_data: rinku_core::types::ContractTransactionData,
    ) {
        let runtime = crate::contracts::ContractRuntime::new();
        let gas_price = {
            let state = self.inner.read().await;
            tx.tx.gas_price.unwrap_or(state.current_gas_price)
        };

        match contract_data {
            rinku_core::types::ContractTransactionData::Deploy { wasm_base64, init_state } => {
                let contract_id = crate::contracts::create_contract_id(&tx.tx.from, tx.tx.nonce);
                let deploy_url = format!("rinku://contract/{}", contract_id);

                let mut final_state = init_state.clone();

                let init_input: std::collections::HashMap<String, serde_json::Value> = std::collections::HashMap::new();
                let init_result = runtime.execute_with_caller(
                    &contract_id,
                    &wasm_base64,
                    "init",
                    &init_input,
                    &init_state,
                    1,
                    Some(1_000_000),
                    &tx.tx.from,
                    tx.tx.timestamp / 1000,
                );

                let execution_gas = init_result.gas_used;
                self.charge_contract_execution_fee(&tx.tx.from, execution_gas, gas_price).await;

                if init_result.success {
                    if let Some(ref diff) = init_result.state_diff {
                        for change in &diff.changes {
                            if let Some(ref new_value) = change.new_value {
                                final_state.insert(change.key.clone(), new_value.clone());
                            } else {
                                final_state.remove(&change.key);
                            }
                        }
                    }
                    tracing::info!(
                        "Contract {} init executed successfully ({} state keys, gas: {})",
                        contract_id, final_state.len(), execution_gas
                    );
                } else {
                    tracing::warn!(
                        "Contract {} init failed (non-fatal, gas: {}): {:?}",
                        contract_id, execution_gas, init_result.error
                    );
                }

                let state_hash = crate::contracts::compute_state_hash(&final_state);

                let contract_state = crate::contracts::ContractState {
                    contract_id: contract_id.clone(),
                    creator: tx.tx.from.clone(),
                    wasm_base64,
                    deploy_url,
                    state: final_state,
                    state_hash,
                    height: 0,
                    created_at: tx.tx.timestamp / 1000,
                    schema: None,
                };

                match self.store_contract(contract_state).await {
                    Ok(()) => {
                        tracing::info!(
                            "Contract {} deployed via finalized tx {} by {}",
                            contract_id, &tx.hash[..16.min(tx.hash.len())],
                            &tx.tx.from[..16.min(tx.tx.from.len())]
                        );
                    }
                    Err(e) => {
                        tracing::error!("Failed to store contract {} from tx: {}", contract_id, e);
                    }
                }
            }
            rinku_core::types::ContractTransactionData::Call { contract_id, entrypoint, input } => {
                let contract = match self.get_contract(&contract_id).await {
                    Some(c) => c,
                    None => {
                        tracing::error!(
                            "Contract {} not found during finalization of tx {}",
                            contract_id, &tx.hash[..16.min(tx.hash.len())]
                        );
                        return;
                    }
                };

                let result = runtime.execute_with_caller(
                    &contract_id,
                    &contract.wasm_base64,
                    &entrypoint,
                    &input,
                    &contract.state,
                    contract.height + 1,
                    tx.tx.gas_limit,
                    &tx.tx.from,
                    tx.tx.timestamp / 1000,
                );

                let execution_gas = result.gas_used;
                self.charge_contract_execution_fee(&tx.tx.from, execution_gas, gas_price).await;

                if result.success {
                    let mut new_state = contract.state.clone();
                    let new_height = contract.height + 1;

                    if let Some(ref diff) = result.state_diff {
                        for change in &diff.changes {
                            if let Some(ref new_value) = change.new_value {
                                new_state.insert(change.key.clone(), new_value.clone());
                            } else {
                                new_state.remove(&change.key);
                            }
                        }
                    }

                    let new_state_hash = crate::contracts::compute_state_hash(&new_state);

                    if let Err(e) = self.update_contract_state(
                        &contract_id,
                        new_state,
                        new_state_hash,
                        new_height,
                    ).await {
                        tracing::error!("Failed to update contract {} state: {}", contract_id, e);
                    } else {
                        tracing::info!(
                            "Contract {} call '{}' executed via finalized tx {} (height: {}, gas: {})",
                            contract_id, entrypoint,
                            &tx.hash[..16.min(tx.hash.len())],
                            new_height, execution_gas
                        );
                    }
                } else {
                    tracing::warn!(
                        "Contract {} call '{}' failed during finalization of tx {} (gas: {}): {:?}",
                        contract_id, entrypoint,
                        &tx.hash[..16.min(tx.hash.len())],
                        execution_gas,
                        result.error
                    );
                }
            }
        }
    }

    async fn charge_contract_execution_fee(&self, from: &str, gas_used: u64, gas_price: u64) {
        use crate::wasm_runtime::BASE_TX_GAS;
        let additional_gas = gas_used.saturating_sub(BASE_TX_GAS);
        let execution_fee = additional_gas * gas_price / BASE_TX_GAS;
        if execution_fee > 0 {
            let mut state = self.inner.write().await;
            if let Some(account) = state.accounts.get_mut(from) {
                account.balance = account.balance.saturating_sub(execution_fee);
            }
            state.total_burned += execution_fee / 2;
            state.total_to_validators += execution_fee / 2;
            tracing::info!(
                "Contract execution fee: {} total gas ({} additional) = {} micro from {}",
                gas_used, additional_gas, execution_fee, &from[..16.min(from.len())]
            );
        }
    }
    
    pub async fn execute_finalized_transaction_rewards(&self, tx: &SignedTransaction) {
        let gas_fee = {
            let state = self.inner.read().await;
            tx.tx.gas_price.unwrap_or(state.current_gas_price)
        };
        
        let tx_hash = &tx.hash;
        let tx_url = format!("rinku://tx/h/{}", tx_hash);
        let tx_amount = tx.tx.amount;
        let from_addr = &tx.tx.from;
        
        let (parent_creators, validator_addr, normalized_parents) = {
            let state = self.inner.read().await;
            let parents: Vec<String> = tx.tx.parents.iter()
                .map(|p| {
                    if p.starts_with("rinku://tx/h/") {
                        p.strip_prefix("rinku://tx/h/").unwrap_or(p).to_string()
                    } else if p.starts_with("rinku://tx/") {
                        p.strip_prefix("rinku://tx/").unwrap_or(p).to_string()
                    } else {
                        p.clone()
                    }
                })
                .collect();
            
            let creators: Vec<(String, String)> = parents.iter()
                .filter_map(|parent_hash| {
                    state.dag.get_node(parent_hash).map(|node| {
                        let parent_url = format!("rinku://tx/h/{}", parent_hash);
                        (parent_url, node.tx.tx.from.clone())
                    })
                })
                .collect();
            
            (creators, state.node_validator_address.clone(), parents)
        };
        
        if tx_amount > 0 || gas_fee > 0 {
            let reward_base = tx_amount + gas_fee;
            let mut rewards = self.rewards.write().await;
            
            if let Some(ref validator) = validator_addr {
                if let Some(first_parent) = normalized_parents.first() {
                    let tip_url = format!("rinku://tx/h/{}", first_parent);
                    rewards.process_tip_reward(&tx_url, &tip_url, validator, reward_base);
                }
            }
            
            for (parent_url, parent_creator) in &parent_creators {
                if parent_creator != from_addr {
                    rewards.process_witness_reward(&tx_url, parent_url, parent_creator, reward_base);
                }
            }
        }
    }


    pub async fn add_transaction_dag_only(&self, tx: SignedTransaction) -> Result<()> {
        let normalized_parents: Vec<String> = tx
            .tx
            .parents
            .iter()
            .map(|p| {
                if p.starts_with("rinku://tx/h/") {
                    p.strip_prefix("rinku://tx/h/").unwrap_or(p).to_string()
                } else if p.starts_with("rinku://tx/") {
                    p.strip_prefix("rinku://tx/").unwrap_or(p).to_string()
                } else {
                    p.clone()
                }
            })
            .collect();

        let _permit = self.dag_write_semaphore.acquire().await
            .map_err(|e| anyhow::anyhow!("DAG write semaphore closed: {}", e))?;
        let mut state = self.inner.write().await;
        
        if state.dag.get_node(&tx.hash).is_some() {
            return Ok(());
        }
        
        let existing_parents: Vec<String> = normalized_parents
            .into_iter()
            .filter(|p| p == "genesis" || state.dag.get_node(p).is_some())
            .collect();

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let node = rinku_core::types::DagNode {
            hash: tx.hash.clone(),
            tx: tx.clone(),
            parents: existing_parents,
            children: Vec::new(),
            weight: 1.0,
            finalized: false,
            checkpoint_height: None,
            received_at_ms: Some(now_ms),
            partition_epoch: None,
            rolled_back: false,
            convergence_certificate: None,
        };

        state.dag.add_node(node)?;
        Ok(())
    }
    
    pub async fn add_transaction_from_sync(&self, tx: SignedTransaction) -> Result<()> {
        self.add_transaction_dag_only(tx).await
    }

    pub async fn add_transaction_from_gossip(&self, tx: SignedTransaction) -> Result<TransactionResult> {
        let is_system_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Consolidation))
            || tx.signature.starts_with("anchor-")
            || tx.tx.from == "faucet"
            || tx.tx.from == "genesis";

        if !is_system_tx {
            let state = self.inner.read().await;
            let confirmed_nonce = state.accounts.get(&tx.tx.from).map(|a| a.nonce).unwrap_or(0);

            if tx.tx.nonce < confirmed_nonce {
                return Err(anyhow::anyhow!(
                    "Stale nonce: confirmed nonce is {}, got {}",
                    confirmed_nonce, tx.tx.nonce
                ));
            }
            
            if let Some(offered_gas) = tx.tx.gas_price {
                if offered_gas < state.current_gas_price {
                    return Err(anyhow::anyhow!(
                        "Gas price too low: offered {}, minimum {}",
                        offered_gas, state.current_gas_price
                    ));
                }
            }
        }

        self.add_transaction_dag_only(tx).await?;
        Ok(TransactionResult::Accepted)
    }
    
    pub async fn set_tx_checkpoint_height(&self, hash: &str, height: u64) {
        let mut state = self.inner.write().await;
        let _ = state.dag.mark_finalized(hash, height);
    }

    pub async fn set_convergence_certificate(&self, hash: &str, finality: &rinku_core::types::FastPathFinality) {
        let mut state = self.inner.write().await;
        if let Some(node) = state.dag.get_node_mut(hash) {
            node.convergence_certificate = Some(rinku_core::types::ConvergenceCertificate {
                total_stake: finality.total_stake_acked,
                quorum_required: finality.quorum_stake_required,
                confirmed_at_ms: finality.confirmed_at_ms.unwrap_or(0),
                acks: finality.acks.clone(),
            });
        }
    }
    
    #[cfg(feature = "p2p")]
    pub async fn apply_p2p_snapshot(&self, snapshot: crate::network::SnapshotData) -> anyhow::Result<()> {
        use tracing::info;
        
        let mut state = self.inner.write().await;
        
        info!("Applying P2P snapshot: {} accounts, {} validators, {} checkpoints",
              snapshot.accounts.len(), snapshot.validators.len(), snapshot.checkpoints.len());
        
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        
        for account_data in snapshot.accounts {
            let mut account = state.accounts.get(&account_data.address)
                .cloned()
                .unwrap_or_else(|| Account::new(account_data.address.clone(), now_ms));
            
            account.balance = account_data.balance;
            account.nonce = account_data.nonce;
            account.staked = account_data.stake;
            state.accounts.insert(account_data.address, account);
        }
        
        for validator_data in snapshot.validators {
            let validator = rinku_core::types::Validator {
                address: validator_data.address.clone(),
                stake: validator_data.stake,
                first_stake_time: 0,
                bls_public_key: Some(validator_data.bls_public_key),
                missed_checkpoints: 0,
            };
            state.validators.insert(validator_data.address, validator);
        }
        
        let genesis_validators = &self.config.trust.genesis_validators;
        if !genesis_validators.is_empty() {
            use crate::validator_identity::GENESIS_VALIDATOR_STAKE;
            let mut augmented = 0;
            for gv in genesis_validators {
                if !state.validators.contains_key(&gv.address) {
                    let validator = rinku_core::types::Validator {
                        address: gv.address.clone(),
                        stake: GENESIS_VALIDATOR_STAKE,
                        first_stake_time: now_ms / 1000,
                        bls_public_key: Some(hex::encode(&gv.bls_public_key)),
                        missed_checkpoints: 0,
                    };
                    state.validators.insert(gv.address.clone(), validator);
                    augmented += 1;
                }
            }
            if augmented > 0 {
                info!("P2P snapshot: augmented validator set with {} missing genesis validators (total: {})",
                      augmented, state.validators.len());
            }
        }
        
        for cp_data in snapshot.checkpoints {
            let checkpoint = rinku_core::types::Checkpoint {
                height: cp_data.height,
                tx_merkle_root: cp_data.merkle_root,
                state_root: String::new(),
                receipt_root: String::new(),
                timestamp: cp_data.timestamp,
                previous_hash: cp_data.previous_hash,
                tip_count: cp_data.tx_count as u32,
                hash: cp_data.hash.unwrap_or_default(),
                signer_bitmap: None,
                aggregated_signature: cp_data.signature,
                validator_signatures: Vec::new(),
                finalized_tx_hashes: Vec::new(),
                weight_trie_root: String::new(),
            provisional: false,
            partition_epoch: None,
            visible_stake_pct: None,
                merge_report_hash: None,
            };
            
            if !state.checkpoints.iter().any(|c| c.height == checkpoint.height) {
                state.checkpoints.push(checkpoint);
            }
        }
        
        state.checkpoints.sort_by_key(|c| c.height);
        let sync_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
        self.checkpoint_height_cache.store(sync_height, std::sync::atomic::Ordering::Relaxed);
        
        for tx_data in snapshot.recent_txs {
            let signed_tx = rinku_core::types::SignedTransaction {
                hash: tx_data.hash.clone(),
                tx: rinku_core::types::Transaction {
                    from: tx_data.from,
                    to: tx_data.to,
                    amount: tx_data.amount,
                    nonce: tx_data.nonce,
                    timestamp: tx_data.timestamp,
                    parents: tx_data.parents,
                    gas_price: Some(tx_data.gas_price),
                    gas_limit: None,
                    data: None,
                    signature: None,
                    kind: None,
                    memo: tx_data.memo,
                    references: tx_data.references,
                },
                signature: tx_data.signature,
            };
            
            if state.dag.get_node(&signed_tx.hash).is_none() {
                let now_dag = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                    
                let node = rinku_core::types::DagNode {
                    hash: signed_tx.hash.clone(),
                    tx: signed_tx,
                    parents: Vec::new(),
                    children: Vec::new(),
                    weight: 1.0,
                    finalized: false,
                    checkpoint_height: None,
                    received_at_ms: Some(now_dag),
                    partition_epoch: None,
                    rolled_back: false,
                    convergence_certificate: None,
                };
                let _ = state.dag.add_node(node);
            }
        }
        
        info!("P2P snapshot applied successfully");
        Ok(())
    }

    pub async fn force_add_transaction_for_vote(&self, tx: SignedTransaction) -> Result<()> {
        self.force_add_transactions_batch_for_vote(vec![tx]).await?;
        Ok(())
    }

    pub async fn force_add_transactions_batch_for_vote(&self, txs: Vec<SignedTransaction>) -> Result<usize> {
        if txs.is_empty() {
            return Ok(0);
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let prepared: Vec<(SignedTransaction, Vec<String>)> = txs.into_iter().map(|tx| {
            let normalized_parents: Vec<String> = tx
                .tx
                .parents
                .iter()
                .map(|p| {
                    if p.starts_with("rinku://tx/h/") {
                        p.strip_prefix("rinku://tx/h/").unwrap_or(p).to_string()
                    } else if p.starts_with("rinku://tx/") {
                        p.strip_prefix("rinku://tx/").unwrap_or(p).to_string()
                    } else {
                        p.clone()
                    }
                })
                .collect();
            (tx, normalized_parents)
        }).collect();

        let mut state = self.inner.write().await;
        let mut added = 0usize;

        for (tx, normalized_parents) in prepared {
            if state.dag.get_node(&tx.hash).is_some() {
                continue;
            }

            let existing_parents: Vec<String> = normalized_parents
                .into_iter()
                .filter(|p| p == "genesis" || state.dag.get_node(p).is_some())
                .collect();

            let node = rinku_core::types::DagNode {
                hash: tx.hash.clone(),
                tx: tx.clone(),
                parents: existing_parents,
                children: Vec::new(),
                weight: 1.0,
                finalized: false,
                checkpoint_height: None,
                received_at_ms: Some(now_ms),
                partition_epoch: None,
                rolled_back: false,
                convergence_certificate: None,
            };

            match state.dag.add_node(node) {
                Ok(_) => { added += 1; }
                Err(e) => {
                    tracing::debug!("Batch force-add: failed to add tx {}: {}", &tx.hash[..16.min(tx.hash.len())], e);
                }
            }
        }

        Ok(added)
    }

    pub async fn add_transactions_batch(&self, txs: Vec<SignedTransaction>) -> Vec<Result<()>> {
        let mut validation_results: Vec<Option<anyhow::Error>> = Vec::with_capacity(txs.len());
        
        let min_stake = {
            let rewards = self.rewards.read().await;
            rewards.get_config().min_stake_amount
        };
        
        {
            let state = self.inner.read().await;
            for tx in txs.iter() {
                let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
                let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
                let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
                let gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);
                
                if is_stake_tx && tx.tx.amount < min_stake {
                    validation_results.push(Some(anyhow::anyhow!(
                        "Minimum stake amount is {} RKU, you tried to stake {}",
                        min_stake, tx.tx.amount
                    )));
                    continue;
                }
                
                let required_balance = if is_stake_tx {
                    tx.tx.amount + gas_fee
                } else if is_unstake_tx || is_claim_tx {
                    gas_fee
                } else {
                    tx.tx.amount + gas_fee
                };
                
                if tx.tx.from != "genesis" {
                    let effective_balance = Self::get_effective_balance(&state, &tx.tx.from);
                    if state.accounts.get(&tx.tx.from).is_none() {
                        validation_results.push(Some(anyhow::anyhow!("Account does not exist")));
                        continue;
                    }
                    if effective_balance < required_balance {
                        validation_results.push(Some(anyhow::anyhow!(
                            "Insufficient balance: have {:.6}, need {:.6}",
                            effective_balance, required_balance
                        )));
                        continue;
                    }
                }
                validation_results.push(None);
            }
        }
        
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let now_secs = now_ms / 1000;

        let client_parents_list: Vec<Vec<String>> = txs
            .iter()
            .map(|tx| {
                tx.tx
                    .parents
                    .iter()
                    .map(|p| {
                        if p.starts_with("rinku://tx/h/") {
                            p.strip_prefix("rinku://tx/h/").unwrap_or(p).to_string()
                        } else if p.starts_with("rinku://tx/") {
                            p.strip_prefix("rinku://tx/").unwrap_or(p).to_string()
                        } else {
                            p.clone()
                        }
                    })
                    .collect()
            })
            .collect();

        let account_weights: std::collections::HashMap<String, f64> = {
            let state = self.inner.read().await;
            txs.iter()
                .map(|tx| {
                    let weight = if let Some(account) = state.accounts.get(&tx.tx.from) {
                        calculate_account_weight(account, now_secs)
                    } else {
                        1.0
                    };
                    (tx.tx.from.clone(), weight)
                })
                .collect()
        };

        let mut state = self.inner.write().await;
        let mut results = Vec::with_capacity(txs.len());

        for (idx, tx) in txs.iter().enumerate() {
            if let Some(err) = validation_results.get(idx).and_then(|r| r.as_ref()) {
                results.push(Err(anyhow::anyhow!("{}", err)));
                continue;
            }
            
            let client_parents = &client_parents_list[idx];
            let tx_weight = account_weights.get(&tx.tx.from).copied().unwrap_or(1.0);
            
            let valid_parents: Vec<String> = client_parents
                .iter()
                .filter(|p| !p.is_empty() && state.dag.get_node(p).is_some())
                .cloned()
                .collect();
            
            let normalized_parents = if valid_parents.is_empty() {
                let current_tips = state.dag.tips();
                current_tips.into_iter().take(2).collect()
            } else {
                valid_parents
            };
            
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            let node = rinku_core::types::DagNode {
                hash: tx.hash.clone(),
                tx: tx.clone(),
                parents: normalized_parents.clone(),
                children: Vec::new(),
                weight: tx_weight,
                finalized: false,
                checkpoint_height: None,
                received_at_ms: Some(now_ms),
                partition_epoch: None,
                rolled_back: false,
                convergence_certificate: None,
            };
            
            let result = state
                .dag
                .add_node(node)
                .map_err(|e| anyhow::anyhow!("{}", e));
            results.push(result);
        }

        drop(state);

        results
    }

    pub async fn get_transaction(&self, hash: &str) -> Option<SignedTransaction> {
        let state = self.inner.read().await;
        state.dag.get_node(hash).map(|n| n.tx.clone())
    }
    
    pub async fn has_transaction(&self, hash: &str) -> bool {
        let state = self.inner.read().await;
        state.dag.get_node(hash).is_some()
    }
    
    pub async fn get_recent_transactions(&self, limit: usize) -> Vec<SignedTransaction> {
        let state = self.inner.read().await;
        state.dag
            .get_all_nodes()
            .into_iter()
            .take(limit)
            .map(|n| n.tx.clone())
            .collect()
    }
    
    pub async fn get_transactions_by_address(&self, address: &str, limit: usize) -> Vec<(SignedTransaction, bool)> {
        let state = self.inner.read().await;
        let mut txs: Vec<_> = state.dag
            .get_all_nodes()
            .into_iter()
            .filter(|n| n.tx.tx.from == address || n.tx.tx.to == address)
            .map(|n| {
                let finalized = n.finalized;
                (n.tx.clone(), finalized)
            })
            .collect();
        
        txs.sort_by(|a, b| b.0.tx.timestamp.cmp(&a.0.tx.timestamp));
        txs.truncate(limit);
        txs
    }

    pub async fn get_transaction_with_weight(&self, hash: &str) -> Option<(SignedTransaction, f64)> {
        let state = self.inner.read().await;
        let result = state.dag.get_node(hash).map(|n| (n.tx.clone(), n.weight));
        if result.is_none() {
            let all_hashes: Vec<_> = state.dag.get_all_nodes().iter().take(5).map(|n| &n.hash).collect();
            tracing::debug!("get_transaction_with_weight: hash '{}' not found. DAG has {} nodes. Sample hashes: {:?}", 
                hash, state.dag.node_count(), all_hashes);
        }
        result
    }

    pub async fn is_finalized(&self, hash: &str) -> bool {
        let state = self.inner.read().await;
        state
            .dag
            .get_node(hash)
            .map(|n| n.finalized)
            .unwrap_or(false)
    }
}
