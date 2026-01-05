use anyhow::Result;
use std::collections::{HashMap, HashSet};
use tracing::{debug, info, warn};

use crate::state::NodeState;

const FORK_CHECK_THRESHOLD: usize = 50;
const MAX_TIPS_FOR_FULL_SCAN: usize = 10;

pub struct ForkRemediationService {
    state: NodeState,
    nonce_index: HashMap<(String, u64), Vec<String>>,
    fork_events: Vec<ForkEvent>,
    new_txs_since_check: usize,
    analyzed_pairs: HashSet<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct ForkEvent {
    pub tip_a: String,
    pub tip_b: String,
    pub weight_a: f64,
    pub weight_b: f64,
    pub detected_at: u64,
    pub resolved: bool,
}

impl ForkRemediationService {
    pub fn new(state: NodeState) -> Self {
        Self {
            state,
            nonce_index: HashMap::new(),
            fork_events: Vec::new(),
            new_txs_since_check: 0,
            analyzed_pairs: HashSet::new(),
        }
    }

    pub async fn start(mut self) -> Result<()> {
        let interval = tokio::time::Duration::from_secs(30);

        loop {
            tokio::time::sleep(interval).await;
            self.detect_forks().await;
            self.cleanup_old_data();
            self.log_summary();
        }
    }

    pub fn index_transaction(&mut self, from: &str, nonce: u64, hash: &str) {
        let key = (from.to_string(), nonce);
        self.nonce_index
            .entry(key.clone())
            .or_default()
            .push(hash.to_string());

        let should_detect = self.nonce_index.get(&key).map(|h| h.len() > 1).unwrap_or(false);
        if should_detect {
            let hashes: Vec<String> = self.nonce_index.get(&key).cloned().unwrap_or_default();
            self.detect_double_spend(from, nonce, &hashes);
        }

        self.new_txs_since_check += 1;
        if self.new_txs_since_check >= FORK_CHECK_THRESHOLD {
            tokio::spawn({
                let state = self.state.clone();
                async move {
                    let mut service = ForkRemediationService::new(state);
                    service.detect_forks().await;
                }
            });
            self.new_txs_since_check = 0;
        }
    }

    fn detect_double_spend(&mut self, account: &str, nonce: u64, tx_hashes: &[String]) {
        warn!(
            "Double-spend detected: account {} nonce {} has {} transactions",
            account,
            nonce,
            tx_hashes.len()
        );
    }

    async fn detect_forks(&mut self) {
        let state = self.state.inner.read().await;
        let tips = state.dag.tips();
        drop(state);

        if tips.len() > 50 {
            debug!("Too many tips ({}), skipping fork detection", tips.len());
            return;
        }

        let tips_to_analyze: Vec<_> = tips.iter().take(MAX_TIPS_FOR_FULL_SCAN).cloned().collect();

        for i in 0..tips_to_analyze.len() {
            for j in (i + 1)..tips_to_analyze.len() {
                let tip_a = &tips_to_analyze[i];
                let tip_b = &tips_to_analyze[j];

                let pair = if tip_a < tip_b {
                    (tip_a.clone(), tip_b.clone())
                } else {
                    (tip_b.clone(), tip_a.clone())
                };

                if self.analyzed_pairs.contains(&pair) {
                    continue;
                }

                self.analyzed_pairs.insert(pair);
            }
        }

        if self.analyzed_pairs.len() > 500 {
            self.analyzed_pairs.clear();
        }
    }

    fn cleanup_old_data(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        self.fork_events
            .retain(|e| now - e.detected_at < 120 || !e.resolved);

        if self.fork_events.len() > 1000 {
            self.fork_events.truncate(500);
        }
    }

    fn log_summary(&self) {
        let active_forks = self.fork_events.iter().filter(|e| !e.resolved).count();
        if active_forks > 0 {
            info!(
                "[ForkRemediation] Active forks: {}, Indexed nonces: {}",
                active_forks,
                self.nonce_index.len()
            );
        }
    }
}
