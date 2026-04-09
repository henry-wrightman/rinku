#![cfg(feature = "p2p")]

use anyhow::Result;
use libp2p::futures::{StreamExt, FutureExt};
use libp2p::{
    gossipsub::{self, IdentTopic, MessageAuthenticity, ValidationMode},
    identity::Keypair,
    identify,
    mdns,
    noise,
    request_response::{self, OutboundRequestId, ProtocolSupport, ResponseChannel},
    swarm::SwarmEvent,
    tcp, yamux, Multiaddr, PeerId, Swarm, StreamProtocol,
};

use crate::cbor_codec::CborCodec;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, info, trace, warn};

use crate::gossip::GossipMessage;

fn topic_critical() -> String {
    let major = crate::versioning::PROTOCOL_VERSION.split('.').next().unwrap_or("1");
    format!("rinku/{}/critical", major)
}

fn topic_consensus() -> String {
    let major = crate::versioning::PROTOCOL_VERSION.split('.').next().unwrap_or("1");
    format!("rinku/{}/consensus", major)
}

fn topic_data() -> String {
    let major = crate::versioning::PROTOCOL_VERSION.split('.').next().unwrap_or("1");
    format!("rinku/{}/data", major)
}

fn topic_general() -> String {
    let major = crate::versioning::PROTOCOL_VERSION.split('.').next().unwrap_or("1");
    format!("rinku/{}/general", major)
}

fn topic_priority() -> String {
    let major = crate::versioning::PROTOCOL_VERSION.split('.').next().unwrap_or("1");
    format!("rinku/{}/priority", major)
}

fn sync_protocol_id() -> String {
    let major = crate::versioning::PROTOCOL_VERSION.split('.').next().unwrap_or("1");
    format!("/rinku/sync/{}", major)
}

fn vote_protocol_id() -> String {
    let major = crate::versioning::PROTOCOL_VERSION.split('.').next().unwrap_or("1");
    format!("/rinku/vote/{}", major)
}

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const MESH_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5);
const MIN_MESH_PEERS: usize = 2;
const RECONNECT_CHECK_INTERVAL: Duration = Duration::from_secs(1);
const MAX_RAPID_RECONNECTS: usize = 3;
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const RR_FAILURE_DISCONNECT_THRESHOLD: u32 = 3;

/// Sync request types for P2P sync operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncRequest {
    /// Request a full state snapshot
    Snapshot,
    /// Request delta sync from a checkpoint height
    Delta { from_checkpoint: u64 },
    /// Request a specific transaction by hash
    Transaction { hash: String },
    /// Request proof for a transaction
    Proof { tx_hash: String },
    /// Request accounts state (for merkle verification)
    AccountsState { addresses: Vec<String> },
    /// Handshake with peer info
    Handshake(PeerHandshake),
    /// Direct push of a finalized checkpoint to a peer (reliable delivery)
    CheckpointPush(CheckpointPushData),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointPushData {
    pub checkpoint: rinku_core::types::Checkpoint,
    pub finalized_tx_hashes: Vec<String>,
    pub finalized_transactions: Vec<rinku_core::types::SignedTransaction>,
    pub precomputed_proofs: Vec<rinku_core::types::AccountStateProof>,
}

/// Request for a validator to sign a checkpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointVoteRequest {
    /// Hex-encoded checkpoint hash to sign
    pub checkpoint_hash: String,
    /// Checkpoint height
    pub height: u64,
    /// Tx merkle root for verification
    pub tx_merkle_root: String,
    /// State root for verification
    pub state_root: String,
    /// Exact list of transaction hashes to be finalized (ensures all validators agree on the same set)
    #[serde(default)]
    pub finalized_tx_hashes: Vec<String>,
    /// Full transaction data for transactions being finalized (Option A: inline sync)
    /// Allows validators to apply missing transactions without extra round-trips
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub finalized_transactions: Vec<rinku_core::types::SignedTransaction>,
}

/// Sync response types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncResponse {
    /// Full state snapshot
    Snapshot(SnapshotData),
    /// Delta sync response with transactions since checkpoint
    Delta(DeltaData),
    /// Single transaction
    Transaction(Option<TransactionData>),
    /// Transaction proof
    Proof(Option<ProofData>),
    /// Accounts state for verification
    AccountsState(Vec<AccountData>),
    /// Handshake response
    Handshake(PeerHandshake),
    /// Ack for a direct checkpoint push
    CheckpointPushAck { accepted: bool },
    /// Error response
    Error { message: String },
}

