use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures::stream::{FuturesUnordered, StreamExt};
use rinku_core::{
    merkle::MerkleTree,
    types::{Checkpoint, ValidatorSignature},
    SignedTransaction,
};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, debug};

use crate::bls::{
    aggregate_signatures, bls_sign, bls_verify, create_signer_bitmap, generate_bls_keypair,
};
use crate::config::TrustConfig;
use crate::consensus::ConsensusService;
use crate::dag_pruning::{DagPruningService, PruningConfig};
use crate::leader_election::{LeaderElectionService, LeaderElectionConfig};
#[cfg(feature = "p2p")]
use crate::network::{CheckpointData, CheckpointPushData, CheckpointVoteRequest, NetworkHandle, SyncRequest, SyncResponse, VoteRequest, VoteResponse};
use crate::slashing::SlashingService;
use crate::state::NodeState;
use crate::trust::TrustVerifier;
use crate::validator_identity::ValidatorIdentityService;

const DYNAMIC_TX_CAP_MIN: usize = 10;
const DYNAMIC_TX_CAP_MAX: usize = 500;
const DYNAMIC_TX_CAP_DEFAULT: usize = 50;

pub struct CheckpointService {
    state: NodeState,
    interval_ms: u64,
    bls_private_key: Vec<u8>,
    bls_public_key: Vec<u8>,
    validator_address: String,
    peers: Vec<String>,
    consecutive_fork_failures: std::sync::atomic::AtomicU32,
    trust_verifier: Arc<TrustVerifier>,
    validator_identity: Option<Arc<RwLock<ValidatorIdentityService>>>,
    pruning_service: Option<Arc<tokio::sync::Mutex<DagPruningService>>>,
    pruning_counter: std::sync::atomic::AtomicU32,
    consensus_service: Option<Arc<RwLock<ConsensusService>>>,
    slashing_service: Option<Arc<RwLock<SlashingService>>>,
    #[cfg(feature = "p2p")]
    network_handle: Option<Arc<NetworkHandle>>,
    our_stake: u64,
    leader_election: Option<LeaderElectionService>,
    local_url: Option<String>,
    mainnet_mode: bool,
    gossip_service: Option<Arc<crate::gossip::GossipService>>,
    event_bus: Option<Arc<crate::events::EventBus>>,
    last_seen_height: u64,
    stuck_iterations: u32,
    dynamic_tx_cap: usize,
    leader_wait_ticks: u32,
    leader_wait_height: u64,
}

const FORK_RECOVERY_THRESHOLD: u32 = 3;
const LEADER_SKIP_BASE_TICKS: u32 = 5;
const LEADER_SKIP_STAGGER_TICKS: u32 = 2;
const LEADER_INTENT_EXTENSION_TICKS: u32 = 5;

// Use centralized constant from config
use crate::config::PROPAGATION_GRACE_MS;

impl CheckpointService {
    pub fn new(
        state: NodeState, 
        interval_ms: u64, 
        validator_address: Option<String>, 
        peers: Vec<String>,
        trust_config: TrustConfig,
        mainnet_mode: bool,
    ) -> Self {
        let keypair = generate_bls_keypair();
        let addr = validator_address.unwrap_or_else(|| keypair.fingerprint.clone());
        Self {
            state,
            interval_ms,
            bls_private_key: keypair.private_key,
            bls_public_key: keypair.public_key,
            validator_address: addr,
            peers,
            consecutive_fork_failures: std::sync::atomic::AtomicU32::new(0),
            trust_verifier: Arc::new(TrustVerifier::new(trust_config)),
            validator_identity: None,
            pruning_service: Some(Arc::new(tokio::sync::Mutex::new(DagPruningService::new(PruningConfig::default())))),
            pruning_counter: std::sync::atomic::AtomicU32::new(0),
            consensus_service: None,
            slashing_service: None,
            #[cfg(feature = "p2p")]
            network_handle: None,
            our_stake: 10_000_000_000,
            leader_election: None,
            local_url: None,
            mainnet_mode,
            gossip_service: None,
            event_bus: None,
            last_seen_height: 0,
            stuck_iterations: 0,
            dynamic_tx_cap: DYNAMIC_TX_CAP_DEFAULT,
            leader_wait_ticks: 0,
            leader_wait_height: 0,
        }
    }
    
    pub fn with_gossip_service(mut self, gossip: Arc<crate::gossip::GossipService>) -> Self {
        self.gossip_service = Some(gossip);
        self
    }

    pub fn with_event_bus(mut self, event_bus: Arc<crate::events::EventBus>) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    pub fn with_validator_identity(mut self, identity: Arc<RwLock<ValidatorIdentityService>>) -> Self {
        self.validator_identity = Some(identity);
        self
    }
    
    pub fn with_pruning_config(mut self, config: PruningConfig) -> Self {
        self.pruning_service = Some(Arc::new(tokio::sync::Mutex::new(DagPruningService::new(config))));
        self
    }
    
    pub fn with_consensus_service(mut self, consensus: Arc<RwLock<ConsensusService>>) -> Self {
        self.consensus_service = Some(consensus);
        self
    }
    
    pub fn with_slashing_service(mut self, slashing: Arc<RwLock<SlashingService>>) -> Self {
        self.slashing_service = Some(slashing);
        self
    }
    
    /// Set the P2P network handle for requesting checkpoint votes from peers
    #[cfg(feature = "p2p")]
    pub fn with_network_handle(mut self, handle: Arc<NetworkHandle>) -> Self {
        self.network_handle = Some(handle);
        self
    }
    
    /// Set our validator's stake for quorum calculation
    pub fn with_stake(mut self, stake: u64) -> Self {
        self.our_stake = stake;
        self
    }
    
    /// Set the local URL for leader election (from PUBLIC_URL env var)
    pub fn with_local_url(mut self, url: Option<String>) -> Self {
        self.local_url = url.clone();
        // Initialize leader election service with our address and URL
        let config = LeaderElectionConfig::default();
        self.leader_election = Some(LeaderElectionService::new(
            self.validator_address.clone(),
            url,
            config,
        ));
        self
    }
    
    /// Enable leader election with custom config
    pub fn with_leader_election(mut self, config: LeaderElectionConfig) -> Self {
        self.leader_election = Some(LeaderElectionService::new(
            self.validator_address.clone(),
            self.local_url.clone(),
            config,
        ));
        self
    }

    pub fn bls_public_key_base64(&self) -> String {
        URL_SAFE_NO_PAD.encode(&self.bls_public_key)
    }

    pub fn with_bls_keypair(mut self, private_key: Vec<u8>, public_key: Vec<u8>) -> Self {
        self.bls_private_key = private_key;
        self.bls_public_key = public_key;
        self
    }

    pub fn bls_public_key_bytes(&self) -> Vec<u8> {
        self.bls_public_key.clone()
    }

    pub fn bls_private_key_bytes(&self) -> Vec<u8> {
        self.bls_private_key.clone()
    }

    pub fn validator_address(&self) -> String {
        self.validator_address.clone()
    }

    pub async fn start(mut self) -> Result<()> {
        self.sign_genesis_checkpoint().await;
        
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_millis(self.interval_ms));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;

