use anyhow::Result;
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::gossip::GossipService;
use crate::state::NodeState;
use rinku_core::types::{SignedTransaction, Transaction, TransactionKind};

/// With Sparse DAG Sampling (MAX_SAMPLED_TIPS=16), tips grow more slowly.
/// These thresholds are still generous to handle burst scenarios.
const UPPER_THRESHOLD: usize = 100;
const LOWER_THRESHOLD: usize = 50;
/// Consolidate same number as MAX_SAMPLED_TIPS for efficient merging
const TIPS_PER_CONSOLIDATION: usize = 16;
/// Anchor interval in milliseconds - how often to create anchor transactions
/// Shoal++ recommends every few hundred ms for faster finality
const ANCHOR_INTERVAL_MS: u64 = 500;
const CONSOLIDATION_COOLDOWN_MS: u64 = 1000;

/// Minimum tips to trigger anchor creation
/// Set to 1 to ensure anchors are always created, preventing network stalls
/// when there's no external transaction activity. Without this, the network
/// can deadlock after a checkpoint finalizes all transactions.
const MIN_TIPS_FOR_ANCHOR: usize = 1;

pub struct TipConsolidator {
    state: NodeState,
    validator_address: Option<String>,
    consolidations_total: u64,
    tips_consolidated_total: u64,
    is_consolidating: bool,
    last_consolidation: std::time::Instant,
    gossip_service: Option<Arc<GossipService>>,
}

impl TipConsolidator {
    pub fn new(state: NodeState, validator_address: Option<String>) -> Self {
        Self {
            state,
            validator_address,
            consolidations_total: 0,
            tips_consolidated_total: 0,
            is_consolidating: false,
            last_consolidation: std::time::Instant::now(),
            gossip_service: None,
        }
    }
    
    /// Set the gossip service for broadcasting anchor transactions
    pub fn with_gossip_service(mut self, gossip: Arc<GossipService>) -> Self {
        self.gossip_service = Some(gossip);
        self
    }

    pub async fn start(mut self) -> Result<()> {
        info!(
            "[TipConsolidator] Started with Shoal++ anchoring (threshold: {}, interval: {}ms)",
            UPPER_THRESHOLD, ANCHOR_INTERVAL_MS
        );

        let mut tick = interval(Duration::from_millis(ANCHOR_INTERVAL_MS));

        loop {
            tick.tick().await;
            self.check_and_consolidate().await;
        }
    }

    async fn check_and_consolidate(&mut self) {
        // Only validators can create anchor transactions
        let validator_address = match &self.validator_address {
            Some(addr) => addr.clone(),
            None => return,
        };
        
        let tips = self.state.get_tips().await;
        let tip_count = tips.len();

        // Track consolidation state for logging
        if tip_count >= UPPER_THRESHOLD && !self.is_consolidating {
            info!(
                "[TipConsolidator] Above upper threshold ({}), aggressive consolidation mode",
                tip_count
            );
            self.is_consolidating = true;
        } else if tip_count < LOWER_THRESHOLD && self.is_consolidating {
            info!(
                "[TipConsolidator] Below lower threshold ({}), normal mode",
                tip_count
            );
            self.is_consolidating = false;
        }

        // Shoal++ enhancement: Create anchor transactions even below threshold
        // This helps converge the DAG faster, reducing checkpoint latency
        if tip_count < MIN_TIPS_FOR_ANCHOR {
            return;
        }

        let elapsed = self.last_consolidation.elapsed();
        if elapsed < Duration::from_millis(CONSOLIDATION_COOLDOWN_MS) && !self.is_consolidating {
            return;
        }

        // Create anchor transaction referencing current tips
        let tips_to_consolidate: Vec<String> = tips
            .into_iter()
            .take(TIPS_PER_CONSOLIDATION)
            .collect();

        if tips_to_consolidate.is_empty() {
            return;
        }

        // Convert to URL format
        let tip_urls: Vec<String> = tips_to_consolidate
            .iter()
            .map(|hash| format!("rinku://tx/h/{}", hash))
            .collect();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        // Use timestamp as nonce for anchor transactions (system tx bypass nonce validation)
        // This ensures unique hashes for each anchor without consuming validator's account nonce
        let nonce = now;

        // Create anchor transaction: zero-value self-transfer with Consolidation kind
        let inner_tx = Transaction {
            from: validator_address.clone(),
            to: validator_address.clone(), // Self-transfer
            amount: 0.0, // Zero value
            nonce,
            timestamp: now,
            parents: tip_urls,
            kind: Some(TransactionKind::Consolidation),
            gas_limit: None,
            gas_price: Some(0.0), // System transaction, no gas
            data: Some("anchor".to_string()), // Mark as anchor
            signature: None,
            memo: None,
            references: None,
        };

        let tx = SignedTransaction {
            tx: inner_tx,
            hash: String::new(),
            signature: format!("anchor-{}", validator_address), // System signature
        };

        // Hash the transaction
        let tx_json = serde_json::to_string(&tx.tx).unwrap_or_default();
        let hash = rinku_core::crypto::hash_transaction(&tx_json);
        let tx = SignedTransaction { hash: hash.clone(), ..tx };

        debug!(
            "[TipConsolidator] Creating anchor tx {} referencing {} tips",
            &hash[..16.min(hash.len())],
            tips_to_consolidate.len()
        );

        // Submit to local DAG
        match self.state.add_transaction(tx.clone()).await {
            Ok(_) => {
                self.consolidations_total += 1;
                self.tips_consolidated_total += tips_to_consolidate.len() as u64;
                self.last_consolidation = std::time::Instant::now();

                // Broadcast to peers
                if let Some(ref gossip) = self.gossip_service {
                    gossip.broadcast_transaction(tx).await;
                }

                debug!(
                    "[TipConsolidator] Anchor {} created, {} tips consolidated (total: {})",
                    &hash[..16.min(hash.len())],
                    tips_to_consolidate.len(),
                    self.consolidations_total
                );
            }
            Err(e) => {
                warn!(
                    "[TipConsolidator] Failed to create anchor tx: {}",
                    e
                );
            }
        }
    }

    pub fn stats(&self) -> (u64, u64, bool) {
        (
            self.consolidations_total,
            self.tips_consolidated_total,
            self.is_consolidating,
        )
    }
}