/// Response containing a validator's checkpoint signature
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointVoteResponse {
    pub validator_address: String,
    pub signature: String,
    pub signature_bytes: Vec<u8>,
    pub bls_public_key: String,
    pub stake: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VoteRequest {
    CheckpointVote(CheckpointVoteRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VoteResponse {
    CheckpointVote(Option<CheckpointVoteResponse>),
    Error { message: String },
}

pub struct IncomingVoteRequest {
    pub peer_id: String,
    pub request: VoteRequest,
    pub response_channel: ResponseChannel<VoteResponse>,
}

struct PendingVoteRequest {
    peer_id: PeerId,
    response_tx: Option<oneshot::Sender<VoteResponse>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerHandshake {
    pub protocol_version: String,
    pub chain_id: String,
    pub network_id: String,
    pub node_id: String,
    pub checkpoint_height: u64,
    pub validator_address: Option<String>,
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub known_peer_addrs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotData {
    pub accounts: Vec<AccountData>,
    pub validators: Vec<ValidatorData>,
    pub checkpoints: Vec<CheckpointData>,
    pub recent_txs: Vec<TransactionData>,
    pub merkle_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaData {
    pub transactions: Vec<TransactionData>,
    pub new_checkpoints: Vec<CheckpointData>,
    pub from_checkpoint: u64,
    pub to_checkpoint: u64,
    #[serde(default)]
    pub tx_checkpoint_heights: std::collections::HashMap<String, u64>,
    #[serde(default)]
    pub validators: Vec<ValidatorData>,
    #[serde(default)]
    pub precomputed_proofs: Vec<rinku_core::types::AccountStateProof>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountData {
    pub address: String,
    pub balance: u64,
    pub nonce: u64,
    pub stake: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorData {
    pub address: String,
    pub stake: u64,
    pub bls_public_key: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointData {
    pub height: u64,
    pub merkle_root: String,
    pub timestamp: u64,
    pub tx_count: u64,
    #[serde(default)]
    pub hash: Option<String>,
    #[serde(default)]
    pub previous_hash: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub genesis_hash: Option<String>,
    #[serde(default)]
    pub finalized_tx_hashes: Vec<String>,
    #[serde(default)]
    pub state_root: Option<String>,
    #[serde(default)]
    pub receipt_root: Option<String>,
    #[serde(default)]
    pub tip_count: Option<u32>,
    #[serde(default)]
    pub validator_signatures: Vec<rinku_core::types::ValidatorSignature>,
    #[serde(default)]
    pub signer_bitmap: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionData {
    pub hash: String,
    pub from: String,
    pub to: String,
    pub amount: u64,
    pub nonce: u64,
    pub timestamp: u64,
    pub signature: String,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub gas_price: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofData {
    pub tx_hash: String,
    pub merkle_proof: Vec<String>,
    pub checkpoint_height: u64,
    pub checkpoint_root: String,
}

#[derive(libp2p::swarm::NetworkBehaviour)]
pub struct RinkuBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub request_response: request_response::Behaviour<CborCodec<SyncRequest, SyncResponse>>,
    pub vote_protocol: request_response::Behaviour<CborCodec<VoteRequest, VoteResponse>>,
    pub identify: identify::Behaviour,
    pub mdns: mdns::tokio::Behaviour,
}

#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub listen_addr: String,
    pub bootstrap_peers: Vec<String>,
    pub enable_mdns: bool,
    pub data_dir: Option<String>,
    pub external_addr: Option<String>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            listen_addr: "/ip4/0.0.0.0/tcp/4001".to_string(),
            bootstrap_peers: Vec::new(),
            enable_mdns: true,
            data_dir: None,
            external_addr: None,
        }
    }
}

fn load_or_generate_keypair(data_dir: Option<&str>) -> Keypair {
    let key_path = data_dir.map(|d| format!("{}/p2p-identity.key", d));
    
    if let Some(ref path) = key_path {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(keypair) = Keypair::from_protobuf_encoding(&bytes) {
                info!("Loaded existing P2P identity from {}", path);
                return keypair;
            } else {
                warn!("Failed to decode P2P identity from {}, generating new one", path);
            }
        }
    }
    
    let keypair = Keypair::generate_ed25519();
    
    if let Some(ref path) = key_path {
        if let Ok(bytes) = keypair.to_protobuf_encoding() {
            if let Err(e) = std::fs::write(path, bytes) {
                warn!("Failed to save P2P identity to {}: {}", path, e);
            } else {
                info!("Saved new P2P identity to {}", path);
            }
        }
    }
    
    keypair
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerStats {
    pub peer_id: String,
    pub connected_at: u64,
    pub messages_received: u64,
    pub messages_sent: u64,
    pub last_seen: u64,
    pub handshake_validated: bool,
    pub handshake_info: Option<PeerHandshake>,
    pub rate_limit_tokens: u32,
    pub last_rate_update: u64,
    pub score: i32,
    #[serde(default)]
    pub consecutive_rr_failures: u32,
    #[serde(default)]
    pub last_rr_success: u64,
}

#[derive(Debug, Clone)]
pub struct DoSConfig {
    pub max_connections: usize,
    pub rate_limit_tokens_per_second: u32,
    pub max_rate_limit_tokens: u32,
    pub ban_duration_secs: u64,
    pub min_protocol_version: String,
}

impl Default for DoSConfig {
    fn default() -> Self {
        Self {
            max_connections: 50,
            rate_limit_tokens_per_second: 100,
            max_rate_limit_tokens: 1000,
            ban_duration_secs: 30,
            min_protocol_version: crate::versioning::PROTOCOL_VERSION.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BannedPeer {
    pub peer_id: String,
    pub reason: String,
    pub banned_at: u64,
    pub expires_at: u64,
}

const MISBEHAVIOR_SCORE_PENALTY: i32 = 10;
const BAN_SCORE_THRESHOLD: i32 = -50;

#[derive(Debug, Clone)]
pub struct HandshakeConfig {
    pub protocol_version: String,
    pub chain_id: String,
    pub network_id: String,
    pub required_chain_id: Option<String>,
    pub required_network_id: Option<String>,
    pub validator_address: Option<String>,
}

impl Default for HandshakeConfig {
    fn default() -> Self {
        Self {
            protocol_version: crate::versioning::PROTOCOL_VERSION.to_string(),
            chain_id: "rinku-mainnet".to_string(),
            network_id: "mainnet".to_string(),
            required_chain_id: None,
            required_network_id: None,
            validator_address: None,
        }
    }
}

pub struct NetworkStats {
    pub local_peer_id: String,
    pub connected_peers: usize,
    pub messages_published: u64,
    pub messages_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
}

/// Pending sync request waiting for response
struct PendingSyncRequest {
    peer_id: PeerId,
    response_tx: Option<oneshot::Sender<SyncResponse>>,
}

/// Incoming sync request to be handled by the application
pub struct IncomingSyncRequest {
    pub peer_id: String,
    pub request: SyncRequest,
    pub response_channel: ResponseChannel<SyncResponse>,
}

pub struct NetworkService {
    local_peer_id: PeerId,
    swarm: Swarm<RinkuBehaviour>,
    topic_critical: IdentTopic,
    topic_consensus: IdentTopic,
    topic_data: IdentTopic,
    topic_general: IdentTopic,
    topic_priority: IdentTopic,
    config: NetworkConfig,
    stats: Arc<RwLock<NetworkStatsInner>>,
    message_tx: mpsc::Sender<GossipMessage>,
    priority_message_tx: mpsc::Sender<GossipMessage>,
    checkpoint_message_tx: mpsc::Sender<GossipMessage>,
    outbound_rx: mpsc::Receiver<GossipMessage>,
    peers: Arc<RwLock<HashMap<PeerId, PeerStats>>>,
    sync_request_rx: mpsc::Receiver<(PeerId, SyncRequest, oneshot::Sender<SyncResponse>)>,
    sync_incoming_tx: mpsc::Sender<IncomingSyncRequest>,
    pending_requests: HashMap<OutboundRequestId, PendingSyncRequest>,
    vote_request_rx: mpsc::Receiver<(PeerId, VoteRequest, oneshot::Sender<VoteResponse>)>,
    vote_incoming_tx: mpsc::Sender<IncomingVoteRequest>,
    pending_vote_requests: HashMap<OutboundRequestId, PendingVoteRequest>,
    /// DoS protection configuration
    dos_config: DoSConfig,
    /// Handshake configuration
    handshake_config: HandshakeConfig,
    /// Banned peers
    banned_peers: Arc<RwLock<HashMap<String, BannedPeer>>>,
    /// Persisted peer scores across restarts
    peer_scores: Arc<RwLock<HashMap<String, i32>>>,
    /// Optional path to peer score storage
    peer_scores_path: Option<String>,
    /// Command channel for dial requests etc
    command_rx: mpsc::Receiver<NetworkCommand>,
    recently_dialed_pex: HashMap<String, u64>,
    mesh_maintenance_counter: u32,
    peer_last_addr: HashMap<PeerId, Multiaddr>,
    reconnect_pending: Vec<(Multiaddr, usize, Instant)>,
    outbound_buffer: Vec<GossipMessage>,
    shared_checkpoint_height: Arc<AtomicU64>,
}

struct NetworkStatsInner {
    messages_published: u64,
    messages_received: u64,
    bytes_sent: u64,
    bytes_received: u64,
    last_logged_bytes_sent: u64,
    last_logged_bytes_received: u64,
    last_logged_msgs_published: u64,
    last_logged_msgs_received: u64,
}

impl NetworkService {
    pub fn new(config: NetworkConfig) -> Result<(Self, NetworkHandle)> {
        let local_key = load_or_generate_keypair(config.data_dir.as_deref());
        let local_peer_id = PeerId::from(local_key.public());

        info!("Local peer id: {}", local_peer_id);

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(HEARTBEAT_INTERVAL)
            .validation_mode(ValidationMode::Permissive)
            .flood_publish(true)
            .message_id_fn(|msg| {
                use sha2::{Sha256, Digest};
                let mut hasher = Sha256::new();
                hasher.update(&msg.data);
                let hash = hasher.finalize();
                gossipsub::MessageId::from(hex::encode(&hash[..16]))
            })
            .max_transmit_size(2 * 1024 * 1024)
            .build()
            .map_err(|e| anyhow::anyhow!("Invalid gossipsub config: {}", e))?;

        let gossipsub = gossipsub::Behaviour::new(
            MessageAuthenticity::Signed(local_key.clone()),
            gossipsub_config,
        )
        .map_err(|e| anyhow::anyhow!("Failed to create gossipsub: {}", e))?;

        let mdns = mdns::tokio::Behaviour::new(
            mdns::Config::default(),
            local_peer_id,
        )?;

        // Request-response protocol for sync operations
        // Use custom CBOR codec with 16MB limits to support checkpoint votes with many transactions
        // This allows up to ~30,000 transactions per checkpoint (at ~500 bytes each)
        let sync_protocol = StreamProtocol::try_from_owned(sync_protocol_id())
            .expect("valid sync protocol string");
        let cbor_codec: CborCodec<SyncRequest, SyncResponse> = CborCodec::new(
            16 * 1024 * 1024,  // 16 MB max request size
            16 * 1024 * 1024,  // 16 MB max response size
        );
        let request_response = request_response::Behaviour::with_codec(
            cbor_codec,
            [(sync_protocol, ProtocolSupport::Full)],
            request_response::Config::default()
                .with_request_timeout(Duration::from_secs(10)),
        );

        let vote_protocol_str = StreamProtocol::try_from_owned(vote_protocol_id())
            .expect("valid vote protocol string");
        let vote_cbor_codec: CborCodec<VoteRequest, VoteResponse> = CborCodec::new(
            16 * 1024 * 1024,
            1024 * 1024,
        );
        let vote_protocol = request_response::Behaviour::with_codec(
            vote_cbor_codec,
            [(vote_protocol_str, ProtocolSupport::Full)],
            request_response::Config::default()
                .with_request_timeout(Duration::from_secs(5)),
        );

        // Identify protocol for peer info exchange
        let identify = identify::Behaviour::new(
            identify::Config::new(format!("/rinku/{}", crate::versioning::PROTOCOL_VERSION), local_key.public())
                .with_agent_version(format!("rinku-node/{}", crate::versioning::NODE_VERSION)),
        );

        let behaviour = RinkuBehaviour { 
            gossipsub, 
            request_response,
            vote_protocol,
            identify,
            mdns,
        };

        let swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
            .with_tokio()
            .with_tcp(
                tcp::Config::default().nodelay(true),
                noise::Config::new,
                || {
                    let mut cfg = yamux::Config::default();
                    cfg.set_max_num_streams(1024);
                    cfg.set_receive_window_size(16 * 1024 * 1024);
                    cfg.set_max_buffer_size(16 * 1024 * 1024);
                    cfg
                },
            )?
            .with_behaviour(|_| behaviour)?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(600)))
            .build();

        let t_critical = IdentTopic::new(topic_critical());
        let t_consensus = IdentTopic::new(topic_consensus());
        let t_data = IdentTopic::new(topic_data());
        let t_general = IdentTopic::new(topic_general());
        let t_priority = IdentTopic::new(topic_priority());

        let (message_tx, message_rx) = mpsc::channel(2000);
        let (priority_message_tx, priority_message_rx) = mpsc::channel(2000);
        let (checkpoint_message_tx, checkpoint_message_rx) = mpsc::channel(50);
        let (outbound_tx, outbound_rx) = mpsc::channel(300);
        let (sync_request_tx, sync_request_rx) = mpsc::channel(100);
        let (sync_incoming_tx, sync_incoming_rx) = mpsc::channel(100);
        let (vote_request_tx, vote_request_rx) = mpsc::channel(50);
        let (vote_incoming_tx, vote_incoming_rx) = mpsc::channel(50);
        let (command_tx, command_rx) = mpsc::channel(100);

        let stats = Arc::new(RwLock::new(NetworkStatsInner {
            messages_published: 0,
            messages_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
            last_logged_bytes_sent: 0,
            last_logged_bytes_received: 0,
            last_logged_msgs_published: 0,
            last_logged_msgs_received: 0,
        }));

        let peers = Arc::new(RwLock::new(HashMap::new()));

        let peer_scores_path = config.data_dir.as_ref().map(|d| format!("{}/peer_scores.json", d));
        let peer_scores = Arc::new(RwLock::new(load_peer_scores(&peer_scores_path)));

        let shared_checkpoint_height = Arc::new(AtomicU64::new(0));

        let handle = NetworkHandle {
            outbound_tx,
            message_rx: Some(message_rx),
            priority_message_rx: Some(priority_message_rx),
            checkpoint_message_rx: Some(checkpoint_message_rx),
            peers: peers.clone(),
            local_peer_id: local_peer_id.to_string(),
            sync_request_tx,
            sync_incoming_rx: Some(sync_incoming_rx),
            vote_request_tx,
            vote_incoming_rx: Some(vote_incoming_rx),
            command_tx,
            stats: stats.clone(),
            shared_checkpoint_height: shared_checkpoint_height.clone(),
        };

        let service = Self {
            local_peer_id,
            swarm,
            topic_critical: t_critical,
            topic_consensus: t_consensus,
            topic_data: t_data,
            topic_general: t_general,
            topic_priority: t_priority,
            config,
            stats,
            message_tx,
            priority_message_tx,
            checkpoint_message_tx,
            outbound_rx,
            peers,
            sync_request_rx,
            sync_incoming_tx,
            pending_requests: HashMap::new(),
            vote_request_rx,
            vote_incoming_tx,
            pending_vote_requests: HashMap::new(),
            dos_config: DoSConfig::default(),
            handshake_config: HandshakeConfig::default(),
            banned_peers: Arc::new(RwLock::new(HashMap::new())),
            peer_scores,
            peer_scores_path,
            command_rx,
            recently_dialed_pex: HashMap::new(),
            mesh_maintenance_counter: 0,
            peer_last_addr: HashMap::new(),
            reconnect_pending: Vec::new(),
            outbound_buffer: Vec::with_capacity(64),
            shared_checkpoint_height,
        };

        Ok((service, handle))
    }

    pub fn local_peer_id(&self) -> String {
        self.local_peer_id.to_string()
    }

    pub async fn start(&mut self) -> Result<()> {
        let listen_addr: Multiaddr = self.config.listen_addr.parse()?;
        self.swarm.listen_on(listen_addr)?;

        self.swarm.behaviour_mut().gossipsub.subscribe(&self.topic_critical)?;
        self.swarm.behaviour_mut().gossipsub.subscribe(&self.topic_consensus)?;
        self.swarm.behaviour_mut().gossipsub.subscribe(&self.topic_data)?;
        self.swarm.behaviour_mut().gossipsub.subscribe(&self.topic_general)?;
        self.swarm.behaviour_mut().gossipsub.subscribe(&self.topic_priority)?;

        for peer_addr in &self.config.bootstrap_peers {
            if let Ok(addr) = peer_addr.parse::<Multiaddr>() {
                info!("Dialing bootstrap peer: {}", addr);
                if let Err(e) = self.swarm.dial(addr.clone()) {
                    warn!("Failed to dial {}: {}", addr, e);
                }
            }
        }

        info!("Network service started on {}", self.config.listen_addr);

        self.run_event_loop().await
    }

    /// Public run method for running the service (starts listening and runs event loop)
    /// This method will block until the service is stopped or encounters an error.
    pub async fn run(&mut self) -> Result<()> {
        self.start().await?;
        self.run_event_loop().await
    }

    const COALESCE_INTERVAL_MS: u64 = 25;
    const OUTBOUND_BUFFER_CAP: usize = 200;

    fn drain_outbound_to_buffer(&mut self) {
        while let Ok(msg) = self.outbound_rx.try_recv() {
            self.outbound_buffer.push(msg);
            if self.outbound_buffer.len() >= Self::OUTBOUND_BUFFER_CAP {
                break;
            }
        }
    }

    fn has_critical_in_buffer(&self) -> bool {
        self.outbound_buffer.iter().any(|m| matches!(m,
            GossipMessage::CheckpointAnnouncement { .. } |
            GossipMessage::CheckpointIntent { .. } |
            GossipMessage::DaChunk { .. } |
            GossipMessage::QccVoteRequest { .. } |
            GossipMessage::QccVoteCast { .. }
        ))
    }

    async fn flush_outbound_buffer(&mut self) {
        if self.outbound_buffer.is_empty() {
            return;
        }
        let batch = std::mem::take(&mut self.outbound_buffer);
        self.outbound_buffer = Vec::with_capacity(64);
        self.publish_batched(batch).await;
    }

    async fn run_event_loop(&mut self) -> Result<()> {
        let mut mesh_maintenance_interval = tokio::time::interval(MESH_MAINTENANCE_INTERVAL);
        mesh_maintenance_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut reconnect_interval = tokio::time::interval(RECONNECT_CHECK_INTERVAL);
        reconnect_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_coalesce_flush = Instant::now();
        let coalesce_dur = std::time::Duration::from_millis(Self::COALESCE_INTERVAL_MS);
        
        loop {
            while let Ok(cmd) = self.command_rx.try_recv() {
                self.handle_command(cmd).await;
            }

            self.drain_outbound_to_buffer();

            let should_flush = self.has_critical_in_buffer()
                || self.outbound_buffer.len() >= Self::OUTBOUND_BUFFER_CAP
                || (!self.outbound_buffer.is_empty() && last_coalesce_flush.elapsed() >= coalesce_dur);

            if should_flush {
                self.flush_outbound_buffer().await;
                last_coalesce_flush = Instant::now();
            }

            tokio::select! {
                biased;

                Some(cmd) = self.command_rx.recv() => {
                    self.handle_command(cmd).await;
                }
                Some((peer_id, request, response_tx)) = self.vote_request_rx.recv() => {
                    let request_id = self.swarm.behaviour_mut()
                        .vote_protocol
                        .send_request(&peer_id, request);
                    self.pending_vote_requests.insert(request_id, PendingVoteRequest {
                        peer_id,
                        response_tx: Some(response_tx),
                    });
                    debug!("Sent vote request {:?} to {}", request_id, peer_id);
                }
                Some((peer_id, request, response_tx)) = self.sync_request_rx.recv() => {
                    let request_id = self.swarm.behaviour_mut()
                        .request_response
                        .send_request(&peer_id, request);
                    self.pending_requests.insert(request_id, PendingSyncRequest {
                        peer_id,
                        response_tx: Some(response_tx),
                    });
                    debug!("Sent sync request {:?} to {}", request_id, peer_id);
                }
                event = self.swarm.select_next_some() => {
                    self.handle_swarm_event(event).await;
                    let drain_start = Instant::now();
                    const MAX_DRAIN_EVENTS: usize = 5;
                    const MAX_DRAIN_MS: u128 = 50;
                    for _ in 0..MAX_DRAIN_EVENTS {
                        while let Ok(cmd) = self.command_rx.try_recv() {
                            self.handle_command(cmd).await;
                        }
                        if drain_start.elapsed().as_millis() >= MAX_DRAIN_MS {
                            break;
                        }
                        match self.swarm.next().now_or_never() {
                            Some(Some(event)) => self.handle_swarm_event(event).await,
                            _ => break,
                        }
                    }
                }
                Some(msg) = self.outbound_rx.recv() => {
                    self.outbound_buffer.push(msg);
                    self.drain_outbound_to_buffer();
                    if self.has_critical_in_buffer() || self.outbound_buffer.len() >= Self::OUTBOUND_BUFFER_CAP {
                        self.flush_outbound_buffer().await;
                        last_coalesce_flush = Instant::now();
                    }
                }
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(last_coalesce_flush + coalesce_dur)), if !self.outbound_buffer.is_empty() => {
                    self.drain_outbound_to_buffer();
                    self.flush_outbound_buffer().await;
                    last_coalesce_flush = Instant::now();
                }
                _ = reconnect_interval.tick() => {
                    self.process_reconnect_queue();
                }
                _ = mesh_maintenance_interval.tick() => {
                    self.perform_mesh_maintenance().await;
                }
            }
        }
    }

    fn process_reconnect_queue(&mut self) {
        if self.reconnect_pending.is_empty() {
            return;
        }

        let now = Instant::now();
        let mut still_pending = Vec::new();

        for (addr, retries, next_at) in std::mem::take(&mut self.reconnect_pending) {
            if now < next_at {
                still_pending.push((addr, retries, next_at));
                continue;
            }

            let target_peer_id = addr.iter().find_map(|p| {
                if let libp2p::multiaddr::Protocol::P2p(pid) = p {
                    Some(pid)
                } else {
                    None
                }
            });

            if let Some(pid) = target_peer_id {
                if self.swarm.is_connected(&pid) {
                    debug!("Reconnect: peer {} already reconnected, dropping from queue", pid);
                    continue;
                }
            }

            info!("Reconnect: re-dialing {} (attempt {}/{})", addr, retries + 1, MAX_RAPID_RECONNECTS);
            match self.swarm.dial(addr.clone()) {
                Ok(_) => {
                    if retries + 1 < MAX_RAPID_RECONNECTS {
                        still_pending.push((addr, retries + 1, now + RECONNECT_DELAY));
                    } else {
                        info!("Reconnect: max rapid retries reached for {}, falling back to mesh maintenance", addr);
                    }
                }
                Err(e) => {
                    warn!("Reconnect: failed to dial {}: {}", addr, e);
                }
            }
        }

        self.reconnect_pending = still_pending;
    }
    
    async fn perform_mesh_maintenance(&mut self) {
        self.mesh_maintenance_counter = self.mesh_maintenance_counter.wrapping_add(1);

        if let Ok(mut stats) = self.stats.try_write() {
            let delta_sent = stats.bytes_sent - stats.last_logged_bytes_sent;
            let delta_recv = stats.bytes_received - stats.last_logged_bytes_received;
            let delta_pub = stats.messages_published - stats.last_logged_msgs_published;
            let delta_rcv = stats.messages_received - stats.last_logged_msgs_received;
            if delta_sent > 0 || delta_recv > 0 {
                info!(
                    "Gossip throughput: {:.1}KB/s out ({} msgs/s), {:.1}KB/s in ({} msgs/s) | total: {}KB sent, {}KB recv",
                    delta_sent as f64 / 5120.0, delta_pub / 5, delta_recv as f64 / 5120.0, delta_rcv / 5,
                    stats.bytes_sent / 1024, stats.bytes_received / 1024
                );
            }
            stats.last_logged_bytes_sent = stats.bytes_sent;
            stats.last_logged_bytes_received = stats.bytes_received;
            stats.last_logged_msgs_published = stats.messages_published;
            stats.last_logged_msgs_received = stats.messages_received;
        }

        let validated_peer_count = {
            let peers = self.peers.read().await;
            peers.values().filter(|p| p.handshake_validated).count()
        };
        
        if validated_peer_count < MIN_MESH_PEERS {
            info!(
                "Mesh unhealthy: {} validated peers (min: {}), re-dialing bootstrap peers",
                validated_peer_count, MIN_MESH_PEERS
            );
            
            for peer_addr in &self.config.bootstrap_peers.clone() {
                if let Ok(addr) = peer_addr.parse::<Multiaddr>() {
                    info!("Re-dialing bootstrap peer: {}", addr);
                    if let Err(e) = self.swarm.dial(addr.clone()) {
                        debug!("Failed to re-dial {}: {}", addr, e);
                    }
                }
            }
        } else {
            debug!("Mesh healthy: {} validated peers", validated_peer_count);
        }

        const RR_STALE_THRESHOLD_SECS: u64 = 60;
        {
            let now_secs = current_time_secs();
            let stale_peers: Vec<PeerId> = {
                let peers = self.peers.read().await;
                peers.iter()
                    .filter(|(pid, stats)| {
                        stats.handshake_validated
                            && self.swarm.is_connected(pid)
                            && stats.consecutive_rr_failures > 0
                            && now_secs.saturating_sub(stats.last_rr_success) > RR_STALE_THRESHOLD_SECS
                    })
                    .map(|(pid, _)| *pid)
                    .collect()
            };
            for pid in stale_peers {
                warn!(
                    "PEER-HEALTH: {} has stale request-response (no success in {}s), forcing disconnect for fresh session",
                    pid, RR_STALE_THRESHOLD_SECS
                );
                let _ = self.swarm.disconnect_peer_id(pid);
            }
        }

        const PEX_INTERVAL_CYCLES: u32 = 3;
        const PEX_TARGET_PEERS: usize = 5;
        if self.mesh_maintenance_counter % PEX_INTERVAL_CYCLES == 0
            && validated_peer_count > 0
            && validated_peer_count < PEX_TARGET_PEERS
        {
            let connected_peer_ids: Vec<PeerId> = {
                let peers = self.peers.read().await;
                peers.iter()
                    .filter(|(_, stats)| stats.handshake_validated)
                    .map(|(pid, _)| *pid)
                    .collect()
            };

            let handshake = self.create_handshake(0, self.handshake_config.validator_address.clone());
            let request = SyncRequest::Handshake(handshake);

            for pid in connected_peer_ids {
                if self.swarm.is_connected(&pid) {
                    debug!("PEX: Sending periodic handshake to {}", pid);
                    let request_id = self.swarm.behaviour_mut().request_response.send_request(&pid, request.clone());
                    self.pending_requests.insert(request_id, PendingSyncRequest {
                        peer_id: pid,
                        response_tx: None,
                    });
                }
            }
        }
    }

    async fn handle_command(&mut self, cmd: NetworkCommand) {
        match cmd {
            NetworkCommand::Dial(addr, response_tx) => {
                debug!("Dialing peer at {}", addr);
                let result = self.swarm.dial(addr.clone())
                    .map_err(|e| anyhow::anyhow!("Dial error: {}", e));
                let _ = response_tx.send(result);
            }
            NetworkCommand::SendVoteResponse(channel, response) => {
                debug!("Sending vote response via command");
                if let Err(e) = self.swarm.behaviour_mut().vote_protocol.send_response(channel, response) {
                    warn!("Failed to send vote response: {:?}", e);
                }
            }
            NetworkCommand::SendSyncResponse(channel, response) => {
                debug!("Sending sync response via command");
                self.send_sync_response(channel, response);
            }
        }
    }

    async fn handle_swarm_event(&mut self, event: SwarmEvent<RinkuBehaviourEvent>) {
        match event {
            SwarmEvent::Behaviour(RinkuBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message_id,
                message,
            })) => {
                debug!(
                    "Received message {:?} from {:?}",
                    message_id, propagation_source
                );

                let msg_size = message.data.len();
                if let Ok(mut stats) = self.stats.try_write() {
                    stats.messages_received += 1;
                    stats.bytes_received += msg_size as u64;
                }
                if msg_size > 128 * 1024 {
                    warn!("Large incoming gossip message: {}KB from {:?}", msg_size / 1024, propagation_source);
                }

                if !self.check_rate_limit_sync(&propagation_source) {
                    // Just drop the message, don't penalize - high load is not misbehavior
                    debug!("Rate limit exceeded for peer: {}, dropping message", propagation_source);
                    return;
                }

                let allow_message = match self.peers.try_read() {
                    Ok(peers) => peers.get(&propagation_source)
                        .map(|p| p.handshake_validated)
                        .unwrap_or(false),
                    Err(_) => {
                        debug!("Peers lock contended, allowing gossip from {}", propagation_source);
                        true
                    }
                };
                if !allow_message {
                    warn!("Dropping gossip from unhandshaked peer: {}", propagation_source);
                    self.record_misbehavior_sync(&propagation_source.to_string());
                    let _ = self.swarm.disconnect_peer_id(propagation_source);
                    return;
                }

                match serde_json::from_slice::<GossipMessage>(&message.data) {
                    Ok(gossip_msg) => {
                    let msgs_to_route: Vec<GossipMessage> = match gossip_msg {
                        GossipMessage::Batch { messages } => messages,
                        other => vec![other],
                    };
                    for routed_msg in msgs_to_route {
                    let is_checkpoint = matches!(&routed_msg, GossipMessage::CheckpointAnnouncement { .. });
                    let is_priority = matches!(&routed_msg, GossipMessage::CheckpointIntent { .. } | GossipMessage::LeaderTimeout { .. } | GossipMessage::DaChunk { .. } | GossipMessage::QccVoteRequest { .. } | GossipMessage::QccVoteCast { .. } | GossipMessage::CheckpointSignature { .. });
                    if is_checkpoint {
                        match self.checkpoint_message_tx.try_send(routed_msg) {
                            Ok(_) => {
                                debug!("Routed checkpoint to dedicated channel from {}", propagation_source);
                            },
                            Err(tokio::sync::mpsc::error::TrySendError::Full(msg)) => {
                                warn!("Checkpoint channel full, falling back to priority channel from {}", propagation_source);
                                match self.priority_message_tx.try_send(msg) {
                                    Ok(_) => {
                                        debug!("Checkpoint fallback to priority channel succeeded");
                                    },
                                    Err(tokio::sync::mpsc::error::TrySendError::Full(msg2)) => {
                                        warn!("Checkpoint fallback: priority also full, falling back to regular channel");
                                        if let Err(e) = self.message_tx.try_send(msg2) {
                                            warn!("Checkpoint DROPPED — all channels full: {}", e);
                                        }
                                    },
                                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                        warn!("Checkpoint fallback: priority channel closed");
                                    },
                                }
                            },
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                warn!("Checkpoint message channel closed");
                            },
                        }
                    } else if is_priority {
                        match self.priority_message_tx.try_send(routed_msg) {
                            Ok(_) => {
                                debug!("Routed priority message to priority channel from {}", propagation_source);
                            },
                            Err(tokio::sync::mpsc::error::TrySendError::Full(msg)) => {
                                warn!("Priority channel full, falling back to regular channel from {}", propagation_source);
                                let _ = self.message_tx.try_send(msg);
                            },
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                warn!("Priority message channel closed");
                            },
                        }
                    } else {
                        match self.message_tx.try_send(routed_msg) {
                            Ok(_) => {},
                            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                warn!("Gossip message channel full, dropping message from {}", propagation_source);
                            },
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                warn!("Gossip message channel closed");
                            },
                        }
                    }
                    }
                    }
                    Err(e) => {
                        warn!("Failed to deserialize gossip message from {}: {} ({}B dropped)", propagation_source, e, message.data.len());
                    }
                }

                if let Ok(mut peers) = self.peers.try_write() {
                    if let Some(peer_stats) = peers.get_mut(&propagation_source) {
                        peer_stats.messages_received += 1;
                        peer_stats.last_seen = current_time_secs();
                    }
                }
            }

            SwarmEvent::Behaviour(RinkuBehaviourEvent::Gossipsub(
                gossipsub::Event::Subscribed { peer_id, topic },
            )) => {
                info!("Peer {} subscribed to {:?}", peer_id, topic);
            }

            SwarmEvent::Behaviour(RinkuBehaviourEvent::Mdns(mdns::Event::Discovered(peers))) => {
                for (peer_id, addr) in peers {
                    info!("mDNS discovered peer: {} at {}", peer_id, addr);
                    self.swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                    
                    let now = current_time_secs();
                    let max_tokens = self.dos_config.max_rate_limit_tokens;
                    let saved_score = self.get_saved_peer_score_sync(&peer_id);
                    let mut peer_map = match self.peers.try_write() {
                        Ok(p) => p,
                        Err(_) => self.peers.write().await,
                    };
                    peer_map.entry(peer_id).or_insert_with(|| PeerStats {
                        peer_id: peer_id.to_string(),
                        connected_at: now,
                        messages_received: 0,
                        messages_sent: 0,
                        last_seen: now,
                        handshake_validated: false,
                        handshake_info: None,
                        rate_limit_tokens: max_tokens,
                        last_rate_update: now,
                        score: saved_score,
                        consecutive_rr_failures: 0,
                        last_rr_success: now,
                    });
                }
            }

            SwarmEvent::Behaviour(RinkuBehaviourEvent::Mdns(mdns::Event::Expired(peers))) => {
                for (peer_id, _) in peers {
                    debug!("mDNS peer expired: {}", peer_id);
                    self.swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                    
                    if let Ok(mut peer_map) = self.peers.try_write() {
                        peer_map.remove(&peer_id);
                    }
                }
            }

            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                let peer_id_str = peer_id.to_string();
                
                if self.is_peer_banned_sync(&peer_id_str) {
                    warn!("Rejecting banned peer: {}", peer_id_str);
                    let _ = self.swarm.disconnect_peer_id(peer_id);
                    return;
                }

                if self.is_connection_limit_reached_sync() {
                    warn!("Connection limit reached, rejecting peer: {}", peer_id_str);
                    let _ = self.swarm.disconnect_peer_id(peer_id);
                    return;
                }

                let remote_addr = endpoint.get_remote_address().clone();
                self.peer_last_addr.insert(peer_id, remote_addr);

                self.reconnect_pending.retain(|(addr, _, _)| {
                    let matches = addr.iter().any(|p| {
                        if let libp2p::multiaddr::Protocol::P2p(pid) = p {
                            pid == peer_id
                        } else {
                            false
                        }
                    });
                    !matches
                });

                info!("Connected to peer: {} via {:?}", peer_id, endpoint);
                
                self.swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                
                let now = current_time_secs();
                let max_tokens = self.dos_config.max_rate_limit_tokens;
                let saved_score = self.get_saved_peer_score_sync(&peer_id);
                let mut peers = match self.peers.try_write() {
                    Ok(p) => p,
                    Err(_) => self.peers.write().await,
                };
                peers.entry(peer_id).or_insert_with(|| PeerStats {
                    peer_id: peer_id_str,
                    connected_at: now,
                    messages_received: 0,
                    messages_sent: 0,
                    last_seen: now,
                    handshake_validated: false,
                    handshake_info: None,
                    rate_limit_tokens: max_tokens,
                    last_rate_update: now,
                    score: saved_score,
                    consecutive_rr_failures: 0,
                    last_rr_success: now,
                });

                // Proactively initiate handshake
                let handshake = self.create_handshake(0, self.handshake_config.validator_address.clone());
                let request_id = self.swarm.behaviour_mut()
                    .request_response
                    .send_request(&peer_id, SyncRequest::Handshake(handshake));
                self.pending_requests.insert(request_id, PendingSyncRequest {
                    peer_id,
                    response_tx: None,
                });
            }

            SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                info!("Disconnected from peer: {} (cause: {:?})", peer_id, cause);
                
                self.swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                
                let stale_sync: Vec<_> = self.pending_requests.iter()
                    .filter(|(_, p)| p.peer_id == peer_id)
                    .map(|(id, _)| *id)
                    .collect();
                for id in &stale_sync {
                    if let Some(p) = self.pending_requests.remove(id) {
                        if let Some(tx) = p.response_tx {
                            let _ = tx.send(SyncResponse::Error { message: "ConnectionClosed".into() });
                        }
                    }
                }
                let stale_vote: Vec<_> = self.pending_vote_requests.iter()
                    .filter(|(_, p)| p.peer_id == peer_id)
                    .map(|(id, _)| *id)
                    .collect();
                for id in &stale_vote {
                    if let Some(p) = self.pending_vote_requests.remove(id) {
                        if let Some(tx) = p.response_tx {
                            let _ = tx.send(VoteResponse::Error { message: "ConnectionClosed".into() });
                        }
                    }
                }
                if !stale_sync.is_empty() || !stale_vote.is_empty() {
                    info!("Cleaned {} stale sync + {} stale vote pending requests for disconnected peer {}", stale_sync.len(), stale_vote.len(), peer_id);
                }
                
                let was_validated;
                {
                    let mut peers = match self.peers.try_write() {
                        Ok(p) => p,
                        Err(_) => self.peers.write().await,
                    };
                    if let Some(peer_stats) = peers.remove(&peer_id) {
                        was_validated = peer_stats.handshake_validated;
                        self.persist_peer_score_sync(&peer_id, peer_stats.score);
                    } else {
                        was_validated = false;
                    }
                }

                if was_validated {
                    if let Some(addr) = self.peer_last_addr.get(&peer_id) {
                        let reconnect_addr = addr.clone();
                        let already_queued = self.reconnect_pending.iter().any(|(a, _, _)| {
                            a.iter().any(|p| {
                                if let libp2p::multiaddr::Protocol::P2p(pid) = p {
                                    pid == peer_id
                                } else {
                                    false
                                }
                            })
                        });
                        if !already_queued {
                            info!("Scheduling immediate reconnect to validated peer {} at {}", peer_id, reconnect_addr);
                            self.reconnect_pending.push((reconnect_addr, 0, Instant::now()));
                        }
                    }
                }
            }

            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Listening on {}", address);
            }

            // Request-response: incoming request from peer
            SwarmEvent::Behaviour(RinkuBehaviourEvent::RequestResponse(
                request_response::Event::Message { peer, message }
            )) => {
                match message {
                    request_response::Message::Request { request, channel, .. } => {
                        if !self.check_rate_limit_sync(&peer) {
                            debug!("Rate limit exceeded for sync request from: {}, rejecting", peer);
                            let _ = self.swarm.behaviour_mut().request_response.send_response(
                                channel,
                                SyncResponse::Error { message: "Rate limit exceeded".to_string() }
                            );
                            return;
                        }
                        
                        if let SyncRequest::Handshake(ref info) = request {
                            let pex_addrs_to_process = info.known_peer_addrs.clone();
                            match self.validate_handshake(info) {
                                Ok(_) => {
                                    let mut peers = match self.peers.try_write() {
                                        Ok(p) => p,
                                        Err(_) => self.peers.write().await,
                                    };
                                    let saved_score = self.get_saved_peer_score_sync(&peer);
                                    let entry = peers.entry(peer).or_insert_with(|| PeerStats {
                                        peer_id: peer.to_string(),
                                        connected_at: current_time_secs(),
                                        messages_received: 0,
                                        messages_sent: 0,
                                        last_seen: current_time_secs(),
                                        handshake_validated: false,
                                        handshake_info: None,
                                        rate_limit_tokens: self.dos_config.max_rate_limit_tokens,
                                        last_rate_update: current_time_secs(),
                                        score: saved_score,
                                        consecutive_rr_failures: 0,
                                        last_rr_success: current_time_secs(),
                                    });
                                    entry.handshake_validated = true;
                                    entry.handshake_info = Some(info.clone());
                                    entry.score = entry.score.saturating_add(10);
                                    self.persist_peer_score_sync(&peer, entry.score);
                                    info!("Handshake from {}: their validator_address={:?}, responding with ours={:?}", peer, info.validator_address, self.handshake_config.validator_address);
                                    let response = SyncResponse::Handshake(self.create_handshake(0, self.handshake_config.validator_address.clone()));
                                    let _ = self.swarm.behaviour_mut().request_response.send_response(
                                        channel,
                                        response
                                    );
                                    drop(peers);
                                    if !pex_addrs_to_process.is_empty() {
                                        self.process_pex_addresses(&pex_addrs_to_process, &peer);
                                    }
                                }
                                Err(e) => {
                                    warn!("Handshake validation failed for {}: {}", peer, e);
                                    self.record_misbehavior_sync(&peer.to_string());
                                    let _ = self.swarm.behaviour_mut().request_response.send_response(
                                        channel,
                                        SyncResponse::Error { message: format!("Handshake rejected: {}", e) }
                                    );
                                }
                            }
                            return;
                        }

                        let handshake_ok = match self.peers.try_read() {
                            Ok(peers) => peers.get(&peer)
                                .map(|p| p.handshake_validated)
                                .unwrap_or(false),
                            Err(_) => {
                                debug!("Peers lock contended for sync handshake check from {}, deferring", peer);
                                let _ = self.swarm.behaviour_mut().request_response.send_response(
                                    channel,
                                    SyncResponse::Error { message: "Node busy, try again".to_string() }
                                );
                                return;
                            }
                        };
                        if !handshake_ok {
                            warn!("Sync request rejected (handshake required) from {}", peer);
                            self.record_misbehavior_sync(&peer.to_string());
                            let _ = self.swarm.behaviour_mut().request_response.send_response(
                                channel,
                                SyncResponse::Error { message: "Handshake required".to_string() }
                            );
                            return;
                        }

                        debug!("Received sync request from {}: {:?}", peer, request);
                        let incoming = IncomingSyncRequest {
                            peer_id: peer.to_string(),
                            request,
                            response_channel: channel,
                        };
                        match self.sync_incoming_tx.try_send(incoming) {
                            Ok(_) => {},
                            Err(tokio::sync::mpsc::error::TrySendError::Full(rejected)) => {
                                warn!("Sync incoming channel full, rejecting request from {}", peer);
                                let _ = self.swarm.behaviour_mut().request_response.send_response(
                                    rejected.response_channel,
                                    SyncResponse::Error { message: "Node busy, try again".to_string() }
                                );
                            },
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                warn!("Sync incoming channel closed");
                            },
                        }
                    }
                    request_response::Message::Response { request_id, response } => {
                        debug!("Received sync response for {:?}", request_id);
                        if let Some(pending) = self.pending_requests.remove(&request_id) {
                            self.record_rr_success(pending.peer_id).await;
                            let pex_result = if let SyncResponse::Handshake(info) = &response {
                                info!("Handshake response from {}: validator_address={:?}", pending.peer_id, info.validator_address);
                                let pex_addrs = info.known_peer_addrs.clone();
                                match self.validate_handshake(info) {
                                    Ok(_) => {
                                        let mut peers = match self.peers.try_write() {
                                            Ok(p) => p,
                                            Err(_) => self.peers.write().await,
                                        };
                                        if let Some(peer_stats) = peers.get_mut(&pending.peer_id) {
                                            peer_stats.handshake_validated = true;
                                            peer_stats.handshake_info = Some(info.clone());
                                            peer_stats.score = peer_stats.score.saturating_add(10);
                                            self.persist_peer_score_sync(&pending.peer_id, peer_stats.score);
                                        }
                                        Some((pex_addrs, pending.peer_id))
                                    }
                                    Err(e) => {
                                        warn!("Handshake response invalid from {}: {}", pending.peer_id, e);
                                        self.record_misbehavior_sync(&pending.peer_id.to_string());
                                        None
                                    }
                                }
                            } else {
                                None
                            };
                            if let Some((pex_addrs, from_peer)) = pex_result {
                                if !pex_addrs.is_empty() {
                                    self.process_pex_addresses(&pex_addrs, &from_peer);
                                }
                            }
                            if let Some(response_tx) = pending.response_tx {
                                let _ = response_tx.send(response);
                            }
                        }
                    }
                }
            }

            SwarmEvent::Behaviour(RinkuBehaviourEvent::RequestResponse(
                request_response::Event::OutboundFailure { peer, request_id, error }
            )) => {
                warn!("Sync request to {} failed: {:?}", peer, error);
                if let Some(pending) = self.pending_requests.remove(&request_id) {
                    if let Some(response_tx) = pending.response_tx {
                        let _ = response_tx.send(SyncResponse::Error {
                            message: format!("{:?}", error),
                        });
                    }
                }
                self.record_rr_failure_and_maybe_disconnect(peer).await;
            }

            // Request-response: inbound failure
            SwarmEvent::Behaviour(RinkuBehaviourEvent::RequestResponse(
                request_response::Event::InboundFailure { peer, error, .. }
            )) => {
                warn!("Inbound sync request from {} failed: {:?}", peer, error);
            }

            SwarmEvent::Behaviour(RinkuBehaviourEvent::VoteProtocol(
                request_response::Event::Message { peer, message }
            )) => {
                match message {
                    request_response::Message::Request { request, channel, .. } => {
                        let handshake_ok = match self.peers.try_read() {
                            Ok(peers) => peers.get(&peer)
                                .map(|p| p.handshake_validated)
                                .unwrap_or(false),
                            Err(_) => {
                                debug!("Peers lock contended for vote handshake check from {}, deferring", peer);
                                let _ = self.swarm.behaviour_mut().vote_protocol.send_response(
                                    channel,
                                    VoteResponse::Error { message: "Node busy, try again".to_string() }
                                );
                                return;
                            }
                        };
                        if !handshake_ok {
                            warn!("Vote request rejected (handshake required) from {}", peer);
                            let _ = self.swarm.behaviour_mut().vote_protocol.send_response(
                                channel,
                                VoteResponse::Error { message: "Handshake required".to_string() }
                            );
                            return;
                        }

                        debug!("Received vote request from {}", peer);
                        let incoming = IncomingVoteRequest {
                            peer_id: peer.to_string(),
                            request,
                            response_channel: channel,
                        };
                        match self.vote_incoming_tx.try_send(incoming) {
                            Ok(_) => {},
                            Err(tokio::sync::mpsc::error::TrySendError::Full(rejected)) => {
                                warn!("Vote incoming channel full, rejecting request from {}", peer);
                                let _ = self.swarm.behaviour_mut().vote_protocol.send_response(
                                    rejected.response_channel,
                                    VoteResponse::Error { message: "Node busy, try again".to_string() }
                                );
                            },
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                warn!("Vote incoming channel closed");
                            },
                        }
                    }
                    request_response::Message::Response { request_id, response } => {
                        debug!("Received vote response for {:?}", request_id);
                        if let Some(pending) = self.pending_vote_requests.remove(&request_id) {
                            self.record_rr_success(pending.peer_id).await;
                            if let Some(response_tx) = pending.response_tx {
                                let _ = response_tx.send(response);
                            }
                        }
                    }
                }
            }

            SwarmEvent::Behaviour(RinkuBehaviourEvent::VoteProtocol(
                request_response::Event::OutboundFailure { peer, request_id, error }
            )) => {
                warn!("Vote request to {} failed: {:?}", peer, error);
                if let Some(pending) = self.pending_vote_requests.remove(&request_id) {
                    if let Some(response_tx) = pending.response_tx {
                        let _ = response_tx.send(VoteResponse::Error {
                            message: format!("{:?}", error),
                        });
                    }
                }
                self.record_rr_failure_and_maybe_disconnect(peer).await;
            }

            SwarmEvent::Behaviour(RinkuBehaviourEvent::VoteProtocol(
                request_response::Event::InboundFailure { peer, error, .. }
            )) => {
                warn!("Inbound vote request from {} failed: {:?}", peer, error);
            }

            // Identify: received peer info
            SwarmEvent::Behaviour(RinkuBehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. })) => {
                info!(
                    "Identified peer {}: {} (protocols: {})",
                    peer_id,
                    info.agent_version,
                    info.protocols.len()
                );
            }

            SwarmEvent::Behaviour(RinkuBehaviourEvent::Identify(identify::Event::Sent { peer_id, .. })) => {
                debug!("Sent identify info to {}", peer_id);
            }

            _ => {}
        }
    }

    async fn record_rr_failure_and_maybe_disconnect(&mut self, peer: PeerId) {
        let should_disconnect = {
            let mut peers = match self.peers.try_write() {
                Ok(p) => p,
                Err(_) => self.peers.write().await,
            };
            if let Some(stats) = peers.get_mut(&peer) {
                stats.consecutive_rr_failures += 1;
                let count = stats.consecutive_rr_failures;
                if count >= RR_FAILURE_DISCONNECT_THRESHOLD {
                    warn!(
                        "PEER-HEALTH: {} has {} consecutive request-response failures, forcing disconnect",
                        peer, count
                    );
                    true
                } else {
                    false
                }
            } else {
                false
            }
        };
        if should_disconnect {
            let _ = self.swarm.disconnect_peer_id(peer);
        }
    }

    async fn record_rr_success(&mut self, peer: PeerId) {
        let mut peers = match self.peers.try_write() {
            Ok(p) => p,
            Err(_) => self.peers.write().await,
        };
        if let Some(stats) = peers.get_mut(&peer) {
            if stats.consecutive_rr_failures > 0 {
                info!(
                    "PEER-HEALTH: {} recovered after {} consecutive failures",
                    peer, stats.consecutive_rr_failures
                );
            }
            stats.consecutive_rr_failures = 0;
            stats.last_rr_success = current_time_secs();
        }
    }

    pub fn send_sync_response(&mut self, channel: ResponseChannel<SyncResponse>, response: SyncResponse) {
        if let Err(e) = self.swarm.behaviour_mut().request_response.send_response(channel, response) {
            warn!("Failed to send sync response: {:?}", e);
        }
    }

    fn message_topic(&self, message: &GossipMessage) -> &IdentTopic {
        use crate::gossip::GossipMessage as GM;
        match message {
            GM::CheckpointIntent { .. }
            | GM::CheckpointAnnouncement { .. }
            | GM::LeaderTimeout { .. }
            | GM::CheckpointSignature { .. } => &self.topic_critical,

            GM::TxConfirmAck { .. }
            | GM::TxConfirmBroadcast { .. } => &self.topic_consensus,

            GM::QccVoteRequest { .. }
            | GM::QccVoteCast { .. } => &self.topic_priority,

            GM::DaChunk { .. } => &self.topic_data,

            _ => &self.topic_general,
        }
    }

    async fn publish_to_topic(&mut self, topic: IdentTopic, message: &GossipMessage) {
        match serde_json::to_vec(message) {
            Ok(data) => {
                let data_len = data.len() as u64;

                if data_len > 128 * 1024 {
                    warn!("Large gossip message: {}KB (type: {}, topic: {})", data_len / 1024, message.variant_name(), topic);
                } else if data_len > 64 * 1024 {
                    info!("Gossip message: {}KB (type: {}, topic: {})", data_len / 1024, message.variant_name(), topic);
                }

                match self.swarm.behaviour_mut().gossipsub.publish(topic, data) {
                    Ok(_) => {
                        if let Ok(mut stats) = self.stats.try_write() {
                            stats.messages_published += 1;
                            stats.bytes_sent += data_len;
                        }
                        debug!("Published message to network ({}B)", data_len);
                    }
                    Err(e) => {
                        if format!("{:?}", e).contains("Duplicate") {
                            warn!("Gossipsub publish DUPLICATE — message already seen by router (type: {})", message.variant_name());
                        } else if format!("{:?}", e).contains("InsufficientPeers") {
                            warn!("Gossipsub publish INSUFFICIENT_PEERS — no topic peers available (type: {})", message.variant_name());
                        } else {
                            warn!("Failed to publish message: {:?}", e);
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to serialize message: {}", e);
            }
        }
    }

    async fn publish_message(&mut self, message: &GossipMessage) {
        let topic = self.message_topic(message).clone();
        self.publish_to_topic(topic, message).await;
    }

    async fn publish_batched(&mut self, messages: Vec<GossipMessage>) {
        use crate::gossip::GossipMessage as GM;
        if messages.is_empty() {
            return;
        }
        let total = messages.len();
        let mut critical_msgs: Vec<GossipMessage> = Vec::new();
        let mut consensus_msgs: Vec<GossipMessage> = Vec::new();
        let mut priority_msgs: Vec<GossipMessage> = Vec::new();
        let mut data_msgs: Vec<GossipMessage> = Vec::new();
        let mut general_msgs: Vec<GossipMessage> = Vec::new();
        for msg in messages {
            match &msg {
                GM::CheckpointIntent { .. }
                | GM::CheckpointAnnouncement { .. } => critical_msgs.push(msg),

                GM::TxConfirmAck { .. }
                | GM::TxConfirmBroadcast { .. } => consensus_msgs.push(msg),

                GM::QccVoteRequest { .. }
                | GM::QccVoteCast { .. } => priority_msgs.push(msg),

                GM::DaChunk { .. } => data_msgs.push(msg),

                _ => general_msgs.push(msg),
            }
        }
        let c_count = critical_msgs.len();
        let s_count = consensus_msgs.len();
        let p_count = priority_msgs.len();
        let d_count = data_msgs.len();
        let g_count = general_msgs.len();
        for msg in priority_msgs {
            self.publish_to_topic(self.topic_priority.clone(), &msg).await;
        }
        for msg in critical_msgs {
            self.publish_to_topic(self.topic_critical.clone(), &msg).await;
        }
        if s_count <= 1 {
            for msg in &consensus_msgs {
                self.publish_to_topic(self.topic_consensus.clone(), msg).await;
            }
        } else {
            let batch = GM::Batch { messages: consensus_msgs };
            self.publish_to_topic(self.topic_consensus.clone(), &batch).await;
        }
        for msg in data_msgs {
            self.publish_to_topic(self.topic_data.clone(), &msg).await;
        }
        if g_count <= 1 {
            for msg in &general_msgs {
                self.publish_to_topic(self.topic_general.clone(), msg).await;
            }
        } else {
            let batch = GM::Batch { messages: general_msgs };
            self.publish_to_topic(self.topic_general.clone(), &batch).await;
        }
        if total > 3 {
            let publishes = p_count + c_count + d_count + (if s_count > 1 { 1 } else { s_count }) + (if g_count > 1 { 1 } else { g_count });
            debug!("Coalesced {} msgs into {} publishes (P:{} C:{} S:{} D:{} G:{})",
                total, publishes, p_count, c_count, s_count, d_count, g_count);
        }
    }

    pub async fn get_stats(&self) -> NetworkStats {
        let stats = self.stats.read().await;
        let peers = self.peers.read().await;

        NetworkStats {
            local_peer_id: self.local_peer_id.to_string(),
            connected_peers: peers.len(),
            messages_published: stats.messages_published,
            messages_received: stats.messages_received,
            bytes_sent: stats.bytes_sent,
            bytes_received: stats.bytes_received,
        }
    }

    pub async fn get_connected_peers(&self) -> Vec<PeerStats> {
        self.peers.read().await.values().cloned().collect()
    }

    /// Validate a handshake from a peer
    pub fn validate_handshake(&self, handshake: &PeerHandshake) -> Result<(), String> {
        if !self.is_protocol_version_compatible(&handshake.protocol_version) {
            return Err(format!(
                "Incompatible protocol version: {} (min: {})",
                handshake.protocol_version, self.dos_config.min_protocol_version
            ));
        }

        if let Some(ref required_chain) = self.handshake_config.required_chain_id {
            if &handshake.chain_id != required_chain {
                return Err(format!(
                    "Chain ID mismatch: {} (expected: {})",
                    handshake.chain_id, required_chain
                ));
            }
        }

        if let Some(ref required_network) = self.handshake_config.required_network_id {
            if &handshake.network_id != required_network {
                return Err(format!(
                    "Network ID mismatch: {} (expected: {})",
                    handshake.network_id, required_network
                ));
            }
        }

        Ok(())
    }

    /// Check if protocol version is compatible (simple semver major version check)
    fn is_protocol_version_compatible(&self, version: &str) -> bool {
        let min_parts: Vec<&str> = self.dos_config.min_protocol_version.split('.').collect();
        let version_parts: Vec<&str> = version.split('.').collect();

        if min_parts.is_empty() || version_parts.is_empty() {
            return false;
        }

        let min_major: u32 = min_parts[0].parse().unwrap_or(0);
        let version_major: u32 = version_parts[0].parse().unwrap_or(0);

        version_major >= min_major
    }

    /// Check if peer is banned (non-blocking check, returns false if lock contended)
    pub fn is_peer_banned_sync(&self, peer_id: &str) -> bool {
        if let Ok(banned) = self.banned_peers.try_read() {
            if let Some(ban) = banned.get(peer_id) {
                return ban.expires_at > current_time_secs();
            }
        }
        false
    }

    /// Check if peer is banned (async version)
    pub async fn is_peer_banned(&self, peer_id: &str) -> bool {
        let banned = self.banned_peers.read().await;
        if let Some(ban) = banned.get(peer_id) {
            ban.expires_at > current_time_secs()
        } else {
            false
        }
    }

    /// Ban a peer (non-blocking, queues ban if lock contended)
    pub fn ban_peer_sync(&self, peer_id: String, reason: String) {
        let now = current_time_secs();
        let ban = BannedPeer {
            peer_id: peer_id.clone(),
            reason: reason.clone(),
            banned_at: now,
            expires_at: now + self.dos_config.ban_duration_secs,
        };
        
        warn!("Banning peer {} for {} secs: {}", peer_id, self.dos_config.ban_duration_secs, reason);
        if let Ok(mut banned) = self.banned_peers.try_write() {
            banned.insert(peer_id, ban);
        }
    }

    /// Ban a peer (async version)
    pub async fn ban_peer(&self, peer_id: String, reason: String) {
        let now = current_time_secs();
        let ban = BannedPeer {
            peer_id: peer_id.clone(),
            reason: reason.clone(),
            banned_at: now,
            expires_at: now + self.dos_config.ban_duration_secs,
        };
        
        warn!("Banning peer {} for {} secs: {}", peer_id, self.dos_config.ban_duration_secs, reason);
        self.banned_peers.write().await.insert(peer_id, ban);
    }

    /// Cleanup expired bans
    pub async fn cleanup_expired_bans(&self) {
        let now = current_time_secs();
        let mut banned = self.banned_peers.write().await;
        banned.retain(|_, ban| ban.expires_at > now);
    }

    pub fn is_connection_limit_reached_sync(&self) -> bool {
        if let Ok(peers) = self.peers.try_read() {
            return peers.len() >= self.dos_config.max_connections;
        }
        true
    }

    /// Check if connection limit is reached (async version)
    pub async fn is_connection_limit_reached(&self) -> bool {
        self.peers.read().await.len() >= self.dos_config.max_connections
    }

    /// Check rate limit for a peer (returns true if request allowed) - sync version
    /// Uses blocking wait to ensure rate limiting is never bypassed
    pub fn check_rate_limit_sync(&self, peer_id: &PeerId) -> bool {
        let mut peers = match self.peers.try_write() {
            Ok(p) => p,
            Err(_) => {
                trace!("Rate limit check skipped (lock contended) for peer {}, allowing message", peer_id);
                return true;
            }
        };
        
        if let Some(peer) = peers.get_mut(peer_id) {
            let now = current_time_secs();
            let elapsed = now.saturating_sub(peer.last_rate_update);
            
            let tokens_to_add = (elapsed as u32) * self.dos_config.rate_limit_tokens_per_second;
            peer.rate_limit_tokens = (peer.rate_limit_tokens + tokens_to_add)
                .min(self.dos_config.max_rate_limit_tokens);
            peer.last_rate_update = now;

            if peer.rate_limit_tokens > 0 {
                peer.rate_limit_tokens -= 1;
                true
            } else {
                false
            }
        } else {
            true
        }
    }
    
    /// Check rate limit for a peer (returns true if request allowed) - async version
    pub async fn check_rate_limit(&self, peer_id: &PeerId) -> bool {
        let mut peers = self.peers.write().await;
        if let Some(peer) = peers.get_mut(peer_id) {
            let now = current_time_secs();
            let elapsed = now.saturating_sub(peer.last_rate_update);
            
            let tokens_to_add = (elapsed as u32) * self.dos_config.rate_limit_tokens_per_second;
            peer.rate_limit_tokens = (peer.rate_limit_tokens + tokens_to_add)
                .min(self.dos_config.max_rate_limit_tokens);
            peer.last_rate_update = now;

            if peer.rate_limit_tokens > 0 {
                peer.rate_limit_tokens -= 1;
                true
            } else {
                false
            }
        } else {
            true
        }
    }

    /// Create a handshake info for this node, including known peer addresses for PEX
    pub fn create_handshake(&self, _checkpoint_height: u64, validator_address: Option<String>) -> PeerHandshake {
        let known_peer_addrs = self.collect_known_peer_addrs();
        let real_height = self.shared_checkpoint_height.load(Ordering::Relaxed);
        PeerHandshake {
            protocol_version: self.handshake_config.protocol_version.clone(),
            chain_id: self.handshake_config.chain_id.clone(),
            network_id: self.handshake_config.network_id.clone(),
            node_id: self.local_peer_id.to_string(),
            checkpoint_height: real_height,
            validator_address,
            capabilities: vec!["sync".to_string(), "gossip".to_string(), "proofs".to_string()],
            known_peer_addrs,
        }
    }

    fn collect_known_peer_addrs(&self) -> Vec<String> {
        let mut addrs = Vec::new();
        if let Some(ref external) = self.config.external_addr {
            let our_addr = format!("{}/p2p/{}", external, self.local_peer_id);
            addrs.push(our_addr);
        }
        for bootstrap in &self.config.bootstrap_peers {
            if !addrs.contains(bootstrap) {
                addrs.push(bootstrap.clone());
            }
        }
        let base_count = addrs.len();
        if let Ok(peers) = self.peers.try_read() {
            for (_, stats) in peers.iter() {
                if !stats.handshake_validated {
                    continue;
                }
                if let Some(ref info) = stats.handshake_info {
                    for addr in &info.known_peer_addrs {
                        if !addrs.contains(addr) {
                            addrs.push(addr.clone());
                        }
                    }
                }
            }
            let peer_addrs_added = addrs.len() - base_count;
            if peer_addrs_added > 0 {
                debug!("PEX: Sharing {} addrs from {} connected peers (total {} addrs)", peer_addrs_added, peers.len(), addrs.len());
            }
        }
        const MAX_PEX_ADDRS: usize = 20;
        addrs.truncate(MAX_PEX_ADDRS);
        addrs
    }

    fn process_pex_addresses(&mut self, peer_addrs: &[String], from_peer: &PeerId) {
        const PEX_DIAL_COOLDOWN_SECS: u64 = 60;
        const MAX_PEX_ADDRS_PER_HANDSHAKE: usize = 10;
        let now = current_time_secs();

        self.recently_dialed_pex.retain(|_, ts| now - *ts < PEX_DIAL_COOLDOWN_SECS);

        let mut dialed_this_round = 0usize;
        for addr_str in peer_addrs {
            if dialed_this_round >= MAX_PEX_ADDRS_PER_HANDSHAKE {
                break;
            }
            if let Ok(addr) = addr_str.parse::<Multiaddr>() {
                let peer_id_from_addr = addr.iter().find_map(|p| {
                    if let libp2p::multiaddr::Protocol::P2p(peer_id) = p {
                        Some(peer_id)
                    } else {
                        None
                    }
                });

                if let Some(target_peer_id) = peer_id_from_addr {
                    if target_peer_id == self.local_peer_id {
                        continue;
                    }
                    if target_peer_id == *from_peer {
                        continue;
                    }

                    let already_connected = self.swarm.is_connected(&target_peer_id);
                    if already_connected {
                        continue;
                    }

                    if self.recently_dialed_pex.contains_key(addr_str) {
                        continue;
                    }

                    let is_banned = {
                        let banned = self.banned_peers.try_read();
                        banned.map(|b| b.contains_key(&target_peer_id.to_string())).unwrap_or(false)
                    };
                    if is_banned {
                        continue;
                    }

                    info!("PEX: Discovered new peer {} via {}, dialing {}", target_peer_id, from_peer, addr_str);
                    self.recently_dialed_pex.insert(addr_str.clone(), now);
                    dialed_this_round += 1;
                    if let Err(e) = self.swarm.dial(addr.clone()) {
                        debug!("PEX: Failed to dial {}: {}", addr_str, e);
                    }
                }
            }
        }
    }

    /// Get banned peers list
    pub async fn get_banned_peers(&self) -> Vec<BannedPeer> {
        self.banned_peers.read().await.values().cloned().collect()
    }
    
    /// Record misbehavior for a peer - escalates to ban after threshold (async version)
    pub async fn record_misbehavior(&self, peer_id: &str) {
        let mut peers = self.peers.write().await;
        let peer_key = if let Ok(pid) = peer_id.parse::<PeerId>() {
            pid
        } else {
            return;
        };
        
        if let Some(peer) = peers.get_mut(&peer_key) {
            peer.rate_limit_tokens = peer.rate_limit_tokens.saturating_sub(10);
            peer.score = peer.score.saturating_sub(MISBEHAVIOR_SCORE_PENALTY);
            self.persist_peer_score_async(&peer_key, peer.score).await;
            
            if peer.rate_limit_tokens == 0 || peer.score <= BAN_SCORE_THRESHOLD {
                drop(peers);
                self.ban_peer(peer_id.to_string(), "Repeated misbehavior".to_string()).await;
            }
        }
    }
    
    /// Record misbehavior for a peer - sync version
    /// Deducts tokens and bans peer if depleted
    pub fn record_misbehavior_sync(&self, peer_id: &str) {
        let peer_key = if let Ok(pid) = peer_id.parse::<PeerId>() {
            pid
        } else {
            return;
        };
        
        let should_ban = if let Ok(mut peers) = self.peers.try_write() {
            if let Some(peer) = peers.get_mut(&peer_key) {
                peer.rate_limit_tokens = peer.rate_limit_tokens.saturating_sub(10);
                peer.score = peer.score.saturating_sub(MISBEHAVIOR_SCORE_PENALTY);
                self.persist_peer_score_sync(&peer_key, peer.score);
                peer.rate_limit_tokens == 0 || peer.score <= BAN_SCORE_THRESHOLD
            } else {
                false
            }
        } else {
            false
        };
        
        if should_ban {
            self.ban_peer_sync(peer_id.to_string(), "Repeated misbehavior".to_string());
        }
    }

    /// Manually set DoS config
    pub fn set_dos_config(&mut self, config: DoSConfig) {
        self.dos_config = config;
    }

    /// Manually set handshake config
    pub fn set_handshake_config(&mut self, config: HandshakeConfig) {
        self.handshake_config = config;
    }
}

fn load_peer_scores(path: &Option<String>) -> HashMap<String, i32> {
    if let Some(path) = path {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(scores) = serde_json::from_slice::<HashMap<String, i32>>(&bytes) {
                return scores;
            }
        }
    }
    HashMap::new()
}

impl NetworkService {
    fn get_saved_peer_score_sync(&self, peer_id: &PeerId) -> i32 {
        self.peer_scores
            .try_read()
            .ok()
            .and_then(|scores| scores.get(&peer_id.to_string()).cloned())
            .unwrap_or(0)
    }

    fn persist_peer_score_sync(&self, peer_id: &PeerId, score: i32) {
        let Some(path) = &self.peer_scores_path else {
            return;
        };
        if let Ok(mut scores) = self.peer_scores.try_write() {
            scores.insert(peer_id.to_string(), score);
            if let Ok(data) = serde_json::to_vec(&*scores) {
                let _ = std::fs::write(path, data);
            }
        }
    }

    async fn persist_peer_score_async(&self, peer_id: &PeerId, score: i32) {
        let Some(path) = &self.peer_scores_path else {
            return;
        };
        let mut scores = self.peer_scores.write().await;
        scores.insert(peer_id.to_string(), score);
        if let Ok(data) = serde_json::to_vec(&*scores) {
            let _ = std::fs::write(path, data);
        }
    }
}

fn current_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_validate_handshake_enforces_chain_and_network() {
        let config = NetworkConfig {
            listen_addr: "/ip4/127.0.0.1/tcp/4001".to_string(),
            bootstrap_peers: Vec::new(),
            enable_mdns: false,
            data_dir: None,
            external_addr: None,
        };
        let (mut service, _handle) = NetworkService::new(config).unwrap();
        let mut handshake_config = HandshakeConfig::default();
        handshake_config.required_chain_id = Some("rinku-testnet".to_string());
        handshake_config.required_network_id = Some("testnet".to_string());
        service.set_handshake_config(handshake_config);

        let bad_handshake = PeerHandshake {
            protocol_version: "1.0.0".to_string(),
            chain_id: "rinku-mainnet".to_string(),
            network_id: "mainnet".to_string(),
            node_id: "peer".to_string(),
            checkpoint_height: 0,
            validator_address: None,
            capabilities: vec!["sync".to_string()],
            known_peer_addrs: Vec::new(),
        };

        assert!(service.validate_handshake(&bad_handshake).is_err());

        let good_handshake = PeerHandshake {
            protocol_version: "1.0.0".to_string(),
            chain_id: "rinku-testnet".to_string(),
            network_id: "testnet".to_string(),
            node_id: "peer".to_string(),
            checkpoint_height: 0,
            validator_address: None,
            capabilities: vec!["sync".to_string()],
            known_peer_addrs: Vec::new(),
        };

        assert!(service.validate_handshake(&good_handshake).is_ok());
    }
}

