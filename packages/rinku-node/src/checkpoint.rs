use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures::future::join_all;
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
use crate::network::{CheckpointData, CheckpointVoteRequest, CheckpointVoteResponse, NetworkHandle, SyncRequest, SyncResponse};
use crate::slashing::SlashingService;
use crate::state::NodeState;
use crate::trust::TrustVerifier;
use crate::validator_identity::ValidatorIdentityService;

/// Quorum threshold for multi-validator checkpoints (2/3 of stake)
/// Use 0.6666 instead of 0.667 to allow exactly 2/3 validators to reach quorum
/// (e.g., 2000/3000 = 0.6667 >= 0.6666, but 2000/3000 < 0.667)
const QUORUM_STAKE_THRESHOLD: f64 = 0.6666;

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
    pruning_service: Option<DagPruningService>,
    pruning_counter: std::sync::atomic::AtomicU32,
    consensus_service: Option<Arc<RwLock<ConsensusService>>>,
    slashing_service: Option<Arc<RwLock<SlashingService>>>,
    #[cfg(feature = "p2p")]
    network_handle: Option<Arc<tokio::sync::Mutex<NetworkHandle>>>,
    /// Our validator's stake (for quorum calculation)
    our_stake: u64,
    /// Leader election service for checkpoint creation
    leader_election: Option<LeaderElectionService>,
    /// Our public URL for leader election
    local_url: Option<String>,
    /// Enforce strict quorum/signature requirements
    mainnet_mode: bool,
    /// GossipService for immediate checkpoint broadcast
    gossip_service: Option<Arc<crate::gossip::GossipService>>,
    event_bus: Option<Arc<crate::events::EventBus>>,
}

