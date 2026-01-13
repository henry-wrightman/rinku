use anyhow::Result;
use rinku_core::types::{Account, Checkpoint, SignedTransaction};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::state::{NodeState, SyncSnapshot};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeerSyncStatus {
    checkpoint_height: u64,
    dag_size: usize,
    tip_count: usize,
    #[allow(dead_code)]
    tips: Vec<String>,
    #[allow(dead_code)]
    merkle_root: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapResponse {
    transactions: Vec<SignedTransaction>,
    checkpoint_height: u64,
    #[allow(dead_code)]
    total_available: usize,
    has_more: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotSyncResponse {
    accounts: HashMap<String, Account>,
    validators: HashMap<String, rinku_core::types::Validator>,
    checkpoints: Vec<Checkpoint>,
    gas_price: f64,
    total_supply: f64,
    genesis_time: u64,
    dag_transactions: Vec<SignedTransaction>,
    total_transactions: u64,
    checkpoint_height: u64,
}

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
    pub round_counter: u64,
    pub last_peer_refresh: u64,
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
                round_counter: 0,
                last_peer_refresh: now,
            })),
            node_id,
            interval_ms,
        }
    }

    pub async fn start(self) -> Result<()> {
        let peer_count = self.inner.read().await.peers.len();
        info!(
            "Gossip service started (interval: {}ms, peers: {})",
            self.interval_ms,
            peer_count
        );

        if peer_count > 0 {
            self.initial_sync().await;
        }

        let mut tick = interval(Duration::from_millis(self.interval_ms));

        loop {
            tick.tick().await;
            self.gossip_round().await;
        }
    }

    async fn initial_sync(&self) {
        info!("Starting initial sync with peers...");
        
        let peers: Vec<String> = {
            let inner = self.inner.read().await;
            inner.peers.keys().cloned().collect()
        };

        for peer in &peers {
            info!("Fetching sync status from peer: {}", peer);
            
            match self.fetch_peer_status(peer).await {
                Ok(status) => {
                    info!(
                        "Peer {} status: checkpoint_height={}, dag_size={}, tips={}",
                        peer, status.checkpoint_height, status.dag_size, status.tip_count
                    );

                    let mut inner = self.inner.write().await;
                    if let Some(peer_info) = inner.peers.get_mut(peer) {
                        peer_info.checkpoint_height = status.checkpoint_height;
                        peer_info.dag_size = status.dag_size;
                        peer_info.is_healthy = true;
                    }
                    drop(inner);

                    let local_height = self.state.get_checkpoint_height().await;
                    if status.checkpoint_height > local_height || status.dag_size > 1 {
                        info!("Peer has more data, requesting bootstrap sync...");
                        if let Err(e) = self.bootstrap_from_peer(peer, local_height).await {
                            warn!("Bootstrap sync from {} failed: {}", peer, e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to get status from peer {}: {}", peer, e);
                    let mut inner = self.inner.write().await;
                    if let Some(peer_info) = inner.peers.get_mut(peer) {
                        peer_info.is_healthy = false;
                    }
                }
            }
        }
        
        info!("Initial sync complete");
    }

    async fn fetch_peer_status(&self, peer: &str) -> Result<PeerSyncStatus> {
        let client = reqwest::Client::new();
        let url = format!("{}/api/sync/status", peer);
        
        let response = client
            .get(&url)
            .timeout(Duration::from_secs(10))
            .send()
            .await?;
        
        if !response.status().is_success() {
            anyhow::bail!("Peer returned status {}", response.status());
        }
        
        let status: PeerSyncStatus = response.json().await?;
        Ok(status)
    }

    async fn bootstrap_from_peer(&self, peer: &str, _from_checkpoint: u64) -> Result<()> {
        let client = reqwest::Client::new();
        
        // Use snapshot-based sync: get complete state snapshot from peer
        // This is efficient because it transfers derived state (accounts) 
        // instead of full transaction history
        let url = format!("{}/api/sync/snapshot", peer);
        
        info!("Requesting snapshot sync from peer: {}", peer);
        
        let response = client
            .get(&url)
            .timeout(Duration::from_secs(60))
            .send()
            .await?;
        
        if !response.status().is_success() {
            anyhow::bail!("Snapshot sync request failed with status {}", response.status());
        }
        
        let snapshot_response: SnapshotSyncResponse = response.json().await?;
        
        info!(
            "Received snapshot: {} accounts, {} checkpoints, {} dag txs, checkpoint height {}",
            snapshot_response.accounts.len(),
            snapshot_response.checkpoints.len(),
            snapshot_response.dag_transactions.len(),
            snapshot_response.checkpoint_height
        );

        // Convert response to SyncSnapshot and apply
        let snapshot = SyncSnapshot {
            accounts: snapshot_response.accounts,
            validators: snapshot_response.validators,
            checkpoints: snapshot_response.checkpoints,
            gas_price: snapshot_response.gas_price,
            total_supply: snapshot_response.total_supply,
            genesis_time: snapshot_response.genesis_time,
            dag_transactions: snapshot_response.dag_transactions,
            total_transactions: snapshot_response.total_transactions,
        };

        let added = self.state.apply_sync_snapshot(snapshot).await?;
        
        info!("Snapshot sync complete: applied {} DAG transactions", added);

        let mut inner = self.inner.write().await;
        inner.stats.sync_requests += 1;
        
        Ok(())
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

        // Increment round counter and check if we should refresh peer status
        // Refresh every 50 rounds (~10 seconds at 200ms interval)
        let should_refresh = {
            let mut inner = self.inner.write().await;
            inner.round_counter += 1;
            inner.round_counter % 50 == 0
        };

        if should_refresh {
            self.refresh_peer_status_and_sync().await;
        }

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

        let mut success_count = 0;
        let mut fail_count = 0;
        
        for peer in &peers {
            match self.send_to_peer(peer, &message).await {
                Ok(_) => {
                    success_count += 1;
                }
                Err(e) => {
                    fail_count += 1;
                    debug!("Gossip to {} failed: {}", peer, e);
                    let mut inner = self.inner.write().await;
                    inner.stats.failed_sends += 1;
                    if let Some(peer_info) = inner.peers.get_mut(peer) {
                        peer_info.is_healthy = false;
                    }
                }
            }
        }

        if success_count > 0 || fail_count > 0 {
            debug!(
                "Gossip round: {} tips announced to {} peers ({} failed), dag_size={}",
                tips.len(), success_count, fail_count, dag_size
            );
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

    async fn refresh_peer_status_and_sync(&self) {
        let peers: Vec<String> = {
            let inner = self.inner.read().await;
            inner.peers.keys().cloned().collect()
        };

        for peer in &peers {
            // Re-check local state before each peer to avoid redundant syncs
            let local_checkpoint = self.state.get_checkpoint_height().await;
            let (local_dag_size, _, _) = self.state.get_dag_stats().await;

            match self.fetch_peer_status(peer).await {
                Ok(status) => {
                    // Update peer info with fresh data
                    {
                        let mut inner = self.inner.write().await;
                        if let Some(peer_info) = inner.peers.get_mut(peer) {
                            peer_info.checkpoint_height = status.checkpoint_height;
                            peer_info.dag_size = status.dag_size;
                            peer_info.is_healthy = true;
                            peer_info.last_seen = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs();
                        }
                    }

                    // Check if peer has more data than us
                    let needs_sync = status.checkpoint_height > local_checkpoint 
                        || status.dag_size > local_dag_size;

                    if needs_sync {
                        info!(
                            "Peer {} has more data (cp: {} vs {}, dag: {} vs {}), requesting sync...",
                            peer, status.checkpoint_height, local_checkpoint, 
                            status.dag_size, local_dag_size
                        );
                        
                        // Request missing transactions via delta sync
                        if let Err(e) = self.sync_from_peer(peer, local_checkpoint).await {
                            warn!("Sync from {} failed: {}", peer, e);
                        }
                    }
                }
                Err(e) => {
                    debug!("Failed to refresh status from {}: {}", peer, e);
                    let mut inner = self.inner.write().await;
                    if let Some(peer_info) = inner.peers.get_mut(peer) {
                        peer_info.is_healthy = false;
                    }
                }
            }
        }
    }

    async fn sync_from_peer(&self, peer: &str, local_checkpoint: u64) -> Result<()> {
        let client = reqwest::Client::new();
        
        // Fetch transactions since our last checkpoint using delta sync endpoint
        let url = format!("{}/api/sync/delta?from_checkpoint={}", peer, local_checkpoint);
        
        let response = client
            .get(&url)
            .timeout(Duration::from_secs(30))
            .send()
            .await?;
        
        if !response.status().is_success() {
            // Fall back to fetching specific missing txs via gossip
            anyhow::bail!("Sync request failed with status {}", response.status());
        }
        
        let txs: Vec<SignedTransaction> = response.json().await?;
        
        if txs.is_empty() {
            return Ok(());
        }
        
        info!("Received {} transactions from peer {}", txs.len(), peer);
        
        let mut added = 0;
        for tx in txs {
            // Check if we already have this tx
            let is_known = {
                let inner = self.inner.read().await;
                inner.known_txs.contains(&tx.hash)
            };
            
            if !is_known {
                if let Err(e) = self.state.add_transaction(tx.clone()).await {
                    debug!("Failed to add synced tx {}: {}", tx.hash, e);
                } else {
                    let mut inner = self.inner.write().await;
                    inner.known_txs.insert(tx.hash.clone());
                    inner.stats.txs_received += 1;
                    added += 1;
                }
            }
        }
        
        if added > 0 {
            info!("Added {} new transactions from peer sync", added);
        }
        
        Ok(())
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
