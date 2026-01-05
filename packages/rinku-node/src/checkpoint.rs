use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rinku_core::{
    merkle::MerkleTree,
    types::{Checkpoint, ValidatorSignature},
};
use sha2::{Digest, Sha256};
use tracing::info;

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
}

impl CheckpointService {
    pub fn new(state: NodeState, interval_ms: u64, validator_address: Option<String>) -> Self {
        let keypair = generate_bls_keypair();
        let addr = validator_address.unwrap_or_else(|| keypair.fingerprint.clone());
        Self {
            state,
            interval_ms,
            bls_private_key: keypair.private_key,
            bls_public_key: keypair.public_key,
            validator_address: addr,
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

    async fn create_checkpoint(&self) -> Result<()> {
        let (unfinalized_hashes, tx_merkle_root, height, previous_hash) = {
            let state = self.state.inner.read().await;

            let unfinalized: Vec<String> = state
                .dag
                .get_unfinalized_nodes()
                .iter()
                .map(|n| n.hash.clone())
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

        let mut state = self.state.inner.write().await;
        state.checkpoints.push(checkpoint.clone());
        state.last_checkpoint_time_ms = now_ms;

        // Record finality times for ALL finalized transactions
        for hash in &unfinalized_hashes {
            if let Some(node) = state.dag.get_node(hash) {
                // Transaction timestamp may be in seconds or milliseconds
                // If timestamp is small (before year 2100 in seconds), it's in seconds
                let tx_timestamp = node.tx.tx.timestamp;
                let tx_time_ms = if tx_timestamp < 4_000_000_000 {
                    tx_timestamp * 1000 // Convert seconds to milliseconds
                } else {
                    tx_timestamp // Already in milliseconds
                };
                let finality_time = now_ms.saturating_sub(tx_time_ms);
                
                // Track aggregate stats for accurate average
                state.finality_sum_ms += finality_time;
                state.finality_count += 1;
                if finality_time > state.finality_max_ms {
                    state.finality_max_ms = finality_time;
                }
                
                // Keep rolling window for percentile calculations (larger window)
                if state.finality_times_ms.len() >= 1000 {
                    state.finality_times_ms.pop_front();
                }
                state.finality_times_ms.push_back(finality_time);
            }
            let _ = state.dag.mark_finalized(hash, height);
        }

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
