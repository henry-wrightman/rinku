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

    /// Adopt a peer's checkpoint instead of creating our own
    /// This prevents forks by deferring to the canonical chain
    async fn adopt_peer_checkpoint(&self, checkpoint: Checkpoint, unfinalized_hashes: &[String]) -> Result<()> {
        let height = checkpoint.height;
        let checkpoint_hash = checkpoint.hash.clone();

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
        state.checkpoints.push(checkpoint);
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

        Ok(())
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
        // If so, adopt their checkpoint instead of creating our own
        if let Some(peer_checkpoint) = self.fetch_peer_checkpoint(height).await {
            // Verify the peer checkpoint has valid structure before adopting
            if peer_checkpoint.height == height && !peer_checkpoint.hash.is_empty() {
                info!(
                    "Peer has checkpoint at height {}, adopting instead of creating",
                    height
                );
                return self.adopt_peer_checkpoint(peer_checkpoint, &unfinalized_hashes).await;
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