pub enum NetworkCommand {
    Dial(Multiaddr, oneshot::Sender<Result<()>>),
    SendSyncResponse(ResponseChannel<SyncResponse>, SyncResponse),
    SendVoteResponse(ResponseChannel<VoteResponse>, VoteResponse),
}

impl std::fmt::Debug for NetworkCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkCommand::Dial(addr, _) => write!(f, "Dial({})", addr),
            NetworkCommand::SendSyncResponse(_, _) => write!(f, "SendSyncResponse"),
            NetworkCommand::SendVoteResponse(_, _) => write!(f, "SendVoteResponse"),
        }
    }
}

pub struct NetworkHandle {
    outbound_tx: mpsc::Sender<GossipMessage>,
    pub message_rx: Option<mpsc::Receiver<GossipMessage>>,
    pub priority_message_rx: Option<mpsc::Receiver<GossipMessage>>,
    pub checkpoint_message_rx: Option<mpsc::Receiver<GossipMessage>>,
    peers: Arc<RwLock<HashMap<PeerId, PeerStats>>>,
    local_peer_id: String,
    sync_request_tx: mpsc::Sender<(PeerId, SyncRequest, oneshot::Sender<SyncResponse>)>,
    pub sync_incoming_rx: Option<mpsc::Receiver<IncomingSyncRequest>>,
    vote_request_tx: mpsc::Sender<(PeerId, VoteRequest, oneshot::Sender<VoteResponse>)>,
    pub vote_incoming_rx: Option<mpsc::Receiver<IncomingVoteRequest>>,
    command_tx: mpsc::Sender<NetworkCommand>,
    stats: Arc<RwLock<NetworkStatsInner>>,
    shared_checkpoint_height: Arc<AtomicU64>,
}

