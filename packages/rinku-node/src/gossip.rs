use anyhow::Result;
use rinku_core::types::SignedTransaction;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::state::NodeState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GossipMessage {
    Transaction {
        hash: String,
        tx: SignedTransaction,
    },
    TipAnnouncement {
        tips: Vec<String>,
        tip_urls: Vec<String>,
        dag_size: usize,
        merkle_root: String,
    },
    CheckpointSignature {
        checkpoint_id: String,
        height: u64,
        validator_address: String,
        signature: String,
        weight: f64,
    },
    PeerDiscovery {
        peers: Vec<String>,
        node_id: String,
    },
    ConflictResolution {
        conflict_id: String,
        tx_hash_1: String,
        tx_hash_2: String,
        winner_hash: String,
        weight_1: f64,
        weight_2: f64,
        resolved_by: String,
    },
    SyncRequest {
        from_checkpoint: u64,
        missing_hashes: Vec<String>,
    },
    SyncResponse {
        transactions: Vec<SignedTransaction>,
        checkpoint_height: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub address: String,
    pub node_id: String,
    pub last_seen: u64,
    pub dag_size: usize,
    pub checkpoint_height: u64,
    pub latency_ms: u64,
    pub is_healthy: bool,
}

pub struct GossipServiceInner {
    pub peers: HashMap<String, PeerInfo>,
    pub known_txs: HashSet<String>,
    pub pending_txs: Vec<SignedTransaction>,
    pub seen_conflicts: HashSet<String>,
    pub stats: GossipStats,
}

#[derive(Debug, Clone, Default)]
pub struct GossipStats {
    pub txs_propagated: u64,
    pub txs_received: u64,
    pub peers_discovered: u64,
    pub conflicts_resolved: u64,
    pub sync_requests: u64,
    pub failed_sends: u64,
}

pub struct GossipService {
    state: NodeState,
    inner: Arc<RwLock<GossipServiceInner>>,
    node_id: String,
    interval_ms: u64,
}

impl GossipService {
    pub fn new(state: NodeState, initial_peers: Vec<String>, interval_ms: u64) -> Self {
        let node_id = format!("{:016x}", rand::random::<u64>());
        
        let mut peers = HashMap::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
            
        for addr in initial_peers {
            peers.insert(addr.clone(), PeerInfo {
                address: addr,
                node_id: String::new(),
                last_seen: now,
                dag_size: 0,
                checkpoint_height: 0,
                latency_ms: 0,
                is_healthy: true,
            });
        }

        Self {
            state,
            inner: Arc::new(RwLock::new(GossipServiceInner {
                peers,
                known_txs: HashSet::new(),
                pending_txs: Vec::new(),
                seen_conflicts: HashSet::new(),
                stats: GossipStats::default(),
            })),
            node_id,
            interval_ms,
        }
    }

    pub async fn start(self) -> Result<()> {
        info!(
            "Gossip service started (interval: {}ms, peers: {})",
            self.interval_ms,
            self.inner.read().await.peers.len()
        );

        let mut tick = interval(Duration::from_millis(self.interval_ms));

        loop {
            tick.tick().await;
            self.gossip_round().await;
        }
    }

    async fn gossip_round(&self) {
        let peers: Vec<String> = {
            let inner = self.inner.read().await;
            inner.peers.keys().cloned().collect()
        };

        if peers.is_empty() {
            return;
        }

        let tips = self.state.get_tips().await;
        let (dag_size, _, _) = self.state.get_dag_stats().await;
        let checkpoint_height = self.state.get_checkpoint_height().await;

        let tip_urls: Vec<String> = tips
            .iter()
            .map(|h| format!("rinku://tx/h/{}", h))
            .collect();

        let merkle_root = self.state.get_dag_merkle_root().await.unwrap_or_default();

        let message = GossipMessage::TipAnnouncement {
            tips: tips.clone(),
            tip_urls,
            dag_size,
            merkle_root,
        };

        for peer in &peers {
            if let Err(e) = self.send_to_peer(peer, &message).await {
                debug!("Failed to gossip tips to {}: {}", peer, e);
                let mut inner = self.inner.write().await;
                inner.stats.failed_sends += 1;
                if let Some(peer_info) = inner.peers.get_mut(peer) {
                    peer_info.is_healthy = false;
                }
            }
        }

        self.propagate_pending_txs().await;
        self.request_sync_if_needed(checkpoint_height).await;
    }

    async fn propagate_pending_txs(&self) {
        let (pending_txs, peers): (Vec<SignedTransaction>, Vec<String>) = {
            let mut inner = self.inner.write().await;
            let txs = std::mem::take(&mut inner.pending_txs);
            let peer_addrs = inner.peers.keys().cloned().collect();
            (txs, peer_addrs)
        };

        for tx in pending_txs {
            let message = GossipMessage::Transaction {
                hash: tx.hash.clone(),
                tx: tx.clone(),
            };

            for peer in &peers {
                if let Err(e) = self.send_to_peer(peer, &message).await {
                    debug!("Failed to propagate tx to {}: {}", peer, e);
                }
            }

            let mut inner = self.inner.write().await;
            inner.stats.txs_propagated += 1;
            inner.known_txs.insert(tx.hash.clone());
        }
    }

    async fn request_sync_if_needed(&self, local_checkpoint: u64) {
        let peers: Vec<(String, u64)> = {
            let inner = self.inner.read().await;
            inner
                .peers
                .iter()
                .filter(|(_, p)| p.is_healthy && p.checkpoint_height > local_checkpoint)
                .map(|(addr, p)| (addr.clone(), p.checkpoint_height))
                .collect()
        };

        for (peer, _remote_height) in peers {
            let message = GossipMessage::SyncRequest {
                from_checkpoint: local_checkpoint,
                missing_hashes: Vec::new(),
            };

            if let Err(e) = self.send_to_peer(&peer, &message).await {
                debug!("Failed to request sync from {}: {}", peer, e);
            } else {
                let mut inner = self.inner.write().await;
                inner.stats.sync_requests += 1;
            }
        }
    }

    async fn send_to_peer(&self, peer: &str, message: &GossipMessage) -> Result<()> {
        let client = reqwest::Client::new();
        let url = format!("{}/api/gossip", peer);

        let start = std::time::Instant::now();
        let response = client
            .post(&url)
            .json(message)
            .timeout(Duration::from_secs(5))
            .send()
            .await?;

        let latency = start.elapsed().as_millis() as u64;

        if response.status().is_success() {
            let mut inner = self.inner.write().await;
            if let Some(peer_info) = inner.peers.get_mut(peer) {
                peer_info.latency_ms = latency;
                peer_info.last_seen = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                peer_info.is_healthy = true;
            }
        }

        Ok(())
    }

    pub async fn handle_message(&self, message: GossipMessage) -> Result<Option<GossipMessage>> {
        match message {
            GossipMessage::Transaction { hash, tx } => {
                let is_new = {
                    let mut inner = self.inner.write().await;
                    if inner.known_txs.contains(&hash) {
                        false
                    } else {
                        inner.known_txs.insert(hash.clone());
                        inner.stats.txs_received += 1;
                        true
                    }
                };

                if is_new {
                    if let Err(e) = self.state.add_transaction(tx.clone()).await {
                        warn!("Failed to add gossiped tx {}: {}", hash, e);
                    } else {
                        let mut inner = self.inner.write().await;
                        inner.pending_txs.push(tx);
                    }
                }
                Ok(None)
            }

            GossipMessage::TipAnnouncement {
                dag_size,
                ..
            } => {
                debug!("Received tip announcement, peer dag_size: {}", dag_size);
                Ok(None)
            }

            GossipMessage::PeerDiscovery { peers, node_id } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();

                let mut inner = self.inner.write().await;
                for addr in peers {
                    if !inner.peers.contains_key(&addr) {
                        inner.peers.insert(addr.clone(), PeerInfo {
                            address: addr,
                            node_id: node_id.clone(),
                            last_seen: now,
                            dag_size: 0,
                            checkpoint_height: 0,
                            latency_ms: 0,
                            is_healthy: true,
                        });
                        inner.stats.peers_discovered += 1;
                    }
                }
                Ok(None)
            }

            GossipMessage::ConflictResolution {
                conflict_id,
                winner_hash,
                ..
            } => {
                let mut inner = self.inner.write().await;
                if !inner.seen_conflicts.contains(&conflict_id) {
                    inner.seen_conflicts.insert(conflict_id);
                    inner.stats.conflicts_resolved += 1;
                    debug!("Conflict resolved, winner: {}", winner_hash);
                }
                Ok(None)
            }

            GossipMessage::SyncRequest {
                from_checkpoint,
                missing_hashes,
            } => {
                let txs = self.state.get_txs_since_checkpoint(from_checkpoint, &missing_hashes).await;
                let checkpoint_height = self.state.get_checkpoint_height().await;

                Ok(Some(GossipMessage::SyncResponse {
                    transactions: txs,
                    checkpoint_height,
                }))
            }

            GossipMessage::SyncResponse {
                transactions,
                ..
            } => {
                for tx in transactions {
                    let hash = tx.hash.clone();
                    let mut inner = self.inner.write().await;
                    if !inner.known_txs.contains(&hash) {
                        inner.known_txs.insert(hash.clone());
                        drop(inner);
                        if let Err(e) = self.state.add_transaction(tx).await {
                            debug!("Failed to add synced tx {}: {}", hash, e);
                        }
                    }
                }
                Ok(None)
            }

            GossipMessage::CheckpointSignature { .. } => {
                Ok(None)
            }
        }
    }

    pub async fn broadcast_transaction(&self, tx: SignedTransaction) {
        let mut inner = self.inner.write().await;
        if !inner.known_txs.contains(&tx.hash) {
            inner.known_txs.insert(tx.hash.clone());
            inner.pending_txs.push(tx);
        }
    }

    pub async fn broadcast_conflict_resolution(
        &self,
        tx_hash_1: &str,
        tx_hash_2: &str,
        winner_hash: &str,
        weight_1: f64,
        weight_2: f64,
    ) {
        let conflict_id = format!("{}:{}", tx_hash_1, tx_hash_2);
        
        {
            let mut inner = self.inner.write().await;
            if inner.seen_conflicts.contains(&conflict_id) {
                return;
            }
            inner.seen_conflicts.insert(conflict_id.clone());
        }

        let message = GossipMessage::ConflictResolution {
            conflict_id,
            tx_hash_1: tx_hash_1.to_string(),
            tx_hash_2: tx_hash_2.to_string(),
            winner_hash: winner_hash.to_string(),
            weight_1,
            weight_2,
            resolved_by: self.node_id.clone(),
        };

        let peers: Vec<String> = {
            let inner = self.inner.read().await;
            inner.peers.keys().cloned().collect()
        };

        for peer in &peers {
            let _ = self.send_to_peer(peer, &message).await;
        }
    }

    pub async fn get_stats(&self) -> GossipStats {
        self.inner.read().await.stats.clone()
    }

    pub async fn get_peer_count(&self) -> usize {
        self.inner.read().await.peers.len()
    }

    pub async fn get_healthy_peer_count(&self) -> usize {
        self.inner
            .read()
            .await
            .peers
            .values()
            .filter(|p| p.is_healthy)
            .count()
    }

    pub async fn add_peer(&self, address: String) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut inner = self.inner.write().await;
        if !inner.peers.contains_key(&address) {
            inner.peers.insert(address.clone(), PeerInfo {
                address,
                node_id: String::new(),
                last_seen: now,
                dag_size: 0,
                checkpoint_height: 0,
                latency_ms: 0,
                is_healthy: true,
            });
        }
    }

    pub async fn remove_peer(&self, address: &str) {
        let mut inner = self.inner.write().await;
        inner.peers.remove(address);
    }

    pub fn mark_tx_known(&self, _hash: &str) {
    }

    pub fn is_tx_known(&self, _hash: &str) -> bool {
        false
    }
}
