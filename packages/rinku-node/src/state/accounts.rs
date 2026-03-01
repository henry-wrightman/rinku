use super::*;

impl NodeState {
    /// Remove ghost accounts from old deployments.
    /// First zeroes out stale stakes on accounts that are NOT in the current
    /// genesis validator set (they retain staked amounts from a previous deployment).
    /// Then removes any non-allowed accounts with 0 balance + 0 staked.
    pub async fn cleanup_stale_accounts(&self, allowed_addresses: &std::collections::HashSet<String>) {
        let mut state = self.inner.write().await;
        
        let stale_stakers: Vec<String> = state.accounts
            .iter()
            .filter(|(addr, account)| {
                *addr != "faucet"
                    && *addr != "genesis"
                    && !allowed_addresses.contains(*addr)
                    && account.staked > 0
            })
            .map(|(addr, _)| addr.clone())
            .collect();
        
        for addr in &stale_stakers {
            if let Some(account) = state.accounts.get_mut(addr) {
                info!(
                    "Zeroing stale stake on non-validator account {}: {:.4} RKU",
                    &addr[..16.min(addr.len())], account.staked
                );
                account.staked = 0;
            }
        }
        if !stale_stakers.is_empty() {
            info!("Zeroed stale stakes on {} ghost validator account(s)", stale_stakers.len());
        }
        
        let stale: Vec<String> = state.accounts
            .iter()
            .filter(|(addr, account)| {
                *addr != "faucet"
                    && *addr != "genesis"
                    && !allowed_addresses.contains(*addr)
                    && account.balance == 0
                    && account.staked == 0
            })
            .map(|(addr, _)| addr.clone())
            .collect();
        
        if !stale.is_empty() {
            for addr in &stale {
                state.accounts.remove(addr);
            }
            info!("Cleaned up {} ghost account(s) from old snapshot", stale.len());
        }
    }
    
    /// Sync all stakes from RewardsService to account.staked fields
    /// Must be called AFTER replace_validators_with_genesis to avoid ghost accounts
    pub async fn sync_stakes_to_accounts(&self) {
        let rewards = self.rewards.read().await;
        let stakes: Vec<(String, u64, u64)> = rewards.get_all_stakes()
            .iter()
            .map(|s| (s.staker.clone(), s.amount, s.staked_at / 1000))
            .collect();
        drop(rewards);
        
        if stakes.is_empty() {
            return;
        }
        
        let mut state = self.inner.write().await;
        let mut synced = 0;
        for (address, amount, staked_at) in stakes {
            if let Some(account) = state.accounts.get_mut(&address) {
                account.staked = amount;
            } else {
                let mut account = Account::new(address.clone(), staked_at);
                account.staked = amount;
                state.accounts.insert(address, account);
            }
            synced += 1;
        }
        info!("Synced {} stakes to account state", synced);
    }
    
    /// Recalculate DAG node weights based on current account state
    /// This is needed on startup to fix weights for transactions that were added
    /// before their sender's stake was synced to account.staked
    pub(crate) async fn recalculate_dag_weights(&self) {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let mut state = self.inner.write().await;
        
        // Get all account weights first
        let account_weights: std::collections::HashMap<String, f64> = state.accounts
            .iter()
            .map(|(addr, acc)| (addr.clone(), calculate_account_weight(acc, now_secs)))
            .collect();
        
        // Update DAG node weights
        let mut updated = 0;
        for node in state.dag.nodes_mut() {
            let sender = &node.tx.tx.from;
            if let Some(&new_weight) = account_weights.get(sender) {
                if (node.weight - new_weight).abs() > 0.01 {
                    node.weight = new_weight;
                    updated += 1;
                }
            }
        }
        
        if updated > 0 {
            info!("Recalculated {} DAG node weights based on current account state", updated);
        }
    }

    pub async fn save_snapshot(&self) -> Result<()> {
        // Run memory cleanup before saving
        self.cleanup_old_data().await;
        
        let state = self.inner.read().await;
        
        // Log memory metrics for monitoring
        info!(
            "Memory metrics: DAG nodes={}, accounts={}, validators={}, checkpoints={}, contracts={}",
            state.dag.node_count(),
            state.accounts.len(),
            state.validators.len(),
            state.checkpoints.len(),
            state.contracts.len()
        );
        // Create DagSnapshotEntry for each node, preserving parent references
        let dag_entries: Vec<crate::storage::DagSnapshotEntry> = state.dag.nodes()
            .map(|node| crate::storage::DagSnapshotEntry {
                tx: node.tx.clone(),
                parents: node.parents.clone(),
                finalized: node.finalized,
                checkpoint_height: node.checkpoint_height,
            })
            .collect();
        self.storage.save_snapshot(
            &state.accounts,
            &state.validators,
            &state.checkpoints,
            state.current_gas_price,
            state.total_supply,
            state.genesis_time,
            &dag_entries,
            state.total_transactions,
        )?;

        // Save weight trie (trust scores)
        if let Some(ref weight_trie) = state.weight_trie {
            let weights = weight_trie.all_weights().clone();
            if !weights.is_empty() {
                self.storage.save_weights(&weights)?;
                info!("Saved {} transaction weight scores to storage", weights.len());
            }
        }
        drop(state);

        // Also save rewards/staking state
        let rewards = self.rewards.read().await;
        let rewards_snapshot = rewards.to_json();
        drop(rewards);
        self.storage.save_rewards(&rewards_snapshot)?;

        // Save emission state
        let emission = self.emission.read().await;
        let emission_snapshot = emission.to_json();
        drop(emission);
        self.storage.save_emission(&emission_snapshot)?;

        Ok(())
    }
    