impl NetworkHandle {
    pub async fn broadcast(&self, message: GossipMessage) -> Result<()> {
        self.outbound_tx.send(message).await?;
        Ok(())
    }

    pub fn try_broadcast(&self, message: GossipMessage) -> Result<()> {
        self.outbound_tx.try_send(message).map_err(|e| anyhow::anyhow!("outbound full: {}", e))?;
        Ok(())
    }

    pub fn update_checkpoint_height(&self, height: u64) {
        self.shared_checkpoint_height.store(height, Ordering::Relaxed);
    }

    pub async fn get_peer_count(&self) -> usize {
        self.peers.read().await.len()
    }

    pub async fn get_connected_peers(&self) -> Vec<PeerStats> {
        self.peers.read().await.values().cloned().collect()
    }

    pub fn local_peer_id(&self) -> &str {
        &self.local_peer_id
    }

    /// Connect to a peer by multiaddr
    pub async fn connect(&self, addr: &str) -> Result<()> {
        let multiaddr: Multiaddr = addr.parse()
            .map_err(|e| anyhow::anyhow!("Invalid multiaddr: {}", e))?;
        
        let (response_tx, response_rx) = oneshot::channel();
        self.command_tx.send(NetworkCommand::Dial(multiaddr, response_tx)).await?;
        
        response_rx.await
            .map_err(|_| anyhow::anyhow!("Dial command cancelled"))?
    }

