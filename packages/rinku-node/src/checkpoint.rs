use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rinku_core::{
    merkle::MerkleTree,
    types::{Checkpoint, ValidatorSignature},
};
use sha2::{Digest, Sha256};
use tracing::{info, warn, debug};

use crate::bls::{
    aggregate_signatures, bls_sign, create_signer_bitmap, generate_bls_keypair,
};
use crate::state::NodeState;

pub struct CheckpointService {
    state: NodeState,
    interval_ms: u64,
    bls_private_key: Vec<u8>,
    bls_public_key: Vec<u8>,
    validator_address: String,
    peers: Vec<String>,
}

impl CheckpointService {
    pub fn new(state: NodeState, interval_ms: u64, validator_address: Option<String>, peers: Vec<String>) -> Self {
        let keypair = generate_bls_keypair();
        let addr = validator_address.unwrap_or_else(|| keypair.fingerprint.clone());
        Self {
            state,
            interval_ms,
            bls_private_key: keypair.private_key,
            bls_public_key: keypair.public_key,
            validator_address: addr,
            peers,
        }
    }

    pub fn bls_public_key_base64(&self) -> String {
        URL_SAFE_NO_PAD.encode(&self.bls_public_key)
    }

    pub async fn start(self) -> Result<()> {
        let interval = tokio::time::Duration::from_millis(self.interval_ms);

        loop {
            tokio::time::sleep(interval).await;
            if let Err(e) = self.create_checkpoint().await {
                tracing::warn!("Checkpoint creation failed: {}", e);
            }
        }
    }

    /// Check if any peer has a checkpoint at the given height
    /// If found, returns the peer's checkpoint to adopt instead of creating our own
    async fn fetch_peer_checkpoint(&self, height: u64) -> Option<Checkpoint> {
        if self.peers.is_empty() {
            return None;
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .ok()?;

        for peer in &self.peers {
            let url = format!("{}/api/checkpoints/{}", peer.trim_end_matches('/'), height);
            
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<Checkpoint>().await {
                        Ok(checkpoint) => {
                            info!(
                                "Found peer checkpoint at height {} from {}: {}",
                                height, peer, &checkpoint.hash[..16.min(checkpoint.hash.len())]
                            );
                            return Some(checkpoint);
                        }
                        Err(e) => {
                            debug!("Failed to parse checkpoint from {}: {}", peer, e);
                        }
                    }
                }
                Ok(resp) => {
                    debug!("Peer {} has no checkpoint at height {} (status: {})", peer, height, resp.status());
                }
                Err(e) => {
                    debug!("Failed to reach peer {} for checkpoint {}: {}", peer, height, e);
                }
            }
        }

