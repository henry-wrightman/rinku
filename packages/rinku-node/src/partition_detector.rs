use std::sync::Arc;
use tracing::{debug, info};

use crate::events::{EventBus, NodeEvent};
use crate::gossip::GossipService;
use crate::state::partition::PartitionStatus;
use crate::state::NodeState;

const DEFAULT_CONFIRMATION_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_RECOVERY_WINDOW_MS: u64 = 10_000;
const DEFAULT_STAKE_VISIBILITY_THRESHOLD: f64 = 0.6666;
const DEFAULT_CHECKPOINT_STALL_MULTIPLIER: u64 = 3;
const DETECTION_INTERVAL_MS: u64 = 5_000;

pub struct PartitionDetectorConfig {
    pub confirmation_timeout_ms: u64,
    pub recovery_window_ms: u64,
    pub stake_visibility_threshold: f64,
    pub checkpoint_stall_multiplier: u64,
    pub checkpoint_interval_ms: u64,
}

impl Default for PartitionDetectorConfig {
    fn default() -> Self {
        Self {
            confirmation_timeout_ms: DEFAULT_CONFIRMATION_TIMEOUT_MS,
            recovery_window_ms: DEFAULT_RECOVERY_WINDOW_MS,
            stake_visibility_threshold: DEFAULT_STAKE_VISIBILITY_THRESHOLD,
            checkpoint_stall_multiplier: DEFAULT_CHECKPOINT_STALL_MULTIPLIER,
            checkpoint_interval_ms: 15_000,
        }
    }
}

pub struct PartitionDetector {
    state: NodeState,
    gossip_service: Option<Arc<GossipService>>,
    event_bus: Option<Arc<EventBus>>,
    config: PartitionDetectorConfig,
    recovery_started_at: Option<u64>,
}

impl PartitionDetector {
    pub fn new(state: NodeState, config: PartitionDetectorConfig) -> Self {
        Self {
            state,
            gossip_service: None,
            event_bus: None,
            config,
            recovery_started_at: None,
        }
    }

    pub fn with_gossip_service(mut self, gossip: Arc<GossipService>) -> Self {
        self.gossip_service = Some(gossip);
        self
    }

    pub fn with_event_bus(mut self, event_bus: Arc<EventBus>) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    pub async fn start(mut self) {
        let interval = tokio::time::Duration::from_millis(DETECTION_INTERVAL_MS);
        info!("Partition detector started (interval: {}ms, confirmation timeout: {}ms, recovery window: {}ms)",
            DETECTION_INTERVAL_MS, self.config.confirmation_timeout_ms, self.config.recovery_window_ms);

        loop {
            tokio::time::sleep(interval).await;
            self.detect_cycle().await;
        }
    }

