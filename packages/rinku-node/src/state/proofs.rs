use super::*;

impl NodeState {
    /// Get pending (unfinalized) transaction stats for a sender using per-sender DAG index
    /// Returns (pending_outgoing_amount, pending_gas, pending_tx_count)
    /// O(K_sender) instead of O(N_total_unfinalized) thanks to sender_unfinalized index
    pub(crate) fn get_pending_stats_for_sender(
        state: &StateInner,
        sender: &str,
    ) -> (u64, u64, u64) {
        let mut pending_amount = 0u64;
        let mut pending_gas = 0u64;
        let mut pending_count = 0u64;

        let confirmed_nonce = state.accounts.get(sender).map(|a| a.nonce).unwrap_or(0);

        for node in state.dag.get_unfinalized_for_sender(sender) {
            if node.tx.tx.nonce < confirmed_nonce {
                continue;
            }
            let gas = node.tx.tx.gas_price.unwrap_or(state.current_gas_price);
            pending_gas += gas;

            let is_unstake = matches!(
                node.tx.tx.kind,
                Some(rinku_core::types::TransactionKind::Unstake)
            );
            let is_claim = matches!(
                node.tx.tx.kind,
                Some(rinku_core::types::TransactionKind::ClaimRewards)
            );
            if !is_unstake && !is_claim {
                pending_amount += node.tx.tx.amount;
            }
            pending_count += 1;
        }

        (pending_amount, pending_gas, pending_count)
    }

    /// Get effective balance AND nonce for a sender.
    /// Balance accounts for ALL pending txs (funds are reserved).
    /// Nonce only counts *contiguous* pending nonces to avoid gap deadlocks.
    pub(crate) fn get_effective_balance_and_nonce(state: &StateInner, sender: &str) -> (u64, u64) {
        let (confirmed_balance, confirmed_nonce) = state
            .accounts
            .get(sender)
            .map(|a| (a.balance, a.nonce))
            .unwrap_or((0, 0));
        let (pending_amount, pending_gas, _) = Self::get_pending_stats_for_sender(state, sender);
        let effective_balance = confirmed_balance
            .saturating_sub(pending_amount)
            .saturating_sub(pending_gas);
        let effective_nonce = Self::get_effective_nonce(state, sender);
        (effective_balance, effective_nonce)
    }

    /// Get the expected nonce for a sender, accounting for pending (unfinalized) transactions.
    /// Only counts *contiguous* pending nonces starting from confirmed_nonce to avoid
    /// nonce-gap deadlocks: if nonce N is lost but N+1..N+K are in the DAG, we return N
    /// (not N+K) so the sender re-submits the missing nonce and unblocks the chain.
    pub(crate) fn get_effective_nonce(state: &StateInner, sender: &str) -> u64 {
        let confirmed_nonce = state.accounts.get(sender).map(|a| a.nonce).unwrap_or(0);

        let mut pending_nonces: Vec<u64> = state
            .dag
            .get_unfinalized_for_sender(sender)
            .iter()
            .map(|node| node.tx.tx.nonce)
            .filter(|&n| n >= confirmed_nonce)
            .collect();
        pending_nonces.sort_unstable();
        pending_nonces.dedup();

        let mut contiguous = 0u64;
        for &nonce in &pending_nonces {
            if nonce == confirmed_nonce + contiguous {
                contiguous += 1;
            } else {
                break;
            }
        }

        confirmed_nonce + contiguous
    }

    /// Get effective balance for a sender, accounting for pending (unfinalized) transactions
    /// effective_balance = confirmed_balance - pending_outgoing - pending_gas
    pub(crate) fn get_effective_balance(state: &StateInner, sender: &str) -> u64 {
        let confirmed_balance = state.accounts.get(sender).map(|a| a.balance).unwrap_or(0);
        let (pending_amount, pending_gas, _) = Self::get_pending_stats_for_sender(state, sender);
        confirmed_balance
            .saturating_sub(pending_amount)
            .saturating_sub(pending_gas)
    }

    /// Compute state root from all account states
    /// This creates a deterministic merkle root from sorted account data
    /// Uses canonical format matching sync_verification: "account:address:balance:nonce:stake"
    /// Internal nodes: "node:left_hash:right_hash"
    pub async fn compute_state_root(&self) -> String {
        let root_start = std::time::Instant::now();
        let state = self.inner.read().await;
        let num_accounts = state.accounts.len();
        let root = state.state_trie.root_hex();
        let root_ms = root_start.elapsed().as_millis();
        if root_ms > 10 {
            tracing::info!(
                "STATE-ROOT (SMT): computed in {}ms ({} accounts)",
                root_ms,
                num_accounts
            );
        }
        root
    }

    /// Compute state root with pending transactions applied (without modifying actual state)
    /// This is used by checkpoint creation to get the correct post-execution state root
    /// before actually executing the transactions
    pub async fn compute_state_root_with_pending_txs(
        &self,
        pending_txs: &[rinku_core::SignedTransaction],
    ) -> String {
        self.compute_state_root_and_proofs(pending_txs, &[], None, "")
            .await
            .state_root
    }

    pub async fn compute_state_root_and_proofs_at_height(
        &self,
        pending_txs: &[rinku_core::SignedTransaction],
        affected_addresses: &[String],
        height: u64,
    ) -> StateRootWithProofs {
        self.compute_state_root_and_proofs(pending_txs, affected_addresses, Some(height), "")
            .await
    }

