use anyhow::Result;
use rinku_core::{
    merkle::MerkleTree,
    types::Checkpoint,
};
use tracing::info;

use crate::state::NodeState;

pub struct CheckpointService {
    state: NodeState,
    interval_ms: u64,
}

impl CheckpointService {
    pub fn new(state: NodeState, interval_ms: u64) -> Self {
        Self { state, interval_ms }
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

        let checkpoint = Checkpoint {
            height,
            hash: format!("{:064x}", height),
            previous_hash,
            tx_merkle_root,
            state_root: "0".repeat(64),
            receipt_root: "0".repeat(64),
            tip_count: unfinalized_hashes.len() as u32,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs(),
            validator_signatures: Vec::new(),
            aggregated_signature: None,
            signer_bitmap: None,
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

        let mut state = self.state.inner.write().await;
        state.checkpoints.push(checkpoint.clone());

        for hash in &unfinalized_hashes {
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
