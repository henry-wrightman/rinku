use anyhow::Result;
use rinku_core::types::{Account, Checkpoint, SignedTransaction};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

const KNOWN_TXS_MAX_SIZE: usize = 50_000;
const SEEN_CONFLICTS_MAX_SIZE: usize = 10_000;

pub(crate) struct BoundedHashSet {
    set: HashSet<String>,
    order: VecDeque<String>,
    max_size: usize,
}

impl BoundedHashSet {
    pub fn new(max_size: usize) -> Self {
        Self {
            set: HashSet::with_capacity(max_size),
            order: VecDeque::with_capacity(max_size),
            max_size,
        }
    }

    pub fn insert(&mut self, value: String) -> bool {
        if self.set.contains(&value) {
            return false;
        }
        
        while self.set.len() >= self.max_size {
            if let Some(oldest) = self.order.pop_front() {
                self.set.remove(&oldest);
            }
        }
        
        self.set.insert(value.clone());
        self.order.push_back(value);
        true
    }

    pub fn contains(&self, value: &str) -> bool {
        self.set.contains(value)
    }

    pub fn clear(&mut self) {
        self.set.clear();
        self.order.clear();
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }
}

use crate::config::TrustConfig;
use crate::state::{NodeState, SyncSnapshot};
use crate::trust::TrustVerifier;

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

