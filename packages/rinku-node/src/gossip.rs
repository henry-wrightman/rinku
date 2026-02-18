use anyhow::Result;
use rinku_core::crypto::sha256_hex;
use rinku_core::types::{Account, Checkpoint, SignedTransaction, Validator};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};
use crate::state::TransactionResult;

const KNOWN_TXS_MAX_SIZE: usize = 50_000;
const SEEN_CONFLICTS_MAX_SIZE: usize = 10_000;
const SEEN_EVIDENCE_MAX_SIZE: usize = 5_000;

/// Default bloom filter size in bits (64KB = 512k bits for ~100K items at 1% FPR)
const BLOOM_FILTER_SIZE_BITS: usize = 524_288;
/// Number of hash functions for bloom filter (optimal k = (m/n) * ln(2) ≈ 7 for 100K items)
const BLOOM_FILTER_HASH_COUNT: usize = 7;

/// A space-efficient probabilistic data structure for set membership testing.
/// Used to efficiently advertise which transactions a node knows about without
/// transmitting full transaction hashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilter {
    /// Bit array stored as bytes
    bits: Vec<u8>,
    /// Number of bits in the filter
    size_bits: usize,
    /// Number of hash functions
    hash_count: usize,
    /// Number of items inserted
    item_count: usize,
}

impl BloomFilter {
    /// Create a new bloom filter with default parameters
    pub fn new() -> Self {
        Self::with_size(BLOOM_FILTER_SIZE_BITS, BLOOM_FILTER_HASH_COUNT)
    }

    /// Create a bloom filter with custom size
    pub fn with_size(size_bits: usize, hash_count: usize) -> Self {
        let size_bytes = (size_bits + 7) / 8;
        Self {
            bits: vec![0u8; size_bytes],
            size_bits,
            hash_count,
            item_count: 0,
        }
    }

    /// Create a compact bloom filter for limited items (e.g., recent tips)
    pub fn compact(expected_items: usize) -> Self {
        let size_bits = (expected_items * 10).max(1024);
        let hash_count = 7.min((size_bits / expected_items.max(1)) * 7 / 10);
        Self::with_size(size_bits, hash_count.max(3))
    }

    /// Insert an item into the bloom filter
    pub fn insert(&mut self, item: &str) {
        for i in 0..self.hash_count {
            let bit_index = self.hash(item, i);
            let byte_index = bit_index / 8;
            let bit_offset = bit_index % 8;
            if byte_index < self.bits.len() {
                self.bits[byte_index] |= 1 << bit_offset;
            }
        }
        self.item_count += 1;
    }

    /// Check if an item might be in the set (may have false positives)
    pub fn might_contain(&self, item: &str) -> bool {
        for i in 0..self.hash_count {
            let bit_index = self.hash(item, i);
            let byte_index = bit_index / 8;
            let bit_offset = bit_index % 8;
            if byte_index >= self.bits.len() || (self.bits[byte_index] & (1 << bit_offset)) == 0 {
                return false;
            }
        }
        true
    }

    /// Generate hash values for bloom filter indexing
    /// Uses double hashing: h(item, i) = (h1 + i * h2) mod m
    fn hash(&self, item: &str, index: usize) -> usize {
        let h1 = self.hash_primary(item);
        let h2 = self.hash_secondary(item);
        ((h1 as u128 + (index as u128) * (h2 as u128)) % (self.size_bits as u128)) as usize
    }

    fn hash_primary(&self, item: &str) -> u64 {
        let hash = sha256_hex(item);
        u64::from_str_radix(&hash[0..16], 16).unwrap_or(0)
    }

    fn hash_secondary(&self, item: &str) -> u64 {
        let hash = sha256_hex(item);
        u64::from_str_radix(&hash[16..32], 16).unwrap_or(1)
    }

    /// Get the estimated false positive rate
    pub fn false_positive_rate(&self) -> f64 {
        if self.item_count == 0 {
            return 0.0;
        }
        let k = self.hash_count as f64;
        let m = self.size_bits as f64;
        let n = self.item_count as f64;
        (1.0 - (-k * n / m).exp()).powf(k)
    }

    /// Get the size in bytes
    pub fn size_bytes(&self) -> usize {
        self.bits.len()
    }

    /// Get the number of items inserted
    pub fn item_count(&self) -> usize {
        self.item_count
    }

    /// Clear the bloom filter
    pub fn clear(&mut self) {
        self.bits.fill(0);
        self.item_count = 0;
    }
}

