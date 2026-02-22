use super::*;

impl NodeState {
    pub async fn store_contract(&self, contract: crate::contracts::ContractState) -> Result<()> {
        let mut state = self.inner.write().await;
        let contract_id = contract.contract_id.clone();
        state.contracts.insert(contract_id.clone(), contract);
        info!("Stored contract {}", contract_id);
        
        let contracts_data: Vec<_> = state.contracts.values().cloned().collect();
        drop(state);
        self.storage.save_contracts(&contracts_data)?;
        Ok(())
    }

    pub async fn get_contract(&self, contract_id: &str) -> Option<crate::contracts::ContractState> {
        let state = self.inner.read().await;
        state.contracts.get(contract_id).cloned()
    }

    pub async fn get_all_contracts(&self) -> Vec<crate::contracts::ContractState> {
        let state = self.inner.read().await;
        state.contracts.values().cloned().collect()
    }

    pub async fn update_contract_state(
        &self,
        contract_id: &str,
        new_state: std::collections::HashMap<String, serde_json::Value>,
        state_hash: String,
        new_height: u64,
    ) -> Result<()> {
        let mut state = self.inner.write().await;
        if let Some(contract) = state.contracts.get_mut(contract_id) {
            contract.state = new_state;
            contract.state_hash = state_hash;
            contract.height = new_height;
            info!("Updated contract {} state at height {}", contract_id, new_height);
            
            let contracts_data: Vec<_> = state.contracts.values().cloned().collect();
            drop(state);
            self.storage.save_contracts(&contracts_data)?;
            Ok(())
        } else {
            anyhow::bail!("Contract {} not found", contract_id)
        }
    }

    /// Reconcile account.staked values with rewards stake positions
    /// This fixes any divergence between the two data stores
    pub async fn reconcile_stakes(&self) -> (usize, Vec<(String, f64, f64)>) {
        let rewards = self.rewards.read().await;
        let stake_positions: Vec<(String, f64)> = rewards
            .get_all_stakes()
            .iter()
            .map(|pos| (pos.staker.clone(), pos.amount))
            .collect();
        drop(rewards);
        
        let mut state = self.inner.write().await;
        let mut reconciled_count = 0;
        let mut changes: Vec<(String, f64, f64)> = Vec::new();
        
        for (staker, rewards_amount) in &stake_positions {
            if let Some(account) = state.accounts.get_mut(staker) {
                let diff = (account.staked - rewards_amount).abs();
                if diff > 0.0001 {
                    info!(
                        "RECONCILE: account.staked for {}: {} -> {} (diff: {:.4})",
                        &staker[..staker.len().min(16)],
                        account.staked,
                        rewards_amount,
                        diff
                    );
                    changes.push((staker.clone(), account.staked, *rewards_amount));
                    account.staked = *rewards_amount;
                    reconciled_count += 1;
                }
            }
        }
        
        if reconciled_count > 0 {
            info!("Reconciled {} account stake values", reconciled_count);
        }
        
        (reconciled_count, changes)
    }

    /// Prune expired pending (unfinalized) transactions from the DAG
    /// Returns the count of transactions that were pruned
    /// This prevents indefinite mempool growth during checkpoint failures
    pub async fn prune_expired_pending_transactions(&self, cutoff_ms: u64) -> usize {
        let mut state = self.inner.write().await;
        
        // Collect hashes of expired unfinalized transactions
        let expired_hashes: Vec<String> = state
            .dag
            .get_all_nodes()
            .into_iter()
            .filter(|node| {
                // Only prune unfinalized transactions
                if node.finalized {
                    return false;
                }
                // Check if transaction has expired based on received_at_ms
                if let Some(received_at) = node.received_at_ms {
                    received_at < cutoff_ms
                } else {
                    // No timestamp - use transaction timestamp as fallback
                    // Convert to milliseconds if needed
                    let tx_ts = node.tx.tx.timestamp;
                    let ts_ms = if tx_ts < 4_000_000_000 {
                        tx_ts * 1000 // Seconds -> milliseconds
                    } else {
                        tx_ts // Already milliseconds
                    };
                    ts_ms < cutoff_ms
                }
            })
            .map(|node| node.hash.clone())
            .collect();

        let count = expired_hashes.len();
        
        // Remove each expired transaction from the DAG
        for hash in expired_hashes {
            if state.dag.remove_node(&hash).is_none() {
                warn!("Failed to remove expired tx {}: not found", &hash[..16.min(hash.len())]);
            }
        }
        
        count
    }
}
