use crate::state::TransactionResult;
use anyhow::Result;
use rinku_core::crypto::sha256_hex;
use rinku_core::types::{
    from_micro_units, to_micro_units, Account, Checkpoint, SignedTransaction, Validator,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};

const KNOWN_TXS_MAX_SIZE: usize = 50_000;
const SEEN_CONFLICTS_MAX_SIZE: usize = 10_000;
const SEEN_EVIDENCE_MAX_SIZE: usize = 5_000;

const CHECKPOINT_BUFFER_MAX_AHEAD: u64 = 50;
const CONVERGENCE_BATCH_CHANNEL_SIZE: usize = 4096;

pub struct BufferedCheckpoint {
    pub checkpoint: Checkpoint,
    pub finalized_tx_hashes: Vec<String>,
    pub finalized_transactions: Vec<SignedTransaction>,
    pub precomputed_proofs: Vec<rinku_core::types::AccountStateProof>,
    pub source: String,
}

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

    pub fn remove(&mut self, value: &str) -> bool {
        if self.set.remove(value) {
            self.order.retain(|v| v != value);
            true
        } else {
            false
        }
    }

    pub fn remove_batch(&mut self, values: &std::collections::HashSet<&String>) {
        for v in values {
            self.set.remove(v.as_str());
        }
        self.order.retain(|v| !values.contains(v));
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
use crate::network::{IncomingSyncRequest, IncomingVoteRequest, NetworkCommand, NetworkHandle};
use crate::slashing::DoubleSignEvidence;
use crate::state::NodeState;
use crate::trust::TrustVerifier;
use crate::validator_identity::GENESIS_VALIDATOR_STAKE;

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
    faucet_balance: u64,
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
        weight: u64,
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
        weight_1: u64,
        weight_2: u64,
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
        /// Empty when DA layer is active (bodies delivered via DaChunk messages)
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        finalized_transactions: Vec<SignedTransaction>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        da_commitment: Option<crate::erasure::DaCommitment>,
    },
    /// Fast-path broadcast for data-only transactions (Mysticeti-FPC style)
    /// These transactions only touch sender's account (gas) and can achieve
    /// sub-second finality via reliable broadcast quorum
    TxConfirmBroadcast {
        tx: SignedTransaction,
        sender_validator: String,
        sender_stake: u64,
        timestamp_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    TxConfirmAck {
        tx_hash: String,
        validator_address: String,
        validator_stake: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bls_signature: Option<String>,
        timestamp_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    MergePayload {
        request: crate::merge::MergeRequest,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    MergeResult {
        report: crate::merge::MergeReport,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    CheckpointIntent {
        height: u64,
        leader_address: String,
        timestamp_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
        #[serde(default)]
        relay_count: u8,
    },
    LeaderTimeout {
        height: u64,
        validator_address: String,
        validator_stake: u64,
        timestamp_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    DaChunk {
        checkpoint_height: u64,
        checkpoint_hash: String,
        shard_index: usize,
        shard_data: String,
        merkle_proof: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    QccVoteRequest {
        height: u64,
        checkpoint_hash: String,
        tx_merkle_root: String,
        state_root: String,
        proposer_address: String,
        finalized_tx_hashes: Vec<String>,
        timestamp_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    QccVoteCast {
        height: u64,
        checkpoint_hash: String,
        validator_address: String,
        bls_public_key: String,
        signature: String,
        stake: u64,
        timestamp_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_url: Option<String>,
    },
    Batch {
        messages: Vec<GossipMessage>,
    },
}

impl GossipMessage {
    pub fn variant_name(&self) -> &'static str {
        match self {
            GossipMessage::Transaction { .. } => "Transaction",
            GossipMessage::TipAnnouncement { .. } => "TipAnnouncement",
            GossipMessage::CheckpointSignature { .. } => "CheckpointSignature",
            GossipMessage::PeerDiscovery { .. } => "PeerDiscovery",
            GossipMessage::ConflictResolution { .. } => "ConflictResolution",
            GossipMessage::SyncRequest { .. } => "SyncRequest",
            GossipMessage::SyncResponse { .. } => "SyncResponse",
            GossipMessage::BloomAnnouncement { .. } => "BloomAnnouncement",
            GossipMessage::SlashingEvidence { .. } => "SlashingEvidence",
            GossipMessage::WeightVote { .. } => "WeightVote",
            GossipMessage::CheckpointAnnouncement { .. } => "CheckpointAnnouncement",
            GossipMessage::TxConfirmBroadcast { .. } => "TxConfirmBroadcast",
            GossipMessage::TxConfirmAck { .. } => "TxConfirmAck",
            GossipMessage::MergePayload { .. } => "MergePayload",
            GossipMessage::MergeResult { .. } => "MergeResult",
            GossipMessage::CheckpointIntent { .. } => "CheckpointIntent",
            GossipMessage::LeaderTimeout { .. } => "LeaderTimeout",
            GossipMessage::DaChunk { .. } => "DaChunk",
            GossipMessage::QccVoteRequest { .. } => "QccVoteRequest",
            GossipMessage::QccVoteCast { .. } => "QccVoteCast",
            GossipMessage::Batch { .. } => "Batch",
        }
    }
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
    pub stale_nonce_cache_hits: std::sync::atomic::AtomicU64,
    /// Flag to prevent overlapping propagation tasks
    pub propagation_in_flight: bool,
    /// Fast-path finality tracking for data-only transactions
    pub convergence_pending: HashMap<String, rinku_core::types::FastPathFinality>,
    pub convergence_confirmed: HashMap<String, rinku_core::types::FastPathFinality>,
    pub convergence_executed: std::collections::HashSet<String>,
    pub convergence_tx_bodies: HashMap<String, SignedTransaction>,
    pub leader_intents: HashMap<u64, (String, u64)>,
    pub recent_checkpoint_data: HashMap<u64, CachedCheckpointData>,
    pub last_tip_hash: u64,
    pub confirmed_pending_execution: Vec<SignedTransaction>,
    pub last_batch_execution: std::time::Instant,
    pub da_collector: HashMap<u64, DaReconstructionState>,
    pub da_early_shards: HashMap<u64, Vec<(String, usize, String, Vec<String>)>>,
    pub leader_timeout_votes: HashMap<u64, HashMap<String, u64>>,
    pub leader_timeout_sent: std::collections::HashSet<u64>,
}

pub struct DaReconstructionState {
    pub checkpoint: Checkpoint,
    pub finalized_tx_hashes: Vec<String>,
    pub precomputed_proofs: Vec<rinku_core::types::AccountStateProof>,
    pub commitment: crate::erasure::DaCommitment,
    pub shards: Vec<Option<crate::erasure::DaShard>>,
    pub received_count: usize,
    pub created_at: std::time::Instant,
    pub source: String,
    pub decode_attempts: u8,
}

pub struct CachedCheckpointData {
    pub checkpoint: Checkpoint,
    pub finalized_tx_hashes: Vec<String>,
    pub finalized_transactions: Vec<SignedTransaction>,
    pub precomputed_proofs: Vec<rinku_core::types::AccountStateProof>,
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

#[derive(Debug, Clone)]
pub struct QccGossipVote {
    pub validator_address: String,
    pub bls_public_key: String,
    pub signature: String,
    pub stake: u64,
    pub height: u64,
    pub checkpoint_hash: String,
}

struct ValidatorCache {
    validators: HashMap<String, Validator>,
    our_address: Option<String>,
    our_stake: u64,
    total_stake: u64,
    quorum_threshold: u64,
    cached_at: std::time::Instant,
}

const VALIDATOR_CACHE_TTL_MS: u128 = 5000;

enum ConvergenceVoteOp {
    BroadcastVote {
        tx: SignedTransaction,
        sender_validator: String,
        sender_stake: u64,
        timestamp_ms: u64,
        send_ack: bool,
    },
    AckVote {
        tx_hash: String,
        validator_address: String,
        validator_stake: u64,
        timestamp_ms: u64,
    },
}

pub struct GossipService {
    state: NodeState,
    inner: Arc<RwLock<GossipServiceInner>>,
    node_id: String,
    interval_ms: u64,
    trust_verifier: Arc<TrustVerifier>,
    sync_verify_strict: bool,
    checkpoint_vote_signer: Option<CheckpointVoteSigner>,
    #[cfg(feature = "p2p")]
    network_handle: Option<Arc<NetworkHandle>>,
    #[cfg(feature = "p2p")]
    p2p_message_rx:
        Option<Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<GossipMessage>>>>>,
    #[cfg(feature = "p2p")]
    p2p_priority_rx:
        Option<Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<GossipMessage>>>>>,
    #[cfg(feature = "p2p")]
    p2p_checkpoint_rx:
        Option<Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<GossipMessage>>>>>,
    #[cfg(feature = "p2p")]
    p2p_sync_rx:
        Option<Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<IncomingSyncRequest>>>>>,
    #[cfg(feature = "p2p")]
    p2p_vote_rx:
        Option<Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Receiver<IncomingVoteRequest>>>>>,
    #[cfg(feature = "p2p")]
    p2p_response_tx: Option<tokio::sync::mpsc::Sender<NetworkCommand>>,
    convergence_vote_tx: Arc<std::sync::OnceLock<tokio::sync::mpsc::Sender<ConvergenceVoteOp>>>,
    convergence_exec_semaphore: Arc<tokio::sync::Semaphore>,
    validator_identity:
        Option<Arc<tokio::sync::RwLock<crate::validator_identity::ValidatorIdentityService>>>,
    event_bus: Option<Arc<crate::events::EventBus>>,
    pub checkpoint_buffer: Arc<tokio::sync::Mutex<HashMap<u64, BufferedCheckpoint>>>,
    validator_cache: Arc<tokio::sync::RwLock<Option<ValidatorCache>>>,
    qcc_vote_tx: Arc<tokio::sync::Mutex<Option<tokio::sync::mpsc::Sender<QccGossipVote>>>>,
    highest_seen_peer_height: Arc<std::sync::atomic::AtomicU64>,
}

impl Clone for GossipService {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            inner: self.inner.clone(),
            node_id: self.node_id.clone(),
            interval_ms: self.interval_ms,
            trust_verifier: self.trust_verifier.clone(),
            sync_verify_strict: self.sync_verify_strict,
            checkpoint_vote_signer: self.checkpoint_vote_signer.clone(),
            #[cfg(feature = "p2p")]
            network_handle: self.network_handle.clone(),
            #[cfg(feature = "p2p")]
            p2p_message_rx: None,
            #[cfg(feature = "p2p")]
            p2p_priority_rx: None,
            #[cfg(feature = "p2p")]
            p2p_checkpoint_rx: None,
            #[cfg(feature = "p2p")]
            p2p_sync_rx: None,
            #[cfg(feature = "p2p")]
            p2p_vote_rx: None,
            #[cfg(feature = "p2p")]
            p2p_response_tx: self.p2p_response_tx.clone(),
            convergence_vote_tx: Arc::clone(&self.convergence_vote_tx),
            convergence_exec_semaphore: Arc::clone(&self.convergence_exec_semaphore),
            validator_identity: self.validator_identity.clone(),
            event_bus: self.event_bus.clone(),
            checkpoint_buffer: self.checkpoint_buffer.clone(),
            validator_cache: self.validator_cache.clone(),
            qcc_vote_tx: self.qcc_vote_tx.clone(),
            highest_seen_peer_height: self.highest_seen_peer_height.clone(),
        }
    }
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
            peers.insert(
                addr.clone(),
                PeerInfo {
                    address: addr,
                    node_id: String::new(),
                    last_seen: now,
                    dag_size: 0,
                    checkpoint_height: 0,
                    latency_ms: 0,
                    is_healthy: true,
                    consecutive_failures: 0,
                    backoff_until: 0,
                },
            );
        }

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
                stale_nonce_cache_hits: std::sync::atomic::AtomicU64::new(0),
                propagation_in_flight: false,
                convergence_pending: HashMap::new(),
                convergence_confirmed: HashMap::new(),
                convergence_executed: std::collections::HashSet::new(),
                convergence_tx_bodies: HashMap::new(),
                leader_intents: HashMap::new(),
                recent_checkpoint_data: HashMap::new(),
                last_tip_hash: 0,
                confirmed_pending_execution: Vec::new(),
                last_batch_execution: std::time::Instant::now(),
                da_collector: HashMap::new(),
                da_early_shards: HashMap::new(),
                leader_timeout_votes: HashMap::new(),
                leader_timeout_sent: std::collections::HashSet::new(),
            })),
            node_id,
            interval_ms,
            trust_verifier: Arc::new(TrustVerifier::new(trust_config)),
            sync_verify_strict,
            checkpoint_vote_signer: None,
            #[cfg(feature = "p2p")]
            network_handle: None,
            #[cfg(feature = "p2p")]
            p2p_message_rx: None,
            #[cfg(feature = "p2p")]
            p2p_priority_rx: None,
            #[cfg(feature = "p2p")]
            p2p_checkpoint_rx: None,
            #[cfg(feature = "p2p")]
            p2p_sync_rx: None,
            #[cfg(feature = "p2p")]
            p2p_vote_rx: None,
            #[cfg(feature = "p2p")]
            p2p_response_tx: None,
            convergence_vote_tx: Arc::new(std::sync::OnceLock::new()),
            convergence_exec_semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
            validator_identity: None,
            event_bus: None,
            checkpoint_buffer: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            validator_cache: Arc::new(tokio::sync::RwLock::new(None)),
            qcc_vote_tx: Arc::new(tokio::sync::Mutex::new(None)),
            highest_seen_peer_height: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    async fn get_cached_validators(
        &self,
    ) -> (HashMap<String, Validator>, Option<String>, u64, u64, u64) {
        {
            let cache = self.validator_cache.read().await;
            if let Some(ref c) = *cache {
                if c.cached_at.elapsed().as_millis() < VALIDATOR_CACHE_TTL_MS {
                    return (
                        c.validators.clone(),
                        c.our_address.clone(),
                        c.our_stake,
                        c.total_stake,
                        c.quorum_threshold,
                    );
                }
            }
        }
        let validators = self.state.get_validators_map().await;
        let (our_addr, _) = self.state.get_validator_info().await;
        let our_stake = if let Some(ref addr) = our_addr {
            self.state.get_validator_stake(addr).await.unwrap_or(0)
        } else {
            0
        };
        let total_stake: u64 = validators.values().map(|v| v.stake).sum();
        let quorum_threshold = total_stake * 2 / 3;
        let mut cache = self.validator_cache.write().await;
        *cache = Some(ValidatorCache {
            validators: validators.clone(),
            our_address: our_addr.clone(),
            our_stake,
            total_stake,
            quorum_threshold,
            cached_at: std::time::Instant::now(),
        });
        (
            validators,
            our_addr,
            our_stake,
            total_stake,
            quorum_threshold,
        )
    }

    #[cfg(feature = "p2p")]
    pub fn set_p2p_vote_channel(
        &mut self,
        vote_rx: Option<tokio::sync::mpsc::Receiver<IncomingVoteRequest>>,
    ) {
        self.p2p_vote_rx = vote_rx.map(|rx| Arc::new(tokio::sync::Mutex::new(Some(rx))));
    }

    pub fn set_event_bus(&mut self, event_bus: Arc<crate::events::EventBus>) {
        self.event_bus = Some(event_bus);
    }

    /// Set the validator identity service for syncing validator registry
    pub fn set_validator_identity(
        &mut self,
        validator_identity: Arc<
            tokio::sync::RwLock<crate::validator_identity::ValidatorIdentityService>,
        >,
    ) {
        self.validator_identity = Some(validator_identity);
    }

    pub fn set_checkpoint_vote_signer(&mut self, signer: CheckpointVoteSigner) {
        self.checkpoint_vote_signer = Some(signer);
    }

    #[cfg(feature = "p2p")]
    pub fn set_network_handle(&mut self, handle: Arc<NetworkHandle>) {
        self.network_handle = Some(handle);
    }

    #[cfg(feature = "p2p")]
    pub fn set_p2p_channels(
        &mut self,
        message_rx: Option<tokio::sync::mpsc::Receiver<GossipMessage>>,
        priority_rx: Option<tokio::sync::mpsc::Receiver<GossipMessage>>,
        checkpoint_rx: Option<tokio::sync::mpsc::Receiver<GossipMessage>>,
        sync_rx: Option<tokio::sync::mpsc::Receiver<IncomingSyncRequest>>,
        response_tx: Option<tokio::sync::mpsc::Sender<NetworkCommand>>,
    ) {
        self.p2p_message_rx = message_rx.map(|rx| Arc::new(tokio::sync::Mutex::new(Some(rx))));
        self.p2p_priority_rx = priority_rx.map(|rx| Arc::new(tokio::sync::Mutex::new(Some(rx))));
        self.p2p_checkpoint_rx =
            checkpoint_rx.map(|rx| Arc::new(tokio::sync::Mutex::new(Some(rx))));
        self.p2p_sync_rx = sync_rx.map(|rx| Arc::new(tokio::sync::Mutex::new(Some(rx))));
        self.p2p_response_tx = response_tx;
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
        let has_p2p = self.network_handle.is_some();
        info!(
            "Gossip service started (interval: {}ms, peers: {}, p2p: {})",
            self.interval_ms, peer_count, has_p2p
        );

        if peer_count > 0 {
            self.initial_sync().await;
        }

        #[cfg(feature = "p2p")]
        {
            let (vote_tx, vote_rx) =
                tokio::sync::mpsc::channel::<ConvergenceVoteOp>(CONVERGENCE_BATCH_CHANNEL_SIZE);
            let _ = self.convergence_vote_tx.set(vote_tx);
            {
                let gossip_batch = Arc::clone(&self);
                tokio::spawn(async move {
                    gossip_batch.run_convergence_batch_processor(vote_rx).await;
                });
            }

            let priority_rx_opt = if let Some(ref rx_slot) = self.p2p_priority_rx {
                rx_slot.lock().await.take()
            } else {
                None
            };

            if let Some(ref rx_slot) = self.p2p_message_rx {
                if let Some(rx) = rx_slot.lock().await.take() {
                    let gossip_clone = Arc::clone(&self);
                    tokio::spawn(async move {
                        gossip_clone.run_p2p_receiver(rx, priority_rx_opt).await;
                    });
                }
            }

            if let Some(ref rx_slot) = self.p2p_checkpoint_rx {
                if let Some(rx) = rx_slot.lock().await.take() {
                    let gossip_clone_cp = Arc::clone(&self);
                    tokio::spawn(async move {
                        gossip_clone_cp.run_checkpoint_receiver(rx).await;
                    });
                }
            }

            if let Some(ref rx_slot) = self.p2p_sync_rx {
                let response_tx = self.p2p_response_tx.clone();
                if let Some(rx) = rx_slot.lock().await.take() {
                    let gossip_clone2 = Arc::clone(&self);
                    tokio::spawn(async move {
                        gossip_clone2
                            .run_sync_request_handler(rx, response_tx)
                            .await;
                    });
                }
            }

            if let Some(ref rx_slot) = self.p2p_vote_rx {
                let response_tx = self.p2p_response_tx.clone();
                if let Some(rx) = rx_slot.lock().await.take() {
                    let gossip_clone3 = Arc::clone(&self);
                    tokio::spawn(async move {
                        gossip_clone3
                            .run_vote_request_handler(rx, response_tx)
                            .await;
                    });
                }
            }
        }

        let mut tick = interval(Duration::from_millis(self.interval_ms));
        let gossip_arc = Arc::clone(&self);

        loop {
            tick.tick().await;
            self.gossip_round(&gossip_arc).await;
        }
    }

    #[cfg(feature = "p2p")]
    async fn run_p2p_receiver(
        self: Arc<Self>,
        mut rx: tokio::sync::mpsc::Receiver<GossipMessage>,
        priority_rx: Option<tokio::sync::mpsc::Receiver<GossipMessage>>,
    ) {
        info!("P2P message receiver started (concurrent, bounded, priority-aware)");
        let semaphore = Arc::new(tokio::sync::Semaphore::new(12));
        let priority_semaphore = Arc::new(tokio::sync::Semaphore::new(64));

        if let Some(mut pri_rx) = priority_rx {
            loop {
                for _ in 0..10 {
                    match pri_rx.try_recv() {
                        Ok(msg) => {
                            debug!("Processing PRIORITY message [dedicated]");
                            let sem = priority_semaphore.clone();
                            let handler = Arc::clone(&self);
                            tokio::spawn(async move {
                                let _permit = sem.acquire_owned().await.unwrap();
                                if let Err(e) = handler.handle_message(msg).await {
                                    warn!("Failed to handle priority P2P message: {}", e);
                                }
                            });
                        }
                        Err(_) => break,
                    }
                }

                tokio::select! {
                    biased;
                    Some(msg) = pri_rx.recv() => {
                        debug!("Processing PRIORITY message [dedicated]");
                        let sem = priority_semaphore.clone();
                        let handler = Arc::clone(&self);
                        tokio::spawn(async move {
                            let _permit = sem.acquire_owned().await.unwrap();
                            if let Err(e) = handler.handle_message(msg).await {
                                warn!("Failed to handle priority P2P message: {}", e);
                            }
                        });
                    }
                    Some(msg) = rx.recv() => {
                        let sem = semaphore.clone();
                        let handler = Arc::clone(&self);
                        tokio::spawn(async move {
                            let _permit = sem.acquire_owned().await.unwrap();
                            if let Err(e) = handler.handle_message(msg).await {
                                warn!("Failed to handle P2P message: {}", e);
                            }
                        });
                    }
                    else => break,
                }
            }
        } else {
            while let Some(msg) = rx.recv().await {
                let permit = semaphore.clone().acquire_owned().await.unwrap();
                let handler = Arc::clone(&self);
                tokio::spawn(async move {
                    if let Err(e) = handler.handle_message(msg).await {
                        warn!("Failed to handle P2P message: {}", e);
                    }
                    drop(permit);
                });
            }
        }
        warn!("P2P message channel closed");
    }

    #[cfg(feature = "p2p")]
    async fn run_checkpoint_receiver(
        self: Arc<Self>,
        mut rx: tokio::sync::mpsc::Receiver<GossipMessage>,
    ) {
        info!("Dedicated checkpoint receiver started (isolated from convergence handlers)");
        let semaphore = Arc::new(tokio::sync::Semaphore::new(4));
        while let Some(msg) = rx.recv().await {
            let sem = semaphore.clone();
            let handler = Arc::clone(&self);
            tokio::spawn(async move {
                let _permit = sem.acquire_owned().await.unwrap();
                if let Err(e) = handler.handle_message(msg).await {
                    warn!("Failed to handle checkpoint message: {}", e);
                }
            });
        }
        warn!("Checkpoint message channel closed");
    }

    #[cfg(feature = "p2p")]
    async fn run_convergence_batch_processor(
        self: Arc<Self>,
        mut rx: tokio::sync::mpsc::Receiver<ConvergenceVoteOp>,
    ) {
        info!("Convergence batch processor started");
        let mut batch = Vec::with_capacity(256);

        loop {
            match rx.recv().await {
                Some(op) => batch.push(op),
                None => break,
            }

            while batch.len() < 256 {
                match rx.try_recv() {
                    Ok(op) => batch.push(op),
                    Err(_) => break,
                }
            }

            if !batch.is_empty() {
                let batch_len = batch.len();
                Self::process_convergence_batch(&self, &mut batch).await;
                batch.clear();
                if batch_len > 1 {
                    debug!(
                        "Convergence batch: processed {} votes in single lock",
                        batch_len
                    );
                }
            }
        }
        warn!("Convergence batch processor channel closed");
    }

    async fn process_convergence_batch(self_arc: &Arc<Self>, batch: &mut Vec<ConvergenceVoteOp>) {
        let (validators, our_addr, our_stake, _total_stake, quorum_threshold) =
            self_arc.get_cached_validators().await;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        struct ConfirmedTx {
            tx_hash: String,
            finality: rinku_core::types::FastPathFinality,
            tx: Option<SignedTransaction>,
        }

        let mut newly_confirmed: Vec<ConfirmedTx> = Vec::new();
        let mut acks_to_send: Vec<(String, String, u64)> = Vec::new();
        let mut already_confirmed_txs: Vec<SignedTransaction> = Vec::new();

        {
            let mut inner = self_arc.inner.write().await;

            for op in batch.drain(..) {
                match op {
                    ConvergenceVoteOp::BroadcastVote {
                        tx,
                        sender_validator,
                        sender_stake,
                        timestamp_ms,
                        send_ack,
                    } => {
                        if inner.convergence_tx_bodies.len() < 10_000 {
                            inner.convergence_tx_bodies.entry(tx.hash.clone()).or_insert_with(|| tx.clone());
                        }

                        if inner.convergence_executed.contains(&tx.hash) {
                            continue;
                        }

                        if inner.convergence_confirmed.contains_key(&tx.hash) {
                            already_confirmed_txs.push(tx.clone());
                            if send_ack {
                                if let Some(ref addr) = our_addr {
                                    if our_stake > 0 {
                                        acks_to_send.push((
                                            tx.hash.clone(),
                                            addr.clone(),
                                            our_stake,
                                        ));
                                    }
                                }
                            }
                            continue;
                        }

                        let finality = inner
                            .convergence_pending
                            .entry(tx.hash.clone())
                            .or_insert_with(|| rinku_core::types::FastPathFinality {
                                tx_hash: tx.hash.clone(),
                                status: rinku_core::types::FastPathStatus::Pending,
                                acks: Vec::new(),
                                total_stake_acked: 0,
                                quorum_stake_required: quorum_threshold,
                                registered_at_ms: timestamp_ms,
                                confirmed_at_ms: None,
                                checkpoint_height: None,
                                tx_created_at_ms: Some(tx.tx.timestamp),
                            });
                        if finality.tx_created_at_ms.is_none() {
                            finality.tx_created_at_ms = Some(tx.tx.timestamp);
                        }

                        if let Some(canonical_validator) = validators.get(&sender_validator) {
                            let canonical_stake = canonical_validator.stake;
                            if canonical_stake > 0 {
                                if sender_stake != canonical_stake {
                                    warn!(
                                        "TxConfirmBroadcast stake mismatch for {}: claimed {} vs canonical {}",
                                        &sender_validator[..16.min(sender_validator.len())],
                                        sender_stake,
                                        canonical_stake
                                    );
                                }
                                let already_acked = finality
                                    .acks
                                    .iter()
                                    .any(|a| a.validator_address == sender_validator);
                                if !already_acked {
                                    finality.acks.push(rinku_core::types::FastPathAck {
                                        tx_hash: tx.hash.clone(),
                                        validator_address: sender_validator.clone(),
                                        validator_stake: canonical_stake,
                                        bls_signature: None,
                                        timestamp_ms,
                                    });
                                    finality.total_stake_acked += canonical_stake;
                                }
                            }
                        }

                        if let Some(ref addr) = our_addr {
                            if our_stake > 0 {
                                let self_acked =
                                    finality.acks.iter().any(|a| a.validator_address == *addr);
                                if !self_acked {
                                    finality.acks.push(rinku_core::types::FastPathAck {
                                        tx_hash: tx.hash.clone(),
                                        validator_address: addr.clone(),
                                        validator_stake: our_stake,
                                        bls_signature: None,
                                        timestamp_ms: now_ms,
                                    });
                                    finality.total_stake_acked += our_stake;
                                }
                            }
                        }

                        if finality.total_stake_acked >= quorum_threshold
                            && finality.status == rinku_core::types::FastPathStatus::Pending
                        {
                            finality.status = rinku_core::types::FastPathStatus::Confirmed;
                            finality.confirmed_at_ms = Some(now_ms);
                            let f = finality.clone();
                            inner
                                .convergence_confirmed
                                .insert(tx.hash.clone(), f.clone());
                            inner.convergence_pending.remove(&tx.hash);
                            newly_confirmed.push(ConfirmedTx {
                                tx_hash: tx.hash.clone(),
                                finality: f,
                                tx: Some(tx.clone()),
                            });
                        }

                        if send_ack {
                            if let Some(ref addr) = our_addr {
                                if our_stake > 0 {
                                    acks_to_send.push((tx.hash.clone(), addr.clone(), our_stake));
                                }
                            }
                        }
                    }
                    ConvergenceVoteOp::AckVote {
                        tx_hash,
                        validator_address,
                        validator_stake,
                        timestamp_ms,
                    } => {
                        if inner.convergence_executed.contains(&tx_hash) {
                            continue;
                        }
                        if inner.convergence_confirmed.contains_key(&tx_hash) {
                            continue;
                        }
                        if !inner.convergence_pending.contains_key(&tx_hash) {
                            if !inner.known_txs.contains(&tx_hash) {
                                continue;
                            }
                        }

                        let canonical_stake = if let Some(v) = validators.get(&validator_address) {
                            if v.stake != validator_stake {
                                warn!(
                                    "FastPathAck stake mismatch for {}: claimed {} vs canonical {}",
                                    &validator_address[..16.min(validator_address.len())],
                                    validator_stake,
                                    v.stake
                                );
                                continue;
                            }
                            v.stake
                        } else {
                            warn!(
                                "FastPathAck from unknown validator: {}",
                                &validator_address[..16.min(validator_address.len())]
                            );
                            continue;
                        };

                        let finality = inner
                            .convergence_pending
                            .entry(tx_hash.clone())
                            .or_insert_with(|| rinku_core::types::FastPathFinality {
                                tx_hash: tx_hash.clone(),
                                status: rinku_core::types::FastPathStatus::Pending,
                                acks: Vec::new(),
                                total_stake_acked: 0,
                                quorum_stake_required: quorum_threshold,
                                registered_at_ms: now_ms,
                                confirmed_at_ms: None,
                                checkpoint_height: None,
                                tx_created_at_ms: None,
                            });

                        let already_acked = finality
                            .acks
                            .iter()
                            .any(|a| a.validator_address == validator_address);
                        if !already_acked {
                            finality.acks.push(rinku_core::types::FastPathAck {
                                tx_hash: tx_hash.clone(),
                                validator_address: validator_address.clone(),
                                validator_stake: canonical_stake,
                                bls_signature: None,
                                timestamp_ms,
                            });
                            finality.total_stake_acked += canonical_stake;
                        }

                        if let Some(ref addr) = our_addr {
                            if our_stake > 0 {
                                let self_acked =
                                    finality.acks.iter().any(|a| a.validator_address == *addr);
                                if !self_acked {
                                    finality.acks.push(rinku_core::types::FastPathAck {
                                        tx_hash: tx_hash.clone(),
                                        validator_address: addr.clone(),
                                        validator_stake: our_stake,
                                        bls_signature: None,
                                        timestamp_ms: now_ms,
                                    });
                                    finality.total_stake_acked += our_stake;
                                }
                            }
                        }

                        if finality.total_stake_acked >= quorum_threshold
                            && finality.status == rinku_core::types::FastPathStatus::Pending
                        {
                            finality.status = rinku_core::types::FastPathStatus::Confirmed;
                            finality.confirmed_at_ms = Some(now_ms);
                            let f = finality.clone();
                            inner
                                .convergence_confirmed
                                .insert(tx_hash.clone(), f.clone());
                            inner.convergence_pending.remove(&tx_hash);
                            let stored_body = inner.convergence_tx_bodies.get(&tx_hash).cloned();
                            newly_confirmed.push(ConfirmedTx {
                                tx_hash: tx_hash.clone(),
                                finality: f,
                                tx: stored_body,
                            });
                        }
                    }
                }
            }
        }

        #[cfg(feature = "p2p")]
        if !acks_to_send.is_empty() {
            let mut acks_sent = 0usize;
            let mut acks_dropped = 0usize;
            let sender_url = std::env::var("PUBLIC_URL").ok();
            if let Some(ref handle) = self_arc.network_handle {
                let mut seen_tx_hashes = std::collections::HashSet::new();
                for (tx_hash, validator_addr, stake) in &acks_to_send {
                    if !seen_tx_hashes.insert(tx_hash.clone()) {
                        continue;
                    }
                    let ack = GossipMessage::TxConfirmAck {
                        tx_hash: tx_hash.clone(),
                        validator_address: validator_addr.clone(),
                        validator_stake: *stake,
                        bls_signature: None,
                        timestamp_ms: now_ms,
                        sender_url: sender_url.clone(),
                    };
                    match handle.try_broadcast(ack) {
                        Ok(_) => acks_sent += 1,
                        Err(_) => {
                            acks_dropped += acks_to_send.len() - acks_sent;
                            break;
                        }
                    }
                }
            }
            if acks_dropped > 0 {
                warn!(
                    "Convergence ack broadcast: {}/{} sent, {} dropped (outbound full)",
                    acks_sent,
                    acks_to_send.len(),
                    acks_dropped
                );
            }
        }

        if !newly_confirmed.is_empty() {
            let confirm_count = newly_confirmed.len();
            for ct in &newly_confirmed {
                self_arc
                    .state
                    .set_convergence_certificate(&ct.tx_hash, &ct.finality)
                    .await;
                info!(
                    "FastPath CONFIRMED (batch) for {} with {} stake (threshold: {})",
                    &ct.tx_hash[..16.min(ct.tx_hash.len())],
                    ct.finality.total_stake_acked,
                    quorum_threshold
                );
            }

            if let Some(ref eb) = self_arc.event_bus {
                for ct in &newly_confirmed {
                    if let Some(ref tx) = ct.tx {
                        eb.publish(crate::events::NodeEvent::FastPathConfirmed {
                            hash: ct.tx_hash.clone(),
                            from: tx.tx.from.clone(),
                            to: tx.tx.to.clone(),
                            amount: from_micro_units(tx.tx.amount),
                            total_stake: from_micro_units(ct.finality.total_stake_acked),
                            threshold: from_micro_units(quorum_threshold),
                        });
                    } else if let Some(tx) = self_arc.state.get_transaction(&ct.tx_hash).await {
                        eb.publish(crate::events::NodeEvent::FastPathConfirmed {
                            hash: ct.tx_hash.clone(),
                            from: tx.tx.from.clone(),
                            to: tx.tx.to.clone(),
                            amount: from_micro_units(tx.tx.amount),
                            total_stake: from_micro_units(ct.finality.total_stake_acked),
                            threshold: from_micro_units(quorum_threshold),
                        });
                    }
                }
            }

            {
                const MAX_PENDING_EXECUTION: usize = 5000;
                let mut inner = self_arc.inner.write().await;
                for ct in newly_confirmed {
                    if inner.confirmed_pending_execution.len() >= MAX_PENDING_EXECUTION {
                        tracing::warn!(
                            "FastPath: pending execution queue full ({}), dropping tx {}",
                            MAX_PENDING_EXECUTION,
                            &ct.tx_hash[..16.min(ct.tx_hash.len())]
                        );
                        continue;
                    }
                    if let Some(tx) = ct.tx {
                        inner.confirmed_pending_execution.push(tx);
                    } else if let Some(tx) = self_arc.state.get_transaction(&ct.tx_hash).await {
                        inner.confirmed_pending_execution.push(tx);
                    } else {
                        tracing::warn!(
                            "FastPath: tx {} confirmed but body unavailable — will be finalized at checkpoint",
                            &ct.tx_hash[..16.min(ct.tx_hash.len())]
                        );
                    }
                }
            }
            debug!("Queued {} confirmed txs for batch execution", confirm_count);
        }

        if !already_confirmed_txs.is_empty() {
            debug!(
                "FastPath: skipped re-execution of {} already-confirmed txs",
                already_confirmed_txs.len()
            );
        }
    }

    #[cfg(feature = "p2p")]
    async fn run_sync_request_handler(
        self: Arc<Self>,
        mut rx: tokio::sync::mpsc::Receiver<IncomingSyncRequest>,
        response_tx: Option<tokio::sync::mpsc::Sender<NetworkCommand>>,
    ) {
        info!("P2P sync request handler started (concurrent)");
        while let Some(incoming) = rx.recv().await {
            let handler = Arc::clone(&self);
            let resp_tx = response_tx.clone();
            tokio::spawn(async move {
                handler.process_sync_request(incoming, resp_tx).await;
            });
        }
        warn!("Sync request channel closed");
    }

    #[cfg(feature = "p2p")]
    async fn run_vote_request_handler(
        self: Arc<Self>,
        mut rx: tokio::sync::mpsc::Receiver<IncomingVoteRequest>,
        response_tx: Option<tokio::sync::mpsc::Sender<NetworkCommand>>,
    ) {
        info!("P2P vote request handler started (dedicated channel)");
        while let Some(incoming) = rx.recv().await {
            let handler = Arc::clone(&self);
            let resp_tx = response_tx.clone();
            tokio::spawn(async move {
                handler.process_vote_request(incoming, resp_tx).await;
            });
        }
        warn!("Vote request channel closed");
    }

    #[cfg(feature = "p2p")]
    async fn process_vote_request(
        &self,
        incoming: IncomingVoteRequest,
        response_tx: Option<tokio::sync::mpsc::Sender<NetworkCommand>>,
    ) {
        use crate::bls::bls_sign;
        use crate::network::{CheckpointVoteResponse, VoteRequest, VoteResponse};
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;

        let response = match incoming.request {
            VoteRequest::CheckpointVote(request) => {
                let vote_start = std::time::Instant::now();
                let vote_height = request.height;
                let local_cp_height = self.state.get_checkpoint_height();
                let vote_response = if vote_height > local_cp_height + 3 {
                    debug!(
                        "Fast-declining vote for checkpoint {} (local height {}, too far ahead)",
                        vote_height, local_cp_height
                    );
                    VoteResponse::CheckpointVote(None)
                } else if vote_height <= local_cp_height {
                    debug!(
                        "Fast-declining stale vote for checkpoint {} (local height {})",
                        vote_height, local_cp_height
                    );
                    VoteResponse::CheckpointVote(None)
                } else if let Some(ref signer) = self.checkpoint_vote_signer {
                    if signer.validator_address.is_empty() {
                        VoteResponse::CheckpointVote(None)
                    } else {
                        let tx_hashes_for_verify = if !request.finalized_tx_hashes.is_empty() {
                            let mut sorted = request.finalized_tx_hashes.clone();
                            sorted.sort();
                            Some(sorted)
                        } else {
                            None
                        };
                        let total_txs = request.finalized_tx_hashes.len();
                        let expected_merkle = request.tx_merkle_root.clone();
                        let checkpoint_hash_hex = request.checkpoint_hash.clone();
                        let bls_key = signer.bls_private_key.clone();

                        let vote_compute =
                            tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
                                if let Some(tx_hashes) = tx_hashes_for_verify {
                                    let computed_root =
                                        rinku_core::MerkleTree::from_hex_leaves(&tx_hashes)
                                            .map(|t| t.root())
                                            .unwrap_or_else(|_| "0".repeat(64));
                                    if computed_root != expected_merkle {
                                        return Err(format!(
                                            "merkle_mismatch:{}:{}",
                                            &computed_root[..16],
                                            &expected_merkle[..16]
                                        ));
                                    }
                                } else if expected_merkle != "0".repeat(64) {
                                    return Err(format!(
                                        "no_hashes_nonzero_root:{}",
                                        &expected_merkle[..16.min(expected_merkle.len())]
                                    ));
                                }

                                let hash_bytes = hex::decode(&checkpoint_hash_hex)
                                    .map_err(|e| format!("hex_decode:{}", e))?;
                                let signature = bls_sign(&hash_bytes, &bls_key)
                                    .map_err(|e| format!("bls_sign:{}", e))?;
                                Ok(signature)
                            })
                            .await;

                        match vote_compute {
                            Ok(Ok(signature)) => {
                                let final_height = self.state.get_checkpoint_height();
                                if vote_height <= final_height {
                                    debug!(
                                        "Post-check: declining vote for checkpoint {} (committed during processing, height now {})",
                                        vote_height, final_height
                                    );
                                    VoteResponse::CheckpointVote(None)
                                } else {
                                    if total_txs > 0 {
                                        info!(
                                            "Verified merkle root for checkpoint {} vote ({} txs)",
                                            request.height, total_txs
                                        );
                                    }
                                    let response = CheckpointVoteResponse {
                                        validator_address: signer.validator_address.clone(),
                                        signature: URL_SAFE_NO_PAD.encode(&signature),
                                        signature_bytes: signature,
                                        bls_public_key: URL_SAFE_NO_PAD
                                            .encode(&signer.bls_public_key),
                                        stake: GENESIS_VALIDATOR_STAKE,
                                    };
                                    VoteResponse::CheckpointVote(Some(response))
                                }
                            }
                            Ok(Err(err_msg)) => {
                                if err_msg.starts_with("merkle_mismatch:") {
                                    let parts: Vec<&str> = err_msg.splitn(3, ':').collect();
                                    warn!(
                                        "Declining checkpoint vote for height {}: merkle root mismatch (ours={}, theirs={})",
                                        request.height, parts.get(1).unwrap_or(&"?"), parts.get(2).unwrap_or(&"?")
                                    );
                                } else if err_msg.starts_with("no_hashes_nonzero_root:") {
                                    warn!(
                                        "Declining vote for height {}: non-empty merkle root but no tx hashes provided",
                                        request.height
                                    );
                                } else {
                                    warn!(
                                        "Vote computation failed for height {}: {}",
                                        request.height, err_msg
                                    );
                                }
                                VoteResponse::CheckpointVote(None)
                            }
                            Err(e) => VoteResponse::Error {
                                message: format!("Vote spawn_blocking failed: {}", e),
                            },
                        }
                    }
                } else {
                    VoteResponse::CheckpointVote(None)
                };
                let vote_ms = vote_start.elapsed().as_millis();
                if vote_ms > 100 {
                    warn!(
                        "Vote request for checkpoint {} processed in {}ms (slow!)",
                        vote_height, vote_ms
                    );
                } else {
                    info!(
                        "Vote request for checkpoint {} processed in {}ms",
                        vote_height, vote_ms
                    );
                }
                vote_response
            }
        };

        if let Some(ref tx) = response_tx {
            let _ = tx
                .send(NetworkCommand::SendVoteResponse(
                    incoming.response_channel,
                    response,
                ))
                .await;
        }
    }

    #[cfg(feature = "p2p")]
    async fn process_sync_request(
        &self,
        incoming: IncomingSyncRequest,
        response_tx: Option<tokio::sync::mpsc::Sender<NetworkCommand>>,
    ) {
        use crate::network::{
            AccountData, CheckpointData, DeltaData, PeerHandshake, SnapshotData, SyncRequest,
            SyncResponse, TransactionData, ValidatorData,
        };

        info!(
            "Handling sync request from {}: {:?}",
            incoming.peer_id, incoming.request
        );

        let response = match incoming.request {
            SyncRequest::Snapshot => {
                // Use the existing get_sync_snapshot() method which gathers all state
                let snapshot = self.state.get_sync_snapshot().await;

                // Convert to network SnapshotData format
                let account_data: Vec<AccountData> = snapshot
                    .accounts
                    .values()
                    .map(|a| AccountData {
                        address: a.address.clone(),
                        balance: a.balance,
                        nonce: a.nonce,
                        stake: a.staked,
                    })
                    .collect();

                // validators is HashMap<String, Validator>, iterate over values
                let validator_data: Vec<ValidatorData> = snapshot
                    .validators
                    .values()
                    .map(|v| ValidatorData {
                        address: v.address.clone(),
                        stake: v.stake,
                        bls_public_key: v.bls_public_key.clone().unwrap_or_default(),
                        status: "Active".to_string(),
                    })
                    .collect();

                // Map Checkpoint fields correctly
                let checkpoint_data: Vec<CheckpointData> = snapshot
                    .checkpoints
                    .iter()
                    .map(|c| CheckpointData {
                        height: c.height,
                        merkle_root: c.tx_merkle_root.clone(),
                        timestamp: c.timestamp,
                        tx_count: c.tip_count as u64,
                        hash: Some(c.hash.clone()),
                        previous_hash: c.previous_hash.clone(),
                        signature: c.aggregated_signature.clone(),
                        genesis_hash: snapshot.genesis_hash.clone(),
                        finalized_tx_hashes: c.finalized_tx_hashes.clone(),
                        state_root: Some(c.state_root.clone()),
                        receipt_root: Some(c.receipt_root.clone()),
                        tip_count: Some(c.tip_count),
                        validator_signatures: c.validator_signatures.clone(),
                        signer_bitmap: c.signer_bitmap.clone(),
                    })
                    .collect();

                let tx_data: Vec<TransactionData> = snapshot
                    .dag_transactions
                    .iter()
                    .map(|stx| TransactionData {
                        hash: stx.hash.clone(),
                        from: stx.tx.from.clone(),
                        to: stx.tx.to.clone(),
                        amount: stx.tx.amount,
                        nonce: stx.tx.nonce,
                        timestamp: stx.tx.timestamp,
                        signature: stx.signature.clone(),
                        parents: stx.tx.parents.clone(),
                        gas_price: stx.tx.gas_price.unwrap_or(0),
                        memo: stx.tx.memo.clone(),
                        references: stx.tx.references.clone(),
                    })
                    .collect();

                // Get merkle root from latest checkpoint
                let merkle_root = snapshot
                    .checkpoints
                    .last()
                    .map(|c| c.tx_merkle_root.clone())
                    .unwrap_or_default();

                info!(
                    "Sending snapshot: {} accounts, {} validators, {} checkpoints, {} txs",
                    account_data.len(),
                    validator_data.len(),
                    checkpoint_data.len(),
                    tx_data.len()
                );

                SyncResponse::Snapshot(SnapshotData {
                    accounts: account_data,
                    validators: validator_data,
                    checkpoints: checkpoint_data,
                    recent_txs: tx_data,
                    merkle_root,
                })
            }
            SyncRequest::Delta { from_checkpoint } => {
                let delta_start = std::time::Instant::now();
                info!(
                    "Handling P2P delta sync request from checkpoint {}",
                    from_checkpoint
                );

                let (
                    new_checkpoints,
                    tx_checkpoint_heights,
                    to_checkpoint,
                    max_included_height,
                    validators,
                    txs,
                ) = {
                    let state_guard = self.state.inner.read().await;

                    let local_tip = state_guard.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
                    let gap = local_tip.saturating_sub(from_checkpoint);
                    let max_sync_checkpoints: usize = if gap > 10 { 5 } else { 20 };
                    let new_checkpoints: Vec<CheckpointData> = state_guard
                        .checkpoints
                        .iter()
                        .filter(|cp| cp.height > from_checkpoint)
                        .take(max_sync_checkpoints)
                        .map(|cp| CheckpointData {
                            height: cp.height,
                            merkle_root: cp.tx_merkle_root.clone(),
                            timestamp: cp.timestamp,
                            tx_count: cp.finalized_tx_hashes.len() as u64,
                            hash: Some(cp.hash.clone()),
                            previous_hash: cp.previous_hash.clone(),
                            signature: cp.aggregated_signature.clone(),
                            genesis_hash: state_guard.genesis_hash.clone(),
                            finalized_tx_hashes: cp.finalized_tx_hashes.clone(),
                            state_root: Some(cp.state_root.clone()),
                            receipt_root: Some(cp.receipt_root.clone()),
                            tip_count: Some(cp.tip_count),
                            validator_signatures: cp.validator_signatures.clone(),
                            signer_bitmap: cp.signer_bitmap.clone(),
                        })
                        .collect();

                    let max_included_height = new_checkpoints
                        .last()
                        .map(|cp| cp.height)
                        .unwrap_or(from_checkpoint);
                    let to_checkpoint = state_guard
                        .checkpoints
                        .last()
                        .map(|cp| cp.height)
                        .unwrap_or(0);

                    let included_tx_hashes: std::collections::HashSet<String> = new_checkpoints
                        .iter()
                        .flat_map(|cp| cp.finalized_tx_hashes.iter().cloned())
                        .collect();

                    let mut tx_checkpoint_heights = std::collections::HashMap::new();
                    let mut txs: Vec<TransactionData> = Vec::new();
                    for node in state_guard.dag.get_all_nodes() {
                        if let Some(height) = node.checkpoint_height {
                            if height > from_checkpoint && height <= max_included_height {
                                tx_checkpoint_heights.insert(node.hash.clone(), height);
                            }
                        }
                        let dominated = node.checkpoint_height
                            .map(|h| h > from_checkpoint)
                            .unwrap_or(true);
                        if dominated && included_tx_hashes.contains(&node.hash) {
                            txs.push(TransactionData {
                                hash: node.hash.clone(),
                                from: node.tx.tx.from.clone(),
                                to: node.tx.tx.to.clone(),
                                amount: node.tx.tx.amount,
                                nonce: node.tx.tx.nonce,
                                timestamp: node.tx.tx.timestamp,
                                signature: node.tx.signature.clone(),
                                parents: node.tx.tx.parents.clone(),
                                gas_price: node.tx.tx.gas_price.unwrap_or(0),
                                memo: node.tx.tx.memo.clone(),
                                references: node.tx.tx.references.clone(),
                            });
                        }
                    }

                    let validators: Vec<ValidatorData> = state_guard
                        .validators
                        .values()
                        .map(|v| ValidatorData {
                            address: v.address.clone(),
                            stake: v.stake,
                            bls_public_key: v.bls_public_key.clone().unwrap_or_default(),
                            status: "Active".to_string(),
                        })
                        .collect();

                    (
                        new_checkpoints,
                        tx_checkpoint_heights,
                        to_checkpoint,
                        max_included_height,
                        validators,
                        txs,
                    )
                };

                let delta_proofs = {
                    let inner = self.inner.read().await;
                    let mut proofs_map: std::collections::HashMap<
                        String,
                        rinku_core::types::AccountStateProof,
                    > = std::collections::HashMap::new();
                    for cp in &new_checkpoints {
                        if let Some(cached) = inner.recent_checkpoint_data.get(&cp.height) {
                            for proof in &cached.precomputed_proofs {
                                proofs_map.insert(proof.address.clone(), proof.clone());
                            }
                        }
                    }
                    let proofs: Vec<rinku_core::types::AccountStateProof> =
                        proofs_map.into_values().collect();
                    if !proofs.is_empty() {
                        info!(
                            "Delta sync: including {} precomputed proofs for state correction",
                            proofs.len()
                        );
                    }
                    proofs
                };

                let delta_ms = delta_start.elapsed().as_millis();
                let response = SyncResponse::Delta(DeltaData {
                    transactions: txs.clone(),
                    new_checkpoints: new_checkpoints.clone(),
                    from_checkpoint,
                    to_checkpoint: max_included_height,
                    tx_checkpoint_heights,
                    validators,
                    precomputed_proofs: delta_proofs,
                });
                let response_size = serde_json::to_string(&response)
                    .map(|s| s.len())
                    .unwrap_or(0);
                info!("P2P delta sync response: {} txs, {} checkpoints (from {} to {}) built in {}ms, ~{}KB serialized",
                                txs.len(), new_checkpoints.len(), from_checkpoint, max_included_height, delta_ms, response_size / 1024);
                if max_included_height < to_checkpoint {
                    info!(
                        "Delta sync chunked: sent up to height {} (peer still needs up to {})",
                        max_included_height, to_checkpoint
                    );
                }

                response
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
                    gas_price: stx.tx.gas_price.unwrap_or(0),
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
                let our_handshake = PeerHandshake {
                    protocol_version: crate::versioning::PROTOCOL_VERSION.to_string(),
                    chain_id: "rinku-testnet".to_string(),
                    network_id: "rinku".to_string(),
                    node_id: self.state.get_node_id().await,
                    checkpoint_height: self.state.get_checkpoint_height(),
                    validator_address: self
                        .checkpoint_vote_signer
                        .as_ref()
                        .map(|s| s.validator_address.clone()),
                    capabilities: vec!["sync".to_string(), "gossip".to_string()],
                    known_peer_addrs: Vec::new(),
                };
                SyncResponse::Handshake(our_handshake)
            }
            SyncRequest::CheckpointPush(push_data) => {
                let applied = self
                    .apply_received_checkpoint(
                        push_data.checkpoint,
                        push_data.finalized_tx_hashes,
                        push_data.finalized_transactions,
                        push_data.precomputed_proofs,
                        "sync-push",
                    )
                    .await;
                if applied {
                    SyncResponse::CheckpointPushAck { accepted: true }
                } else {
                    SyncResponse::CheckpointPushAck { accepted: false }
                }
            }
        };

        if let Some(ref tx) = response_tx {
            if let Err(e) = tx.try_send(NetworkCommand::SendSyncResponse(
                incoming.response_channel,
                response,
            )) {
                warn!("Failed to send sync response: {}", e);
            }
        } else {
            warn!(
                "No response sender available for sync request from {}",
                incoming.peer_id
            );
        }
    }

    #[cfg(feature = "p2p")]
    async fn broadcast_via_p2p(&self, message: &GossipMessage) {
        if let Some(ref handle) = self.network_handle {
            if let Err(e) = handle.broadcast(message.clone()).await {
                warn!(
                    "Failed to broadcast via P2P: {} (type: {})",
                    e,
                    message.variant_name()
                );
            }
        }
    }

    async fn initial_sync(&self) {
        info!("Initial sync deferred to P2P layer (TipAnnouncement-triggered)");
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
        let checkpoint_height = self.state.get_checkpoint_height();

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
                inner
                    .peer_last_sync
                    .retain(|_, last_sync| last_sync.elapsed().as_secs() < 60);

                // Cleanup stale peer_last_recovery entries (older than 60 seconds)
                inner
                    .peer_last_recovery
                    .retain(|_, last_recovery| last_recovery.elapsed().as_secs() < 60);

                // Cleanup stale peer_bloom_filters for disconnected peers
                let active_peer_ids: std::collections::HashSet<String> =
                    inner.peers.keys().cloned().collect();
                let old_bloom_count = inner.peer_bloom_filters.len();
                inner
                    .peer_bloom_filters
                    .retain(|peer_id, _| active_peer_ids.contains(peer_id));
                let bloom_pruned = old_bloom_count - inner.peer_bloom_filters.len();
                if bloom_pruned > 0 {
                    info!("Pruned {} stale peer bloom filters", bloom_pruned);
                }

                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                const CONVERGENCE_TTL_MS: u64 = 60_000; // 60 seconds
                const CONVERGENCE_PENDING_TTL_MS: u64 = 60_000; // 60 seconds
                const CONVERGENCE_MAX_CONFIRMED: usize = 500;
                const CONVERGENCE_MAX_PENDING: usize = 500;
                const CONVERGENCE_MAX_EXECUTED: usize = 2_000;

                let old_confirmed_count = inner.convergence_confirmed.len();
                let executed_snapshot: std::collections::HashSet<String> =
                    inner.convergence_executed.clone();
                inner.convergence_confirmed.retain(|tx_hash, finality| {
                    if executed_snapshot.contains(tx_hash)
                        && !matches!(
                            finality.status,
                            rinku_core::types::FastPathStatus::Finalized
                        )
                    {
                        return true;
                    }
                    if let Some(confirmed_at) = finality.confirmed_at_ms {
                        now_ms.saturating_sub(confirmed_at) < CONVERGENCE_TTL_MS
                    } else {
                        now_ms.saturating_sub(finality.registered_at_ms) < CONVERGENCE_TTL_MS
                    }
                });
                if inner.convergence_confirmed.len() > CONVERGENCE_MAX_CONFIRMED {
                    let mut entries: Vec<(String, u64)> = inner
                        .convergence_confirmed
                        .iter()
                        .map(|(k, v)| (k.clone(), v.confirmed_at_ms.unwrap_or(v.registered_at_ms)))
                        .collect();
                    entries.sort_by_key(|(_, ts)| *ts);
                    let to_evict = entries.len() - CONVERGENCE_MAX_CONFIRMED;
                    for (key, _) in entries.into_iter().take(to_evict) {
                        inner.convergence_confirmed.remove(&key);
                    }
                }
                let confirmed_pruned = old_confirmed_count - inner.convergence_confirmed.len();

                let old_pending_count = inner.convergence_pending.len();
                inner.convergence_pending.retain(|_, finality| {
                    now_ms.saturating_sub(finality.registered_at_ms) < CONVERGENCE_PENDING_TTL_MS
                });
                if inner.convergence_pending.len() > CONVERGENCE_MAX_PENDING {
                    let mut entries: Vec<(String, u64)> = inner
                        .convergence_pending
                        .iter()
                        .map(|(k, v)| (k.clone(), v.registered_at_ms))
                        .collect();
                    entries.sort_by_key(|(_, ts)| *ts);
                    let to_evict = entries.len() - CONVERGENCE_MAX_PENDING;
                    for (key, _) in entries.into_iter().take(to_evict) {
                        inner.convergence_pending.remove(&key);
                    }
                }
                let pending_pruned = old_pending_count - inner.convergence_pending.len();

                let old_executed_count = inner.convergence_executed.len();
                let confirmed_keys: std::collections::HashSet<&String> =
                    inner.convergence_confirmed.keys().collect();
                let orphaned: Vec<String> = inner
                    .convergence_executed
                    .iter()
                    .filter(|h| !confirmed_keys.contains(h))
                    .cloned()
                    .collect();
                for key in orphaned {
                    inner.convergence_executed.remove(&key);
                }
                if inner.convergence_executed.len() > CONVERGENCE_MAX_EXECUTED {
                    let drain_count = inner.convergence_executed.len() - CONVERGENCE_MAX_EXECUTED;
                    let to_drain: Vec<String> = inner
                        .convergence_executed
                        .iter()
                        .take(drain_count)
                        .cloned()
                        .collect();
                    for key in to_drain {
                        inner.convergence_executed.remove(&key);
                    }
                }
                let executed_pruned = old_executed_count - inner.convergence_executed.len();

                let executed_snapshot: std::collections::HashSet<String> =
                    inner.convergence_executed.clone();

                {
                    let live_hashes: std::collections::HashSet<String> = inner.convergence_pending.keys()
                        .chain(inner.convergence_confirmed.keys())
                        .cloned()
                        .collect();
                    inner.convergence_tx_bodies.retain(|h, _| live_hashes.contains(h));
                }

                let old_pending_exec_count = inner.confirmed_pending_execution.len();
                inner
                    .confirmed_pending_execution
                    .retain(|tx| !executed_snapshot.contains(&tx.hash));
                let pending_exec_pruned =
                    old_pending_exec_count - inner.confirmed_pending_execution.len();
                if pending_exec_pruned > 0 {
                    tracing::debug!(
                        "Fast-path cleanup: pruned {} already-executed from pending batch ({} remaining)",
                        pending_exec_pruned, inner.confirmed_pending_execution.len()
                    );
                }

                drop(inner);

                let inner = self.inner.read().await;
                if confirmed_pruned > 0 || pending_pruned > 0 || executed_pruned > 0 {
                    info!(
                        "Fast-path cleanup: {} confirmed, {} pending, {} executed pruned (remaining: {} confirmed, {} pending, {} executed)",
                        confirmed_pruned, pending_pruned, executed_pruned,
                        inner.convergence_confirmed.len(), inner.convergence_pending.len(), inner.convergence_executed.len()
                    );
                }

                // Log stale nonce cache stats
                // The cache tracks (address -> min_expected_nonce) to quick-reject stale gossip
                let cache_size = inner.stale_nonce_cache.len();
                let cache_hits = inner
                    .stale_nonce_cache_hits
                    .load(std::sync::atomic::Ordering::Relaxed);
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

        self.execute_convergence_batch().await;
        self.cleanup_stale_da_collectors().await;

        // Cap tips to 10 to prevent bandwidth explosion when tip count is high
        // Peers only need a sample of tips for sync, not the full set
        const MAX_GOSSIP_TIPS: usize = 10;
        let tips_capped: Vec<String> = tips.iter().take(MAX_GOSSIP_TIPS).cloned().collect();
        let tip_urls: Vec<String> = tips_capped
            .iter()
            .map(|h| format!("rinku://tx/h/{}", h))
            .collect();

        let merkle_root = self.state.get_dag_merkle_root().await.unwrap_or_default();

        let mut tips_sorted = tips_capped.clone();
        tips_sorted.sort();
        let mut tip_hash_state = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&tips_sorted, &mut tip_hash_state);
        std::hash::Hash::hash(&dag_size, &mut tip_hash_state);
        std::hash::Hash::hash(&merkle_root, &mut tip_hash_state);
        std::hash::Hash::hash(&checkpoint_height, &mut tip_hash_state);
        let tip_content_hash = std::hash::Hasher::finish(&tip_hash_state);

        let should_publish_tip = {
            let inner = self.inner.read().await;
            inner.last_tip_hash != tip_content_hash
        };

        if should_publish_tip {
            let public_url = std::env::var("PUBLIC_URL").ok();
            let tips_announced = tips_capped.len();
            let message = GossipMessage::TipAnnouncement {
                tips: tips_capped,
                tip_urls,
                dag_size,
                merkle_root,
                sender_url: public_url.clone(),
            };

            #[cfg(feature = "p2p")]
            self.broadcast_via_p2p(&message).await;

            {
                let mut inner = self.inner.write().await;
                inner.last_tip_hash = tip_content_hash;
            }

            debug!(
                "Gossip round: {}/{} tips announced via p2p, dag_size={}",
                tips_announced,
                tips.len(),
                dag_size
            );
        }

        // Spawn propagation as background task (non-blocking)
        Self::spawn_propagation_task(gossip_arc);

        self.request_sync_if_needed(checkpoint_height).await;

        // Periodic peer exchange: every 30 seconds, share known peers
        let should_exchange_peers = {
            let inner = self.inner.read().await;
            inner.round_counter % 150 == 0 // ~30s at 200ms interval
        };
        if should_exchange_peers {
            self.broadcast_peer_list().await;
        }

        self.broadcast_slashing_evidence_round().await;
    }

    fn evidence_id(evidence: &DoubleSignEvidence) -> String {
        format!(
            "{}:{}:{}:{}",
            evidence.validator, evidence.checkpoint_height, evidence.hash1, evidence.hash2
        )
    }

    async fn broadcast_slashing_evidence_round(&self) {
        let pending: Vec<DoubleSignEvidence> = {
            let slashing = self.state.slashing.read().await;
            slashing
                .get_pending_evidence()
                .into_iter()
                .cloned()
                .collect()
        };

        if pending.is_empty() {
            return;
        }

        let public_url = std::env::var("PUBLIC_URL").ok();

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

            #[cfg(feature = "p2p")]
            self.broadcast_via_p2p(&message).await;
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

        let message = GossipMessage::SlashingEvidence {
            evidence,
            sender_url: public_url,
        };

        #[cfg(feature = "p2p")]
        self.broadcast_via_p2p(&message).await;
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

        let message = GossipMessage::WeightVote {
            tx_hash: tx_hash.clone(),
            validator_pubkey,
            vote,
            timestamp_ms,
            bls_signature,
            sender_url: public_url,
        };

        #[cfg(feature = "p2p")]
        self.broadcast_via_p2p(&message).await;

        debug!(
            "Broadcast weight vote for tx {} via p2p",
            &tx_hash[..16.min(tx_hash.len())]
        );
    }

    pub async fn apply_received_checkpoint(
        &self,
        checkpoint: Checkpoint,
        finalized_tx_hashes: Vec<String>,
        finalized_transactions: Vec<SignedTransaction>,
        precomputed_proofs: Vec<rinku_core::types::AccountStateProof>,
        source: &str,
    ) -> bool {
        self.record_peer_height(checkpoint.height);

        let local_height = self.state.get_checkpoint_height();

        'already_seen: {
            if checkpoint.height <= local_height {
                let fork_resolved = {
                    let state_guard = self.state.inner.read().await;
                    let local_checkpoint_at_height = state_guard
                        .checkpoints
                        .iter()
                        .find(|c| c.height == checkpoint.height);
                    if let Some(local_cp) = local_checkpoint_at_height {
                        if local_cp.hash != checkpoint.hash {
                            let local_hash = local_cp.hash.clone();
                            let remote_hash = checkpoint.hash.clone();
                            let local_is_last = state_guard
                                .checkpoints
                                .last()
                                .map(|c| c.height == checkpoint.height)
                                .unwrap_or(false);
                            let prev_hash_matches =
                                if local_cp.previous_hash == checkpoint.previous_hash {
                                    true
                                } else {
                                    false
                                };
                            drop(state_guard);

                            if !prev_hash_matches {
                                warn!(
                                "FORK RESOLUTION: Rejecting competing checkpoint at height {} — previous_hash mismatch (not from same fork point, via {})",
                                checkpoint.height, source
                            );
                                return false;
                            }

                            if remote_hash < local_hash && local_is_last {
                                warn!(
                                "FORK RESOLUTION: Incoming checkpoint {} wins over local {} at height {} (lower hash, via {}) — rolling back and re-applying",
                                &remote_hash[..16.min(remote_hash.len())],
                                &local_hash[..16.min(local_hash.len())],
                                checkpoint.height,
                                source
                            );

                                match self.state.rollback_last_checkpoint().await {
                                    Ok((rolled_back_cp, _rolled_back_hashes)) => {
                                        info!(
                                        "FORK RESOLUTION: Successfully rolled back checkpoint {} at height {} — now applying winner",
                                        &rolled_back_cp.hash[..16.min(rolled_back_cp.hash.len())],
                                        rolled_back_cp.height
                                    );
                                        true
                                    }
                                    Err(e) => {
                                        error!(
                                        "FORK RESOLUTION: Rollback failed at height {}: {} — keeping local checkpoint",
                                        checkpoint.height, e
                                    );
                                        return false;
                                    }
                                }
                            } else if remote_hash < local_hash && !local_is_last {
                                warn!(
                                "FORK RESOLUTION: Incoming checkpoint wins at height {} but fork is too deep (not last checkpoint) — needs full re-sync",
                                checkpoint.height
                            );
                                return false;
                            } else {
                                debug!(
                                "FORK RESOLUTION: Keeping local checkpoint {} over incoming {} at height {} (local hash wins, via {})",
                                &local_hash[..16.min(local_hash.len())],
                                &remote_hash[..16.min(remote_hash.len())],
                                checkpoint.height,
                                source
                            );
                                return false;
                            }
                        } else {
                            drop(state_guard);
                            false
                        }
                    } else {
                        drop(state_guard);
                        false
                    }
                };

                if fork_resolved {
                    break 'already_seen;
                }

                if source == "sync-push" && !finalized_transactions.is_empty() {
                    info!(
                    "Late sync-push for already-applied checkpoint {} at height {} — retroactively adding {} tx bodies and {} proofs",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    checkpoint.height,
                    finalized_transactions.len(),
                    precomputed_proofs.len()
                );
                    let mut added_hashes: Vec<String> = Vec::new();
                    for tx in &finalized_transactions {
                        if !self.state.has_transaction(&tx.hash).await {
                            if let Ok(()) = self.state.add_transaction_dag_only(tx.clone()).await {
                                added_hashes.push(tx.hash.clone());
                            }
                        }
                    }
                    let added = added_hashes.len();
                    if added > 0 {
                        let mut sorted_finalized = finalized_transactions.clone();
                        sorted_finalized.sort_by(|a, b| {
                            a.tx.from
                                .cmp(&b.tx.from)
                                .then(a.tx.nonce.cmp(&b.tx.nonce))
                                .then(a.hash.cmp(&b.hash))
                        });
                        let empty_convergence = std::collections::HashSet::new();
                        let _ = self
                            .state
                            .execute_finalized_transactions_batch(
                                &sorted_finalized,
                                &empty_convergence,
                            )
                            .await;
                        for tx_hash in &finalized_tx_hashes {
                            self.state
                                .set_tx_checkpoint_height(tx_hash, checkpoint.height)
                                .await;
                        }
                        info!(
                        "Late sync-push: added {} new txs to DAG, executed, and finalized at height {} for checkpoint {}",
                        added, checkpoint.height, &checkpoint.hash[..16.min(checkpoint.hash.len())]
                    );
                    }
                    if !precomputed_proofs.is_empty() {
                        let proof_map: std::collections::HashMap<
                            String,
                            rinku_core::types::AccountStateProof,
                        > = precomputed_proofs
                            .into_iter()
                            .map(|p| (p.address.clone(), p))
                            .collect();
                        self.state.store_precomputed_proofs(&proof_map).await;
                        self.state.purge_stale_deferred_txs().await;
                        self.state.purge_zombie_dag_txs().await;
                        info!(
                            "Late sync-push: applied {} leader proofs for checkpoint {}",
                            proof_map.len(),
                            checkpoint.height
                        );
                    }
                } else {
                    debug!(
                        "Ignoring checkpoint {} at height {} (local height: {}, via {})",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        checkpoint.height,
                        local_height,
                        source
                    );
                }
                return false;
            }
        }

        let local_height = self.state.get_checkpoint_height();

        if checkpoint.height > 2 {
            let total_stake = if let Some(ref identity) = self.validator_identity {
                let identity_guard = identity.read().await;
                let validators = identity_guard.active_validators();
                if validators.len() > 1 {
                    let total: u64 = validators.iter().map(|(_, v)| v.effective_stake).sum();
                    Some(total)
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(total_stake) = total_stake {
                let quorum_threshold = (total_stake * 2 + 2) / 3;

                let canonical_sig_stake: u64 = if let Some(ref identity) = self.validator_identity {
                    let identity_guard = identity.read().await;
                    let validators = identity_guard.active_validators();
                    checkpoint
                        .validator_signatures
                        .iter()
                        .map(|s| {
                            validators
                                .iter()
                                .find(|(addr, _)| *addr == &s.validator)
                                .map(|(_, v)| v.effective_stake)
                                .unwrap_or(0)
                        })
                        .sum()
                } else {
                    checkpoint
                        .validator_signatures
                        .iter()
                        .map(|s| s.weight)
                        .sum()
                };

                if canonical_sig_stake < quorum_threshold {
                    warn!(
                        "QCC REJECT: Checkpoint {} at height {} has insufficient canonical quorum ({}/{} stake, need {}, {} sigs) — rejecting (via {})",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        checkpoint.height,
                        canonical_sig_stake, total_stake, quorum_threshold,
                        checkpoint.validator_signatures.len(),
                        source
                    );
                    return false;
                }

                if checkpoint.aggregated_signature.is_none() {
                    warn!(
                        "QCC REJECT: Checkpoint {} at height {} missing aggregated signature — rejecting (via {})",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        checkpoint.height,
                        source
                    );
                    return false;
                }

                info!(
                    "QCC VALID: Checkpoint {} at height {} has valid canonical quorum ({}/{} stake, {} sigs, via {})",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    checkpoint.height,
                    canonical_sig_stake, total_stake,
                    checkpoint.validator_signatures.len(),
                    source
                );
            }
        }

        if checkpoint.height != local_height + 1 {
            if checkpoint.height <= local_height + CHECKPOINT_BUFFER_MAX_AHEAD {
                let mut buffer = self.checkpoint_buffer.lock().await;
                if !buffer.contains_key(&checkpoint.height) {
                    info!(
                        "Buffering checkpoint {} at height {} (local: {}, via {}) — will apply when caught up",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        checkpoint.height,
                        local_height,
                        source
                    );
                    buffer.insert(
                        checkpoint.height,
                        BufferedCheckpoint {
                            checkpoint,
                            finalized_tx_hashes,
                            finalized_transactions,
                            precomputed_proofs,
                            source: source.to_string(),
                        },
                    );
                }
                drop(buffer);
                let updated_height = self.state.get_checkpoint_height();
                if updated_height >= local_height + 1 {
                    debug!(
                        "Height advanced during buffering ({} -> {}), draining buffer",
                        local_height, updated_height
                    );
                    self.drain_checkpoint_buffer().await;
                }
            } else {
                debug!(
                    "Checkpoint {} at height {} too far ahead (local: {}, via {}), will sync",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    checkpoint.height,
                    local_height,
                    source
                );
            }
            return false;
        }

        let applied = self
            .apply_single_checkpoint(
                checkpoint,
                finalized_tx_hashes,
                finalized_transactions,
                precomputed_proofs,
                source,
            )
            .await;

        if applied {
            self.drain_checkpoint_buffer().await;
        }

        applied
    }

    async fn apply_single_checkpoint(
        &self,
        checkpoint: Checkpoint,
        finalized_tx_hashes: Vec<String>,
        finalized_transactions: Vec<SignedTransaction>,
        precomputed_proofs: Vec<rinku_core::types::AccountStateProof>,
        source: &str,
    ) -> bool {
        let current_height = self.state.get_checkpoint_height();
        if checkpoint.height <= current_height {
            debug!(
                "Skipping duplicate apply of checkpoint {} at height {} (current: {}, via {})",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                checkpoint.height,
                current_height,
                source
            );
            return false;
        }
        if checkpoint.height != current_height + 1 {
            warn!(
                "Checkpoint {} at height {} is not sequential (current: {}, via {}) — skipping",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                checkpoint.height,
                current_height,
                source
            );
            return false;
        }
        info!(
            "Received checkpoint {} at height {} via {} ({} finalized txs, {} tx bodies, {} proofs)",
            &checkpoint.hash[..16.min(checkpoint.hash.len())],
            checkpoint.height,
            source,
            finalized_tx_hashes.len(),
            finalized_transactions.len(),
            precomputed_proofs.len()
        );

        if let Some(ref vi) = self.validator_identity {
            let vi_guard = vi.read().await;
            let mut sorted_entries: Vec<(String, Vec<u8>, u64)> = vi_guard
                .active_validators()
                .iter()
                .filter(|(_, v)| !v.bls_public_key.is_empty())
                .map(|(addr, v)| (addr.clone(), v.bls_public_key.clone(), v.effective_stake))
                .collect();
            sorted_entries.sort_by(|a, b| a.0.cmp(&b.0));
            let bls_keys_and_stakes: Vec<(Vec<u8>, u64)> =
                sorted_entries.into_iter().map(|(_, k, s)| (k, s)).collect();
            drop(vi_guard);

            if !bls_keys_and_stakes.is_empty() {
                match crate::state::NodeState::verify_checkpoint_bls_signature_only(
                    &checkpoint,
                    &bls_keys_and_stakes,
                ) {
                    crate::state::checkpoints::BlsVerifyResult::ValidWithQuorum => {}
                    crate::state::checkpoints::BlsVerifyResult::ValidNoQuorum { .. } => {}
                    crate::state::checkpoints::BlsVerifyResult::NoSignature => {}
                    crate::state::checkpoints::BlsVerifyResult::Invalid(reason) => {
                        warn!(
                            "BLS signature INVALID for checkpoint {} at height {} — rejecting: {}",
                            &checkpoint.hash[..16.min(checkpoint.hash.len())],
                            checkpoint.height,
                            reason
                        );
                        return false;
                    }
                }
            }
        }

        let mut added_from_leader = 0usize;
        let mut rejected_count = 0usize;

        let finalized_hash_set: std::collections::HashSet<&String> =
            finalized_tx_hashes.iter().collect();

        if !finalized_transactions.is_empty() {
            let existing_hashes: std::collections::HashSet<String> = {
                let state = self.state.inner.read().await;
                finalized_tx_hashes
                    .iter()
                    .filter(|h| state.dag.get_node(h).is_some())
                    .cloned()
                    .collect()
            };

            let mut txs_to_add: Vec<SignedTransaction> = Vec::new();
            for tx in &finalized_transactions {
                if existing_hashes.contains(&tx.hash) {
                    continue;
                }
                if !finalized_hash_set.contains(&tx.hash) {
                    warn!(
                        "SECURITY: Rejecting tx {} - not in finalized hash list for checkpoint {}",
                        &tx.hash[..16.min(tx.hash.len())],
                        checkpoint.height
                    );
                    rejected_count += 1;
                    continue;
                }
                txs_to_add.push(tx.clone());
            }

            if !txs_to_add.is_empty() {
                let batch_count = txs_to_add.len();
                let added_hashes: Vec<String> =
                    txs_to_add.iter().map(|tx| tx.hash.clone()).collect();
                match self
                    .state
                    .force_add_transactions_batch_for_vote(txs_to_add)
                    .await
                {
                    Ok(count) => {
                        added_from_leader = count;
                    }
                    Err(e) => {
                        debug!(
                            "Batch add {} txs for checkpoint {} failed: {}",
                            batch_count, checkpoint.height, e
                        );
                    }
                }

            }

            if added_from_leader > 0 || rejected_count > 0 {
                info!(
                    "Checkpoint {} tx sync: added {} from leader, rejected {} (security)",
                    checkpoint.height, added_from_leader, rejected_count
                );
            }
        }

        {
            let mut emission = self.state.emission.write().await;
            let reward = emission.get_checkpoint_reward(checkpoint.height);
            if emission.record_emission_for_height(checkpoint.height, reward) {
                let mut rewards = self.state.rewards.write().await;
                rewards.distribute_checkpoint_rewards(reward);
            } else {
                tracing::debug!(
                    "Skipping reward distribution for checkpoint {} — already distributed (leader pre-distribution)",
                    checkpoint.height
                );
            }
        }

        let finalized_hashes_for_fastpath = finalized_tx_hashes.clone();
        match self
            .state
            .apply_checkpoint_with_finalized_hashes(checkpoint.clone(), finalized_tx_hashes)
            .await
        {
            Err(e) => {
                warn!(
                    "Failed to apply checkpoint {} via {}: {}",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    source,
                    e
                );
                return false;
            }
            Ok(missing_tx_count) => {
                if missing_tx_count > 0 {
                    warn!(
                        "CONSENSUS WARNING: Still missing {} txs after leader sync for checkpoint {} (added {} from leader, had {} tx bodies, via {})",
                        missing_tx_count, checkpoint.height,
                        added_from_leader, finalized_transactions.len(), source
                    );
                }

                let proofs_for_cache = precomputed_proofs.clone();

                if !precomputed_proofs.is_empty() {
                    let proof_map: std::collections::HashMap<
                        String,
                        rinku_core::types::AccountStateProof,
                    > = precomputed_proofs
                        .into_iter()
                        .map(|p| (p.address.clone(), p))
                        .collect();
                    self.state.store_precomputed_proofs(&proof_map).await;
                    self.state.purge_stale_deferred_txs().await;
                    self.state.purge_zombie_dag_txs().await;
                    if missing_tx_count > 0 {
                        info!(
                            "Applied {} leader proofs for checkpoint {} despite {} missing txs — proofs are authoritative state",
                            proof_map.len(), checkpoint.height, missing_tx_count
                        );
                    } else {
                        info!(
                            "Stored {} precomputed proofs from leader for checkpoint {}",
                            proof_map.len(),
                            checkpoint.height
                        );
                    }
                }

                {
                    let finalized_set: std::collections::HashSet<&String> =
                        finalized_hashes_for_fastpath.iter().collect();
                    let mut inner = self.inner.write().await;
                    let mut executed_cleaned = 0usize;
                    for tx_hash in &finalized_hashes_for_fastpath {
                        inner.convergence_confirmed.remove(tx_hash);
                        inner.convergence_pending.remove(tx_hash);
                        if inner.convergence_executed.remove(tx_hash) {
                            executed_cleaned += 1;
                        }
                    }
                    if executed_cleaned > 0 {
                        info!(
                            "Checkpoint {} finalization: cleaned {} entries from convergence_executed ({} remaining)",
                            checkpoint.height, executed_cleaned, inner.convergence_executed.len()
                        );
                    }
                    inner.known_txs.remove_batch(&finalized_set);
                    inner.stale_nonce_cache.clear();
                    inner.stale_nonce_cache_order.clear();
                    inner.leader_intents.remove(&checkpoint.height);
                    inner
                        .leader_timeout_votes
                        .retain(|h, _| *h > checkpoint.height);
                    inner.leader_timeout_sent.retain(|h| *h > checkpoint.height);

                    let cp_height = checkpoint.height;
                    inner.recent_checkpoint_data.insert(
                        cp_height,
                        CachedCheckpointData {
                            checkpoint: checkpoint.clone(),
                            finalized_tx_hashes: finalized_hashes_for_fastpath.clone(),
                            finalized_transactions,
                            precomputed_proofs: proofs_for_cache,
                        },
                    );
                    const CHECKPOINT_CACHE_RETAIN: u64 = 10;
                    if cp_height > CHECKPOINT_CACHE_RETAIN {
                        inner
                            .recent_checkpoint_data
                            .retain(|h, _| *h > cp_height - CHECKPOINT_CACHE_RETAIN);
                    }
                }

                info!(
                    "Applied checkpoint {} at height {} via {} (added {} txs from leader)",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    checkpoint.height,
                    source,
                    added_from_leader
                );

                if let Some(ref nh) = self.network_handle {
                    nh.update_checkpoint_height(checkpoint.height);
                }

                if let Some(ref eb) = self.event_bus {
                    eb.publish(crate::events::NodeEvent::CheckpointCreated {
                        hash: checkpoint.hash.clone(),
                        height: checkpoint.height,
                        txs_finalized: finalized_hashes_for_fastpath.len(),
                        reward: 0.0,
                        validator_rewards: vec![],
                    });
                }
            }
        }

        true
    }

    pub fn get_highest_seen_peer_height(&self) -> u64 {
        self.highest_seen_peer_height
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn record_peer_height(&self, height: u64) {
        let local = self.state.get_checkpoint_height();
        let max_plausible = local + CHECKPOINT_BUFFER_MAX_AHEAD;
        if height <= max_plausible {
            self.highest_seen_peer_height
                .fetch_max(height, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub async fn has_buffered_checkpoint(&self, height: u64) -> bool {
        let buffer = self.checkpoint_buffer.lock().await;
        buffer.contains_key(&height)
    }

    pub async fn max_buffered_height(&self) -> Option<u64> {
        let buffer = self.checkpoint_buffer.lock().await;
        buffer.keys().max().copied()
    }

    pub async fn remove_finalized_from_convergence(&self, finalized_hashes: &[String]) {
        let finalized_set: std::collections::HashSet<&String> = finalized_hashes.iter().collect();
        let mut inner = self.inner.write().await;
        for tx_hash in finalized_hashes {
            inner.convergence_confirmed.remove(tx_hash);
            inner.convergence_pending.remove(tx_hash);
        }
        inner.known_txs.remove_batch(&finalized_set);
    }

    pub async fn broadcast_checkpoint_intent(&self, height: u64, leader_address: &str) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let public_url = std::env::var("PUBLIC_URL").ok();
        let message = GossipMessage::CheckpointIntent {
            height,
            leader_address: leader_address.to_string(),
            timestamp_ms: now_ms,
            sender_url: public_url,
            relay_count: 0,
        };
        info!(
            "Broadcasting checkpoint intent for height {} (leader: {})",
            height,
            &leader_address[..16.min(leader_address.len())]
        );
        #[cfg(feature = "p2p")]
        self.broadcast_via_p2p(&message).await;
    }

    pub async fn set_qcc_vote_channel(&self, tx: tokio::sync::mpsc::Sender<QccGossipVote>) {
        let mut guard = self.qcc_vote_tx.lock().await;
        *guard = Some(tx);
    }

    pub async fn clear_qcc_vote_channel(&self) {
        let mut guard = self.qcc_vote_tx.lock().await;
        *guard = None;
    }

    pub async fn broadcast_qcc_vote_request(
        &self,
        height: u64,
        checkpoint_hash: &str,
        tx_merkle_root: &str,
        state_root: &str,
        proposer_address: &str,
        finalized_tx_hashes: &[String],
    ) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let public_url = std::env::var("PUBLIC_URL").ok();
        let message = GossipMessage::QccVoteRequest {
            height,
            checkpoint_hash: checkpoint_hash.to_string(),
            tx_merkle_root: tx_merkle_root.to_string(),
            state_root: state_root.to_string(),
            proposer_address: proposer_address.to_string(),
            finalized_tx_hashes: finalized_tx_hashes.to_vec(),
            timestamp_ms: now_ms,
            sender_url: public_url,
        };
        info!(
            "QCC-GOSSIP: Broadcasting vote request for height {} ({} txs)",
            height,
            finalized_tx_hashes.len()
        );
        #[cfg(feature = "p2p")]
        self.broadcast_via_p2p(&message).await;
    }

    pub async fn has_leader_intent_for_height(&self, height: u64) -> bool {
        let inner = self.inner.read().await;
        inner.leader_intents.contains_key(&height)
    }

    pub async fn get_leader_intent_address(&self, height: u64) -> Option<String> {
        let inner = self.inner.read().await;
        inner
            .leader_intents
            .get(&height)
            .map(|(addr, _)| addr.clone())
    }

    pub async fn has_valid_leader_intent(&self, height: u64, interval_ms: u64) -> bool {
        let inner = self.inner.read().await;
        if let Some((leader_addr, timestamp_ms)) = inner.leader_intents.get(&height) {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let max_age_ms = interval_ms * 8;
            let age_ms = now_ms.saturating_sub(*timestamp_ms);
            if age_ms > max_age_ms {
                info!(
                    "Stale checkpoint intent for height {} from {} (age {}ms > {}ms max) — ignoring",
                    height, &leader_addr[..16.min(leader_addr.len())], age_ms, max_age_ms
                );
                return false;
            }
            true
        } else {
            false
        }
    }

    pub async fn clear_leader_intent(&self, height: u64) {
        let mut inner = self.inner.write().await;
        inner.leader_intents.remove(&height);
    }

    pub async fn broadcast_leader_timeout(
        &self,
        height: u64,
        validator_address: &str,
        validator_stake: u64,
    ) {
        let mut inner = self.inner.write().await;
        if inner.leader_timeout_sent.contains(&height) {
            return;
        }
        inner.leader_timeout_sent.insert(height);
        let height_votes = inner
            .leader_timeout_votes
            .entry(height)
            .or_insert_with(HashMap::new);
        height_votes.insert(validator_address.to_string(), validator_stake);
        drop(inner);

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let message = GossipMessage::LeaderTimeout {
            height,
            validator_address: validator_address.to_string(),
            validator_stake,
            timestamp_ms: now_ms,
            sender_url: std::env::var("PUBLIC_URL").ok(),
        };
        info!(
            "Broadcasting leader timeout vote for height {} (validator={}, stake={})",
            height,
            &validator_address[..16.min(validator_address.len())],
            validator_stake
        );
        #[cfg(feature = "p2p")]
        self.broadcast_via_p2p(&message).await;
    }

    pub async fn has_leader_timeout_quorum(&self, height: u64) -> bool {
        let (_, _, _, total_stake, _) = self.get_cached_validators().await;
        let inner = self.inner.read().await;
        if let Some(votes) = inner.leader_timeout_votes.get(&height) {
            let accumulated: u64 = votes.values().sum();
            let quorum_threshold = (total_stake * 2 + 2) / 3;
            accumulated >= quorum_threshold
        } else {
            false
        }
    }

    pub async fn get_leader_timeout_info(&self, height: u64) -> (u64, u64, bool) {
        let (_, _, _, total_stake, _) = self.get_cached_validators().await;
        let inner = self.inner.read().await;
        if let Some(votes) = inner.leader_timeout_votes.get(&height) {
            let accumulated: u64 = votes.values().sum();
            let quorum_threshold = (total_stake * 2 + 2) / 3;
            (accumulated, total_stake, accumulated >= quorum_threshold)
        } else {
            (0, total_stake, false)
        }
    }

    pub async fn clear_leader_timeouts_up_to(&self, height: u64) {
        let mut inner = self.inner.write().await;
        inner.leader_timeout_votes.retain(|h, _| *h > height);
        inner.leader_timeout_sent.retain(|h| *h > height);
    }

    pub async fn drain_checkpoint_buffer(&self) {
        loop {
            let next_height = self.state.get_checkpoint_height() + 1;
            let buffered = {
                let mut buffer = self.checkpoint_buffer.lock().await;
                buffer.remove(&next_height)
            };
            match buffered {
                Some(b) => {
                    info!(
                        "Applying buffered checkpoint at height {} (source: {})",
                        next_height, b.source
                    );
                    let applied = self
                        .apply_single_checkpoint(
                            b.checkpoint,
                            b.finalized_tx_hashes,
                            b.finalized_transactions,
                            b.precomputed_proofs,
                            &format!("buffered-{}", b.source),
                        )
                        .await;
                    if !applied {
                        break;
                    }
                }
                None => break,
            }
        }
        let local_height = self.state.get_checkpoint_height();
        let mut buffer = self.checkpoint_buffer.lock().await;
        buffer.retain(|h, _| *h > local_height);
    }

    pub async fn broadcast_checkpoint(
        &self,
        checkpoint: Checkpoint,
        finalized_tx_hashes: Vec<String>,
        finalized_transactions: Vec<SignedTransaction>,
        precomputed_proofs: Vec<rinku_core::types::AccountStateProof>,
    ) {
        let public_url = std::env::var("PUBLIC_URL").ok();

        info!(
            "Broadcasting checkpoint {} at height {} via p2p ({} finalized tx hashes, {} proofs, {} bodies)",
            &checkpoint.hash[..16.min(checkpoint.hash.len())],
            checkpoint.height,
            finalized_tx_hashes.len(),
            precomputed_proofs.len(),
            finalized_transactions.len(),
        );

        {
            let mut inner = self.inner.write().await;
            for tx_hash in &finalized_tx_hashes {
                inner.convergence_confirmed.remove(tx_hash);
                inner.convergence_pending.remove(tx_hash);
            }

            let height = checkpoint.height;
            inner.recent_checkpoint_data.insert(
                height,
                CachedCheckpointData {
                    checkpoint: checkpoint.clone(),
                    finalized_tx_hashes: finalized_tx_hashes.clone(),
                    finalized_transactions: finalized_transactions.clone(),
                    precomputed_proofs: precomputed_proofs.clone(),
                },
            );
            const CHECKPOINT_CACHE_RETAIN: u64 = 10;
            if height > CHECKPOINT_CACHE_RETAIN {
                inner
                    .recent_checkpoint_data
                    .retain(|h, _| *h > height - CHECKPOINT_CACHE_RETAIN);
            }
        }

        let serialized_bodies = serde_json::to_vec(&finalized_transactions).unwrap_or_default();
        let body_size = serialized_bodies.len();
        let proofs_size = serde_json::to_vec(&precomputed_proofs)
            .map(|v| v.len())
            .unwrap_or(0);
        let total_payload_size = body_size + proofs_size;

        info!(
            "Checkpoint {} gossip sizing: bodies={}KB, proofs={}KB, total={}KB (DA threshold={}KB)",
            &checkpoint.hash[..16.min(checkpoint.hash.len())],
            body_size / 1024,
            proofs_size / 1024,
            total_payload_size / 1024,
            crate::erasure::DA_SIZE_THRESHOLD / 1024,
        );

        if total_payload_size > crate::erasure::DA_SIZE_THRESHOLD {
            let validator_count = self.state.get_validators_map().await.len().max(3);
            let (k, m) = crate::erasure::compute_shard_params(validator_count);

            let (da_payload, is_compressed) =
                match crate::erasure::compress_da_payload(&serialized_bodies) {
                    Ok(compressed) => {
                        info!(
                            "DA compression: checkpoint {} body {}KB → {}KB ({:.0}% reduction)",
                            &checkpoint.hash[..16.min(checkpoint.hash.len())],
                            body_size / 1024,
                            compressed.len() / 1024,
                            (1.0 - compressed.len() as f64 / body_size as f64) * 100.0,
                        );
                        (compressed, true)
                    }
                    Err(e) => {
                        warn!("DA compression failed, using uncompressed: {}", e);
                        (serialized_bodies.clone(), false)
                    }
                };

            match crate::erasure::encode(&da_payload, k, m) {
                Ok((mut commitment, shards)) => {
                    commitment.compressed = is_compressed;
                    if is_compressed {
                        commitment.uncompressed_size = body_size;
                    }
                    info!(
                        "DA mode: checkpoint {} body {}KB → {} shards ({}+{}) for {} validators",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        da_payload.len() / 1024,
                        shards.len(),
                        k,
                        m,
                        validator_count,
                    );

                    let message = GossipMessage::CheckpointAnnouncement {
                        checkpoint: checkpoint.clone(),
                        sender_url: public_url.clone(),
                        finalized_tx_hashes,
                        precomputed_proofs: Vec::new(),
                        finalized_transactions: Vec::new(),
                        da_commitment: Some(commitment),
                    };

                    #[cfg(feature = "p2p")]
                    self.broadcast_via_p2p(&message).await;

                    #[cfg(feature = "p2p")]
                    for shard in &shards {
                        let chunk_msg = GossipMessage::DaChunk {
                            checkpoint_height: checkpoint.height,
                            checkpoint_hash: checkpoint.hash.clone(),
                            shard_index: shard.index,
                            shard_data: shard.data.clone(),
                            merkle_proof: shard.merkle_proof.clone(),
                            sender_url: public_url.clone(),
                        };
                        self.broadcast_via_p2p(&chunk_msg).await;
                    }

                    info!(
                        "DA broadcast complete: checkpoint {} — {} shard messages sent",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        shards.len(),
                    );
                    return;
                }
                Err(e) => {
                    warn!(
                        "DA encoding failed for checkpoint {}, falling back to inline: {}",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        e
                    );
                }
            }
        }

        let message = GossipMessage::CheckpointAnnouncement {
            checkpoint,
            sender_url: public_url,
            finalized_tx_hashes,
            precomputed_proofs: Vec::new(),
            finalized_transactions,
            da_commitment: None,
        };

        #[cfg(feature = "p2p")]
        self.broadcast_via_p2p(&message).await;
    }

    async fn broadcast_peer_list(&self) {
        let known_peers: Vec<String> = {
            let inner = self.inner.read().await;
            inner
                .peers
                .keys()
                .filter(|p| inner.peers.get(*p).map(|i| i.is_healthy).unwrap_or(false))
                .cloned()
                .collect()
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

        #[cfg(feature = "p2p")]
        self.broadcast_via_p2p(&message).await;

        debug!("Broadcast peer list via p2p");
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
        let (batch_txs, overflow_txs, peers): (
            Vec<SignedTransaction>,
            Vec<SignedTransaction>,
            Vec<String>,
        ) = {
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
            debug!(
                "Background propagation: {} transactions to {} peers ({} deferred to next cycle)",
                batch_count,
                peers.len(),
                overflow_count
            );
        } else {
            debug!(
                "Background propagation: {} transactions to {} peers",
                batch_count,
                peers.len()
            );
        }

        let public_url = std::env::var("PUBLIC_URL").ok();
        let mut propagated_count = 0u64;
        let mut tx_hashes: Vec<String> = Vec::with_capacity(batch_count);

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

        debug!(
            "Background propagation complete: {} transactions",
            propagated_count
        );
    }

    async fn request_sync_if_needed(&self, _local_checkpoint: u64) {}

    async fn refresh_peer_status_and_sync(&self) {}

    /// P2P-based delta sync using libp2p request-response protocol
    /// This replaces the HTTP-based sync for true P2P operation
    #[cfg(feature = "p2p")]
    async fn sync_from_peer_p2p(
        &self,
        peer_id: &str,
        local_checkpoint: u64,
    ) -> Result<DeltaSyncResult> {
        use crate::network::SyncResponse;

        let handle = match &self.network_handle {
            Some(h) => h.clone(),
            None => return Err(anyhow::anyhow!("P2P network not available")),
        };

        info!(
            "P2P delta sync from peer {} starting at checkpoint {}",
            peer_id, local_checkpoint
        );

        let mut result = DeltaSyncResult::default();
        let mut failed_count = 0usize;

        // Make P2P delta request
        let response = handle.request_delta(peer_id, local_checkpoint).await?;

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

                let mut applied_heights: std::collections::HashSet<u64> =
                    std::collections::HashSet::new();

                let proofs_by_height: std::collections::HashMap<u64, Vec<rinku_core::types::AccountStateProof>> = {
                    let mut map: std::collections::HashMap<u64, Vec<rinku_core::types::AccountStateProof>> = std::collections::HashMap::new();
                    for proof in &delta.precomputed_proofs {
                        map.entry(proof.checkpoint_height).or_default().push(proof.clone());
                    }
                    map
                };

                for cp_data in &delta.new_checkpoints {
                    {
                        let mut emission = self.state.emission.write().await;
                        let reward = emission.get_checkpoint_reward(cp_data.height);
                        if emission.record_emission_for_height(cp_data.height, reward) {
                            let mut rewards = self.state.rewards.write().await;
                            rewards.distribute_checkpoint_rewards(reward);
                        } else {
                            tracing::debug!(
                                "Skipping reward distribution for delta checkpoint {} — already distributed",
                                cp_data.height
                            );
                        }
                    }

                    let checkpoint = rinku_core::types::Checkpoint {
                        height: cp_data.height,
                        tx_merkle_root: cp_data.merkle_root.clone(),
                        state_root: cp_data.state_root.clone().unwrap_or_default(),
                        receipt_root: cp_data.receipt_root.clone().unwrap_or_default(),
                        timestamp: cp_data.timestamp,
                        previous_hash: cp_data.previous_hash.clone(),
                        tip_count: cp_data.tip_count.unwrap_or(cp_data.tx_count as u32),
                        hash: cp_data.hash.clone().unwrap_or_default(),
                        signer_bitmap: cp_data.signer_bitmap.clone(),
                        aggregated_signature: cp_data.signature.clone(),
                        validator_signatures: cp_data.validator_signatures.clone(),
                        finalized_tx_hashes: cp_data.finalized_tx_hashes.clone(),
                        weight_trie_root: String::new(),
                        provisional: false,
                        partition_epoch: None,
                        visible_stake_pct: None,
                        merge_report_hash: None,
                    };

                    if cp_data.finalized_tx_hashes.is_empty() {
                        if let Err(e) = self.state.apply_checkpoint(checkpoint).await {
                            warn!(
                                "Failed to apply synced checkpoint {}: {}",
                                cp_data.height, e
                            );
                            failed_count += 1;
                        } else {
                            applied_heights.insert(cp_data.height);
                            result.added += 1;
                        }
                    } else {
                        match self
                            .state
                            .apply_checkpoint_with_finalized_hashes(
                                checkpoint,
                                cp_data.finalized_tx_hashes.clone(),
                            )
                            .await
                        {
                            Err(e) => {
                                warn!("Failed to apply synced checkpoint {} with finalized hashes: {}", cp_data.height, e);
                                failed_count += 1;
                            }
                            Ok(missing_tx_count) => {
                                if missing_tx_count > 0 {
                                    warn!(
                                        "Delta sync checkpoint {}: {} txs missing locally (of {} finalized)",
                                        cp_data.height, missing_tx_count, cp_data.finalized_tx_hashes.len()
                                    );
                                }
                                {
                                    let mut inner = self.inner.write().await;
                                    for tx_hash in &cp_data.finalized_tx_hashes {
                                        inner.convergence_confirmed.remove(tx_hash);
                                        inner.convergence_pending.remove(tx_hash);
                                    }
                                }
                                applied_heights.insert(cp_data.height);
                                info!(
                                    "Applied delta sync checkpoint at height {} ({} txs finalized, {} missing)",
                                    cp_data.height, cp_data.finalized_tx_hashes.len(), missing_tx_count
                                );
                                result.added += 1;
                            }
                        }
                    }

                    if let Some(cp_proofs) = proofs_by_height.get(&cp_data.height) {
                        if applied_heights.contains(&cp_data.height) {
                            let proofs_map: std::collections::HashMap<
                                String,
                                rinku_core::types::AccountStateProof,
                            > = cp_proofs
                                .iter()
                                .map(|p| (p.address.clone(), p.clone()))
                                .collect();
                            info!(
                                "Delta sync: applying {} proofs for checkpoint {} (per-checkpoint state correction)",
                                proofs_map.len(), cp_data.height
                            );
                            self.state.store_precomputed_proofs(&proofs_map).await;
                        }
                    }
                }

                if !applied_heights.is_empty() {
                    self.state.purge_stale_deferred_txs().await;
                    self.state.purge_zombie_dag_txs().await;
                }

                // Merge validators
                if !delta.validators.is_empty() {
                    let validators: std::collections::HashMap<String, Validator> = delta
                        .validators
                        .iter()
                        .map(|v| {
                            let validator = Validator {
                                address: v.address.clone(),
                                stake: v.stake,
                                first_stake_time: 0,
                                bls_public_key: Some(v.bls_public_key.clone()),
                                missed_checkpoints: 0,
                            };
                            (v.address.clone(), validator)
                        })
                        .collect();
                    self.state.merge_validators_from_peer(&validators).await;
                    self.sync_validator_identity_from_state().await;
                }

                result.failed = failed_count;

                // ERROR HANDLING: If too many failures, return error to trigger retry
                if failed_count > 0 && failed_count > result.added {
                    warn!(
                        "P2P delta sync had more failures ({}) than successes ({})",
                        failed_count, result.added
                    );
                    return Err(anyhow::anyhow!(
                        "P2P sync partial failure: {} added, {} failed",
                        result.added,
                        failed_count
                    ));
                }

                info!(
                    "P2P delta sync complete: {} added, {} failed",
                    result.added, failed_count
                );
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

        let response = handle.request_snapshot(peer_id).await?;

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
            _ => Err(anyhow::anyhow!("Unexpected P2P response type for snapshot")),
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

        let local_checkpoint = self.state.get_checkpoint_height();
        let node_id = self.state.get_node_id().await;
        let validator_address = self
            .checkpoint_vote_signer
            .as_ref()
            .map(|s| s.validator_address.clone());

        let handshake = PeerHandshake {
            protocol_version: crate::versioning::PROTOCOL_VERSION.to_string(),
            chain_id: "rinku-testnet".to_string(),
            network_id: "testnet".to_string(),
            node_id,
            checkpoint_height: local_checkpoint,
            validator_address,
            capabilities: vec!["sync".to_string(), "gossip".to_string()],
            known_peer_addrs: Vec::new(),
        };

        let response = handle.handshake(peer_id, handshake).await?;

        match response {
            SyncResponse::Handshake(peer_info) => {
                Ok(PeerSyncStatus {
                    checkpoint_height: peer_info.checkpoint_height,
                    dag_size: 0, // Not available via handshake
                    tip_count: 0,
                    tips: Vec::new(),
                    merkle_root: None,
                    faucet_balance: 0,
                })
            }
            SyncResponse::Error { message } => {
                Err(anyhow::anyhow!("P2P handshake error: {}", message))
            }
            _ => Err(anyhow::anyhow!(
                "Unexpected P2P response type for handshake"
            )),
        }
    }

    /// Trigger P2P sync from a connected peer (100% P2P, no HTTP fallback)
    #[cfg(feature = "p2p")]
    async fn trigger_sync_from_peer_p2p(&self, peer_id: &str) -> anyhow::Result<()> {
        let local_checkpoint = self.state.get_checkpoint_height();

        // Get peer status via P2P handshake
        let status = self
            .fetch_peer_status_p2p(peer_id)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch peer status via P2P: {}", e))?;

        let checkpoint_gap = status.checkpoint_height.saturating_sub(local_checkpoint);

        if checkpoint_gap >= 2 {
            // Peer is significantly ahead - use snapshot sync
            info!(
                "P2P sync: peer {} is {} checkpoint(s) ahead, using snapshot sync",
                peer_id, checkpoint_gap
            );
            self.bootstrap_from_peer_p2p(peer_id).await.map_err(|e| {
                warn!("P2P snapshot sync from {} failed: {}", peer_id, e);
                e
            })?;
        } else {
            // Same or 1 checkpoint ahead - use delta sync
            info!(
                "P2P sync: peer {} at checkpoint {}, using delta sync",
                peer_id, status.checkpoint_height
            );
            self.sync_from_peer_p2p(peer_id, local_checkpoint)
                .await
                .map_err(|e| {
                    warn!("P2P delta sync from {} failed: {}", peer_id, e);
                    e
                })?;
        }

        Ok(())
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
                {
                    let inner = self.inner.read().await;
                    if let Some(&min_nonce) = inner.stale_nonce_cache.get(&tx.tx.from) {
                        if tx.tx.nonce < min_nonce {
                            inner
                                .stale_nonce_cache_hits
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            return Ok(None);
                        }
                    }
                    if inner.known_txs.contains(&hash) {
                        return Ok(None);
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
                    match self.state.add_transaction_from_gossip(tx.clone()).await {
                        Ok(TransactionResult::Accepted) => {}
                        Ok(TransactionResult::Buffered) => {
                            debug!(
                                "Gossiped tx {} buffered (future nonce), not propagating",
                                hash
                            );
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            if err_str.contains("Stale nonce") {
                                let expected_nonce = if let Some(account) =
                                    self.state.get_account(&tx.tx.from).await
                                {
                                    account.nonce
                                } else {
                                    0
                                };

                                let mut inner = self.inner.write().await;
                                let current = inner
                                    .stale_nonce_cache
                                    .get(&tx.tx.from)
                                    .copied()
                                    .unwrap_or(0);

                                if expected_nonce > current {
                                    let is_new_entry =
                                        !inner.stale_nonce_cache.contains_key(&tx.tx.from);
                                    inner
                                        .stale_nonce_cache
                                        .insert(tx.tx.from.clone(), expected_nonce);

                                    if is_new_entry {
                                        inner.stale_nonce_cache_order.push_back(tx.tx.from.clone());

                                        while inner.stale_nonce_cache.len() > STALE_NONCE_CACHE_MAX
                                        {
                                            if let Some(oldest_addr) =
                                                inner.stale_nonce_cache_order.pop_front()
                                            {
                                                inner.stale_nonce_cache.remove(&oldest_addr);
                                            } else {
                                                break;
                                            }
                                        }
                                    }
                                }
                                inner.stale_nonce_rejections += 1;
                            } else if !err_str.contains("Duplicate") {
                                warn!("Failed to add gossiped tx {}: {}", hash, e);
                                let mut inner = self.inner.write().await;
                                inner.known_txs.remove(&hash);
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
                                    debug!(
                                        "TipAnnouncement: peer {} synced recently, skipping",
                                        peer_url
                                    );
                                    return Ok(None);
                                }
                            }
                            // Record this sync attempt for the peer
                            inner
                                .peer_last_sync
                                .insert(peer_url.clone(), std::time::Instant::now());
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
                                    let sync_result = gossip_clone
                                        .trigger_sync_from_peer_p2p(&peer_url_clone)
                                        .await;

                                    // Update sync state based on result
                                    let mut inner = gossip_clone.inner.write().await;
                                    inner.sync_in_flight = false;

                                    // If sync failed, set cooldown to prevent sync storm
                                    if sync_result.is_err() {
                                        inner.last_sync_failure = Some(std::time::Instant::now());
                                        debug!(
                                            "Sync from {} failed, entering cooldown",
                                            peer_url_clone
                                        );
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
                    debug!(
                        "Received tip announcement, peer dag_size: {} (local: {})",
                        dag_size, local_dag_size
                    );
                }
                Ok(None)
            }

            GossipMessage::PeerDiscovery {
                peers,
                node_id,
                sender_url,
                ..
            } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();

                let own_url = std::env::var("PUBLIC_URL").ok();
                let own_normalized = own_url
                    .as_ref()
                    .map(|u| u.trim_end_matches('/').to_string());

                let mut inner = self.inner.write().await;
                for addr in &peers {
                    let addr_normalized = addr.trim_end_matches('/');
                    if own_normalized.as_deref() == Some(addr_normalized) {
                        continue;
                    }

                    if !inner.peers.contains_key(addr) {
                        inner.peers.insert(
                            addr.clone(),
                            PeerInfo {
                                address: addr.clone(),
                                node_id: node_id.clone(),
                                last_seen: now,
                                dag_size: 0,
                                checkpoint_height: 0,
                                latency_ms: 0,
                                is_healthy: true,
                                consecutive_failures: 0,
                                backoff_until: 0,
                            },
                        );
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
                let txs = self
                    .state
                    .get_txs_since_checkpoint(from_checkpoint, &missing_hashes)
                    .await;
                let checkpoint_height = self.state.get_checkpoint_height();

                Ok(Some(GossipMessage::SyncResponse {
                    transactions: txs,
                    checkpoint_height,
                    sender_url: std::env::var("PUBLIC_URL").ok(),
                }))
            }

            GossipMessage::SyncResponse { transactions, .. } => {
                for tx in transactions {
                    let hash = tx.hash.clone();
                    let mut inner = self.inner.write().await;
                    if !inner.known_txs.contains(&hash) {
                        inner.known_txs.insert(hash.clone());
                        drop(inner);
                        match self.state.add_transaction_from_gossip(tx).await {
                            Ok(TransactionResult::Accepted) => {}
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

            GossipMessage::CheckpointSignature { .. } => Ok(None),

            GossipMessage::BloomAnnouncement {
                filter,
                checkpoint_height,
                tip_count,
                sender_url,
            } => {
                debug!(
                    "Received bloom filter from peer with {} items, checkpoint height {}, {} tips",
                    filter.item_count(),
                    checkpoint_height,
                    tip_count
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

            GossipMessage::WeightVote {
                tx_hash,
                validator_pubkey,
                vote,
                timestamp_ms,
                bls_signature,
                ..
            } => {
                use rinku_core::types::{PendingWeightVote, WeightVote as WV};

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
                    debug!(
                        "Applied gossiped weight vote for tx {} from {}",
                        &tx_hash[..16.min(tx_hash.len())],
                        &validator_pubkey[..16.min(validator_pubkey.len())]
                    );
                }

                Ok(None)
            }

            GossipMessage::CheckpointAnnouncement {
                checkpoint,
                finalized_tx_hashes,
                precomputed_proofs,
                finalized_transactions,
                da_commitment,
                ..
            } => {
                if let Some(commitment) = da_commitment {
                    if finalized_transactions.is_empty() {
                        const MAX_TOTAL_SHARDS: usize = 100;
                        const MAX_ORIGINAL_SIZE: usize = 50 * 1024 * 1024;
                        let total = commitment.data_shard_count + commitment.parity_shard_count;
                        if commitment.data_shard_count == 0
                            || commitment.parity_shard_count == 0
                            || total > MAX_TOTAL_SHARDS
                            || commitment.original_size > MAX_ORIGINAL_SIZE
                        {
                            warn!(
                                "Rejecting DA commitment with invalid bounds: data={}, parity={}, original_size={}",
                                commitment.data_shard_count, commitment.parity_shard_count, commitment.original_size
                            );
                            return Ok(None);
                        }
                        let height = checkpoint.height;
                        let hash_prefix =
                            checkpoint.hash[..16.min(checkpoint.hash.len())].to_string();
                        info!(
                            "DA checkpoint {} at height {}: awaiting {}/{} shards for reconstruction ({}KB original)",
                            hash_prefix, height,
                            commitment.data_shard_count, total,
                            commitment.original_size / 1024,
                        );

                        let ready_for_reconstruct = {
                            let mut inner = self.inner.write().await;
                            let local_height = self.state.get_checkpoint_height();
                            if height <= local_height {
                                debug!(
                                    "Ignoring DA checkpoint at height {} (local: {})",
                                    height, local_height
                                );
                                return Ok(None);
                            }
                            if inner.da_collector.contains_key(&height) {
                                debug!("DA collector already exists for height {}, keeping existing shards", height);
                                false
                            } else {
                                const DA_COLLECTOR_MAX: usize = 10;
                                if inner.da_collector.len() >= DA_COLLECTOR_MAX {
                                    let oldest = inner.da_collector.keys().min().copied();
                                    if let Some(old_h) = oldest {
                                        inner.da_collector.remove(&old_h);
                                    }
                                }
                                let mut state = DaReconstructionState {
                                    checkpoint,
                                    finalized_tx_hashes,
                                    precomputed_proofs,
                                    commitment: commitment.clone(),
                                    shards: vec![None; total],
                                    received_count: 0,
                                    created_at: std::time::Instant::now(),
                                    source: "gossip-da".to_string(),
                                    decode_attempts: 0,
                                };

                                if let Some(early) = inner.da_early_shards.remove(&height) {
                                    for (cp_hash, idx, data, proof) in early {
                                        if cp_hash == state.checkpoint.hash
                                            && idx < total
                                            && state.shards[idx].is_none()
                                        {
                                            state.shards[idx] = Some(crate::erasure::DaShard {
                                                index: idx,
                                                data,
                                                merkle_proof: proof,
                                            });
                                            state.received_count += 1;
                                        }
                                    }
                                    if state.received_count > 0 {
                                        info!(
                                            "DA collector for height {}: merged {} early shards",
                                            height, state.received_count
                                        );
                                    }
                                }

                                let ready =
                                    state.received_count >= state.commitment.data_shard_count;
                                inner.da_collector.insert(height, state);
                                ready
                            }
                        };

                        if ready_for_reconstruct {
                            self.try_da_reconstruction(height).await;
                        }
                        return Ok(None);
                    }
                }
                self.apply_received_checkpoint(
                    checkpoint,
                    finalized_tx_hashes,
                    finalized_transactions,
                    precomputed_proofs,
                    "gossip",
                )
                .await;
                Ok(None)
            }

            GossipMessage::TxConfirmBroadcast {
                tx,
                sender_validator,
                sender_stake,
                timestamp_ms,
                ..
            } => {
                {
                    let inner = self.inner.read().await;
                    if inner.convergence_executed.contains(&tx.hash) {
                        return Ok(None);
                    }
                    if inner.known_txs.contains(&tx.hash) {
                        drop(inner);
                        if let Some(vote_tx) = self.convergence_vote_tx.get() {
                            if let Err(e) = vote_tx.try_send(ConvergenceVoteOp::BroadcastVote {
                                tx,
                                sender_validator,
                                sender_stake,
                                timestamp_ms,
                                send_ack: true,
                            }) {
                                warn!(
                                    "Convergence vote channel full (fast-path), dropped vote: {}",
                                    e
                                );
                            }
                        }
                        return Ok(None);
                    }
                }

                let is_new = {
                    let mut inner = self.inner.write().await;
                    if inner.convergence_executed.contains(&tx.hash) {
                        return Ok(None);
                    }
                    if inner.known_txs.contains(&tx.hash) {
                        false
                    } else {
                        inner.known_txs.insert(tx.hash.clone());
                        inner.stats.txs_received += 1;
                        true
                    }
                };

                if is_new {
                    let is_consolidation_tx = matches!(
                        tx.tx.kind,
                        Some(rinku_core::types::TransactionKind::Consolidation)
                    );
                    if is_consolidation_tx {
                        let _ = self.state.add_transaction_from_gossip(tx).await;
                        return Ok(None);
                    }

                    let already_in_dag = self.state.has_transaction(&tx.hash).await;

                    if !already_in_dag {
                        match self.state.add_transaction_from_gossip(tx.clone()).await {
                            Ok(_) => {}
                            Err(e) => {
                                let err_str = e.to_string();
                                if !err_str.contains("Stale nonce")
                                    && !err_str.contains("Duplicate")
                                {
                                    warn!(
                                        "Failed to add fast-path tx {}: {}",
                                        &tx.hash[..16.min(tx.hash.len())],
                                        e
                                    );
                                    let mut inner = self.inner.write().await;
                                    inner.known_txs.remove(&tx.hash);
                                }
                            }
                        }
                    }

                    if let Some(vote_tx) = self.convergence_vote_tx.get() {
                        if let Err(e) = vote_tx.try_send(ConvergenceVoteOp::BroadcastVote {
                            tx,
                            sender_validator,
                            sender_stake,
                            timestamp_ms,
                            send_ack: true,
                        }) {
                            warn!(
                                "Convergence vote channel full, dropped BroadcastVote: {}",
                                e
                            );
                        }
                    }
                } else {
                    if let Some(vote_tx) = self.convergence_vote_tx.get() {
                        if let Err(e) = vote_tx.try_send(ConvergenceVoteOp::BroadcastVote {
                            tx,
                            sender_validator,
                            sender_stake,
                            timestamp_ms,
                            send_ack: true,
                        }) {
                            warn!(
                                "Convergence vote channel full, dropped BroadcastVote: {}",
                                e
                            );
                        }
                    }
                }

                Ok(None)
            }

            GossipMessage::TxConfirmAck {
                tx_hash,
                validator_address,
                validator_stake,
                timestamp_ms,
                ..
            } => {
                {
                    let inner = self.inner.read().await;
                    if inner.convergence_executed.contains(&tx_hash) {
                        return Ok(None);
                    }
                }

                if let Some(vote_tx) = self.convergence_vote_tx.get() {
                    if let Err(e) = vote_tx.try_send(ConvergenceVoteOp::AckVote {
                        tx_hash,
                        validator_address,
                        validator_stake,
                        timestamp_ms,
                    }) {
                        warn!("Convergence vote channel full, dropped AckVote: {}", e);
                    }
                }

                Ok(None)
            }

            GossipMessage::MergePayload { request, .. } => {
                info!(
                    "Gossip: received merge payload from partition epoch {}, {} remote txs",
                    request.partition_epoch,
                    request.transactions.len()
                );

                let mut orchestrator =
                    crate::merge::orchestrator::MergeOrchestrator::new(self.state.clone());
                if let Some(ref eb) = self.event_bus {
                    orchestrator = orchestrator.with_event_bus(eb.clone());
                }
                match orchestrator.execute_merge(request).await {
                    Ok(report) => {
                        self.state.set_latest_merge_report(report.clone()).await;

                        if let Some(ref eb) = self.event_bus {
                            eb.publish(crate::events::NodeEvent::MergeCompleted {
                                epoch: report.merge_epoch,
                                direct_conflicts: report.direct_conflicts.len(),
                                economic_conflicts: report.economic_conflicts.len(),
                                transactions_kept: report.transactions_kept.len(),
                                transactions_rejected: report.transactions_rejected.len(),
                                duration_ms: report
                                    .completed_at_ms
                                    .unwrap_or(0)
                                    .saturating_sub(report.started_at_ms),
                            });
                        }

                        info!(
                            "Gossip: merge complete — {} kept, {} rejected, {} conflicts",
                            report.transactions_kept.len(),
                            report.transactions_rejected.len(),
                            report.direct_conflicts.len() + report.economic_conflicts.len(),
                        );

                        let public_url = std::env::var("PUBLIC_URL").ok();
                        Ok(Some(GossipMessage::MergeResult {
                            report,
                            sender_url: public_url,
                        }))
                    }
                    Err(e) => {
                        warn!("Gossip: merge execution failed: {}", e);
                        Ok(None)
                    }
                }
            }

            GossipMessage::MergeResult { report, .. } => {
                info!(
                    "Gossip: received merge result for epoch {} — {} kept, {} rejected",
                    report.merge_epoch,
                    report.transactions_kept.len(),
                    report.transactions_rejected.len()
                );
                self.state.set_latest_merge_report(report).await;
                Ok(None)
            }

            GossipMessage::CheckpointIntent {
                height,
                leader_address,
                timestamp_ms,
                relay_count,
                ..
            } => {
                let current_height = self.state.get_checkpoint_height();
                let target_height = current_height + 1;
                if height >= target_height && height <= target_height + 5 {
                    let mut inner = self.inner.write().await;
                    let is_new = !inner.leader_intents.contains_key(&height);
                    inner
                        .leader_intents
                        .insert(height, (leader_address.clone(), timestamp_ms));
                    let intent_count = inner.leader_intents.len();
                    if intent_count > 10 {
                        let min_relevant = target_height.saturating_sub(1);
                        inner.leader_intents.retain(|h, _| *h >= min_relevant);
                    }
                    info!(
                        "Received checkpoint intent for height {} from leader {} (relay={})",
                        height,
                        &leader_address[..16.min(leader_address.len())],
                        relay_count
                    );
                    if is_new && relay_count == 0 {
                        let relay_msg = GossipMessage::CheckpointIntent {
                            height,
                            leader_address,
                            timestamp_ms,
                            sender_url: std::env::var("PUBLIC_URL").ok(),
                            relay_count: 1,
                        };
                        drop(inner);
                        info!("Relaying checkpoint intent for height {}", height);
                        #[cfg(feature = "p2p")]
                        self.broadcast_via_p2p(&relay_msg).await;
                        return Ok(None);
                    }
                } else {
                    debug!(
                        "Ignored checkpoint intent for height {} (current={}, target={}, relay={})",
                        height, current_height, target_height, relay_count
                    );
                }
                Ok(None)
            }

            GossipMessage::LeaderTimeout {
                height,
                validator_address,
                validator_stake: _,
                timestamp_ms: _,
                ..
            } => {
                let current_height = self.state.get_checkpoint_height();
                let target_height = current_height + 1;
                if height != target_height {
                    debug!(
                        "Ignored leader timeout for height {} (target={})",
                        height, target_height
                    );
                    return Ok(None);
                }

                let (validators, our_addr, our_stake, total_stake, _quorum) =
                    self.get_cached_validators().await;
                if !validators.contains_key(&validator_address) {
                    warn!(
                        "Rejected leader timeout from non-validator {} for height {}",
                        &validator_address[..16.min(validator_address.len())],
                        height
                    );
                    return Ok(None);
                }

                let canonical_stake = validators
                    .get(&validator_address)
                    .map(|v| v.stake)
                    .unwrap_or(0);

                let (is_new, should_echo, accumulated, has_quorum) = {
                    let mut inner = self.inner.write().await;
                    let already_sent = inner.leader_timeout_sent.contains(&height);
                    let votes = inner.leader_timeout_votes.entry(height).or_default();
                    let is_new = !votes.contains_key(&validator_address);
                    votes.insert(validator_address.clone(), canonical_stake);

                    let should_echo =
                        is_new && !already_sent && our_addr.is_some() && our_stake > 0;

                    if should_echo {
                        if let Some(ref addr) = our_addr {
                            votes.insert(addr.clone(), our_stake);
                        }
                    }

                    let accumulated: u64 = votes.values().sum();
                    let quorum_threshold = (total_stake * 2 + 2) / 3;
                    let has_quorum = accumulated >= quorum_threshold;

                    if should_echo {
                        inner.leader_timeout_sent.insert(height);
                    }

                    (is_new, should_echo, accumulated, has_quorum)
                };

                if is_new {
                    info!(
                        "Leader timeout vote for height {} from {} (stake={}, accumulated={}/{}, quorum={})",
                        height,
                        &validator_address[..16.min(validator_address.len())],
                        canonical_stake,
                        accumulated,
                        total_stake,
                        if has_quorum { "REACHED" } else { "pending" }
                    );
                }

                if should_echo {
                    if let Some(ref addr) = our_addr {
                        let echo_msg = GossipMessage::LeaderTimeout {
                            height,
                            validator_address: addr.clone(),
                            validator_stake: our_stake,
                            timestamp_ms: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                            sender_url: std::env::var("PUBLIC_URL").ok(),
                        };
                        info!(
                            "Echoing leader timeout vote for height {} (our stake={})",
                            height, our_stake
                        );
                        #[cfg(feature = "p2p")]
                        self.broadcast_via_p2p(&echo_msg).await;
                        return Ok(None);
                    }
                }

                Ok(None)
            }

            GossipMessage::QccVoteRequest {
                height,
                checkpoint_hash,
                tx_merkle_root,
                state_root: _,
                proposer_address,
                finalized_tx_hashes,
                timestamp_ms: _,
                ..
            } => {
                self.record_peer_height(height.saturating_sub(1));

                let local_cp_height = self.state.get_checkpoint_height();
                if height <= local_cp_height {
                    debug!(
                        "QCC-GOSSIP: Ignoring vote request for height {} (already at {})",
                        height, local_cp_height
                    );
                    return Ok(None);
                }
                if height > local_cp_height + 3 {
                    debug!(
                        "QCC-GOSSIP: Ignoring vote request for height {} (too far ahead, local={})",
                        height, local_cp_height
                    );
                    return Ok(None);
                }

                let signer = match self.checkpoint_vote_signer.as_ref() {
                    Some(s) if !s.validator_address.is_empty() => s,
                    _ => return Ok(None),
                };

                if signer.validator_address == proposer_address {
                    return Ok(None);
                }

                let tx_hashes_sorted = if !finalized_tx_hashes.is_empty() {
                    let mut sorted = finalized_tx_hashes.clone();
                    sorted.sort();
                    sorted
                } else {
                    vec![]
                };
                let expected_merkle = tx_merkle_root.clone();
                let checkpoint_hash_hex = checkpoint_hash.clone();
                let bls_key = signer.bls_private_key.clone();
                let vote_height = height;

                let vote_compute =
                    tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
                        if !tx_hashes_sorted.is_empty() {
                            let computed_root =
                                rinku_core::MerkleTree::from_hex_leaves(&tx_hashes_sorted)
                                    .map(|t| t.root())
                                    .unwrap_or_else(|_| "0".repeat(64));
                            if computed_root != expected_merkle {
                                return Err(format!(
                                    "merkle_mismatch:{}:{}",
                                    &computed_root[..16.min(computed_root.len())],
                                    &expected_merkle[..16.min(expected_merkle.len())]
                                ));
                            }
                        }
                        let hash_bytes = hex::decode(&checkpoint_hash_hex)
                            .map_err(|e| format!("hex_decode:{}", e))?;
                        let signature = crate::bls::bls_sign(&hash_bytes, &bls_key)
                            .map_err(|e| format!("bls_sign:{}", e))?;
                        Ok(signature)
                    })
                    .await;

                match vote_compute {
                    Ok(Ok(signature)) => {
                        let final_height = self.state.get_checkpoint_height();
                        if vote_height <= final_height {
                            debug!(
                                "QCC-GOSSIP: Declining vote — height {} committed during signing (now {})",
                                vote_height, final_height
                            );
                            return Ok(None);
                        }
                        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
                        use base64::Engine;
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let vote_msg = GossipMessage::QccVoteCast {
                            height: vote_height,
                            checkpoint_hash,
                            validator_address: signer.validator_address.clone(),
                            bls_public_key: URL_SAFE_NO_PAD.encode(&signer.bls_public_key),
                            signature: URL_SAFE_NO_PAD.encode(&signature),
                            stake: GENESIS_VALIDATOR_STAKE,
                            timestamp_ms: now_ms,
                            sender_url: std::env::var("PUBLIC_URL").ok(),
                        };
                        info!(
                            "QCC-GOSSIP: Broadcasting vote for height {} (validator: {})",
                            vote_height,
                            &signer.validator_address[..16.min(signer.validator_address.len())]
                        );
                        #[cfg(feature = "p2p")]
                        self.broadcast_via_p2p(&vote_msg).await;
                    }
                    Ok(Err(err_msg)) => {
                        warn!(
                            "QCC-GOSSIP: Vote computation failed for height {}: {}",
                            vote_height, err_msg
                        );
                    }
                    Err(e) => {
                        warn!(
                            "QCC-GOSSIP: spawn_blocking failed for height {}: {}",
                            vote_height, e
                        );
                    }
                }

                Ok(None)
            }

            GossipMessage::QccVoteCast {
                height,
                checkpoint_hash,
                validator_address,
                bls_public_key,
                signature,
                stake,
                ..
            } => {
                let guard = self.qcc_vote_tx.lock().await;
                if let Some(ref tx) = *guard {
                    let vote = QccGossipVote {
                        validator_address,
                        bls_public_key,
                        signature,
                        stake,
                        height,
                        checkpoint_hash,
                    };
                    if let Err(e) = tx.try_send(vote) {
                        debug!("QCC-GOSSIP: Vote channel full or closed: {}", e);
                    }
                }
                Ok(None)
            }

            GossipMessage::DaChunk {
                checkpoint_height,
                checkpoint_hash,
                shard_index,
                shard_data,
                merkle_proof,
                ..
            } => {
                let should_try_reconstruct = {
                    let mut inner = self.inner.write().await;
                    let local_height = self.state.get_checkpoint_height();
                    if checkpoint_height <= local_height {
                        debug!(
                            "DA shard {} for height {} — already applied (local: {})",
                            shard_index, checkpoint_height, local_height
                        );
                        return Ok(None);
                    }
                    if let Some(state) = inner.da_collector.get_mut(&checkpoint_height) {
                        if state.checkpoint.hash != checkpoint_hash {
                            warn!(
                                "DA shard {} for height {} has mismatched hash (expected {}, got {})",
                                shard_index, checkpoint_height,
                                &state.checkpoint.hash[..16.min(state.checkpoint.hash.len())],
                                &checkpoint_hash[..16.min(checkpoint_hash.len())],
                            );
                            false
                        } else {
                            let total = state.commitment.data_shard_count
                                + state.commitment.parity_shard_count;
                            if shard_index >= total {
                                warn!(
                                    "DA shard index {} out of range for height {} (total {})",
                                    shard_index, checkpoint_height, total
                                );
                                false
                            } else if state.shards[shard_index].is_some() {
                                debug!(
                                    "DA shard {} for height {} already received, skipping",
                                    shard_index, checkpoint_height
                                );
                                false
                            } else {
                                state.shards[shard_index] = Some(crate::erasure::DaShard {
                                    index: shard_index,
                                    data: shard_data,
                                    merkle_proof,
                                });
                                state.received_count += 1;
                                debug!(
                                    "DA shard {}/{} received for checkpoint {} at height {} ({}/{})",
                                    shard_index, total,
                                    &checkpoint_hash[..16.min(checkpoint_hash.len())],
                                    checkpoint_height,
                                    state.received_count, state.commitment.data_shard_count,
                                );
                                state.received_count >= state.commitment.data_shard_count
                            }
                        }
                    } else {
                        const DA_EARLY_SHARD_MAX_PER_HEIGHT: usize = 30;
                        const DA_EARLY_SHARD_MAX_HEIGHTS: usize = 20;
                        if inner.da_early_shards.len() >= DA_EARLY_SHARD_MAX_HEIGHTS
                            && !inner.da_early_shards.contains_key(&checkpoint_height)
                        {
                            debug!("DA early shard buffer full ({} heights), dropping shard for height {}", DA_EARLY_SHARD_MAX_HEIGHTS, checkpoint_height);
                        } else {
                            let early = inner
                                .da_early_shards
                                .entry(checkpoint_height)
                                .or_insert_with(Vec::new);
                            if early.len() < DA_EARLY_SHARD_MAX_PER_HEIGHT {
                                debug!(
                                    "DA shard {} for height {} buffered (announcement not yet received)",
                                    shard_index, checkpoint_height,
                                );
                                early.push((
                                    checkpoint_hash,
                                    shard_index,
                                    shard_data,
                                    merkle_proof,
                                ));
                            }
                        }
                        false
                    }
                };

                if should_try_reconstruct {
                    self.try_da_reconstruction(checkpoint_height).await;
                }

                Ok(None)
            }

            GossipMessage::Batch { .. } => {
                warn!("Received Batch message in handle_message - batches should be unpacked at network layer");
                Ok(None)
            }
        }
    }

    async fn try_da_reconstruction(&self, height: u64) {
        const MAX_DECODE_ATTEMPTS: u8 = 3;

        let da_state = {
            let mut inner = self.inner.write().await;
            let state = match inner.da_collector.get_mut(&height) {
                Some(s) => s,
                None => return,
            };

            if state.decode_attempts >= MAX_DECODE_ATTEMPTS {
                let hash_prefix =
                    state.checkpoint.hash[..16.min(state.checkpoint.hash.len())].to_string();
                warn!(
                    "DA reconstruction for checkpoint {} at height {} exhausted {} decode attempts — removing collector and requesting delta sync",
                    hash_prefix, height, MAX_DECODE_ATTEMPTS,
                );
                inner.da_collector.remove(&height);
                return;
            }

            state.decode_attempts += 1;

            let shard_opts: Vec<Option<crate::erasure::DaShard>> = state.shards.clone();
            let commitment = state.commitment.clone();
            let cp = state.checkpoint.clone();
            let tx_hashes = state.finalized_tx_hashes.clone();
            let proofs = state.precomputed_proofs.clone();
            let source = state.source.clone();
            let attempt = state.decode_attempts;

            (
                shard_opts, commitment, cp, tx_hashes, proofs, source, attempt,
            )
        };

        let (shard_opts, commitment, checkpoint, tx_hashes, proofs, source, attempt) = da_state;
        let hash_prefix = &checkpoint.hash[..16.min(checkpoint.hash.len())];

        match crate::erasure::decode(&commitment, &shard_opts) {
            Ok(reconstructed) => {
                let payload = if commitment.compressed && commitment.uncompressed_size > 0 {
                    match crate::erasure::decompress_da_payload(
                        &reconstructed,
                        commitment.uncompressed_size,
                    ) {
                        Ok(decompressed) => {
                            info!(
                                "DA decompression for checkpoint {} at height {}: {}KB → {}KB",
                                hash_prefix,
                                height,
                                reconstructed.len() / 1024,
                                decompressed.len() / 1024,
                            );
                            decompressed
                        }
                        Err(e) => {
                            warn!(
                                "DA decompression failed for checkpoint {} at height {} (attempt {}): {}. Trying raw.",
                                hash_prefix, height, attempt, e
                            );
                            reconstructed
                        }
                    }
                } else {
                    reconstructed
                };
                match serde_json::from_slice::<Vec<SignedTransaction>>(&payload) {
                    Ok(finalized_transactions) => {
                        info!(
                            "DA reconstruction succeeded for checkpoint {} at height {}: {} tx bodies ({}KB, attempt {})",
                            hash_prefix, height,
                            finalized_transactions.len(),
                            payload.len() / 1024,
                            attempt,
                        );
                        {
                            let mut inner = self.inner.write().await;
                            inner.da_collector.remove(&height);
                        }
                        self.apply_received_checkpoint(
                            checkpoint,
                            tx_hashes,
                            finalized_transactions,
                            proofs,
                            &source,
                        )
                        .await;
                    }
                    Err(e) => {
                        warn!(
                            "DA reconstruction for checkpoint {} at height {} — deserialization failed (attempt {}): {}. Collector retained for additional shards.",
                            hash_prefix, height, attempt, e
                        );
                    }
                }
            }
            Err(e) => {
                warn!(
                    "DA reconstruction failed for checkpoint {} at height {} (attempt {}): {}. Collector retained for additional shards.",
                    hash_prefix, height, attempt, e
                );
            }
        }
    }

    async fn cleanup_stale_da_collectors(&self) {
        const DA_TIMEOUT_SECS: u64 = 30;
        let local_height = self.state.get_checkpoint_height();
        let mut timed_out_heights: Vec<u64> = Vec::new();
        {
            let mut inner = self.inner.write().await;
            let before = inner.da_collector.len();
            inner.da_collector.retain(|height, state| {
                if *height <= local_height {
                    return false;
                }
                if state.created_at.elapsed().as_secs() > DA_TIMEOUT_SECS {
                    warn!(
                        "DA collector for checkpoint {} at height {} timed out after {}s ({}/{} shards received, {} decode attempts)",
                        &state.checkpoint.hash[..16.min(state.checkpoint.hash.len())],
                        height,
                        state.created_at.elapsed().as_secs(),
                        state.received_count,
                        state.commitment.data_shard_count + state.commitment.parity_shard_count,
                        state.decode_attempts,
                    );
                    timed_out_heights.push(*height);
                    return false;
                }
                true
            });
            let removed = before - inner.da_collector.len();
            if removed > 0 {
                debug!("Cleaned up {} stale DA collector entries", removed);
            }

            inner
                .da_early_shards
                .retain(|height, _| *height > local_height);
        }

        if !timed_out_heights.is_empty() {
            debug!(
                "DA timeout fallback: {} heights need delta sync: {:?}",
                timed_out_heights.len(),
                timed_out_heights
            );
        }
    }

    pub async fn send_merge_payload(&self, fork_point_checkpoint_height: u64) {
        let orchestrator = crate::merge::orchestrator::MergeOrchestrator::new(self.state.clone());
        let payload = match orchestrator
            .prepare_merge_payload(fork_point_checkpoint_height)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to prepare merge payload: {}", e);
                return;
            }
        };

        if let Some(ref eb) = self.event_bus {
            eb.publish(crate::events::NodeEvent::MergeStarted {
                epoch: payload.partition_epoch,
                fork_point_checkpoint: fork_point_checkpoint_height,
                remote_tx_count: 0,
            });
        }

        let public_url = std::env::var("PUBLIC_URL").ok();
        let message = GossipMessage::MergePayload {
            request: payload,
            sender_url: public_url,
        };

        info!("Broadcasting merge payload via p2p");
        #[cfg(feature = "p2p")]
        self.broadcast_via_p2p(&message).await;
    }

    pub async fn get_healthy_peer_addresses(&self) -> Vec<String> {
        let inner = self.inner.read().await;
        inner
            .peers
            .iter()
            .filter(|(_, info)| info.is_healthy)
            .map(|(_, info)| info.address.clone())
            .collect()
    }

    pub async fn broadcast_transaction(&self, tx: SignedTransaction) {
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
            let message = GossipMessage::Transaction {
                hash: tx_hash.clone(),
                tx,
                sender_url: public_url,
            };
            #[cfg(feature = "p2p")]
            self.broadcast_via_p2p(&message).await;
            debug!(
                "Broadcast tx {} via P2P immediately",
                &tx_hash[..16.min(tx_hash.len())]
            );
        }
    }

    pub async fn get_convergence_status(
        &self,
        tx_hash: &str,
    ) -> Option<rinku_core::types::FastPathFinality> {
        let inner = self.inner.read().await;
        if let Some(finality) = inner.convergence_confirmed.get(tx_hash) {
            return Some(finality.clone());
        }
        if let Some(finality) = inner.convergence_pending.get(tx_hash) {
            return Some(finality.clone());
        }
        None
    }

    pub async fn is_convergence_executed(&self, tx_hash: &str) -> bool {
        let inner = self.inner.read().await;
        inner.convergence_executed.contains(tx_hash)
    }

    pub async fn get_all_convergence_executed(&self) -> std::collections::HashSet<String> {
        let inner = self.inner.read().await;
        inner.convergence_executed.clone()
    }


    pub async fn broadcast_convergence_transaction(
        &self,
        tx: SignedTransaction,
        validator_address: &str,
        validator_stake: u64,
    ) -> bool {
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

            if validator_stake > 0 {
                let validators = self.state.get_validators_map().await;
                let total_stake: u64 = validators.values().map(|v| v.stake).sum();
                let quorum_threshold = total_stake * 2 / 3;

                let mut inner = self.inner.write().await;
                let finality = inner
                    .convergence_pending
                    .entry(tx_hash.clone())
                    .or_insert_with(|| rinku_core::types::FastPathFinality {
                        tx_hash: tx_hash.clone(),
                        status: rinku_core::types::FastPathStatus::Pending,
                        acks: Vec::new(),
                        total_stake_acked: 0,
                        quorum_stake_required: quorum_threshold,
                        registered_at_ms: timestamp_ms,
                        confirmed_at_ms: None,
                        checkpoint_height: None,
                        tx_created_at_ms: Some(tx.tx.timestamp),
                    });
                let already_acked = finality
                    .acks
                    .iter()
                    .any(|a| a.validator_address == validator_address);
                if !already_acked {
                    finality.acks.push(rinku_core::types::FastPathAck {
                        tx_hash: tx_hash.clone(),
                        validator_address: validator_address.to_string(),
                        validator_stake,
                        bls_signature: None,
                        timestamp_ms,
                    });
                    finality.total_stake_acked += validator_stake;
                }
                drop(inner);
            }

            let message = GossipMessage::TxConfirmBroadcast {
                tx,
                sender_validator: validator_address.to_string(),
                sender_stake: validator_stake,
                timestamp_ms,
                sender_url: public_url,
            };

            #[cfg(feature = "p2p")]
            self.broadcast_via_p2p(&message).await;

            info!(
                "Broadcast convergence tx {} via P2P (validator: {}, stake: {})",
                &tx_hash[..16.min(tx_hash.len())],
                &validator_address[..16.min(validator_address.len())],
                validator_stake
            );

            true
        } else {
            false
        }
    }

    async fn execute_convergence_batch(&self) {
        const BATCH_INTERVAL_MS: u128 = 500;

        let should_run = {
            let inner = self.inner.read().await;
            inner.last_batch_execution.elapsed().as_millis() >= BATCH_INTERVAL_MS
        };
        if !should_run {
            return;
        }

        let pending: Vec<SignedTransaction> = {
            let mut inner = self.inner.write().await;
            inner.last_batch_execution = std::time::Instant::now();
            std::mem::take(&mut inner.confirmed_pending_execution)
        };

        if pending.is_empty() {
            return;
        }

        let already_executed: std::collections::HashSet<String> = {
            let inner = self.inner.read().await;
            inner.convergence_executed.clone()
        };

        let mut txs: Vec<SignedTransaction> = pending
            .into_iter()
            .filter(|tx| !already_executed.contains(&tx.hash))
            .collect();

        txs.sort_by(|a, b| {
            a.tx.from
                .cmp(&b.tx.from)
                .then(a.tx.nonce.cmp(&b.tx.nonce))
                .then(a.hash.cmp(&b.hash))
        });

        let total = txs.len();

        let results = self.state.execute_confirmed_batch(&txs).await;

        let mut executed_count = 0u32;
        let mut dropped_count = 0u32;
        let mut deferred_hashes: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        {
            let mut inner = self.inner.write().await;
            for entry in &results {
                match entry.result {
                    crate::state::ConvergenceExecResult::Executed => {
                        inner.convergence_executed.insert(entry.hash.clone());
                        if let Some(finality) = inner.convergence_confirmed.get_mut(&entry.hash) {
                            finality.status = rinku_core::types::FastPathStatus::Executed;
                        }
                        executed_count += 1;
                    }
                    crate::state::ConvergenceExecResult::AlreadyApplied => {
                        inner.convergence_executed.insert(entry.hash.clone());
                    }
                    crate::state::ConvergenceExecResult::Deferred => {
                        deferred_hashes.insert(entry.hash.clone());
                        dropped_count += 1;
                    }
                    crate::state::ConvergenceExecResult::Rejected => {
                        inner.convergence_executed.insert(entry.hash.clone());
                        if let Some(finality) = inner.convergence_confirmed.get_mut(&entry.hash) {
                            finality.status = rinku_core::types::FastPathStatus::Finalized;
                        }
                    }
                }
            }

            if !deferred_hashes.is_empty() && executed_count > 0 {
                let local_height = self.state.get_checkpoint_height();
                let network_height = self.highest_seen_peer_height.load(std::sync::atomic::Ordering::Relaxed);
                let checkpoint_gap = network_height.saturating_sub(local_height);

                if checkpoint_gap >= 2 {
                    tracing::info!(
                        "Convergence batch: SKIPPING re-queue of {} deferred txs — node is behind by {} checkpoints (local={}, network={}), checkpoint sync will resolve them",
                        deferred_hashes.len(), checkpoint_gap, local_height, network_height
                    );
                } else {
                    const MAX_REQUEUE: usize = 200;
                    let requeue_budget =
                        MAX_REQUEUE.saturating_sub(inner.confirmed_pending_execution.len());
                    let mut requeued = 0usize;
                    for tx in &txs {
                        if requeued >= requeue_budget {
                            break;
                        }
                        if deferred_hashes.contains(&tx.hash) {
                            inner.confirmed_pending_execution.push(tx.clone());
                            requeued += 1;
                        }
                    }
                    if requeued > 0 {
                        tracing::debug!(
                            "Convergence batch: re-queued {}/{} deferred txs for next batch ({} executed this round)",
                            requeued, deferred_hashes.len(), executed_count
                        );
                    }
                }
            }
        }

        if let Some(ref eb) = self.event_bus {
            for entry in &results {
                if !matches!(entry.result, crate::state::ConvergenceExecResult::Executed) {
                    continue;
                }
                eb.publish(crate::events::NodeEvent::FastPathExecuted {
                    hash: entry.hash.clone(),
                    from: entry.from_addr.clone(),
                    to: entry.to_addr.clone(),
                    amount: from_micro_units(entry.amount),
                });
                if let Some(ref acc) = entry.from_account {
                    if !entry.from_addr.is_empty() && entry.from_addr != "faucet" {
                        eb.publish(crate::events::NodeEvent::AccountUpdated {
                            address: entry.from_addr.clone(),
                            balance: from_micro_units(acc.balance),
                            nonce: acc.nonce,
                            staked: from_micro_units(acc.staked),
                        });
                    }
                }
                if let Some(ref acc) = entry.to_account {
                    if !entry.to_addr.is_empty() {
                        eb.publish(crate::events::NodeEvent::AccountUpdated {
                            address: entry.to_addr.clone(),
                            balance: from_micro_units(acc.balance),
                            nonce: acc.nonce,
                            staked: from_micro_units(acc.staked),
                        });
                    }
                }
            }
        }

        if executed_count > 0 || dropped_count > 0 {
            tracing::info!(
                "Convergence batch: executed {}/{}, {} deferred (nonce gap, checkpoint will finalize)",
                executed_count, total, dropped_count
            );
        }
    }


    pub async fn broadcast_conflict_resolution(
        &self,
        tx_hash_1: &str,
        tx_hash_2: &str,
        winner_hash: &str,
        weight_1: u64,
        weight_2: u64,
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

        #[cfg(feature = "p2p")]
        self.broadcast_via_p2p(&message).await;
    }

    pub async fn get_stats(&self) -> GossipStats {
        self.inner.read().await.stats.clone()
    }

    pub async fn get_peer_count(&self) -> usize {
        #[cfg(feature = "p2p")]
        {
            if let Some(ref handle) = self.network_handle {
                return handle.get_peer_count().await;
            }
        }

        self.inner
            .read()
            .await
            .peers
            .values()
            .filter(|p| p.is_healthy)
            .count()
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
                return handle.get_connected_peers().await;
            }
        }
        Vec::new()
    }

    pub async fn get_convergence_stats(&self) -> crate::fast_path::FastPathStats {
        let inner = self.inner.read().await;

        let mut times: Vec<u64> = inner
            .convergence_confirmed
            .values()
            .filter_map(|f| f.finality_time_ms())
            .collect();
        times.sort();
        let avg_confirmation_ms = if !times.is_empty() {
            Some(times[times.len() / 2])
        } else {
            None
        };

        // Get total validator stake from state
        let rewards = self.state.rewards.read().await;
        let total_validator_stake = rewards.get_total_staked();
        drop(rewards);

        crate::fast_path::FastPathStats {
            enabled: true,
            pending_count: inner.convergence_pending.len(),
            confirmed_count: inner.convergence_confirmed.len(),
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
            info!(
                "PEER DISCOVERY: Adding new peer {} (total peers: {})",
                address,
                inner.peers.len() + 1
            );
            inner.peers.insert(
                address.clone(),
                PeerInfo {
                    address,
                    node_id: String::new(),
                    last_seen: now,
                    dag_size: 0,
                    checkpoint_height: 0,
                    latency_ms: 0,
                    is_healthy: true,
                    consecutive_failures: 0,
                    backoff_until: 0,
                },
            );
            inner.stats.peers_discovered += 1;
        }
    }

    pub async fn remove_peer(&self, address: &str) {
        let mut inner = self.inner.write().await;
        inner.peers.remove(address);
    }

    pub fn mark_tx_known(&self, _hash: &str) {}

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
            GossipMessage::TxConfirmBroadcast { sender_url, .. } => sender_url.clone(),
            GossipMessage::TxConfirmAck { sender_url, .. } => sender_url.clone(),
            GossipMessage::MergePayload { sender_url, .. } => sender_url.clone(),
            GossipMessage::MergeResult { sender_url, .. } => sender_url.clone(),
            GossipMessage::CheckpointIntent { sender_url, .. } => sender_url.clone(),
            GossipMessage::LeaderTimeout { sender_url, .. } => sender_url.clone(),
            GossipMessage::DaChunk { sender_url, .. } => sender_url.clone(),
            GossipMessage::QccVoteRequest { sender_url, .. } => sender_url.clone(),
            GossipMessage::QccVoteCast { sender_url, .. } => sender_url.clone(),
            GossipMessage::Batch { .. } => None,
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
        let checkpoint_height = self.state.get_checkpoint_height();
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
            assert!(
                filter.might_contain(item),
                "Bloom filter should never have false negatives"
            );
        }
    }

    #[test]
    fn test_bloom_filter_compact() {
        let mut filter = BloomFilter::compact(100);

        for i in 0..100 {
            filter.insert(&format!("tx_{}", i));
        }

        assert!(
            filter.size_bytes() < 2000,
            "Compact filter should use minimal memory"
        );

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
        assert!(
            fpr < 0.05,
            "False positive rate should be under 5% for 10K items in default filter"
        );
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
        let deserialized: BloomFilter =
            serde_json::from_str(&serialized).expect("Should deserialize");

        assert!(deserialized.might_contain("tx1"));
        assert!(deserialized.might_contain("tx2"));
        assert!(!deserialized.might_contain("tx3"));
        assert_eq!(deserialized.item_count(), 2);
    }
}
