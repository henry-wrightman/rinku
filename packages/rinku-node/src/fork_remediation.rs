use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::state::NodeState;

const FORK_CHECK_THRESHOLD: usize = 50;
const MAX_TIPS_FOR_FULL_SCAN: usize = 10;
const WEIGHT_THRESHOLD: f64 = 1.5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkEvent {
    pub fork_id: String,
    pub tip_a: String,
    pub tip_b: String,
    pub weight_a: f64,
    pub weight_b: f64,
    pub winner: Option<String>,
    pub loser: Option<String>,
    pub detected_at: u64,
    pub resolved_at: Option<u64>,
    pub pruned_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoubleSpendEvent {
    pub account: String,
    pub nonce: u64,
    pub tx_hashes: Vec<String>,
    pub winner: Option<String>,
    pub detected_at: u64,
    pub resolved: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ForkStats {
    pub total_forks_detected: u64,
    pub total_forks_resolved: u64,
    pub total_double_spends: u64,
    pub total_branches_pruned: u64,
    pub total_txs_pruned: u64,
    pub active_forks: usize,
}

pub struct ForkRemediationServiceInner {
    pub nonce_index: HashMap<(String, u64), Vec<String>>,
    pub fork_events: Vec<ForkEvent>,
    pub double_spend_events: Vec<DoubleSpendEvent>,
    pub analyzed_pairs: HashSet<(String, String)>,
    pub stats: ForkStats,
}

pub struct ForkRemediationService {
    state: NodeState,
    inner: Arc<RwLock<ForkRemediationServiceInner>>,
    new_txs_since_check: Arc<RwLock<usize>>,
}

impl Clone for ForkRemediationService {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            inner: self.inner.clone(),
            new_txs_since_check: self.new_txs_since_check.clone(),
        }
    }
}

impl ForkRemediationService {
    pub fn new(state: NodeState) -> Self {
        Self {
            state,
            inner: Arc::new(RwLock::new(ForkRemediationServiceInner {
                nonce_index: HashMap::new(),
                fork_events: Vec::new(),
                double_spend_events: Vec::new(),
                analyzed_pairs: HashSet::new(),
                stats: ForkStats::default(),
            })),
            new_txs_since_check: Arc::new(RwLock::new(0)),
        }
    }

    pub async fn start(self) -> Result<()> {
        let interval = tokio::time::Duration::from_secs(30);
        info!("Fork remediation service started (interval: 30s)");

        loop {
            tokio::time::sleep(interval).await;
            self.detect_and_resolve_forks().await;
            self.cleanup_old_data().await;
            self.log_summary().await;
        }
    }

    pub async fn index_transaction(&self, from: &str, nonce: u64, hash: &str) {
        let key = (from.to_string(), nonce);
        
        let should_detect = {
            let mut inner = self.inner.write().await;
            inner.nonce_index
                .entry(key.clone())
                .or_default()
                .push(hash.to_string());
            inner.nonce_index.get(&key).map(|h| h.len() > 1).unwrap_or(false)
        };

        if should_detect {
            self.detect_double_spend(from, nonce).await;
        }

        let should_check = {
            let mut count = self.new_txs_since_check.write().await;
            *count += 1;
            if *count >= FORK_CHECK_THRESHOLD {
                *count = 0;
                true
            } else {
                false
            }
        };

        if should_check {
            let service = self.clone();
            tokio::spawn(async move {
                service.detect_and_resolve_forks().await;
            });
        }
    }

