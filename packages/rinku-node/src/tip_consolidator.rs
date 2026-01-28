use anyhow::Result;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
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

/// Dynamic consolidation intervals based on TPS
const INTERVAL_HIGH_TPS_MS: u64 = 150; // >500 TPS: aggressive
const INTERVAL_MEDIUM_TPS_MS: u64 = 500; // 50-500 TPS: balanced
const INTERVAL_LOW_TPS_MS: u64 = 1500; // 10-50 TPS: relaxed
const INTERVAL_IDLE_MS: u64 = 10000; // <10 TPS: minimal overhead

/// TPS thresholds for interval selection
const TPS_HIGH: f64 = 500.0;
const TPS_MEDIUM: f64 = 50.0;
const TPS_LOW: f64 = 10.0;

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
    current_interval_ms: u64,
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
            current_interval_ms: INTERVAL_MEDIUM_TPS_MS,
        }
    }

    /// Set the gossip service for broadcasting anchor transactions
    pub fn with_gossip_service(mut self, gossip: Arc<GossipService>) -> Self {
        self.gossip_service = Some(gossip);
        self
    }

    /// Calculate dynamic interval based on current network TPS
    async fn calculate_interval(&self) -> u64 {
        let tps = self.state.get_finalized_tps().await;
        debug!("[TipConsolidator] Current TPS: {:.1}", tps);

        if tps >= TPS_HIGH {
            INTERVAL_HIGH_TPS_MS
        } else if tps >= TPS_MEDIUM {
            INTERVAL_MEDIUM_TPS_MS
        } else if tps >= TPS_LOW {
            INTERVAL_LOW_TPS_MS
        } else {
            INTERVAL_IDLE_MS
        }
    }

    pub async fn start(mut self) -> Result<()> {
        info!(
            "[TipConsolidator] Started with dynamic intervals (high TPS: {}ms, medium: {}ms, low: {}ms, idle: {}ms)",
            INTERVAL_HIGH_TPS_MS, INTERVAL_MEDIUM_TPS_MS, INTERVAL_LOW_TPS_MS, INTERVAL_IDLE_MS
        );

        loop {
            // Calculate dynamic interval based on current TPS
            let new_interval = self.calculate_interval().await;

            // Log interval changes
            if new_interval != self.current_interval_ms {
                let tps = self.state.get_finalized_tps().await;
                debug!(
                    "[TipConsolidator] Interval adjusted: {}ms -> {}ms (TPS: {:.1})",
                    self.current_interval_ms, new_interval, tps
                );
                self.current_interval_ms = new_interval;
            }

            sleep(Duration::from_millis(self.current_interval_ms)).await;
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

        // In aggressive mode (high tips), always consolidate
        // In normal mode, respect dynamic interval timing
        if !self.is_consolidating {
            let elapsed = self.last_consolidation.elapsed();
            // Add small buffer to prevent excessive consolidation at idle
            let min_cooldown = Duration::from_millis(self.current_interval_ms / 2);
            if elapsed < min_cooldown {
                return;
            }
        }

        // Create anchor transaction referencing current tips
        let tips_to_consolidate: Vec<String> =
            tips.into_iter().take(TIPS_PER_CONSOLIDATION).collect();

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
            amount: 0.0,                   // Zero value
            nonce,
            timestamp: now,
            parents: tip_urls,
            kind: Some(TransactionKind::Consolidation),
            gas_limit: None,
            gas_price: Some(0.0),             // System transaction, no gas
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
        let tx = SignedTransaction {
            hash: hash.clone(),
            ..tx
        };

        debug!(
            "[TipConsolidator] Creating anchor tx {} referencing {} tips (interval: {}ms)",
            &hash[..16.min(hash.len())],
            tips_to_consolidate.len(),
            self.current_interval_ms
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
                warn!("[TipConsolidator] Failed to create anchor tx: {}", e);
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