    /// Periodic cleanup to prevent memory leaks
    async fn cleanup_old_data(&self) {
        const MAX_CHECKPOINTS: usize = 500;  // Keep last ~2 hours of checkpoints
        const MAX_ACCOUNTS: usize = 50000;   // Cap on accounts
        
        let mut state = self.inner.write().await;
        
        // Prune old checkpoints (keep most recent MAX_CHECKPOINTS)
        if state.checkpoints.len() > MAX_CHECKPOINTS {
            let to_remove = state.checkpoints.len() - MAX_CHECKPOINTS;
            state.checkpoints.drain(0..to_remove);
            info!("Pruned {} old checkpoints, {} remaining", to_remove, state.checkpoints.len());
        }
        
        // Prune zero-balance accounts with no stake (keep accounts under limit)
        if state.accounts.len() > MAX_ACCOUNTS {
            let mut removable: Vec<String> = state.accounts
                .iter()
                .filter(|(_, a)| a.balance == 0 && a.staked == 0)
                .map(|(k, _)| k.clone())
                .collect();
            
            // Remove oldest first (by first_seen)
            removable.sort_by(|a, b| {
                let a_time = state.accounts.get(a).map(|acc| acc.first_seen).unwrap_or(0);
                let b_time = state.accounts.get(b).map(|acc| acc.first_seen).unwrap_or(0);
                a_time.cmp(&b_time)
            });
            
            let to_remove = (state.accounts.len() - MAX_ACCOUNTS).min(removable.len());
            for key in removable.into_iter().take(to_remove) {
                state.accounts.remove(&key);
            }
            
            if to_remove > 0 {
                info!("Pruned {} inactive accounts, {} remaining", to_remove, state.accounts.len());
            }
        }
        
        drop(state);
        
        // Prune rewards data
        let mut rewards = self.rewards.write().await;
        let pruned = rewards.prune_old_data();
        if pruned > 0 {
            info!("Pruned {} expired witness entries", pruned);
        }
    }