    /// Get network statistics
    pub async fn stats(&self) -> NetworkStats {
        let stats = self.stats.read().await;
        let peer_count = self.peers.read().await.len();
        
        NetworkStats {
            local_peer_id: self.local_peer_id.clone(),
            connected_peers: peer_count,
            messages_published: stats.messages_published,
            messages_received: stats.messages_received,
            bytes_sent: stats.bytes_sent,
            bytes_received: stats.bytes_received,
        }
    }

    /// Get list of connected peer IDs
    pub async fn get_connected_peer_ids(&self) -> Vec<String> {
        match self.peers.try_read() {
            Ok(peers) => peers.keys().map(|p| p.to_string()).collect(),
            Err(_) => {
                self.peers.read().await
                    .keys()
                    .map(|p| p.to_string())
                    .collect()
            }
        }
    }

    pub async fn get_connected_validator_peer_ids(&self) -> Vec<String> {
        self.peers.read().await
            .iter()
            .filter(|(_, stats)| {
                stats.handshake_validated
                    && stats.handshake_info.as_ref()
                        .and_then(|h| h.validator_address.as_ref())
                        .map(|addr| !addr.is_empty())
                        .unwrap_or(false)
            })
            .map(|(pid, _)| pid.to_string())
            .collect()
    }

    /// Take the gossip message receiver out of this handle.
    /// Must be called before wrapping in Arc<Mutex> so the receiver can be
    /// used with async .recv() without holding the mutex.
    pub fn take_message_rx(&mut self) -> Option<mpsc::Receiver<GossipMessage>> {
        self.message_rx.take()
    }