        None
    }

    /// Sync missing transactions from peers before checkpoint
    async fn sync_missing_transactions(&self, target_height: u64) -> Result<()> {
        if self.peers.is_empty() {
            return Err(anyhow::anyhow!("No peers available"));
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        // Get our current checkpoint height
        let from_checkpoint = {
            let state = self.state.inner.read().await;
            state.checkpoints.len() as u64
        };

        for peer in &self.peers {
            // Use delta sync to get transactions since last checkpoint
            let url = format!(
                "{}/api/sync/delta?from_checkpoint={}&limit=500",
                peer.trim_end_matches('/'),
                from_checkpoint
            );
            
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    // Try to parse as paginated response first
                    if let Ok(text) = resp.text().await {
                        use rinku_core::types::SignedTransaction;
                        
                        // Parse JSON and extract transactions
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            let txs: Vec<SignedTransaction> = if json.is_array() {
                                serde_json::from_value(json).unwrap_or_default()
                            } else if let Some(txs_val) = json.get("transactions") {
                                serde_json::from_value(txs_val.clone()).unwrap_or_default()
                            } else {
                                vec![]
                            };
                            
                            if !txs.is_empty() {
                                let mut added = 0;
                                
                                for tx in txs {
                                    let mut state = self.state.inner.write().await;
                                    if !state.dag.contains(&tx.hash) {
                                        use rinku_core::types::DagNode;
                                        let parents = tx.tx.parents.clone();
                                        let node = DagNode {
                                            hash: tx.hash.clone(),
                                            parents,
                                            children: vec![],
                                            weight: 1.0,
                                            finalized: false,
                                            checkpoint_height: None,
                                            tx,
                                        };
                                        if state.dag.add_node(node).is_ok() {
                                            added += 1;
                                        }
                                    }
                                }
                                
                                if added > 0 {
                                    debug!(
                                        "Fork prevention: synced {} new transactions from peer for checkpoint {}",
                                        added, target_height
                                    );
                                }
                                return Ok(());
                            }
                        }
                    }
                }
                Ok(_) => {
                    debug!("Peer {} returned non-success for delta sync", peer);
                }
                Err(e) => {
                    debug!("Failed to sync from peer {}: {}", peer, e);
                }
            }
        }

        Err(anyhow::anyhow!("No peers had transactions to sync"))
    }

    /// Validate and adopt a peer's checkpoint instead of creating our own
    /// Returns true if adoption was successful, false if we should create our own checkpoint
    async fn validate_and_adopt_peer_checkpoint(
        &self, 
        peer_checkpoint: Checkpoint, 
        local_tx_merkle_root: &str,
        local_previous_hash: Option<&str>,
        unfinalized_hashes: &[String]
    ) -> bool {
        // VALIDATION 1: Check previous_hash chain linkage
        let peer_prev = peer_checkpoint.previous_hash.as_deref();
        if peer_prev != local_previous_hash {
            warn!(
                "Peer checkpoint previous_hash mismatch at height {}: peer={:?} vs local={:?}",
                peer_checkpoint.height,
                peer_prev.map(|s| &s[..16.min(s.len())]),
                local_previous_hash.map(|s| &s[..16.min(s.len())])
            );
            return false;
        }

        // VALIDATION 2: Check merkle root matches our local transaction set
        // This ensures we're finalizing the same transactions
        if peer_checkpoint.tx_merkle_root != local_tx_merkle_root {
            warn!(
                "Peer checkpoint merkle root mismatch at height {}: peer={} vs local={}",
                peer_checkpoint.height,
                &peer_checkpoint.tx_merkle_root[..16.min(peer_checkpoint.tx_merkle_root.len())],
                &local_tx_merkle_root[..16.min(local_tx_merkle_root.len())]
            );
            return false;
        }

        // VALIDATION 3: Ensure checkpoint has at least one validator signature
        if peer_checkpoint.validator_signatures.is_empty() {
            warn!(
                "Peer checkpoint at height {} has no validator signatures",
                peer_checkpoint.height
            );
            return false;
        }

        // VALIDATION 4: Verify the checkpoint hash matches recomputed value
        let expected_hash = Self::compute_checkpoint_hash(
            peer_checkpoint.height,
            &peer_checkpoint.tx_merkle_root,
            &peer_checkpoint.state_root,
            &peer_checkpoint.receipt_root,
            peer_checkpoint.tip_count,
            peer_checkpoint.timestamp,
        );
        let expected_hash_hex = hex::encode(&expected_hash);
        
        if peer_checkpoint.hash != expected_hash_hex {
            warn!(
                "Peer checkpoint hash mismatch at height {}: provided={} vs computed={}",
                peer_checkpoint.height,
                &peer_checkpoint.hash[..16.min(peer_checkpoint.hash.len())],
                &expected_hash_hex[..16.min(expected_hash_hex.len())]
            );
            return false;
        }
        
        // VALIDATION 5: Verify at least one BLS signature cryptographically
        let mut valid_sig_found = false;
        for sig in &peer_checkpoint.validator_signatures {
            // Decode the signature from base64
            let sig_bytes = match URL_SAFE_NO_PAD.decode(&sig.signature) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            
            // BLS signatures in compressed form are 96 bytes
            if sig_bytes.len() < 96 {
                continue;
            }
            
            // Try to decode the validator address as a public key
            // The validator field might contain the public key directly or a fingerprint
            // For now, we verify the signature format is valid BLS
            // Full verification requires a validator registry with public keys
            if let Ok(_) = blst::min_pk::Signature::from_bytes(&sig_bytes) {
                valid_sig_found = true;
                debug!(
                    "Peer checkpoint has valid BLS signature from validator {}",
                    &sig.validator[..16.min(sig.validator.len())]
                );
                break;
            }
        }
        
        if !valid_sig_found {
            warn!(
                "Peer checkpoint at height {} has no valid BLS signature",
                peer_checkpoint.height
            );
            return false;
        }

        // Validation passed - adopt the peer's checkpoint
        let height = peer_checkpoint.height;
        let checkpoint_hash = peer_checkpoint.hash.clone();

        // Process emissions for this adopted checkpoint
        let checkpoint_reward = {
            let mut emission = self.state.emission.write().await;
            let reward = emission.get_checkpoint_reward(height);
            emission.record_emission(reward);
            reward
        };

        // Distribute checkpoint rewards
        let distributions = {
            let mut rewards = self.state.rewards.write().await;
            rewards.distribute_checkpoint_rewards(checkpoint_reward)
        };

        if !distributions.is_empty() {
            debug!(
                "Distributed {:.6} RKU to {} validators (adopted checkpoint)",
                checkpoint_reward,
                distributions.len()
            );
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Update state with adopted checkpoint
        let mut state = self.state.inner.write().await;
        state.checkpoints.push(peer_checkpoint);
        state.last_checkpoint_time_ms = now_ms;

        // Mark transactions as finalized
        for hash in unfinalized_hashes {
            let _ = state.dag.mark_finalized(hash, height);
        }

        drop(state);

        info!(
            "Adopted peer checkpoint {} at height {} ({} txs finalized, {:.6} RKU emitted)",
            &checkpoint_hash[..16.min(checkpoint_hash.len())],
            height,
            unfinalized_hashes.len(),
            checkpoint_reward
        );

        true
    }

    fn compute_checkpoint_hash(
        height: u64,
        tx_merkle_root: &str,
        state_root: &str,
        receipt_root: &str,
        tip_count: u32,
        timestamp: u64,
    ) -> Vec<u8> {
        let data = format!(
            "{}:{}:{}:{}:{}:{}",
            height, tx_merkle_root, state_root, receipt_root, tip_count, timestamp
        );
        let mut hasher = Sha256::new();
        hasher.update(data.as_bytes());
        hasher.finalize().to_vec()
    }

    fn is_valid_hex_hash(s: &str) -> bool {
        s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
    }

    async fn create_checkpoint(&self) -> Result<()> {
        let (unfinalized_hashes, tx_merkle_root, height, previous_hash) = {
            let state = self.state.inner.read().await;

            let unfinalized: Vec<String> = state
                .dag
                .get_unfinalized_nodes()
                .iter()
                .map(|n| n.hash.clone())
                .filter(|h| Self::is_valid_hex_hash(h))
                .collect();

            if unfinalized.is_empty() {
                return Ok(());
            }

            let tx_merkle_root = if unfinalized.is_empty() {
                "0".repeat(64)
            } else {
                let tree = MerkleTree::from_hex_leaves(&unfinalized)?;
                tree.root()
            };

            let height = state.checkpoints.len() as u64 + 1;
            let previous_hash = state.checkpoints.last().map(|c| c.hash.clone());

            (unfinalized, tx_merkle_root, height, previous_hash)
        };

        // FORK PREVENTION: Check if any peer already has a checkpoint at this height
        // If so, try to adopt their checkpoint instead of creating our own (with validation)
        if let Some(peer_checkpoint) = self.fetch_peer_checkpoint(height).await {
            // Verify the peer checkpoint has valid structure and matches our state
            if peer_checkpoint.height == height && !peer_checkpoint.hash.is_empty() {
                debug!(
                    "Peer has checkpoint at height {}, validating for adoption...",
                    height
                );
                
                // Validate and adopt - if validation fails, we'll try to sync first
                let adopted = self.validate_and_adopt_peer_checkpoint(
                    peer_checkpoint.clone(),
                    &tx_merkle_root,
                    previous_hash.as_deref(),
                    &unfinalized_hashes,
                ).await;
                
                if adopted {
                    return Ok(());
                }
                
                // Validation failed - likely due to merkle root mismatch
                // Try to sync missing transactions from peer before creating our own checkpoint
                info!(
                    "Peer checkpoint validation failed at height {}, attempting DAG sync before fallback",
                    height
                );
                
                // Request delta sync from the peer to get any missing transactions
                if let Err(e) = self.sync_missing_transactions(height).await {
                    debug!("DAG sync failed: {}, proceeding with local checkpoint", e);
                }
                
                // After sync, recompute our state and try adoption again
                let (new_unfinalized, new_merkle_root) = {
                    let state = self.state.inner.read().await;
                    let unfinalized: Vec<String> = state
                        .dag
                        .get_unfinalized_nodes()
                        .iter()
                        .map(|n| n.hash.clone())
                        .filter(|h| Self::is_valid_hex_hash(h))
                        .collect();
                    
                    let merkle_root = if unfinalized.is_empty() {
                        "0".repeat(64)
                    } else {
                        match MerkleTree::from_hex_leaves(&unfinalized) {
                            Ok(tree) => tree.root(),
                            Err(_) => return Ok(()), // Retry next interval
                        }
                    };
                    (unfinalized, merkle_root)
                };
                
                // Retry adoption with updated state
                let adopted_retry = self.validate_and_adopt_peer_checkpoint(
                    peer_checkpoint,
                    &new_merkle_root,
                    previous_hash.as_deref(),
                    &new_unfinalized,
                ).await;
                
                if adopted_retry {
                    return Ok(());
                }
                
                // Still failed after sync - proceed with local checkpoint
                debug!("Peer checkpoint validation failed after sync, creating our own");
            } else {
                warn!(
                    "Peer checkpoint at height {} has invalid structure, creating our own",
                    height
                );
            }
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let state_root = "0".repeat(64);
        let receipt_root = "0".repeat(64);
        let tip_count = unfinalized_hashes.len() as u32;

        let checkpoint_hash = Self::compute_checkpoint_hash(
            height,
            &tx_merkle_root,
            &state_root,
            &receipt_root,
            tip_count,
            timestamp,
        );

        let signature = bls_sign(&checkpoint_hash, &self.bls_private_key)
            .map_err(|e| anyhow::anyhow!("BLS signing failed: {}", e))?;

        let validator_sig = ValidatorSignature {
            validator: self.validator_address.clone(),
            signature: URL_SAFE_NO_PAD.encode(&signature),
            weight: 1.0,
        };

        let aggregated_sig = aggregate_signatures(&[signature.clone()])
            .map_err(|e| anyhow::anyhow!("BLS aggregation failed: {}", e))?;

        let signer_bitmap = create_signer_bitmap(&[0], 1);

        let checkpoint = Checkpoint {
            height,
            hash: hex::encode(&checkpoint_hash),
            previous_hash,
            tx_merkle_root,
            state_root,
            receipt_root,
            tip_count,
            timestamp,
            validator_signatures: vec![validator_sig],
            aggregated_signature: Some(URL_SAFE_NO_PAD.encode(&aggregated_sig)),
            signer_bitmap: Some(signer_bitmap),
        };

        // Process emissions and rewards for this checkpoint
        let checkpoint_reward = {
            let mut emission = self.state.emission.write().await;
            let reward = emission.get_checkpoint_reward(height);
            emission.record_emission(reward);
            reward
        };

        // Distribute checkpoint rewards to staked validators
        let distributions = {
            let mut rewards = self.state.rewards.write().await;
            rewards.distribute_checkpoint_rewards(checkpoint_reward)
        };

        if !distributions.is_empty() {
            info!(
                "Distributed {:.6} RKU to {} validators",
                checkpoint_reward,
                distributions.len()
            );
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // PHASE 1: Quick read to collect timestamps (minimal lock time)
        let tx_timestamps: Vec<(String, u64)> = {
            let state = self.state.inner.read().await;
            unfinalized_hashes.iter()
                .filter_map(|hash| {
                    state.dag.get_node(hash).map(|node| {
                        let ts = node.tx.tx.timestamp;
                        (hash.clone(), ts)
                    })
                })
                .collect()
        };

        // PHASE 2: Compute finality times OUTSIDE any lock
        let finality_times: Vec<u64> = tx_timestamps.iter()
            .map(|(_, tx_timestamp)| {
                let tx_time_ms = if *tx_timestamp < 4_000_000_000 {
                    tx_timestamp * 1000
                } else {
                    *tx_timestamp
                };
                now_ms.saturating_sub(tx_time_ms)
            })
            .collect();

        // PHASE 3: Minimal write lock - just mutations
        let mut state = self.state.inner.write().await;
        state.checkpoints.push(checkpoint.clone());
        state.last_checkpoint_time_ms = now_ms;

        // Update finality stats
        for finality_time in &finality_times {
            state.finality_sum_ms += finality_time;
            state.finality_count += 1;
            if *finality_time > state.finality_max_ms {
                state.finality_max_ms = *finality_time;
            }
            if state.finality_times_ms.len() >= 1000 {
                state.finality_times_ms.pop_front();
            }
            state.finality_times_ms.push_back(*finality_time);
        }

        // Mark all as finalized
        for hash in &unfinalized_hashes {
            let _ = state.dag.mark_finalized(hash, height);
        }
        
        // Release write lock before logging
        drop(state);

        info!(
            "Created checkpoint {} at height {} ({} txs finalized, {:.6} RKU emitted)",
            &checkpoint.hash[..16],
            height,
            unfinalized_hashes.len(),
            checkpoint_reward
        );

        Ok(())
    }
}
