use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::state::NodeState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum GossipMessage {
    Transaction { hash: String, payload: String },
    TipAnnouncement { tips: Vec<String> },
    CheckpointSignature { height: u64, signature: String },
    PeerDiscovery { peers: Vec<String> },
}

pub struct GossipService {
    state: NodeState,
    peers: Vec<String>,
    known_txs: HashSet<String>,
    interval_ms: u64,
}

impl GossipService {
    pub fn new(state: NodeState, peers: Vec<String>, interval_ms: u64) -> Self {
        Self {
            state,
            peers,
            known_txs: HashSet::new(),
            interval_ms,
        }
    }

    pub async fn start(mut self) -> Result<()> {
        info!(
            "Gossip service started (interval: {}ms, peers: {})",
            self.interval_ms,
            self.peers.len()
        );

        let mut tick = interval(Duration::from_millis(self.interval_ms));

        loop {
            tick.tick().await;
            self.gossip_round().await;
        }
    }

    async fn gossip_round(&mut self) {
        if self.peers.is_empty() {
            return;
        }

        let tips = self.state.get_tips().await;
        let message = GossipMessage::TipAnnouncement { tips };

        for peer in &self.peers {
            if let Err(e) = self.send_to_peer(peer, &message).await {
                debug!("Failed to gossip to {}: {}", peer, e);
            }
        }
    }

    async fn send_to_peer(&self, peer: &str, message: &GossipMessage) -> Result<()> {
        let client = reqwest::Client::new();
        let url = format!("{}/api/gossip", peer);

        client
            .post(&url)
            .json(message)
            .timeout(Duration::from_secs(5))
            .send()
            .await?;

        Ok(())
    }

    pub fn add_peer(&mut self, peer: String) {
        if !self.peers.contains(&peer) {
            self.peers.push(peer);
        }
    }

    pub fn remove_peer(&mut self, peer: &str) {
        self.peers.retain(|p| p != peer);
    }

    pub fn mark_tx_known(&mut self, hash: &str) {
        self.known_txs.insert(hash.to_string());

        if self.known_txs.len() > 10000 {
            let to_remove: Vec<_> = self.known_txs.iter().take(5000).cloned().collect();
            for h in to_remove {
                self.known_txs.remove(&h);
            }
        }
    }

    pub fn is_tx_known(&self, hash: &str) -> bool {
        self.known_txs.contains(hash)
    }
}