#[derive(Debug, Default)]
struct DeltaSyncResult {
    added: usize,
    failed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GossipMessage {
    Transaction {
        hash: String,
        tx: SignedTransaction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    TipAnnouncement {
        tips: Vec<String>,
        tip_urls: Vec<String>,
        dag_size: usize,
        merkle_root: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    CheckpointSignature {
        checkpoint_id: String,
        height: u64,
        validator_address: String,
        signature: String,
        weight: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    PeerDiscovery {
        peers: Vec<String>,
        node_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    ConflictResolution {
        conflict_id: String,
        tx_hash_1: String,
        tx_hash_2: String,
        winner_hash: String,
        weight_1: f64,
        weight_2: f64,
        resolved_by: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    SyncRequest {
        from_checkpoint: u64,
        missing_hashes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    SyncResponse {
        transactions: Vec<SignedTransaction>,
        checkpoint_height: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
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
    pub known_txs: BoundedHashSet,
    pub pending_txs: Vec<SignedTransaction>,
    pub seen_conflicts: BoundedHashSet,
    pub stats: GossipStats,
    pub round_counter: u64,
    pub last_peer_refresh: u64,
    pub sync_in_flight: bool,
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

#[derive(Clone)]
pub struct GossipService {
    state: NodeState,
    inner: Arc<RwLock<GossipServiceInner>>,
    node_id: String,
    interval_ms: u64,
    trust_verifier: Arc<TrustVerifier>,
}

impl GossipService {
    pub fn new(state: NodeState, initial_peers: Vec<String>, interval_ms: u64, trust_config: TrustConfig) -> Self {
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
                known_txs: BoundedHashSet::new(KNOWN_TXS_MAX_SIZE),
                pending_txs: Vec::new(),
                seen_conflicts: BoundedHashSet::new(SEEN_CONFLICTS_MAX_SIZE),
                stats: GossipStats::default(),
                round_counter: 0,
                last_peer_refresh: now,
                sync_in_flight: false,
            })),
            node_id,
            interval_ms,
            trust_verifier: Arc::new(TrustVerifier::new(trust_config)),
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

        // Weak subjectivity: if a trust checkpoint hash is configured, verify it exists anywhere in the chain
        let mut found_trusted_checkpoint = false;
        for checkpoint in &snapshot_response.checkpoints {
            if self.trust_verifier.is_trusted_checkpoint(&checkpoint.hash) {
                info!(
                    "Weak subjectivity: verified trusted checkpoint hash at height {}",
                    checkpoint.height
                );
                found_trusted_checkpoint = true;
                break;
            }
        }
        
        if !found_trusted_checkpoint {
            if self.trust_verifier.has_genesis_validators() {
                // Verify checkpoint chain with stake-weighted BLS signatures
                if let Err(e) = self.trust_verifier.verify_checkpoint_chain(
                    &snapshot_response.checkpoints,
                    &snapshot_response.validators,
                ) {
                    anyhow::bail!("Checkpoint verification failed: {}", e);
                }
                info!(
                    "Verified {} checkpoints with stake-weighted BLS signatures",
                    snapshot_response.checkpoints.len()
                );
            } else {
                // No trust configuration - log warning for testnet mode
                warn!(
                    "No trust configuration set - accepting snapshot without verification (TESTNET MODE)"
                );
            }
        }

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

    /// Force bootstrap from peer - used for recovery when delta sync fails due to state divergence
    /// This forces the snapshot to be applied even if checkpoint counts are equal
    async fn bootstrap_from_peer_force(&self, peer: &str) -> Result<()> {
        let client = reqwest::Client::new();
        let url = format!("{}/api/sync/snapshot", peer);
        
        warn!("RECOVERY: Forcing snapshot sync from peer {} to fix state divergence", peer);
        
        let response = client
            .get(&url)
            .timeout(Duration::from_secs(60))
            .send()
            .await?;
        
        if !response.status().is_success() {
            anyhow::bail!("Snapshot sync request failed with status {}", response.status());
        }
        
        let snapshot_response: SnapshotSyncResponse = response.json().await?;
        
        warn!(
            "RECOVERY: Received snapshot: {} accounts, {} checkpoints, {} dag txs",
            snapshot_response.accounts.len(),
            snapshot_response.checkpoints.len(),
            snapshot_response.dag_transactions.len()
        );

        // Weak subjectivity check (same as regular bootstrap)
        if !self.trust_verifier.has_genesis_validators() {
            warn!("TESTNET MODE: Accepting snapshot without verification");
        }

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

        // Use force apply to bypass checkpoint count check
        let added = self.state.apply_sync_snapshot_force(snapshot).await?;
        
        warn!("RECOVERY: Force snapshot sync complete - applied {} DAG transactions", added);

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

        // Check if we should refresh peer status (every 10 seconds, time-based)
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let should_refresh = {
            let mut inner = self.inner.write().await;
            inner.round_counter += 1;
            let elapsed = now_secs.saturating_sub(inner.last_peer_refresh);
            if elapsed >= 10 {
                inner.last_peer_refresh = now_secs;
                true
            } else {
                false
            }
        };

        if should_refresh {
            debug!("Periodic sync: checking peer status (every 10s)");
            self.refresh_peer_status_and_sync().await;
        }

        // Cap tips to 10 to prevent bandwidth explosion when tip count is high
        // Peers only need a sample of tips for sync, not the full set
        const MAX_GOSSIP_TIPS: usize = 10;
        let tips_capped: Vec<String> = tips.iter().take(MAX_GOSSIP_TIPS).cloned().collect();
        let tip_urls: Vec<String> = tips_capped
            .iter()
            .map(|h| format!("rinku://tx/h/{}", h))
            .collect();

        let merkle_root = self.state.get_dag_merkle_root().await.unwrap_or_default();

        let public_url = std::env::var("PUBLIC_URL").ok();
        let tips_announced = tips_capped.len();
        let message = GossipMessage::TipAnnouncement {
            tips: tips_capped,
            tip_urls,
            dag_size,
            merkle_root,
            sender_url: public_url.clone(),
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
                "Gossip round: {}/{} tips announced to {} peers ({} failed), dag_size={}",
                tips_announced, tips.len(), success_count, fail_count, dag_size
            );
        }

        self.propagate_pending_txs().await;
        self.request_sync_if_needed(checkpoint_height).await;
        
        // Periodic peer exchange: every 30 seconds, share known peers
        let should_exchange_peers = {
            let inner = self.inner.read().await;
            inner.round_counter % 150 == 0  // ~30s at 200ms interval
        };
        if should_exchange_peers {
            self.broadcast_peer_list().await;
        }
    }
    
    async fn broadcast_peer_list(&self) {
        let (known_peers, current_peers): (Vec<String>, Vec<String>) = {
            let inner = self.inner.read().await;
            let known: Vec<String> = inner.peers.keys()
                .filter(|p| inner.peers.get(*p).map(|i| i.is_healthy).unwrap_or(false))
                .cloned()
                .collect();
            let current: Vec<String> = inner.peers.keys().cloned().collect();
            (known, current)
        };
        
        if known_peers.is_empty() {
            return;
        }
        
        let public_url = std::env::var("PUBLIC_URL").ok();
        let message = GossipMessage::PeerDiscovery {
            peers: known_peers,
            node_id: self.node_id.clone(),
            sender_url: public_url,
        };
        
        for peer in &current_peers {
            if let Err(e) = self.send_to_peer(peer, &message).await {
                debug!("Failed to share peer list with {}: {}", peer, e);
            }
        }
        
        info!("Broadcast peer list to {} peers", current_peers.len());
    }

    async fn propagate_pending_txs(&self) {
        let (pending_txs, peers): (Vec<SignedTransaction>, Vec<String>) = {
            let mut inner = self.inner.write().await;
            let txs = std::mem::take(&mut inner.pending_txs);
            let peer_addrs = inner.peers.keys().cloned().collect();
            (txs, peer_addrs)
        };

        let public_url = std::env::var("PUBLIC_URL").ok();
        for tx in pending_txs {
            let message = GossipMessage::Transaction {
                hash: tx.hash.clone(),
                tx: tx.clone(),
                sender_url: public_url.clone(),
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

        let public_url = std::env::var("PUBLIC_URL").ok();
        for (peer, _remote_height) in peers {
            let message = GossipMessage::SyncRequest {
                from_checkpoint: local_checkpoint,
                missing_hashes: Vec::new(),
                sender_url: public_url.clone(),
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
                        // SYNC STRATEGY: Always use snapshot sync when behind on checkpoints
                        // Delta sync only transfers transactions, NOT account state (nonces/balances)
                        // This causes validation failures when nonces don't match
                        // Snapshot sync is more reliable and includes complete derived state
                        let checkpoint_gap = status.checkpoint_height.saturating_sub(local_checkpoint);
                        
                        if checkpoint_gap >= 1 {
                            // Any checkpoint gap means we're missing finalized state
                            // Use snapshot sync to get complete account state including nonces
                            info!(
                                "Peer {} is {} checkpoint(s) ahead (cp: {} vs {}), requesting snapshot sync...",
                                peer, checkpoint_gap, status.checkpoint_height, local_checkpoint
                            );
                            
                            if let Err(e) = self.bootstrap_from_peer(peer, local_checkpoint).await {
                                warn!("Snapshot sync from {} failed: {}", peer, e);
                            }
                        } else if status.dag_size > local_dag_size {
                            // Same checkpoint height but peer has more unfinalized DAG transactions
                            // Try delta sync for just the recent transactions
                            info!(
                                "Peer {} has more DAG data (dag: {} vs {}), requesting delta sync...",
                                peer, status.dag_size, local_dag_size
                            );
                            
                            let result = self.sync_from_peer(peer, local_checkpoint).await;
                            match result {
                                Ok(sync_result) => {
                                    // If delta sync had failures, force snapshot to fix state divergence
                                    if sync_result.failed > 0 && sync_result.failed > sync_result.added {
                                        warn!(
                                            "Delta sync had {} failures vs {} added - forcing snapshot recovery",
                                            sync_result.failed, sync_result.added
                                        );
                                        if let Err(e) = self.bootstrap_from_peer_force(peer).await {
                                            warn!("RECOVERY: Force snapshot sync from {} failed: {}", peer, e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Sync from {} failed: {}", peer, e);
                                }
                            }
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

    async fn sync_from_peer(&self, peer: &str, local_checkpoint: u64) -> Result<DeltaSyncResult> {
        let client = reqwest::Client::new();
        let mut offset = 0usize;
        let limit = 500usize;
        let mut result = DeltaSyncResult::default();
        
        loop {
            // Fetch transactions with pagination
            let url = format!(
                "{}/api/sync/delta?from_checkpoint={}&offset={}&limit={}", 
                peer, local_checkpoint, offset, limit
            );
            
            let response = client
                .get(&url)
                .timeout(Duration::from_secs(30))
                .send()
                .await?;
            
            if !response.status().is_success() {
                anyhow::bail!("Sync request failed with status {}", response.status());
            }
            
            // Response struct for paginated mode with account nonces
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct DeltaResponse {
                transactions: Vec<SignedTransaction>,
                #[serde(default)]
                account_nonces: std::collections::HashMap<String, u64>,
                #[serde(default)]
                total: usize,
                #[serde(default)]
                offset: usize,
                #[serde(default)]
                has_more: bool,
            }
            
            let response_text = response.text().await?;
            let delta: DeltaResponse = serde_json::from_str(&response_text)
                .map_err(|e| anyhow::anyhow!("Failed to parse sync response: {}", e))?;
            
            info!(
                "Sync page: received {}/{} transactions, {} account nonces (offset={}, has_more={})", 
                delta.transactions.len(), delta.total, delta.account_nonces.len(), delta.offset, delta.has_more
            );
            
            // Apply account nonces BEFORE processing transactions to prevent nonce mismatches
            if !delta.account_nonces.is_empty() {
                for (address, nonce) in &delta.account_nonces {
                    self.state.sync_account_nonce(address, *nonce).await;
                }
                info!("Applied {} account nonces from peer", delta.account_nonces.len());
            }
            
            let (transactions, has_more) = (delta.transactions, delta.has_more);
            
            if transactions.is_empty() {
                break;
            }
            
            for tx in transactions {
                // Check if we already have this tx
                let is_known = {
                    let inner = self.inner.read().await;
                    inner.known_txs.contains(&tx.hash)
                };
                
                if !is_known {
                    if let Err(e) = self.state.add_transaction(tx.clone()).await {
                        debug!("Failed to add synced tx {}: {}", tx.hash, e);
                        result.failed += 1;
                    } else {
                        let mut inner = self.inner.write().await;
                        inner.known_txs.insert(tx.hash.clone());
                        inner.stats.txs_received += 1;
                        result.added += 1;
                    }
                }
            }
            
            if !has_more {
                break;
            }
            
            offset += limit;
            
            // Safety limit to prevent infinite loops
            if offset > 100_000 {
                warn!("Sync pagination exceeded safety limit");
                break;
            }
        }
        
        if result.added > 0 || result.failed > 0 {
            info!(
                "Delta sync complete: {} added, {} failed",
                result.added, result.failed
            );
        }
        
        Ok(result)
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
        // Centralized reverse peer discovery: extract sender_url from any message type
        let sender_url = Self::extract_sender_url(&message);
        if let Some(url) = sender_url {
            self.add_peer(url).await;
        }
        
        match message {
            GossipMessage::Transaction { hash, tx, .. } => {
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
                sender_url,
                ..
            } => {
                // Check if peer has more data than us and trigger sync if needed
                let (local_dag_size, _, _) = self.state.get_dag_stats().await;
                
                if dag_size > local_dag_size + 10 {
                    // Peer has significantly more DAG data (10+ transactions ahead)
                    // Check if sync is already in flight to avoid duplicate syncs
                    let should_sync = {
                        let mut inner = self.inner.write().await;
                        if inner.sync_in_flight {
                            false
                        } else {
                            inner.sync_in_flight = true;
                            true
                        }
                    };
                    
                    if should_sync {
                        if let Some(peer_url) = sender_url {
                            info!(
                                "TipAnnouncement: peer {} has {} DAG nodes vs our {} - spawning background sync",
                                peer_url, dag_size, local_dag_size
                            );
                            
                            // Spawn background task to avoid blocking the gossip handler
                            let gossip_clone = self.clone();
                            let peer_url_clone = peer_url.clone();
                            tokio::spawn(async move {
                                gossip_clone.trigger_sync_from_peer(&peer_url_clone).await;
                                // Clear the sync_in_flight flag when done
                                let mut inner = gossip_clone.inner.write().await;
                                inner.sync_in_flight = false;
                            });
                        } else {
                            // No sender_url, clear the flag
                            let mut inner = self.inner.write().await;
                            inner.sync_in_flight = false;
                        }
                    } else {
                        debug!("TipAnnouncement: sync already in flight, skipping");
                    }
                } else {
                    debug!("Received tip announcement, peer dag_size: {} (local: {})", dag_size, local_dag_size);
                }
                Ok(None)
            }

            GossipMessage::PeerDiscovery { peers, node_id, .. } => {
                // sender_url already handled at top of handle_message
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
                ..
            } => {
                // sender_url already handled at top of handle_message
                let txs = self.state.get_txs_since_checkpoint(from_checkpoint, &missing_hashes).await;
                let checkpoint_height = self.state.get_checkpoint_height().await;

                Ok(Some(GossipMessage::SyncResponse {
                    transactions: txs,
                    checkpoint_height,
                    sender_url: std::env::var("PUBLIC_URL").ok(),
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

    /// Trigger sync from a specific peer (used by TipAnnouncement background task)
    async fn trigger_sync_from_peer(&self, peer_url: &str) {
        let local_checkpoint = self.state.get_checkpoint_height().await;
        
        match self.fetch_peer_status(peer_url).await {
            Ok(status) => {
                let checkpoint_gap = status.checkpoint_height.saturating_sub(local_checkpoint);
                
                if checkpoint_gap >= 1 {
                    // Peer is ahead on checkpoints - use snapshot sync
                    info!("Background sync: peer {} is {} checkpoint(s) ahead, using snapshot sync", peer_url, checkpoint_gap);
                    if let Err(e) = self.bootstrap_from_peer(peer_url, local_checkpoint).await {
                        warn!("Background snapshot sync from {} failed: {}", peer_url, e);
                    }
                } else {
                    // Same checkpoint, just missing DAG transactions - use delta sync
                    info!("Background sync: peer {} at same checkpoint, using delta sync", peer_url);
                    if let Err(e) = self.sync_from_peer(peer_url, local_checkpoint).await {
                        warn!("Background delta sync from {} failed: {}", peer_url, e);
                    }
                }
            }
            Err(e) => {
                debug!("Failed to fetch peer status for background sync: {}", e);
            }
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
            sender_url: std::env::var("PUBLIC_URL").ok(),
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

    pub async fn get_peer_addresses(&self) -> Vec<String> {
        self.inner.read().await.peers.keys().cloned().collect()
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
    
    fn extract_sender_url(message: &GossipMessage) -> Option<String> {
        match message {
            GossipMessage::Transaction { sender_url, .. } => sender_url.clone(),
            GossipMessage::TipAnnouncement { sender_url, .. } => sender_url.clone(),
            GossipMessage::CheckpointSignature { sender_url, .. } => sender_url.clone(),
            GossipMessage::PeerDiscovery { sender_url, .. } => sender_url.clone(),
            GossipMessage::ConflictResolution { sender_url, .. } => sender_url.clone(),
            GossipMessage::SyncRequest { sender_url, .. } => sender_url.clone(),
            GossipMessage::SyncResponse { sender_url, .. } => sender_url.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounded_hash_set_basic_operations() {
        let mut set = BoundedHashSet::new(10);
        
        assert!(set.insert("a".to_string()));
        assert!(set.insert("b".to_string()));
        assert!(set.insert("c".to_string()));
        
        assert!(set.contains("a"));
        assert!(set.contains("b"));
        assert!(set.contains("c"));
        assert!(!set.contains("d"));
        
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn test_bounded_hash_set_duplicate_rejected() {
        let mut set = BoundedHashSet::new(10);
        
        assert!(set.insert("a".to_string()));
        assert!(!set.insert("a".to_string()));
        
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_bounded_hash_set_fifo_eviction() {
        let mut set = BoundedHashSet::new(3);
        
        set.insert("a".to_string());
        set.insert("b".to_string());
        set.insert("c".to_string());
        
        assert_eq!(set.len(), 3);
        assert!(set.contains("a"));
        assert!(set.contains("b"));
        assert!(set.contains("c"));
        
        set.insert("d".to_string());
        
        assert_eq!(set.len(), 3);
        assert!(!set.contains("a"));
        assert!(set.contains("b"));
        assert!(set.contains("c"));
        assert!(set.contains("d"));
    }

    #[test]
    fn test_bounded_hash_set_eviction_order() {
        let mut set = BoundedHashSet::new(2);
        
        set.insert("first".to_string());
        set.insert("second".to_string());
        set.insert("third".to_string());
        
        assert!(!set.contains("first"));
        assert!(set.contains("second"));
        assert!(set.contains("third"));
        
        set.insert("fourth".to_string());
        
        assert!(!set.contains("second"));
        assert!(set.contains("third"));
        assert!(set.contains("fourth"));
    }

    #[test]
    fn test_bounded_hash_set_clear() {
        let mut set = BoundedHashSet::new(10);
        
        set.insert("a".to_string());
        set.insert("b".to_string());
        set.insert("c".to_string());
        
        assert_eq!(set.len(), 3);
        
        set.clear();
        
        assert_eq!(set.len(), 0);
        assert!(!set.contains("a"));
        assert!(!set.contains("b"));
        assert!(!set.contains("c"));
    }

    #[test]
    fn test_bounded_hash_set_capacity_one() {
        let mut set = BoundedHashSet::new(1);
        
        set.insert("a".to_string());
        assert!(set.contains("a"));
        assert_eq!(set.len(), 1);
        
        set.insert("b".to_string());
        assert!(!set.contains("a"));
        assert!(set.contains("b"));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_bounded_hash_set_large_capacity() {
        let mut set = BoundedHashSet::new(1000);
        
        for i in 0..1000 {
            set.insert(format!("item_{}", i));
        }
        
        assert_eq!(set.len(), 1000);
        
        set.insert("overflow".to_string());
        
        assert_eq!(set.len(), 1000);
        assert!(!set.contains("item_0"));
        assert!(set.contains("overflow"));
    }

    #[test]
    fn test_bounded_hash_set_re_insert_after_eviction() {
        let mut set = BoundedHashSet::new(2);
        
        set.insert("a".to_string());
        set.insert("b".to_string());
        set.insert("c".to_string());
        
        assert!(!set.contains("a"));
        
        assert!(set.insert("a".to_string()));
        
        assert!(set.contains("a"));
        assert!(!set.contains("b"));
        assert!(set.contains("c"));
    }
}