impl Default for BloomFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// Bloom filter announcement message for bandwidth-efficient tx advertising
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomAnnouncement {
    /// The bloom filter containing known tx hashes
    pub filter: BloomFilter,
    /// Checkpoint height this filter is associated with
    pub checkpoint_height: u64,
    /// Timestamp of announcement
    pub timestamp: u64,
    /// Peer's known DAG tip count
    pub tip_count: usize,
}

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
#[cfg(feature = "p2p")]
use crate::network::NetworkHandle;
use crate::network::{AccountData, CheckpointData, DeltaData, TransactionData};
use crate::slashing::DoubleSignEvidence;
use crate::state::{NodeState, SyncSnapshot};
use crate::sync_verification::{build_account_merkle_root_sorted, verify_delta, VerificationResult};
use crate::trust::TrustVerifier;
use crate::validator_identity::MIN_VALIDATOR_STAKE;

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
    #[serde(default)]
    faucet_balance: f64,
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
    #[serde(default)]
    contracts: HashMap<String, crate::contracts::ContractState>,
    #[serde(default)]
    accounts_merkle_root: String,
    #[serde(default)]
    rewards_snapshot: Option<crate::rewards::RewardsSnapshot>,
    #[serde(default)]
    emission_snapshot: Option<crate::emission::EmissionSnapshot>,
    #[serde(default)]
    slashing_snapshot: Option<crate::slashing::SlashingSnapshot>,
    #[serde(default)]
    total_burned: f64,
    #[serde(default)]
    total_to_validators: f64,
    #[serde(default)]
    genesis_hash: Option<String>,
    #[serde(default)]
    finalized_tx_hashes: Vec<String>,
    #[serde(default)]
    tx_checkpoint_heights: std::collections::HashMap<String, u64>,
    #[serde(default)]
    weight_scores: HashMap<String, rinku_core::types::AggregatedWeight>,
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
    BloomAnnouncement {
        filter: BloomFilter,
        checkpoint_height: u64,
        tip_count: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    SlashingEvidence {
        evidence: DoubleSignEvidence,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    /// Weight attestation vote for transaction trust scoring
    WeightVote {
        tx_hash: String,
        validator_pubkey: String,
        vote: String,
        timestamp_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bls_signature: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    /// Immediate broadcast of a newly created checkpoint (high priority)
    CheckpointAnnouncement {
        checkpoint: Checkpoint,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
        /// List of transaction hashes finalized in this checkpoint
        /// Used by receivers to finalize transactions even if merkle roots don't match
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        finalized_tx_hashes: Vec<String>,
        /// Precomputed account state proofs for affected addresses
        /// Generated by leader node and propagated to followers for storage
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        precomputed_proofs: Vec<rinku_core::types::AccountStateProof>,
        /// CRITICAL: Actual finalized transactions for consensus consistency
        /// Followers MUST have all transactions to execute and get same state as leader
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        finalized_transactions: Vec<SignedTransaction>,
    },
    /// Fast-path broadcast for data-only transactions (Mysticeti-FPC style)
    /// These transactions only touch sender's account (gas) and can achieve
    /// sub-second finality via reliable broadcast quorum
    FastPathBroadcast {
        tx: SignedTransaction,
        sender_validator: String,
        sender_stake: f64,
        timestamp_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    /// Validator acknowledgment for fast-path transaction
    /// When 2/3+ stake ACKs are collected, the tx is considered confirmed
    FastPathAck {
        tx_hash: String,
        validator_address: String,
        validator_stake: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bls_signature: Option<String>,
        timestamp_ms: u64,
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
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub backoff_until: u64,
}

/// Maximum size for stale nonce cache (addresses tracked)
const STALE_NONCE_CACHE_MAX: usize = 10_000;

/// Maximum pending transactions to buffer before dropping (prevents OOM under high load)
const PENDING_TXS_MAX: usize = 10_000;

pub struct GossipServiceInner {
    pub peers: HashMap<String, PeerInfo>,
    pub known_txs: BoundedHashSet,
    pub pending_txs: Vec<SignedTransaction>,
    pub seen_conflicts: BoundedHashSet,
    pub seen_evidence: BoundedHashSet,
    pub stats: GossipStats,
    pub round_counter: u64,
    pub last_peer_refresh: u64,
    pub sync_in_flight: bool,
    pub peer_bloom_filters: HashMap<String, BloomFilter>,
    /// Cooldown: last time sync failed (prevents sync storm)
    pub last_sync_failure: Option<std::time::Instant>,
    /// Global debounce: last time ANY sync was started (prevents sync storm across all peers)
    pub last_any_sync: Option<std::time::Instant>,
    /// Per-peer debounce: last sync attempt time for each peer
    pub peer_last_sync: HashMap<String, std::time::Instant>,
    /// Rate limit recovery attempts: max 1 per 30 seconds per peer
    pub peer_last_recovery: HashMap<String, std::time::Instant>,
    /// Count of active sync requests (to limit concurrency)
    pub active_sync_count: u32,
    /// Quick-reject cache for stale nonces: maps sender address to minimum expected nonce
    /// Transactions with nonce < cached value are rejected instantly without full validation
    pub stale_nonce_cache: HashMap<String, u64>,
    /// FIFO queue for cache eviction - tracks insertion order of addresses
    pub stale_nonce_cache_order: VecDeque<String>,
    /// Counter for stale nonce rejections (for stats/logging)
    pub stale_nonce_rejections: u64,
    /// Counter for cache hits (quick-rejected without full validation)
    pub stale_nonce_cache_hits: u64,
    /// Flag to prevent overlapping propagation tasks
    pub propagation_in_flight: bool,
    /// Fast-path finality tracking for data-only transactions
    pub fast_path_pending: HashMap<String, rinku_core::types::FastPathFinality>,
    pub fast_path_confirmed: HashMap<String, rinku_core::types::FastPathFinality>,
    /// Tracks tx hashes that have been executed on fast-path (balances applied)
    /// to prevent double-execution at checkpoint time
    pub fast_path_executed: std::collections::HashSet<String>,
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
pub struct CheckpointVoteSigner {
    pub validator_address: String,
    pub bls_private_key: Vec<u8>,
    pub bls_public_key: Vec<u8>,
}

#[derive(Clone)]
pub struct GossipService {
    state: NodeState,
    inner: Arc<RwLock<GossipServiceInner>>,
    node_id: String,
    interval_ms: u64,
    trust_verifier: Arc<TrustVerifier>,
    sync_verify_strict: bool,
    http_client: reqwest::Client,
    checkpoint_vote_signer: Option<CheckpointVoteSigner>,
    #[cfg(feature = "p2p")]
    network_handle: Option<Arc<tokio::sync::Mutex<NetworkHandle>>>,
    /// Validator identity service for syncing validator registry
    validator_identity: Option<Arc<tokio::sync::RwLock<crate::validator_identity::ValidatorIdentityService>>>,
}

impl GossipService {
    pub fn new(
        state: NodeState,
        initial_peers: Vec<String>,
        interval_ms: u64,
        trust_config: TrustConfig,
        sync_verify_strict: bool,
    ) -> Self {
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
                consecutive_failures: 0,
                backoff_until: 0,
            });
        }

        // Create a shared HTTP client with connection pooling
        let http_client = reqwest::Client::builder()
            .pool_max_idle_per_host(2)
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        
        Self {
            state,
            inner: Arc::new(RwLock::new(GossipServiceInner {
                peers,
                known_txs: BoundedHashSet::new(KNOWN_TXS_MAX_SIZE),
                pending_txs: Vec::new(),
                seen_conflicts: BoundedHashSet::new(SEEN_CONFLICTS_MAX_SIZE),
                seen_evidence: BoundedHashSet::new(SEEN_EVIDENCE_MAX_SIZE),
                stats: GossipStats::default(),
                round_counter: 0,
                last_peer_refresh: now,
                sync_in_flight: false,
                peer_bloom_filters: HashMap::new(),
                last_sync_failure: None,
                last_any_sync: None,
                peer_last_sync: HashMap::new(),
                peer_last_recovery: HashMap::new(),
                active_sync_count: 0,
                stale_nonce_cache: HashMap::new(),
                stale_nonce_cache_order: VecDeque::new(),
                stale_nonce_rejections: 0,
                stale_nonce_cache_hits: 0,
                propagation_in_flight: false,
                fast_path_pending: HashMap::new(),
                fast_path_confirmed: HashMap::new(),
                fast_path_executed: std::collections::HashSet::new(),
            })),
            node_id,
            interval_ms,
            trust_verifier: Arc::new(TrustVerifier::new(trust_config)),
            sync_verify_strict,
            http_client,
            checkpoint_vote_signer: None,
            #[cfg(feature = "p2p")]
            network_handle: None,
            validator_identity: None,
        }
    }
    
    /// Set the validator identity service for syncing validator registry
    pub fn set_validator_identity(
        &mut self,
        validator_identity: Arc<tokio::sync::RwLock<crate::validator_identity::ValidatorIdentityService>>,
    ) {
        self.validator_identity = Some(validator_identity);
    }

    pub fn set_checkpoint_vote_signer(&mut self, signer: CheckpointVoteSigner) {
        self.checkpoint_vote_signer = Some(signer);
    }

    #[cfg(feature = "p2p")]
    pub fn set_network_handle(&mut self, handle: Arc<tokio::sync::Mutex<NetworkHandle>>) {
        self.network_handle = Some(handle);
    }
    
    /// Sync the validator identity service from the synced state.validators
    /// This ensures all nodes have the same validator registry for deterministic leader election.
    /// Public so API can call it after merging validators from incoming requests
    pub async fn sync_validator_identity_from_state(&self) {
        if let Some(ref validator_identity) = self.validator_identity {
            let synced_validators = self.state.get_validators_map().await;
            if !synced_validators.is_empty() {
                let mut vi_guard = validator_identity.write().await;
                vi_guard.sync_from_legacy_validators(&synced_validators);
                info!(
                    "Synced validator identity service with {} validators from peer",
                    synced_validators.len()
                );
            }
        }
    }

    pub async fn start(self: Arc<Self>) -> Result<()> {
        let peer_count = self.inner.read().await.peers.len();
        #[cfg(feature = "p2p")]
        let has_p2p = self.network_handle.is_some();
        #[cfg(not(feature = "p2p"))]
        let has_p2p = false;
        info!(
            "Gossip service started (interval: {}ms, peers: {}, p2p: {})",
            self.interval_ms,
            peer_count,
            has_p2p
        );

        if peer_count > 0 {
            self.initial_sync().await;
        }

        // If we have a libp2p network handle, spawn a task to receive messages
        #[cfg(feature = "p2p")]
        if let Some(ref handle) = self.network_handle {
            let gossip_clone = Arc::clone(&self);
            let handle_clone = handle.clone();
            tokio::spawn(async move {
                gossip_clone.run_p2p_receiver(handle_clone).await;
            });
            
            // Spawn a task to handle incoming sync requests
            let gossip_clone2 = Arc::clone(&self);
            let handle_clone2 = handle.clone();
            tokio::spawn(async move {
                gossip_clone2.run_sync_request_handler(handle_clone2).await;
            });
        }

        let mut tick = interval(Duration::from_millis(self.interval_ms));
        let gossip_arc = Arc::clone(&self);

        loop {
            tick.tick().await;
            self.gossip_round(&gossip_arc).await;
        }
    }

    #[cfg(feature = "p2p")]
    async fn run_p2p_receiver(&self, handle: Arc<tokio::sync::Mutex<NetworkHandle>>) {
        info!("P2P message receiver started");
        loop {
            // Use try_recv with a short sleep to avoid holding the lock
            // This prevents deadlock when broadcast_via_p2p tries to acquire the lock
            let recv_result = {
                let mut locked = handle.lock().await;
                locked.message_rx.try_recv()
            };
            
            match recv_result {
                Ok(msg) => {
                    debug!("Received P2P gossip message: {:?}", std::mem::discriminant(&msg));
                    if let Err(e) = self.handle_message(msg).await {
                        warn!("Failed to handle P2P message: {}", e);
                    }
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    // No message available, sleep briefly and try again
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    warn!("P2P message channel closed");
                    break;
                }
            }
        }
    }

    #[cfg(feature = "p2p")]
    async fn run_sync_request_handler(&self, handle: Arc<tokio::sync::Mutex<NetworkHandle>>) {
        use crate::network::{
            AccountData, CheckpointData, CheckpointVoteResponse, DeltaData, PeerHandshake,
            SnapshotData, SyncRequest, SyncResponse, TransactionData, ValidatorData,
        };
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        use crate::bls::bls_sign;
        
        info!("P2P sync request handler started");
        loop {
            let recv_result = {
                let mut locked = handle.lock().await;
                locked.sync_incoming_rx.try_recv()
            };
            
            match recv_result {
                Ok(incoming) => {
                    info!("Handling sync request from {}: {:?}", incoming.peer_id, incoming.request);
                    
                    let response = match incoming.request {
                        SyncRequest::Snapshot => {
                            // Use the existing get_sync_snapshot() method which gathers all state
                            let snapshot = self.state.get_sync_snapshot().await;
                            
                            // Convert to network SnapshotData format
                            let account_data: Vec<AccountData> = snapshot.accounts.values().map(|a| AccountData {
                                address: a.address.clone(),
                                balance: a.balance,
                                nonce: a.nonce,
                                stake: a.staked,
                            }).collect();
                            
                            // validators is HashMap<String, Validator>, iterate over values
                            let validator_data: Vec<ValidatorData> = snapshot.validators.values().map(|v| ValidatorData {
                                address: v.address.clone(),
                                stake: v.stake,
                                bls_public_key: v.bls_public_key.clone().unwrap_or_default(),
                                status: "Active".to_string(),
                            }).collect();
                            
                            // Map Checkpoint fields correctly
                            let checkpoint_data: Vec<CheckpointData> = snapshot.checkpoints.iter().map(|c| CheckpointData {
                                height: c.height,
                                merkle_root: c.tx_merkle_root.clone(),
                                timestamp: c.timestamp,
                                tx_count: c.tip_count as u64,
                                hash: Some(c.hash.clone()),
                                previous_hash: c.previous_hash.clone(),
                                signature: c.aggregated_signature.clone(),
                                genesis_hash: snapshot.genesis_hash.clone(),
                            }).collect();
                            
                            let tx_data: Vec<TransactionData> = snapshot.dag_transactions.iter().map(|stx| TransactionData {
                                hash: stx.hash.clone(),
                                from: stx.tx.from.clone(),
                                to: stx.tx.to.clone(),
                                amount: stx.tx.amount,
                                nonce: stx.tx.nonce,
                                timestamp: stx.tx.timestamp,
                                signature: stx.signature.clone(),
                                parents: stx.tx.parents.clone(),
                                gas_price: stx.tx.gas_price.unwrap_or(0.0),
                                memo: stx.tx.memo.clone(),
                                references: stx.tx.references.clone(),
                            }).collect();
                            
                            // Get merkle root from latest checkpoint
                            let merkle_root = snapshot.checkpoints.last()
                                .map(|c| c.tx_merkle_root.clone())
                                .unwrap_or_default();
                            
                            info!("Sending snapshot: {} accounts, {} validators, {} checkpoints, {} txs",
                                account_data.len(), validator_data.len(), checkpoint_data.len(), tx_data.len());
                            
                            SyncResponse::Snapshot(SnapshotData {
                                accounts: account_data,
                                validators: validator_data,
                                checkpoints: checkpoint_data,
                                recent_txs: tx_data,
                                merkle_root,
                            })
                        }
                        SyncRequest::Delta { from_checkpoint } => {
                            info!("Handling P2P delta sync request from checkpoint {}", from_checkpoint);
                            
                            // Get checkpoints and transaction mappings
                            let (new_checkpoints, tx_checkpoint_heights, to_checkpoint, validators) = {
                                let state_guard = self.state.inner.read().await;
                                let mut tx_checkpoint_heights = std::collections::HashMap::new();
                                let mut tx_count_by_checkpoint: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
                                
                                for node in state_guard.dag.get_all_nodes() {
                                    if let Some(height) = node.checkpoint_height {
                                        tx_checkpoint_heights.insert(node.hash.clone(), height);
                                        *tx_count_by_checkpoint.entry(height).or_insert(0) += 1;
                                    }
                                }
                                
                                let new_checkpoints: Vec<CheckpointData> = state_guard
                                    .checkpoints
                                    .iter()
                                    .filter(|cp| cp.height > from_checkpoint)
                                    .map(|cp| CheckpointData {
                                        height: cp.height,
                                        merkle_root: cp.tx_merkle_root.clone(),
                                        timestamp: cp.timestamp,
                                        tx_count: *tx_count_by_checkpoint.get(&cp.height).unwrap_or(&0),
                                        hash: Some(cp.hash.clone()),
                                        previous_hash: cp.previous_hash.clone(),
                                        signature: cp.aggregated_signature.clone(),
                                        genesis_hash: state_guard.genesis_hash.clone(),
                                    })
                                    .collect();
                                
                                let to_checkpoint = state_guard.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
                                
                                let validators: Vec<ValidatorData> = state_guard.validators.values().map(|v| ValidatorData {
                                    address: v.address.clone(),
                                    stake: v.stake,
                                    bls_public_key: v.bls_public_key.clone().unwrap_or_default(),
                                    status: "Active".to_string(),
                                }).collect();
                                
                                (new_checkpoints, tx_checkpoint_heights, to_checkpoint, validators)
                            };
                            
                            // Get transactions since checkpoint
                            let all_txs = self.state.get_txs_since_checkpoint(from_checkpoint, &[]).await;
                            
                            // Limit to prevent massive responses (P2P has size limits)
                            let txs: Vec<TransactionData> = all_txs.into_iter()
                                .take(5000) // Cap at 5000 txs per request
                                .map(|stx| TransactionData {
                                    hash: stx.hash.clone(),
                                    from: stx.tx.from.clone(),
                                    to: stx.tx.to.clone(),
                                    amount: stx.tx.amount,
                                    nonce: stx.tx.nonce,
                                    timestamp: stx.tx.timestamp,
                                    signature: stx.signature.clone(),
                                    parents: stx.tx.parents.clone(),
                                    gas_price: stx.tx.gas_price.unwrap_or(0.0),
                                    memo: stx.tx.memo.clone(),
                                    references: stx.tx.references.clone(),
                                })
                                .collect();
                            
                            info!("P2P delta sync response: {} txs, {} checkpoints (from {} to {})",
                                txs.len(), new_checkpoints.len(), from_checkpoint, to_checkpoint);
                            
                            SyncResponse::Delta(DeltaData {
                                transactions: txs,
                                new_checkpoints,
                                from_checkpoint,
                                to_checkpoint,
                                tx_checkpoint_heights,
                                validators,
                            })
                        }
                        SyncRequest::Transaction { hash } => {
                            let tx = self.state.get_transaction(&hash).await;
                            SyncResponse::Transaction(tx.map(|stx| TransactionData {
                                hash: stx.hash.clone(),
                                from: stx.tx.from.clone(),
                                to: stx.tx.to.clone(),
                                amount: stx.tx.amount,
                                nonce: stx.tx.nonce,
                                timestamp: stx.tx.timestamp,
                                signature: stx.signature.clone(),
                                parents: stx.tx.parents.clone(),
                                gas_price: stx.tx.gas_price.unwrap_or(0.0),
                                memo: stx.tx.memo.clone(),
                                references: stx.tx.references.clone(),
                            }))
                        }
                        SyncRequest::Proof { tx_hash } => {
                            // TODO: Implement proof generation
                            SyncResponse::Proof(None)
                        }
                        SyncRequest::AccountsState { addresses } => {
                            let accounts = self.state.get_accounts_by_addresses(&addresses).await;
                            SyncResponse::AccountsState(accounts)
                        }
                        SyncRequest::Handshake(_handshake) => {
                            // Return our peer info
                            let our_handshake = PeerHandshake {
                                protocol_version: "1.0.0".to_string(),
                                chain_id: "rinku-testnet".to_string(),
                                network_id: "rinku".to_string(),
                                node_id: self.state.get_node_id().await,
                                checkpoint_height: self.state.get_checkpoint_height().await,
                                validator_address: self.checkpoint_vote_signer.as_ref().map(|s| s.validator_address.clone()),
                                capabilities: vec!["sync".to_string(), "gossip".to_string()],
                            };
                            SyncResponse::Handshake(our_handshake)
                        }
                        SyncRequest::CheckpointVote(request) => {
                            if let Some(ref signer) = self.checkpoint_vote_signer {
                                if signer.validator_address.is_empty() {
                                    SyncResponse::CheckpointVote(None)
                                } else {
                                    // OPTION A: Apply missing transactions from the vote request before signing
                                    // This ensures consensus can progress even during high transaction volume
                                    let can_sign = if !request.finalized_tx_hashes.is_empty() {
                                        // First, check which transactions we're missing
                                        let missing_hashes: Vec<String> = {
                                            let state = self.state.inner.read().await;
                                            request.finalized_tx_hashes.iter()
                                                .filter(|hash| state.dag.get_node(hash).is_none())
                                                .cloned()
                                                .collect()
                                        };
                                        
                                        let mut applied_count = 0;
                                        
                                        // If we have missing transactions and the leader sent the data, apply them
                                        if !missing_hashes.is_empty() && !request.finalized_transactions.is_empty() {
                                            info!(
                                                "Vote request includes {} transactions, need {} missing for checkpoint {}",
                                                request.finalized_transactions.len(),
                                                missing_hashes.len(),
                                                request.height
                                            );
                                            
                                            // Apply missing transactions from the vote request using force-add
                                            // This bypasses nonce/parent validation since we just need the hash
                                            // in the DAG for merkle verification - we're not executing these
                                            for tx in &request.finalized_transactions {
                                                if missing_hashes.contains(&tx.hash) {
                                                    // Force-add to DAG without validation (for checkpoint vote only)
                                                    match self.state.force_add_transaction_for_vote(tx.clone()).await {
                                                        Ok(_) => {
                                                            applied_count += 1;
                                                        }
                                                        Err(e) => {
                                                            // Log but continue - might already exist or have other issues
                                                            debug!("Could not force-add tx {} for vote: {}", &tx.hash[..16.min(tx.hash.len())], e);
                                                        }
                                                    }
                                                }
                                            }
                                            
                                            if applied_count > 0 {
                                                info!(
                                                    "Force-added {}/{} missing transactions for checkpoint {} vote verification",
                                                    applied_count, missing_hashes.len(), request.height
                                                );
                                            }
                                        }
                                        
                                        // Re-check missing count after applying transactions
                                        let (still_missing, still_missing_hashes) = {
                                            let state = self.state.inner.read().await;
                                            let missing: Vec<String> = request.finalized_tx_hashes.iter()
                                                .filter(|hash| state.dag.get_node(hash).is_none())
                                                .cloned()
                                                .collect();
                                            (missing.len(), missing)
                                        };
                                        
                                        if still_missing > 0 {
                                            // Log which specific hashes are missing
                                            let embedded_hashes: std::collections::HashSet<&str> = 
                                                request.finalized_transactions.iter()
                                                    .map(|tx| tx.hash.as_str())
                                                    .collect();
                                            
                                            for missing_hash in still_missing_hashes.iter().take(5) {
                                                let in_embedded = embedded_hashes.contains(missing_hash.as_str());
                                                warn!(
                                                    "Missing hash after sync: {} (in embedded txs: {})",
                                                    &missing_hash[..32.min(missing_hash.len())], in_embedded
                                                );
                                            }
                                            
                                            warn!(
                                                "Declining checkpoint vote for height {}: still missing {}/{} transactions after inline sync (embedded: {})",
                                                request.height, still_missing, request.finalized_tx_hashes.len(),
                                                request.finalized_transactions.len()
                                            );
                                            false
                                        } else {
                                            // Verify merkle root matches
                                            let mut tx_hashes = request.finalized_tx_hashes.clone();
                                            tx_hashes.sort();
                                            let computed_root = rinku_core::MerkleTree::from_hex_leaves(&tx_hashes)
                                                .map(|t| t.root())
                                                .unwrap_or_else(|_| "0".repeat(64));
                                            
                                            if computed_root != request.tx_merkle_root {
                                                warn!(
                                                    "Declining checkpoint vote for height {}: merkle root mismatch (ours={}, theirs={})",
                                                    request.height, &computed_root[..16], &request.tx_merkle_root[..16]
                                                );
                                                false
                                            } else {
                                                info!(
                                                    "Verified {} transactions for checkpoint {} vote (applied {} via inline sync)",
                                                    request.finalized_tx_hashes.len(), request.height, applied_count
                                                );
                                                true
                                            }
                                        }
                                    } else {
                                        // Legacy request without tx hashes - sign blindly (backwards compatible)
                                        debug!("Legacy vote request without tx hashes for height {}", request.height);
                                        true
                                    };
                                    
                                    if !can_sign {
                                        SyncResponse::CheckpointVote(None)
                                    } else {
                                        match hex::decode(&request.checkpoint_hash) {
                                            Ok(hash_bytes) => {
                                                match bls_sign(&hash_bytes, &signer.bls_private_key) {
                                                    Ok(signature) => {
                                                        let response = CheckpointVoteResponse {
                                                            validator_address: signer.validator_address.clone(),
                                                            signature: URL_SAFE_NO_PAD.encode(&signature),
                                                            signature_bytes: signature,
                                                            bls_public_key: URL_SAFE_NO_PAD.encode(&signer.bls_public_key),
                                                            stake: MIN_VALIDATOR_STAKE,
                                                        };
                                                        SyncResponse::CheckpointVote(Some(response))
                                                    }
                                                    Err(e) => SyncResponse::Error {
                                                        message: format!("Failed to sign checkpoint: {}", e),
                                                    },
                                                }
                                            }
                                            Err(_) => SyncResponse::Error {
                                                message: "Invalid checkpoint hash".to_string(),
                                            },
                                        }
                                    }
                                }
                            } else {
                                SyncResponse::CheckpointVote(None)
                            }
                        }
                        _ => {
                            debug!("Unsupported sync request type from {}", incoming.peer_id);
                            SyncResponse::Error { message: "Unsupported request type".to_string() }
                        }
                    };
                    
                    // Send response back through the channel
                    let locked = handle.lock().await;
                    locked.send_sync_response(incoming.response_channel, response);
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    warn!("Sync request channel closed");
                    break;
                }
            }
        }
    }

    #[cfg(feature = "p2p")]
    async fn broadcast_via_p2p(&self, message: &GossipMessage) {
        if let Some(ref handle) = self.network_handle {
            let locked = handle.lock().await;
            if let Err(e) = locked.broadcast(message.clone()).await {
                debug!("Failed to broadcast via P2P: {}", e);
            }
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

    /// DEPRECATED: Use fetch_peer_status_p2p for 100% P2P operation
    #[allow(dead_code)]
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

    /// DEPRECATED: Use bootstrap_from_peer_p2p for 100% P2P operation
    #[allow(dead_code)]
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

        if self.sync_verify_strict {
            if snapshot_response.accounts_merkle_root.is_empty() {
                anyhow::bail!("MAINNET_MODE: Snapshot missing accounts_merkle_root");
            }
            let mut account_data: Vec<AccountData> = snapshot_response
                .accounts
                .iter()
                .map(|(address, account)| AccountData {
                    address: address.clone(),
                    balance: account.balance,
                    nonce: account.nonce,
                    stake: account.staked,
                })
                .collect();
            account_data.sort_by(|a, b| a.address.cmp(&b.address));
            let computed_root = build_account_merkle_root_sorted(&account_data);
            if computed_root != snapshot_response.accounts_merkle_root {
                anyhow::bail!(
                    "MAINNET_MODE: Snapshot accounts merkle mismatch (computed {} != received {})",
                    &computed_root[..16],
                    &snapshot_response.accounts_merkle_root[..16.min(snapshot_response.accounts_merkle_root.len())]
                );
            }
        }

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

        // CRITICAL: Before replacing our DAG, broadcast any pending transactions to the peer
        // This prevents locally-created transactions from being lost during snapshot sync
        self.flush_pending_txs_to_peer(peer).await;

        // Get our local accounts BEFORE applying snapshot (to push back local-only accounts)
        let local_accounts_before = self.state.get_all_accounts_map().await;
        let peer_fingerprints: std::collections::HashSet<String> = 
            snapshot_response.accounts.keys().cloned().collect();

        // CRITICAL: Validate genesis hash before applying snapshot
        // This prevents syncing from nodes on a different chain (e.g., old deployments)
        let local_genesis_hash = self.state.get_genesis_hash().await;
        let peer_genesis_hash = snapshot_response.genesis_hash.clone();
        
        // Check if this node has ever successfully synced from the network.
        // If not, it's a new node that should adopt the peer's genesis hash.
        // This is more reliable than checking account/tx counts, which grow with checkpoints.
        let has_synced = self.state.has_synced_from_network().await;
        
        if let (Some(local), Some(peer_gh)) = (&local_genesis_hash, &peer_genesis_hash) {
            if local != peer_gh {
                if !has_synced {
                    // New node that hasn't synced yet - adopt peer's genesis hash
                    info!(
                        "New node adopting peer genesis hash from {} (local {} -> peer {})",
                        peer, &local[..16.min(local.len())], &peer_gh[..16.min(peer_gh.len())]
                    );
                } else {
                    // Established node that has synced before - reject mismatched genesis
                    warn!(
                        "GENESIS MISMATCH: Rejecting sync from peer {} - local genesis {} != peer genesis {}",
                        peer, &local[..16.min(local.len())], &peer_gh[..16.min(peer_gh.len())]
                    );
                    anyhow::bail!(
                        "Genesis hash mismatch: this node is on a different chain than peer {}",
                        peer
                    );
                }
            } else {
                info!("Genesis hash verified: {}", &local[..16.min(local.len())]);
            }
        } else if local_genesis_hash.is_some() && peer_genesis_hash.is_none() {
            warn!(
                "Peer {} does not provide genesis_hash - accepting snapshot (legacy peer)",
                peer
            );
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
            contracts: snapshot_response.contracts,
            rewards_snapshot: snapshot_response.rewards_snapshot,
            emission_snapshot: snapshot_response.emission_snapshot,
            slashing_snapshot: snapshot_response.slashing_snapshot,
            total_burned: snapshot_response.total_burned,
            total_to_validators: snapshot_response.total_to_validators,
            genesis_hash: peer_genesis_hash.clone(),
            finalized_tx_hashes: snapshot_response.finalized_tx_hashes,
            tx_checkpoint_heights: snapshot_response.tx_checkpoint_heights,
            weight_scores: snapshot_response.weight_scores,
        };

        let added = self.state.apply_sync_snapshot(snapshot).await?;
        
        // CRITICAL: Sync the validator identity service with the synced validators
        // This ensures all nodes have the same validator registry for leader election
        self.sync_validator_identity_from_state().await;
        
        // Mark this node as having synced from the network
        self.state.mark_synced_from_network().await;
        
        if let Some(gh) = peer_genesis_hash {
            self.state.set_genesis_hash(gh.clone()).await;
            info!("Persisted peer genesis hash: {}", &gh[..16.min(gh.len())]);
        }
        
        info!("Snapshot sync complete: applied {} DAG transactions", added);

        // CRITICAL: Push local-only accounts back to peer to prevent account loss
        // This ensures accounts created on this node are shared with the peer
        let local_only_accounts: std::collections::HashMap<String, rinku_core::types::Account> = 
            local_accounts_before
                .into_iter()
                .filter(|(fingerprint, _)| !peer_fingerprints.contains(fingerprint))
                .collect();
        
        if !local_only_accounts.is_empty() {
            info!(
                "Pushing {} local-only accounts back to peer {} after sync",
                local_only_accounts.len(), peer
            );
            
            if let Err(e) = self.push_accounts_to_peer(peer, local_only_accounts).await {
                warn!("Failed to push local accounts to peer {}: {}", peer, e);
            }
        }

        let mut inner = self.inner.write().await;
        inner.stats.sync_requests += 1;
        
        Ok(())
    }

    /// Push accounts to peer - used to share local-only accounts after receiving a snapshot
    async fn push_accounts_to_peer(
        &self, 
        peer: &str, 
        accounts: std::collections::HashMap<String, rinku_core::types::Account>
    ) -> Result<()> {
        let client = reqwest::Client::new();
        let url = format!("{}/api/sync/merge-accounts", peer);
        
        #[derive(Serialize)]
        struct MergeRequest {
            accounts: std::collections::HashMap<String, rinku_core::types::Account>,
        }
        
        let response = client
            .post(&url)
            .json(&MergeRequest { accounts })
            .timeout(Duration::from_secs(30))
            .send()
            .await?;
        
        if response.status().is_success() {
            #[derive(Deserialize)]
            struct MergeResponse {
                added: usize,
                updated: usize,
            }
            
            if let Ok(result) = response.json::<MergeResponse>().await {
                info!(
                    "Pushed accounts to peer {}: {} added, {} updated",
                    peer, result.added, result.updated
                );
            }
        } else {
            warn!("Failed to push accounts to peer {}: HTTP {}", peer, response.status());
        }
        
        Ok(())
    }

    /// Force bootstrap from peer - used for recovery when delta sync fails due to state divergence
    /// This forces the snapshot to be applied even if checkpoint counts are equal
    async fn bootstrap_from_peer_force(&self, peer: &str) -> Result<()> {
        // CRITICAL: Before replacing our DAG, broadcast any pending transactions to the peer
        // This prevents locally-created transactions from being lost during snapshot sync
        self.flush_pending_txs_to_peer(peer).await;
        
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

        if self.sync_verify_strict {
            if snapshot_response.accounts_merkle_root.is_empty() {
                anyhow::bail!("MAINNET_MODE: Snapshot missing accounts_merkle_root");
            }
            let mut account_data: Vec<AccountData> = snapshot_response
                .accounts
                .iter()
                .map(|(address, account)| AccountData {
                    address: address.clone(),
                    balance: account.balance,
                    nonce: account.nonce,
                    stake: account.staked,
                })
                .collect();
            account_data.sort_by(|a, b| a.address.cmp(&b.address));
            let computed_root = build_account_merkle_root_sorted(&account_data);
            if computed_root != snapshot_response.accounts_merkle_root {
                anyhow::bail!(
                    "MAINNET_MODE: Snapshot accounts merkle mismatch (computed {} != received {})",
                    &computed_root[..16],
                    &snapshot_response.accounts_merkle_root[..16.min(snapshot_response.accounts_merkle_root.len())]
                );
            }
        }

        // Weak subjectivity check (same as regular bootstrap)
        if !self.trust_verifier.has_genesis_validators() {
            warn!("TESTNET MODE: Accepting snapshot without verification");
        }

        // CRITICAL: Validate genesis hash before applying snapshot
        let local_genesis_hash = self.state.get_genesis_hash().await;
        let peer_genesis_hash = snapshot_response.genesis_hash.clone();
        
        // Check if this node has ever successfully synced from the network.
        // If not, it's a new node that should adopt the peer's genesis hash.
        let has_synced = self.state.has_synced_from_network().await;
        
        if let (Some(local), Some(peer_gh)) = (&local_genesis_hash, &peer_genesis_hash) {
            if local != peer_gh {
                if !has_synced {
                    // New node that hasn't synced yet - adopt peer's genesis hash
                    info!(
                        "RECOVERY: New node adopting peer genesis hash from {} (local {} -> peer {})",
                        peer, &local[..16.min(local.len())], &peer_gh[..16.min(peer_gh.len())]
                    );
                } else {
                    // Established node that has synced before - reject mismatched genesis
                    warn!(
                        "GENESIS MISMATCH: Rejecting recovery sync from peer {} - local genesis {} != peer genesis {}",
                        peer, &local[..16.min(local.len())], &peer_gh[..16.min(peer_gh.len())]
                    );
                    anyhow::bail!(
                        "Genesis hash mismatch: this node is on a different chain than peer {}",
                        peer
                    );
                }
            }
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
            contracts: snapshot_response.contracts,
            rewards_snapshot: snapshot_response.rewards_snapshot,
            emission_snapshot: snapshot_response.emission_snapshot,
            slashing_snapshot: snapshot_response.slashing_snapshot,
            total_burned: snapshot_response.total_burned,
            total_to_validators: snapshot_response.total_to_validators,
            genesis_hash: peer_genesis_hash.clone(),
            finalized_tx_hashes: snapshot_response.finalized_tx_hashes,
            tx_checkpoint_heights: snapshot_response.tx_checkpoint_heights,
            weight_scores: snapshot_response.weight_scores,
        };

        // Get our local accounts before applying snapshot (to push back local-only accounts)
        let local_accounts_before = self.state.get_all_accounts_map().await;
        let peer_fingerprints: std::collections::HashSet<String> = 
            snapshot.accounts.keys().cloned().collect();
        
        // Use force apply to bypass checkpoint count check
        let added = self.state.apply_sync_snapshot_force(snapshot).await?;
        
        // CRITICAL: Sync the validator identity service with the synced validators
        // This ensures all nodes have the same validator registry for leader election
        self.sync_validator_identity_from_state().await;
        
        // Mark this node as having synced from the network
        self.state.mark_synced_from_network().await;
        
        if let Some(gh) = peer_genesis_hash {
            self.state.set_genesis_hash(gh.clone()).await;
            info!("Persisted peer genesis hash: {}", &gh[..16.min(gh.len())]);
        }
        
        warn!("RECOVERY: Force snapshot sync complete - applied {} DAG transactions", added);

        // Identify accounts that were local-only (not in peer's snapshot)
        // These need to be pushed back to the peer
        let local_only_accounts: std::collections::HashMap<String, rinku_core::types::Account> = 
            local_accounts_before
                .into_iter()
                .filter(|(fingerprint, _)| !peer_fingerprints.contains(fingerprint))
                .collect();
        
        if !local_only_accounts.is_empty() {
            info!(
                "Pushing {} local-only accounts back to peer {}",
                local_only_accounts.len(), peer
            );
            
            // Push local-only accounts back to peer
            if let Err(e) = self.push_accounts_to_peer(peer, local_only_accounts).await {
                warn!("Failed to push local accounts to peer {}: {}", peer, e);
            }
        }

        let mut inner = self.inner.write().await;
        inner.stats.sync_requests += 1;
        
        Ok(())
    }

    async fn gossip_round(&self, gossip_arc: &Arc<Self>) {
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
                
                // Cleanup stale peer_last_sync entries to prevent unbounded growth
                // Remove entries older than 60 seconds
                inner.peer_last_sync.retain(|_, last_sync| {
                    last_sync.elapsed().as_secs() < 60
                });
                
                // Cleanup stale peer_last_recovery entries (older than 60 seconds)
                inner.peer_last_recovery.retain(|_, last_recovery| {
                    last_recovery.elapsed().as_secs() < 60
                });
                
                // Cleanup stale peer_bloom_filters for disconnected peers
                let active_peer_ids: std::collections::HashSet<String> = inner.peers.keys().cloned().collect();
                let old_bloom_count = inner.peer_bloom_filters.len();
                inner.peer_bloom_filters.retain(|peer_id, _| active_peer_ids.contains(peer_id));
                let bloom_pruned = old_bloom_count - inner.peer_bloom_filters.len();
                if bloom_pruned > 0 {
                    info!("Pruned {} stale peer bloom filters", bloom_pruned);
                }
                
                // Cleanup fast_path_confirmed: Remove finalized entries older than 5 minutes
                // This prevents unbounded memory growth from tracking every transaction forever
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                const FAST_PATH_TTL_MS: u64 = 300_000; // 5 minutes
                let old_confirmed_count = inner.fast_path_confirmed.len();
                // Snapshot executed set to avoid borrow conflict with retain
                let executed_snapshot: std::collections::HashSet<String> = inner.fast_path_executed.clone();
                inner.fast_path_confirmed.retain(|tx_hash, finality| {
                    // Never prune entries that have been executed but not yet finalized
                    // These must survive until checkpoint anchors them so the API can report status
                    if executed_snapshot.contains(tx_hash) 
                        && !matches!(finality.status, rinku_core::types::FastPathStatus::Finalized) {
                        return true;
                    }
                    // TTL-based cleanup for other entries
                    if let Some(confirmed_at) = finality.confirmed_at_ms {
                        now_ms.saturating_sub(confirmed_at) < FAST_PATH_TTL_MS
                    } else {
                        now_ms.saturating_sub(finality.registered_at_ms) < FAST_PATH_TTL_MS
                    }
                });
                let confirmed_pruned = old_confirmed_count - inner.fast_path_confirmed.len();
                
                // Cleanup fast_path_pending: Remove stuck entries older than 10 minutes
                const FAST_PATH_PENDING_TTL_MS: u64 = 600_000; // 10 minutes
                let old_pending_count = inner.fast_path_pending.len();
                inner.fast_path_pending.retain(|_, finality| {
                    now_ms.saturating_sub(finality.registered_at_ms) < FAST_PATH_PENDING_TTL_MS
                });
                let pending_pruned = old_pending_count - inner.fast_path_pending.len();
                
                let old_executed_count = inner.fast_path_executed.len();
                let executed_to_remove: Vec<String> = inner.fast_path_executed.iter()
                    .filter(|tx_hash| {
                        match inner.fast_path_confirmed.get(*tx_hash) {
                            Some(finality) => matches!(finality.status, rinku_core::types::FastPathStatus::Finalized),
                            None => false,
                        }
                    })
                    .cloned()
                    .collect();
                for tx_hash in &executed_to_remove {
                    inner.fast_path_executed.remove(tx_hash);
                }
                let executed_pruned = old_executed_count - inner.fast_path_executed.len();
                
                if confirmed_pruned > 0 || pending_pruned > 0 || executed_pruned > 0 {
                    info!(
                        "Fast-path cleanup: {} confirmed, {} pending, {} executed pruned (remaining: {} confirmed, {} pending, {} executed)",
                        confirmed_pruned, pending_pruned, executed_pruned,
                        inner.fast_path_confirmed.len(), inner.fast_path_pending.len(), inner.fast_path_executed.len()
                    );
                }
                
                // Log stale nonce cache stats
                // The cache tracks (address -> min_expected_nonce) to quick-reject stale gossip
                let cache_size = inner.stale_nonce_cache.len();
                let cache_hits = inner.stale_nonce_cache_hits;
                let rejections = inner.stale_nonce_rejections;
                if cache_size > 0 || cache_hits > 0 || rejections > 0 {
                    info!(
                        "Stale nonce cache: {} addresses, {} cache hits, {} new rejections",
                        cache_size, cache_hits, rejections
                    );
                }
                
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

        // Also broadcast via libp2p if available
        #[cfg(feature = "p2p")]
        if self.network_handle.is_some() {
            self.broadcast_via_p2p(&message).await;
        }

        if success_count > 0 || fail_count > 0 {
            debug!(
                "Gossip round: {}/{} tips announced to {} peers ({} failed), dag_size={}",
                tips_announced, tips.len(), success_count, fail_count, dag_size
            );
        }

        // Spawn propagation as background task (non-blocking)
        Self::spawn_propagation_task(gossip_arc);
        
        self.request_sync_if_needed(checkpoint_height).await;
        
        // Periodic peer exchange: every 30 seconds, share known peers
        let should_exchange_peers = {
            let inner = self.inner.read().await;
            inner.round_counter % 150 == 0  // ~30s at 200ms interval
        };
        if should_exchange_peers {
            self.broadcast_peer_list().await;
        }

        self.broadcast_slashing_evidence_round().await;
    }

    fn evidence_id(evidence: &DoubleSignEvidence) -> String {
        format!(
            "{}:{}:{}:{}",
            evidence.validator,
            evidence.checkpoint_height,
            evidence.hash1,
            evidence.hash2
        )
    }

    async fn broadcast_slashing_evidence_round(&self) {
        let pending: Vec<DoubleSignEvidence> = {
            let slashing = self.state.slashing.read().await;
            slashing.get_pending_evidence().into_iter().cloned().collect()
        };

        if pending.is_empty() {
            return;
        }

        let public_url = std::env::var("PUBLIC_URL").ok();
        let peers: Vec<String> = {
            let inner = self.inner.read().await;
            inner.peers.keys().cloned().collect()
        };
        
        for evidence in pending {
            let evidence_key = Self::evidence_id(&evidence);
            let should_broadcast = {
                let mut inner = self.inner.write().await;
                inner.seen_evidence.insert(evidence_key)
            };
            if !should_broadcast {
                continue;
            }
            
            let message = GossipMessage::SlashingEvidence {
                evidence: evidence.clone(),
                sender_url: public_url.clone(),
            };

            for peer in &peers {
                let _ = self.send_to_peer(peer, &message).await;
            }

            #[cfg(feature = "p2p")]
            if self.network_handle.is_some() {
                self.broadcast_via_p2p(&message).await;
            }
        }
    }

    pub async fn broadcast_slashing_evidence(&self, evidence: DoubleSignEvidence) {
        let evidence_key = Self::evidence_id(&evidence);
        let should_broadcast = {
            let mut inner = self.inner.write().await;
            inner.seen_evidence.insert(evidence_key)
        };
        if !should_broadcast {
            return;
        }

        let public_url = std::env::var("PUBLIC_URL").ok();
        let peers: Vec<String> = {
            let inner = self.inner.read().await;
            inner.peers.keys().cloned().collect()
        };

        let message = GossipMessage::SlashingEvidence {
            evidence,
            sender_url: public_url,
        };

        for peer in &peers {
            let _ = self.send_to_peer(peer, &message).await;
        }

        #[cfg(feature = "p2p")]
        if self.network_handle.is_some() {
            self.broadcast_via_p2p(&message).await;
        }
    }
    
    /// Broadcast a weight attestation vote to all peers
    pub async fn broadcast_weight_vote(
        &self,
        tx_hash: String,
        validator_pubkey: String,
        vote: String,
        timestamp_ms: u64,
        bls_signature: Option<String>,
    ) {
        let public_url = std::env::var("PUBLIC_URL").ok();
        let peers: Vec<String> = {
            let inner = self.inner.read().await;
            inner.peers.keys().cloned().collect()
        };

        let message = GossipMessage::WeightVote {
            tx_hash: tx_hash.clone(),
            validator_pubkey,
            vote,
            timestamp_ms,
            bls_signature,
            sender_url: public_url,
        };

        for peer in &peers {
            let _ = self.send_to_peer(peer, &message).await;
        }

        #[cfg(feature = "p2p")]
        if self.network_handle.is_some() {
            self.broadcast_via_p2p(&message).await;
        }
        
        debug!("Broadcast weight vote for tx {} to {} peers", &tx_hash[..16.min(tx_hash.len())], peers.len());
    }
    
    /// Immediately broadcast a newly created checkpoint to all peers
    /// This is called by the leader after creating a checkpoint for fast propagation
    /// Includes precomputed proofs so followers can store them without regenerating
    /// CRITICAL: Also includes the actual finalized transactions so followers can execute them
    /// even if they missed the original transaction gossip (prevents balance divergence)
    pub async fn broadcast_checkpoint(
        &self, 
        checkpoint: Checkpoint, 
        finalized_tx_hashes: Vec<String>,
        finalized_transactions: Vec<SignedTransaction>,
        precomputed_proofs: Vec<rinku_core::types::AccountStateProof>,
    ) {
        let public_url = std::env::var("PUBLIC_URL").ok();
        let peers: Vec<String> = {
            let inner = self.inner.read().await;
            inner.peers.keys().cloned().collect()
        };
        
        info!(
            "Broadcasting checkpoint {} at height {} to {} peers ({} finalized txs, {} tx bodies)",
            &checkpoint.hash[..16.min(checkpoint.hash.len())],
            checkpoint.height,
            peers.len(),
            finalized_tx_hashes.len(),
            finalized_transactions.len()
        );
        
        // Upgrade fast-path status from 'confirmed' to 'finalized' for all finalized transactions
        // This ensures the explorer correctly shows "fast-path + finalized" for transactions
        // that achieved fast-path confirmation before checkpoint finalization (for leader node)
        {
            let mut inner = self.inner.write().await;
            for tx_hash in &finalized_tx_hashes {
                if let Some(finality) = inner.fast_path_confirmed.get_mut(tx_hash) {
                    if matches!(finality.status, rinku_core::types::FastPathStatus::Confirmed | rinku_core::types::FastPathStatus::Executed) {
                        finality.status = rinku_core::types::FastPathStatus::Finalized;
                        finality.checkpoint_height = Some(checkpoint.height);
                    }
                }
            }
        }

        let message = GossipMessage::CheckpointAnnouncement {
            checkpoint,
            sender_url: public_url,
            finalized_tx_hashes,
            precomputed_proofs,
            finalized_transactions,
        };

        // Broadcast via HTTP to all known peers
        for peer in &peers {
            let _ = self.send_to_peer(peer, &message).await;
        }

        // Also broadcast via P2P GossipSub for redundancy
        #[cfg(feature = "p2p")]
        if self.network_handle.is_some() {
            self.broadcast_via_p2p(&message).await;
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

    /// Non-blocking propagation: spawns a background task to propagate transactions
    /// so it doesn't block the gossip round or API handlers
    fn spawn_propagation_task(self: &Arc<Self>) {
        // Check if propagation is already in flight (non-blocking check)
        let should_spawn = {
            let inner = self.inner.try_read();
            match inner {
                Ok(guard) => !guard.propagation_in_flight && !guard.pending_txs.is_empty(),
                Err(_) => false, // Lock contention, skip this round
            }
        };
        
        if !should_spawn {
            return;
        }
        
        // Clone self for the spawned task
        let gossip = Arc::clone(self);
        
        tokio::spawn(async move {
            gossip.propagate_pending_txs_background().await;
        });
    }
    
    /// Background propagation task - runs in a separate tokio task
    async fn propagate_pending_txs_background(&self) {
        // Limit batch size to prevent overwhelming P2P layer and causing node crashes
        // This was the root cause of validator-1 crash during stress test (783 txs in one batch)
        const MAX_PROPAGATION_BATCH: usize = 100;
        
        // Set propagation_in_flight flag and take pending transactions (limited batch)
        let (batch_txs, overflow_txs, peers): (Vec<SignedTransaction>, Vec<SignedTransaction>, Vec<String>) = {
            let mut inner = self.inner.write().await;
            if inner.propagation_in_flight {
                return; // Another task is already propagating
            }
            inner.propagation_in_flight = true;
            let all_txs = std::mem::take(&mut inner.pending_txs);
            let peer_addrs = inner.peers.keys().cloned().collect();
            
            // Split into batch (to propagate now) and overflow (to re-queue)
            if all_txs.len() <= MAX_PROPAGATION_BATCH {
                (all_txs, Vec::new(), peer_addrs)
            } else {
                let (batch, overflow) = all_txs.split_at(MAX_PROPAGATION_BATCH);
                (batch.to_vec(), overflow.to_vec(), peer_addrs)
            }
        };
        
        if batch_txs.is_empty() {
            // Nothing to propagate, clear flag and return
            let mut inner = self.inner.write().await;
            inner.propagation_in_flight = false;
            return;
        }

        let batch_count = batch_txs.len();
        let overflow_count = overflow_txs.len();
        if overflow_count > 0 {
            info!("Background propagation: {} transactions to {} peers ({} deferred to next cycle)", 
                  batch_count, peers.len(), overflow_count);
        } else {
            info!("Background propagation: {} transactions to {} peers", batch_count, peers.len());
        }
        
        if peers.is_empty() {
            warn!("NO PEERS AVAILABLE - transactions will NOT be propagated!");
        }

        let public_url = std::env::var("PUBLIC_URL").ok();
        let mut propagated_count = 0u64;
        let mut tx_hashes: Vec<String> = Vec::with_capacity(batch_count);
        
        // Batch process: broadcast via p2p (limited to MAX_PROPAGATION_BATCH)
        for tx in &batch_txs {
            let message = GossipMessage::Transaction {
                hash: tx.hash.clone(),
                tx: tx.clone(),
                sender_url: public_url.clone(),
            };

            #[cfg(feature = "p2p")]
            self.broadcast_via_p2p(&message).await;
            
            tx_hashes.push(tx.hash.clone());
            propagated_count += 1;
        }
        
        // HTTP propagation: same batch (already limited to 100)
        if !batch_txs.is_empty() && !peers.is_empty() {
            for peer in &peers {
                let mut peer_failures = 0;
                for tx in &batch_txs {
                    let message = GossipMessage::Transaction {
                        hash: tx.hash.clone(),
                        tx: tx.clone(),
                        sender_url: public_url.clone(),
                    };
                    
                    if let Err(_) = self.send_to_peer(peer, &message).await {
                        peer_failures += 1;
                        // Stop sending to this peer after 3 consecutive failures
                        if peer_failures >= 3 {
                            debug!("Stopping HTTP propagation to {} after {} failures", peer, peer_failures);
                            break;
                        }
                    }
                }
            }
        }
        
        // Batch update stats and re-queue overflow transactions
        {
            let mut inner = self.inner.write().await;
            inner.stats.txs_propagated += propagated_count;
            for hash in tx_hashes {
                inner.known_txs.insert(hash);
            }
            
            // Re-queue overflow transactions for next propagation cycle
            if !overflow_txs.is_empty() {
                for tx in overflow_txs {
                    inner.pending_txs.push(tx);
                }
            }
            
            inner.propagation_in_flight = false;
        }
        
        debug!("Background propagation complete: {} transactions", propagated_count);
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
        // CRITICAL: Check debounce before any sync operations
        // This prevents sync storms that block the API under high volume
        const GLOBAL_SYNC_DEBOUNCE_SECS: u64 = 15;
        const PEER_SYNC_DEBOUNCE_SECS: u64 = 30;
        const SYNC_FAILURE_COOLDOWN_SECS: u64 = 30;
        // Minimum DAG difference to trigger delta sync - small diffs handled by gossip
        const MIN_DAG_DIFF_FOR_SYNC: usize = 50;
        
        {
            let inner = self.inner.read().await;
            
            // Check 1: Is sync already in flight?
            if inner.sync_in_flight {
                debug!("refresh_peer_status_and_sync: sync already in flight, skipping");
                return;
            }
            
            // Check 2: Are we in cooldown after a failed sync?
            if let Some(last_fail) = inner.last_sync_failure {
                if last_fail.elapsed().as_secs() < SYNC_FAILURE_COOLDOWN_SECS {
                    debug!("refresh_peer_status_and_sync: sync cooldown active, skipping");
                    return;
                }
            }
            
            // Check 3: Global sync debounce - did ANY sync happen recently?
            if let Some(last_sync) = inner.last_any_sync {
                if last_sync.elapsed().as_secs() < GLOBAL_SYNC_DEBOUNCE_SECS {
                    debug!("refresh_peer_status_and_sync: global debounce active, skipping");
                    return;
                }
            }
        }
        
        let peers: Vec<String> = {
            let inner = self.inner.read().await;
            inner.peers.keys().cloned().collect()
        };

        for peer in &peers {
            // Check per-peer debounce
            {
                let inner = self.inner.read().await;
                if let Some(last_sync) = inner.peer_last_sync.get(peer) {
                    if last_sync.elapsed().as_secs() < PEER_SYNC_DEBOUNCE_SECS {
                        debug!("refresh_peer_status_and_sync: peer {} synced recently, skipping", peer);
                        continue;
                    }
                }
            }
            
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

                    // Check if peer has significantly more data than us
                    // Small differences (<50 txs) are handled by gossip propagation
                    let dag_diff = (status.dag_size as usize).saturating_sub(local_dag_size);
                    let needs_checkpoint_sync = status.checkpoint_height > local_checkpoint;
                    let needs_dag_sync = dag_diff >= MIN_DAG_DIFF_FOR_SYNC;

                    if needs_checkpoint_sync || needs_dag_sync {
                        // Mark sync in flight and update timestamps BEFORE starting sync
                        {
                            let mut inner = self.inner.write().await;
                            if inner.sync_in_flight {
                                debug!("refresh_peer_status_and_sync: sync started by another task, aborting");
                                return;
                            }
                            inner.sync_in_flight = true;
                            inner.last_any_sync = Some(std::time::Instant::now());
                            inner.peer_last_sync.insert(peer.clone(), std::time::Instant::now());
                        }
                        
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
                            
                            let sync_result = self.bootstrap_from_peer(peer, local_checkpoint).await;
                            
                            // Update sync state based on result
                            let mut inner = self.inner.write().await;
                            inner.sync_in_flight = false;
                            if sync_result.is_err() {
                                inner.last_sync_failure = Some(std::time::Instant::now());
                                warn!("Snapshot sync from {} failed: {:?}", peer, sync_result.err());
                            } else {
                                inner.last_sync_failure = None;
                            }
                            return; // Only sync from one peer per cycle
                        } else if dag_diff >= MIN_DAG_DIFF_FOR_SYNC {
                            // Same checkpoint height but peer has more unfinalized DAG transactions
                            // Try delta sync for just the recent transactions
                            info!(
                                "Peer {} has more DAG data (dag: {} vs {}, diff: {}), requesting delta sync...",
                                peer, status.dag_size, local_dag_size, dag_diff
                            );
                            
                            let result = self.sync_from_peer_optimized(peer, local_checkpoint, local_dag_size as u64).await;
                            
                            // Clear sync_in_flight after delta sync completes
                            let mut inner = self.inner.write().await;
                            inner.sync_in_flight = false;
                            
                            match result {
                                Ok(sync_result) => {
                                    inner.last_sync_failure = None;
                                    // If delta sync had failures, force snapshot to fix state divergence
                                    if sync_result.failed > 0 && sync_result.failed > sync_result.added {
                                        // Rate limit recovery attempts: max 1 per 30 seconds per peer
                                        let should_recover = if let Some(last_recovery) = inner.peer_last_recovery.get(peer) {
                                            last_recovery.elapsed().as_secs() >= 30
                                        } else {
                                            true
                                        };
                                        
                                        if should_recover {
                                            warn!(
                                                "Delta sync had {} failures vs {} added - forcing snapshot recovery",
                                                sync_result.failed, sync_result.added
                                            );
                                            // Record this recovery attempt
                                            inner.peer_last_recovery.insert(peer.to_string(), std::time::Instant::now());
                                            drop(inner); // Release lock before sync
                                            if let Err(e) = self.bootstrap_from_peer_force(peer).await {
                                                warn!("RECOVERY: Force snapshot sync from {} failed: {}", peer, e);
                                            }
                                        } else {
                                            debug!("Skipping recovery from {} - rate limited (30s cooldown)", peer);
                                        }
                                    }
                                }
                                Err(e) => {
                                    inner.last_sync_failure = Some(std::time::Instant::now());
                                    warn!("Sync from {} failed: {}", peer, e);
                                }
                            }
                            return; // Only sync from one peer per cycle
                        }
                    } else if status.checkpoint_height == local_checkpoint && status.faucet_balance > 0.0 {
                        // ACCOUNT STATE VERIFICATION: Even when checkpoints match, account states may differ
                        // This can happen when different faucets create different wallets on different nodes
                        // Compare faucet balance as a proxy for overall account state consistency
                        let local_faucet_balance = self.state.get_faucet_balance().await;
                        let balance_diff = (status.faucet_balance - local_faucet_balance).abs();
                        
                        // If faucet balances differ by more than 1.0 RKU, we likely have different accounts
                        // Merge accounts from peer regardless of which has higher faucet balance
                        // The snapshot apply now MERGES accounts instead of replacing, so this is safe
                        if balance_diff > 1.0 {
                            // Rate limit recovery attempts: max 1 per 30 seconds per peer
                            let should_recover = {
                                let inner = self.inner.read().await;
                                if let Some(last_recovery) = inner.peer_last_recovery.get(peer) {
                                    last_recovery.elapsed().as_secs() >= 30
                                } else {
                                    true
                                }
                            };
                            
                            if should_recover {
                                info!(
                                    "[AccountSync] Faucet balance differs from peer {}: local={:.2}, peer={:.2}, diff={:.2} - merging accounts",
                                    peer, local_faucet_balance, status.faucet_balance, balance_diff
                                );
                                // Record this recovery attempt
                                {
                                    let mut inner = self.inner.write().await;
                                    inner.peer_last_recovery.insert(peer.to_string(), std::time::Instant::now());
                                }
                                // Always sync from peer to merge their accounts into ours
                                // The apply_sync_snapshot now merges instead of replacing
                                if let Err(e) = self.bootstrap_from_peer_force(peer).await {
                                    warn!("[AccountSync] Merge sync from {} failed: {}", peer, e);
                                }
                            } else {
                                debug!("[AccountSync] Skipping merge from {} - rate limited (30s cooldown)", peer);
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

    /// P2P-based delta sync using libp2p request-response protocol
    /// This replaces the HTTP-based sync for true P2P operation
    #[cfg(feature = "p2p")]
    async fn sync_from_peer_p2p(&self, peer_id: &str, local_checkpoint: u64) -> Result<DeltaSyncResult> {
        use crate::network::SyncResponse;
        
        let handle = match &self.network_handle {
            Some(h) => h.clone(),
            None => return Err(anyhow::anyhow!("P2P network not available")),
        };
        
        info!("P2P delta sync from peer {} starting at checkpoint {}", peer_id, local_checkpoint);
        
        let mut result = DeltaSyncResult::default();
        let mut failed_count = 0usize;
        
        // Make P2P delta request
        let response = {
            let locked = handle.lock().await;
            locked.request_delta(peer_id, local_checkpoint).await?
        };
        
        match response {
            SyncResponse::Delta(delta) => {
                info!(
                    "P2P delta sync received: {} txs, {} checkpoints (from {} to {})",
                    delta.transactions.len(),
                    delta.new_checkpoints.len(),
                    delta.from_checkpoint,
                    delta.to_checkpoint
                );
                
                // ORDERING: First, apply all transactions before any checkpoints
                // This ensures checkpoints have their referenced transactions
                let mut tx_hashes_added = std::collections::HashSet::new();
                
                for tx_data in &delta.transactions {
                    let signed_tx = rinku_core::types::SignedTransaction {
                        hash: tx_data.hash.clone(),
                        tx: rinku_core::types::Transaction {
                            from: tx_data.from.clone(),
                            to: tx_data.to.clone(),
                            amount: tx_data.amount,
                            nonce: tx_data.nonce,
                            timestamp: tx_data.timestamp,
                            parents: tx_data.parents.clone(),
                            gas_price: Some(tx_data.gas_price),
                            gas_limit: None,
                            data: None,
                            signature: None,
                            kind: None,
                            memo: tx_data.memo.clone(),
                            references: tx_data.references.clone(),
                        },
                        signature: tx_data.signature.clone(),
                    };
                    
                    // Use sync-add which bypasses nonce validation for synced transactions
                    if let Err(e) = self.state.add_transaction_from_sync(signed_tx).await {
                        debug!("Failed to add synced tx {}: {}", tx_data.hash, e);
                        failed_count += 1;
                    } else {
                        tx_hashes_added.insert(tx_data.hash.clone());
                        result.added += 1;
                    }
                }
                
                // ORDERING: Set checkpoint heights BEFORE applying checkpoints
                // This ensures consistent state during finalization
                for (hash, height) in &delta.tx_checkpoint_heights {
                    self.state.set_tx_checkpoint_height(hash, *height).await;
                }
                
                // ORDERING: Now apply checkpoints (after all txs are in place)
                for cp_data in &delta.new_checkpoints {
                    let checkpoint = rinku_core::types::Checkpoint {
                        height: cp_data.height,
                        tx_merkle_root: cp_data.merkle_root.clone(),
                        state_root: String::new(),
                        receipt_root: String::new(),
                        timestamp: cp_data.timestamp,
                        previous_hash: cp_data.previous_hash.clone(),
                        tip_count: cp_data.tx_count as u32,
                        hash: cp_data.hash.clone().unwrap_or_default(),
                        signer_bitmap: None,
                        aggregated_signature: cp_data.signature.clone(),
                        validator_signatures: Vec::new(),
                        finalized_tx_hashes: Vec::new(),
                        weight_trie_root: String::new(),
                    };
                    
                    if let Err(e) = self.state.apply_checkpoint(checkpoint, None).await {
                        warn!("Failed to apply synced checkpoint {}: {}", cp_data.height, e);
                        failed_count += 1;
                    } else {
                        result.added += 1;
                    }
                }
                
                // Merge validators
                if !delta.validators.is_empty() {
                    let validators: std::collections::HashMap<String, Validator> = delta.validators.iter().map(|v| {
                        let validator = Validator {
                            address: v.address.clone(),
                            stake: v.stake,
                            first_stake_time: 0,
                            bls_public_key: Some(v.bls_public_key.clone()),
                            missed_checkpoints: 0,
                        };
                        (v.address.clone(), validator)
                    }).collect();
                    self.state.merge_validators_from_peer(&validators).await;
                }
                
                result.failed = failed_count;
                
                // ERROR HANDLING: If too many failures, return error to trigger retry
                if failed_count > 0 && failed_count > result.added {
                    warn!("P2P delta sync had more failures ({}) than successes ({})", failed_count, result.added);
                    return Err(anyhow::anyhow!("P2P sync partial failure: {} added, {} failed", result.added, failed_count));
                }
                
                info!("P2P delta sync complete: {} added, {} failed", result.added, failed_count);
            }
            SyncResponse::Error { message } => {
                return Err(anyhow::anyhow!("P2P sync error from peer: {}", message));
            }
            _ => {
                return Err(anyhow::anyhow!("Unexpected P2P response type"));
            }
        }
        
        Ok(result)
    }
    
    /// P2P-based snapshot sync using libp2p request-response protocol
    #[cfg(feature = "p2p")]
    async fn bootstrap_from_peer_p2p(&self, peer_id: &str) -> Result<()> {
        use crate::network::SyncResponse;
        
        let handle = match &self.network_handle {
            Some(h) => h.clone(),
            None => return Err(anyhow::anyhow!("P2P network not available")),
        };
        
        info!("P2P snapshot sync from peer {}", peer_id);
        
        let response = {
            let locked = handle.lock().await;
            locked.request_snapshot(peer_id).await?
        };
        
        match response {
            SyncResponse::Snapshot(snapshot) => {
                info!(
                    "P2P snapshot received: {} accounts, {} validators, {} checkpoints, {} txs",
                    snapshot.accounts.len(),
                    snapshot.validators.len(),
                    snapshot.checkpoints.len(),
                    snapshot.recent_txs.len()
                );
                
                // Apply snapshot to state
                self.state.apply_p2p_snapshot(snapshot).await?;
                
                info!("P2P snapshot sync complete");
                Ok(())
            }
            SyncResponse::Error { message } => {
                Err(anyhow::anyhow!("P2P snapshot error from peer: {}", message))
            }
            _ => {
                Err(anyhow::anyhow!("Unexpected P2P response type for snapshot"))
            }
        }
    }
    
    /// P2P-based peer status fetch using handshake
    #[cfg(feature = "p2p")]
    async fn fetch_peer_status_p2p(&self, peer_id: &str) -> Result<PeerSyncStatus> {
        use crate::network::{PeerHandshake, SyncResponse};
        
        let handle = match &self.network_handle {
            Some(h) => h.clone(),
            None => return Err(anyhow::anyhow!("P2P network not available")),
        };
        
        let local_checkpoint = self.state.get_checkpoint_height().await;
        let node_id = self.state.get_node_id().await;
        let validator_address = self.checkpoint_vote_signer.as_ref().map(|s| s.validator_address.clone());
        
        let handshake = PeerHandshake {
            protocol_version: "1.0.0".to_string(),
            chain_id: "rinku-testnet".to_string(),
            network_id: "testnet".to_string(),
            node_id,
            checkpoint_height: local_checkpoint,
            validator_address,
            capabilities: vec!["sync".to_string(), "gossip".to_string()],
        };
        
        let response = {
            let locked = handle.lock().await;
            locked.handshake(peer_id, handshake).await?
        };
        
        match response {
            SyncResponse::Handshake(peer_info) => {
                Ok(PeerSyncStatus {
                    checkpoint_height: peer_info.checkpoint_height,
                    dag_size: 0, // Not available via handshake
                    tip_count: 0,
                    tips: Vec::new(),
                    merkle_root: None,
                    faucet_balance: 0.0,
                })
            }
            SyncResponse::Error { message } => {
                Err(anyhow::anyhow!("P2P handshake error: {}", message))
            }
            _ => {
                Err(anyhow::anyhow!("Unexpected P2P response type for handshake"))
            }
        }
    }
    
    /// Trigger P2P sync from a connected peer (100% P2P, no HTTP fallback)
    #[cfg(feature = "p2p")]
    async fn trigger_sync_from_peer_p2p(&self, peer_id: &str) -> anyhow::Result<()> {
        let local_checkpoint = self.state.get_checkpoint_height().await;
        
        // Get peer status via P2P handshake
        let status = self.fetch_peer_status_p2p(peer_id).await
            .map_err(|e| anyhow::anyhow!("Failed to fetch peer status via P2P: {}", e))?;
        
        let checkpoint_gap = status.checkpoint_height.saturating_sub(local_checkpoint);
        
        if checkpoint_gap >= 2 {
            // Peer is significantly ahead - use snapshot sync
            info!("P2P sync: peer {} is {} checkpoint(s) ahead, using snapshot sync", peer_id, checkpoint_gap);
            self.bootstrap_from_peer_p2p(peer_id).await
                .map_err(|e| {
                    warn!("P2P snapshot sync from {} failed: {}", peer_id, e);
                    e
                })?;
        } else {
            // Same or 1 checkpoint ahead - use delta sync
            info!("P2P sync: peer {} at checkpoint {}, using delta sync", peer_id, status.checkpoint_height);
            self.sync_from_peer_p2p(peer_id, local_checkpoint).await
                .map_err(|e| {
                    warn!("P2P delta sync from {} failed: {}", peer_id, e);
                    e
                })?;
        }
        
        Ok(())
    }

    /// Optimized delta sync that starts from an offset based on local DAG size
    /// This avoids downloading the entire DAG when we only need recent transactions
    /// DEPRECATED: Use sync_from_peer_p2p instead for true P2P operation
    #[allow(dead_code)]
    async fn sync_from_peer_optimized(&self, peer: &str, local_checkpoint: u64, local_dag_size: u64) -> Result<DeltaSyncResult> {
        let client = reqwest::Client::new();
        // Start from an offset near our local DAG size, with a buffer for safety
        // This dramatically reduces bandwidth when we only need recent transactions
        let start_offset = if local_dag_size > 500 {
            (local_dag_size - 500) as usize // Buffer of 500 to catch any gaps
        } else {
            0
        };
        let mut offset = start_offset;
        let limit = 1000usize;
        let mut result = DeltaSyncResult::default();
        
        info!(
            "Optimized delta sync from {} starting at offset {} (local DAG: {})",
            peer, start_offset, local_dag_size
        );
        
        // Get local validators for bidirectional sync
        let local_validators = self.state.get_validators_map().await;
        
        loop {
            // Use POST for bidirectional validator sync
            let url = format!("{}/api/sync/delta", peer);
            
            #[derive(serde::Serialize)]
            #[serde(rename_all = "camelCase")]
            struct DeltaSyncRequest {
                from_checkpoint: u64,
                offset: usize,
                limit: usize,
                validators: std::collections::HashMap<String, Validator>,
            }
            
            let request_body = DeltaSyncRequest {
                from_checkpoint: local_checkpoint,
                offset,
                limit,
                validators: local_validators.clone(),
            };
            
            let response = client
                .post(&url)
                .json(&request_body)
                .timeout(Duration::from_secs(30))
                .send()
                .await?;
            
            if !response.status().is_success() {
                anyhow::bail!("Sync request failed with status {}", response.status());
            }
            
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct DeltaResponse {
                transactions: Vec<SignedTransaction>,
                #[serde(default)]
                account_states: std::collections::HashMap<String, Account>,
                #[serde(default)]
                total: usize,
                #[serde(default)]
                has_more: bool,
                #[serde(default)]
                validators: std::collections::HashMap<String, Validator>,
            }
            
            let response_text = response.text().await?;
            let delta: DeltaResponse = serde_json::from_str(&response_text)
                .map_err(|e| anyhow::anyhow!("Failed to parse sync response: {}", e))?;
            
            info!(
                "Optimized sync page: received {}/{} txs (offset={}, has_more={})", 
                delta.transactions.len(), delta.total, offset, delta.has_more
            );
            
            // Merge validators from peer
            if !delta.validators.is_empty() {
                self.state.merge_validators_from_peer(&delta.validators).await;
            }
            
            // Apply transactions (skip known ones)
            for tx in &delta.transactions {
                // Quick check if we already have this transaction
                if self.state.has_transaction(&tx.hash).await {
                    continue; // Skip - we already have it
                }
                
                match self.state.add_transaction(tx.clone()).await {
                    Ok(_) => result.added += 1,
                    Err(e) => {
                        // Only count as failure if it's not a "duplicate" type error
                        let err_msg = format!("{:?}", e);
                        if !err_msg.contains("already exists") && !err_msg.contains("duplicate") {
                            result.failed += 1;
                        }
                    }
                }
            }
            
            if !delta.has_more {
                break;
            }
            offset += limit;
        }
        
        info!("Optimized delta sync complete: {} added, {} failed", result.added, result.failed);
        Ok(result)
    }
    
    async fn sync_from_peer(&self, peer: &str, local_checkpoint: u64) -> Result<DeltaSyncResult> {
        let client = reqwest::Client::new();
        let mut offset = 0usize;
        // Increased from 500 to reduce round trips during high-volume sync
        let limit = 1000usize;
        let mut result = DeltaSyncResult::default();
        
        // Get local validators for bidirectional sync
        let local_validators = self.state.get_validators_map().await;
        
        loop {
            // Use POST for bidirectional validator sync
            let url = format!("{}/api/sync/delta", peer);
            
            #[derive(serde::Serialize)]
            #[serde(rename_all = "camelCase")]
            struct DeltaSyncRequest {
                from_checkpoint: u64,
                offset: usize,
                limit: usize,
                validators: std::collections::HashMap<String, Validator>,
            }
            
            let request_body = DeltaSyncRequest {
                from_checkpoint: local_checkpoint,
                offset,
                limit,
                validators: local_validators.clone(),
            };
            
            let response = client
                .post(&url)
                .json(&request_body)
                .timeout(Duration::from_secs(30))
                .send()
                .await?;
            
            if !response.status().is_success() {
                anyhow::bail!("Sync request failed with status {}", response.status());
            }
            
            // Response struct for paginated mode with account states
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct DeltaResponse {
                transactions: Vec<SignedTransaction>,
                #[serde(default)]
                account_nonces: std::collections::HashMap<String, u64>,
                #[serde(default)]
                account_states: std::collections::HashMap<String, Account>,
                #[serde(default)]
                total: usize,
                #[serde(default)]
                offset: usize,
                #[serde(default)]
                has_more: bool,
                #[serde(default)]
                new_checkpoints: Vec<CheckpointData>,
                #[serde(default)]
                tx_checkpoint_heights: std::collections::HashMap<String, u64>,
                #[serde(default)]
                from_checkpoint: u64,
                #[serde(default)]
                to_checkpoint: u64,
                #[serde(default)]
                validators: std::collections::HashMap<String, Validator>,
            }
            
            let response_text = response.text().await?;
            let delta: DeltaResponse = serde_json::from_str(&response_text)
                .map_err(|e| anyhow::anyhow!("Failed to parse sync response: {}", e))?;
            
            info!(
                "Sync page: received {}/{} transactions, {} account states (offset={}, has_more={})", 
                delta.transactions.len(), delta.total, delta.account_states.len(), delta.offset, delta.has_more
            );

            if self.sync_verify_strict {
                // Only require new_checkpoints when peer is actually ahead (has newer checkpoints)
                // Same-checkpoint delta sync (from_checkpoint == to_checkpoint) doesn't need new checkpoints
                // because both nodes already have the same checkpoint data
                let peer_has_newer_checkpoint = delta.to_checkpoint > delta.from_checkpoint;
                
                if peer_has_newer_checkpoint {
                    // Peer claims to have newer checkpoints - verify the data
                    if delta.new_checkpoints.is_empty() {
                        anyhow::bail!("MAINNET_MODE: Delta response missing new_checkpoints (peer claims newer checkpoint)");
                    }
                    if delta.tx_checkpoint_heights.is_empty() {
                        anyhow::bail!("MAINNET_MODE: Delta response missing tx_checkpoint_heights");
                    }
                    
                    let delta_transactions: Vec<TransactionData> = delta.transactions.iter().map(|tx| {
                        TransactionData {
                            hash: tx.hash.clone(),
                            from: tx.tx.from.clone(),
                            to: tx.tx.to.clone(),
                            amount: tx.tx.amount,
                            nonce: tx.tx.nonce,
                            timestamp: tx.tx.timestamp,
                            signature: tx.signature.clone(),
                            parents: tx.tx.parents.clone(),
                            gas_price: tx.tx.gas_price.unwrap_or(0.0),
                            memo: tx.tx.memo.clone(),
                            references: tx.tx.references.clone(),
                        }
                    }).collect();
                    let delta_data = DeltaData {
                        transactions: delta_transactions,
                        new_checkpoints: delta.new_checkpoints.clone(),
                        from_checkpoint: delta.from_checkpoint,
                        to_checkpoint: delta.to_checkpoint,
                        tx_checkpoint_heights: delta.tx_checkpoint_heights.clone(),
                        validators: Vec::new(),
                    };
                    match verify_delta(&delta_data) {
                        VerificationResult::Valid => {}
                        VerificationResult::Invalid(reason) => {
                            anyhow::bail!("MAINNET_MODE: Delta verification failed: {}", reason);
                        }
                        VerificationResult::Skipped(reason) => {
                            anyhow::bail!("MAINNET_MODE: Delta verification skipped: {}", reason);
                        }
                    }
                } else {
                    // Same-checkpoint sync - skip checkpoint verification since both nodes have the same checkpoint
                    debug!("Same-checkpoint delta sync: skipping checkpoint verification (from={}, to={})", 
                        delta.from_checkpoint, delta.to_checkpoint);
                }
            }
            
            // SECURITY: Delta sync is for same-checkpoint peers, so we only apply nonces (not full states)
            // Full account states are only applied during snapshot sync from peers with higher checkpoint
            // This prevents malicious peers from overwriting balances at equal checkpoint height
            if !delta.account_nonces.is_empty() {
                for (address, nonce) in &delta.account_nonces {
                    self.state.sync_account_nonce(address, *nonce).await;
                }
                debug!("Applied {} account nonces from delta sync peer", delta.account_nonces.len());
            }
            
            // VALIDATOR SYNC: Merge validators from peer to ensure all nodes converge
            // This is critical for leader election - nodes need consistent validator sets
            if !delta.validators.is_empty() {
                let merged_count = self.state.merge_validators_from_peer(&delta.validators).await;
                if merged_count > 0 {
                    info!("Merged {} validators from delta sync peer", merged_count);
                    // CRITICAL: Sync to ValidatorIdentityService for vote validation
                    // Without this, votes will fail BLS key verification
                    self.sync_validator_identity_from_state().await;
                }
            }
            
            let (transactions, has_more) = (delta.transactions, delta.has_more);
            
            if transactions.is_empty() {
                break;
            }
            
            // TRUST MODEL: Delta sync trusts the peer to provide valid transactions.
            // This is consistent with snapshot sync which also trusts peer state.
            // For adversarial environments, use checkpoint verification or peer allowlists.
            // 
            // Collect transactions that need to be processed
            // Filter out already-known transactions
            let mut pending: Vec<SignedTransaction> = Vec::new();
            {
                let inner = self.inner.read().await;
                for tx in transactions {
                    if !inner.known_txs.contains(&tx.hash) {
                        pending.push(tx);
                    }
                }
            }
            
            // Process with retry loop for parent ordering issues
            // Parents may arrive before children, so retry deferred txs
            let mut retries = 0;
            const MAX_RETRIES: usize = 3;
            
            while !pending.is_empty() && retries < MAX_RETRIES {
                let mut deferred: Vec<SignedTransaction> = Vec::new();
                let pending_count = pending.len();
                
                for tx in pending.drain(..) {
                    match self.state.add_transaction_dag_only(tx.clone()).await {
                        Ok(()) => {
                            let mut inner = self.inner.write().await;
                            inner.known_txs.insert(tx.hash.clone());
                            inner.stats.txs_received += 1;
                            result.added += 1;
                        }
                        Err(e) => {
                            let err_msg = e.to_string();
                            if err_msg.contains("Parent") && err_msg.contains("not found") {
                                // Defer for retry - parent might arrive later
                                deferred.push(tx);
                            } else {
                                debug!("Failed to add synced tx {}: {}", tx.hash, e);
                                result.failed += 1;
                            }
                        }
                    }
                }
                
                if deferred.is_empty() {
                    break;
                }
                
                // If no progress was made (all txs deferred again), stop retrying
                if deferred.len() == pending_count {
                    retries += 1;
                }
                
                pending = deferred;
            }
            
            // Any remaining deferred txs are failures (parents never arrived)
            if !pending.is_empty() {
                debug!("Delta sync: {} transactions deferred (missing parents)", pending.len());
                result.failed += pending.len();
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
        // Check if peer is in backoff
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        {
            let inner = self.inner.read().await;
            if let Some(peer_info) = inner.peers.get(peer) {
                if peer_info.backoff_until > now {
                    return Err(anyhow::anyhow!("Peer {} is in backoff until {}", peer, peer_info.backoff_until));
                }
            }
        }

        let url = format!("{}/api/gossip", peer);

        let start = std::time::Instant::now();
        let result = self.http_client
            .post(&url)
            .json(message)
            .timeout(Duration::from_secs(5))
            .send()
            .await;

        match result {
            Ok(response) if response.status().is_success() => {
                let latency = start.elapsed().as_millis() as u64;
                let mut inner = self.inner.write().await;
                if let Some(peer_info) = inner.peers.get_mut(peer) {
                    peer_info.latency_ms = latency;
                    peer_info.last_seen = now;
                    peer_info.is_healthy = true;
                    peer_info.consecutive_failures = 0;
                    peer_info.backoff_until = 0;
                }
                Ok(())
            }
            Ok(_) | Err(_) => {
                // Track failure and apply exponential backoff
                let mut inner = self.inner.write().await;
                if let Some(peer_info) = inner.peers.get_mut(peer) {
                    peer_info.consecutive_failures += 1;
                    peer_info.is_healthy = false;
                    // Exponential backoff: 10s, 20s, 40s, 80s, max 5 minutes
                    let backoff_secs = std::cmp::min(10 * (1 << peer_info.consecutive_failures.min(5)), 300);
                    peer_info.backoff_until = now + backoff_secs;
                    if peer_info.consecutive_failures == 1 || peer_info.consecutive_failures % 10 == 0 {
                        warn!("Peer {} failed {} times, backoff for {}s", peer, peer_info.consecutive_failures, backoff_secs);
                    }
                    inner.stats.failed_sends += 1;
                }
                Err(anyhow::anyhow!("Failed to send to peer"))
            }
        }
    }

    pub async fn handle_message(&self, message: GossipMessage) -> Result<Option<GossipMessage>> {
        // Centralized reverse peer discovery: extract sender_url from any message type
        let sender_url = Self::extract_sender_url(&message);
        if let Some(ref url) = sender_url {
            debug!("Gossip message received with sender_url: {}", url);
            self.add_peer(url.clone()).await;
        } else {
            debug!("Gossip message received WITHOUT sender_url (peer won't be discovered)");
        }
        
        match message {
            GossipMessage::Transaction { hash, tx, .. } => {
                // QUICK-REJECT: Check stale nonce cache BEFORE any state operations
                // This prevents CPU-intensive validation of known-stale transactions
                {
                    let inner = self.inner.read().await;
                    if let Some(&min_nonce) = inner.stale_nonce_cache.get(&tx.tx.from) {
                        if tx.tx.nonce < min_nonce {
                            // Silently drop - no logging to avoid log spam
                            // This is a known-stale transaction from gossip echo
                            drop(inner);
                            let mut inner_mut = self.inner.write().await;
                            inner_mut.stale_nonce_cache_hits += 1;
                            return Ok(None);
                        }
                    }
                }
                
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
                    match self.state.add_transaction(tx.clone()).await {
                        Ok(TransactionResult::Accepted) => {
                            // Only propagate fully accepted transactions
                            let mut inner = self.inner.write().await;
                            // Limit pending_txs to prevent OOM under high load
                            if inner.pending_txs.len() < PENDING_TXS_MAX {
                                inner.pending_txs.push(tx);
                            } else {
                                debug!("Pending TX queue full ({} txs), dropping propagation for {}", 
                                    PENDING_TXS_MAX, &hash[..16.min(hash.len())]);
                            }
                        }
                        Ok(TransactionResult::Buffered) => {
                            // Transaction was buffered for later - do NOT propagate
                            debug!("Gossiped tx {} buffered (future nonce), not propagating", hash);
                        }
                        Err(e) => {
                            // Check if this is a stale nonce error and update cache
                            let err_str = e.to_string();
                            if err_str.contains("Stale nonce") {
                                // Get the actual account nonce from state (robust, not string parsing)
                                let expected_nonce = if let Some(account) = self.state.get_account(&tx.tx.from).await {
                                    account.nonce
                                } else {
                                    0 // New account, expected nonce is 0
                                };
                                
                                let mut inner = self.inner.write().await;
                                let current = inner.stale_nonce_cache.get(&tx.tx.from).copied().unwrap_or(0);
                                
                                // Only update if new nonce is higher
                                if expected_nonce > current {
                                    // Check if this is a new entry or update
                                    let is_new_entry = !inner.stale_nonce_cache.contains_key(&tx.tx.from);
                                    inner.stale_nonce_cache.insert(tx.tx.from.clone(), expected_nonce);
                                    
                                    if is_new_entry {
                                        // Track insertion order for FIFO eviction
                                        inner.stale_nonce_cache_order.push_back(tx.tx.from.clone());
                                        
                                        // FIFO eviction when cache is full
                                        while inner.stale_nonce_cache.len() > STALE_NONCE_CACHE_MAX {
                                            if let Some(oldest_addr) = inner.stale_nonce_cache_order.pop_front() {
                                                inner.stale_nonce_cache.remove(&oldest_addr);
                                            } else {
                                                break;
                                            }
                                        }
                                    }
                                }
                                inner.stale_nonce_rejections += 1;
                            } else {
                                warn!("Failed to add gossiped tx {}: {}", hash, e);
                            }
                        }
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
                
                // Sync cooldown and debounce constants
                // Increased from 10s/5s to reduce sync storm that overwhelms API
                const SYNC_FAILURE_COOLDOWN_SECS: u64 = 30;
                const PEER_SYNC_DEBOUNCE_SECS: u64 = 30;
                const GLOBAL_SYNC_DEBOUNCE_SECS: u64 = 15;
                // Minimum DAG difference to trigger sync - small diffs handled by gossip
                const MIN_DAG_DIFF_FOR_SYNC: usize = 50;
                
                if dag_size > local_dag_size + MIN_DAG_DIFF_FOR_SYNC {
                    // Peer has significantly more DAG data (50+ transactions ahead)
                    // Check cooldowns, debounce, and if sync is already in flight
                    let should_sync = {
                        let mut inner = self.inner.write().await;
                        
                        // Check 1: Is sync already in flight?
                        if inner.sync_in_flight {
                            debug!("TipAnnouncement: sync already in flight, skipping");
                            return Ok(None);
                        }
                        
                        // Check 2: Are we in cooldown after a failed sync?
                        if let Some(last_fail) = inner.last_sync_failure {
                            if last_fail.elapsed().as_secs() < SYNC_FAILURE_COOLDOWN_SECS {
                                debug!("TipAnnouncement: sync cooldown active ({}s remaining), skipping", 
                                    SYNC_FAILURE_COOLDOWN_SECS - last_fail.elapsed().as_secs());
                                return Ok(None);
                            }
                        }
                        
                        // Check 3: Global sync debounce - did ANY sync happen recently?
                        if let Some(last_sync) = inner.last_any_sync {
                            if last_sync.elapsed().as_secs() < GLOBAL_SYNC_DEBOUNCE_SECS {
                                debug!("TipAnnouncement: global sync debounce active ({}s remaining), skipping",
                                    GLOBAL_SYNC_DEBOUNCE_SECS - last_sync.elapsed().as_secs());
                                return Ok(None);
                            }
                        }
                        
                        // Check 4: Per-peer debounce - did we try this peer recently?
                        if let Some(ref peer_url) = sender_url {
                            if let Some(last_sync) = inner.peer_last_sync.get(peer_url) {
                                if last_sync.elapsed().as_secs() < PEER_SYNC_DEBOUNCE_SECS {
                                    debug!("TipAnnouncement: peer {} synced recently, skipping", peer_url);
                                    return Ok(None);
                                }
                            }
                            // Record this sync attempt for the peer
                            inner.peer_last_sync.insert(peer_url.clone(), std::time::Instant::now());
                        }
                        
                        // All checks passed, proceed with sync
                        inner.sync_in_flight = true;
                        inner.last_any_sync = Some(std::time::Instant::now());
                        true
                    };
                    
                    if should_sync {
                        // P2P-only sync: Only trigger sync if we have a valid P2P peer ID
                        // HTTP sync has been deprecated - ignore HTTP URLs
                        if let Some(peer_url) = sender_url {
                            if peer_url.starts_with("12D3") {
                                // Valid P2P peer ID
                                info!(
                                    "TipAnnouncement: P2P peer {} has {} DAG nodes vs our {} - spawning background sync",
                                    &peer_url[..16.min(peer_url.len())], dag_size, local_dag_size
                                );
                                
                                // Spawn background task to avoid blocking the gossip handler
                                let gossip_clone = self.clone();
                                let peer_url_clone = peer_url.clone();
                                tokio::spawn(async move {
                                    let sync_result = gossip_clone.trigger_sync_from_peer(&peer_url_clone).await;
                                    
                                    // Update sync state based on result
                                    let mut inner = gossip_clone.inner.write().await;
                                    inner.sync_in_flight = false;
                                    
                                    // If sync failed, set cooldown to prevent sync storm
                                    if sync_result.is_err() {
                                        inner.last_sync_failure = Some(std::time::Instant::now());
                                        debug!("Sync from {} failed, entering cooldown", peer_url_clone);
                                    } else {
                                        // Clear failure state on success
                                        inner.last_sync_failure = None;
                                    }
                                });
                            } else {
                                // HTTP URL - skip sync, clear the flag
                                // Sync will happen via P2P gossip and checkpoint tx bodies
                                debug!(
                                    "TipAnnouncement: skipping HTTP peer {} - using P2P-only sync",
                                    &peer_url[..40.min(peer_url.len())]
                                );
                                let mut inner = self.inner.write().await;
                                inner.sync_in_flight = false;
                            }
                        } else {
                            // No sender_url, clear the flag
                            let mut inner = self.inner.write().await;
                            inner.sync_in_flight = false;
                        }
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

                // Get own URL for self-filtering
                let own_url = std::env::var("PUBLIC_URL").ok();
                let own_normalized = own_url.as_ref().map(|u| u.trim_end_matches('/').to_string());

                let mut inner = self.inner.write().await;
                for addr in peers {
                    // Skip adding self as a peer
                    let addr_normalized = addr.trim_end_matches('/');
                    if own_normalized.as_deref() == Some(addr_normalized) {
                        continue;
                    }
                    
                    if !inner.peers.contains_key(&addr) {
                        inner.peers.insert(addr.clone(), PeerInfo {
                            address: addr,
                            node_id: node_id.clone(),
                            last_seen: now,
                            dag_size: 0,
                            checkpoint_height: 0,
                            latency_ms: 0,
                            is_healthy: true,
                            consecutive_failures: 0,
                            backoff_until: 0,
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
                        match self.state.add_transaction(tx).await {
                            Ok(TransactionResult::Accepted) => {
                                // Transaction fully processed
                            }
                            Ok(TransactionResult::Buffered) => {
                                debug!("Synced tx {} buffered (future nonce)", hash);
                            }
                            Err(e) => {
                                debug!("Failed to add synced tx {}: {}", hash, e);
                            }
                        }
                    }
                }
                Ok(None)
            }

            GossipMessage::CheckpointSignature { .. } => {
                Ok(None)
            }

            GossipMessage::BloomAnnouncement { filter, checkpoint_height, tip_count, sender_url } => {
                debug!(
                    "Received bloom filter from peer with {} items, checkpoint height {}, {} tips",
                    filter.item_count(), checkpoint_height, tip_count
                );
                
                if let Some(peer_url) = sender_url {
                    let mut inner = self.inner.write().await;
                    inner.peer_bloom_filters.insert(peer_url, filter);
                }
                
                Ok(None)
            }

            GossipMessage::SlashingEvidence { evidence, .. } => {
                if !self.state.verify_slashing_evidence(&evidence).await {
                    warn!(
                        "Rejected slashing evidence (invalid signature) for {} at height {}",
                        &evidence.validator[..16.min(evidence.validator.len())],
                        evidence.checkpoint_height
                    );
                    return Ok(None);
                }
                let evidence_key = Self::evidence_id(&evidence);
                let is_new = {
                    let mut inner = self.inner.write().await;
                    inner.seen_evidence.insert(evidence_key)
                };
                if is_new {
                    let mut slashing = self.state.slashing.write().await;
                    let accepted = slashing.submit_double_sign_evidence(
                        evidence.validator.clone(),
                        evidence.checkpoint_height,
                        evidence.hash1.clone(),
                        evidence.hash2.clone(),
                        evidence.signature1.clone(),
                        evidence.signature2.clone(),
                    );
                    if accepted.is_some() {
                        info!(
                            "Accepted slashing evidence for {} at height {}",
                            &evidence.validator[..16.min(evidence.validator.len())],
                            evidence.checkpoint_height
                        );
                    }
                }
                Ok(None)
            }
            
            GossipMessage::WeightVote { tx_hash, validator_pubkey, vote, timestamp_ms, bls_signature, .. } => {
                use rinku_core::types::{WeightVote as WV, PendingWeightVote};
                
                let vote_type = match vote.to_lowercase().as_str() {
                    "boost" => WV::Boost,
                    "suppress" => WV::Suppress,
                    "neutral" => WV::Neutral,
                    _ => {
                        debug!("Received invalid vote type via gossip: {}", vote);
                        return Ok(None);
                    }
                };
                
                let pending_vote = PendingWeightVote {
                    tx_hash: tx_hash.clone(),
                    validator_pubkey: validator_pubkey.clone(),
                    vote: vote_type,
                    timestamp_ms,
                    bls_signature,
                };
                
                let mut state = self.state.inner.write().await;
                if let Some(ref mut wt) = state.weight_trie {
                    wt.add_vote(pending_vote);
                    debug!("Applied gossiped weight vote for tx {} from {}", 
                        &tx_hash[..16.min(tx_hash.len())], 
                        &validator_pubkey[..16.min(validator_pubkey.len())]);
                }
                
                Ok(None)
            }
            
            GossipMessage::CheckpointAnnouncement { checkpoint, finalized_tx_hashes, precomputed_proofs, finalized_transactions, .. } => {
                // Immediately apply checkpoint from network leader
                let local_height = self.state.get_checkpoint_height().await;
                
                if checkpoint.height <= local_height {
                    debug!(
                        "Ignoring checkpoint {} at height {} (local height: {})",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        checkpoint.height,
                        local_height
                    );
                    return Ok(None);
                }
                
                // Check if this is the next expected checkpoint
                if checkpoint.height == local_height + 1 {
                    info!(
                        "Received checkpoint announcement {} at height {} (leader broadcast, {} finalized txs, {} tx bodies, {} proofs)",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        checkpoint.height,
                        finalized_tx_hashes.len(),
                        finalized_transactions.len(),
                        precomputed_proofs.len()
                    );
                    
                    // CRITICAL CONSENSUS FIX: Add missing transactions from leader BEFORE applying checkpoint
                    // This ensures all nodes execute the SAME transactions and reach the SAME state
                    // Without this, nodes that missed the original gossip would have different balances
                    let mut added_from_leader = 0usize;
                    let mut rejected_count = 0usize;
                    
                    // Build set of finalized hashes for validation
                    let finalized_hash_set: std::collections::HashSet<&String> = 
                        finalized_tx_hashes.iter().collect();
                    
                    if !finalized_transactions.is_empty() {
                        // Build lookup of transactions we already have
                        let existing_hashes: std::collections::HashSet<String> = {
                            let state = self.state.inner.read().await;
                            finalized_tx_hashes.iter()
                                .filter(|h| state.dag.get_node(h).is_some())
                                .cloned()
                                .collect()
                        };
                        
                        // Add missing transactions from leader WITH SECURITY VALIDATION
                        for tx in &finalized_transactions {
                            if existing_hashes.contains(&tx.hash) {
                                continue; // Already have this transaction
                            }
                            
                            // SECURITY: Verify tx hash is in the finalized list (prevents injection)
                            // Note: We trust the leader's hash since JSON serialization order is non-deterministic
                            // and recomputing the hash would produce false mismatches. The finalized hash list
                            // is signed by the leader, providing the security guarantee we need.
                            if !finalized_hash_set.contains(&tx.hash) {
                                warn!(
                                    "SECURITY: Rejecting tx {} - not in finalized hash list for checkpoint {}",
                                    &tx.hash[..16.min(tx.hash.len())],
                                    checkpoint.height
                                );
                                rejected_count += 1;
                                continue;
                            }
                            
                            // Add validated transaction to DAG
                            match self.state.force_add_transaction_for_vote(tx.clone()).await {
                                Ok(_) => {
                                    added_from_leader += 1;
                                }
                                Err(e) => {
                                    debug!(
                                        "Failed to add tx {} from checkpoint: {}",
                                        &tx.hash[..16.min(tx.hash.len())],
                                        e
                                    );
                                }
                            }
                        }
                        
                        if added_from_leader > 0 || rejected_count > 0 {
                            info!(
                                "Checkpoint {} tx sync: added {} from leader, rejected {} (security)",
                                checkpoint.height,
                                added_from_leader,
                                rejected_count
                            );
                        }
                    }
                    
                    // Apply the checkpoint with the finalized transaction hashes
                    // Now we should have ALL transactions that the leader finalized
                    // Clone hashes before move so we can use them for fast-path status upgrade later
                    let finalized_hashes_for_fastpath = finalized_tx_hashes.clone();
                    // Get fast-path executed set to skip double-execution at checkpoint
                    let fp_executed_set = {
                        let inner = self.inner.read().await;
                        inner.fast_path_executed.clone()
                    };
                    match self.state.apply_checkpoint_with_finalized_hashes(
                        checkpoint.clone(), 
                        finalized_tx_hashes,
                        &fp_executed_set,
                    ).await {
                        Err(e) => {
                            warn!(
                                "Failed to apply announced checkpoint {}: {}",
                                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                                e
                            );
                        }
                        Ok(missing_tx_count) => {
                            // After adding missing txs from leader, missing_tx_count should be 0
                            // If not, log a warning as this indicates a serious consensus issue
                            if missing_tx_count > 0 {
                                warn!(
                                    "CONSENSUS WARNING: Still missing {} txs after leader sync for checkpoint {} (added {} from leader, had {} tx bodies)",
                                    missing_tx_count,
                                    checkpoint.height,
                                    added_from_leader,
                                    finalized_transactions.len()
                                );
                            }
                            
                            // Store precomputed proofs if we have all transactions
                            if missing_tx_count == 0 && !precomputed_proofs.is_empty() {
                                let proof_map: std::collections::HashMap<String, rinku_core::types::AccountStateProof> = 
                                    precomputed_proofs.into_iter()
                                        .map(|p| (p.address.clone(), p))
                                        .collect();
                                self.state.store_precomputed_proofs(&proof_map).await;
                                info!(
                                    "Stored {} precomputed proofs from leader for checkpoint {}",
                                    proof_map.len(),
                                    checkpoint.height
                                );
                            } else if missing_tx_count > 0 && !precomputed_proofs.is_empty() {
                                // DO NOT store proofs - they would have incorrect nonce/balance
                                warn!(
                                    "Skipped storing {} proofs for checkpoint {} - missing {} txs (proofs would have incorrect state)",
                                    precomputed_proofs.len(),
                                    checkpoint.height,
                                    missing_tx_count
                                );
                            }
                            
                            // Upgrade fast-path status from 'confirmed' to 'finalized' for all finalized transactions
                            // This ensures the explorer correctly shows "fast-path + finalized" for transactions
                            // that achieved fast-path confirmation before checkpoint finalization
                            {
                                let mut inner = self.inner.write().await;
                                for tx_hash in &finalized_hashes_for_fastpath {
                                    if let Some(finality) = inner.fast_path_confirmed.get_mut(tx_hash) {
                                        if matches!(finality.status, rinku_core::types::FastPathStatus::Confirmed | rinku_core::types::FastPathStatus::Executed) {
                                            finality.status = rinku_core::types::FastPathStatus::Finalized;
                                            finality.checkpoint_height = Some(checkpoint.height);
                                        }
                                    }
                                }
                            }
                            
                            info!(
                                "Applied announced checkpoint {} at height {} (added {} txs from leader)",
                                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                                checkpoint.height,
                                added_from_leader
                            );
                        }
                    }
                } else {
                    debug!(
                        "Checkpoint {} at height {} too far ahead (local: {}), will sync",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        checkpoint.height,
                        local_height
                    );
                }
                
                Ok(None)
            }
            
            GossipMessage::FastPathBroadcast {
                tx,
                sender_validator,
                sender_stake,
                timestamp_ms,
                ..
            } => {
                if !tx.is_fast_path_eligible() {
                    debug!(
                        "FastPathBroadcast rejected: tx {} not eligible for fast-path",
                        &tx.hash[..16.min(tx.hash.len())]
                    );
                    return Ok(None);
                }

                let is_new = {
                    let mut inner = self.inner.write().await;
                    if inner.known_txs.contains(&tx.hash) {
                        false
                    } else {
                        inner.known_txs.insert(tx.hash.clone());
                        inner.stats.txs_received += 1;
                        true
                    }
                };

                if is_new {
                    match self.state.add_transaction(tx.clone()).await {
                        Ok(TransactionResult::Accepted) | Ok(TransactionResult::Buffered) => {
                            let validators = self.state.get_validators_map().await;
                            let total_stake: f64 = validators.values().map(|v| v.stake).sum();
                            let quorum_threshold = total_stake * 2.0 / 3.0;
                            
                            let now_ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64;
                            
                            let mut confirmed_finality: Option<rinku_core::types::FastPathFinality> = None;
                            
                            {
                                let mut inner = self.inner.write().await;
                                let finality = inner.fast_path_pending.entry(tx.hash.clone()).or_insert_with(|| {
                                    rinku_core::types::FastPathFinality {
                                        tx_hash: tx.hash.clone(),
                                        status: rinku_core::types::FastPathStatus::Pending,
                                        acks: Vec::new(),
                                        total_stake_acked: 0.0,
                                        quorum_stake_required: quorum_threshold,
                                        registered_at_ms: timestamp_ms,
                                        confirmed_at_ms: None,
                                        checkpoint_height: None,
                                    }
                                });
                                
                                if validators.contains_key(&sender_validator) && sender_stake > 0.0 {
                                    let already_acked = finality.acks.iter().any(|a| a.validator_address == sender_validator);
                                    if !already_acked {
                                        finality.acks.push(rinku_core::types::FastPathAck {
                                            tx_hash: tx.hash.clone(),
                                            validator_address: sender_validator.clone(),
                                            validator_stake: sender_stake,
                                            bls_signature: None,
                                            timestamp_ms,
                                        });
                                        finality.total_stake_acked += sender_stake;
                                    }
                                }
                                
                                if finality.total_stake_acked >= quorum_threshold && finality.status == rinku_core::types::FastPathStatus::Pending {
                                    finality.status = rinku_core::types::FastPathStatus::Confirmed;
                                    finality.confirmed_at_ms = Some(now_ms);
                                    confirmed_finality = Some(finality.clone());
                                }
                            }
                            
                            if let Some(finalized) = confirmed_finality {
                                let stake_acked = finalized.total_stake_acked;
                                let mut inner = self.inner.write().await;
                                inner.fast_path_confirmed.insert(tx.hash.clone(), finalized);
                                inner.fast_path_pending.remove(&tx.hash);
                                
                                info!(
                                    "FastPath CONFIRMED for {} with {:.2} stake (threshold: {:.2})",
                                    &tx.hash[..16.min(tx.hash.len())],
                                    stake_acked,
                                    quorum_threshold
                                );
                                drop(inner);
                                self.execute_on_fast_path(&tx).await;
                            }
                            
                            let (our_validator, our_stake) = self.state.get_validator_info().await;
                            
                            if let (Some(validator_addr), Some(_)) = (our_validator, our_stake) {
                                let our_stake = self.state.get_validator_stake(&validator_addr).await.unwrap_or(0.0);
                                
                                if our_stake > 0.0 {
                                    let mut just_confirmed = false;
                                    {
                                        let mut inner = self.inner.write().await;
                                        if let Some(finality) = inner.fast_path_pending.get_mut(&tx.hash) {
                                            let already_acked = finality.acks.iter().any(|a| a.validator_address == validator_addr);
                                            if !already_acked {
                                                finality.acks.push(rinku_core::types::FastPathAck {
                                                    tx_hash: tx.hash.clone(),
                                                    validator_address: validator_addr.clone(),
                                                    validator_stake: our_stake,
                                                    bls_signature: None,
                                                    timestamp_ms: now_ms,
                                                });
                                                finality.total_stake_acked += our_stake;
                                                
                                                if finality.total_stake_acked >= quorum_threshold && finality.status == rinku_core::types::FastPathStatus::Pending {
                                                    finality.status = rinku_core::types::FastPathStatus::Confirmed;
                                                    finality.confirmed_at_ms = Some(now_ms);
                                                    let confirmed = finality.clone();
                                                    let stake = finality.total_stake_acked;
                                                    inner.fast_path_confirmed.insert(tx.hash.clone(), confirmed);
                                                    just_confirmed = true;
                                                    
                                                    info!(
                                                        "FastPath CONFIRMED for {} with {:.2} stake (threshold: {:.2})",
                                                        &tx.hash[..16.min(tx.hash.len())],
                                                        stake,
                                                        quorum_threshold
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    
                                    if just_confirmed {
                                        self.execute_on_fast_path(&tx).await;
                                    }
                                    
                                    let ack = GossipMessage::FastPathAck {
                                        tx_hash: tx.hash.clone(),
                                        validator_address: validator_addr,
                                        validator_stake: our_stake,
                                        bls_signature: None,
                                        timestamp_ms: now_ms,
                                        sender_url: std::env::var("PUBLIC_URL").ok(),
                                    };
                                    
                                    #[cfg(feature = "p2p")]
                                    self.broadcast_via_p2p(&ack).await;
                                    
                                    debug!(
                                        "Sent FastPathAck for tx {} (stake: {:.2})",
                                        &tx.hash[..16.min(tx.hash.len())],
                                        our_stake
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to add fast-path tx {}: {}", &tx.hash[..16.min(tx.hash.len())], e);
                        }
                    }
                }

                Ok(None)
            }
            
            GossipMessage::FastPathAck {
                tx_hash,
                validator_address,
                validator_stake,
                bls_signature,
                timestamp_ms,
                ..
            } => {
                let validators = self.state.get_validators_map().await;
                let valid_ack = if let Some(validator) = validators.get(&validator_address) {
                    if (validator.stake - validator_stake).abs() > 0.01 {
                        warn!(
                            "FastPathAck stake mismatch for {}: claimed {:.2} vs known {:.2}",
                            &validator_address[..16.min(validator_address.len())],
                            validator_stake,
                            validator.stake
                        );
                        false
                    } else {
                        true
                    }
                } else {
                    warn!(
                        "FastPathAck from unknown validator: {}",
                        &validator_address[..16.min(validator_address.len())]
                    );
                    false
                };

                if valid_ack {
                    let total_stake: f64 = validators.values().map(|v| v.stake).sum();
                    let quorum_threshold = total_stake * 2.0 / 3.0;
                    
                    let mut confirmed_finality: Option<rinku_core::types::FastPathFinality> = None;
                    
                    {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let mut inner = self.inner.write().await;
                        let finality = inner.fast_path_pending.entry(tx_hash.clone()).or_insert_with(|| {
                            rinku_core::types::FastPathFinality {
                                tx_hash: tx_hash.clone(),
                                status: rinku_core::types::FastPathStatus::Pending,
                                acks: Vec::new(),
                                total_stake_acked: 0.0,
                                quorum_stake_required: quorum_threshold,
                                registered_at_ms: now_ms,
                                confirmed_at_ms: None,
                                checkpoint_height: None,
                            }
                        });
                        
                        let already_acked = finality.acks.iter().any(|a| a.validator_address == validator_address);
                        if !already_acked {
                            finality.acks.push(rinku_core::types::FastPathAck {
                                tx_hash: tx_hash.clone(),
                                validator_address: validator_address.clone(),
                                validator_stake,
                                bls_signature: bls_signature.clone(),
                                timestamp_ms,
                            });
                            finality.total_stake_acked += validator_stake;
                            
                            debug!(
                                "FastPathAck for {}: {:.2}/{:.2} stake ({}% quorum)",
                                &tx_hash[..16.min(tx_hash.len())],
                                finality.total_stake_acked,
                                quorum_threshold,
                                (finality.total_stake_acked / quorum_threshold * 100.0) as u32
                            );
                            
                            if finality.total_stake_acked >= quorum_threshold && finality.status == rinku_core::types::FastPathStatus::Pending {
                                finality.status = rinku_core::types::FastPathStatus::Confirmed;
                                finality.confirmed_at_ms = Some(std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64);
                                
                                confirmed_finality = Some(finality.clone());
                            }
                        }
                    }
                    
                    if let Some(finalized) = confirmed_finality {
                        let stake_acked = finalized.total_stake_acked;
                        let mut inner = self.inner.write().await;
                        inner.fast_path_confirmed.insert(tx_hash.clone(), finalized);
                        inner.fast_path_pending.remove(&tx_hash);
                        
                        info!(
                            "FastPath CONFIRMED for {} with {:.2} stake (threshold: {:.2})",
                            &tx_hash[..16.min(tx_hash.len())],
                            stake_acked,
                            quorum_threshold
                        );
                        drop(inner);
                        self.execute_on_fast_path_by_hash(&tx_hash).await;
                    }
                }
                
                Ok(None)
            }
        }
    }

    pub async fn broadcast_transaction(&self, tx: SignedTransaction) {
        let is_new = {
            let mut inner = self.inner.write().await;
            if !inner.known_txs.contains(&tx.hash) {
                inner.known_txs.insert(tx.hash.clone());
                // Limit pending_txs to prevent OOM under high load
                if inner.pending_txs.len() < PENDING_TXS_MAX {
                    inner.pending_txs.push(tx.clone());
                }
                inner.stats.txs_propagated += 1;
                true
            } else {
                false
            }
        };
        
        // Immediately broadcast via P2P for fast propagation
        if is_new {
            let tx_hash = tx.hash.clone();
            let public_url = std::env::var("PUBLIC_URL").ok();
            let message = GossipMessage::Transaction {
                hash: tx_hash.clone(),
                tx,
                sender_url: public_url,
            };
            #[cfg(feature = "p2p")]
            self.broadcast_via_p2p(&message).await;
            debug!("Broadcast tx {} via P2P immediately", &tx_hash[..16.min(tx_hash.len())]);
        }
    }

    pub async fn get_fast_path_status(&self, tx_hash: &str) -> Option<rinku_core::types::FastPathFinality> {
        let inner = self.inner.read().await;
        if let Some(finality) = inner.fast_path_confirmed.get(tx_hash) {
            return Some(finality.clone());
        }
        if let Some(finality) = inner.fast_path_pending.get(tx_hash) {
            return Some(finality.clone());
        }
        None
    }

    pub async fn is_fast_path_executed(&self, tx_hash: &str) -> bool {
        let inner = self.inner.read().await;
        inner.fast_path_executed.contains(tx_hash)
    }

    pub async fn get_all_fast_path_executed(&self) -> std::collections::HashSet<String> {
        let inner = self.inner.read().await;
        inner.fast_path_executed.clone()
    }

    async fn execute_on_fast_path(&self, tx: &rinku_core::types::SignedTransaction) {
        {
            let inner = self.inner.read().await;
            if inner.fast_path_executed.contains(&tx.hash) {
                return;
            }
        }

        let executed = self.state.execute_fast_path_transaction(tx).await;

        if executed {
            let mut inner = self.inner.write().await;
            inner.fast_path_executed.insert(tx.hash.clone());
            if let Some(finality) = inner.fast_path_confirmed.get_mut(&tx.hash) {
                finality.status = rinku_core::types::FastPathStatus::Executed;
            }
        }
    }

    async fn execute_on_fast_path_by_hash(&self, tx_hash: &str) {
        {
            let inner = self.inner.read().await;
            if inner.fast_path_executed.contains(tx_hash) {
                return;
            }
        }

        if let Some(tx) = self.state.get_transaction(tx_hash).await {
            self.execute_on_fast_path(&tx).await;
        } else {
            tracing::warn!(
                "FastPath execution skipped: tx {} not found in DAG",
                &tx_hash[..16.min(tx_hash.len())]
            );
        }
    }

    pub async fn broadcast_fast_path_transaction(&self, tx: SignedTransaction, validator_address: &str, validator_stake: f64) -> bool {
        if !tx.is_fast_path_eligible() {
            warn!("Transaction {} not eligible for fast-path", &tx.hash[..16.min(tx.hash.len())]);
            return false;
        }

        let is_new = {
            let mut inner = self.inner.write().await;
            if !inner.known_txs.contains(&tx.hash) {
                inner.known_txs.insert(tx.hash.clone());
                inner.stats.txs_propagated += 1;
                true
            } else {
                false
            }
        };

        if is_new {
            let tx_hash = tx.hash.clone();
            let public_url = std::env::var("PUBLIC_URL").ok();
            let timestamp_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            let message = GossipMessage::FastPathBroadcast {
                tx,
                sender_validator: validator_address.to_string(),
                sender_stake: validator_stake,
                timestamp_ms,
                sender_url: public_url,
            };

            #[cfg(feature = "p2p")]
            self.broadcast_via_p2p(&message).await;
            
            info!(
                "Broadcast fast-path tx {} via P2P (validator: {}, stake: {:.2})",
                &tx_hash[..16.min(tx_hash.len())],
                &validator_address[..16.min(validator_address.len())],
                validator_stake
            );
            
            true
        } else {
            false
        }
    }

    /// Flush all local transactions to a specific peer before snapshot sync
    /// This prevents locally-created transactions from being lost when the DAG is replaced
    async fn flush_pending_txs_to_peer(&self, peer: &str) {
        // Get all transactions from the local DAG (not just pending_txs queue)
        let local_txs = self.state.get_recent_transactions(1000).await;
        
        if local_txs.is_empty() {
            return;
        }
        
        info!(
            "PRE-SYNC FLUSH: Broadcasting {} local transactions to {} before snapshot sync",
            local_txs.len(),
            peer
        );
        
        let public_url = std::env::var("PUBLIC_URL").ok();
        let mut success_count = 0;
        let mut fail_count = 0;
        
        for tx in local_txs {
            // Skip genesis transactions
            if tx.hash == "genesis" || tx.tx.from == "genesis" {
                continue;
            }
            
            let message = GossipMessage::Transaction {
                hash: tx.hash.clone(),
                tx: tx.clone(),
                sender_url: public_url.clone(),
            };
            
            match self.send_to_peer(peer, &message).await {
                Ok(_) => success_count += 1,
                Err(e) => {
                    debug!("Failed to flush tx {} to peer: {}", &tx.hash[..16.min(tx.hash.len())], e);
                    fail_count += 1;
                }
            }
        }
        
        if success_count > 0 || fail_count > 0 {
            info!(
                "PRE-SYNC FLUSH: Sent {} transactions to peer ({} failed)",
                success_count,
                fail_count
            );
        }
    }

    /// Trigger sync from a specific peer (used by TipAnnouncement background task)
    /// DEPRECATED: Use trigger_sync_from_peer_p2p for 100% P2P operation
    #[allow(dead_code)]
    async fn trigger_sync_from_peer(&self, peer_url: &str) -> anyhow::Result<()> {
        // P2P-only sync: require valid libp2p peer ID
        // HTTP sync has been deprecated and removed
        
        // Check if peer_url looks like a P2P peer ID (starts with "12D3" for libp2p)
        if peer_url.starts_with("12D3") {
            if self.network_handle.is_some() {
                return self.trigger_sync_from_peer_p2p(peer_url).await;
            } else {
                warn!("P2P network not available for sync with {}", peer_url);
                return Err(anyhow::anyhow!("P2P network not initialized"));
            }
        }
        
        // Reject HTTP URLs - P2P-only mode enforced
        warn!("HTTP sync rejected for {} - P2P-only mode enabled. Use libp2p peer IDs (12D3...)", peer_url);
        Err(anyhow::anyhow!("HTTP sync is deprecated. Only P2P sync (libp2p peer IDs starting with 12D3) is supported"))
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
        let http_peers = self
            .inner
            .read()
            .await
            .peers
            .values()
            .filter(|p| p.is_healthy)
            .count();
        
        // Include P2P peer count if available
        #[cfg(feature = "p2p")]
        {
            if let Some(ref handle) = self.network_handle {
                let p2p_peers = handle.lock().await.get_peer_count().await;
                return http_peers + p2p_peers;
            }
        }
        
        http_peers
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

    pub async fn get_http_peers(&self) -> Vec<PeerInfo> {
        self.inner.read().await.peers.values().cloned().collect()
    }

    pub async fn get_p2p_peers(&self) -> Vec<crate::network::PeerStats> {
        #[cfg(feature = "p2p")]
        {
            if let Some(ref handle) = self.network_handle {
                return handle.lock().await.get_connected_peers().await;
            }
        }
        Vec::new()
    }

    pub async fn get_fast_path_stats(&self) -> crate::fast_path::FastPathStats {
        let inner = self.inner.read().await;
        
        // Calculate average confirmation time from confirmed transactions
        let mut total_ms: u64 = 0;
        let mut count: usize = 0;
        for finality in inner.fast_path_confirmed.values() {
            if let Some(time_ms) = finality.finality_time_ms() {
                total_ms += time_ms;
                count += 1;
            }
        }
        let avg_confirmation_ms = if count > 0 {
            Some(total_ms / count as u64)
        } else {
            None
        };
        
        // Get total validator stake from state
        let rewards = self.state.rewards.read().await;
        let total_validator_stake = rewards.get_total_staked();
        drop(rewards);
        
        crate::fast_path::FastPathStats {
            enabled: true,
            pending_count: inner.fast_path_pending.len(),
            confirmed_count: inner.fast_path_confirmed.len(),
            total_validator_stake,
            quorum_threshold: 0.667,
            avg_confirmation_ms,
        }
    }

    pub async fn add_peer(&self, address: String) {
        // Skip adding self as a peer
        if let Ok(own_url) = std::env::var("PUBLIC_URL") {
            let own_normalized = own_url.trim_end_matches('/');
            let addr_normalized = address.trim_end_matches('/');
            if own_normalized == addr_normalized {
                debug!("Skipping self as peer: {}", address);
                return;
            }
        }
        
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut inner = self.inner.write().await;
        if !inner.peers.contains_key(&address) {
            info!("PEER DISCOVERY: Adding new peer {} (total peers: {})", address, inner.peers.len() + 1);
            inner.peers.insert(address.clone(), PeerInfo {
                address,
                node_id: String::new(),
                last_seen: now,
                dag_size: 0,
                checkpoint_height: 0,
                latency_ms: 0,
                is_healthy: true,
                consecutive_failures: 0,
                backoff_until: 0,
            });
            inner.stats.peers_discovered += 1;
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
            GossipMessage::BloomAnnouncement { sender_url, .. } => sender_url.clone(),
            GossipMessage::SlashingEvidence { sender_url, .. } => sender_url.clone(),
            GossipMessage::WeightVote { sender_url, .. } => sender_url.clone(),
            GossipMessage::CheckpointAnnouncement { sender_url, .. } => sender_url.clone(),
            GossipMessage::FastPathBroadcast { sender_url, .. } => sender_url.clone(),
            GossipMessage::FastPathAck { sender_url, .. } => sender_url.clone(),
        }
    }
    /// Generate a bloom filter containing all known transaction hashes
    pub async fn generate_bloom_filter(&self) -> BloomFilter {
        let inner = self.inner.read().await;
        let mut filter = BloomFilter::new();
        
        for tx_hash in inner.known_txs.set.iter() {
            filter.insert(tx_hash);
        }
        
        filter
    }

    /// Check if a peer likely has a transaction (using their bloom filter)
    pub async fn peer_likely_has_tx(&self, peer_url: &str, tx_hash: &str) -> bool {
        let inner = self.inner.read().await;
        if let Some(filter) = inner.peer_bloom_filters.get(peer_url) {
            filter.might_contain(tx_hash)
        } else {
            false
        }
    }

    /// Broadcast bloom filter announcement to peers
    pub async fn broadcast_bloom_filter(&self) {
        let filter = self.generate_bloom_filter().await;
        let checkpoint_height = self.state.get_checkpoint_height().await;
        let (_, tip_count, _) = self.state.get_dag_stats().await;
        
        let message = GossipMessage::BloomAnnouncement {
            filter,
            checkpoint_height,
            tip_count,
            sender_url: std::env::var("PUBLIC_URL").ok(),
        };
        
        #[cfg(feature = "p2p")]
        self.broadcast_via_p2p(&message).await;
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

    #[test]
    fn test_bloom_filter_basic_operations() {
        let mut filter = BloomFilter::new();
        
        filter.insert("tx1");
        filter.insert("tx2");
        filter.insert("tx3");
        
        assert!(filter.might_contain("tx1"));
        assert!(filter.might_contain("tx2"));
        assert!(filter.might_contain("tx3"));
        assert!(!filter.might_contain("tx4"));
        
        assert_eq!(filter.item_count(), 3);
    }

    #[test]
    fn test_bloom_filter_no_false_negatives() {
        let mut filter = BloomFilter::new();
        let items: Vec<String> = (0..1000).map(|i| format!("tx_{}", i)).collect();
        
        for item in &items {
            filter.insert(item);
        }
        
        for item in &items {
            assert!(filter.might_contain(item), "Bloom filter should never have false negatives");
        }
    }

    #[test]
    fn test_bloom_filter_compact() {
        let mut filter = BloomFilter::compact(100);
        
        for i in 0..100 {
            filter.insert(&format!("tx_{}", i));
        }
        
        assert!(filter.size_bytes() < 2000, "Compact filter should use minimal memory");
        
        for i in 0..100 {
            assert!(filter.might_contain(&format!("tx_{}", i)));
        }
    }

    #[test]
    fn test_bloom_filter_false_positive_rate() {
        let mut filter = BloomFilter::new();
        
        for i in 0..10000 {
            filter.insert(&format!("tx_{}", i));
        }
        
        let fpr = filter.false_positive_rate();
        assert!(fpr < 0.05, "False positive rate should be under 5% for 10K items in default filter");
    }

    #[test]
    fn test_bloom_filter_clear() {
        let mut filter = BloomFilter::new();
        
        filter.insert("tx1");
        filter.insert("tx2");
        assert!(filter.might_contain("tx1"));
        assert_eq!(filter.item_count(), 2);
        
        filter.clear();
        
        assert!(!filter.might_contain("tx1"));
        assert!(!filter.might_contain("tx2"));
        assert_eq!(filter.item_count(), 0);
    }

    #[test]
    fn test_bloom_filter_serialization() {
        let mut filter = BloomFilter::new();
        filter.insert("tx1");
        filter.insert("tx2");
        
        let serialized = serde_json::to_string(&filter).expect("Should serialize");
        let deserialized: BloomFilter = serde_json::from_str(&serialized).expect("Should deserialize");
        
        assert!(deserialized.might_contain("tx1"));
        assert!(deserialized.might_contain("tx2"));
        assert!(!deserialized.might_contain("tx3"));
        assert_eq!(deserialized.item_count(), 2);
    }
}