    pub async fn compute_state_root_and_proofs(
        &self,
        pending_txs: &[rinku_core::SignedTransaction],
        affected_addresses: &[String],
        checkpoint_height: Option<u64>,
        tx_hash: &str,
    ) -> StateRootWithProofs {
        use std::collections::HashMap;
        let root_start = std::time::Instant::now();

        if pending_txs.is_empty() {
            let state = self.inner.read().await;
            let state_root = state.state_trie.root_hex();
            let num_accounts = state.accounts.len();
            drop(state);

            let root_ms = root_start.elapsed().as_millis();
            if root_ms > 5 {
                tracing::info!(
                    "STATE-ROOT-PROOFS (SMT): computed in {}ms ({} accounts, 0 changed, 0 txs simulated, 0 proofs, h={:?}) [FAST-PATH-SKIP]",
                    root_ms, num_accounts, checkpoint_height
                );
            }
            return StateRootWithProofs {
                state_root,
                proofs: HashMap::new(),
                executed_tx_hashes: std::collections::HashSet::new(),
            };
        }

        let state = self.inner.read().await;

        let current_gas_price = state.current_gas_price;

        let mut simulated_accounts: HashMap<String, (u64, u64, u64)> = state
            .accounts
            .iter()
            .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
            .collect();

        let mut sim_changed_addrs: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let original_root = state.state_trie.root_hex();
        let forked_trie = state.state_trie.fork();

        struct TxParentInfo {
            tx_hash: String,
            reward_base: u64,
            first_parent_hash: Option<String>,
            witness_parents: Vec<(String, String)>,
        }
        let tx_parent_infos: Vec<TxParentInfo> = pending_txs
            .iter()
            .filter(|tx| !matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Consolidation)))
            .filter_map(|tx| {
                let gas_fee = tx.tx.gas_price.unwrap_or(current_gas_price);
                let reward_base = tx.tx.amount + gas_fee;
                if reward_base == 0 {
                    return None;
                }
                fn normalize_parent(p: &str) -> &str {
                    if p.starts_with("rinku://tx/h/") {
                        p.strip_prefix("rinku://tx/h/").unwrap_or(p)
                    } else if p.starts_with("rinku://tx/") {
                        p.strip_prefix("rinku://tx/").unwrap_or(p)
                    } else {
                        p
                    }
                }
                let first_parent_hash = tx
                    .tx
                    .parents
                    .first()
                    .map(|p| normalize_parent(p).to_string());
                let witness_parents: Vec<(String, String)> = tx
                    .tx
                    .parents
                    .iter()
                    .filter_map(|parent_ref| {
                        let ph = normalize_parent(parent_ref);
                        state.dag.get_node(ph).and_then(|node| {
                            let creator = node.tx.tx.from.clone();
                            if creator != tx.tx.from {
                                Some((format!("rinku://tx/h/{}", ph), creator))
                            } else {
                                None
                            }
                        })
                    })
                    .collect();
                Some(TxParentInfo {
                    tx_hash: tx.hash.clone(),
                    reward_base,
                    first_parent_hash,
                    witness_parents,
                })
            })
            .collect();

        let node_validator_address = state.node_validator_address.clone();

        let fast_path_nonces: HashMap<String, u64> = {
            let mut nonces: HashMap<String, u64> = HashMap::new();
            for entry in state.fast_path_finalized_txs.values() {
                let e = nonces.entry(entry.from.clone()).or_insert(0);
                if entry.nonce + 1 > *e {
                    *e = entry.nonce + 1;
                }
            }
            nonces
        };

        drop(state);

        // Get pending rewards and stake amounts snapshot for claim/unstake simulation
        // CRITICAL: Must use rewards service as source of truth to match execute_finalized_transaction
        let rewards = self.rewards.read().await;
        let pending_rewards_snapshot: HashMap<String, u64> = pending_txs
            .iter()
            .filter(|tx| matches!(tx.tx.kind, Some(rinku_core::TransactionKind::ClaimRewards)))
            .map(|tx| (tx.tx.from.clone(), rewards.get_pending_rewards(&tx.tx.from)))
            .collect();
        let stake_amounts_snapshot: HashMap<String, u64> = pending_txs
            .iter()
            .filter(|tx| matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Unstake)))
            .filter_map(|tx| {
                rewards
                    .get_stake(&tx.tx.from)
                    .map(|p| (tx.tx.from.clone(), p.amount))
            })
            .collect();

        // Build simulated_reward_state for v3 proofs
        // Structure: (pending_rewards, staked_at, last_reward_at, claimed_rewards_total)
        let mut simulated_reward_state: HashMap<String, (u64, u64, Option<u64>, u64)> =
            HashMap::new();

        // Collect reward state for all affected addresses
        let mut all_reward_addresses: std::collections::HashSet<String> =
            affected_addresses.iter().cloned().collect();
        for info in &tx_parent_infos {
            for (_, creator) in &info.witness_parents {
                all_reward_addresses.insert(creator.clone());
            }
        }
        if let Some(ref v) = node_validator_address {
            all_reward_addresses.insert(v.clone());
        }
        for address in &all_reward_addresses {
            let pending = rewards.get_pending_rewards(address);
            let stake_info = rewards.get_stake(address);
            let (staked_at, last_reward_at) = stake_info
                .map(|p| (p.staked_at, p.last_reward_at))
                .unwrap_or((0, None));
            let claimed_total = rewards.get_claimed_total(address);
            simulated_reward_state.insert(
                address.clone(),
                (pending, staked_at, last_reward_at, claimed_total),
            );
        }
        let tip_pct = rewards.get_config().tip_reward_percent;
        let witness_pct = rewards.get_config().witness_reward_percent;
        drop(rewards);

        let mut sorted_txs: Vec<&SignedTransaction> = pending_txs.iter().collect();
        sorted_txs.sort_by(|a, b| {
            a.tx.from
                .cmp(&b.tx.from)
                .then(a.tx.nonce.cmp(&b.tx.nonce))
                .then(a.hash.cmp(&b.hash))
        });
        let mut executed_tx_hashes: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut balance_deferred_indices: Vec<usize> = Vec::new();

        let proof_available_nonces: std::collections::HashMap<
            String,
            std::collections::BTreeSet<u64>,
        > = {
            let mut map: std::collections::HashMap<String, std::collections::BTreeSet<u64>> =
                std::collections::HashMap::new();
            for tx in &sorted_txs {
                if !matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Consolidation)) {
                    map.entry(tx.tx.from.clone())
                        .or_default()
                        .insert(tx.tx.nonce);
                }
            }
            map
        };
        for (sender_addr, sender_nonces) in &proof_available_nonces {
            if let Some(sender) = simulated_accounts.get_mut(sender_addr) {
                if let Some(&first_nonce) = sender_nonces.iter().next() {
                    if first_nonce > sender.1 {
                        if let Some(&fp_nonce) = fast_path_nonces.get(sender_addr) {
                            if fp_nonce >= first_nonce {
                                tracing::info!(
                                    "FAST-PATH-NONCE-ADVANCE (proof-sim): sender {} nonce {} -> {} (fast-path nonce: {}, bridging {} gap nonces)",
                                    &sender_addr[..16.min(sender_addr.len())],
                                    sender.1, first_nonce, fp_nonce, first_nonce - sender.1
                                );
                                sender.1 = first_nonce;
                            }
                        } else {
                            tracing::debug!(
                                "NONCE-GAP-SKIP (proof-sim): sender {} has gap — account nonce {} but first tx nonce {} (deferring)",
                                &sender_addr[..16.min(sender_addr.len())],
                                sender.1, first_nonce
                            );
                        }
                    }
                }
            }
        }

        for (idx, tx) in sorted_txs.iter().enumerate() {
            if matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Consolidation)) {
                executed_tx_hashes.insert(tx.hash.clone());
                continue;
            }
            let from = &tx.tx.from;
            let to = &tx.tx.to;
            let amount = tx.tx.amount;
            let fee = tx.tx.gas_price.unwrap_or(current_gas_price);

            let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Stake));
            let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Unstake));
            let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::TransactionKind::ClaimRewards));
            let is_contract_tx = matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Contract));

            let sender_exists = simulated_accounts.contains_key(from);
            let mut tx_applied = false;

            if let Some(sender) = simulated_accounts.get_mut(from) {
                if tx.tx.nonce != sender.1 {
                    continue;
                }
                let tx_cost = if is_stake_tx {
                    amount + fee
                } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                    fee
                } else {
                    amount + fee
                };
                if sender.0 < tx_cost {
                    balance_deferred_indices.push(idx);
                    continue;
                }
                sender.0 -= tx_cost;
                sender.1 = tx.tx.nonce + 1;
                tx_applied = true;
            }

            if tx_applied || !sender_exists {
                executed_tx_hashes.insert(tx.hash.clone());
                sim_changed_addrs.insert(from.clone());
                if !is_stake_tx && !is_unstake_tx && !is_claim_tx && !is_contract_tx {
                    if let Some(receiver) = simulated_accounts.get_mut(to) {
                        receiver.0 += amount;
                    } else {
                        simulated_accounts.insert(to.clone(), (amount, 0, 0));
                    }
                    sim_changed_addrs.insert(to.clone());
                }
            }

            if tx_applied || !sender_exists {
                if is_stake_tx {
                    if let Some(staker) = simulated_accounts.get_mut(from) {
                        staker.2 += amount;
                    }
                    if let Some(reward_state) = simulated_reward_state.get_mut(from) {
                        if reward_state.1 == 0 {
                            reward_state.1 = tx.tx.timestamp;
                        }
                    } else {
                        simulated_reward_state.insert(from.clone(), (0, tx.tx.timestamp, None, 0));
                    }
                } else if is_unstake_tx {
                    if let Some(rewards_stake) = stake_amounts_snapshot.get(from) {
                        if let Some(staker) = simulated_accounts.get_mut(from) {
                            staker.0 += rewards_stake;
                            staker.2 = 0;
                        }
                    } else {
                        if let Some(staker) = simulated_accounts.get_mut(from) {
                            let unstaked = staker.2;
                            staker.0 += unstaked;
                            staker.2 = 0;
                        }
                    }
                } else if is_claim_tx {
                    if let Some(claimed) = pending_rewards_snapshot.get(from) {
                        if *claimed > 0 {
                            if let Some(claimer) = simulated_accounts.get_mut(from) {
                                let old_balance = claimer.0;
                                claimer.0 += claimed;
                                tracing::info!(
                                    "[SIMULATION] Claim for {}: pending_rewards={}, old_balance={}, new_balance={}",
                                    &from[..16.min(from.len())],
                                    claimed,
                                    old_balance,
                                    claimer.0
                                );

                                if let Some(reward_state) = simulated_reward_state.get_mut(from) {
                                    reward_state.0 = 0;
                                    reward_state.3 += claimed;
                                }
                            }
                        } else {
                            tracing::warn!(
                                "[SIMULATION] Claim for {}: pending_rewards is 0!",
                                &from[..16.min(from.len())]
                            );
                        }
                    } else {
                        tracing::warn!(
                            "[SIMULATION] Claim for {}: NO pending_rewards in snapshot!",
                            &from[..16.min(from.len())]
                        );
                    }
                }
            }
        }

        for _retry_pass in 0..5 {
            if balance_deferred_indices.is_empty() {
                break;
            }

            let mut still_deferred: Vec<usize> = Vec::new();
            let mut made_progress = false;

            for &idx in &balance_deferred_indices {
                let tx = sorted_txs[idx];
                if executed_tx_hashes.contains(&tx.hash) {
                    continue;
                }

                let from = &tx.tx.from;
                let to = &tx.tx.to;
                let amount = tx.tx.amount;
                let fee = tx.tx.gas_price.unwrap_or(current_gas_price);

                let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Stake));
                let is_unstake_tx =
                    matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Unstake));
                let is_claim_tx =
                    matches!(tx.tx.kind, Some(rinku_core::TransactionKind::ClaimRewards));
                let is_contract_tx =
                    matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Contract));

                let sender_exists = simulated_accounts.contains_key(from);
                let mut tx_applied = false;

                if let Some(sender) = simulated_accounts.get_mut(from) {
                    if tx.tx.nonce != sender.1 {
                        still_deferred.push(idx);
                        continue;
                    }
                    let tx_cost = if is_stake_tx {
                        amount + fee
                    } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                        fee
                    } else {
                        amount + fee
                    };
                    if sender.0 < tx_cost {
                        still_deferred.push(idx);
                        continue;
                    }
                    sender.0 -= tx_cost;
                    sender.1 = tx.tx.nonce + 1;
                    tx_applied = true;
                }

                if tx_applied || !sender_exists {
                    executed_tx_hashes.insert(tx.hash.clone());
                    made_progress = true;
                    sim_changed_addrs.insert(from.clone());
                    if !is_stake_tx && !is_unstake_tx && !is_claim_tx && !is_contract_tx {
                        if let Some(receiver) = simulated_accounts.get_mut(to) {
                            receiver.0 += amount;
                        } else {
                            simulated_accounts.insert(to.clone(), (amount, 0, 0));
                        }
                        sim_changed_addrs.insert(to.clone());
                    }
                }

                if tx_applied || !sender_exists {
                    if is_stake_tx {
                        if let Some(staker) = simulated_accounts.get_mut(from) {
                            staker.2 += amount;
                        }
                        if let Some(reward_state) = simulated_reward_state.get_mut(from) {
                            if reward_state.1 == 0 {
                                reward_state.1 = tx.tx.timestamp;
                            }
                        } else {
                            simulated_reward_state
                                .insert(from.clone(), (0, tx.tx.timestamp, None, 0));
                        }
                    } else if is_unstake_tx {
                        if let Some(rewards_stake) = stake_amounts_snapshot.get(from) {
                            if let Some(staker) = simulated_accounts.get_mut(from) {
                                staker.0 += rewards_stake;
                                staker.2 = 0;
                            }
                        } else {
                            if let Some(staker) = simulated_accounts.get_mut(from) {
                                let unstaked = staker.2;
                                staker.0 += unstaked;
                                staker.2 = 0;
                            }
                        }
                    } else if is_claim_tx {
                        if let Some(claimed) = pending_rewards_snapshot.get(from) {
                            if *claimed > 0 {
                                if let Some(claimer) = simulated_accounts.get_mut(from) {
                                    claimer.0 += claimed;
                                    if let Some(reward_state) = simulated_reward_state.get_mut(from)
                                    {
                                        reward_state.0 = 0;
                                        reward_state.3 += claimed;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if made_progress {
                tracing::info!(
                    "Proof simulation cross-sender retry: executed {} more txs ({} still pending)",
                    balance_deferred_indices.len() - still_deferred.len(),
                    still_deferred.len()
                );
            }

            balance_deferred_indices = still_deferred;
            if !made_progress {
                break;
            }
        }

        {
            let mut sim_tip_witness_total = 0u64;
            for info in &tx_parent_infos {
                if !executed_tx_hashes.contains(&info.tx_hash) {
                    continue;
                }
                if let Some(ref validator) = node_validator_address {
                    if info.first_parent_hash.is_some() {
                        let tip_amount = (info.reward_base as f64 * tip_pct).round() as u64;
                        if tip_amount > 0 {
                            let entry = simulated_reward_state
                                .entry(validator.clone())
                                .or_insert((0, 0, None, 0));
                            entry.0 += tip_amount;
                            sim_tip_witness_total += tip_amount;
                        }
                    }
                }
                for (_, creator) in &info.witness_parents {
                    let witness_amount = (info.reward_base as f64 * witness_pct).round() as u64;
                    if witness_amount > 0 {
                        let entry = simulated_reward_state
                            .entry(creator.clone())
                            .or_insert((0, 0, None, 0));
                        entry.0 += witness_amount;
                        sim_tip_witness_total += witness_amount;
                    }
                }
            }
            if sim_tip_witness_total > 0 {
                tracing::info!(
                    "Proof computation: simulated {} micro-RKU tip/witness rewards for {} executed txs",
                    sim_tip_witness_total, executed_tx_hashes.len()
                );
            }
        }

        let simulated_account_count = simulated_accounts.len();

        let (state_root, mut proof_trie) = if sim_changed_addrs.is_empty() {
            let root = original_root;
            (root, None)
        } else {
            let mut pt = forked_trie;
            use crate::sparse_merkle_trie::hash_account_key;
            let entries: Vec<([u8; 32], Vec<u8>)> = sim_changed_addrs
                .iter()
                .filter_map(|addr| {
                    simulated_accounts
                        .get(addr)
                        .map(|(balance, nonce, staked)| {
                            let key = hash_account_key(addr);
                            let value =
                                format!("account:{}:{}:{}:{}", addr, balance, nonce, staked);
                            (key, value.into_bytes())
                        })
                })
                .collect();
            if let Err(e) = pt.batch_set(&entries, None) {
                tracing::error!("Failed to batch_set proof trie: {}", e);
            }
            let root = pt.root_hex();
            (root, Some(pt))
        };

        let mut proofs: HashMap<String, rinku_core::types::AccountStateProof> = HashMap::new();

        if let (Some(height), Some(ref mut pt)) = (checkpoint_height, proof_trie.as_mut()) {
            use crate::sparse_merkle_trie::hash_account_key;
            let proof_addresses: Vec<&String> = sim_changed_addrs.iter().collect();
            tracing::info!(
                "Proof generation: {} changed addresses (affected_addresses={}, sim_changed={})",
                proof_addresses.len(),
                affected_addresses.len(),
                sim_changed_addrs.len()
            );
            for address in &proof_addresses {
                if let Some((balance, nonce, staked)) = simulated_accounts.get(*address) {
                    let key = hash_account_key(address);
                    match pt.prove(&key, None) {
                        Ok(merkle_proof_data) => {
                            let merkle_proof: Vec<String> = merkle_proof_data
                                .siblings
                                .iter()
                                .map(|s| hex::encode(s))
                                .collect();

                            tracing::debug!(
                                "Generating SMT proof for {}: balance={}, nonce={}, staked={} (checkpoint {}, state_root={})",
                                &address[..16.min(address.len())],
                                balance, nonce, staked,
                                height,
                                &state_root[..16.min(state_root.len())]
                            );

                            let (pending_rewards, staked_at, last_reward_at, claimed_total) =
                                simulated_reward_state
                                    .get(*address)
                                    .cloned()
                                    .unwrap_or((0, 0, None, 0));

                            let proof = rinku_core::types::AccountStateProof {
                                version: 4,
                                address: (*address).clone(),
                                balance_micro: *balance,
                                balance: rinku_core::types::from_micro_units(*balance),
                                nonce: *nonce,
                                staked_micro: *staked,
                                staked: rinku_core::types::from_micro_units(*staked),
                                pending_rewards_micro: pending_rewards,
                                pending_rewards: rinku_core::types::from_micro_units(
                                    pending_rewards,
                                ),
                                staked_at,
                                last_reward_at,
                                claimed_rewards_total_micro: claimed_total,
                                claimed_rewards_total: rinku_core::types::from_micro_units(
                                    claimed_total,
                                ),
                                checkpoint_height: height,
                                checkpoint_hash: String::new(),
                                checkpoint_timestamp: 0,
                                state_root: state_root.clone(),
                                merkle_proof,
                                merkle_index: 0,
                                is_on_demand: false,
                                bls_aggregated_sig: None,
                                bls_signer_bitmap: None,
                                tx_hash: tx_hash.to_string(),
                            };

                            proofs.insert((*address).clone(), proof);
                        }
                        Err(e) => {
                            tracing::error!(
                                "Failed to generate SMT proof for {}: {}",
                                &address[..16.min(address.len())],
                                e
                            );
                        }
                    }
                }
            }
        }

        let skipped = sorted_txs.len() - executed_tx_hashes.len();
        if skipped > 0 {
            tracing::warn!(
                "Proof simulation: {}/{} TXs skipped (nonce gaps or insufficient balance)",
                skipped,
                sorted_txs.len()
            );
        }
        let root_ms = root_start.elapsed().as_millis();
        if root_ms > 20 {
            tracing::info!(
                "STATE-ROOT-PROOFS (SMT): computed in {}ms ({} accounts, {} changed, {} txs simulated, {} proofs, h={:?})",
                root_ms, simulated_account_count, sim_changed_addrs.len(), pending_txs.len(), proofs.len(), checkpoint_height
            );
        }
        StateRootWithProofs {
            state_root,
            proofs,
            executed_tx_hashes,
        }
    }

    /// Normalize f64 to 8 decimal places for consistent hashing (matches sync_verification)
    /// Convert f64 balance to u64 micro-units (1 RKU = 100,000,000 micro-RKU)
    pub(crate) fn to_micro_units(value: f64) -> u64 {
        rinku_core::types::to_micro_units(value)
    }

    /// Hash data using SHA256 and return hex string (matches sync_verification)
    fn sha256_hex_for_proof(data: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(data.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Hash an account leaf using u64 micro-units for deterministic cross-language verification
    ///
    /// Canonical format: "account:{address}:{balance_micro}:{nonce}:{staked_micro}"
    /// Where balance_micro and staked_micro are u64 values (1 RKU = 100,000,000 micro-RKU)
    pub(crate) fn hash_account_leaf_for_proof(
        addr: &str,
        balance: u64,
        nonce: u64,
        stake: u64,
    ) -> String {
        let data = format!("account:{}:{}:{}:{}", addr, balance, nonce, stake);
        Self::sha256_hex_for_proof(&data)
    }

    /// Hash internal merkle node (matches sync_verification::hash_internal format)
    pub(crate) fn hash_internal_for_proof(left: &str, right: &str) -> String {
        let data = format!("node:{}:{}", left, right);
        Self::sha256_hex_for_proof(&data)
    }

    /// Generate a self-contained proof for an account's current state
    /// Returns the proof along with the merkle path for verification
    /// Uses the same leaf/node format as sync_verification for consistency
    pub async fn generate_account_state_proof(
        &self,
        address: &str,
        checkpoint: &rinku_core::types::Checkpoint,
        tx_hash: &str,
    ) -> Option<rinku_core::types::AccountStateProof> {
        let (balance_micro, nonce, staked_micro, merkle_proof) = {
            let state = self.inner.read().await;

            let account = state.accounts.get(address)?;
            let bal = account.balance;
            let n = account.nonce;
            let stk = account.staked;

            tracing::debug!(
                "Generating SMT proof for {}: balance={}, nonce={}, staked={} (checkpoint {}, state_root={})",
                &address[..16.min(address.len())],
                bal, n, stk,
                checkpoint.height,
                &checkpoint.state_root[..16.min(checkpoint.state_root.len())]
            );

            use crate::sparse_merkle_trie::hash_account_key;
            let key = hash_account_key(address);
            match state.state_trie.prove(&key, None) {
                Ok(proof_data) => {
                    let proof: Vec<String> =
                        proof_data.siblings.iter().map(|s| hex::encode(s)).collect();
                    (bal, n, stk, proof)
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to generate SMT proof for {}: {}",
                        &address[..16.min(address.len())],
                        e
                    );
                    return None;
                }
            }
        };

        let rewards = self.rewards.read().await;
        let pending_rewards = rewards.get_pending_rewards(address);
        let stake_info = rewards.get_stake(address);
        let (staked_at, last_reward_at) = stake_info
            .map(|p| (p.staked_at, p.last_reward_at))
            .unwrap_or((0, None));
        let claimed_total = rewards.get_claimed_total(address);
        drop(rewards);

        Some(rinku_core::types::AccountStateProof {
            version: 4,
            address: address.to_string(),
            balance_micro,
            balance: rinku_core::types::from_micro_units(balance_micro),
            nonce,
            staked_micro,
            staked: rinku_core::types::from_micro_units(staked_micro),
            pending_rewards_micro: pending_rewards,
            pending_rewards: rinku_core::types::from_micro_units(pending_rewards),
            staked_at,
            last_reward_at,
            claimed_rewards_total_micro: claimed_total,
            claimed_rewards_total: rinku_core::types::from_micro_units(claimed_total),
            checkpoint_height: checkpoint.height,
            checkpoint_hash: checkpoint.hash.clone(),
            checkpoint_timestamp: checkpoint.timestamp,
            state_root: checkpoint.state_root.clone(),
            merkle_proof,
            merkle_index: 0,
            bls_aggregated_sig: checkpoint.aggregated_signature.clone(),
            bls_signer_bitmap: checkpoint.signer_bitmap.as_ref().map(|b| hex::encode(b)),
            tx_hash: tx_hash.to_string(),
            is_on_demand: false,
        })
    }

    /// Generate a fresh proof for an account at the latest checkpoint
    /// This is used when users request a proof via the explorer, regardless of recent activity
    /// The proof uses the checkpoint's actual BLS-signed state_root
    pub async fn generate_account_state_proof_on_demand(
        &self,
        address: &str,
    ) -> Option<rinku_core::types::AccountStateProof> {
        let state = self.inner.read().await;

        let checkpoint = state.checkpoints.last()?.clone();

        let (snapshot_height, snapshot) = state.checkpoint_accounts_snapshot.as_ref()?;

        if *snapshot_height != checkpoint.height {
            tracing::warn!(
                "On-demand proof skipped for {}: snapshot height {} != checkpoint height {}",
                &address[..16.min(address.len())],
                snapshot_height,
                checkpoint.height
            );
            return None;
        }

        let (balance_micro, nonce, staked_micro) = snapshot.get(address).copied()?;

        use crate::sparse_merkle_trie::hash_account_key;
        let mut on_demand_trie = state.state_trie.fork();
        for (addr, (balance, n, staked)) in snapshot.iter() {
            let key = hash_account_key(addr);
            let value = format!("account:{}:{}:{}:{}", addr, balance, n, staked);
            let _ = on_demand_trie.set(&key, value.into_bytes(), None);
        }

        let key = hash_account_key(address);
        let merkle_proof_result = on_demand_trie.prove(&key, None);
        let merkle_proof = match merkle_proof_result {
            Ok(proof_data) => proof_data.siblings.iter().map(|s| hex::encode(s)).collect(),
            Err(e) => {
                tracing::error!(
                    "Failed to generate on-demand SMT proof for {}: {}",
                    &address[..16.min(address.len())],
                    e
                );
                return None;
            }
        };

        drop(state);

        let rewards = self.rewards.read().await;
        let pending_rewards = rewards.get_pending_rewards(address);
        let stake_info = rewards.get_stake(address);
        let (staked_at, last_reward_at) = stake_info
            .map(|p| (p.staked_at, p.last_reward_at))
            .unwrap_or((0, None));
        let claimed_total = rewards.get_claimed_total(address);
        drop(rewards);

        tracing::info!(
            "On-demand proof for {} using checkpoint {} snapshot: balance={}, nonce={}, staked={}",
            &address[..16.min(address.len())],
            checkpoint.height,
            balance_micro,
            nonce,
            staked_micro
        );

        Some(rinku_core::types::AccountStateProof {
            version: 4,
            address: address.to_string(),
            balance_micro,
            balance: rinku_core::types::from_micro_units(balance_micro),
            nonce,
            staked_micro,
            staked: rinku_core::types::from_micro_units(staked_micro),
            pending_rewards_micro: pending_rewards,
            pending_rewards: rinku_core::types::from_micro_units(pending_rewards),
            staked_at,
            last_reward_at,
            claimed_rewards_total_micro: claimed_total,
            claimed_rewards_total: rinku_core::types::from_micro_units(claimed_total),
            checkpoint_height: checkpoint.height,
            checkpoint_hash: checkpoint.hash.clone(),
            checkpoint_timestamp: checkpoint.timestamp,
            state_root: checkpoint.state_root.clone(),
            merkle_proof,
            merkle_index: 0,
            bls_aggregated_sig: checkpoint.aggregated_signature.clone(),
            bls_signer_bitmap: checkpoint.signer_bitmap.as_ref().map(|b| hex::encode(b)),
            tx_hash: "on-demand".to_string(),
            is_on_demand: true,
        })
    }

    fn build_merkle_tree_levels(leaves: &[String]) -> Vec<Vec<String>> {
        let mut levels: Vec<Vec<String>> = Vec::new();
        levels.push(leaves.to_vec());

        let mut current_level = leaves.to_vec();
        while current_level.len() > 1 {
            let mut next_level = Vec::new();
            for chunk in current_level.chunks(2) {
                let left = &chunk[0];
                let right = if chunk.len() > 1 {
                    &chunk[1]
                } else {
                    &chunk[0]
                };
                next_level.push(Self::hash_internal_for_proof(left, right));
            }
            levels.push(next_level.clone());
            current_level = next_level;
        }

        levels
    }

    fn extract_proof_from_levels(levels: &[Vec<String>], target_index: usize) -> Vec<String> {
        if levels.is_empty() || levels.len() == 1 {
            return vec![];
        }

        let mut proof = Vec::new();
        let mut current_index = target_index;

        for level in &levels[..levels.len() - 1] {
            let sibling_index = if current_index % 2 == 0 {
                current_index + 1
            } else {
                current_index - 1
            };

            if sibling_index < level.len() {
                proof.push(level[sibling_index].clone());
            } else {
                proof.push(level[current_index].clone());
            }

            current_index /= 2;
        }

        proof
    }

    fn compute_merkle_proof_path_canonical(leaves: &[String], target_index: usize) -> Vec<String> {
        if leaves.is_empty() || leaves.len() == 1 {
            return vec![];
        }

        let levels = Self::build_merkle_tree_levels(leaves);
        Self::extract_proof_from_levels(&levels, target_index)
    }

    /// Update balance proofs for accounts affected by finalized transactions
    /// IMPORTANT: This must be called AFTER execute_finalized_transaction has completed
    /// for all transactions, so that state.accounts contains the post-execution values
    /// that match what was simulated in compute_state_root_with_pending_txs
    pub async fn update_account_balance_proofs(
        &self,
        addresses: &[String],
        checkpoint: &rinku_core::types::Checkpoint,
        tx_hash: &str,
    ) {
        for address in addresses {
            if let Some(proof) = self
                .generate_account_state_proof(address, checkpoint, tx_hash)
                .await
            {
                let mut state = self.inner.write().await;
                if let Some(account) = state.accounts.get_mut(address) {
                    // Log before updating to help debug proof issues
                    tracing::info!(
                        "Updating balance proof for {} at checkpoint {}: balance={:.4}, nonce={}, staked={:.4}",
                        &address[..16.min(address.len())],
                        checkpoint.height,
                        proof.balance,
                        proof.nonce,
                        proof.staked
                    );
                    account.latest_balance_proof = Some(proof);
                }
            } else {
                tracing::warn!(
                    "Failed to generate balance proof for {} at checkpoint {}",
                    &address[..16.min(address.len())],
                    checkpoint.height
                );
            }
        }
    }

    /// Store precomputed proofs from checkpoint simulation
    /// CRITICAL: These proofs were computed from the same simulated account set used for state_root,
    /// guaranteeing that merkle proof verification will succeed against the checkpoint's state_root.
    /// This is used by the checkpoint LEADER to store proofs computed before transaction execution.
    ///
    /// CONSENSUS FIX: For followers, this also SYNCHRONIZES local account state to match the leader's
    /// authoritative values. This is essential because non-deterministic operations (like ClaimRewards
    /// where pending_rewards can vary based on timing) could cause balance divergence if followers
    /// only execute locally without syncing to leader's computed state.
    pub async fn store_precomputed_proofs(
        &self,
        proofs: &std::collections::HashMap<String, rinku_core::types::AccountStateProof>,
    ) {
        // First pass: sync account state and collect addresses needing RewardsService sync
        // For v3 proofs, we now have authoritative reward state: (address, pending_rewards, staked_at, last_reward_at, claimed_total, staked_amount)
        let (rewards_to_sync, fast_path_stake_adjustments) = {
            let mut rewards_to_sync: Vec<(String, u64, u64, Option<u64>, u64, u64)> = Vec::new();
            let mut state = self.inner.write().await;

            let mut sync_count = 0usize;
            let mut synced_addresses: Vec<String> = Vec::new();
            for (address, proof) in proofs {
                let is_v3_proof = proof.version >= 3;

                if let Some(account) = state.accounts.get_mut(address) {
                    let balance_diff = account.balance.abs_diff(proof.balance_micro);
                    let staked_diff = account.staked.abs_diff(proof.staked_micro);

                    if balance_diff > 0 || staked_diff > 0 || account.nonce != proof.nonce {
                        tracing::debug!(
                            "STATE SYNC for {} at checkpoint {}: local(bal={}, nonce={}, stk={}) -> leader(bal={}, nonce={}, stk={})",
                            &address[..16.min(address.len())],
                            proof.checkpoint_height,
                            account.balance, account.nonce, account.staked,
                            proof.balance_micro, proof.nonce, proof.staked_micro
                        );
                        sync_count += 1;
                        synced_addresses.push(address.clone());
                    }
                    account.balance = proof.balance_micro;
                    account.nonce = proof.nonce;
                    account.staked = proof.staked_micro;
                    if is_v3_proof {
                        rewards_to_sync.push((
                            address.clone(),
                            proof.pending_rewards_micro,
                            proof.staked_at,
                            proof.last_reward_at,
                            proof.claimed_rewards_total_micro,
                            proof.staked_micro,
                        ));
                    }
                    account.latest_balance_proof = Some(proof.clone());
                } else {
                    tracing::info!(
                        "Creating account {} from leader proof at checkpoint {}: balance={}, nonce={}, staked={}",
                        &address[..16.min(address.len())],
                        proof.checkpoint_height,
                        proof.balance_micro,
                        proof.nonce,
                        proof.staked_micro
                    );
                    let mut new_account =
                        Account::new(address.clone(), proof.checkpoint_height as u64);
                    new_account.balance = proof.balance_micro;
                    new_account.nonce = proof.nonce;
                    new_account.staked = proof.staked_micro;
                    new_account.latest_balance_proof = Some(proof.clone());
                    state.accounts.insert(address.clone(), new_account);
                    sync_count += 1;
                    synced_addresses.push(address.clone());
                    if is_v3_proof {
                        rewards_to_sync.push((
                            address.clone(),
                            proof.pending_rewards_micro,
                            proof.staked_at,
                            proof.last_reward_at,
                            proof.claimed_rewards_total_micro,
                            proof.staked_micro,
                        ));
                    }
                }
            }
            let fast_path_stake_adjustments: Vec<(String, u64)>;

            if sync_count > 0 {
                state.update_state_trie_accounts(&synced_addresses);

                let fp_count = state.fast_path_finalized_txs.len();

                tracing::info!(
                    "Proof sync complete: {} accounts corrected ({} fast-path finalized entries)",
                    sync_count,
                    fp_count
                );
                fast_path_stake_adjustments = state
                    .fast_path_finalized_txs
                    .values()
                    .filter(|e| matches!(e.kind, Some(rinku_core::types::TransactionKind::Stake)))
                    .map(|e| (e.from.clone(), e.amount))
                    .collect();
            } else {
                tracing::debug!(
                    "Proof sync: no state corrections needed ({} fast-path finalized entries)",
                    state.fast_path_finalized_txs.len()
                );
                fast_path_stake_adjustments = Vec::new();
            }

            (rewards_to_sync, fast_path_stake_adjustments)
        }; // Release state lock

        if !rewards_to_sync.is_empty() {
            let mut rewards = self.rewards.write().await;
            for (
                address,
                pending_rewards,
                staked_at,
                last_reward_at,
                claimed_total,
                staked_amount,
            ) in &rewards_to_sync
            {
                rewards.sync_from_leader_v3(
                    address,
                    *pending_rewards,
                    *staked_at,
                    *last_reward_at,
                    *claimed_total,
                    *staked_amount,
                );
            }
            for (address, fp_amount) in &fast_path_stake_adjustments {
                if let Some(existing) = rewards.get_stake_mut(address) {
                    existing.amount += fp_amount;
                    tracing::info!(
                        "Proof sync: adjusted rewards.stakes for {} by +{} for fast-path finalized (new total={})",
                        &address[..16.min(address.len())],
                        fp_amount,
                        existing.amount
                    );
                } else {
                    rewards.sync_stake_amount(address, *fp_amount);
                }
            }
        } else if !fast_path_stake_adjustments.is_empty() {
            let mut rewards = self.rewards.write().await;
            for (address, fp_amount) in &fast_path_stake_adjustments {
                if let Some(existing) = rewards.get_stake_mut(address) {
                    existing.amount += fp_amount;
                } else {
                    rewards.sync_stake_amount(address, *fp_amount);
                }
            }
        }
    }
}