const FORK_RECOVERY_THRESHOLD: u32 = 3;

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
            pruning_service: Some(DagPruningService::new(PruningConfig::default())),
            pruning_counter: std::sync::atomic::AtomicU32::new(0),
            consensus_service: None,
            slashing_service: None,
            #[cfg(feature = "p2p")]
            network_handle: None,
            our_stake: 10_000_000_000, // Default stake in micro-units (100 RKU)
            leader_election: None,
            local_url: None,
            mainnet_mode,
            gossip_service: None,
            event_bus: None,
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
        self.pruning_service = Some(DagPruningService::new(config));
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
    pub fn with_network_handle(mut self, handle: Arc<tokio::sync::Mutex<NetworkHandle>>) -> Self {
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
        
        let interval = tokio::time::Duration::from_millis(self.interval_ms);

        loop {
            tokio::time::sleep(interval).await;

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

            if let Err(e) = self.create_checkpoint().await {
                tracing::warn!("Checkpoint creation failed: {}", e);
            }
            
            let prune_count = self.pruning_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            const PRUNE_EVERY_N_CHECKPOINTS: u32 = 10;
            if prune_count > 0 && prune_count % PRUNE_EVERY_N_CHECKPOINTS == 0 {
                let state_guard = self.state.inner.read().await;
                // CRITICAL: Use actual checkpoint height, NOT len() which breaks after pruning
                let current_height = state_guard.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
                let finalized_hashes: std::collections::HashSet<String> = state_guard.dag
                    .get_all_nodes()
                    .iter()
                    .filter(|n| n.finalized)
                    .map(|n| n.hash.clone())
                    .collect();
                drop(state_guard);
                
                if current_height > 100 {
                    if let Some(ref mut pruning) = self.pruning_service.as_mut() {
                        let storage = self.state.storage();
                        match pruning.prune_dag(storage.as_ref(), current_height, &finalized_hashes) {
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

    /// Check if any peer has a checkpoint at the given height via P2P delta sync
    /// If found, returns the peer's checkpoint to adopt instead of creating our own
    async fn fetch_peer_checkpoint(&self, height: u64) -> Option<Checkpoint> {
        #[cfg(feature = "p2p")]
        {
            let network_handle = self.network_handle.as_ref()?;
            let handle = network_handle.lock().await;
            let peers = handle.get_connected_peers().await;
            drop(handle);

            let from_cp = if height > 0 { height - 1 } else { 0 };

            for peer in &peers {
                let peer_id = peer.peer_id.clone();
                let result = {
                    let handle = network_handle.lock().await;
                    handle.request_delta(&peer_id, from_cp).await
                };
                match result {
                    Ok(SyncResponse::Delta(delta)) => {
                        for cp_data in &delta.new_checkpoints {
                            if cp_data.height == height {
                                let checkpoint = self.checkpoint_data_to_checkpoint(cp_data, &delta.new_checkpoints);
                                info!(
                                    "Found peer checkpoint at height {} from p2p peer {}: {}",
                                    height, &peer_id[..16.min(peer_id.len())],
                                    &checkpoint.hash[..16.min(checkpoint.hash.len())]
                                );
                                return Some(checkpoint);
                            }
                        }
                        debug!("P2P peer {} had no checkpoint at height {} in delta", &peer_id[..16.min(peer_id.len())], height);
                    }
                    Ok(_) => {
                        debug!("P2P peer {} returned unexpected response for delta", &peer_id[..16.min(peer_id.len())]);
                    }
                    Err(e) => {
                        debug!("Failed to request delta from p2p peer {}: {}", &peer_id[..16.min(peer_id.len())], e);
                    }
                }
            }
        }
        None
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
            state_root: cp_data.merkle_root.clone(),
            receipt_root: String::new(),
            tip_count: 0,
            timestamp: cp_data.timestamp,
            validator_signatures: Vec::new(),
            aggregated_signature: cp_data.signature.clone(),
            signer_bitmap: None,
            finalized_tx_hashes: Vec::new(),
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

            let handle = network_handle.lock().await;
            let peers = handle.get_connected_peers().await;
            drop(handle);

            for peer in &peers {
                let peer_id = peer.peer_id.clone();
                let result = {
                    let handle = network_handle.lock().await;
                    handle.request_delta(&peer_id, from_checkpoint).await
                };
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
                                    provisional_finality: false,
                                    rolled_back: false,
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
            emission.record_emission(reward);
            reward
        };

        // Distribute checkpoint rewards
        let distributions = {
            let mut rewards = self.state.rewards.write().await;
            rewards.distribute_checkpoint_rewards(checkpoint_reward)
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
        let txs_to_execute: Vec<SignedTransaction> = {
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

        // Update state with adopted checkpoint
        let mut state = self.state.inner.write().await;
        state.checkpoints.push(peer_checkpoint);
        state.last_checkpoint_time_ms = now_ms;

        // Mark transactions as finalized
        for hash in unfinalized_hashes {
            let _ = state.dag.mark_finalized(hash, height);
        }
        state.total_transactions += unfinalized_hashes.len() as u64;

        drop(state);

        info!(
            "Adopted peer checkpoint {} at height {} ({} txs finalized, {:.6} RKU emitted)",
            &checkpoint_hash[..16.min(checkpoint_hash.len())],
            height,
            unfinalized_hashes.len(),
            rinku_core::types::from_micro_units(checkpoint_reward)
        );
        
        // Execute finalized transactions - skip those already executed on fast-path
        let fp_executed = if let Some(ref gossip) = self.gossip_service {
            gossip.get_all_fast_path_executed().await
        } else {
            std::collections::HashSet::new()
        };
        for tx in txs_to_execute {
            if fp_executed.contains(&tx.hash) {
                tracing::debug!(
                    "Skipping fast-path-executed tx {} at checkpoint (already applied)",
                    &tx.hash[..16.min(tx.hash.len())]
                );
                self.state.execute_finalized_transaction_rewards(&tx).await;
            } else {
                self.state.execute_finalized_transaction(&tx).await;
            }
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

            let handle = network_handle.lock().await;
            let peers = handle.get_connected_peers().await;
            drop(handle);

            if peers.is_empty() {
                return Err(anyhow::anyhow!("No P2P peers available for chain recovery"));
            }

            for peer in &peers {
                let peer_id = peer.peer_id.clone();
                let peer_id_short = &peer_id[..16.min(peer_id.len())];
                info!("[ForkRecovery] Requesting full snapshot sync from p2p peer {}", peer_id_short);

                let result = {
                    let handle = network_handle.lock().await;
                    handle.request_snapshot(&peer_id).await
                };
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
                                    provisional_finality: false,
                                    rolled_back: false,
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

    async fn create_checkpoint(&self) -> Result<()> {
        // STEP 1: Get checkpoint height and previous hash first (needed for leader election)
        // CRITICAL: Use actual checkpoint height, NOT len() which breaks after pruning
        let (height, previous_hash) = {
            let state = self.state.inner.read().await;
            let current_height = state.checkpoints.last().map(|c| c.height).unwrap_or(0);
            let height = current_height + 1;
            let previous_hash = state.checkpoints.last().map(|c| c.hash.clone());
            (height, previous_hash)
        };

        // STEP 2: LEADER ELECTION FIRST - before checking unfinalized transactions
        // This prevents deadlock where:
        // - Leader has no unfinalized txs (behind), returns early without creating checkpoint
        // - Non-leaders wait for leader's checkpoint that never comes
        // 
        // CRITICAL: Use validator addresses (synced across all nodes) instead of peer URLs
        // to ensure ALL nodes elect the same leader. This prevents divergent checkpoint creation.
        let is_leader = if let Some(ref leader_election) = self.leader_election {
            let prev_hash_for_election = previous_hash.as_deref().unwrap_or("genesis");
            
            // Get validator addresses from the synced validator registry
            // This ensures deterministic leader election across all nodes
            let mut validator_addresses: Vec<String> = if let Some(ref identity) = self.validator_identity {
                let identity_guard = identity.read().await;
                identity_guard.active_validators()
                    .keys()
                    .cloned()
                    .collect()
            } else {
                // Fallback to using our own address if no validator identity service
                vec![self.validator_address.clone()]
            };
            
            // Sort for deterministic ordering across all nodes
            validator_addresses.sort();
            
            // Log validator set at INFO level to diagnose leader election divergence
            let validator_preview: Vec<String> = validator_addresses.iter()
                .take(5)
                .map(|a| a[..16.min(a.len())].to_string())
                .collect();
            info!(
                "Leader election input: checkpoint={}, prev_hash={}, validators={} {:?}, local={}",
                height, &prev_hash_for_election[..12.min(prev_hash_for_election.len())], 
                validator_addresses.len(), validator_preview,
                &self.validator_address[..16.min(self.validator_address.len())]
            );
            
            let (should_create, election_result) = leader_election.should_create_checkpoint_from_validators(
                height,
                prev_hash_for_election,
                &validator_addresses,
                &self.validator_address,
            );
            
            if !should_create {
                // We are not the leader - try to fetch and adopt leader's checkpoint
                debug!(
                    "LEADER ELECTION: Not elected for checkpoint {} (leader: {}), waiting for peer checkpoint",
                    height,
                    &election_result.leader_address[..16.min(election_result.leader_address.len())]
                );
                
                // Try to fetch and adopt the leader's checkpoint
                if let Some(peer_checkpoint) = self.fetch_peer_checkpoint(height).await {
                    // CRITICAL FIX: Use leader's finalized_tx_hashes if available
                    // This ensures we finalize the exact same transaction set as the leader
                    let (unfinalized_hashes, tx_merkle_root) = if !peer_checkpoint.finalized_tx_hashes.is_empty() {
                        // Leader provided exact tx list - verify we have them and use that
                        let state = self.state.inner.read().await;
                        let missing_count = peer_checkpoint.finalized_tx_hashes.iter()
                            .filter(|hash| state.dag.get_node(hash).is_none())
                            .count();
                        
                        if missing_count > 0 {
                            warn!(
                                "Cannot adopt checkpoint {}: missing {}/{} transactions from leader's list",
                                height, missing_count, peer_checkpoint.finalized_tx_hashes.len()
                            );
                            return Ok(()); // Skip this round, will retry later
                        }
                        
                        // Compute merkle root from leader's exact list
                        let mut sorted_hashes = peer_checkpoint.finalized_tx_hashes.clone();
                        sorted_hashes.sort();
                        let merkle_root = MerkleTree::from_hex_leaves(&sorted_hashes)
                            .map(|t| t.root())
                            .unwrap_or_else(|_| "0".repeat(64));
                        
                        info!(
                            "Using leader's {} finalized tx hashes for checkpoint {} adoption",
                            peer_checkpoint.finalized_tx_hashes.len(), height
                        );
                        (peer_checkpoint.finalized_tx_hashes.clone(), merkle_root)
                    } else {
                        // Legacy checkpoint without tx list - compute from local state
                        let state = self.state.inner.read().await;
                        
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let cutoff_time = now_ms.saturating_sub(PROPAGATION_GRACE_MS);
                        
                        let mut unfinalized: Vec<String> = state
                            .dag
                            .get_unfinalized_nodes()
                            .iter()
                            .filter(|n| n.tx.tx.timestamp <= cutoff_time)
                            .map(|n| n.hash.clone())
                            .filter(|h| Self::is_valid_hex_hash(h))
                            .collect();
                        unfinalized.sort();
                        let tx_merkle_root = if unfinalized.is_empty() {
                            "0".repeat(64)
                        } else {
                            MerkleTree::from_hex_leaves(&unfinalized).map(|t| t.root()).unwrap_or_else(|_| "0".repeat(64))
                        };
                        (unfinalized, tx_merkle_root)
                    };
                    
                    let (adopted, _) = self.validate_and_adopt_peer_checkpoint(
                        peer_checkpoint,
                        &tx_merkle_root,
                        previous_hash.as_deref(),
                        &unfinalized_hashes,
                    ).await;
                    
                    if adopted {
                        self.reset_fork_failures();
                        return Ok(());
                    }
                }
                
                // No checkpoint from leader yet - skip this round
                // Leader might be behind or still creating
                return Ok(());
            }
            
            true // We are the leader
        } else {
            true // No leader election configured, proceed with checkpoint creation
        };

        // STEP 3: Get unfinalized transactions (only leaders reach this point)
        // Apply propagation grace period: only include transactions that are old enough
        // This reduces merkle root mismatches due to transaction propagation delays
        let (unfinalized_hashes, unfinalized_txs, tx_merkle_root) = {
            let state = self.state.inner.read().await;
            
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let cutoff_time = now_ms.saturating_sub(PROPAGATION_GRACE_MS);

            // Collect both hashes and full transactions for vote requests
            // With custom CBOR codec (16MB limit) and ~500 bytes per tx, we can handle ~30,000 txs
            // Use 10,000 as a practical limit to balance throughput vs memory usage
            const MAX_TXS_PER_CHECKPOINT: usize = 10_000;
            
            let all_unfinalized = state.dag.get_unfinalized_nodes();
            let mut unfinalized_nodes: Vec<_> = all_unfinalized
                .iter()
                .filter(|n| {
                    // Only include transactions older than the grace period
                    // This gives time for the tx to propagate to all validators
                    n.tx.tx.timestamp <= cutoff_time
                })
                .filter(|n| Self::is_valid_hex_hash(&n.hash))
                .collect();
            
            // Limit to MAX_TXS_PER_CHECKPOINT to ensure vote request fits in P2P message
            if unfinalized_nodes.len() > MAX_TXS_PER_CHECKPOINT {
                info!(
                    "Limiting checkpoint {} to {} of {} eligible transactions (remaining will be finalized in next checkpoint)",
                    height, MAX_TXS_PER_CHECKPOINT, unfinalized_nodes.len()
                );
                // Sort by timestamp to finalize oldest first
                unfinalized_nodes.sort_by_key(|n| n.tx.tx.timestamp);
                unfinalized_nodes.truncate(MAX_TXS_PER_CHECKPOINT);
            }
            
            let mut unfinalized: Vec<String> = unfinalized_nodes.iter()
                .map(|n| n.hash.clone())
                .collect();
            
            // Collect full transaction data for sending to peers who may be missing txs
            let unfinalized_txs: Vec<SignedTransaction> = unfinalized_nodes.iter()
                .map(|n| n.tx.clone())
                .collect();
            
            // Log how many txs were excluded due to propagation grace
            let total_unfinalized = all_unfinalized.len();
            if total_unfinalized > unfinalized.len() {
                debug!(
                    "Propagation grace: {} of {} unfinalized txs excluded (too new, cutoff={}ms)",
                    total_unfinalized - unfinalized.len(),
                    total_unfinalized,
                    PROPAGATION_GRACE_MS
                );
            }

            if unfinalized.is_empty() {
                // LEADER WITH NO UNFINALIZED TXS: We're the leader but have no transactions to checkpoint
                // This can happen if we're behind. Try to adopt a peer's checkpoint if available.
                if is_leader {
                    info!(
                        "Leader for checkpoint {} but no unfinalized transactions - checking for peer checkpoint to adopt",
                        height
                    );
                    
                    // Drop the lock before async call
                    drop(state);
                    
                    if let Some(peer_checkpoint) = self.fetch_peer_checkpoint(height).await {
                        // Validate the peer checkpoint before adopting
                        // 1. Height must match expected
                        if peer_checkpoint.height != height {
                            warn!(
                                "Peer checkpoint height mismatch: expected {}, got {}",
                                height, peer_checkpoint.height
                            );
                            return Ok(());
                        }
                        
                        // 2. Previous hash must link to our chain
                        let peer_prev = peer_checkpoint.previous_hash.as_deref().unwrap_or("");
                        let our_prev = previous_hash.as_deref().unwrap_or("");
                        if peer_prev != our_prev {
                            warn!(
                                "Peer checkpoint prev_hash mismatch: expected {}, got {}",
                                &our_prev[..16.min(our_prev.len())],
                                &peer_prev[..16.min(peer_prev.len())]
                            );
                            return Ok(());
                        }
                        
                        info!(
                            "Found valid peer checkpoint at height {} - adopting as leader fallback",
                            height
                        );
                        
                        // Apply the checkpoint (apply_checkpoint does additional validation)
                        if let Err(e) = self.state.apply_checkpoint(peer_checkpoint, None).await {
                            warn!("Failed to apply peer checkpoint as leader fallback: {}", e);
                        } else {
                            // Record that we adopted a checkpoint (even though we didn't create it)
                            if let Some(ref leader_election) = self.leader_election {
                                leader_election.record_checkpoint_created();
                            }
                            self.reset_fork_failures();
                            return Ok(());
                        }
                    }
                }
                return Ok(());
            }

            // CRITICAL: Sort hashes for deterministic merkle root computation
            // This ensures proof generation uses the same order as checkpoint creation
            unfinalized.sort();

            let tx_merkle_root = if unfinalized.is_empty() {
                "0".repeat(64)
            } else {
                let tree = MerkleTree::from_hex_leaves(&unfinalized)?;
                tree.root()
            };

            (unfinalized, unfinalized_txs, tx_merkle_root)
        };

        // Record that we're creating the checkpoint (leader only)
        if let Some(ref leader_election) = self.leader_election {
            leader_election.record_checkpoint_created();
        }

        // FORK PREVENTION: Check if any peer already has a checkpoint at this height
        // If so, try to adopt their checkpoint instead of creating our own (with validation)
        if let Some(peer_checkpoint) = self.fetch_peer_checkpoint(height).await {
            // Verify the peer checkpoint has valid structure and matches our state
            if peer_checkpoint.height == height && !peer_checkpoint.hash.is_empty() {
                debug!(
                    "Peer has checkpoint at height {}, validating for adoption...",
                    height
                );
                
                // CRITICAL FIX: Use peer's finalized_tx_hashes if available for validation
                let (adoption_hashes, adoption_merkle_root): (Vec<String>, String) = if !peer_checkpoint.finalized_tx_hashes.is_empty() {
                    let state = self.state.inner.read().await;
                    let missing = peer_checkpoint.finalized_tx_hashes.iter()
                        .filter(|h| state.dag.get_node(h).is_none())
                        .count();
                    
                    if missing == 0 {
                        let mut sorted = peer_checkpoint.finalized_tx_hashes.clone();
                        sorted.sort();
                        let root = MerkleTree::from_hex_leaves(&sorted)
                            .map(|t| t.root())
                            .unwrap_or_else(|_| "0".repeat(64));
                        (peer_checkpoint.finalized_tx_hashes.clone(), root)
                    } else {
                        (unfinalized_hashes.clone(), tx_merkle_root.clone())
                    }
                } else {
                    (unfinalized_hashes.clone(), tx_merkle_root.clone())
                };
                
                // Validate and adopt - if validation fails, we'll try to sync first
                let (adopted, prev_hash_mismatch) = self.validate_and_adopt_peer_checkpoint(
                    peer_checkpoint.clone(),
                    &adoption_merkle_root,
                    previous_hash.as_deref(),
                    &adoption_hashes,
                ).await;
                
                if adopted {
                    self.reset_fork_failures();
                    return Ok(());
                }
                
                // Track previous_hash mismatches and potentially trigger chain recovery
                if prev_hash_mismatch {
                    if self.record_fork_failure() {
                        // Trigger full checkpoint chain recovery
                        info!("[ForkRecovery] Attempting to recover checkpoint chain from peer...");
                        match self.recover_checkpoint_chain().await {
                            Ok(true) => {
                                info!("[ForkRecovery] Chain recovery successful, skipping local checkpoint creation");
                                return Ok(());
                            }
                            Ok(false) => {
                                warn!("[ForkRecovery] Chain recovery returned false");
                            }
                            Err(e) => {
                                warn!("[ForkRecovery] Chain recovery failed: {}", e);
                            }
                        }
                    }
                }
                
                // Validation failed - likely due to merkle root mismatch
                // Try to sync missing transactions from peer before creating our own checkpoint
                info!(
                    "Peer checkpoint validation failed at height {}, attempting DAG sync before fallback",
                    height
                );
                
                // Request delta sync from the peer to get any missing transactions
                if let Err(e) = self.sync_missing_transactions(height).await {
                    debug!("DAG sync failed: {}, proceeding with local checkpoint", e);
                }
                
                // After sync, recompute our state and try adoption again
                // CRITICAL FIX: Use peer's finalized_tx_hashes if available
                let (new_unfinalized, new_merkle_root) = if !peer_checkpoint.finalized_tx_hashes.is_empty() {
                    let state = self.state.inner.read().await;
                    let missing = peer_checkpoint.finalized_tx_hashes.iter()
                        .filter(|h| state.dag.get_node(h).is_none())
                        .count();
                    
                    if missing == 0 {
                        let mut sorted = peer_checkpoint.finalized_tx_hashes.clone();
                        sorted.sort();
                        let root = MerkleTree::from_hex_leaves(&sorted)
                            .map(|t| t.root())
                            .unwrap_or_else(|_| "0".repeat(64));
                        (peer_checkpoint.finalized_tx_hashes.clone(), root)
                    } else {
                        // Still missing after sync - fall back to local
                        let mut unfinalized: Vec<String> = state
                            .dag
                            .get_unfinalized_nodes()
                            .iter()
                            .map(|n| n.hash.clone())
                            .filter(|h| Self::is_valid_hex_hash(h))
                            .collect();
                        unfinalized.sort();
                        let merkle_root = MerkleTree::from_hex_leaves(&unfinalized)
                            .map(|t| t.root())
                            .unwrap_or_else(|_| "0".repeat(64));
                        (unfinalized, merkle_root)
                    }
                } else {
                    let state = self.state.inner.read().await;
                    let mut unfinalized: Vec<String> = state
                        .dag
                        .get_unfinalized_nodes()
                        .iter()
                        .map(|n| n.hash.clone())
                        .filter(|h| Self::is_valid_hex_hash(h))
                        .collect();
                    
                    // CRITICAL: Sort for deterministic merkle root
                    unfinalized.sort();
                    
                    let merkle_root = if unfinalized.is_empty() {
                        "0".repeat(64)
                    } else {
                        match MerkleTree::from_hex_leaves(&unfinalized) {
                            Ok(tree) => tree.root(),
                            Err(_) => return Ok(()), // Retry next interval
                        }
                    };
                    (unfinalized, merkle_root)
                };
                
                // Retry adoption with updated state
                let (adopted_retry, prev_hash_mismatch_retry) = self.validate_and_adopt_peer_checkpoint(
                    peer_checkpoint,
                    &new_merkle_root,
                    previous_hash.as_deref(),
                    &new_unfinalized,
                ).await;
                
                if adopted_retry {
                    self.reset_fork_failures();
                    return Ok(());
                }
                
                // Track the retry mismatch too
                if prev_hash_mismatch_retry {
                    if self.record_fork_failure() {
                        info!("[ForkRecovery] Attempting chain recovery after retry...");
                        if let Ok(true) = self.recover_checkpoint_chain().await {
                            info!("[ForkRecovery] Chain recovery successful on retry");
                            return Ok(());
                        }
                    }
                }
                
                // Still failed after sync - proceed with local checkpoint
                debug!("Peer checkpoint validation failed after sync, creating our own");
            } else {
                warn!(
                    "Peer checkpoint at height {} has invalid structure, creating our own",
                    height
                );
            }
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        
        // CRITICAL: Process emissions and distribute checkpoint rewards BEFORE computing state_root
        // This ensures that if a claim transaction is pending, the simulation sees the same
        // pending_rewards value that the execution will claim. Otherwise, rewards distributed
        // after state_root calculation would cause claim proofs to be invalid.
        let checkpoint_reward = {
            let mut emission = self.state.emission.write().await;
            let reward = emission.get_checkpoint_reward(height);
            emission.record_emission(reward);
            reward
        };

        // Distribute checkpoint rewards to staked validators BEFORE state root computation
        let distributions = {
            let mut rewards = self.state.rewards.write().await;
            rewards.distribute_checkpoint_rewards(checkpoint_reward)
        };

        if !distributions.is_empty() {
            info!(
                "Pre-distributed {:.6} RKU to {} validators before state root computation",
                rinku_core::types::from_micro_units(checkpoint_reward),
                distributions.len()
            );
        }
        
        // Collect affected addresses BEFORE computing state_root
        // This is needed to generate proofs using the same simulated account set
        let mut affected_addresses_for_proofs: std::collections::HashSet<String> = std::collections::HashSet::new();
        for tx in &unfinalized_txs {
            affected_addresses_for_proofs.insert(tx.tx.from.clone());
            if !tx.tx.to.is_empty() {
                affected_addresses_for_proofs.insert(tx.tx.to.clone());
            }
        }
        let affected_addresses_vec: Vec<String> = affected_addresses_for_proofs.into_iter().collect();
        
        // Get fast-path executed set BEFORE state root computation
        // Transactions already executed on fast-path have their effects in current state,
        // so the simulation must skip them to avoid double-counting
        let fp_executed = if let Some(ref gossip) = self.gossip_service {
            gossip.get_all_fast_path_executed().await
        } else {
            std::collections::HashSet::new()
        };
        
        // Compute state_root with pending transactions applied (post-execution state)
        // This ensures the state_root matches what proofs will verify against
        // NOTE: Rewards were distributed above, so claim transactions will see the updated pending_rewards
        // CRITICAL: We don't generate proofs yet because we don't have the checkpoint hash
        // CRITICAL: Skip fast-path-executed txs in simulation (already in current state)
        let state_root = self.state.compute_state_root_with_pending_txs(&unfinalized_txs, &fp_executed).await;
        let receipt_root = "0".repeat(64);
        let tip_count = unfinalized_hashes.len() as u32;

        let checkpoint_hash = Self::compute_checkpoint_hash(
            height,
            &tx_merkle_root,
            &state_root,
            &receipt_root,
            tip_count,
            timestamp,
        );

        let signature = bls_sign(&checkpoint_hash, &self.bls_private_key)
            .map_err(|e| anyhow::anyhow!("BLS signing failed: {}", e))?;

        let my_stake = self.state.get_validator_stake(&self.validator_address).await.unwrap_or(0);
        let validator_sig = ValidatorSignature {
            validator: self.validator_address.clone(),
            signature: URL_SAFE_NO_PAD.encode(&signature),
            weight: my_stake,
            bls_public_key: Some(self.bls_public_key_base64()),
        };

        // Multi-validator quorum collection - attempt to gather votes from peers
        // Falls back to single-validator mode if no peer votes are collected
        // CRITICAL: Pass the exact tx list so validators can verify before signing
        let (all_signatures, raw_signatures, total_stake) = self.collect_validator_quorum(
            &checkpoint_hash,
            height,
            signature.clone(),
            validator_sig.clone(),
            &tx_merkle_root,
            &state_root,
            &unfinalized_hashes,
            &unfinalized_txs,
        ).await;
        
        // Check partition status for provisional checkpoint creation
        let partition_info = self.state.get_partition_state().await;
        let is_partitioned = partition_info.status == crate::state::partition::PartitionStatus::Partitioned;

        // Get total network stake for quorum check
        let total_network_stake = if let Some(ref identity) = self.validator_identity {
            let identity = identity.read().await;
            identity.total_active_stake().max(self.our_stake)
        } else {
            self.our_stake
        };
        let quorum_stake_needed = if is_partitioned {
            let visible_stake = (total_network_stake as f64 * partition_info.visible_stake_pct) as u64;
            (visible_stake as f64 * QUORUM_STAKE_THRESHOLD) as u64
        } else {
            (total_network_stake as f64 * QUORUM_STAKE_THRESHOLD) as u64
        };
        let mut quorum_reached = total_stake >= quorum_stake_needed && all_signatures.len() > 1;
        if !quorum_reached && self.trust_verifier.has_genesis_validators() {
            let genesis_addrs = self.trust_verifier.genesis_validator_addresses();
            let signed: HashSet<String> = all_signatures
                .iter()
                .map(|s| s.validator.clone())
                .collect();
            if genesis_addrs.iter().all(|addr| signed.contains(addr)) {
                info!(
                    "All genesis validators signed checkpoint {} - overriding quorum stake check",
                    height
                );
                quorum_reached = true;
            }
        }
        
        if !quorum_reached && self.mainnet_mode {
            warn!(
                "Checkpoint {} quorum not reached in MAINNET_MODE ({:.0}/{:.0} stake, {} votes)",
                height, total_stake, quorum_stake_needed, all_signatures.len()
            );
            return Ok(());
        }

        // Use collected signatures if quorum was reached, otherwise use single validator
        let use_single_validator = Self::should_use_single_validator(quorum_reached, self.mainnet_mode);
        let (final_signatures, final_raw_sigs) = if !use_single_validator {
            info!(
                "Checkpoint {} has {} validator signatures with {:.0}/{:.0} stake (quorum reached)",
                height, all_signatures.len(), total_stake, total_network_stake
            );
            (all_signatures, raw_signatures)
        } else {
            // Single-validator mode - use only our signature
            debug!(
                "Checkpoint {} using single-validator mode ({:.0}/{:.0} stake, {} votes)",
                height, total_stake, quorum_stake_needed, all_signatures.len()
            );
            (vec![validator_sig], vec![signature])
        };
        
        let signer_indices: Vec<usize> = (0..final_signatures.len()).collect();
        let aggregated_sig = aggregate_signatures(&final_raw_sigs)
            .map_err(|e| anyhow::anyhow!("BLS aggregation failed: {}", e))?;
        let signer_bitmap = create_signer_bitmap(&signer_indices, final_signatures.len());

        // Finalize pending weight votes and compute weight trie root for this checkpoint
        let weight_trie_root = {
            // Get ALL stakes: both validators AND regular accounts with stake (convert to micro-RKU as u64)
            let (all_stakes, total_network_stake): (std::collections::HashMap<String, u64>, u64) = {
                let state = self.state.inner.read().await;
                let mut stakes: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
                
                // 1. Add validator stakes
                for (addr, v) in state.validators.iter() {
                    if v.stake > 0 {
                        stakes.insert(addr.clone(), v.stake);
                    }
                }
                
                // 2. Add account stakes (regular users who staked but aren't validators)
                for (addr, account) in state.accounts.iter() {
                    if account.staked > 0 {
                        let stake_micro = account.staked;
                        // Use entry API to combine or add stake
                        stakes.entry(addr.clone())
                            .and_modify(|s| *s = (*s).max(stake_micro)) // Take max if both exist
                            .or_insert(stake_micro);
                    }
                }
                
                let total: u64 = stakes.values().sum();
                (stakes, total)
            };
            let validator_stakes = all_stakes; // Rename for compatibility
            
            let mut state = self.state.inner.write().await;
            if let Some(ref mut weight_trie) = state.weight_trie {
                // Finalize pending votes (aggregate by stake weight)
                let pending_count = weight_trie.pending_vote_count();
                if pending_count > 0 {
                    // Log stakes map for debugging - includes both validators and stakers
                    tracing::info!(
                        "Weight vote finalization: {} stakers (validators + accounts), total_stake={}",
                        validator_stakes.len(), total_network_stake
                    );
                    for (addr, stake) in &validator_stakes {
                        tracing::info!("  Stakes map: {} (len={}) = {} micro ({:.2} RKU)", 
                            addr, addr.len(), stake, *stake as f64 / 1_000_000.0);
                    }
                    
                    let updated = weight_trie.finalize_votes(&validator_stakes, total_network_stake);
                    tracing::info!(
                        "Checkpoint {}: finalized {} pending weight votes into {} tx aggregations",
                        height, pending_count, updated.len()
                    );
                    
                    // Log aggregation results for debugging
                    for (tx_hash, weight) in &updated {
                        tracing::info!(
                            "  Weight aggregation for {}: boost={}, suppress={}, neutral={}, count={}",
                            &tx_hash[..16.min(tx_hash.len())],
                            weight.boost_stake_micro,
                            weight.suppress_stake_micro, 
                            weight.neutral_stake_micro,
                            weight.attestation_count
                        );
                    }
                }
                
                // Compute root after finalizing votes
                weight_trie.compute_root()
            } else {
                String::new()
            }
        };
        
        let checkpoint = Checkpoint {
            height,
            hash: hex::encode(&checkpoint_hash),
            previous_hash,
            tx_merkle_root,
            state_root,
            receipt_root,
            tip_count,
            timestamp,
            validator_signatures: final_signatures,
            aggregated_signature: Some(URL_SAFE_NO_PAD.encode(&aggregated_sig)),
            signer_bitmap: Some(signer_bitmap),
            finalized_tx_hashes: unfinalized_hashes.clone(),
            weight_trie_root,
            provisional: is_partitioned,
            partition_epoch: if is_partitioned { partition_info.current_epoch } else { None },
            visible_stake_pct: if is_partitioned { Some(partition_info.visible_stake_pct) } else { None },
            merge_report_hash: None,
        };

        // Process emissions and rewards for this checkpoint
        // NOTE: Checkpoint rewards were already distributed earlier (before state_root computation)
        // to ensure claim transaction simulations match execution

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // PHASE 1: Quick read to collect timestamps and transactions (minimal lock time)
        // FINALITY-FIRST MODEL: Also collect transactions for execution after finalization
        // DOUBLE-EXECUTION GUARD: Only collect transactions that aren't already finalized
        let (tx_timestamps, txs_to_execute): (Vec<(String, u64)>, Vec<SignedTransaction>) = {
            let state = self.state.inner.read().await;
            let timestamps: Vec<(String, u64)> = unfinalized_hashes.iter()
                .filter_map(|hash| {
                    state.dag.get_node(hash).and_then(|node| {
                        if node.finalized {
                            None  // Skip already-finalized for stats
                        } else {
                            Some((hash.clone(), node.tx.tx.timestamp))
                        }
                    })
                })
                .collect();
            let txs: Vec<SignedTransaction> = unfinalized_hashes.iter()
                .filter_map(|hash| {
                    state.dag.get_node(hash).and_then(|node| {
                        if node.finalized {
                            None  // Skip already-finalized transactions
                        } else {
                            Some(node.tx.clone())
                        }
                    })
                })
                .collect();
            (timestamps, txs)
        };

        // PHASE 2: Compute finality times OUTSIDE any lock
        let finality_times: Vec<u64> = tx_timestamps.iter()
            .map(|(_, tx_timestamp)| {
                let tx_time_ms = if *tx_timestamp < 4_000_000_000 {
                    tx_timestamp * 1000
                } else {
                    *tx_timestamp
                };
                now_ms.saturating_sub(tx_time_ms)
            })
            .collect();

        // PHASE 3: Pre-compute finality stats outside the lock
        let finality_sum: u64 = finality_times.iter().sum();
        let finality_max: u64 = finality_times.iter().copied().max().unwrap_or(0);
        let finality_count = finality_times.len() as u64;

        // PHASE 4: Minimal write lock - batch mutations
        let mut state = self.state.inner.write().await;
        state.checkpoints.push(checkpoint.clone());
        state.last_checkpoint_time_ms = now_ms;

        // Update finality stats (pre-computed values)
        state.finality_sum_ms += finality_sum;
        state.finality_count += finality_count;
        if finality_max > state.finality_max_ms {
            state.finality_max_ms = finality_max;
        }
        // Add to rolling window (batch)
        for finality_time in &finality_times {
            if state.finality_times_ms.len() >= 1000 {
                state.finality_times_ms.pop_front();
            }
            state.finality_times_ms.push_back(*finality_time);
        }

        // Batch mark all as finalized (single operation instead of loop)
        let _finalized = state.dag.mark_finalized_batch(&unfinalized_hashes, height);
        state.total_transactions += unfinalized_hashes.len() as u64;

        if is_partitioned {
            for hash in &unfinalized_hashes {
                if let Some(node) = state.dag.get_node_mut(hash) {
                    node.provisional_finality = true;
                    node.partition_epoch = partition_info.current_epoch;
                }
            }
        }
        
        // Release write lock before executing transactions
        drop(state);

        info!(
            "Created checkpoint {} at height {} ({} txs finalized, {:.6} RKU emitted)",
            &checkpoint.hash[..16],
            height,
            unfinalized_hashes.len(),
            rinku_core::types::from_micro_units(checkpoint_reward)
        );
        if let Some(ref eb) = self.event_bus {
            eb.publish(crate::events::NodeEvent::CheckpointCreated {
                hash: checkpoint.hash.clone(),
                height,
                txs_finalized: unfinalized_hashes.len(),
                reward: rinku_core::types::from_micro_units(checkpoint_reward),
            });
        }
        
        // FINALITY-FIRST MODEL: Execute finalized transactions using TWO-PASS approach
        // This ensures simulation/execution parity for claim transactions:
        // - Pass 1: Execute PHASES 1 & 2 (balance changes, stakes, unstakes, claims)
        // - Pass 2: Execute PHASE 3 (witness/tip rewards)
        // 
        // By separating reward distribution from claim execution, we ensure claims
        // see exactly the pending_rewards that were snapshotted during simulation,
        // not additional witness rewards from earlier transactions in this checkpoint.
        
        // LEADER NODE PROOF GENERATION: Generate proofs BEFORE execution using simulated state
        // CRITICAL: Proofs must use the SAME transaction set that was used for state_root computation
        // We use `unfinalized_txs` (collected early, at line ~1062) NOT `txs_to_execute` (collected later)
        // This ensures nonces match exactly: simulation starts from the same base state that was used
        // for state_root computation, avoiding race conditions where concurrent checkpoints could
        // modify account.nonce between the two simulation runs.
        let proof_tx_hash = unfinalized_hashes.first()
            .cloned()
            .unwrap_or_else(|| checkpoint.hash.clone());
        
        // Get precomputed proofs using the same transaction set as state_root computation
        // CRITICAL FIX: Use unfinalized_txs (same set as state_root) NOT txs_to_execute
        // CRITICAL: Pass fp_executed to skip fast-path-executed txs (same as state_root computation)
        let precomputed_proofs = self.state.compute_state_root_and_proofs(
            &unfinalized_txs,
            &affected_addresses_vec,
            Some(&checkpoint),
            &proof_tx_hash,
            &fp_executed,
        ).await;

        // PASS 1: Execute balance/nonce changes, stakes, unstakes, and claims
        // Skip core execution for fast-path-executed txs (already applied)
        for tx in &txs_to_execute {
            if !fp_executed.contains(&tx.hash) {
                self.state.execute_finalized_transaction_core(tx).await;
            } else {
                tracing::debug!(
                    "Checkpoint: skipping core execution for fast-path-executed tx {}",
                    &tx.hash[..16.min(tx.hash.len())]
                );
            }
        }
        
        // PASS 2: Distribute witness/tip rewards (always runs - rewards deferred from fast-path)
        for tx in &txs_to_execute {
            self.state.execute_finalized_transaction_rewards(tx).await;
        }
        
        // Store precomputed proofs (generated from simulated state, not post-execution state)
        // These proofs are guaranteed to verify against checkpoint.state_root
        self.state.store_precomputed_proofs(&precomputed_proofs.proofs).await;
        
        // Track validator liveness - record which validators participated
        if let Some(ref consensus) = self.consensus_service {
            let participating_validators: Vec<String> = checkpoint.validator_signatures
                .iter()
                .map(|sig| sig.validator.clone())
                .collect();
            
            let mut consensus_guard = consensus.write().await;
            consensus_guard.track_liveness(height, &participating_validators).await;
            debug!(
                "Tracked liveness for checkpoint {}: {} validators participated",
                height, participating_validators.len()
            );
        }
        
        // IMMEDIATELY broadcast the checkpoint to all peers for fast propagation
        // This ensures other nodes receive the checkpoint without waiting for sync
        // Include the list of finalized transaction hashes so receivers can finalize
        // transactions even if their merkle roots don't match (due to propagation delays)
        // Also include precomputed proofs so followers can store them without regeneration
        // CRITICAL: Include actual transactions so followers can execute them even if they
        // missed the original gossip - this prevents balance divergence between nodes
        if let Some(ref gossip) = self.gossip_service {
            let proofs_vec: Vec<rinku_core::types::AccountStateProof> = 
                precomputed_proofs.proofs.values().cloned().collect();
            gossip.broadcast_checkpoint(
                checkpoint, 
                unfinalized_hashes.clone(), 
                txs_to_execute.clone(),
                proofs_vec
            ).await;
        }

        Ok(())
    }
    
    /// Collect validator votes from peers for multi-validator quorum.
    /// 
    /// Uses stake-weighted quorum threshold (2/3 of total stake) to determine
    /// when enough signatures have been collected. Falls back to single-validator
    /// mode if P2P network is unavailable or insufficient votes are collected.
    /// 
    /// CRITICAL: Requests votes from all peers in PARALLEL to avoid blocking on slow peers.
    /// A slow/unresponsive peer should not prevent quorum from being reached with healthy peers.
    /// 
    /// The finalized_tx_hashes are included in vote requests so validators can verify
    /// they have all the transactions before signing, preventing merkle root mismatches.
    async fn collect_validator_quorum(
        &self,
        checkpoint_hash: &[u8],
        height: u64,
        our_signature: Vec<u8>,
        our_validator_sig: ValidatorSignature,
        tx_merkle_root: &str,
        state_root: &str,
        finalized_tx_hashes: &[String],
        finalized_transactions: &[SignedTransaction],
    ) -> (Vec<ValidatorSignature>, Vec<Vec<u8>>, u64) {
        // Per-peer timeout for vote requests (truly parallel now that mutex is not held during await)
        // 5s is generous for same-region VMs; slow peers won't block others
        const QUORUM_TIMEOUT_MS: u64 = 5000;
        
        let mut signatures = vec![our_validator_sig.clone()];
        let mut raw_signatures = vec![our_signature];
        let mut total_stake_collected = our_validator_sig.weight;
        
        // Get total network stake from validator identity if available
        let total_network_stake = if let Some(ref identity) = self.validator_identity {
            let identity = identity.read().await;
            identity.total_active_stake().max(self.our_stake)
        } else {
            self.our_stake // Fallback: assume we're the only validator
        };
        
        let quorum_stake_needed = (total_network_stake as f64 * QUORUM_STAKE_THRESHOLD) as u64;
        debug!(
            "Quorum collection: need {}/{} stake ({:.1}%)",
            quorum_stake_needed, total_network_stake, QUORUM_STAKE_THRESHOLD * 100.0
        );
        
        // Check if we already have quorum with just our stake
        if total_stake_collected >= quorum_stake_needed {
            debug!("Single validator has sufficient stake for quorum");
            return (signatures, raw_signatures, total_stake_collected);
        }
        
        // Try P2P vote collection if network handle is available
        #[cfg(feature = "p2p")]
        if let Some(ref network) = self.network_handle {
            let checkpoint_hash_hex = hex::encode(checkpoint_hash);
            
            // Get connected peer IDs
            let peer_ids = {
                let locked = network.lock().await;
                locked.get_connected_peer_ids().await
            };
            debug!("Requesting checkpoint votes from {} connected peers IN PARALLEL", peer_ids.len());
            
            // PARALLEL VOTE COLLECTION: Request votes from ALL peers simultaneously
            // This prevents a slow peer from blocking the entire quorum collection
            let finalized_tx_hashes_vec: Vec<String> = finalized_tx_hashes.to_vec();
            let finalized_txs_vec: Vec<SignedTransaction> = finalized_transactions.to_vec();
            let vote_futures: Vec<_> = peer_ids.iter().map(|peer_id| {
                let network = Arc::clone(network);
                let checkpoint_hash_hex = checkpoint_hash_hex.clone();
                let tx_merkle_root = tx_merkle_root.to_string();
                let state_root = state_root.to_string();
                let checkpoint_hash_bytes = checkpoint_hash.to_vec();
                let peer_id = peer_id.clone();
                let validator_identity = self.validator_identity.clone();
                let tx_hashes = finalized_tx_hashes_vec.clone();
                let txs = finalized_txs_vec.clone();
                
                async move {
                    // Per-request timeout to prevent blocking on slow peers
                    let result = tokio::time::timeout(
                        std::time::Duration::from_millis(QUORUM_TIMEOUT_MS),
                        Self::request_checkpoint_vote_p2p_static(
                            &network,
                            &peer_id,
                            &checkpoint_hash_hex,
                            height,
                            &tx_merkle_root,
                            &state_root,
                            &checkpoint_hash_bytes,
                            validator_identity.as_ref(),
                            &tx_hashes,
                            &txs,
                        )
                    ).await;
                    
                    match result {
                        Ok(Some(vote)) => Some(vote),
                        Ok(None) => None,
                        Err(_) => {
                            debug!("Timeout requesting vote from peer {}", peer_id);
                            None
                        }
                    }
                }
            }).collect();
            
            // Wait for all vote requests to complete (with their individual timeouts)
            let results = join_all(vote_futures).await;
            
            // Process results and collect valid signatures
            for result in results {
                if let Some((peer_sig, peer_raw, peer_stake)) = result {
                    // Avoid duplicate signatures from same validator
                    if !signatures.iter().any(|s| s.validator == peer_sig.validator) {
                        info!(
                            "Received quorum vote from {} (stake: {:.0}) for checkpoint {}",
                            &peer_sig.validator[..16.min(peer_sig.validator.len())],
                            peer_stake,
                            height
                        );
                        signatures.push(peer_sig);
                        raw_signatures.push(peer_raw);
                        total_stake_collected += peer_stake;
                    }
                }
            }
            
            if total_stake_collected >= quorum_stake_needed {
                info!(
                    "Quorum reached with {:.0}/{:.0} stake from {} validators",
                    total_stake_collected, total_network_stake, signatures.len()
                );
            }
        }
        
        (signatures, raw_signatures, total_stake_collected)
    }
    
    /// Static version of request_checkpoint_vote_p2p for parallel execution
    #[cfg(feature = "p2p")]
    async fn request_checkpoint_vote_p2p_static(
        network: &Arc<tokio::sync::Mutex<NetworkHandle>>,
        peer_id: &str,
        checkpoint_hash_hex: &str,
        height: u64,
        tx_merkle_root: &str,
        state_root: &str,
        checkpoint_hash_bytes: &[u8],
        validator_identity: Option<&Arc<RwLock<ValidatorIdentityService>>>,
        finalized_tx_hashes: &[String],
        finalized_transactions: &[SignedTransaction],
    ) -> Option<(ValidatorSignature, Vec<u8>, u64)> {
        // With custom 16MB CBOR codec, we can send many more embedded transactions
        // Average transaction is ~500 bytes, so 10,000 txs ≈ 5MB, well within 16MB limit
        const MAX_EMBEDDED_TXS: usize = 10_000;
        let limited_transactions: Vec<SignedTransaction> = if finalized_transactions.len() > MAX_EMBEDDED_TXS {
            info!(
                "Limiting embedded transactions in vote request from {} to {} (checkpoint {})",
                finalized_transactions.len(), MAX_EMBEDDED_TXS, height
            );
            finalized_transactions.iter().take(MAX_EMBEDDED_TXS).cloned().collect()
        } else {
            finalized_transactions.to_vec()
        };
        
        let request = SyncRequest::CheckpointVote(CheckpointVoteRequest {
            checkpoint_hash: checkpoint_hash_hex.to_string(),
            height,
            tx_merkle_root: tx_merkle_root.to_string(),
            state_root: state_root.to_string(),
            finalized_tx_hashes: finalized_tx_hashes.to_vec(),
            finalized_transactions: limited_transactions,
        });
        
        let response_rx = {
            let locked = network.lock().await;
            locked.send_sync_request(peer_id, request).await
        };
        let response: Result<SyncResponse, anyhow::Error> = match response_rx {
            Ok(rx) => match rx.await {
                Ok(r) => Ok(r),
                Err(_) => {
                    debug!("Vote request cancelled for peer {}", peer_id);
                    return None;
                }
            },
            Err(e) => {
                debug!("Failed to send vote request to {}: {}", peer_id, e);
                return None;
            }
        };
        match response {
            Ok(SyncResponse::CheckpointVote(Some(vote))) => {
                // SECURITY: Validate the validator against our local registry
                let (verified_pk, verified_stake) = if let Some(identity) = validator_identity {
                    let identity = identity.read().await;
                    
                    // Check if this validator is known and active in our registry
                    if !identity.is_active_validator(&vote.validator_address) {
                        warn!(
                            "Rejecting vote from unknown/inactive validator {} (peer {})",
                            &vote.validator_address[..16.min(vote.validator_address.len())],
                            peer_id
                        );
                        return None;
                    }
                    
                    // Get the known BLS public key for this validator from our registry
                    match identity.get_validator_bls_key(&vote.validator_address) {
                        Some(known_pk) => {
                            // Verify the peer's claimed public key matches our known key
                            let claimed_pk = URL_SAFE_NO_PAD.decode(&vote.bls_public_key).ok()?;
                            if claimed_pk != known_pk {
                                warn!(
                                    "Rejecting vote: BLS key mismatch for validator {} (peer {})",
                                    &vote.validator_address[..16.min(vote.validator_address.len())],
                                    peer_id
                                );
                                return None;
                            }
                            let stake = identity.get_validator_stake(&vote.validator_address).unwrap_or(0);
                            (known_pk, stake)
                        }
                        None => {
                            warn!(
                                "Rejecting vote: no BLS key in registry for validator {} (peer {})",
                                &vote.validator_address[..16.min(vote.validator_address.len())],
                                peer_id
                            );
                            return None;
                        }
                    }
                } else {
                    // No validator identity service, accept the peer's claimed key/stake (testnet mode)
                    let pk = URL_SAFE_NO_PAD.decode(&vote.bls_public_key).ok()?;
                    (pk, vote.stake)
                };
                
                // Verify the BLS signature using the verified public key
                let raw_signature = URL_SAFE_NO_PAD.decode(&vote.signature).ok()?;
                if !bls_verify(checkpoint_hash_bytes, &raw_signature, &verified_pk) {
                    warn!(
                        "Rejecting vote: invalid BLS signature from validator {} (peer {})",
                        &vote.validator_address[..16.min(vote.validator_address.len())],
                        peer_id
                    );
                    return None;
                }
                
                let validator_sig = ValidatorSignature {
                    validator: vote.validator_address,
                    signature: vote.signature,
                    weight: verified_stake,
                    bls_public_key: Some(URL_SAFE_NO_PAD.encode(&verified_pk)),
                };
                
                Some((validator_sig, raw_signature, verified_stake))
            }
            Ok(SyncResponse::CheckpointVote(None)) => {
                debug!("Peer {} declined to vote on checkpoint {}", peer_id, height);
                None
            }
            Ok(SyncResponse::Error { message }) => {
                warn!("Peer {} returned error for checkpoint vote: {}", peer_id, message);
                None
            }
            _ => {
                debug!("Unexpected or failed response from peer {} for checkpoint vote", peer_id);
                None
            }
        }
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
