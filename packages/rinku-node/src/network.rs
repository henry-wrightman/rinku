#![cfg(feature = "p2p")]

use anyhow::Result;
use libp2p::futures::StreamExt;
use libp2p::{
    gossipsub::{self, IdentTopic, MessageAuthenticity, ValidationMode},
    identity::Keypair,
    mdns,
    noise,
    swarm::SwarmEvent,
    tcp, yamux, Multiaddr, PeerId, Swarm,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

use crate::gossip::GossipMessage;

const PROTOCOL_TOPIC: &str = "rinku/1.0.0";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(libp2p::swarm::NetworkBehaviour)]
pub struct RinkuBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub mdns: mdns::tokio::Behaviour,
}

#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub listen_addr: String,
    pub bootstrap_peers: Vec<String>,
    pub enable_mdns: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            listen_addr: "/ip4/0.0.0.0/tcp/4001".to_string(),
            bootstrap_peers: Vec::new(),
            enable_mdns: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PeerStats {
    pub peer_id: String,
    pub connected_at: u64,
    pub messages_received: u64,
    pub messages_sent: u64,
    pub last_seen: u64,
}

pub struct NetworkStats {
    pub local_peer_id: String,
    pub connected_peers: usize,
    pub messages_published: u64,
    pub messages_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
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
}

struct NetworkStatsInner {
    messages_published: u64,
    messages_received: u64,
    bytes_sent: u64,
    bytes_received: u64,
}

impl NetworkService {
    pub fn new(config: NetworkConfig) -> Result<(Self, NetworkHandle)> {
        let local_key = Keypair::generate_ed25519();
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
            .max_transmit_size(65536)
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

        let behaviour = RinkuBehaviour { gossipsub, mdns };

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

        let stats = Arc::new(RwLock::new(NetworkStatsInner {
            messages_published: 0,
            messages_received: 0,
            bytes_sent: 0,
            bytes_received: 0,
        }));

        let peers = Arc::new(RwLock::new(HashMap::new()));

        let handle = NetworkHandle {
            outbound_tx,
            message_rx,
            peers: peers.clone(),
            local_peer_id: local_peer_id.to_string(),
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

    async fn run_event_loop(&mut self) -> Result<()> {
        loop {
            tokio::select! {
                Some(msg) = self.outbound_rx.recv() => {
                    self.publish_message(&msg).await;
                }
                event = self.swarm.select_next_some() => {
                    self.handle_swarm_event(event).await;
                }
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
                    
                    let mut peer_map = self.peers.write().await;
                    peer_map.entry(peer_id).or_insert_with(|| PeerStats {
                        peer_id: peer_id.to_string(),
                        connected_at: current_time_secs(),
                        messages_received: 0,
                        messages_sent: 0,
                        last_seen: current_time_secs(),
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
                info!("Connected to peer: {} via {:?}", peer_id, endpoint);
                
                let mut peers = self.peers.write().await;
                peers.entry(peer_id).or_insert_with(|| PeerStats {
                    peer_id: peer_id.to_string(),
                    connected_at: current_time_secs(),
                    messages_received: 0,
                    messages_sent: 0,
                    last_seen: current_time_secs(),
                });
            }

            SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
                info!("Disconnected from peer: {} (cause: {:?})", peer_id, cause);
                
                let mut peers = self.peers.write().await;
                peers.remove(&peer_id);
            }

            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Listening on {}", address);
            }

            _ => {}
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
                        warn!("Failed to publish message: {:?}", e);
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
}

fn current_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

pub struct NetworkHandle {
    outbound_tx: mpsc::Sender<GossipMessage>,
    pub message_rx: mpsc::Receiver<GossipMessage>,
    peers: Arc<RwLock<HashMap<PeerId, PeerStats>>>,
    local_peer_id: String,
}

impl NetworkHandle {
    pub async fn broadcast(&self, message: GossipMessage) -> Result<()> {
        self.outbound_tx.send(message).await?;
        Ok(())
    }

    pub async fn get_peer_count(&self) -> usize {
        self.peers.read().await.len()
    }

    pub fn local_peer_id(&self) -> &str {
        &self.local_peer_id
    }
}
