use super::*;

impl NodeState {
    pub async fn add_transaction(&self, tx: SignedTransaction) -> Result<TransactionResult> {
        let is_relay_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Relay));
        if is_relay_tx {
            let relay_data = tx.tx.data.as_ref().and_then(|d| {
                serde_json::from_str::<serde_json::Value>(d).ok()
            });
            let inner_kind_str = relay_data.as_ref()
                .and_then(|v| v.get("innerKind").and_then(|k| k.as_str()))
                .unwrap_or("transfer");
            let inner_kind = match inner_kind_str {
                "stake" | "Stake" => rinku_core::types::TransactionKind::Stake,
                "unstake" | "Unstake" => rinku_core::types::TransactionKind::Unstake,
                "claimRewards" | "ClaimRewards" => rinku_core::types::TransactionKind::ClaimRewards,
                "contract" | "Contract" => rinku_core::types::TransactionKind::Contract,
                _ => rinku_core::types::TransactionKind::Transfer,
            };
            let relayer_addr = tx.tx.from.clone();
            return self.add_relay_transaction(tx, &relayer_addr, inner_kind).await;
        }

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
            let gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);
            
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
                
                let effective_balance = Self::get_effective_balance(&state, &tx.tx.from);
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
                
                let effective_nonce = Self::get_effective_nonce(&state, &tx.tx.from);
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
            
            let effective_balance = Self::get_effective_balance(&state, &tx.tx.from);
            if effective_balance < required_balance {
                return Err(anyhow::anyhow!(
                    "Insufficient balance: have {:.6}, need {:.6}",
                    effective_balance, required_balance
                ));
            }
            
            let effective_nonce = Self::get_effective_nonce(&state, &tx.tx.from);
            if tx.tx.nonce != effective_nonce {
                return Err(anyhow::anyhow!(
                    "Invalid nonce: expected {}, got {}",
                    effective_nonce, tx.tx.nonce
                ));
            }
        }

        state.dag.add_node(node)?;
        
        state.txs_this_period += 1;

        const PERIOD_MS: u64 = 15000;
        const TARGET_TPS: f64 = 10.0;
        const MAX_CHANGE_PERCENT: f64 = 0.125;
        const ELASTICITY: f64 = 2.0;

        if now_ms - state.period_start_ms >= PERIOD_MS {
            let target_txs = TARGET_TPS * (PERIOD_MS as f64 / 1000.0);
            let utilization = state.txs_this_period as f64 / target_txs;
            let change_ratio = ((utilization - 1.0) / (ELASTICITY - 1.0)).clamp(-1.0, 1.0);
            let change_factor = 1.0 + change_ratio * MAX_CHANGE_PERCENT;
            state.current_gas_price = (state.current_gas_price * change_factor).clamp(
                state.config.gas.min_gas_price,
                state.config.gas.max_gas_price,
            );
            state.txs_this_period = 0;
            state.period_start_ms = now_ms;
        }
        
        drop(state);

        Ok(TransactionResult::Accepted)
    }
    
    pub async fn add_relay_transaction(
        &self,
        tx: SignedTransaction,
        relayer_address: &str,
        inner_kind: rinku_core::types::TransactionKind,
    ) -> Result<TransactionResult> {
        let is_stake_tx = matches!(inner_kind, rinku_core::types::TransactionKind::Stake);
        let is_unstake_tx = matches!(inner_kind, rinku_core::types::TransactionKind::Unstake);
        let is_claim_tx = matches!(inner_kind, rinku_core::types::TransactionKind::ClaimRewards);
        let is_contract_tx = matches!(inner_kind, rinku_core::types::TransactionKind::Contract);

        let relay_data_parsed = tx.tx.data.as_ref().and_then(|d| {
            serde_json::from_str::<serde_json::Value>(d).ok()
        });
        let intent_from_addr = relay_data_parsed.as_ref()
            .and_then(|v| v.get("intentFrom")?.as_str().map(|s| s.to_string()))
            .ok_or_else(|| anyhow::anyhow!("Relay transaction missing intentFrom in data"))?;
        let relay_fee = relay_data_parsed.as_ref()
            .and_then(|v| v.get("relayFee")?.as_f64())
            .unwrap_or(0.0);

        {
            let state = self.inner.read().await;
            let gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);

            if let Some(ref memo) = tx.tx.memo {
                if memo.len() > 1024 {
                    return Err(anyhow::anyhow!("Memo too large: {} bytes (max 1024)", memo.len()));
                }
            }
            if let Some(ref refs) = tx.tx.references {
                if refs.len() > 4 {
                    return Err(anyhow::anyhow!("Too many references: {} (max 4)", refs.len()));
                }
            }

            if is_stake_tx {
                let rewards = self.rewards.read().await;
                let min_stake = rewards.get_config().min_stake_amount;
                drop(rewards);
                if tx.tx.amount < min_stake {
                    return Err(anyhow::anyhow!(
                        "Minimum stake amount is {} RKU, you tried to stake {}",
                        min_stake, tx.tx.amount
                    ));
                }
            }

            let required_balance_intent = if is_stake_tx {
                tx.tx.amount + relay_fee
            } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                relay_fee
            } else {
                tx.tx.amount + relay_fee
            };

            if intent_from_addr != "genesis" {
                if !state.accounts.contains_key(&intent_from_addr) {
                    return Err(anyhow::anyhow!("Intent signer account does not exist"));
                }

                let effective_balance = Self::get_effective_balance(&state, &intent_from_addr);
                if effective_balance < required_balance_intent {
                    return Err(anyhow::anyhow!(
                        "Insufficient balance for intent signer: have {:.6}, need {:.6}",
                        effective_balance, required_balance_intent
                    ));
                }

                let effective_nonce = Self::get_effective_nonce(&state, &intent_from_addr);
                let confirmed_nonce = state.accounts.get(&intent_from_addr).map(|a| a.nonce).unwrap_or(0);
                if tx.tx.nonce < confirmed_nonce {
                    return Err(anyhow::anyhow!(
                        "Stale nonce: confirmed nonce is {}, got {}",
                        confirmed_nonce, tx.tx.nonce
                    ));
                }
                if tx.tx.nonce != effective_nonce {
                    return Err(anyhow::anyhow!(
                        "Invalid nonce: expected {}, got {}",
                        effective_nonce, tx.tx.nonce
                    ));
                }
            }

            if !state.accounts.contains_key(relayer_address) {
                return Err(anyhow::anyhow!("Relayer account does not exist"));
            }
            let relayer_balance = state.accounts.get(relayer_address)
                .map(|a| a.balance)
                .unwrap_or(0.0);
            if relayer_balance < gas_fee {
                return Err(anyhow::anyhow!(
                    "Relayer insufficient gas balance: have {:.6}, need {:.6}",
                    relayer_balance, gas_fee
                ));
            }
        }

        let client_parents: Vec<String> = tx.tx.parents.iter()
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
        let now_ms = now_secs * 1000;

        let (tx_weight, normalized_parents) = {
            let state = self.inner.read().await;
            let weight = if let Some(account) = state.accounts.get(&tx.tx.from) {
                calculate_account_weight(account, now_secs)
            } else {
                1.0
            };
            let valid_parents: Vec<String> = client_parents.iter()
                .filter(|p| !p.is_empty() && state.dag.get_node(p).is_some())
                .cloned()
                .collect();
            let final_parents = if valid_parents.is_empty() {
                state.dag.tips().into_iter().take(2).collect()
            } else {
                valid_parents
            };
            (weight, final_parents)
        };

        let node = rinku_core::types::DagNode {
            hash: tx.hash.clone(),
            tx: tx.clone(),
            parents: normalized_parents.clone(),
            children: Vec::new(),
            weight: tx_weight,
            finalized: false,
            checkpoint_height: None,
            received_at_ms: Some(now_ms),
        };

        let mut state = self.inner.write().await;

        let gas_fee = tx.tx.gas_price.unwrap_or(state.current_gas_price);
        let relayer_balance = state.accounts.get(relayer_address)
            .map(|a| a.balance)
            .unwrap_or(0.0);
        if relayer_balance < gas_fee {
            return Err(anyhow::anyhow!(
                "Relayer insufficient gas balance: have {:.6}, need {:.6}",
                relayer_balance, gas_fee
            ));
        }

        let effective_nonce = Self::get_effective_nonce(&state, &intent_from_addr);
        if tx.tx.nonce != effective_nonce {
            return Err(anyhow::anyhow!(
                "Invalid nonce: expected {}, got {}",
                effective_nonce, tx.tx.nonce
            ));
        }

        if state.dag.get_node(&tx.hash).is_some() {
            return Err(anyhow::anyhow!("Duplicate transaction hash"));
        }

        state.dag.add_node(node)?;
        state.txs_this_period += 1;

        tracing::info!(
            "RELAY TX accepted: hash={}, intentFrom={}, relayer={}, amount={:.4}",
            &tx.hash[..16.min(tx.hash.len())],
            &intent_from_addr[..16.min(intent_from_addr.len())],
            &relayer_address[..16.min(relayer_address.len())],
            tx.tx.amount
        );

        drop(state);
        Ok(TransactionResult::Accepted)
    }

    pub async fn execute_fast_path_transaction(&self, tx: &SignedTransaction) -> bool {
        let gas_fee = {
            let state = self.inner.read().await;
            tx.tx.gas_price.unwrap_or(state.current_gas_price)
        };

        let is_relay_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Relay));
        let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
        let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
        let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
        let is_contract_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Contract));

        {
            let state = self.inner.read().await;

            if is_relay_tx {
                let relay_parsed = tx.tx.data.as_ref().and_then(|d| {
                    serde_json::from_str::<serde_json::Value>(d).ok()
                });
                let intent_from = relay_parsed.as_ref()
                    .and_then(|v| v.get("intentFrom")?.as_str().map(|s| s.to_string()));
                let fp_relay_fee = relay_parsed.as_ref()
                    .and_then(|v| v.get("relayFee")?.as_f64())
                    .unwrap_or(0.0);

                if let Some(ref intent_sender) = intent_from {
                    if let Some(from_account) = state.accounts.get(intent_sender) {
                        let required_balance = tx.tx.amount + fp_relay_fee;
                        if from_account.balance < required_balance {
                            tracing::warn!(
                                "FastPath relay rejected: intent signer {} insufficient balance ({:.8} < {:.8})",
                                &intent_sender[..16.min(intent_sender.len())],
                                from_account.balance,
                                required_balance
                            );
                            return false;
                        }
                        if tx.tx.nonce != from_account.nonce {
                            tracing::warn!(
                                "FastPath relay rejected: intent signer {} nonce mismatch (tx={} vs account={})",
                                &intent_sender[..16.min(intent_sender.len())],
                                tx.tx.nonce,
                                from_account.nonce
                            );
                            return false;
                        }
                    } else {
                        tracing::warn!("FastPath relay rejected: intent signer account {} not found", &intent_sender[..16.min(intent_sender.len())]);
                        return false;
                    }
                } else {
                    tracing::warn!("FastPath relay rejected: missing intentFrom in relay data");
                    return false;
                }

                if let Some(relayer_account) = state.accounts.get(&tx.tx.from) {
                    if relayer_account.balance < gas_fee {
                        tracing::warn!(
                            "FastPath relay rejected: relayer {} insufficient gas ({:.8} < {:.8})",
                            &tx.tx.from[..16.min(tx.tx.from.len())],
                            relayer_account.balance,
                            gas_fee
                        );
                        return false;
                    }
                } else {
                    tracing::warn!("FastPath relay rejected: relayer account {} not found", &tx.tx.from[..16.min(tx.tx.from.len())]);
                    return false;
                }
            } else {
                if let Some(from_account) = state.accounts.get(&tx.tx.from) {
                    let required = if is_stake_tx {
                        tx.tx.amount + gas_fee
                    } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                        gas_fee
                    } else {
                        tx.tx.amount + gas_fee
                    };
                    if from_account.balance < required {
                        tracing::warn!(
                            "FastPath execution rejected: {} insufficient balance ({:.8} < {:.8})",
                            &tx.tx.from[..16.min(tx.tx.from.len())],
                            from_account.balance,
                            required
                        );
                        return false;
                    }
                    if tx.tx.nonce != from_account.nonce {
                        tracing::warn!(
                            "FastPath execution rejected: {} nonce mismatch (tx={} vs account={})",
                            &tx.tx.from[..16.min(tx.tx.from.len())],
                            tx.tx.nonce,
                            from_account.nonce
                        );
                        return false;
                    }
                } else {
                    tracing::warn!(
                        "FastPath execution rejected: account {} not found",
                        &tx.tx.from[..16.min(tx.tx.from.len())]
                    );
                    return false;
                }
            }
        }

        self.execute_finalized_transaction_core(tx).await;

        tracing::info!(
            "FastPath EXECUTED tx {} ({} -> {}, amount={:.8}, gas={:.8})",
            &tx.hash[..16.min(tx.hash.len())],
            &tx.tx.from[..16.min(tx.tx.from.len())],
            &tx.tx.to[..16.min(tx.tx.to.len())],
            tx.tx.amount,
            gas_fee
        );

        true
    }

    pub async fn execute_finalized_transaction(&self, tx: &SignedTransaction) {
        self.execute_finalized_transaction_core(tx).await;
        self.execute_finalized_transaction_rewards(tx).await;
    }
    
    pub async fn execute_finalized_transaction_core(&self, tx: &SignedTransaction) {
        let gas_fee = {
            let state = self.inner.read().await;
            tx.tx.gas_price.unwrap_or(state.current_gas_price)
        };
        
        let is_relay_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Relay));
        let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Stake));
        let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
        let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
        let is_contract_tx = matches!(tx.tx.kind, Some(rinku_core::types::TransactionKind::Contract));

        let relay_info = if is_relay_tx {
            tx.tx.data.as_ref().and_then(|d| {
                serde_json::from_str::<serde_json::Value>(d).ok().and_then(|v| {
                    let intent_from = v.get("intentFrom")?.as_str()?.to_string();
                    let inner_kind_str = v.get("innerKind").and_then(|k| k.as_str()).unwrap_or("transfer");
                    let inner_kind = match inner_kind_str {
                        "stake" | "Stake" => rinku_core::types::TransactionKind::Stake,
                        "unstake" | "Unstake" => rinku_core::types::TransactionKind::Unstake,
                        "claimRewards" | "ClaimRewards" => rinku_core::types::TransactionKind::ClaimRewards,
                        "contract" | "Contract" => rinku_core::types::TransactionKind::Contract,
                        _ => rinku_core::types::TransactionKind::Transfer,
                    };
                    let relay_fee = v.get("relayFee").and_then(|f| f.as_f64()).unwrap_or(0.0);
                    let intent_hash = v.get("intentHash").and_then(|h| h.as_str()).unwrap_or("").to_string();
                    Some((intent_from, inner_kind, relay_fee, intent_hash))
                })
            })
        } else {
            None
        };
        
        {
            let mut state = self.inner.write().await;

            if is_relay_tx {
                if let Some((ref intent_from_addr, ref inner_kind, relay_fee, _)) = relay_info {
                    let is_inner_transfer = matches!(inner_kind, rinku_core::types::TransactionKind::Transfer);

                    if let Some(from_account) = state.accounts.get_mut(intent_from_addr) {
                        if is_inner_transfer {
                            from_account.balance -= tx.tx.amount + relay_fee;
                        } else if matches!(inner_kind, rinku_core::types::TransactionKind::Stake) {
                            from_account.balance -= tx.tx.amount + relay_fee;
                        } else {
                            from_account.balance -= relay_fee;
                        }
                        from_account.nonce = tx.tx.nonce + 1;
                    }

                    if let Some(relayer_account) = state.accounts.get_mut(&tx.tx.from) {
                        relayer_account.balance -= gas_fee;
                        relayer_account.balance += relay_fee;
                    }

                    if is_inner_transfer {
                        let to_account = state
                            .accounts
                            .entry(tx.tx.to.clone())
                            .or_insert_with(|| Account::new(tx.tx.to.clone(), tx.tx.timestamp));
                        to_account.balance += tx.tx.amount;
                    }
                }
            } else {
                if let Some(from_account) = state.accounts.get_mut(&tx.tx.from) {
                    if is_stake_tx {
                        from_account.balance -= tx.tx.amount + gas_fee;
                    } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                        from_account.balance -= gas_fee;
                    } else {
                        from_account.balance -= tx.tx.amount + gas_fee;
                    }
                    from_account.nonce = tx.tx.nonce + 1;
                }
            
                if !is_stake_tx && !is_unstake_tx && !is_claim_tx && !is_contract_tx {
                    let to_account = state
                        .accounts
                        .entry(tx.tx.to.clone())
                        .or_insert_with(|| Account::new(tx.tx.to.clone(), tx.tx.timestamp));
                    to_account.balance += tx.tx.amount;
                }
            }
            
            state.total_burned += gas_fee * 0.5;
            state.total_to_validators += gas_fee * 0.5;
        }
        
        if let Some(ref kind) = tx.tx.kind {
            use rinku_core::types::TransactionKind;
            let from_addr = &tx.tx.from;
            let stake_amount = tx.tx.amount;
            
            match kind {
                TransactionKind::Stake => {
                    let stake_update: Option<(f64, u64)> = {
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
                    let unstake_result: Option<f64> = {
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
                            account.staked = 0.0;
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
                    let claimed: f64 = {
                        let mut rewards = self.rewards.write().await;
                        rewards.claim_rewards(from_addr)
                    };
                    tracing::info!(
                        "[EXECUTION] Claim for {}: claimed_amount={:.8}",
                        &from_addr[..16.min(from_addr.len())],
                        claimed
                    );
                    if claimed > 0.0 {
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
                TransactionKind::Relay => {
                    if let Some((ref intent_from_addr, ref inner_kind, relay_fee, ref intent_hash)) = relay_info {
                        if relay_fee > 0.0 {
                            let mut rewards = self.rewards.write().await;
                            rewards.process_relay_reward(&tx.tx.from, relay_fee, &tx.hash, intent_hash);
                        }
                        match inner_kind {
                            TransactionKind::Stake => {
                                let stake_update: Option<(f64, u64)> = {
                                    let mut rewards = self.rewards.write().await;
                                    if let Err(e) = rewards.stake(intent_from_addr, stake_amount) {
                                        tracing::warn!("Failed to process relayed stake tx: {}", e);
                                        None
                                    } else {
                                        tracing::debug!("Finalized relayed stake: {} staked {} RKU", &intent_from_addr[..16.min(intent_from_addr.len())], stake_amount);
                                        rewards.get_stake(intent_from_addr).map(|p| (p.amount, p.staked_at))
                                    }
                                };
                                if let Some((amount, staked_at)) = stake_update {
                                    self.update_account_staked(intent_from_addr, amount, Some(staked_at / 1000)).await;
                                }
                            }
                            TransactionKind::Unstake => {
                                let unstake_result: Option<f64> = {
                                    let mut rewards = self.rewards.write().await;
                                    match rewards.unstake(intent_from_addr) {
                                        Ok(amount) => Some(amount),
                                        Err(e) => {
                                            tracing::warn!("Failed to process relayed unstake tx: {}", e);
                                            None
                                        }
                                    }
                                };
                                if let Some(unstaked_amount) = unstake_result {
                                    let mut state = self.inner.write().await;
                                    if let Some(account) = state.accounts.get_mut(intent_from_addr.as_str()) {
                                        account.balance += unstaked_amount;
                                        account.staked = 0.0;
                                    }
                                }
                            }
                            TransactionKind::ClaimRewards => {
                                let claimed: f64 = {
                                    let mut rewards = self.rewards.write().await;
                                    rewards.claim_rewards(intent_from_addr)
                                };
                                if claimed > 0.0 {
                                    let mut state = self.inner.write().await;
                                    if let Some(account) = state.accounts.get_mut(intent_from_addr.as_str()) {
                                        account.balance += claimed;
                                    }
                                }
                            }
                            _ => {}
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

    async fn charge_contract_execution_fee(&self, from: &str, gas_used: u64, gas_price: f64) {
        use crate::wasm_runtime::BASE_TX_GAS;
        let additional_gas = gas_used.saturating_sub(BASE_TX_GAS);
        let execution_fee = (additional_gas as f64 / BASE_TX_GAS as f64) * gas_price;
        if execution_fee > 0.0 {
            let mut state = self.inner.write().await;
            if let Some(account) = state.accounts.get_mut(from) {
                account.balance -= execution_fee;
                if account.balance < 0.0 {
                    account.balance = 0.0;
                }
            }
            state.total_burned += execution_fee * 0.5;
            state.total_to_validators += execution_fee * 0.5;
            tracing::info!(
                "Contract execution fee: {} total gas ({} additional) = {:.6} RKU from {}",
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
        
        if tx_amount > 0.0 || gas_fee > 0.0 {
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

        let mut state = self.inner.write().await;
        
        if state.dag.get_node(&tx.hash).is_some() {
            return Ok(());
        }
        
        for p in &normalized_parents {
            if p != "genesis" && state.dag.get_node(p).is_none() {
                anyhow::bail!("Parent {} not found for tx {}", p, &tx.hash);
            }
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let node = rinku_core::types::DagNode {
            hash: tx.hash.clone(),
            tx: tx.clone(),
            parents: normalized_parents,
            children: Vec::new(),
            weight: 1.0,
            finalized: false,
            checkpoint_height: None,
            received_at_ms: Some(now_ms),
        };

        state.dag.add_node(node)?;
        Ok(())
    }
    
    pub async fn add_transaction_from_sync(&self, tx: SignedTransaction) -> Result<()> {
        self.add_transaction_dag_only(tx).await
    }
    
    pub async fn set_tx_checkpoint_height(&self, hash: &str, height: u64) {
        let mut state = self.inner.write().await;
        if let Some(node) = state.dag.get_node_mut(hash) {
            node.checkpoint_height = Some(height);
            node.finalized = true;
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
            };
            
            if !state.checkpoints.iter().any(|c| c.height == checkpoint.height) {
                state.checkpoints.push(checkpoint);
            }
        }
        
        state.checkpoints.sort_by_key(|c| c.height);
        
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
                };
                let _ = state.dag.add_node(node);
            }
        }
        
        info!("P2P snapshot applied successfully");
        Ok(())
    }

    pub async fn force_add_transaction_for_vote(&self, tx: SignedTransaction) -> Result<()> {
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
        };

        state.dag.add_node(node)?;
        Ok(())
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
            };
            
            let result = state
                .dag
                .add_node(node)
                .map_err(|e| anyhow::anyhow!("{}", e));
            if result.is_ok() {
                state.txs_this_period += 1;
            }
            results.push(result);
        }

        const PERIOD_MS: u64 = 15000;
        const TARGET_TPS: f64 = 1000.0;
        const MAX_CHANGE_PERCENT: f64 = 0.125;
        const ELASTICITY: f64 = 2.0;

        if now_ms - state.period_start_ms >= PERIOD_MS {
            let target_txs = TARGET_TPS * (PERIOD_MS as f64 / 1000.0);
            let utilization = state.txs_this_period as f64 / target_txs;
            let change_ratio = ((utilization - 1.0) / (ELASTICITY - 1.0)).clamp(-1.0, 1.0);
            let change_factor = 1.0 + change_ratio * MAX_CHANGE_PERCENT;
            state.current_gas_price = (state.current_gas_price * change_factor).clamp(
                state.config.gas.min_gas_price,
                state.config.gas.max_gas_price,
            );
            state.txs_this_period = 0;
            state.period_start_ms = now_ms;
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

    pub async fn count_relay_txs_by_sender(&self, address: &str) -> u64 {
        let state = self.inner.read().await;
        state.dag
            .get_all_nodes()
            .into_iter()
            .filter(|n| n.tx.tx.from == address && matches!(n.tx.tx.kind, Some(rinku_core::types::TransactionKind::Relay)))
            .count() as u64
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