        loop {
            ticker.tick().await;

            if let Some(ref validator_identity) = self.validator_identity {
                let result = validator_identity.write().await.process_epoch_transition();
                if result.new_epoch > result.old_epoch {
                    info!(
                        "Epoch transition: {} -> {} (activated: {}, exited: {})",
                        result.old_epoch, result.new_epoch,
                        result.activated.len(), result.exited.len()
                    );
                }
            }

            if let Err(e) = self.create_state_snapshot().await {
                tracing::warn!("State snapshot failed: {}", e);
            }
            
            let prune_count = self.pruning_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            const PRUNE_EVERY_N_CHECKPOINTS: u32 = 10;
            const IN_MEMORY_RETENTION: u64 = 10;

            if prune_count > 0 && prune_count % PRUNE_EVERY_N_CHECKPOINTS == 0 {
                let current_height = self.state.get_checkpoint_height();
                
                if current_height > 50 {
                    let finalized_hashes: std::collections::HashSet<String> = {
                        let state_guard = self.state.inner.read().await;
                        state_guard.dag
                            .get_all_nodes()
                            .iter()
                            .filter(|n| n.finalized)
                            .map(|n| n.hash.clone())
                            .collect()
                    };

                    if let Some(ref pruning_arc) = self.pruning_service {
                        let pruning = Arc::clone(pruning_arc);
                        let storage = Arc::clone(self.state.storage());
                        tokio::spawn(async move {
                            let mut pruning_guard = pruning.lock().await;
                            match pruning_guard.prune_dag(&storage, current_height, &finalized_hashes) {
                                Ok(stats) => {
                                    info!(
                                        "DAG pruning completed: {} nodes pruned, {} checkpoints pruned, oldest retained: {}",
                                        stats.nodes_pruned, stats.checkpoints_pruned, stats.oldest_retained_checkpoint
                                    );
                                }
                                Err(e) => {
                                    warn!("DAG pruning failed: {}", e);
                                }
                            }
                        });
                    }
                }
            }

            {
                let current_height = self.state.get_checkpoint_height();
                if current_height > IN_MEMORY_RETENTION {
                    let mut state_guard = self.state.inner.write().await;
                    let pruned = state_guard.dag.prune_finalized_before(current_height - IN_MEMORY_RETENTION);
                    let remaining = state_guard.dag.node_count();
                    drop(state_guard);
                    if pruned > 0 {
                        info!(
                            "DAG in-memory prune: {} finalized nodes removed, {} remaining",
                            pruned, remaining
                        );
                    }
                }
            }
        }
    }
    
    /// Sign the genesis checkpoint (height 0) if it exists but has no BLS signatures
    /// Note: We keep the original hash to ensure all nodes have the same genesis checkpoint hash
    async fn sign_genesis_checkpoint(&self) {
        let mut state = self.state.inner.write().await;
        
        let my_stake = state.validators.get(&self.validator_address)
            .map(|v| v.stake)
            .unwrap_or(0);
        
        // Find genesis checkpoint (height 0)
        if let Some(genesis_cp) = state.checkpoints.iter_mut().find(|cp| cp.height == 0) {
            // Check if it already has valid signatures with BLS keys
            let has_bls_keys = genesis_cp.validator_signatures.iter().any(|s| s.bls_public_key.is_some());
            
            if !has_bls_keys {
                info!("Signing genesis checkpoint with node's BLS key");
                
                // Use the existing hash as the message to sign (keeps hash deterministic across nodes)
                let hash_bytes = hex::decode(&genesis_cp.hash).unwrap_or_else(|_| {
                    // Fallback: compute hash if current hash is not valid hex
                    Self::compute_checkpoint_hash(
                        genesis_cp.height,
                        &genesis_cp.tx_merkle_root,
                        &genesis_cp.state_root,
                        &genesis_cp.receipt_root,
                        genesis_cp.tip_count,
                        genesis_cp.timestamp,
                    )
                });
                
                // Sign with our BLS key
                if let Ok(signature) = bls_sign(&hash_bytes, &self.bls_private_key) {
                    let validator_sig = ValidatorSignature {
                        validator: self.validator_address.clone(),
                        signature: URL_SAFE_NO_PAD.encode(&signature),
                        weight: my_stake,
                        bls_public_key: Some(self.bls_public_key_base64()),
                    };
                    
                    // Don't change the hash - keep it deterministic across all nodes
                    genesis_cp.validator_signatures = vec![validator_sig];
                    
                    if let Ok(agg_sig) = aggregate_signatures(&[signature]) {
                        genesis_cp.aggregated_signature = Some(URL_SAFE_NO_PAD.encode(&agg_sig));
                        genesis_cp.signer_bitmap = Some(create_signer_bitmap(&[0], 1));
                    }
                    
                    info!("Genesis checkpoint signed: {}", &genesis_cp.hash[..16.min(genesis_cp.hash.len())]);
                }
            }
        }
    }

    /// Fetch and apply a peer checkpoint at the given height via P2P delta sync.
    /// Tries each peer sequentially, returning as soon as one provides useful data.
    /// This avoids waiting for dead peers' 3s timeouts when healthy peers respond fast.
    async fn fetch_and_apply_peer_checkpoint(&self, _height: u64) -> bool {
        #[cfg(feature = "p2p")]
        {
            let network_handle = match self.network_handle.as_ref() {
                Some(h) => h,
                None => return false,
            };
            let peers = network_handle.get_connected_peers().await;

            let local_height = self.state.get_checkpoint_height();
            let from_cp = local_height;

            for peer in &peers {
                let peer_id = peer.peer_id.clone();
                let result = tokio::time::timeout(
                    std::time::Duration::from_millis(1500),
                    network_handle.request_delta(&peer_id, from_cp),
                ).await;

                let result = match result {
                    Ok(r) => r,
                    Err(_) => {
                        warn!("Delta sync request to peer {} timed out after 1.5s", &peer_id[..16.min(peer_id.len())]);
                        continue;
                    }
                };
                match result {
                    Ok(SyncResponse::Delta(delta)) => {
                        if !delta.transactions.is_empty() {
                            let mut ingested = 0u64;
                            for tx_data in &delta.transactions {
                                let stx = SignedTransaction {
                                    tx: rinku_core::types::Transaction {
                                        from: tx_data.from.clone(),
                                        to: tx_data.to.clone(),
                                        amount: tx_data.amount,
                                        nonce: tx_data.nonce,
                                        timestamp: tx_data.timestamp,
                                        parents: tx_data.parents.clone(),
                                        kind: None,
                                        gas_limit: None,
                                        gas_price: Some(tx_data.gas_price),
                                        data: None,
                                        signature: Some(tx_data.signature.clone()),
                                        memo: tx_data.memo.clone(),
                                        references: tx_data.references.clone(),
                                    },
                                    hash: tx_data.hash.clone(),
                                    signature: tx_data.signature.clone(),
                                };
                                if self.state.add_transaction_from_sync(stx).await.is_ok() {
                                    ingested += 1;
                                }
                            }
                            if ingested > 0 {
                                info!(
                                    "Ingested {} peer txs from delta (peer {}, {} checkpoints available)",
                                    ingested,
                                    &peer_id[..16.min(peer_id.len())],
                                    delta.new_checkpoints.len()
                                );
                            }
                        }

                        let mut sorted_cps: Vec<&CheckpointData> = delta.new_checkpoints.iter().collect();
                        sorted_cps.sort_by_key(|c| c.height);

                        let mut applied_count = 0u64;
                        for cp_data in &sorted_cps {
                            let current = self.state.get_checkpoint_height();
                            if cp_data.height <= current {
                                continue;
                            }
                            if cp_data.height != current + 1 {
                                if let Some(ref gossip) = self.gossip_service {
                                    let checkpoint = self.checkpoint_data_to_checkpoint(cp_data, &delta.new_checkpoints);
                                    let mut buffer = gossip.checkpoint_buffer.lock().await;
                                    if !buffer.contains_key(&checkpoint.height) {
                                        info!(
                                            "Delta sync: buffering checkpoint {} at height {} (current: {}) for later",
                                            &checkpoint.hash[..16.min(checkpoint.hash.len())],
                                            checkpoint.height,
                                            current
                                        );
                                        buffer.insert(checkpoint.height, crate::gossip::BufferedCheckpoint {
                                            checkpoint,
                                            finalized_tx_hashes: cp_data.finalized_tx_hashes.clone(),
                                            finalized_transactions: Vec::new(),
                                            precomputed_proofs: Vec::new(),
                                            source: format!("delta-{}", &peer_id[..16.min(peer_id.len())]),
                                        });
                                    }
                                }
                                continue;
                            }

                            let checkpoint = self.checkpoint_data_to_checkpoint(cp_data, &delta.new_checkpoints);

                            {
                                let mut emission = self.state.emission.write().await;
                                let reward = emission.get_checkpoint_reward(checkpoint.height);
                                if emission.record_emission_for_height(checkpoint.height, reward) {
                                    let mut rewards = self.state.rewards.write().await;
                                    rewards.distribute_checkpoint_rewards(reward);
                                }
                            }

                            let finalized_tx_hashes = checkpoint.finalized_tx_hashes.clone();
                            match self.state.apply_checkpoint_with_finalized_hashes(
                                checkpoint.clone(),
                                finalized_tx_hashes,
                            ).await {
                                Ok(missing_tx_count) => {
                                    if missing_tx_count > 0 {
                                        warn!(
                                            "Delta-sync recovery: {} txs missing after checkpoint {} at height {}",
                                            missing_tx_count,
                                            &checkpoint.hash[..16.min(checkpoint.hash.len())],
                                            checkpoint.height
                                        );
                                    }
                                    applied_count += 1;
                                    if let Some(ref gossip) = self.gossip_service {
                                        gossip.remove_finalized_from_convergence(&checkpoint.finalized_tx_hashes).await;
                                    }
                                    info!(
                                        "Applied recovered checkpoint {} at height {} from delta sync",
                                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                                        checkpoint.height
                                    );
                                    if let Some(ref eb) = self.event_bus {
                                        eb.publish(crate::events::NodeEvent::CheckpointCreated {
                                            hash: checkpoint.hash.clone(),
                                            height: checkpoint.height,
                                            txs_finalized: checkpoint.finalized_tx_hashes.len(),
                                            reward: 0.0,
                                        });
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to apply recovered checkpoint {} at height {}: {}",
                                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                                        checkpoint.height,
                                        e
                                    );
                                    break;
                                }
                            }
                        }

                        if applied_count > 0 {
                            info!(
                                "Delta sync from peer {}: applied {} checkpoints, draining buffer",
                                &peer_id[..16.min(peer_id.len())],
                                applied_count
                            );
                            if let Some(ref gossip) = self.gossip_service {
                                gossip.drain_checkpoint_buffer().await;
                            }
                            return true;
                        }
                    }
                    Ok(_) => {
                        warn!("P2P peer {} returned unexpected response for delta", &peer_id[..16.min(peer_id.len())]);
                    }
                    Err(e) => {
                        warn!("Failed to request delta from p2p peer {}: {}", &peer_id[..16.min(peer_id.len())], e);
                    }
                }
            }
        }
        false
    }

    #[cfg(feature = "p2p")]
    fn checkpoint_data_to_checkpoint(&self, cp_data: &CheckpointData, all_cps: &[CheckpointData]) -> Checkpoint {
        let previous_hash = all_cps.iter()
            .find(|c| c.height + 1 == cp_data.height)
            .and_then(|c| c.hash.clone());

        Checkpoint {
            height: cp_data.height,
            hash: cp_data.hash.clone().unwrap_or_else(|| rinku_core::sha256_hex(&format!("cp:{}", cp_data.height))),
            previous_hash: previous_hash.or_else(|| cp_data.previous_hash.clone()),
            tx_merkle_root: cp_data.merkle_root.clone(),
            state_root: cp_data.state_root.clone().unwrap_or_else(|| cp_data.merkle_root.clone()),
            receipt_root: cp_data.receipt_root.clone().unwrap_or_default(),
            tip_count: cp_data.tip_count.unwrap_or(0),
            timestamp: cp_data.timestamp,
            validator_signatures: cp_data.validator_signatures.clone(),
            aggregated_signature: cp_data.signature.clone(),
            signer_bitmap: cp_data.signer_bitmap.clone(),
            finalized_tx_hashes: cp_data.finalized_tx_hashes.clone(),
            weight_trie_root: String::new(),
            provisional: false,
            partition_epoch: None,
            visible_stake_pct: None,
            merge_report_hash: None,
        }
    }

    /// Sync missing transactions from peers via P2P delta sync
    async fn sync_missing_transactions(&self, target_height: u64) -> Result<()> {
        #[cfg(feature = "p2p")]
        {
            let network_handle = self.network_handle.as_ref()
                .ok_or_else(|| anyhow::anyhow!("No P2P network handle available"))?;

            let from_checkpoint = {
                let state = self.state.inner.read().await;
                state.checkpoints.last().map(|cp| cp.height).unwrap_or(0)
            };

            let peers = network_handle.get_connected_peers().await;

            for peer in &peers {
                let peer_id = peer.peer_id.clone();
                let result = network_handle.request_delta(&peer_id, from_checkpoint).await;
                match result {
                    Ok(SyncResponse::Delta(delta)) => {
                        let mut added = 0;
                        for tx_data in &delta.transactions {
                            let mut state = self.state.inner.write().await;
                            if !state.dag.contains(&tx_data.hash) {
                                use rinku_core::types::{DagNode, Transaction};
                                let now_ms = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64;
                                let tx = SignedTransaction {
                                    tx: Transaction {
                                        from: tx_data.from.clone(),
                                        to: tx_data.to.clone(),
                                        amount: tx_data.amount,
                                        nonce: tx_data.nonce,
                                        timestamp: tx_data.timestamp,
                                        parents: tx_data.parents.clone(),
                                        kind: None,
                                        gas_limit: None,
                                        gas_price: Some(tx_data.gas_price),
                                        data: None,
                                        signature: Some(tx_data.signature.clone()),
                                        memo: tx_data.memo.clone(),
                                        references: tx_data.references.clone(),
                                    },
                                    hash: tx_data.hash.clone(),
                                    signature: tx_data.signature.clone(),
                                };
                                let node = DagNode {
                                    hash: tx_data.hash.clone(),
                                    parents: tx_data.parents.clone(),
                                    children: vec![],
                                    weight: 1.0,
                                    finalized: false,
                                    checkpoint_height: None,
                                    tx,
                                    received_at_ms: Some(now_ms),
                                    partition_epoch: None,
                                    rolled_back: false,
                                    convergence_certificate: None,
                                };
                                if state.dag.add_node(node).is_ok() {
                                    added += 1;
                                }
                            }
                        }

                        if added > 0 {
                            debug!(
                                "Fork prevention: synced {} new transactions from p2p peer for checkpoint {}",
                                added, target_height
                            );
                            return Ok(());
                        }
                    }
                    Ok(_) => {
                        debug!("P2P peer {} returned unexpected response for delta", &peer_id[..16.min(peer_id.len())]);
                    }
                    Err(e) => {
                        debug!("Failed to sync delta from p2p peer {}: {}", &peer_id[..16.min(peer_id.len())], e);
                    }
                }
            }
        }

        Err(anyhow::anyhow!("No peers had transactions to sync"))
    }

    /// Validate and adopt a peer's checkpoint instead of creating our own
    /// Returns (adopted: bool, previous_hash_mismatch: bool)
    async fn validate_and_adopt_peer_checkpoint(
        &self, 
        peer_checkpoint: Checkpoint, 
        local_tx_merkle_root: &str,
        local_previous_hash: Option<&str>,
        unfinalized_hashes: &[String]
    ) -> (bool, bool) {
        // VALIDATION 1: Check previous_hash chain linkage
        let peer_prev = peer_checkpoint.previous_hash.as_deref();
        if peer_prev != local_previous_hash {
            warn!(
                "Peer checkpoint previous_hash mismatch at height {}: peer={:?} vs local={:?}",
                peer_checkpoint.height,
                peer_prev.map(|s| &s[..16.min(s.len())]),
                local_previous_hash.map(|s| &s[..16.min(s.len())])
            );
            return (false, true); // previous_hash mismatch - indicates fork
        }

        // VALIDATION 2: Check merkle root matches our local transaction set
        // This ensures we're finalizing the same transactions
        if peer_checkpoint.tx_merkle_root != local_tx_merkle_root {
            warn!(
                "Peer checkpoint merkle root mismatch at height {}: peer={} vs local={}",
                peer_checkpoint.height,
                &peer_checkpoint.tx_merkle_root[..16.min(peer_checkpoint.tx_merkle_root.len())],
                &local_tx_merkle_root[..16.min(local_tx_merkle_root.len())]
            );
            return (false, false); // merkle mismatch, but not a chain fork
        }

        // VALIDATION 4: Verify the checkpoint hash matches recomputed value
        let expected_hash = Self::compute_checkpoint_hash(
            peer_checkpoint.height,
            &peer_checkpoint.tx_merkle_root,
            &peer_checkpoint.state_root,
            &peer_checkpoint.receipt_root,
            peer_checkpoint.tip_count,
            peer_checkpoint.timestamp,
        );
        let expected_hash_hex = hex::encode(&expected_hash);
        
        if peer_checkpoint.hash != expected_hash_hex {
            warn!(
                "Peer checkpoint hash mismatch at height {}: provided={} vs computed={}",
                peer_checkpoint.height,
                &peer_checkpoint.hash[..16.min(peer_checkpoint.hash.len())],
                &expected_hash_hex[..16.min(expected_hash_hex.len())]
            );
            return (false, false);
        }
        
        // VALIDATION 5: Verify signatures and quorum based on trust configuration
        if self.mainnet_mode {
            let validators = {
                let state = self.state.inner.read().await;
                state.validators.clone()
            };
            let result = self.trust_verifier.verify_checkpoint(&peer_checkpoint, &validators);
            if !result.valid {
                warn!(
                    "Peer checkpoint at height {} failed quorum verification: {}",
                    peer_checkpoint.height,
                    result.error.unwrap_or_else(|| "unknown error".to_string())
                );
                return (false, false);
            }
        } else {
            // Non-mainnet mode: require at least one valid BLS signature
            if peer_checkpoint.validator_signatures.is_empty() && peer_checkpoint.height > 1 {
                warn!(
                    "Peer checkpoint at height {} has no validator signatures",
                    peer_checkpoint.height
                );
                return (false, false);
            }

            let mut valid_sig_found = false;
            for sig in &peer_checkpoint.validator_signatures {
                let sig_bytes = match URL_SAFE_NO_PAD.decode(&sig.signature) {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                };
                
                if sig_bytes.len() < 96 {
                    continue;
                }
                
                if let Ok(_) = blst::min_pk::Signature::from_bytes(&sig_bytes) {
                    valid_sig_found = true;
                    debug!(
                        "Peer checkpoint has valid BLS signature from validator {}",
                        &sig.validator[..16.min(sig.validator.len())]
                    );
                    break;
                }
            }
            
            if !valid_sig_found {
                warn!(
                    "Peer checkpoint at height {} has no valid BLS signature",
                    peer_checkpoint.height
                );
                return (false, false);
            }
        }

        // Validation passed - adopt the peer's checkpoint
        let height = peer_checkpoint.height;
        let checkpoint_hash = peer_checkpoint.hash.clone();

        // Process emissions for this adopted checkpoint
        let checkpoint_reward = {
            let mut emission = self.state.emission.write().await;
            let reward = emission.get_checkpoint_reward(height);
            if emission.record_emission_for_height(height, reward) {
                reward
            } else {
                0
            }
        };

        // Distribute checkpoint rewards (only if emission was recorded)
        let distributions = if checkpoint_reward > 0 {
            let mut rewards = self.state.rewards.write().await;
            rewards.distribute_checkpoint_rewards(checkpoint_reward)
        } else {
            vec![]
        };

        if !distributions.is_empty() {
            debug!(
                "Distributed {:.6} RKU to {} validators (adopted checkpoint)",
                rinku_core::types::from_micro_units(checkpoint_reward),
                distributions.len()
            );
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // FINALITY-FIRST MODEL: Collect transactions for execution before marking finalized
        // DOUBLE-EXECUTION GUARD: Only collect transactions that aren't already finalized
        let mut txs_to_execute: Vec<SignedTransaction> = {
            let state = self.state.inner.read().await;
            unfinalized_hashes.iter()
                .filter_map(|hash| {
                    state.dag.get_node(hash).and_then(|node| {
                        if node.finalized {
                            None  // Skip already-finalized transactions
                        } else {
                            Some(node.tx.clone())
                        }
                    })
                })
                .collect()
        };

        txs_to_execute.sort_by(|a, b| {
            a.tx.from.cmp(&b.tx.from)
                .then(a.tx.nonce.cmp(&b.tx.nonce))
                .then(a.hash.cmp(&b.hash))
        });

        // Update state with adopted checkpoint
        let mut state = self.state.inner.write().await;
        state.checkpoints.push(peer_checkpoint);
        state.last_checkpoint_time_ms = now_ms;
        self.state.checkpoint_height_cache.store(height, std::sync::atomic::Ordering::Relaxed);

        // Mark transactions as finalized
        for hash in unfinalized_hashes {
            let _ = state.dag.mark_finalized(hash, height);
        }
        let finalized_count = unfinalized_hashes.len() as u64;

        drop(state);

        if finalized_count > 0 {
            self.state.record_finalized_batch(finalized_count).await;
        }

        info!(
            "Adopted peer checkpoint {} at height {} ({} txs finalized, {:.6} RKU emitted)",
            &checkpoint_hash[..16.min(checkpoint_hash.len())],
            height,
            unfinalized_hashes.len(),
            rinku_core::types::from_micro_units(checkpoint_reward)
        );
        
        let fp_executed = if let Some(ref gossip) = self.gossip_service {
            gossip.get_all_convergence_executed().await
        } else {
            std::collections::HashSet::new()
        };

        for tx in &txs_to_execute {
            if fp_executed.contains(&tx.hash) {
                tracing::debug!(
                    "Skipping fast-path-executed tx {} at checkpoint (already applied)",
                    &tx.hash[..16.min(tx.hash.len())]
                );
                self.state.execute_finalized_transaction_rewards(tx).await;
            } else {
                self.state.execute_finalized_transaction(tx).await;
            }
        }

        if let Some(ref eb) = self.event_bus {
            eb.publish(crate::events::NodeEvent::CheckpointCreated {
                hash: checkpoint_hash.clone(),
                height,
                txs_finalized: unfinalized_hashes.len(),
                reward: rinku_core::types::from_micro_units(checkpoint_reward),
            });
        }

        (true, false)
    }

    fn compute_checkpoint_hash(
        height: u64,
        tx_merkle_root: &str,
        state_root: &str,
        receipt_root: &str,
        tip_count: u32,
        timestamp: u64,
    ) -> Vec<u8> {
        let data = format!(
            "{}:{}:{}:{}:{}:{}",
            height, tx_merkle_root, state_root, receipt_root, tip_count, timestamp
        );
        let mut hasher = Sha256::new();
        hasher.update(data.as_bytes());
        hasher.finalize().to_vec()
    }

    fn is_valid_hex_hash(s: &str) -> bool {
        s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
    }

    /// Recover from a forked state by requesting a full snapshot sync from the peer.
    /// This replaces accounts, checkpoints, and recent DAG atomically using the
    /// validated snapshot sync system.
    async fn recover_checkpoint_chain(&self) -> Result<bool> {
        #[cfg(feature = "p2p")]
        {
            let network_handle = self.network_handle.as_ref()
                .ok_or_else(|| anyhow::anyhow!("No P2P network handle available for chain recovery"))?;

            let peers = network_handle.get_connected_peers().await;

            if peers.is_empty() {
                return Err(anyhow::anyhow!("No P2P peers available for chain recovery"));
            }

            for peer in &peers {
                let peer_id = peer.peer_id.clone();
                let peer_id_short = &peer_id[..16.min(peer_id.len())];
                info!("[ForkRecovery] Requesting full snapshot sync from p2p peer {}", peer_id_short);

                let result = network_handle.request_snapshot(&peer_id).await;
                match result {
                    Ok(SyncResponse::Snapshot(snapshot_data)) => {
                        use crate::state::presync::convert_snapshot_data_to_sync_snapshot;
                        let sync_snapshot = convert_snapshot_data_to_sync_snapshot(snapshot_data);

                        let mut linkage_valid = true;
                        for i in 1..sync_snapshot.checkpoints.len() {
                            let expected_prev = &sync_snapshot.checkpoints[i - 1].hash;
                            if sync_snapshot.checkpoints[i].previous_hash.as_deref() != Some(expected_prev) {
                                warn!(
                                    "[ForkRecovery] Peer snapshot has invalid checkpoint chain at height {}",
                                    sync_snapshot.checkpoints[i].height
                                );
                                linkage_valid = false;
                                break;
                            }
                        }

                        if !linkage_valid {
                            continue;
                        }

                        let mut hash_valid = true;
                        for checkpoint in &sync_snapshot.checkpoints {
                            let expected_hash = Self::compute_checkpoint_hash(
                                checkpoint.height,
                                &checkpoint.tx_merkle_root,
                                &checkpoint.state_root,
                                &checkpoint.receipt_root,
                                checkpoint.tip_count,
                                checkpoint.timestamp,
                            );
                            let expected_hash_hex = hex::encode(&expected_hash);

                            if checkpoint.hash != expected_hash_hex {
                                warn!(
                                    "[ForkRecovery] Peer checkpoint hash mismatch at height {}",
                                    checkpoint.height
                                );
                                hash_valid = false;
                                break;
                            }
                        }

                        if !hash_valid {
                            continue;
                        }

                        if self.trust_verifier.has_genesis_validators() {
                            if let Err(e) = self.trust_verifier.verify_checkpoint_chain(
                                &sync_snapshot.checkpoints,
                                &sync_snapshot.validators,
                            ) {
                                warn!("[ForkRecovery] Stake-weighted verification failed: {}", e);
                                continue;
                            }
                            info!(
                                "[ForkRecovery] Verified {} checkpoints with stake-weighted BLS signatures",
                                sync_snapshot.checkpoints.len()
                            );
                        } else {
                            let mut format_valid = true;
                            for checkpoint in &sync_snapshot.checkpoints {
                                if checkpoint.validator_signatures.is_empty() && checkpoint.height > 1 {
                                    continue;
                                }
                                for sig in &checkpoint.validator_signatures {
                                    if let Ok(sig_bytes) = URL_SAFE_NO_PAD.decode(&sig.signature) {
                                        if sig_bytes.len() < 96 || blst::min_pk::Signature::from_bytes(&sig_bytes).is_err() {
                                            warn!(
                                                "[ForkRecovery] Invalid BLS signature format at height {}",
                                                checkpoint.height
                                            );
                                            format_valid = false;
                                            break;
                                        }
                                    }
                                }
                                if !format_valid {
                                    break;
                                }
                            }
                            if !format_valid {
                                continue;
                            }
                            warn!(
                                "[ForkRecovery] No genesis validators configured - using format validation only (TESTNET MODE)"
                            );
                        }

                        let checkpoint_count = sync_snapshot.checkpoints.len();
                        let account_count = sync_snapshot.accounts.len();
                        let tx_count = sync_snapshot.dag_transactions.len();
                        let latest_height = sync_snapshot.checkpoints.last().map(|c| c.height).unwrap_or(0);

                        {
                            let mut state = self.state.inner.write().await;

                            state.checkpoints = sync_snapshot.checkpoints;
                            let sync_cp_height = state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
                            self.state.checkpoint_height_cache.store(sync_cp_height, std::sync::atomic::Ordering::Relaxed);

                            state.accounts.clear();
                            for (fingerprint, account) in sync_snapshot.accounts {
                                state.accounts.insert(fingerprint, account);
                            }

                            state.validators.clear();
                            for (addr, validator) in sync_snapshot.validators {
                                state.validators.insert(addr, validator);
                            }

                            let max_nodes = state.dag.node_count().max(10000);
                            state.dag = rinku_core::dag::Dag::new(max_nodes);

                            for tx in sync_snapshot.dag_transactions {
                                let parents = tx.tx.parents.clone();
                                let timestamp_ms = crate::config::normalize_timestamp_to_ms(tx.tx.timestamp);
                                let node = rinku_core::types::DagNode {
                                    hash: tx.hash.clone(),
                                    parents,
                                    children: vec![],
                                    weight: 1.0,
                                    finalized: true,
                                    checkpoint_height: Some(latest_height),
                                    tx: tx.clone(),
                                    received_at_ms: Some(timestamp_ms),
                                    partition_epoch: None,
                                    rolled_back: false,
                                    convergence_certificate: None,
                                };
                                let _ = state.dag.add_node(node);
                            }

                            state.current_gas_price = sync_snapshot.gas_price;
                            state.total_supply = sync_snapshot.total_supply;
                            state.genesis_time = sync_snapshot.genesis_time;
                        }

                        info!(
                            "[ForkRecovery] Applied snapshot from p2p peer {}: {} checkpoints, {} accounts, {} txs (height: {})",
                            peer_id_short, checkpoint_count, account_count, tx_count, latest_height
                        );

                        self.consecutive_fork_failures.store(0, std::sync::atomic::Ordering::SeqCst);

                        return Ok(true);
                    }
                    Ok(_) => {
                        debug!("[ForkRecovery] P2P peer {} returned unexpected response for snapshot", peer_id_short);
                    }
                    Err(e) => {
                        debug!("[ForkRecovery] Failed to reach p2p peer {}: {}", peer_id_short, e);
                    }
                }
            }
        }

        Err(anyhow::anyhow!("No peer had a valid snapshot for recovery"))
    }

    /// Record a fork failure (previous_hash mismatch) and potentially trigger recovery
    fn record_fork_failure(&self) -> bool {
        let failures = self.consecutive_fork_failures.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        
        if failures >= FORK_RECOVERY_THRESHOLD {
            warn!(
                "[ForkRecovery] {} consecutive previous_hash mismatches detected, triggering chain recovery",
                failures
            );
            true
        } else {
            debug!(
                "[ForkRecovery] previous_hash mismatch count: {}/{}",
                failures, FORK_RECOVERY_THRESHOLD
            );
            false
        }
    }

    /// Reset the fork failure counter (called on successful checkpoint adoption)
    fn reset_fork_failures(&self) {
        self.consecutive_fork_failures.store(0, std::sync::atomic::Ordering::SeqCst);
    }

    fn should_use_single_validator(quorum_reached: bool, mainnet_mode: bool) -> bool {
        !quorum_reached && !mainnet_mode
    }

    async fn create_state_snapshot(&mut self) -> Result<()> {
        let (height, previous_hash, local_checkpoint_height) = {
            let state = self.state.inner.read().await;
            let current_height = state.checkpoints.last().map(|c| c.height).unwrap_or(0);
            let height = current_height + 1;
            let previous_hash = state.checkpoints.last().map(|c| c.hash.clone());
            (height, previous_hash, current_height)
        };

        if height != self.last_seen_height {
            self.last_seen_height = height;
            self.stuck_iterations = 0;
        } else {
            self.stuck_iterations += 1;
        }

        if self.stuck_iterations > 3 {
            if let Some(ref gossip) = self.gossip_service {
                gossip.drain_checkpoint_buffer().await;
                let new_height = self.state.get_checkpoint_height();
                if new_height > local_checkpoint_height {
                    info!(
                        "Snapshot sync: buffer drain advanced height from {} to {} — resuming",
                        local_checkpoint_height, new_height
                    );
                    return Ok(());
                }
            }

            let unfinalized_count = {
                let state = self.state.inner.read().await;
                state.dag.unfinalized_count()
            };

            let is_proposer_for_reset = self.is_snapshot_proposer(height).await;

            if unfinalized_count > 0 && is_proposer_for_reset {
                self.stuck_iterations = 0;
                info!("Snapshot proposer: resetting stuck counter — {} unfinalized txs available (we are proposer)", unfinalized_count);
            } else if unfinalized_count == 0 {
                if self.stuck_iterations % 2 == 0 {
                    if self.fetch_and_apply_peer_checkpoint(height).await {
                        info!("Snapshot sync: recovered from peer at height {}", height);
                        return Ok(());
                    }
                }
                return Ok(());
            }
        }

        let is_proposer = self.is_snapshot_proposer(height).await;

        if !is_proposer {
            if let Some(ref gossip) = self.gossip_service {
                gossip.drain_checkpoint_buffer().await;
                let new_height = self.state.get_checkpoint_height();
                if new_height > local_checkpoint_height {
                    self.leader_wait_ticks = 0;
                    self.leader_wait_height = 0;
                    return Ok(());
                }
            }

            if self.stuck_iterations >= 3 && self.stuck_iterations % 2 == 1 {
                if self.fetch_and_apply_peer_checkpoint(height).await {
                    info!("Non-proposer recovered checkpoint at height {} from peer", height);
                    self.leader_wait_ticks = 0;
                    self.leader_wait_height = 0;
                    return Ok(());
                }
            }

            let unfinalized_count = {
                let state = self.state.inner.read().await;
                state.dag.unfinalized_count()
            };

            if unfinalized_count > 0 && height == self.leader_wait_height {
                self.leader_wait_ticks += 1;
            } else if unfinalized_count > 0 && height != self.leader_wait_height {
                self.leader_wait_height = height;
                self.leader_wait_ticks = 1;
            } else {
                self.leader_wait_ticks = 0;
                self.leader_wait_height = 0;
            }

            let backup_rank = if let Some(ref leader_election) = self.leader_election {
                let prev_hash = {
                    let state = self.state.inner.read().await;
                    state.checkpoints.last().map(|c| c.hash.clone()).unwrap_or_else(|| "genesis".to_string())
                };
                let validator_addresses_with_stakes: Vec<(String, u64)> = if let Some(ref identity) = self.validator_identity {
                    let identity_guard = identity.read().await;
                    identity_guard.active_validators()
                        .iter()
                        .map(|(addr, v)| (addr.clone(), v.effective_stake))
                        .collect()
                } else {
                    vec![(self.validator_address.clone(), 1)]
                };
                leader_election.get_backup_rank_from_validators(
                    height,
                    &prev_hash,
                    &validator_addresses_with_stakes,
                    &self.validator_address,
                ).unwrap_or(0)
            } else {
                0
            };

            let base_skip_threshold = LEADER_SKIP_BASE_TICKS + (backup_rank * LEADER_SKIP_STAGGER_TICKS);
            let has_valid_intent = if let Some(ref gossip) = self.gossip_service {
                gossip.has_valid_leader_intent(height, self.interval_ms).await
            } else {
                false
            };
            let skip_threshold = if has_valid_intent {
                if self.leader_wait_ticks == base_skip_threshold {
                    info!(
                        "Leader intent active for height {} — extending failover from {} to {} ticks",
                        height, base_skip_threshold, base_skip_threshold + LEADER_INTENT_EXTENSION_TICKS
                    );
                }
                base_skip_threshold + LEADER_INTENT_EXTENSION_TICKS
            } else {
                base_skip_threshold
            };

            if self.leader_wait_ticks >= skip_threshold {
                if let Some(ref gossip) = self.gossip_service {
                    gossip.drain_checkpoint_buffer().await;
                    let new_height = self.state.get_checkpoint_height();
                    if new_height > local_checkpoint_height {
                        info!(
                            "LEADER SKIP aborted: checkpoint arrived at height {} while preparing skip for height {} (after {} ticks)",
                            new_height, height, self.leader_wait_ticks
                        );
                        self.leader_wait_ticks = 0;
                        self.leader_wait_height = 0;
                        return Ok(());
                    }
                }

                if self.fetch_and_apply_peer_checkpoint(height).await {
                    info!(
                        "LEADER SKIP aborted: recovered leader's checkpoint at height {} from peer delta sync",
                        height
                    );
                    self.leader_wait_ticks = 0;
                    self.leader_wait_height = 0;
                    return Ok(());
                }

                info!(
                    "LEADER SKIP: Designated leader timed out for height {} after {} ticks (~{}s) — this node taking over (backup rank {}, {} unfinalized txs)",
                    height, self.leader_wait_ticks, self.leader_wait_ticks as u64 * self.interval_ms / 1000, backup_rank, unfinalized_count
                );
                self.leader_wait_ticks = 0;
                self.leader_wait_height = 0;
            } else {
                return Ok(());
            }
        } else {
            self.leader_wait_ticks = 0;
            self.leader_wait_height = 0;
        }

        if let Some(ref gossip) = self.gossip_service {
            gossip.broadcast_checkpoint_intent(height, &self.validator_address).await;
        }

        let (hashes, txs, merkle_root) = self.gather_unfinalized_txs(height, true, &previous_hash).await?;

        if hashes.is_empty() {
            return Ok(());
        }

        let t_start = std::time::Instant::now();

        if let Some(ref gossip) = self.gossip_service {
            gossip.drain_checkpoint_buffer().await;
            let new_height = self.state.get_checkpoint_height();
            if new_height >= height {
                info!(
                    "Checkpoint production aborted: height {} already covered (tip now {}) — late checkpoint arrived before emission",
                    height, new_height
                );
                return Ok(());
            }
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();

        let checkpoint_reward = {
            let mut emission = self.state.emission.write().await;
            let reward = emission.get_checkpoint_reward(height);
            let is_new = emission.record_emission_for_height(height, reward);

            if is_new {
                let mut rewards = self.state.rewards.write().await;
                let distributions = rewards.distribute_checkpoint_rewards(reward);
                if !distributions.is_empty() {
                    info!(
                        "Pre-distributed {:.6} RKU to {} validators before state root computation",
                        rinku_core::types::from_micro_units(reward),
                        distributions.len()
                    );
                }
            }

            reward
        };

        let mut affected_addresses_for_proofs: std::collections::HashSet<String> = std::collections::HashSet::new();
        for tx in &txs {
            affected_addresses_for_proofs.insert(tx.tx.from.clone());
            if !tx.tx.to.is_empty() {
                affected_addresses_for_proofs.insert(tx.tx.to.clone());
            }
        }
        let affected_vec: Vec<String> = affected_addresses_for_proofs.into_iter().collect();

        let convergence_executed = if let Some(ref gossip) = self.gossip_service {
            gossip.get_all_convergence_executed().await
        } else {
            std::collections::HashSet::new()
        };

        let proofs_result = self.state.compute_state_root_and_proofs_at_height(
            &txs, &affected_vec, height, &convergence_executed
        ).await;
        let state_root = proofs_result.state_root.clone();
        let finalized_proofs = proofs_result.proofs;
        let tip_count = hashes.len() as u32;

        let checkpoint_hash_bytes = Self::compute_checkpoint_hash(
            height,
            &merkle_root,
            &state_root,
            &"0".repeat(64),
            tip_count,
            timestamp,
        );

        let signature = bls_sign(&checkpoint_hash_bytes, &self.bls_private_key)
            .map_err(|e| anyhow::anyhow!("BLS signing failed: {}", e))?;

        let my_stake = self.state.get_validator_stake(&self.validator_address).await.unwrap_or(0);
        let proposer_sig = ValidatorSignature {
            validator: self.validator_address.clone(),
            signature: URL_SAFE_NO_PAD.encode(&signature),
            weight: my_stake,
            bls_public_key: Some(self.bls_public_key_base64()),
        };

        let proposer_bitmap = if let Some(ref identity) = self.validator_identity {
            let identity_guard = identity.read().await;
            let mut sorted_addrs: Vec<&String> = identity_guard.active_validators()
                .iter()
                .filter(|(_, v)| !v.bls_public_key.is_empty())
                .map(|(addr, _)| addr)
                .collect();
            sorted_addrs.sort();
            let total_validators = sorted_addrs.len();
            if let Some(my_index) = sorted_addrs.iter().position(|a| **a == self.validator_address) {
                Some(create_signer_bitmap(&[my_index], total_validators))
            } else {
                warn!(
                    "Proposer {} not found in sorted validator set ({} validators) — checkpoint will be unsigned",
                    &self.validator_address[..16.min(self.validator_address.len())],
                    total_validators
                );
                None
            }
        } else {
            None
        };

        let partition_info = self.state.get_partition_state().await;
        let is_partitioned = partition_info.status == crate::state::partition::PartitionStatus::Partitioned;

        let weight_trie_root = {
            let (all_stakes, total_network_stake): (std::collections::HashMap<String, u64>, u64) = {
                let state = self.state.inner.read().await;
                let mut stakes: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
                for (addr, v) in state.validators.iter() {
                    if v.stake > 0 {
                        stakes.insert(addr.clone(), v.stake);
                    }
                }
                for (addr, account) in state.accounts.iter() {
                    if account.staked > 0 {
                        let stake_micro = account.staked;
                        stakes.entry(addr.clone())
                            .and_modify(|s| *s = (*s).max(stake_micro))
                            .or_insert(stake_micro);
                    }
                }
                let total: u64 = stakes.values().sum();
                (stakes, total)
            };

            let mut state = self.state.inner.write().await;
            if let Some(ref mut weight_trie) = state.weight_trie {
                let pending_count = weight_trie.pending_vote_count();
                if pending_count > 0 {
                    let updated = weight_trie.finalize_votes(&all_stakes, total_network_stake);
                    info!(
                        "Snapshot {}: finalized {} pending weight votes into {} tx aggregations",
                        height, pending_count, updated.len()
                    );
                }
                weight_trie.compute_root()
            } else {
                String::new()
            }
        };

        let checkpoint = Checkpoint {
            height,
            hash: hex::encode(&checkpoint_hash_bytes),
            previous_hash,
            tx_merkle_root: merkle_root,
            state_root,
            receipt_root: "0".repeat(64),
            tip_count,
            timestamp,
            validator_signatures: vec![proposer_sig],
            aggregated_signature: if proposer_bitmap.is_some() { Some(URL_SAFE_NO_PAD.encode(&signature)) } else { None },
            signer_bitmap: proposer_bitmap,
            finalized_tx_hashes: hashes.clone(),
            weight_trie_root,
            provisional: is_partitioned,
            partition_epoch: if is_partitioned { partition_info.current_epoch } else { None },
            visible_stake_pct: if is_partitioned { Some(partition_info.visible_stake_pct) } else { None },
            merge_report_hash: None,
        };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut txs_to_execute: Vec<SignedTransaction> = {
            let state = self.state.inner.read().await;
            hashes.iter()
                .filter_map(|hash| {
                    state.dag.get_node(hash).and_then(|node| {
                        if node.finalized { None } else { Some(node.tx.clone()) }
                    })
                })
                .collect()
        };

        let finality_times: Vec<u64> = Vec::new();
        let finality_sum: u64 = 0;
        let finality_max: u64 = 0;
        let finality_count: u64 = 0;

        if let Some(ref gossip) = self.gossip_service {
            gossip.drain_checkpoint_buffer().await;
            let new_height = self.state.get_checkpoint_height();
            if new_height >= height {
                info!(
                    "Checkpoint production aborted: height {} already covered (tip now {}) — late checkpoint arrived during computation",
                    height, new_height
                );
                return Ok(());
            }
        }

        let mut state = self.state.inner.write().await;
        let current_tip = state.checkpoints.last().map(|c| c.height).unwrap_or(0);
        if current_tip + 1 != height {
            drop(state);
            info!(
                "LEADER SKIP ABORT: Local tip advanced to {} while computing checkpoint {} — another node produced it first",
                current_tip, height
            );
            return Ok(());
        }
        state.checkpoints.push(checkpoint.clone());
        state.last_checkpoint_time_ms = now_ms;
        self.state.checkpoint_height_cache.store(checkpoint.height, std::sync::atomic::Ordering::Relaxed);
        state.finality_sum_ms += finality_sum;
        state.finality_count += finality_count;
        if finality_max > state.finality_max_ms {
            state.finality_max_ms = finality_max;
        }
        for finality_time in &finality_times {
            if state.finality_times_ms.len() >= 1000 {
                state.finality_times_ms.pop_front();
            }
            state.finality_times_ms.push_back(*finality_time);
        }

        let _finalized = state.dag.mark_finalized_batch(&hashes, height);

        for hash in &hashes {
            state.convergence_executed_hashes.remove(hash);
        }

        if is_partitioned {
            for hash in &hashes {
                if let Some(node) = state.dag.get_node_mut(hash) {
                    node.partition_epoch = partition_info.current_epoch;
                }
            }
        }

        let snapshot_finalized_count = hashes.len() as u64;
        drop(state);

        if snapshot_finalized_count > 0 {
            self.state.record_finalized_batch(snapshot_finalized_count).await;
        }

        let prep_ms = t_start.elapsed().as_millis();
        info!(
            "Created state snapshot {} at height {} ({} txs finalized, {:.6} RKU emitted, {}ms)",
            &checkpoint.hash[..16],
            height,
            hashes.len(),
            rinku_core::types::from_micro_units(checkpoint_reward),
            prep_ms
        );

        if let Some(ref eb) = self.event_bus {
            eb.publish(crate::events::NodeEvent::CheckpointCreated {
                hash: checkpoint.hash.clone(),
                height,
                txs_finalized: hashes.len(),
                reward: rinku_core::types::from_micro_units(checkpoint_reward),
            });
        }

        let proof_tx_hash = hashes.first()
            .cloned()
            .unwrap_or_else(|| checkpoint.hash.clone());

        let mut final_proofs = finalized_proofs;
        for proof in final_proofs.values_mut() {
            proof.checkpoint_hash = checkpoint.hash.clone();
            proof.checkpoint_timestamp = checkpoint.timestamp;
            proof.bls_aggregated_sig = checkpoint.aggregated_signature.clone();
            proof.bls_signer_bitmap = checkpoint.signer_bitmap.as_ref().map(|b| hex::encode(b));
            proof.tx_hash = proof_tx_hash.clone();
        }

        txs_to_execute.sort_by(|a, b| {
            a.tx.from.cmp(&b.tx.from)
                .then(a.tx.nonce.cmp(&b.tx.nonce))
                .then(a.hash.cmp(&b.hash))
        });

        let mut newly_applied: Vec<bool> = Vec::with_capacity(txs_to_execute.len());
        for tx in &txs_to_execute {
            if !convergence_executed.contains(&tx.hash) {
                let applied = self.state.execute_finalized_transaction_core(tx).await;
                newly_applied.push(applied);
            } else {
                newly_applied.push(true);
            }
        }

        for (i, tx) in txs_to_execute.iter().enumerate() {
            if newly_applied.get(i).copied().unwrap_or(false) {
                self.state.execute_finalized_transaction_rewards(tx).await;
            }
        }

        self.state.store_precomputed_proofs(&final_proofs).await;

        if let Some(ref consensus) = self.consensus_service {
            let participating_validators: Vec<String> = checkpoint.validator_signatures
                .iter()
                .map(|sig| sig.validator.clone())
                .collect();
            let mut consensus_guard = consensus.write().await;
            consensus_guard.track_liveness(height, &participating_validators).await;
        }

        if let Some(ref gossip) = self.gossip_service {
            let proofs_vec: Vec<rinku_core::types::AccountStateProof> =
                final_proofs.values().cloned().collect();
            gossip.broadcast_checkpoint(
                checkpoint.clone(),
                hashes.clone(),
                txs_to_execute.clone(),
                proofs_vec.clone(),
            ).await;

            #[cfg(feature = "p2p")]
            if let Some(ref network) = self.network_handle {
                let push_data = CheckpointPushData {
                    checkpoint,
                    finalized_tx_hashes: hashes,
                    finalized_transactions: txs_to_execute,
                    precomputed_proofs: proofs_vec,
                };
                let peer_ids = network.get_connected_peer_ids().await;
                let peer_count = peer_ids.len();
                let net = network.clone();
                tokio::spawn(async move {
                    let mut sent = 0usize;
                    for peer_id in &peer_ids {
                        let request = SyncRequest::CheckpointPush(push_data.clone());
                        match net.send_sync_request(peer_id, request).await {
                            Ok(_rx) => { sent += 1; }
                            Err(e) => {
                                debug!("Failed to push snapshot to {}: {}", &peer_id[..12.min(peer_id.len())], e);
                            }
                        }
                    }
                    if sent > 0 {
                        info!("Pushed state snapshot to {}/{} peers via sync channel", sent, peer_count);
                    }
                });
            }
        }

        Ok(())
    }

    async fn is_snapshot_proposer(&self, height: u64) -> bool {
        if let Some(ref leader_election) = self.leader_election {
            let prev_hash = {
                let state = self.state.inner.read().await;
                state.checkpoints.last().map(|c| c.hash.clone()).unwrap_or_else(|| "genesis".to_string())
            };

            let validator_addresses_with_stakes: Vec<(String, u64)> = if let Some(ref identity) = self.validator_identity {
                let identity_guard = identity.read().await;
                identity_guard.active_validators()
                    .iter()
                    .map(|(addr, v)| (addr.clone(), v.effective_stake))
                    .collect()
            } else {
                vec![(self.validator_address.clone(), 1)]
            };

            let (should_create, _) = leader_election.should_create_checkpoint_from_validators(
                height,
                &prev_hash,
                &validator_addresses_with_stakes,
                &self.validator_address,
            );

            if should_create {
                if let Some(ref gossip) = self.gossip_service {
                    let has_next = gossip.has_buffered_checkpoint(height).await;
                    if has_next {
                        return false;
                    }
                }
            }

            should_create
        } else {
            true
        }
    }

    async fn gather_unfinalized_txs(
        &mut self,
        height: u64,
        is_leader: bool,
        _previous_hash: &Option<String>,
    ) -> Result<(Vec<String>, Vec<SignedTransaction>, String)> {
        let (mut unfinalized, mut unfinalized_txs, total_count, too_new_count, eligible_count) = {
            let state = self.state.inner.read().await;

            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let cutoff_time = now_ms.saturating_sub(PROPAGATION_GRACE_MS);

            let tx_cap = self.dynamic_tx_cap;

            let all_unfinalized = state.dag.get_unfinalized_nodes();
            let total = all_unfinalized.len();
            let mut unfinalized_nodes: Vec<_> = all_unfinalized
                .iter()
                .filter(|n| n.tx.tx.timestamp <= cutoff_time)
                .filter(|n| Self::is_valid_hex_hash(&n.hash))
                .collect();

            let too_new = total - unfinalized_nodes.len();
            let eligible = unfinalized_nodes.len();

            if eligible > tx_cap {
                unfinalized_nodes.sort_by(|a, b| {
                    let gas_a = a.tx.tx.gas_price.unwrap_or(0);
                    let gas_b = b.tx.tx.gas_price.unwrap_or(0);
                    gas_b.cmp(&gas_a)
                        .then_with(|| a.tx.tx.timestamp.cmp(&b.tx.tx.timestamp))
                });
                unfinalized_nodes.truncate(tx_cap);
            }

            let hashes: Vec<String> = unfinalized_nodes.iter()
                .map(|n| n.hash.clone())
                .collect();

            let txs: Vec<SignedTransaction> = unfinalized_nodes.iter()
                .map(|n| n.tx.clone())
                .collect();

            if total > hashes.len() {
                debug!(
                    "Propagation grace: {} of {} unfinalized txs excluded (too new, cutoff={}ms)",
                    total - hashes.len(),
                    total,
                    PROPAGATION_GRACE_MS
                );
            }

            (hashes, txs, total, too_new, eligible)
        };

        let tx_cap = self.dynamic_tx_cap;
        if eligible_count > tx_cap {
            let new_cap = ((tx_cap as f64) * 2.0).ceil() as usize;
            let new_cap = new_cap.min(DYNAMIC_TX_CAP_MAX).max(DYNAMIC_TX_CAP_MIN);
            info!(
                "Checkpoint {} congested: {} eligible txs, dynamic cap {} -> {}, prioritizing by gas price",
                height, eligible_count, tx_cap, new_cap
            );
            self.dynamic_tx_cap = new_cap;
        } else if eligible_count < tx_cap / 2 && tx_cap > DYNAMIC_TX_CAP_DEFAULT {
            let new_cap = ((tx_cap as f64) * 0.95).floor() as usize;
            let new_cap = new_cap.max(DYNAMIC_TX_CAP_DEFAULT);
            if new_cap != tx_cap {
                info!(
                    "Checkpoint {} under-utilized: {} eligible txs, dynamic cap {} -> {}",
                    height, eligible_count, tx_cap, new_cap
                );
                self.dynamic_tx_cap = new_cap;
            }
        }

        const LEADER_MIN_TX_THRESHOLD: usize = 1;

        if !is_leader && unfinalized.is_empty() {
            return Ok((vec![], vec![], String::new()));
        }

        let needs_peer_sync = is_leader && unfinalized.len() < LEADER_MIN_TX_THRESHOLD;
        if needs_peer_sync {
            #[cfg(feature = "p2p")]
            {
                if let Some(ref network_handle) = self.network_handle {
                    info!(
                        "Leader has 0 eligible txs for checkpoint {} — syncing from peers",
                        height
                    );
                    let peers = network_handle.get_connected_peers().await;
                    let local_cp_height = self.state.get_checkpoint_height();

                    let mut peer_futures = Vec::new();
                    for peer in &peers {
                        let peer_id = peer.peer_id.clone();
                        let nh = network_handle.clone();
                        peer_futures.push(async move {
                            let result = tokio::time::timeout(
                                std::time::Duration::from_millis(800),
                                nh.request_delta(&peer_id, local_cp_height),
                            ).await;
                            (peer_id, result)
                        });
                    }

                    let results = futures::future::join_all(peer_futures).await;
                    let mut total_ingested = 0u64;

                    for (peer_id, result) in results {
                        let result = match result {
                            Ok(r) => r,
                            Err(_) => {
                                debug!("Pre-checkpoint sync to peer {} timed out", &peer_id[..16.min(peer_id.len())]);
                                continue;
                            }
                        };
                        if let Ok(SyncResponse::Delta(delta)) = result {
                            for tx_data in &delta.transactions {
                                let stx = SignedTransaction {
                                    tx: rinku_core::types::Transaction {
                                        from: tx_data.from.clone(),
                                        to: tx_data.to.clone(),
                                        amount: tx_data.amount,
                                        nonce: tx_data.nonce,
                                        timestamp: tx_data.timestamp,
                                        parents: tx_data.parents.clone(),
                                        kind: None,
                                        gas_limit: None,
                                        gas_price: Some(tx_data.gas_price),
                                        data: None,
                                        signature: Some(tx_data.signature.clone()),
                                        memo: tx_data.memo.clone(),
                                        references: tx_data.references.clone(),
                                    },
                                    hash: tx_data.hash.clone(),
                                    signature: tx_data.signature.clone(),
                                };
                                if self.state.add_transaction_from_sync(stx).await.is_ok() {
                                    total_ingested += 1;
                                }
                            }
                        }
                    }

                    if total_ingested > 0 {
                        info!(
                            "Pre-checkpoint sync ingested {} txs from peers — re-checking unfinalized",
                            total_ingested
                        );
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let cutoff_time = now_ms.saturating_sub(PROPAGATION_GRACE_MS);
                        let tx_cap = self.dynamic_tx_cap;

                        let state = self.state.inner.read().await;
                        let all_unfinalized = state.dag.get_unfinalized_nodes();
                        let mut eligible: Vec<_> = all_unfinalized
                            .iter()
                            .filter(|n| n.tx.tx.timestamp <= cutoff_time)
                            .filter(|n| Self::is_valid_hex_hash(&n.hash))
                            .collect();

                        if !eligible.is_empty() {
                            if eligible.len() > tx_cap {
                                eligible.sort_by(|a, b| {
                                    let gas_a = a.tx.tx.gas_price.unwrap_or(0);
                                    let gas_b = b.tx.tx.gas_price.unwrap_or(0);
                                    gas_b.cmp(&gas_a)
                                        .then_with(|| a.tx.tx.timestamp.cmp(&b.tx.tx.timestamp))
                                });
                                eligible.truncate(tx_cap);
                            }
                            info!(
                                "Peer sync recovered {} eligible txs for checkpoint {} (was {})",
                                eligible.len(), height, unfinalized.len()
                            );
                            unfinalized = eligible.iter().map(|n| n.hash.clone()).collect();
                            unfinalized_txs = eligible.iter().map(|n| n.tx.clone()).collect();
                            drop(state);
                        } else {
                            drop(state);
                        }
                    }
                }
            }
        }

        if is_leader && unfinalized.is_empty() {
            debug!(
                "Leader skip: checkpoint {} has 0 eligible txs after peer sync ({} unfinalized, {} too new) — network idle",
                height, total_count, too_new_count
            );
            return Ok((vec![], vec![], String::new()));
        }

        unfinalized.sort();

        let tx_merkle_root = if unfinalized.is_empty() {
            "0".repeat(64)
        } else {
            let hashes_clone = unfinalized.clone();
            let tree = tokio::task::spawn_blocking(move || MerkleTree::from_hex_leaves(&hashes_clone))
                .await
                .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))??;
            tree.root()
        };

        Ok((unfinalized, unfinalized_txs, tx_merkle_root))
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_checkpoint_hash_deterministic() {
        let hash1 = CheckpointService::compute_checkpoint_hash(
            100,
            "merkle_root_abc",
            "state_root_def",
            "receipt_root_ghi",
            50,
            1700000000,
        );
        
        let hash2 = CheckpointService::compute_checkpoint_hash(
            100,
            "merkle_root_abc",
            "state_root_def",
            "receipt_root_ghi",
            50,
            1700000000,
        );
        
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 32);
    }

    #[test]
    fn test_compute_checkpoint_hash_different_heights() {
        let hash1 = CheckpointService::compute_checkpoint_hash(
            100,
            "merkle_root",
            "state_root",
            "receipt_root",
            50,
            1700000000,
        );
        
        let hash2 = CheckpointService::compute_checkpoint_hash(
            101,
            "merkle_root",
            "state_root",
            "receipt_root",
            50,
            1700000000,
        );
        
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_compute_checkpoint_hash_different_merkle_roots() {
        let hash1 = CheckpointService::compute_checkpoint_hash(
            100,
            "merkle_root_a",
            "state_root",
            "receipt_root",
            50,
            1700000000,
        );
        
        let hash2 = CheckpointService::compute_checkpoint_hash(
            100,
            "merkle_root_b",
            "state_root",
            "receipt_root",
            50,
            1700000000,
        );
        
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_compute_checkpoint_hash_different_timestamps() {
        let hash1 = CheckpointService::compute_checkpoint_hash(
            100,
            "merkle_root",
            "state_root",
            "receipt_root",
            50,
            1700000000,
        );
        
        let hash2 = CheckpointService::compute_checkpoint_hash(
            100,
            "merkle_root",
            "state_root",
            "receipt_root",
            50,
            1700000001,
        );
        
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_compute_checkpoint_hash_different_tip_counts() {
        let hash1 = CheckpointService::compute_checkpoint_hash(
            100,
            "merkle_root",
            "state_root",
            "receipt_root",
            50,
            1700000000,
        );
        
        let hash2 = CheckpointService::compute_checkpoint_hash(
            100,
            "merkle_root",
            "state_root",
            "receipt_root",
            51,
            1700000000,
        );
        
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_should_use_single_validator_mode() {
        assert!(CheckpointService::should_use_single_validator(false, false));
        assert!(!CheckpointService::should_use_single_validator(false, true));
        assert!(!CheckpointService::should_use_single_validator(true, false));
        assert!(!CheckpointService::should_use_single_validator(true, true));
    }

    #[test]
    fn test_compute_checkpoint_hash_hex_encoding() {
        let hash = CheckpointService::compute_checkpoint_hash(
            1,
            "test_merkle",
            "test_state",
            "test_receipt",
            10,
            1000000,
        );
        
        let hex_hash = hex::encode(&hash);
        assert_eq!(hex_hash.len(), 64);
        assert!(hex_hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_fork_recovery_threshold_constant() {
        assert_eq!(FORK_RECOVERY_THRESHOLD, 3);
    }
}
