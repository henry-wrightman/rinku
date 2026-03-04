use super::*;

impl NodeState {
    /// Get pending (unfinalized) transaction stats for a sender
    /// Returns (pending_outgoing_amount, pending_gas, pending_tx_count)
    /// Used for finality-first validation: effective_balance = confirmed - pending_outgoing - pending_gas
    pub(crate) fn get_pending_stats_for_sender(state: &StateInner, sender: &str) -> (u64, u64, u64) {
        let mut pending_amount = 0u64;
        let mut pending_gas = 0u64;
        let mut pending_count = 0u64;
        
        for node in state.dag.get_all_nodes() {
            if !node.finalized && node.tx.tx.from == sender {
                let gas = node.tx.tx.gas_price.unwrap_or(state.current_gas_price);
                pending_gas += gas;
                
                let is_unstake = matches!(node.tx.tx.kind, Some(rinku_core::types::TransactionKind::Unstake));
                let is_claim = matches!(node.tx.tx.kind, Some(rinku_core::types::TransactionKind::ClaimRewards));
                if !is_unstake && !is_claim {
                    pending_amount += node.tx.tx.amount;
                }
                pending_count += 1;
            }
        }
        
        (pending_amount, pending_gas, pending_count)
    }
    
    /// Get the expected nonce for a sender, accounting for pending (unfinalized) transactions
    /// effective_nonce = confirmed_nonce + pending_tx_count
    pub(crate) fn get_effective_nonce(state: &StateInner, sender: &str) -> u64 {
        let confirmed_nonce = state.accounts.get(sender).map(|a| a.nonce).unwrap_or(0);
        let (_, _, pending_count) = Self::get_pending_stats_for_sender(state, sender);
        confirmed_nonce + pending_count
    }
    
    /// Get effective balance for a sender, accounting for pending (unfinalized) transactions
    /// effective_balance = confirmed_balance - pending_outgoing - pending_gas
    pub(crate) fn get_effective_balance(state: &StateInner, sender: &str) -> u64 {
        let confirmed_balance = state.accounts.get(sender).map(|a| a.balance).unwrap_or(0);
        let (pending_amount, pending_gas, _) = Self::get_pending_stats_for_sender(state, sender);
        confirmed_balance.saturating_sub(pending_amount).saturating_sub(pending_gas)
    }

    /// Compute state root from all account states
    /// This creates a deterministic merkle root from sorted account data
    /// Uses canonical format matching sync_verification: "account:address:balance:nonce:stake"
    /// Internal nodes: "node:left_hash:right_hash"
    pub async fn compute_state_root(&self) -> String {
        let state = self.inner.read().await;
        
        // Get sorted accounts for deterministic ordering (same as sync_verification)
        let mut account_entries: Vec<_> = state.accounts.iter().collect();
        account_entries.sort_by(|a, b| a.0.cmp(b.0));
        
        // Create leaf hashes using canonical format (matches sync_verification::hash_account_leaf)
        let leaves: Vec<String> = account_entries
            .iter()
            .map(|(address, account)| {
                Self::hash_account_leaf_for_proof(address, account.balance, account.nonce, account.staked)
            })
            .collect();
        
        if leaves.is_empty() {
            return "0".repeat(64);
        }
        
        if leaves.len() == 1 {
            return leaves[0].clone();
        }
        
        // Build merkle tree using canonical internal node format (matches sync_verification::hash_internal)
        let mut current_level = leaves;
        while current_level.len() > 1 {
            let mut next_level = Vec::new();
            for chunk in current_level.chunks(2) {
                let left = &chunk[0];
                let right = if chunk.len() > 1 { &chunk[1] } else { &chunk[0] };
                next_level.push(Self::hash_internal_for_proof(left, right));
            }
            current_level = next_level;
        }
        
        current_level[0].clone()
    }
    
