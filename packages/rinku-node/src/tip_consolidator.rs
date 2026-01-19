use anyhow::Result;
use tokio::time::{interval, Duration};
use tracing::{debug, info};

use crate::state::NodeState;

const UPPER_THRESHOLD: usize = 500;
const LOWER_THRESHOLD: usize = 200;
const TIPS_PER_CONSOLIDATION: usize = 64;
const CONSOLIDATION_INTERVAL_MS: u64 = 1000;
const CONSOLIDATION_COOLDOWN_MS: u64 = 2000;

pub struct TipConsolidator {
    state: NodeState,
    validator_address: Option<String>,
    consolidations_total: u64,
    tips_consolidated_total: u64,
    is_consolidating: bool,
    last_consolidation: std::time::Instant,
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
        }
    }

    pub async fn start(mut self) -> Result<()> {
        info!(
            "[TipConsolidator] Started (threshold: {}, interval: {}ms)",
            UPPER_THRESHOLD, CONSOLIDATION_INTERVAL_MS
        );

        let mut tick = interval(Duration::from_millis(CONSOLIDATION_INTERVAL_MS));

        loop {
            tick.tick().await;
            self.check_and_consolidate().await;
        }
    }

    async fn check_and_consolidate(&mut self) {
        let tips = self.state.get_tips().await;
        let tip_count = tips.len();

        if tip_count < UPPER_THRESHOLD && !self.is_consolidating {
            return;
        }

        if tip_count < LOWER_THRESHOLD && self.is_consolidating {
            info!(
                "[TipConsolidator] Below lower threshold ({}), stopping consolidation",
                tip_count
            );
            self.is_consolidating = false;
            return;
        }

        if tip_count >= UPPER_THRESHOLD && !self.is_consolidating {
            info!(
                "[TipConsolidator] Above upper threshold ({}), starting consolidation",
                tip_count
            );
            self.is_consolidating = true;
        }

        let elapsed = self.last_consolidation.elapsed();
        if elapsed < Duration::from_millis(CONSOLIDATION_COOLDOWN_MS) {
            return;
        }

        if let Some(_validator) = &self.validator_address {
            let tips_to_consolidate: Vec<_> = tips.into_iter().take(TIPS_PER_CONSOLIDATION).collect();

            if tips_to_consolidate.is_empty() {
                return;
            }

            debug!(
                "[TipConsolidator] Creating consolidation tx for {} tips",
                tips_to_consolidate.len()
            );

            self.consolidations_total += 1;
            self.tips_consolidated_total += tips_to_consolidate.len() as u64;
            self.last_consolidation = std::time::Instant::now();
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