    async fn detect_cycle(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let (visible_validators, visible_stake_pct) = self.compute_visibility().await;

        self.state
            .update_partition_visibility(visible_validators.clone(), visible_stake_pct)
            .await;

        let current_status = {
            let ps = self.state.get_partition_state().await;
            ps.status
        };

        match current_status {
            PartitionStatus::Normal => {
                if visible_stake_pct < self.config.stake_visibility_threshold {
                    self.state.transition_to_suspected().await;
                    self.recovery_started_at = None;
                    if let Some(ref bus) = self.event_bus {
                        let known_validators = self.get_known_validators().await;
                        let missing: Vec<String> = known_validators
                            .into_iter()
                            .filter(|v| !visible_validators.contains(v))
                            .collect();
                        bus.publish(NodeEvent::PartitionSuspected {
                            visible_stake_pct,
                            missing_validators: missing,
                        });
                    }
                }
            }
            PartitionStatus::Suspected => {
                if visible_stake_pct >= self.config.stake_visibility_threshold {
                    self.state.transition_to_normal().await;
                    self.recovery_started_at = None;
                } else {
                    let ps = self.state.get_partition_state().await;
                    if let Some(suspected_since) = ps.suspected_since {
                        if now - suspected_since >= self.config.confirmation_timeout_ms {
                            let epoch = self.state.transition_to_partitioned().await;
                            if let Some(ref bus) = self.event_bus {
                                bus.publish(NodeEvent::PartitionConfirmed {
                                    epoch,
                                    visible_validators: visible_validators.clone(),
                                });
                            }
                        }
                    }
                }
            }
            PartitionStatus::Partitioned => {
                if visible_stake_pct >= self.config.stake_visibility_threshold {
                    match self.recovery_started_at {
                        None => {
                            self.recovery_started_at = Some(now);
                            debug!(
                                "Partition recovery candidate: visible stake at {:.1}%",
                                visible_stake_pct * 100.0
                            );
                        }
                        Some(started) => {
                            if now - started >= self.config.recovery_window_ms {
                                let ps = self.state.get_partition_state().await;
                                info!(
                                    "Partition healed: quorum restored for {}ms (epoch {} complete)",
                                    self.config.recovery_window_ms,
                                    ps.current_epoch.unwrap_or(0)
                                );
                                let fork_point = {
                                    let ps = self.state.get_partition_state().await;
                                    ps.epoch_start_checkpoint.unwrap_or(0)
                                };
                                self.state.transition_to_normal().await;
                                self.recovery_started_at = None;
                                if let Some(ref bus) = self.event_bus {
                                    bus.publish(NodeEvent::PartitionHealed {
                                        visible_validators: visible_validators.clone(),
                                    });
                                }
                                if let Some(ref gossip) = self.gossip_service {
                                    info!("Auto-triggering merge payload exchange (fork point checkpoint: {})", fork_point);
                                    gossip.send_merge_payload(fork_point).await;
                                }
                            }
                        }
                    }
                } else {
                    self.recovery_started_at = None;
                }
            }
        }
    }

    async fn compute_visibility(&self) -> (Vec<String>, f64) {
        let known_validators = self.get_known_validators().await;
        if known_validators.is_empty() {
            return (Vec::new(), 1.0);
        }

        let peer_validator_addresses = self.get_peer_validator_addresses().await;

        let state = self.state.inner.read().await;
        let total_stake: u64 = state.validators.values().map(|v| v.stake).sum();
        if total_stake == 0 {
            return (known_validators, 1.0);
        }

        let our_address = state.node_validator_address.clone();
        let validator_addrs: Vec<String> = state.validators.keys().cloned().collect();
        tracing::debug!(
            "Partition visibility check: our_addr={:?}, peer_validator_addrs={:?}, known_validators={:?}",
            our_address, peer_validator_addresses, validator_addrs
        );

        let mut visible_validators = Vec::new();
        let mut visible_stake: u64 = 0;

        for (addr, validator) in &state.validators {
            let is_us = our_address.as_ref().map(|a| a == addr).unwrap_or(false);
            let is_reachable = peer_validator_addresses.iter().any(|va| va == addr);

            if is_us || is_reachable {
                visible_validators.push(addr.clone());
                visible_stake += validator.stake;
            }
        }

        let visible_pct = if total_stake > 0 {
            visible_stake as f64 / total_stake as f64
        } else {
            1.0
        };

        (visible_validators, visible_pct)
    }

    async fn get_known_validators(&self) -> Vec<String> {
        let state = self.state.inner.read().await;
        state.validators.keys().cloned().collect()
    }

    async fn get_peer_validator_addresses(&self) -> Vec<String> {
        if let Some(ref gossip) = self.gossip_service {
            let p2p_peers = gossip.get_p2p_peers().await;
            p2p_peers
                .iter()
                .filter_map(|p| {
                    p.handshake_info
                        .as_ref()
                        .and_then(|h| h.validator_address.clone())
                })
                .collect()
        } else {
            Vec::new()
        }
    }
}