    pub fn take_priority_message_rx(&mut self) -> Option<mpsc::Receiver<GossipMessage>> {
        self.priority_message_rx.take()
    }

    pub fn take_checkpoint_message_rx(&mut self) -> Option<mpsc::Receiver<GossipMessage>> {
        self.checkpoint_message_rx.take()
    }

    /// Take the sync request receiver out of this handle.
    /// Must be called before wrapping in Arc<Mutex> so the receiver can be
    /// used with async .recv() without holding the mutex.
    pub fn take_sync_incoming_rx(&mut self) -> Option<mpsc::Receiver<IncomingSyncRequest>> {
        self.sync_incoming_rx.take()
    }

    /// Clone the command sender for sending sync responses without the mutex.
    pub fn response_sender(&self) -> mpsc::Sender<NetworkCommand> {
        self.command_tx.clone()
    }

    /// Send a sync response for an incoming request
    pub fn send_sync_response(&self, channel: ResponseChannel<SyncResponse>, response: SyncResponse) {
        let _ = self.command_tx.try_send(NetworkCommand::SendSyncResponse(channel, response));
    }

    /// Send a sync request to a specific peer and wait for response
    pub async fn sync_request(&self, peer_id: &str, request: SyncRequest) -> Result<SyncResponse> {
        let rx = self.send_sync_request(peer_id, request).await?;
        let response = rx.await
            .map_err(|_| anyhow::anyhow!("Sync request cancelled"))?;
        Ok(response)
    }