    /// Compute state root with pending transactions applied (without modifying actual state)
    /// This is used by checkpoint creation to get the correct post-execution state root
    /// before actually executing the transactions
    pub async fn compute_state_root_with_pending_txs(&self, pending_txs: &[rinku_core::SignedTransaction], skip_hashes: &std::collections::HashSet<String>) -> String {
        self.compute_state_root_and_proofs(pending_txs, &[], None, "", skip_hashes).await.state_root
    }
    
    /// Compute state root AND precomputed proofs for affected addresses
    /// CRITICAL: Proofs must be computed from the same simulated account set used for state_root
    /// to ensure merkle proof verification will succeed
    pub async fn compute_state_root_and_proofs(
        &self,
        pending_txs: &[rinku_core::SignedTransaction],
        affected_addresses: &[String],
        checkpoint_template: Option<&rinku_core::types::Checkpoint>,
        tx_hash: &str,
        skip_hashes: &std::collections::HashSet<String>,
    ) -> StateRootWithProofs {
        use std::collections::HashMap;
        
        let state = self.inner.read().await;
        
        // Get the current gas price from state (used as fallback for tx without explicit gas_price)
        // CRITICAL: Must match execute_finalized_transaction which uses state.current_gas_price
        let current_gas_price = state.current_gas_price;
        
        // Clone accounts into a mutable HashMap for simulation
        let mut simulated_accounts: HashMap<String, (u64, u64, u64)> = state.accounts.iter()
            .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
            .collect();
        
        drop(state); // Release state lock before acquiring rewards lock
        
        // Get pending rewards and stake amounts snapshot for claim/unstake simulation
        // CRITICAL: Must use rewards service as source of truth to match execute_finalized_transaction
        let rewards = self.rewards.read().await;
        let pending_rewards_snapshot: HashMap<String, u64> = pending_txs.iter()
            .filter(|tx| matches!(tx.tx.kind, Some(rinku_core::TransactionKind::ClaimRewards)))
            .map(|tx| (tx.tx.from.clone(), rewards.get_pending_rewards(&tx.tx.from)))
            .collect();
        let stake_amounts_snapshot: HashMap<String, u64> = pending_txs.iter()
            .filter(|tx| matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Unstake)))
            .filter_map(|tx| {
                rewards.get_stake(&tx.tx.from).map(|p| (tx.tx.from.clone(), p.amount))
            })
            .collect();
        
        // Build simulated_reward_state for v3 proofs
        // Structure: (pending_rewards, staked_at, last_reward_at, claimed_rewards_total)
        let mut simulated_reward_state: HashMap<String, (u64, u64, Option<u64>, u64)> = HashMap::new();
        
        // Collect reward state for all affected addresses
        for address in affected_addresses {
            let pending = rewards.get_pending_rewards(address);
            let stake_info = rewards.get_stake(address);
            let (staked_at, last_reward_at) = stake_info
                .map(|p| (p.staked_at, p.last_reward_at))
                .unwrap_or((0, None));
            let claimed_total = rewards.get_claimed_total(address);
            simulated_reward_state.insert(
                address.clone(),
                (pending, staked_at, last_reward_at, claimed_total)
            );
        }
        drop(rewards);
        
        // Apply pending transactions to simulated state
        // This must match execute_finalized_transaction exactly!
        // CRITICAL: Skip transactions already executed on fast-path, since their
        // effects are already reflected in the current state we cloned above
        // CRITICAL: Skip consolidation transactions — execute_finalized_transactions_batch
        // skips them, so simulation must also skip them to maintain parity
        for tx in pending_txs {
            if skip_hashes.contains(&tx.hash) {
                continue;
            }
            if matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Consolidation)) {
                continue;
            }
            let from = &tx.tx.from;
            let to = &tx.tx.to;
            let amount = tx.tx.amount;
            // CRITICAL: Use the same gas price fallback as execute_finalized_transaction
            let fee = tx.tx.gas_price.unwrap_or(current_gas_price);
            
            let is_stake_tx = matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Stake));
            let is_unstake_tx = matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Unstake));
            let is_claim_tx = matches!(tx.tx.kind, Some(rinku_core::TransactionKind::ClaimRewards));
            let is_contract_tx = matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Contract));
            
            // Deduct from sender based on transaction type (matches execute_finalized_transaction)
            if let Some(sender) = simulated_accounts.get_mut(from) {
                if is_stake_tx {
                    // Stake: deduct amount + fee (amount goes to stake, not recipient)
                    sender.0 = sender.0.saturating_sub(amount).saturating_sub(fee);
                } else if is_unstake_tx || is_claim_tx || is_contract_tx {
                    sender.0 = sender.0.saturating_sub(fee);
                } else {
                    sender.0 = sender.0.saturating_sub(amount).saturating_sub(fee);
                }
                sender.1 += 1; // Increment nonce
            }
            
            // Credit recipient only for regular transfers (not stake/unstake/claim/contract)
            if !is_stake_tx && !is_unstake_tx && !is_claim_tx && !is_contract_tx {
                if let Some(receiver) = simulated_accounts.get_mut(to) {
                    receiver.0 += amount;
                } else {
                    // Create new account for receiver
                    simulated_accounts.insert(to.clone(), (amount, 0, 0));
                }
            }
            
            // Handle staking state changes
            if is_stake_tx {
                if let Some(staker) = simulated_accounts.get_mut(from) {
                    staker.2 += amount; // Increase stake
                }
                // Update simulated_reward_state with staked_at timestamp
                if let Some(reward_state) = simulated_reward_state.get_mut(from) {
                    if reward_state.1 == 0 { // staked_at was 0, set it now
                        reward_state.1 = tx.tx.timestamp;
                    }
                } else {
                    // Create new reward state entry for new stakers
                    simulated_reward_state.insert(from.clone(), (0, tx.tx.timestamp, None, 0));
                }
            } else if is_unstake_tx {
                // CRITICAL: Use rewards service stake amount (not account.staked) to match execution
                // execute_finalized_transaction uses rewards.unstake() which returns the rewards service value
                if let Some(rewards_stake) = stake_amounts_snapshot.get(from) {
                    if let Some(staker) = simulated_accounts.get_mut(from) {
                        staker.0 += rewards_stake; // Return stake to balance (from rewards service)
                        staker.2 = 0; // Clear stake
                    }
                } else {
                    if let Some(staker) = simulated_accounts.get_mut(from) {
                        let unstaked = staker.2;
                        staker.0 += unstaked;
                        staker.2 = 0;
                    }
                }
            } else if is_claim_tx {
                // Claim adds pending rewards to balance (matches execute_finalized_transaction)
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
                                reward_state.0 = 0; // pending_rewards = 0 after claim
                                reward_state.3 += claimed; // claimed_total += claimed amount
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
        
        // Get sorted accounts for deterministic ordering
        let mut account_entries: Vec<_> = simulated_accounts.iter().collect();
        account_entries.sort_by(|a, b| a.0.cmp(b.0));
        
        // Log simulated state for debugging proof generation issues
        for (addr, (balance, nonce, staked)) in account_entries.iter().take(5) {
            tracing::debug!(
                "Simulated state for {}: balance={}, nonce={}, staked={}",
                &addr[..16.min(addr.len())],
                balance,
                nonce,
                staked
            );
        }
        
        // Create leaf hashes using canonical format
        let leaves: Vec<String> = account_entries
            .iter()
            .map(|(address, (balance, nonce, staked))| {
                Self::hash_account_leaf_for_proof(address, *balance, *nonce, *staked)
            })
            .collect();
        
        if leaves.is_empty() {
            return StateRootWithProofs {
                state_root: "0".repeat(64),
                proofs: HashMap::new(),
            };
        }
        
        let state_root = if leaves.len() == 1 {
            leaves[0].clone()
        } else {
            // Build merkle tree using canonical internal node format
            let mut current_level = leaves.clone();
            while current_level.len() > 1 {
                let mut next_level = Vec::new();
                for chunk in current_level.chunks(2) {
                    let left = &chunk[0];
                    let right = if chunk.len() > 1 { &chunk[1] } else { &chunk[0] };
                    next_level.push(Self::hash_internal_for_proof(left, right));
                }
                current_level = next_level;
            }
            current_level[0].clone()
        };
        
        // Generate proofs for affected addresses using the SAME simulated account set
        // This is CRITICAL: proofs must be computed from identical data as state_root
        let mut proofs: HashMap<String, rinku_core::types::AccountStateProof> = HashMap::new();
        
        if let Some(checkpoint) = checkpoint_template {
            for address in affected_addresses {
                // Find the account in simulated_accounts (sorted by address)
                if let Some(idx) = account_entries.iter().position(|(addr, _)| *addr == address) {
                    let (_, (balance, nonce, staked)) = &account_entries[idx];
                    
                    // Compute merkle proof path from the simulated leaves
                    let merkle_proof = Self::compute_merkle_proof_path_canonical(&leaves, idx);
                    
                    tracing::info!(
                        "Generating proof for {}: balance={:.8}, nonce={}, staked={:.8} (checkpoint {}, state_root={})",
                        &address[..16.min(address.len())],
                        balance,
                        nonce,
                        staked,
                        checkpoint.height,
                        &state_root[..16.min(state_root.len())]
                    );
                    
                    // Get reward state from simulated_reward_state if available
                    let (pending_rewards, staked_at, last_reward_at, claimed_total) = 
                        simulated_reward_state.get(address)
                            .cloned()
                            .unwrap_or((0, 0, None, 0));
                    
                    let proof = rinku_core::types::AccountStateProof {
                        version: 3,
                        address: address.clone(),
                        balance_micro: *balance,
                        balance: rinku_core::types::from_micro_units(*balance),
                        nonce: *nonce,
                        staked_micro: *staked,
                        staked: rinku_core::types::from_micro_units(*staked),
                        pending_rewards_micro: pending_rewards,
                        pending_rewards: rinku_core::types::from_micro_units(pending_rewards),
                        staked_at,
                        last_reward_at,
                        claimed_rewards_total_micro: claimed_total,
                        claimed_rewards_total: rinku_core::types::from_micro_units(claimed_total),
                        checkpoint_height: checkpoint.height,
                        checkpoint_hash: checkpoint.hash.clone(),
                        checkpoint_timestamp: checkpoint.timestamp,
                        state_root: state_root.clone(),
                        merkle_proof,
                        merkle_index: idx,
                        is_on_demand: false,
                        bls_aggregated_sig: checkpoint.aggregated_signature.clone(),
                        bls_signer_bitmap: checkpoint.signer_bitmap.as_ref().map(|b| hex::encode(b)),
                        tx_hash: tx_hash.to_string(),
                    };
                    
                    proofs.insert(address.clone(), proof);
                }
            }
        }
        
        StateRootWithProofs { state_root, proofs }
    }

    /// Normalize f64 to 8 decimal places for consistent hashing (matches sync_verification)
    /// Convert f64 balance to u64 micro-units (1 RKU = 100,000,000 micro-RKU)
    pub(crate) fn to_micro_units(value: f64) -> u64 {
        rinku_core::types::to_micro_units(value)
    }
    
    /// Hash data using SHA256 and return hex string (matches sync_verification)
    fn sha256_hex_for_proof(data: &str) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(data.as_bytes());
        hex::encode(hasher.finalize())
    }
    
    /// Hash an account leaf using u64 micro-units for deterministic cross-language verification
    /// 
    /// Canonical format: "account:{address}:{balance_micro}:{nonce}:{staked_micro}"
    /// Where balance_micro and staked_micro are u64 values (1 RKU = 100,000,000 micro-RKU)
    pub(crate) fn hash_account_leaf_for_proof(addr: &str, balance: u64, nonce: u64, stake: u64) -> String {
        let data = format!(
            "account:{}:{}:{}:{}",
            addr,
            balance,
            nonce,
            stake
        );
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
        let (balance_micro, nonce, staked_micro, merkle_proof, merkle_index) = {
            let state = self.inner.read().await;
            
            let account = state.accounts.get(address)?;
            let bal = account.balance;
            let n = account.nonce;
            let stk = account.staked;
            
            tracing::info!(
                "Generating proof for {}: balance={:.8}, nonce={}, staked={:.8} (checkpoint {}, state_root={})",
                &address[..16.min(address.len())],
                bal,
                n,
                stk,
                checkpoint.height,
                &checkpoint.state_root[..16.min(checkpoint.state_root.len())]
            );
            
            let mut account_entries: Vec<_> = state.accounts.iter().collect();
            account_entries.sort_by(|a, b| a.0.cmp(b.0));
            
            let idx = account_entries
                .iter()
                .position(|(addr, _)| *addr == address)?;
            
            let leaves: Vec<String> = account_entries
                .iter()
                .map(|(addr, acc)| {
                    Self::hash_account_leaf_for_proof(addr, acc.balance, acc.nonce, acc.staked)
                })
                .collect();
            
            if leaves.is_empty() {
                return None;
            }
            
            let proof = Self::compute_merkle_proof_path_canonical(&leaves, idx);
            (bal, n, stk, proof, idx)
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
            version: 3,
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
            merkle_index,
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
        let account = state.accounts.get(address)?.clone();
        
        let mut account_entries: Vec<_> = state.accounts.iter().collect();
        account_entries.sort_by(|a, b| a.0.cmp(b.0));
        
        let merkle_index = account_entries
            .iter()
            .position(|(addr, _)| *addr == address)?;
        
        let leaves: Vec<String> = account_entries
            .iter()
            .map(|(addr, acc)| {
                Self::hash_account_leaf_for_proof(addr, acc.balance, acc.nonce, acc.staked)
            })
            .collect();
        
        if leaves.is_empty() {
            return None;
        }
        
        let merkle_proof = Self::compute_merkle_proof_path_canonical(&leaves, merkle_index);
        
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
            "Generated proof for {} at checkpoint {}: balance={}, nonce={}, staked={}",
            &address[..16.min(address.len())],
            checkpoint.height,
            account.balance,
            account.nonce,
            account.staked
        );
        
        Some(rinku_core::types::AccountStateProof {
            version: 3,
            address: address.to_string(),
            balance_micro: account.balance,
            balance: rinku_core::types::from_micro_units(account.balance),
            nonce: account.nonce,
            staked_micro: account.staked,
            staked: rinku_core::types::from_micro_units(account.staked),
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
            merkle_index,
            bls_aggregated_sig: checkpoint.aggregated_signature.clone(),
            bls_signer_bitmap: checkpoint.signer_bitmap.as_ref().map(|b| hex::encode(b)),
            tx_hash: "on-demand".to_string(),
            is_on_demand: false,
        })
    }
    
    /// Compute merkle proof path for a leaf at given index
    /// Uses canonical format matching sync_verification (hash_internal)
    fn compute_merkle_proof_path_canonical(leaves: &[String], target_index: usize) -> Vec<String> {
        if leaves.is_empty() || leaves.len() == 1 {
            return vec![];
        }
        
        let mut proof = Vec::new();
        let mut current_level: Vec<String> = leaves.to_vec();
        let mut current_index = target_index;
        
        while current_level.len() > 1 {
            // Get sibling
            let sibling_index = if current_index % 2 == 0 {
                current_index + 1
            } else {
                current_index - 1
            };
            
            if sibling_index < current_level.len() {
                proof.push(current_level[sibling_index].clone());
            } else {
                // Odd number of nodes, duplicate the last one
                proof.push(current_level[current_index].clone());
            }
            
            // Build next level using canonical hash_internal format
            let mut next_level = Vec::new();
            for chunk in current_level.chunks(2) {
                let left = &chunk[0];
                let right = if chunk.len() > 1 { &chunk[1] } else { &chunk[0] };
                next_level.push(Self::hash_internal_for_proof(left, right));
            }
            
            current_level = next_level;
            current_index /= 2;
        }
        
        proof
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
            if let Some(proof) = self.generate_account_state_proof(address, checkpoint, tx_hash).await {
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
        let mut rewards_to_sync: Vec<(String, u64, u64, Option<u64>, u64, u64)> = Vec::new();
        
        {
            let mut state = self.inner.write().await;
            for (address, proof) in proofs {
                let is_v3_proof = proof.version >= 3;
                
                if let Some(account) = state.accounts.get_mut(address) {
                    // Detect corrupted nonces: sequential nonces should never exceed ~1 billion
                    // (even at 1000 TPS for 30 years). Values >= 1 trillion are timestamp artifacts
                    // from the old tip consolidator bug (nonce = timestamp_ms).
                    const NONCE_CORRUPTION_THRESHOLD: u64 = 1_000_000_000_000;
                    let nonce_corrupted = account.nonce >= NONCE_CORRUPTION_THRESHOLD;
                    
                    if account.nonce > proof.nonce && !nonce_corrupted {
                        tracing::debug!(
                            "Skipping STATE SYNC for {} at checkpoint {}: local nonce {} > proof nonce {} (un-checkpointed txs)",
                            &address[..16.min(address.len())],
                            proof.checkpoint_height,
                            account.nonce,
                            proof.nonce
                        );
                    } else {
                        if nonce_corrupted {
                            tracing::warn!(
                                "NONCE REPAIR for {} at checkpoint {}: corrupted nonce {} -> leader nonce {}",
                                &address[..16.min(address.len())],
                                proof.checkpoint_height,
                                account.nonce,
                                proof.nonce
                            );
                        }
                        let balance_diff = account.balance.abs_diff(proof.balance_micro);
                        let staked_diff = account.staked.abs_diff(proof.staked_micro);
                        
                        if balance_diff > 0 || staked_diff > 0 || account.nonce != proof.nonce {
                            tracing::warn!(
                                "STATE SYNC for {} at checkpoint {}: local(bal={}, nonce={}, stk={}) -> leader(bal={}, nonce={}, stk={})",
                                &address[..16.min(address.len())],
                                proof.checkpoint_height,
                                account.balance, account.nonce, account.staked,
                                proof.balance_micro, proof.nonce, proof.staked_micro
                            );
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
                                    proof.staked_micro
                                ));
                            }
                        } else {
                            tracing::info!(
                                "Storing precomputed proof for {} at checkpoint {}: balance={}, nonce={}, staked={}",
                                &address[..16.min(address.len())],
                                proof.checkpoint_height,
                                proof.balance_micro,
                                proof.nonce,
                                proof.staked_micro
                            );
                        }
                    }
                    account.latest_balance_proof = Some(proof.clone());
                } else {
                    // Account doesn't exist locally - create it from leader's proof
                    tracing::info!(
                        "Creating account {} from leader proof at checkpoint {}: balance={}, nonce={}, staked={}",
                        &address[..16.min(address.len())],
                        proof.checkpoint_height,
                        proof.balance_micro,
                        proof.nonce,
                        proof.staked_micro
                    );
                    let mut new_account = Account::new(address.clone(), proof.checkpoint_height as u64);
                    new_account.balance = proof.balance_micro;
                    new_account.nonce = proof.nonce;
                    new_account.staked = proof.staked_micro;
                    new_account.latest_balance_proof = Some(proof.clone());
                    state.accounts.insert(address.clone(), new_account);
                    if proof.version >= 3 && proof.staked_micro > 0 {
                        rewards_to_sync.push((
                            address.clone(),
                            proof.pending_rewards_micro,
                            proof.staked_at,
                            proof.last_reward_at,
                            proof.claimed_rewards_total_micro,
                            proof.staked_micro
                        ));
                    }
                }
            }
        } // Release state lock
        
        // Second pass: sync RewardsService for accounts with divergence using authoritative v3 values
        if !rewards_to_sync.is_empty() {
            let mut rewards = self.rewards.write().await;
            for (address, pending_rewards, staked_at, last_reward_at, claimed_total, staked_amount) in rewards_to_sync {
                rewards.sync_from_leader_v3(&address, pending_rewards, staked_at, last_reward_at, claimed_total, staked_amount);
            }
        }
    }
}