    /// Update account's staked amount (syncs with RewardsService)
    pub async fn update_account_staked(&self, address: &str, staked_amount: u64, staked_at: Option<u64>) {
        let mut state = self.inner.write().await;
        if let Some(account) = state.accounts.get_mut(address) {
            account.staked = staked_amount;
            if let Some(ts) = staked_at {
                account.first_seen = ts;
            }
        } else {
            // Create account if doesn't exist
            let now = staked_at.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            });
            let mut account = Account::new(address.to_string(), now);
            account.staked = staked_amount;
            state.accounts.insert(address.to_string(), account);
        }
    }

    pub async fn apply_contract_transfer_effects(&self, effects: &[crate::contracts::TransferEffect]) -> anyhow::Result<()> {
        if effects.is_empty() {
            return Ok(());
        }
        let mut state = self.inner.write().await;
        for effect in effects {
            let amount_micro = rinku_core::types::to_micro_units(effect.amount);
            let from_balance = match state.accounts.get(&effect.from) {
                Some(acct) => acct.balance,
                None => {
                    tracing::error!(
                        "Contract transfer rejected: sender {} does not exist in state",
                        &effect.from[..16.min(effect.from.len())]
                    );
                    return Err(anyhow::anyhow!(
                        "Contract transfer sender {} not found in state",
                        effect.from
                    ));
                }
            };

            if from_balance < amount_micro {
                tracing::error!(
                    "Contract transfer rejected: {} has {} but needs {}",
                    &effect.from[..16.min(effect.from.len())],
                    from_balance,
                    amount_micro
                );
                return Err(anyhow::anyhow!(
                    "Contract transfer insufficient balance: {} has {} but needs {}",
                    effect.from, from_balance, amount_micro
                ));
            }

            if let Some(from_acct) = state.accounts.get_mut(&effect.from) {
                from_acct.balance -= amount_micro;
            }

            let to_acct = state
                .accounts
                .entry(effect.to.clone())
                .or_insert_with(|| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    Account::new(effect.to.clone(), now)
                });
            to_acct.balance += amount_micro;
            tracing::debug!(
                "Contract transfer applied: {} -> {} ({} micro-RKU)",
                &effect.from[..16.min(effect.from.len())],
                &effect.to[..16.min(effect.to.len())],
                amount_micro
            );
        }
        Ok(())
    }

    pub async fn get_or_create_account(&self, address: &str) -> Account {
        let mut state = self.inner.write().await;
        if let Some(account) = state.accounts.get(address) {
            account.clone()
        } else {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let account = Account::new(address.to_string(), now);
            state.accounts.insert(address.to_string(), account.clone());
            account
        }
    }

    pub async fn get_account(&self, address: &str) -> Option<Account> {
        let state = self.inner.read().await;
        state.accounts.get(address).cloned()
    }

    pub async fn get_account_nonce(&self, address: &str) -> u64 {
        let state = self.inner.read().await;
        state.accounts.get(address).map(|a| a.nonce).unwrap_or(0)
    }

    /// Sync account nonce from peer during delta sync.
    /// Only updates if the peer's nonce is greater (prevents regression).
    pub async fn sync_account_nonce(&self, address: &str, peer_nonce: u64) {
        let mut state = self.inner.write().await;
        if let Some(account) = state.accounts.get_mut(address) {
            if peer_nonce > account.nonce {
                tracing::debug!(
                    "Syncing nonce for {}: {} -> {}",
                    address, account.nonce, peer_nonce
                );
                account.nonce = peer_nonce;
            }
        } else {
            // Create account if it doesn't exist with the peer's nonce
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let mut account = Account::new(address.to_string(), now);
            account.nonce = peer_nonce;
            state.accounts.insert(address.to_string(), account);
            tracing::debug!("Created account {} with nonce {}", address, peer_nonce);
        }
    }

    /// Merge accounts pushed from a peer.
    /// AUTHORITATIVE MERGE: Updates accounts when peer has higher/equal nonce
    /// This fixes balance divergence where nonces match but balances differ
    /// Rejects stale accounts (zero balance, zero nonce) to prevent ghost contamination
    /// Returns (accounts_added, accounts_updated, accounts_balance_fixed)
    pub async fn merge_accounts_from_peer(&self, accounts: HashMap<String, Account>) -> (usize, usize, usize) {
        let mut state = self.inner.write().await;
        let mut added = 0;
        let mut updated = 0;
        let mut balance_fixed = 0;
        let mut rejected_stale = 0;
        
        for (fingerprint, peer_account) in accounts {
            if fingerprint == "faucet" {
                continue;
            }
            let is_stale = peer_account.nonce == 0 && peer_account.balance == 0;
            let is_ghost_validator = peer_account.staked > 0 
                && peer_account.nonce == 0 
                && !state.validators.contains_key(&fingerprint);
            if (is_stale || is_ghost_validator) && !state.accounts.contains_key(&fingerprint) {
                if is_ghost_validator {
                    info!(
                        "Rejecting ghost validator account {} from merge (staked={:.4}, nonce=0, not in validator set)",
                        &fingerprint[..fingerprint.len().min(16)], peer_account.staked
                    );
                }
                rejected_stale += 1;
                continue;
            }
            if let Some(local_account) = state.accounts.get_mut(&fingerprint) {
                // Account exists locally - AUTHORITATIVE SYNC
                if peer_account.nonce > local_account.nonce {
                    // Peer has more transactions - take their state
                    *local_account = peer_account;
                    updated += 1;
                } else if peer_account.nonce == local_account.nonce {
                    // Same nonce - check for balance/stake divergence
                    let balance_diff = peer_account.balance.abs_diff(local_account.balance);
                    let stake_diff = peer_account.staked.abs_diff(local_account.staked);
                    if balance_diff > 0 || stake_diff > 0 {
                        // Accept peer's state to fix divergence
                        // Peer is authoritative since they initiated the sync
                        info!(
                            "Balance fix (merge) for {}: local={:.6} peer={:.6}",
                            &fingerprint[..12.min(fingerprint.len())], local_account.balance, peer_account.balance
                        );
                        *local_account = peer_account;
                        balance_fixed += 1;
                    }
                }
                // If local has higher nonce, keep local
            } else {
                // Account doesn't exist locally - add it
                state.accounts.insert(fingerprint, peer_account);
                added += 1;
            }
        }
        
        if added > 0 || updated > 0 || balance_fixed > 0 || rejected_stale > 0 {
            info!(
                "Merged accounts from peer: {} added, {} updated, {} balance-fixed, {} rejected-stale, {} total",
                added, updated, balance_fixed, rejected_stale, state.accounts.len()
            );
        }
        
        (added, updated, balance_fixed)
    }

    /// Get all accounts with fingerprints (for pushing to peer)
    pub async fn get_all_accounts_map(&self) -> HashMap<String, Account> {
        let state = self.inner.read().await;
        state.accounts.clone()
    }

    /// Get account count
    pub async fn get_account_count(&self) -> usize {
        let state = self.inner.read().await;
        state.accounts.len()
    }

    pub async fn get_all_accounts(&self) -> Vec<Account> {
        let state = self.inner.read().await;
        state.accounts.values().cloned().collect()
    }
}
