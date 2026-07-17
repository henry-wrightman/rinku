use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::config::{MEMPOOL_CLEANUP_INTERVAL_MS, TRANSACTION_TTL_MS};
use crate::state::NodeState;

pub struct MempoolCleanupService {
    state: NodeState,
    interval_ms: u64,
    ttl_ms: u64,
}

impl MempoolCleanupService {
    pub fn new(state: NodeState) -> Self {
        Self {
            state,
            interval_ms: MEMPOOL_CLEANUP_INTERVAL_MS,
            ttl_ms: TRANSACTION_TTL_MS,
        }
    }

    pub async fn start(self) {
        info!(
            "[MempoolCleanup] Started with interval={}ms, TTL={}ms",
            self.interval_ms, self.ttl_ms
        );

        let mut ticker = interval(Duration::from_millis(self.interval_ms));

        loop {
            ticker.tick().await;
            self.cleanup_expired_transactions().await;
        }
    }

    async fn cleanup_expired_transactions(&self) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let cutoff_ms = now_ms.saturating_sub(self.ttl_ms);

        let expired_count = self
            .state
            .prune_expired_pending_transactions(cutoff_ms)
            .await;

        if expired_count > 0 {
            info!(
                "[MempoolCleanup] Pruned {} expired pending transactions (TTL: {}s)",
                expired_count,
                self.ttl_ms / 1000
            );
        } else {
            debug!("[MempoolCleanup] No expired transactions found");
        }
    }
}