    async fn detect_double_spend(&self, account: &str, nonce: u64) {
        let mut inner = self.inner.write().await;
        let key = (account.to_string(), nonce);
        
        let tx_hashes = inner.nonce_index.get(&key).cloned().unwrap_or_default();
        
        if tx_hashes.len() < 2 {
            return;
        }

        warn!(
            "Double-spend detected: account {} nonce {} has {} transactions",
            account, nonce, tx_hashes.len()
        );

        inner.stats.total_double_spends += 1;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        inner.double_spend_events.push(DoubleSpendEvent {
            account: account.to_string(),
            nonce,
            tx_hashes: tx_hashes.clone(),
            winner: None,
            detected_at: now,
            resolved: false,
        });

        drop(inner);

        let mut weights: Vec<(String, f64)> = Vec::new();
        for hash in &tx_hashes {
            let weight = self.state.calculate_cumulative_weight(hash).await;
            weights.push((hash.clone(), weight));
        }

        weights.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        if weights.len() >= 2 && weights[0].1 > weights[1].1 * WEIGHT_THRESHOLD {
            let winner = &weights[0].0;
            let loser = &weights[1].0;

            info!(
                "Resolving double-spend: winner {} (weight {:.2}) vs loser {} (weight {:.2})",
                &winner[..16.min(winner.len())],
                weights[0].1,
                &loser[..16.min(loser.len())],
                weights[1].1
            );

            if let Ok(pruned) = self.state.prune_losing_branch(loser).await {
                let mut inner = self.inner.write().await;
                inner.stats.total_txs_pruned += pruned as u64;
                inner.stats.total_branches_pruned += 1;

                if let Some(event) = inner.double_spend_events.iter_mut()
                    .find(|e| e.account == account && e.nonce == nonce && !e.resolved)
                {
                    event.winner = Some(winner.clone());
                    event.resolved = true;
                }
            }
        }
    }

