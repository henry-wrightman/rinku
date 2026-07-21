use super::*;

impl NodeState {
    /// Post-genesis-seed stake reconciliation.
    ///
    /// Preserves all real user / delegator stakes. Only clears *redeploy ghost
    /// validators* (prior `GENESIS_VALIDATORS` leftovers) and empty accounts.
    /// Never confiscates stake without crediting `account.balance`.
    pub async fn reconcile_stakes_after_genesis_replace(
        &self,
        genesis_addresses: &std::collections::HashSet<String>,
    ) {
        self.cleanup_redeploy_ghost_validators(genesis_addresses)
            .await;
        self.cleanup_empty_ghost_accounts(genesis_addresses).await;
    }

    /// Legacy entry point — safe wrapper (does **not** zero user stakes).
    pub async fn cleanup_stale_accounts(
        &self,
        allowed_addresses: &std::collections::HashSet<String>,
    ) {
        self.reconcile_stakes_after_genesis_replace(allowed_addresses)
            .await;
    }

    /// Clear leftover genesis-validator accounts from a previous deploy.
    ///
    /// A redeploy ghost is an account that is:
    /// - not in the current genesis validator set
    /// - staked at exactly `GENESIS_VALIDATOR_STAKE`
    /// - `nonce == 0` (never sent a user tx — real stakers always bump nonce)
    ///
    /// Stake is credited back to balance before clearing. Matching rewards
    /// entries are removed. Arbitrary user stakes are never touched.
    pub async fn cleanup_redeploy_ghost_validators(
        &self,
        genesis_addresses: &std::collections::HashSet<String>,
    ) {
        use crate::validator_identity::GENESIS_VALIDATOR_STAKE;

        let ghosts: Vec<(String, u64)> = {
            let state = self.inner.read().await;
            state
                .accounts
                .iter()
                .filter(|(addr, account)| {
                    *addr != "faucet"
                        && *addr != "genesis"
                        && !genesis_addresses.contains(*addr)
                        && account.staked == GENESIS_VALIDATOR_STAKE
                        && account.nonce == 0
                })
                .map(|(addr, account)| (addr.clone(), account.staked))
                .collect()
        };

        if !ghosts.is_empty() {
            let mut state = self.inner.write().await;
            let mut credited = Vec::with_capacity(ghosts.len());
            for (addr, staked) in &ghosts {
                if let Some(account) = state.accounts.get_mut(addr) {
                    account.balance = account.balance.saturating_add(*staked);
                    account.staked = 0;
                    info!(
                        "Credited redeploy-ghost validator stake to balance for {}: +{} µRKU (stake cleared)",
                        &addr[..16.min(addr.len())],
                        staked
                    );
                    credited.push(addr.clone());
                }
            }
            if !credited.is_empty() {
                state.update_state_trie_accounts(&credited);
                info!(
                    "Cleared {} redeploy-ghost validator account stake(s) with balance credit",
                    credited.len()
                );
            }
        }

        // Scrub rewards.stakes for the same ghost heuristic (and rewards-only
        // orphans that look like old genesis validators with no live user account).
        {
            let account_meta: std::collections::HashMap<String, (u64, u64)> = {
                let state = self.inner.read().await;
                state
                    .accounts
                    .iter()
                    .map(|(addr, acc)| (addr.clone(), (acc.staked, acc.nonce)))
                    .collect()
            };
            let mut rewards = self.rewards.write().await;
            let reward_stakers: Vec<(String, u64)> = rewards
                .get_all_stakes()
                .iter()
                .map(|s| (s.staker.clone(), s.amount))
                .collect();
            let mut removed = 0u32;
            for (staker, amount) in reward_stakers {
                if genesis_addresses.contains(&staker) {
                    continue;
                }
                if amount != GENESIS_VALIDATOR_STAKE {
                    continue;
                }
                let is_user_stake = account_meta.get(&staker).is_some_and(|(staked, nonce)| {
                    *nonce > 0 || (*staked > 0 && *staked != GENESIS_VALIDATOR_STAKE)
                });
                if is_user_stake {
                    continue;
                }
                // Ghost: missing account, or nonce==0 with stake already cleared / still genesis-sized.
                rewards.remove_stake(&staker);
                removed += 1;
            }
            if removed > 0 {
                info!(
                    "Removed {} redeploy-ghost stake(s) from rewards service",
                    removed
                );
            }
        }
    }

