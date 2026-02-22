use super::*;

impl NodeState {
    pub async fn get_validators(&self) -> Vec<Validator> {
        let state = self.inner.read().await;
        state.validators.values().cloned().collect()
    }
    
    /// Check if an address is a registered validator
    pub async fn is_validator(&self, address: &str) -> bool {
        let state = self.inner.read().await;
        state.validators.contains_key(address)
    }
    
    /// Get the stake amount for a validator
    pub async fn get_validator_stake(&self, address: &str) -> Option<f64> {
        let state = self.inner.read().await;
        state.validators.get(address).map(|v| v.stake)
    }
    
    /// Get total validator stake for fast-path quorum calculation
    pub async fn get_total_validator_stake(&self) -> f64 {
        let state = self.inner.read().await;
        state.validators.values().map(|v| v.stake).sum()
    }
    
    /// Get the validators as a HashMap for syncing to the ValidatorIdentityService
    pub async fn get_validators_map(&self) -> std::collections::HashMap<String, Validator> {
        let state = self.inner.read().await;
        state.validators.clone()
    }
    
    /// Replace the validator registry with genesis validators for consensus verification.
    /// When `is_genesis_node` is true, also creates validator accounts with stake (genesis
    /// is the authoritative source of accounts). Validator nodes only update the registry
    /// and trust that accounts were inherited via PRE-SYNC from the genesis node.
    pub async fn replace_validators_with_genesis(&self, genesis_validators: &[(String, Vec<u8>)], is_genesis_node: bool) {
        use crate::validator_identity::MIN_VALIDATOR_STAKE;
        
        let mut state = self.inner.write().await;
        let old_count = state.validators.len();
        
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        let mut new_validators = std::collections::HashMap::new();
        
        for (address, bls_public_key) in genesis_validators {
            let validator = Validator {
                address: address.clone(),
                stake: MIN_VALIDATOR_STAKE,
                first_stake_time: now_secs * 1000,
                bls_public_key: Some(hex::encode(bls_public_key)),
                missed_checkpoints: 0,
            };
            new_validators.insert(address.clone(), validator);
            
            if is_genesis_node {
                if !state.accounts.contains_key(address) {
                    let mut account = Account::new(address.clone(), now_secs);
                    account.staked = MIN_VALIDATOR_STAKE;
                    state.accounts.insert(address.clone(), account);
                    info!(
                        "Genesis validator account created: {} (staked: {} RKU)",
                        &address[..16.min(address.len())], MIN_VALIDATOR_STAKE
                    );
                } else {
                    let account = state.accounts.get_mut(address).unwrap();
                    if account.staked < MIN_VALIDATOR_STAKE {
                        account.staked = MIN_VALIDATOR_STAKE;
                        info!(
                            "Genesis validator account updated staked amount: {} -> {} RKU",
                            &address[..16.min(address.len())], MIN_VALIDATOR_STAKE
                        );
                    }
                }
            }
        }
        
        let new_count = new_validators.len();
        state.validators = new_validators;
        
        info!(
            "state.validators: REPLACED with genesis validators ({} -> {}), accounts_modified={}",
            old_count, new_count, is_genesis_node
        );
    }

    /// Merge validators from peer during delta sync
    /// This ensures all nodes converge to the same validator set for leader election
    /// Returns the number of validators added or updated
    pub async fn merge_validators_from_peer(
        &self,
        peer_validators: &std::collections::HashMap<String, Validator>,
    ) -> usize {
        let mut state = self.inner.write().await;
        let mut merged_count = 0;
        
        for (addr, peer_validator) in peer_validators {
            match state.validators.get_mut(addr) {
                Some(existing) => {
                    // Update BLS key if we don't have one but peer does
                    if existing.bls_public_key.is_none() && peer_validator.bls_public_key.is_some() {
                        existing.bls_public_key = peer_validator.bls_public_key.clone();
                        merged_count += 1;
                        info!("Updated BLS key for validator {} from peer", addr);
                    }
                    // Take higher stake value (peer might have more up-to-date stake info)
                    if peer_validator.stake > existing.stake {
                        existing.stake = peer_validator.stake;
                    }
                }
                None => {
                    // Add new validator from peer
                    state.validators.insert(addr.clone(), peer_validator.clone());
                    merged_count += 1;
                    info!("Added validator {} from peer (stake: {})", addr, peer_validator.stake);
                }
            }
        }
        
        merged_count
    }

    pub async fn get_finalization_info(&self, hash: &str) -> (bool, Option<u64>) {
        let state = self.inner.read().await;
        if let Some(node) = state.dag.get_node(hash) {
            (node.finalized, node.checkpoint_height)
        } else {
            (false, None)
        }
    }
}
