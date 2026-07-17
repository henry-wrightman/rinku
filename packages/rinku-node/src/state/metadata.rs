use super::*;

impl NodeState {
    pub async fn get_uptime_seconds(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }

    pub async fn set_validator_info(
        &self,
        address: Option<String>,
        bls_public_key: Option<String>,
        allow_auto_register: bool,
    ) {
        use crate::validator_identity::MIN_VALIDATOR_STAKE;

        let mut state = self.inner.write().await;
        state.node_validator_address = address.clone();
        state.node_bls_public_key = bls_public_key.clone();

        // Register or update in state.validators so it gets synced to peers via snapshots
        if let Some(ref addr) = address {
            if let Some(existing) = state.validators.get_mut(addr) {
                // Update existing entry if BLS key is missing or different
                if existing.bls_public_key.is_none() || existing.bls_public_key != bls_public_key {
                    info!(
                        "Updating BLS key for validator {} in state.validators",
                        addr
                    );
                    existing.bls_public_key = bls_public_key.clone();
                }
            } else if allow_auto_register {
                // Create new entry ONLY if auto-registration is allowed
                // When GENESIS_VALIDATORS is set, we should NOT auto-register because
                // GENESIS_VALIDATORS is the authoritative source of truth
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let validator = Validator {
                    address: addr.clone(),
                    stake: MIN_VALIDATOR_STAKE,
                    first_stake_time: now_secs * 1000,
                    bls_public_key: bls_public_key.clone(),
                    missed_checkpoints: 0,
                };
                info!(
                    "Registering local validator {} in state.validators for peer sync",
                    addr
                );
                state.validators.insert(addr.clone(), validator);
            } else {
                warn!(
                    "Local validator {} not in GENESIS_VALIDATORS - skipping auto-registration",
                    addr
                );
            }
        }
    }

    pub async fn get_validator_info(&self) -> (Option<String>, Option<String>) {
        let state = self.inner.read().await;
        (
            state.node_validator_address.clone(),
            state.node_bls_public_key.clone(),
        )
    }

    pub async fn set_peer_info(&self, peer_id: String, listen_addr: String) {
        let mut state = self.inner.write().await;
        state.node_peer_id = Some(peer_id);
        state.node_listen_addr = Some(listen_addr);
    }

    pub async fn get_bootstrap_info(
        &self,
    ) -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        let state = self.inner.read().await;
        (
            state.node_peer_id.clone(),
            state.node_listen_addr.clone(),
            state.node_validator_address.clone(),
            state.node_bls_public_key.clone(),
        )
    }

    pub async fn get_validator_bls_pubkey_bytes(&self, address: &str) -> Option<Vec<u8>> {
        let state = self.inner.read().await;
        let key = state.validators.get(address)?.bls_public_key.as_ref()?;
        let decoded = URL_SAFE_NO_PAD
            .decode(key)
            .ok()
            .or_else(|| hex::decode(key).ok());
        decoded
    }

    pub async fn verify_slashing_evidence(
        &self,
        evidence: &crate::slashing::DoubleSignEvidence,
    ) -> bool {
        let pubkey = match self
            .get_validator_bls_pubkey_bytes(&evidence.validator)
            .await
        {
            Some(key) => key,
            None => return false,
        };
        let sig2 = match evidence.signature2.as_ref() {
            Some(s) => s,
            None => return false,
        };

        let sig1_bytes = URL_SAFE_NO_PAD
            .decode(&evidence.signature1)
            .ok()
            .or_else(|| hex::decode(&evidence.signature1).ok());
        let sig2_bytes = URL_SAFE_NO_PAD
            .decode(sig2)
            .ok()
            .or_else(|| hex::decode(sig2).ok());
        let (Some(sig1), Some(sig2)) = (sig1_bytes, sig2_bytes) else {
            return false;
        };

        let hash1_ok = self.verify_signature_for_hash(
            &evidence.hash1,
            &sig1,
            &pubkey,
            evidence.checkpoint_height,
        );
        let hash2_ok = self.verify_signature_for_hash(
            &evidence.hash2,
            &sig2,
            &pubkey,
            evidence.checkpoint_height,
        );
        hash1_ok && hash2_ok
    }

    fn verify_signature_for_hash(
        &self,
        hash: &str,
        signature: &[u8],
        pubkey: &[u8],
        checkpoint_height: u64,
    ) -> bool {
        use crate::bls::bls_verify;
        use crate::consensus::VoteType;

        if bls_verify(hash.as_bytes(), signature, pubkey) {
            return true;
        }
        let vote_types = [VoteType::Prepare, VoteType::Commit, VoteType::Finalize];
        for vote_type in vote_types {
            let mut msg = Vec::new();
            msg.extend_from_slice(&[vote_type as u8]);
            msg.extend_from_slice(&checkpoint_height.to_le_bytes());
            msg.extend_from_slice(hash.as_bytes());
            if bls_verify(&msg, signature, pubkey) {
                return true;
            }
        }
        false
    }

    pub async fn get_genesis_hash(&self) -> Option<String> {
        let state = self.inner.read().await;
        if let Some(ref hash) = state.genesis_hash {
            return Some(hash.clone());
        }
        for node in state.dag.get_all_nodes() {
            if node.tx.tx.from == "genesis" {
                return Some(node.hash.clone());
            }
        }
        if let Some(first_checkpoint) = state.checkpoints.first() {
            return Some(first_checkpoint.tx_merkle_root.clone());
        }
        None
    }

    pub async fn set_genesis_hash(&self, hash: String) {
        let mut state = self.inner.write().await;
        state.genesis_hash = Some(hash.clone());
        drop(state);
        let storage = self.storage.clone();
        let hash_clone = hash.clone();
        if let Err(e) =
            crate::storage::blocking_io(move || storage.save_genesis_hash(&hash_clone)).await
        {
            warn!("Failed to persist genesis hash: {}", e);
        }
    }

    /// Check if this node has ever successfully synced from the network.
    /// If false, the node is new and should adopt the peer's genesis hash.
    pub async fn has_synced_from_network(&self) -> bool {
        let state = self.inner.read().await;
        state.has_synced_from_network
    }

    /// Mark this node as having synced from the network.
    /// Called after successfully applying a sync snapshot.
    pub async fn mark_synced_from_network(&self) {
        let mut state = self.inner.write().await;
        state.has_synced_from_network = true;
    }
}