    /// Remove non-system accounts with zero balance and zero stake.
    pub async fn cleanup_empty_ghost_accounts(
        &self,
        allowed_addresses: &std::collections::HashSet<String>,
    ) {
        let mut state = self.inner.write().await;

        let stale: Vec<String> = state
            .accounts
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
            info!(
                "Cleaned up {} empty ghost account(s) from old snapshot",
                stale.len()
            );
        }
    }

    /// Push canonical `account.staked` into `RewardsService` for every known
    /// staker — including zeros, which remove orphaned rewards entries.
    ///
    /// Call after checkpoint apply so account state and rewards.stakes converge.
    pub async fn reconcile_rewards_stakes_from_accounts(&self) {
        let account_stakes: std::collections::HashMap<String, u64> = {
            let state = self.inner.read().await;
            state
                .accounts
                .iter()
                .map(|(addr, acc)| (addr.clone(), acc.staked))
                .collect()
        };

        let mut rewards = self.rewards.write().await;
        let reward_stakers: Vec<String> = rewards
            .get_all_stakes()
            .iter()
            .map(|s| s.staker.clone())
            .collect();

        for staker in &reward_stakers {
            let canonical = account_stakes.get(staker).copied().unwrap_or(0);
            rewards.sync_stake_amount(staker, canonical);
        }
        for (addr, amount) in &account_stakes {
            if *amount > 0 {
                rewards.sync_stake_amount(addr, *amount);
            }
        }
    }

    /// Sync all stakes from RewardsService to account.staked fields
    /// Must be called AFTER replace_validators_with_genesis to avoid ghost accounts
    pub async fn sync_stakes_to_accounts(&self) {
        let rewards = self.rewards.read().await;
        let stakes: Vec<(String, u64, u64)> = rewards
            .get_all_stakes()
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
        let account_weights: std::collections::HashMap<String, f64> = state
            .accounts
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
            info!(
                "Recalculated {} DAG node weights based on current account state",
                updated
            );
        }
    }

    pub async fn save_snapshot(&self) -> Result<()> {
        self.cleanup_old_data().await;

        let state = self.inner.read().await;

        info!(
            "Memory metrics: DAG nodes={}, accounts={}, validators={}, checkpoints={}, contracts={}",
            state.dag.node_count(),
            state.accounts.len(),
            state.validators.len(),
            state.checkpoints.len(),
            state.contracts.len()
        );

        let accounts = state.accounts.clone();
        let validators = state.validators.clone();
        let checkpoints = state.checkpoints.clone();
        let gas_price = state.current_gas_price;
        let total_supply = state.total_supply;
        let genesis_time = state.genesis_time;
        let dag_node_count = state.dag.node_count() as u64;
        let total_transactions = std::cmp::max(state.total_transactions, dag_node_count);
        let dag_entries: Vec<crate::storage::DagSnapshotEntry> = state
            .dag
            .nodes()
            .map(|node| crate::storage::DagSnapshotEntry {
                tx: node.tx.clone(),
                parents: node.parents.clone(),
                finalized: node.finalized,
                checkpoint_height: node.checkpoint_height,
                fast_path_cert: node.fast_path_cert.clone(),
            })
            .collect();
        let weights = state
            .weight_trie
            .as_ref()
            .map(|wt| wt.all_weights().clone());
        drop(state);

        let rewards = self.rewards.read().await;
        let rewards_snapshot = rewards.to_json();
        drop(rewards);

        let emission = self.emission.read().await;
        let emission_snapshot = emission.to_json();
        drop(emission);

        let storage = self.storage.clone();
        crate::storage::blocking_io(move || {
            storage.save_snapshot(
                &accounts,
                &validators,
                &checkpoints,
                gas_price,
                total_supply,
                genesis_time,
                &dag_entries,
                total_transactions,
            )?;

            if let Some(ref w) = weights {
                if !w.is_empty() {
                    storage.save_weights(w)?;
                    info!("Saved {} transaction weight scores to storage", w.len());
                }
            }

            storage.save_rewards(&rewards_snapshot)?;
            storage.save_emission(&emission_snapshot)?;

            Ok(())
        })
        .await?;

        Ok(())
    }

    /// Periodic cleanup to prevent memory leaks
    async fn cleanup_old_data(&self) {
        const MAX_CHECKPOINTS: usize = 500; // Keep last ~2 hours of checkpoints
        const MAX_ACCOUNTS: usize = 50000; // Cap on accounts

        let mut state = self.inner.write().await;

        // Prune old checkpoints (keep most recent MAX_CHECKPOINTS)
        if state.checkpoints.len() > MAX_CHECKPOINTS {
            let to_remove = state.checkpoints.len() - MAX_CHECKPOINTS;
            state.checkpoints.drain(0..to_remove);
            info!(
                "Pruned {} old checkpoints, {} remaining",
                to_remove,
                state.checkpoints.len()
            );
        }

        // Prune zero-balance accounts with no stake (keep accounts under limit)
        if state.accounts.len() > MAX_ACCOUNTS {
            let mut removable: Vec<String> = state
                .accounts
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
                info!(
                    "Pruned {} inactive accounts, {} remaining",
                    to_remove,
                    state.accounts.len()
                );
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
    pub async fn update_account_staked(
        &self,
        address: &str,
        staked_amount: u64,
        staked_at: Option<u64>,
    ) {
        let mut state = self.inner.write().await;
        if let Some(account) = state.accounts.get_mut(address) {
            account.staked = staked_amount;
            if let Some(ts) = staked_at {
                account.first_seen = ts;
            }
        } else {
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
        state.update_state_trie_accounts(&[address.to_string()]);
    }

    pub async fn apply_contract_transfer_effects(
        &self,
        effects: &[crate::contracts::TransferEffect],
    ) -> anyhow::Result<()> {
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
                    effect.from,
                    from_balance,
                    amount_micro
                ));
            }

            if let Some(from_acct) = state.accounts.get_mut(&effect.from) {
                from_acct.balance -= amount_micro;
            }

            let to_acct = state.accounts.entry(effect.to.clone()).or_insert_with(|| {
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
        let changed: Vec<String> = effects
            .iter()
            .flat_map(|e| vec![e.from.clone(), e.to.clone()])
            .collect();
        state.update_state_trie_accounts(&changed);
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

    pub async fn get_effective_nonce_for(&self, sender: &str) -> u64 {
        let state = self.inner.read().await;
        Self::get_effective_nonce(&state, sender)
    }

    pub async fn get_effective_balance_for(&self, sender: &str) -> u64 {
        let state = self.inner.read().await;
        Self::get_effective_balance(&state, sender)
    }

    /// Sync account nonce from peer during delta sync.
    /// Only updates if the peer's nonce is greater (prevents regression).
    pub async fn sync_account_nonce(&self, address: &str, peer_nonce: u64) {
        let mut state = self.inner.write().await;
        if let Some(account) = state.accounts.get_mut(address) {
            if peer_nonce > account.nonce {
                tracing::debug!(
                    "Syncing nonce for {}: {} -> {}",
                    address,
                    account.nonce,
                    peer_nonce
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
    pub async fn merge_accounts_from_peer(
        &self,
        accounts: HashMap<String, Account>,
    ) -> (usize, usize, usize) {
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
                            &fingerprint[..12.min(fingerprint.len())],
                            local_account.balance,
                            peer_account.balance
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

    pub async fn get_all_accounts_with_effective(&self) -> Vec<(Account, u64, u64)> {
        let state = self.inner.read().await;
        state
            .accounts
            .values()
            .map(|a| {
                let eff_nonce = Self::get_effective_nonce(&state, &a.address);
                let eff_balance = Self::get_effective_balance(&state, &a.address);
                (a.clone(), eff_nonce, eff_balance)
            })
            .collect()
    }
}

#[cfg(test)]
mod stake_cleanup_tests {
    use super::*;
    use crate::config::NodeConfig;
    use crate::validator_identity::GENESIS_VALIDATOR_STAKE;
    use rinku_core::types::to_micro_units;

    fn acct(addr: &str, bal: u64, nonce: u64, staked: u64) -> Account {
        Account {
            address: addr.to_string(),
            balance: bal,
            nonce,
            first_seen: 0,
            staked,
            unbonding: 0,
            unbonding_release: None,
            latest_balance_proof: None,
            partition_violations: 0,
            reputation_penalty: 0.0,
            penalty_decay_checkpoint: None,
            partition_budget: None,
            partition_budget_spent: 0,
            ecdsa_public_key: None,
        }
    }

    async fn fresh_state() -> (tempfile::TempDir, NodeState) {
        let dir = tempfile::tempdir().unwrap();
        let config = NodeConfig {
            data_dir: dir.path().to_string_lossy().to_string(),
            is_genesis_node: true,
            ..NodeConfig::default()
        };
        let state = NodeState::new(config).await.expect("NodeState");
        (dir, state)
    }

    async fn seed(state: &NodeState, accounts: &[Account]) {
        let mut inner = state.inner.write().await;
        inner.accounts.clear();
        for a in accounts {
            inner.accounts.insert(a.address.clone(), a.clone());
        }
        inner.state_trie = StateInner::build_state_trie_from_accounts(&inner.accounts);
    }

    /// Regression: user stake must survive genesis-set cleanup that previously
    /// zeroed every non-GENESIS_VALIDATORS account.staked on restart.
    #[tokio::test]
    async fn genesis_cleanup_preserves_user_stake_and_balance() {
        let (_dir, state) = fresh_state().await;
        let user = "923cc639b27a6d07cd1f2879166b8a53e01bb8ef";
        let val_a = "01332180dd6879ed3e4fa853f5c15dbd0b903e97";
        let user_stake = to_micro_units(199.0);
        let user_bal = to_micro_units(101.0);

        seed(
            &state,
            &[
                acct(val_a, 0, 0, GENESIS_VALIDATOR_STAKE),
                acct(user, user_bal, 2, user_stake),
            ],
        )
        .await;
        {
            let mut rewards = state.rewards.write().await;
            rewards.stake(val_a, GENESIS_VALIDATOR_STAKE, "").unwrap();
            rewards.stake(user, user_stake, "user-stake-tx").unwrap();
            rewards.add_pending_reward(user, to_micro_units(11.6));
        }

        let mut genesis = std::collections::HashSet::new();
        genesis.insert(val_a.to_string());

        state.reconcile_stakes_after_genesis_replace(&genesis).await;

        let (staked, balance, nonce) = {
            let inner = state.inner.read().await;
            let a = inner.accounts.get(user).expect("user account");
            (a.staked, a.balance, a.nonce)
        };
        assert_eq!(staked, user_stake, "user stake must not be confiscated");
        assert_eq!(balance, user_bal, "user balance must be unchanged");
        assert_eq!(nonce, 2);

        let rewards = state.rewards.read().await;
        assert_eq!(
            rewards.get_stake(user).map(|s| s.amount),
            Some(user_stake),
            "rewards.stakes for user must survive genesis cleanup"
        );
        assert!(
            (rewards.get_pending_rewards(user) as i64 - to_micro_units(11.6) as i64).abs() < 2,
            "pending rewards must survive"
        );
        assert_eq!(
            rewards.get_stake(val_a).map(|s| s.amount),
            Some(GENESIS_VALIDATOR_STAKE)
        );
    }

    /// Redeploy ghost validators (exact genesis stake, nonce 0, not in set)
    /// are cleared, but principal is credited back to balance.
    #[tokio::test]
    async fn redeploy_ghost_validator_stake_credited_not_burned() {
        let (_dir, state) = fresh_state().await;
        let live = "livevalidator000000000000000000000000001";
        let ghost = "oldvalidator000000000000000000000000001";

        seed(
            &state,
            &[
                acct(live, 0, 0, GENESIS_VALIDATOR_STAKE),
                acct(ghost, 0, 0, GENESIS_VALIDATOR_STAKE),
            ],
        )
        .await;
        {
            let mut rewards = state.rewards.write().await;
            let _ = rewards.stake(live, GENESIS_VALIDATOR_STAKE, "");
            let _ = rewards.stake(ghost, GENESIS_VALIDATOR_STAKE, "");
        }

        let mut genesis = std::collections::HashSet::new();
        genesis.insert(live.to_string());
        state.reconcile_stakes_after_genesis_replace(&genesis).await;

        let inner = state.inner.read().await;
        let g = inner
            .accounts
            .get(ghost)
            .expect("ghost account kept (has balance now)");
        assert_eq!(g.staked, 0, "ghost stake cleared");
        assert_eq!(
            g.balance, GENESIS_VALIDATOR_STAKE,
            "ghost stake must be credited to balance, not burned"
        );
        let l = inner.accounts.get(live).unwrap();
        assert_eq!(l.staked, GENESIS_VALIDATOR_STAKE);
        drop(inner);

        let rewards = state.rewards.read().await;
        assert!(rewards.get_stake(ghost).is_none());
        assert_eq!(
            rewards.get_stake(live).map(|s| s.amount),
            Some(GENESIS_VALIDATOR_STAKE)
        );
    }

    /// A wallet that happens to hold GENESIS_VALIDATOR_STAKE but has nonce > 0
    /// is a real user, not a redeploy ghost — must be preserved.
    #[tokio::test]
    async fn large_user_stake_with_nonce_is_not_treated_as_ghost() {
        let (_dir, state) = fresh_state().await;
        let user = "bigstaker00000000000000000000000000001";
        seed(
            &state,
            &[acct(user, to_micro_units(1.0), 1, GENESIS_VALIDATOR_STAKE)],
        )
        .await;
        {
            let mut rewards = state.rewards.write().await;
            let _ = rewards.stake(user, GENESIS_VALIDATOR_STAKE, "user-tx");
        }

        let genesis = std::collections::HashSet::new(); // empty — user not a genesis val
        state.reconcile_stakes_after_genesis_replace(&genesis).await;

        let inner = state.inner.read().await;
        let a = inner.accounts.get(user).unwrap();
        assert_eq!(a.staked, GENESIS_VALIDATOR_STAKE);
        assert_eq!(a.balance, to_micro_units(1.0));
        drop(inner);
        assert!(state.rewards.read().await.get_stake(user).is_some());
    }

    /// account.staked=0 with a leftover rewards.stakes entry must converge to
    /// remove the orphan (the live val-1/val-2 199 RKU ghost symptom).
    #[tokio::test]
    async fn reconcile_removes_orphaned_rewards_stake_when_account_staked_zero() {
        let (_dir, state) = fresh_state().await;
        let user = "923cc639b27a6d07cd1f2879166b8a53e01bb8ef";
        seed(&state, &[acct(user, to_micro_units(101.0), 2, 0)]).await;
        {
            let mut rewards = state.rewards.write().await;
            rewards
                .stake(user, to_micro_units(199.0), "orphan-tx")
                .unwrap();
        }
        assert_eq!(
            state.rewards.read().await.get_total_staked(),
            to_micro_units(199.0)
        );

        state.reconcile_rewards_stakes_from_accounts().await;

        assert!(
            state.rewards.read().await.get_stake(user).is_none(),
            "orphan rewards.stakes must be removed when account.staked=0"
        );
        assert_eq!(state.rewards.read().await.get_total_staked(), 0);
    }

    #[tokio::test]
    async fn reconcile_creates_rewards_entry_from_account_stake() {
        let (_dir, state) = fresh_state().await;
        let user = "newstaker0000000000000000000000000001";
        let amount = to_micro_units(150.0);
        seed(&state, &[acct(user, 0, 1, amount)]).await;

        state.reconcile_rewards_stakes_from_accounts().await;

        assert_eq!(
            state.rewards.read().await.get_stake(user).map(|s| s.amount),
            Some(amount)
        );
    }

    #[tokio::test]
    async fn empty_zero_balance_accounts_are_removed_user_stakers_are_not() {
        let (_dir, state) = fresh_state().await;
        let empty = "emptyacct0000000000000000000000000001";
        let user = "useracct00000000000000000000000000001";
        seed(
            &state,
            &[
                acct(empty, 0, 0, 0),
                acct(user, to_micro_units(10.0), 1, to_micro_units(100.0)),
            ],
        )
        .await;
        {
            let mut rewards = state.rewards.write().await;
            let _ = rewards.stake(user, to_micro_units(100.0), "tx");
        }

        let genesis = std::collections::HashSet::new();
        state.reconcile_stakes_after_genesis_replace(&genesis).await;

        let inner = state.inner.read().await;
        assert!(inner.accounts.get(empty).is_none());
        assert!(inner.accounts.get(user).is_some());
        assert_eq!(
            inner.accounts.get(user).unwrap().staked,
            to_micro_units(100.0)
        );
    }

    /// End-to-end conservation: after simulated genesis restart cleanup,
    /// user (balance + staked) is unchanged and equals pre-cleanup total.
    #[tokio::test]
    async fn user_funds_conserved_across_simulated_genesis_restart_cleanup() {
        let (_dir, state) = fresh_state().await;
        let user = "923cc639b27a6d07cd1f2879166b8a53e01bb8ef";
        let v1 = "val10000000000000000000000000000000001";
        let v2 = "val20000000000000000000000000000000002";
        let v3 = "val30000000000000000000000000000000003";
        let user_stake = to_micro_units(199.0);
        let user_bal = to_micro_units(101.2324536);

        seed(
            &state,
            &[
                acct(v1, 0, 0, GENESIS_VALIDATOR_STAKE),
                acct(v2, 0, 0, GENESIS_VALIDATOR_STAKE),
                acct(v3, 0, 0, GENESIS_VALIDATOR_STAKE),
                acct(user, user_bal, 2, user_stake),
            ],
        )
        .await;
        {
            let mut rewards = state.rewards.write().await;
            for v in [&v1, &v2, &v3] {
                let _ = rewards.stake(v, GENESIS_VALIDATOR_STAKE, "");
            }
            rewards.stake(user, user_stake, "stake-tx").unwrap();
        }

        let before_total = user_bal + user_stake;
        let mut genesis = std::collections::HashSet::new();
        genesis.insert(v1.to_string());
        genesis.insert(v2.to_string());
        genesis.insert(v3.to_string());

        // Simulate the exact startup path: register genesis stakes (no wipe),
        // then reconcile.
        {
            let mut rewards = state.rewards.write().await;
            for v in [&v1, &v2, &v3] {
                if rewards.get_stake(v).is_none() {
                    let _ = rewards.stake(v, GENESIS_VALIDATOR_STAKE, "");
                }
            }
        }
        state.reconcile_stakes_after_genesis_replace(&genesis).await;
        state.sync_stakes_to_accounts().await;

        let (bal, staked) = {
            let inner = state.inner.read().await;
            let a = inner.accounts.get(user).unwrap();
            (a.balance, a.staked)
        };
        assert_eq!(bal + staked, before_total, "user funds must be conserved");
        assert_eq!(staked, user_stake);
        assert_eq!(bal, user_bal);

        let rewards = state.rewards.read().await;
        assert_eq!(rewards.get_stake(user).unwrap().amount, user_stake);
        assert_eq!(
            rewards.get_total_staked(),
            GENESIS_VALIDATOR_STAKE * 3 + user_stake
        );
        assert_eq!(rewards.get_active_validators().len(), 4);
    }
}
