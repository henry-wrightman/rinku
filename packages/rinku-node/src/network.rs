#![cfg(feature = "p2p")]

use anyhow::Result;
use libp2p::futures::StreamExt;
use libp2p::{
    gossipsub::{self, IdentTopic, MessageAuthenticity, ValidationMode},
    identity::Keypair,
    identify,
    mdns,
    noise,
    request_response::{self, cbor, OutboundRequestId, ProtocolSupport, ResponseChannel},
    swarm::SwarmEvent,
    tcp, yamux, Multiaddr, PeerId, Swarm, StreamProtocol,
};
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, info, trace, warn};

use crate::gossip::GossipMessage;

const PROTOCOL_TOPIC: &str = "rinku/1.0.0";
const SYNC_PROTOCOL: &str = "/rinku/sync/1.0.0";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const MESH_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(10);
const MIN_MESH_PEERS: usize = 1;

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
    /// Request a checkpoint vote from a validator peer
    CheckpointVote(CheckpointVoteRequest),
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
    /// Error response
    Error { message: String },
    /// Checkpoint vote response from a validator
    CheckpointVote(Option<CheckpointVoteResponse>),
}

/// Response containing a validator's checkpoint signature
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointVoteResponse {
    /// Validator address that signed
    pub validator_address: String,
    /// Base64-encoded BLS signature
    pub signature: String,
    /// Raw signature bytes (for aggregation)
    pub signature_bytes: Vec<u8>,
    /// Base64-encoded BLS public key
    pub bls_public_key: String,
    /// Validator's stake weight
    pub stake: f64,
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
    /// Validators for bidirectional validator sync
    #[serde(default)]
    pub validators: Vec<ValidatorData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountData {
    pub address: String,
    pub balance: f64,
    pub nonce: u64,
    pub stake: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorData {
    pub address: String,
    pub stake: f64,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionData {
    pub hash: String,
    pub from: String,
    pub to: String,
    pub amount: f64,
    pub nonce: u64,
    pub timestamp: u64,
    pub signature: String,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub gas_price: f64,
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
    pub request_response: cbor::Behaviour<SyncRequest, SyncResponse>,
    pub identify: identify::Behaviour,
    pub mdns: mdns::tokio::Behaviour,
}

#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub listen_addr: String,
    pub bootstrap_peers: Vec<String>,
    pub enable_mdns: bool,
    pub data_dir: Option<String>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            listen_addr: "/ip4/0.0.0.0/tcp/4001".to_string(),
            bootstrap_peers: Vec::new(),
            enable_mdns: true,
            data_dir: None,
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
            rate_limit_tokens_per_second: 100,  // Increased 10x for testnet activity bots
            max_rate_limit_tokens: 1000,         // Increased 10x burst capacity
            ban_duration_secs: 30,               // Reduced ban time for testnet
            min_protocol_version: "1.0.0".to_string(),
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
}

impl Default for HandshakeConfig {
    fn default() -> Self {
        Self {
            protocol_version: "1.0.0".to_string(),
            chain_id: "rinku-mainnet".to_string(),
            network_id: "mainnet".to_string(),
            required_chain_id: None,
            required_network_id: None,
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
    topic: IdentTopic,
    config: NetworkConfig,
    stats: Arc<RwLock<NetworkStatsInner>>,
    message_tx: mpsc::Sender<GossipMessage>,
    outbound_rx: mpsc::Receiver<GossipMessage>,
    peers: Arc<RwLock<HashMap<PeerId, PeerStats>>>,
    /// Channel for outbound sync requests
    sync_request_rx: mpsc::Receiver<(PeerId, SyncRequest, oneshot::Sender<SyncResponse>)>,
    /// Channel for incoming sync requests (to be handled by application)
    sync_incoming_tx: mpsc::Sender<IncomingSyncRequest>,
    /// Pending requests awaiting responses
    pending_requests: HashMap<OutboundRequestId, PendingSyncRequest>,
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
}

struct NetworkStatsInner {
    messages_published: u64,
    messages_received: u64,
    bytes_sent: u64,
    bytes_received: u64,
}

impl NetworkService {
    pub fn new(config: NetworkConfig) -> Result<(Self, NetworkHandle)> {
        let local_key = load_or_generate_keypair(config.data_dir.as_deref());
        let local_peer_id = PeerId::from(local_key.public());

        info!("Local peer id: {}", local_peer_id);

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(HEARTBEAT_INTERVAL)
            .validation_mode(ValidationMode::Strict)
            .message_id_fn(|msg| {
                use sha2::{Sha256, Digest};
                let mut hasher = Sha256::new();
                hasher.update(&msg.data);
                let hash = hasher.finalize();
                gossipsub::MessageId::from(hex::encode(&hash[..16]))
            })
            // Increased from 64KB to 2MB to handle checkpoint announcements with thousands of finalized tx hashes
            // Each tx hash is ~64 chars, so 10k transactions = ~640KB. 2MB provides headroom for growth.
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
        let sync_protocol = StreamProtocol::new(SYNC_PROTOCOL);
        let request_response = cbor::Behaviour::new(
            [(sync_protocol, ProtocolSupport::Full)],
            request_response::Config::default()
                .with_request_timeout(Duration::from_secs(30)),
        );

        // Identify protocol for peer info exchange
        let identify = identify::Behaviour::new(
            identify::Config::new("/rinku/1.0.0".to_string(), local_key.public())
                .with_agent_version(format!("rinku-node/{}", env!("CARGO_PKG_VERSION"))),
        );

        let behaviour = RinkuBehaviour { 
            gossipsub, 
            request_response,
            identify,
            mdns,
        };

        let swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|_| behaviour)?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        let topic = IdentTopic::new(PROTOCOL_TOPIC);

        let (message_tx, message_rx) = mpsc::channel(1000);
        let (outbound_tx, outbound_rx) = mpsc::channel(1000);
        // Sync request channels
        let (sync_request_tx, sync_request_rx) = mpsc::channel(100);
        let (sync_incoming_tx, sync_incoming_rx) = mpsc::channel(100);
        // Command channel for dial requests etc
        let (command_tx, command_rx) = mpsc::channel(100);

        let stats = Arc::new(RwLock::new(NetworkStatsInner {
            messages_published: 0,
            messages_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
        }));

        let peers = Arc::new(RwLock::new(HashMap::new()));

        let peer_scores_path = config.data_dir.as_ref().map(|d| format!("{}/peer_scores.json", d));
        let peer_scores = Arc::new(RwLock::new(load_peer_scores(&peer_scores_path)));

        let handle = NetworkHandle {
            outbound_tx,
            message_rx,
            peers: peers.clone(),
            local_peer_id: local_peer_id.to_string(),
            sync_request_tx,
            sync_incoming_rx,
            command_tx,
            stats: stats.clone(),
        };

        let service = Self {
            local_peer_id,
            swarm,
            topic,
            config,
            stats,
            message_tx,
            outbound_rx,
            peers,
            sync_request_rx,
            sync_incoming_tx,
            pending_requests: HashMap::new(),
            dos_config: DoSConfig::default(),
            handshake_config: HandshakeConfig::default(),
            banned_peers: Arc::new(RwLock::new(HashMap::new())),
            peer_scores,
            peer_scores_path,
            command_rx,
        };

        Ok((service, handle))
    }

    pub fn local_peer_id(&self) -> String {
        self.local_peer_id.to_string()
    }

    pub async fn start(&mut self) -> Result<()> {
        let listen_addr: Multiaddr = self.config.listen_addr.parse()?;
        self.swarm.listen_on(listen_addr)?;

        self.swarm.behaviour_mut().gossipsub.subscribe(&self.topic)?;

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

    async fn run_event_loop(&mut self) -> Result<()> {
        let mut mesh_maintenance_interval = tokio::time::interval(MESH_MAINTENANCE_INTERVAL);
        mesh_maintenance_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        
        loop {
            tokio::select! {
                Some(msg) = self.outbound_rx.recv() => {
                    self.publish_message(&msg).await;
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
                Some(cmd) = self.command_rx.recv() => {
                    self.handle_command(cmd).await;
                }
                _ = mesh_maintenance_interval.tick() => {
                    self.perform_mesh_maintenance().await;
                }
                event = self.swarm.select_next_some() => {
                    self.handle_swarm_event(event).await;
                }
            }
        }
    }
    
    async fn perform_mesh_maintenance(&mut self) {
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
    }

    async fn handle_command(&mut self, cmd: NetworkCommand) {
        match cmd {
            NetworkCommand::Dial(addr, response_tx) => {
                debug!("Dialing peer at {}", addr);
                let result = self.swarm.dial(addr.clone())
                    .map_err(|e| anyhow::anyhow!("Dial error: {}", e));
                let _ = response_tx.send(result);
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

                {
                    let mut stats = self.stats.write().await;
                    stats.messages_received += 1;
                    stats.bytes_received += message.data.len() as u64;
                }

                if !self.check_rate_limit_sync(&propagation_source) {
                    // Just drop the message, don't penalize - high load is not misbehavior
                    debug!("Rate limit exceeded for peer: {}, dropping message", propagation_source);
                    return;
                }

                let allow_message = {
                    let peers = self.peers.read().await;
                    peers.get(&propagation_source)
                        .map(|p| p.handshake_validated)
                        .unwrap_or(false)
                };
                if !allow_message {
                    warn!("Dropping gossip from unhandshaked peer: {}", propagation_source);
                    self.record_misbehavior_sync(&propagation_source.to_string());
                    let _ = self.swarm.disconnect_peer_id(propagation_source);
                    return;
                }

                if let Ok(gossip_msg) = serde_json::from_slice::<GossipMessage>(&message.data) {
                    let _ = self.message_tx.send(gossip_msg).await;
                }

                {
                    let mut peers = self.peers.write().await;
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
                    let mut peer_map = self.peers.write().await;
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
                    });
                }
            }

            SwarmEvent::Behaviour(RinkuBehaviourEvent::Mdns(mdns::Event::Expired(peers))) => {
                for (peer_id, _) in peers {
                    debug!("mDNS peer expired: {}", peer_id);
                    self.swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                    
                    let mut peer_map = self.peers.write().await;
                    peer_map.remove(&peer_id);
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

                info!("Connected to peer: {} via {:?}", peer_id, endpoint);
                
                // Add peer to GossipSub mesh immediately upon connection
                // This is critical for non-mDNS environments (e.g., fly.io)
                self.swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);
                
                let now = current_time_secs();
                let max_tokens = self.dos_config.max_rate_limit_tokens;
                let saved_score = self.get_saved_peer_score_sync(&peer_id);
                let mut peers = self.peers.write().await;
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
                });

                // Proactively initiate handshake
                let handshake = self.create_handshake(0, None);
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
                
                // Remove from GossipSub mesh
                self.swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);
                
                let mut peers = self.peers.write().await;
                if let Some(peer_stats) = peers.remove(&peer_id) {
                    self.persist_peer_score_sync(&peer_id, peer_stats.score);
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
                            // Just reject request, don't penalize - high load is not misbehavior
                            debug!("Rate limit exceeded for sync request from: {}, rejecting", peer);
                            let _ = self.swarm.behaviour_mut().request_response.send_response(
                                channel,
                                SyncResponse::Error { message: "Rate limit exceeded".to_string() }
                            );
                            return;
                        }
                        
                        if let SyncRequest::Handshake(ref info) = request {
                            match self.validate_handshake(info) {
                                Ok(_) => {
                                    let mut peers = self.peers.write().await;
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
                                    });
                                    entry.handshake_validated = true;
                                    entry.handshake_info = Some(info.clone());
                                    entry.score = entry.score.saturating_add(10);
                                    self.persist_peer_score_sync(&peer, entry.score);
                                    let response = SyncResponse::Handshake(self.create_handshake(0, None));
                                    let _ = self.swarm.behaviour_mut().request_response.send_response(
                                        channel,
                                        response
                                    );
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

                        let handshake_ok = {
                            let peers = self.peers.read().await;
                            peers.get(&peer)
                                .map(|p| p.handshake_validated)
                                .unwrap_or(false)
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
                        if let Err(e) = self.sync_incoming_tx.send(incoming).await {
                            warn!("Failed to forward sync request: {}", e);
                        }
                    }
                    request_response::Message::Response { request_id, response } => {
                        debug!("Received sync response for {:?}", request_id);
                        if let Some(pending) = self.pending_requests.remove(&request_id) {
                            if let SyncResponse::Handshake(info) = &response {
                                match self.validate_handshake(info) {
                                    Ok(_) => {
                                        let mut peers = self.peers.write().await;
                                        if let Some(peer_stats) = peers.get_mut(&pending.peer_id) {
                                            peer_stats.handshake_validated = true;
                                            peer_stats.handshake_info = Some(info.clone());
                                            peer_stats.score = peer_stats.score.saturating_add(10);
                                            self.persist_peer_score_sync(&pending.peer_id, peer_stats.score);
                                        }
                                    }
                                    Err(e) => {
                                        warn!("Handshake response invalid from {}: {}", pending.peer_id, e);
                                        self.record_misbehavior_sync(&pending.peer_id.to_string());
                                    }
                                }
                            }
                            if let Some(response_tx) = pending.response_tx {
                                let _ = response_tx.send(response);
                            }
                        }
                    }
                }
            }

            // Request-response: outbound failure
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
            }

            // Request-response: inbound failure
            SwarmEvent::Behaviour(RinkuBehaviourEvent::RequestResponse(
                request_response::Event::InboundFailure { peer, error, .. }
            )) => {
                warn!("Inbound sync request from {} failed: {:?}", peer, error);
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

    /// Send a response to an incoming sync request
    pub fn send_sync_response(&mut self, channel: ResponseChannel<SyncResponse>, response: SyncResponse) {
        if let Err(e) = self.swarm.behaviour_mut().request_response.send_response(channel, response) {
            warn!("Failed to send sync response: {:?}", e);
        }
    }

    async fn publish_message(&mut self, message: &GossipMessage) {
        match serde_json::to_vec(message) {
            Ok(data) => {
                let data_len = data.len() as u64;
                
                match self.swarm.behaviour_mut().gossipsub.publish(self.topic.clone(), data) {
                    Ok(_) => {
                        let mut stats = self.stats.write().await;
                        stats.messages_published += 1;
                        stats.bytes_sent += data_len;
                        debug!("Published message to network");
                    }
                    Err(e) => {
                        // Duplicate messages are normal in gossipsub - don't spam logs
                        if format!("{:?}", e).contains("Duplicate") {
                            trace!("Message already published (duplicate)");
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

    /// Check if connection limit is reached (non-blocking, returns false if lock contended)
    pub fn is_connection_limit_reached_sync(&self) -> bool {
        if let Ok(peers) = self.peers.try_read() {
            return peers.len() >= self.dos_config.max_connections;
        }
        false
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
                warn!("Rate limit check blocked, rejecting request as precaution");
                return false;
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

    /// Create a handshake info for this node
    pub fn create_handshake(&self, checkpoint_height: u64, validator_address: Option<String>) -> PeerHandshake {
        PeerHandshake {
            protocol_version: self.handshake_config.protocol_version.clone(),
            chain_id: self.handshake_config.chain_id.clone(),
            network_id: self.handshake_config.network_id.clone(),
            node_id: self.local_peer_id.to_string(),
            checkpoint_height,
            validator_address,
            capabilities: vec!["sync".to_string(), "gossip".to_string(), "proofs".to_string()],
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

    #[test]
    fn test_validate_handshake_enforces_chain_and_network() {
        let config = NetworkConfig {
            listen_addr: "/ip4/127.0.0.1/tcp/4001".to_string(),
            bootstrap_peers: Vec::new(),
            enable_mdns: false,
            data_dir: None,
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
        };

        assert!(service.validate_handshake(&good_handshake).is_ok());
    }
}

/// Command to send to the network service
#[derive(Debug)]
pub enum NetworkCommand {
    Dial(Multiaddr, oneshot::Sender<Result<()>>),
    SendSyncResponse(ResponseChannel<SyncResponse>, SyncResponse),
}

pub struct NetworkHandle {
    outbound_tx: mpsc::Sender<GossipMessage>,
    pub message_rx: mpsc::Receiver<GossipMessage>,
    peers: Arc<RwLock<HashMap<PeerId, PeerStats>>>,
    local_peer_id: String,
    /// Channel for sending sync requests
    sync_request_tx: mpsc::Sender<(PeerId, SyncRequest, oneshot::Sender<SyncResponse>)>,
    /// Channel for receiving incoming sync requests (to be handled by application)
    pub sync_incoming_rx: mpsc::Receiver<IncomingSyncRequest>,
    /// Channel for sending commands to the network service
    command_tx: mpsc::Sender<NetworkCommand>,
    /// Stats tracking
    stats: Arc<RwLock<NetworkStatsInner>>,
}

impl NetworkHandle {
    pub async fn broadcast(&self, message: GossipMessage) -> Result<()> {
        self.outbound_tx.send(message).await?;
        Ok(())
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
        self.peers.read().await
            .keys()
            .map(|p| p.to_string())
            .collect()
    }

    /// Send a sync response for an incoming request
    pub fn send_sync_response(&self, channel: ResponseChannel<SyncResponse>, response: SyncResponse) {
        let _ = self.command_tx.try_send(NetworkCommand::SendSyncResponse(channel, response));
    }

    /// Send a sync request to a specific peer and wait for response
    pub async fn sync_request(&self, peer_id: &str, request: SyncRequest) -> Result<SyncResponse> {
        let peer_id: PeerId = peer_id.parse()
            .map_err(|e| anyhow::anyhow!("Invalid peer ID: {}", e))?;
        
        let (response_tx, response_rx) = oneshot::channel();
        self.sync_request_tx.send((peer_id, request, response_tx)).await?;
        
        let response = response_rx.await
            .map_err(|_| anyhow::anyhow!("Sync request cancelled"))?;
        
        Ok(response)
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
}