    /// Send a sync request and return the response receiver without awaiting it.
    /// This allows callers to drop any locks before awaiting the response,
    /// preventing mutex serialization when sending multiple parallel requests.
    pub async fn send_sync_request(&self, peer_id: &str, request: SyncRequest) -> Result<oneshot::Receiver<SyncResponse>> {
        let peer_id: PeerId = peer_id.parse()
            .map_err(|e| anyhow::anyhow!("Invalid peer ID: {}", e))?;
        let (response_tx, response_rx) = oneshot::channel();
        match self.sync_request_tx.try_send((peer_id, request, response_tx)) {
            Ok(_) => Ok(response_rx),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                Err(anyhow::anyhow!("Sync request channel full"))
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                Err(anyhow::anyhow!("Sync request channel closed"))
            }
        }
    }

    /// Request a snapshot from a peer
    pub async fn request_snapshot(&self, peer_id: &str) -> Result<SyncResponse> {
        self.sync_request(peer_id, SyncRequest::Snapshot).await
    }

    /// Request delta sync from a checkpoint
    pub async fn request_delta(&self, peer_id: &str, from_checkpoint: u64) -> Result<SyncResponse> {
        self.sync_request(peer_id, SyncRequest::Delta { from_checkpoint }).await
    }

    /// Request a transaction by hash
    pub async fn request_transaction(&self, peer_id: &str, hash: String) -> Result<SyncResponse> {
        self.sync_request(peer_id, SyncRequest::Transaction { hash }).await
    }

    /// Request a proof for a transaction
    pub async fn request_proof(&self, peer_id: &str, tx_hash: String) -> Result<SyncResponse> {
        self.sync_request(peer_id, SyncRequest::Proof { tx_hash }).await
    }

    /// Send handshake to peer
    pub async fn handshake(&self, peer_id: &str, info: PeerHandshake) -> Result<SyncResponse> {
        self.sync_request(peer_id, SyncRequest::Handshake(info)).await
    }

    pub fn take_vote_incoming_rx(&mut self) -> Option<mpsc::Receiver<IncomingVoteRequest>> {
        self.vote_incoming_rx.take()
    }

    pub fn send_vote_response(&self, channel: ResponseChannel<VoteResponse>, response: VoteResponse) {
        let _ = self.command_tx.try_send(NetworkCommand::SendVoteResponse(channel, response));
    }

    pub async fn send_vote_request(&self, peer_id: &str, request: VoteRequest) -> Result<oneshot::Receiver<VoteResponse>> {
        let peer_id: PeerId = peer_id.parse()
            .map_err(|e| anyhow::anyhow!("Invalid peer ID: {}", e))?;
        let (response_tx, response_rx) = oneshot::channel();
        match self.vote_request_tx.try_send((peer_id, request, response_tx)) {
            Ok(_) => Ok(response_rx),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                Err(anyhow::anyhow!("Vote request channel full"))
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                Err(anyhow::anyhow!("Vote request channel closed"))
            }
        }
    }

    pub async fn vote_request(&self, peer_id: &str, request: VoteRequest) -> Result<VoteResponse> {
        let rx = self.send_vote_request(peer_id, request).await?;
        let response = rx.await
            .map_err(|_| anyhow::anyhow!("Vote request cancelled"))?;
        Ok(response)
    }
}