    async fn detect_and_resolve_forks(&self) {
        let tips = self.state.get_tips().await;

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

                let should_analyze = {
                    let inner = self.inner.read().await;
                    !inner.analyzed_pairs.contains(&pair)
                };

                if !should_analyze {
                    continue;
                }

                if let Some((winner, loser, weight_a, weight_b)) = 
                    self.state.resolve_fork(tip_a, tip_b).await 
                {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs();

                    let fork_id = format!("{}:{}", &pair.0[..8.min(pair.0.len())], &pair.1[..8.min(pair.1.len())]);

                    if weight_a > weight_b * WEIGHT_THRESHOLD || weight_b > weight_a * WEIGHT_THRESHOLD {
                        info!(
                            "Fork detected and resolved: {} wins over {} ({:.2} vs {:.2})",
                            &winner[..16.min(winner.len())],
                            &loser[..16.min(loser.len())],
                            weight_a.max(weight_b),
                            weight_a.min(weight_b)
                        );

                        let pruned_count = if let Ok(pruned) = self.state.prune_losing_branch(&loser).await {
                            pruned
                        } else {
                            0
                        };

                        let mut inner = self.inner.write().await;
                        inner.analyzed_pairs.insert(pair);
                        inner.stats.total_forks_detected += 1;
                        inner.stats.total_forks_resolved += 1;
                        inner.stats.total_txs_pruned += pruned_count as u64;
                        inner.stats.total_branches_pruned += 1;

                        inner.fork_events.push(ForkEvent {
                            fork_id,
                            tip_a: tip_a.clone(),
                            tip_b: tip_b.clone(),
                            weight_a,
                            weight_b,
                            winner: Some(winner),
                            loser: Some(loser),
                            detected_at: now,
                            resolved_at: Some(now),
                            pruned_count,
                        });
                    } else {
                        let mut inner = self.inner.write().await;
                        inner.analyzed_pairs.insert(pair);
                        inner.stats.total_forks_detected += 1;

                        inner.fork_events.push(ForkEvent {
                            fork_id,
                            tip_a: tip_a.clone(),
                            tip_b: tip_b.clone(),
                            weight_a,
                            weight_b,
                            winner: None,
                            loser: None,
                            detected_at: now,
                            resolved_at: None,
                            pruned_count: 0,
                        });
                    }
                }
            }
        }

        let mut inner = self.inner.write().await;
        if inner.analyzed_pairs.len() > 500 {
            inner.analyzed_pairs.clear();
        }
    }

    async fn cleanup_old_data(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut inner = self.inner.write().await;

        inner.fork_events.retain(|e| {
            now - e.detected_at < 300 || e.resolved_at.is_none()
        });

        inner.double_spend_events.retain(|e| {
            now - e.detected_at < 300 || !e.resolved
        });

        if inner.fork_events.len() > 1000 {
            inner.fork_events.truncate(500);
        }

        if inner.double_spend_events.len() > 1000 {
            inner.double_spend_events.truncate(500);
        }

        const MAX_NONCE_INDEX_SIZE: usize = 50000;
        if inner.nonce_index.len() > MAX_NONCE_INDEX_SIZE {
            let entries_to_remove = inner.nonce_index.len() - (MAX_NONCE_INDEX_SIZE / 2);
            let keys_to_remove: Vec<_> = inner.nonce_index.keys().take(entries_to_remove).cloned().collect();
            for key in keys_to_remove {
                inner.nonce_index.remove(&key);
            }
            debug!("Pruned nonce_index: removed {} entries, {} remaining", entries_to_remove, inner.nonce_index.len());
        }

        inner.stats.active_forks = inner.fork_events.iter()
            .filter(|e| e.resolved_at.is_none())
            .count();
    }

    async fn log_summary(&self) {
        let inner = self.inner.read().await;
        let active_forks = inner.fork_events.iter().filter(|e| e.resolved_at.is_none()).count();
        
        if active_forks > 0 || inner.stats.total_forks_resolved > 0 {
            info!(
                "[ForkRemediation] Active: {}, Resolved: {}, Pruned TXs: {}, Double-spends: {}",
                active_forks,
                inner.stats.total_forks_resolved,
                inner.stats.total_txs_pruned,
                inner.stats.total_double_spends
            );
        }
    }

    pub async fn get_stats(&self) -> ForkStats {
        self.inner.read().await.stats.clone()
    }

    pub async fn get_active_forks(&self) -> Vec<ForkEvent> {
        self.inner.read().await
            .fork_events
            .iter()
            .filter(|e| e.resolved_at.is_none())
            .cloned()
            .collect()
    }

    pub async fn get_recent_events(&self, limit: usize) -> Vec<ForkEvent> {
        let inner = self.inner.read().await;
        inner.fork_events.iter().rev().take(limit).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fork_event_serialization() {
        let event = ForkEvent {
            fork_id: "abc:def".to_string(),
            tip_a: "abc123".to_string(),
            tip_b: "def456".to_string(),
            weight_a: 10.5,
            weight_b: 5.2,
            winner: Some("abc123".to_string()),
            loser: Some("def456".to_string()),
            detected_at: 1000,
            resolved_at: Some(1001),
            pruned_count: 3,
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: ForkEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.fork_id, "abc:def");
        assert_eq!(deserialized.weight_a, 10.5);
        assert_eq!(deserialized.pruned_count, 3);
    }

    #[test]
    fn test_double_spend_event_serialization() {
        let event = DoubleSpendEvent {
            account: "alice".to_string(),
            nonce: 5,
            tx_hashes: vec!["tx1".to_string(), "tx2".to_string()],
            winner: Some("tx1".to_string()),
            detected_at: 2000,
            resolved: true,
        };

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: DoubleSpendEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.account, "alice");
        assert_eq!(deserialized.nonce, 5);
        assert_eq!(deserialized.tx_hashes.len(), 2);
        assert!(deserialized.resolved);
    }

    #[test]
    fn test_fork_stats_default() {
        let stats = ForkStats::default();

        assert_eq!(stats.total_forks_detected, 0);
        assert_eq!(stats.total_forks_resolved, 0);
        assert_eq!(stats.total_double_spends, 0);
        assert_eq!(stats.total_branches_pruned, 0);
        assert_eq!(stats.total_txs_pruned, 0);
        assert_eq!(stats.active_forks, 0);
    }

    #[test]
    fn test_weight_threshold() {
        assert_eq!(WEIGHT_THRESHOLD, 1.5);
        
        let weight_a = 16.0;
        let weight_b = 10.0;
        assert!(weight_a > weight_b * WEIGHT_THRESHOLD);

        let weight_c = 14.0;
        let weight_d = 10.0;
        assert!(!(weight_c > weight_d * WEIGHT_THRESHOLD));
    }
}
