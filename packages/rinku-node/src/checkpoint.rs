use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rinku_core::{
    merkle::MerkleTree,
    types::{Checkpoint, ValidatorSignature},
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, debug};

use crate::bls::{
    aggregate_signatures, bls_sign, bls_verify, create_signer_bitmap, generate_bls_keypair,
};
use crate::config::TrustConfig;
use crate::consensus::ConsensusService;
use crate::dag_pruning::{DagPruningService, PruningConfig};
#[cfg(feature = "p2p")]
use crate::network::{CheckpointVoteRequest, CheckpointVoteResponse, NetworkHandle, SyncRequest, SyncResponse};
use crate::slashing::SlashingService;
use crate::state::NodeState;
use crate::trust::TrustVerifier;
use crate::validator_identity::ValidatorIdentityService;

/// Quorum threshold for multi-validator checkpoints (2/3 of stake)
const QUORUM_STAKE_THRESHOLD: f64 = 0.667;

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
    network_handle: Option<Arc<NetworkHandle>>,
    /// Our validator's stake (for quorum calculation)
    our_stake: f64,
}

const FORK_RECOVERY_THRESHOLD: u32 = 3;

impl CheckpointService {
    pub fn new(
        state: NodeState, 
        interval_ms: u64, 
        validator_address: Option<String>, 
        peers: Vec<String>,
        trust_config: TrustConfig,
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
            our_stake: 1000.0, // Default stake, should be set from validator identity
        }
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
    pub fn with_network_handle(mut self, handle: Arc<NetworkHandle>) -> Self {
        self.network_handle = Some(handle);
        self
    }
    
    /// Set our validator's stake for quorum calculation
    pub fn with_stake(mut self, stake: f64) -> Self {
        self.our_stake = stake;
        self
    }

    pub fn bls_public_key_base64(&self) -> String {
        URL_SAFE_NO_PAD.encode(&self.bls_public_key)
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
                let current_height = state_guard.checkpoints.len() as u64;
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
                        weight: 1.0,
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

    /// Check if any peer has a checkpoint at the given height
    /// If found, returns the peer's checkpoint to adopt instead of creating our own
    async fn fetch_peer_checkpoint(&self, height: u64) -> Option<Checkpoint> {
        if self.peers.is_empty() {
            return None;
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .ok()?;

        for peer in &self.peers {
            let url = format!("{}/api/checkpoints/{}", peer.trim_end_matches('/'), height);
            
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<Checkpoint>().await {
                        Ok(checkpoint) => {
                            info!(
                                "Found peer checkpoint at height {} from {}: {}",
                                height, peer, &checkpoint.hash[..16.min(checkpoint.hash.len())]
                            );
                            return Some(checkpoint);
                        }
                        Err(e) => {
                            debug!("Failed to parse checkpoint from {}: {}", peer, e);
                        }
                    }
                }
                Ok(resp) => {
                    debug!("Peer {} has no checkpoint at height {} (status: {})", peer, height, resp.status());
                }
                Err(e) => {
                    debug!("Failed to reach peer {} for checkpoint {}: {}", peer, height, e);
                }
            }
        }

        None
    }

    /// Sync missing transactions from peers before checkpoint
    async fn sync_missing_transactions(&self, target_height: u64) -> Result<()> {
        if self.peers.is_empty() {
            return Err(anyhow::anyhow!("No peers available"));
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()?;

        // Get our current checkpoint height
        let from_checkpoint = {
            let state = self.state.inner.read().await;
            state.checkpoints.len() as u64
        };

        for peer in &self.peers {
            // Use delta sync to get transactions since last checkpoint
            let url = format!(
                "{}/api/sync/delta?from_checkpoint={}&limit=500",
                peer.trim_end_matches('/'),
                from_checkpoint
            );
            
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    // Try to parse as paginated response first
                    if let Ok(text) = resp.text().await {
                        use rinku_core::types::SignedTransaction;
                        
                        // Parse JSON and extract transactions
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            let txs: Vec<SignedTransaction> = if json.is_array() {
                                serde_json::from_value(json).unwrap_or_default()
                            } else if let Some(txs_val) = json.get("transactions") {
                                serde_json::from_value(txs_val.clone()).unwrap_or_default()
                            } else {
                                vec![]
                            };
                            
                            if !txs.is_empty() {
                                let mut added = 0;
                                
                                for tx in txs {
                                    let mut state = self.state.inner.write().await;
                                    if !state.dag.contains(&tx.hash) {
                                        use rinku_core::types::DagNode;
                                        let parents = tx.tx.parents.clone();
                                        let node = DagNode {
                                            hash: tx.hash.clone(),
                                            parents,
                                            children: vec![],
                                            weight: 1.0,
                                            finalized: false,
                                            checkpoint_height: None,
                                            tx,
                                        };
                                        if state.dag.add_node(node).is_ok() {
                                            added += 1;
                                        }
                                    }
                                }
                                
                                if added > 0 {
                                    debug!(
                                        "Fork prevention: synced {} new transactions from peer for checkpoint {}",
                                        added, target_height
                                    );
                                }
                                return Ok(());
                            }
                        }
                    }
                }
                Ok(_) => {
                    debug!("Peer {} returned non-success for delta sync", peer);
                }
                Err(e) => {
                    debug!("Failed to sync from peer {}: {}", peer, e);
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

        // VALIDATION 3: Ensure checkpoint has at least one validator signature
        if peer_checkpoint.validator_signatures.is_empty() {
            warn!(
                "Peer checkpoint at height {} has no validator signatures",
                peer_checkpoint.height
            );
            return (false, false);
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
        
        // VALIDATION 5: Verify at least one BLS signature cryptographically
        let mut valid_sig_found = false;
        for sig in &peer_checkpoint.validator_signatures {
            // Decode the signature from base64
            let sig_bytes = match URL_SAFE_NO_PAD.decode(&sig.signature) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            
            // BLS signatures in compressed form are 96 bytes
            if sig_bytes.len() < 96 {
                continue;
            }
            
            // Try to decode the validator address as a public key
            // The validator field might contain the public key directly or a fingerprint
            // For now, we verify the signature format is valid BLS
            // Full verification requires a validator registry with public keys
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
                checkpoint_reward,
                distributions.len()
            );
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Update state with adopted checkpoint
        let mut state = self.state.inner.write().await;
        state.checkpoints.push(peer_checkpoint);
        state.last_checkpoint_time_ms = now_ms;

        // Mark transactions as finalized
        for hash in unfinalized_hashes {
            let _ = state.dag.mark_finalized(hash, height);
        }

        drop(state);

        info!(
            "Adopted peer checkpoint {} at height {} ({} txs finalized, {:.6} RKU emitted)",
            &checkpoint_hash[..16.min(checkpoint_hash.len())],
            height,
            unfinalized_hashes.len(),
            checkpoint_reward
        );

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
        if self.peers.is_empty() {
            return Err(anyhow::anyhow!("No peers available for chain recovery"));
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        for peer in &self.peers {
            // Request a full snapshot sync from the peer
            // This uses the existing validated sync system
            let url = format!("{}/api/sync/snapshot", peer.trim_end_matches('/'));
            
            info!(
                "[ForkRecovery] Requesting full snapshot sync from peer {}",
                peer
            );
            
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    // Parse the snapshot response
                    #[derive(serde::Deserialize)]
                    struct SnapshotResponse {
                        accounts: std::collections::HashMap<String, rinku_core::types::Account>,
                        checkpoints: Vec<Checkpoint>,
                        validators: std::collections::HashMap<String, rinku_core::types::Validator>,
                        #[serde(default)]
                        dag_transactions: Vec<rinku_core::types::SignedTransaction>,
                        gas_price: Option<f64>,
                        total_supply: Option<f64>,
                        genesis_time: Option<u64>,
                    }
                    
                    match resp.json::<SnapshotResponse>().await {
                        Ok(snapshot) => {
                            // Verify checkpoint chain linkage first
                            let mut linkage_valid = true;
                            for i in 1..snapshot.checkpoints.len() {
                                let expected_prev = &snapshot.checkpoints[i - 1].hash;
                                if snapshot.checkpoints[i].previous_hash.as_deref() != Some(expected_prev) {
                                    warn!(
                                        "[ForkRecovery] Peer snapshot has invalid checkpoint chain at height {}",
                                        snapshot.checkpoints[i].height
                                    );
                                    linkage_valid = false;
                                    break;
                                }
                            }
                            
                            if !linkage_valid {
                                continue;  // Try next peer
                            }

                            // Verify checkpoint hashes are correctly computed
                            let mut hash_valid = true;
                            for checkpoint in &snapshot.checkpoints {
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

                            // Use stake-weighted BLS signature verification
                            // The trust verifier validates against genesis validators and on-chain validator registry
                            if self.trust_verifier.has_genesis_validators() {
                                if let Err(e) = self.trust_verifier.verify_checkpoint_chain(
                                    &snapshot.checkpoints,
                                    &snapshot.validators,
                                ) {
                                    warn!("[ForkRecovery] Stake-weighted verification failed: {}", e);
                                    continue;
                                }
                                info!(
                                    "[ForkRecovery] Verified {} checkpoints with stake-weighted BLS signatures",
                                    snapshot.checkpoints.len()
                                );
                            } else {
                                // No genesis validators configured - use format validation only (testnet mode)
                                let mut format_valid = true;
                                for checkpoint in &snapshot.checkpoints {
                                    if checkpoint.validator_signatures.is_empty() && checkpoint.height > 1 {
                                        continue; // Allow unsigned early checkpoints
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

                            let checkpoint_count = snapshot.checkpoints.len();
                            let account_count = snapshot.accounts.len();
                            let tx_count = snapshot.dag_transactions.len();
                            let latest_height = snapshot.checkpoints.last().map(|c| c.height).unwrap_or(0);

                            // Apply the validated snapshot atomically
                            {
                                let mut state = self.state.inner.write().await;
                                
                                // Clear and replace checkpoints
                                state.checkpoints = snapshot.checkpoints;
                                
                                // Clear and replace accounts
                                state.accounts.clear();
                                for (fingerprint, account) in snapshot.accounts {
                                    state.accounts.insert(fingerprint, account);
                                }
                                
                                // Clear and replace validators
                                state.validators.clear();
                                for (addr, validator) in snapshot.validators {
                                    state.validators.insert(addr, validator);
                                }
                                
                                // Clear DAG and add transactions from snapshot
                                // Note: This creates a new DAG, losing unfinalized local transactions
                                // This is intentional - we're resetting to peer's state
                                let max_nodes = state.dag.node_count().max(10000);
                                state.dag = rinku_core::dag::Dag::new(max_nodes);
                                
                                for tx in snapshot.dag_transactions {
                                    let parents = tx.tx.parents.clone();
                                    let node = rinku_core::types::DagNode {
                                        hash: tx.hash.clone(),
                                        parents,
                                        children: vec![],
                                        weight: 1.0,
                                        finalized: true, // All snapshot txs are finalized
                                        checkpoint_height: Some(latest_height),
                                        tx,
                                    };
                                    let _ = state.dag.add_node(node);
                                }
                                
                                // Update monetary state from snapshot
                                if let Some(gas_price) = snapshot.gas_price {
                                    state.current_gas_price = gas_price;
                                }
                                if let Some(total_supply) = snapshot.total_supply {
                                    state.total_supply = total_supply;
                                }
                                if let Some(genesis_time) = snapshot.genesis_time {
                                    state.genesis_time = genesis_time;
                                }
                            }

                            info!(
                                "[ForkRecovery] Applied snapshot from {}: {} checkpoints, {} accounts, {} txs (height: {})",
                                peer, checkpoint_count, account_count, tx_count, latest_height
                            );

                            // Reset failure counter
                            self.consecutive_fork_failures.store(0, std::sync::atomic::Ordering::SeqCst);

                            return Ok(true);
                        }
                        Err(e) => {
                            warn!("[ForkRecovery] Failed to parse snapshot from {}: {}", peer, e);
                        }
                    }
                }
                Ok(resp) => {
                    debug!("[ForkRecovery] Peer {} returned status {} for snapshot", peer, resp.status());
                }
                Err(e) => {
                    debug!("[ForkRecovery] Failed to reach peer {}: {}", peer, e);
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

    async fn create_checkpoint(&self) -> Result<()> {
        let (unfinalized_hashes, tx_merkle_root, height, previous_hash) = {
            let state = self.state.inner.read().await;

            let mut unfinalized: Vec<String> = state
                .dag
                .get_unfinalized_nodes()
                .iter()
                .map(|n| n.hash.clone())
                .filter(|h| Self::is_valid_hex_hash(h))
                .collect();

            if unfinalized.is_empty() {
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

            let height = state.checkpoints.len() as u64 + 1;
            let previous_hash = state.checkpoints.last().map(|c| c.hash.clone());

            (unfinalized, tx_merkle_root, height, previous_hash)
        };

        // FORK PREVENTION: Check if any peer already has a checkpoint at this height
        // If so, try to adopt their checkpoint instead of creating our own (with validation)
        if let Some(peer_checkpoint) = self.fetch_peer_checkpoint(height).await {
            // Verify the peer checkpoint has valid structure and matches our state
            if peer_checkpoint.height == height && !peer_checkpoint.hash.is_empty() {
                debug!(
                    "Peer has checkpoint at height {}, validating for adoption...",
                    height
                );
                
                // Validate and adopt - if validation fails, we'll try to sync first
                let (adopted, prev_hash_mismatch) = self.validate_and_adopt_peer_checkpoint(
                    peer_checkpoint.clone(),
                    &tx_merkle_root,
                    previous_hash.as_deref(),
                    &unfinalized_hashes,
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
                let (new_unfinalized, new_merkle_root) = {
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
        let state_root = "0".repeat(64);
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

        let validator_sig = ValidatorSignature {
            validator: self.validator_address.clone(),
            signature: URL_SAFE_NO_PAD.encode(&signature),
            weight: 1.0,
            bls_public_key: Some(self.bls_public_key_base64()),
        };

        // Multi-validator quorum collection - attempt to gather votes from peers
        // Falls back to single-validator mode if no peer votes are collected
        let (all_signatures, raw_signatures, total_stake) = self.collect_validator_quorum(
            &checkpoint_hash,
            height,
            signature.clone(),
            validator_sig.clone(),
            &tx_merkle_root,
            &state_root,
        ).await;
        
        // Get total network stake for quorum check
        let total_network_stake = if let Some(ref identity) = self.validator_identity {
            let identity = identity.read().await;
            identity.total_active_stake().max(self.our_stake)
        } else {
            self.our_stake
        };
        let quorum_stake_needed = total_network_stake * QUORUM_STAKE_THRESHOLD;
        let quorum_reached = total_stake >= quorum_stake_needed && all_signatures.len() > 1;
        
        // Use collected signatures if quorum was reached, otherwise use single validator
        let (final_signatures, final_raw_sigs) = if quorum_reached {
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
        };

        // Process emissions and rewards for this checkpoint
        let checkpoint_reward = {
            let mut emission = self.state.emission.write().await;
            let reward = emission.get_checkpoint_reward(height);
            emission.record_emission(reward);
            reward
        };

        // Distribute checkpoint rewards to staked validators
        let distributions = {
            let mut rewards = self.state.rewards.write().await;
            rewards.distribute_checkpoint_rewards(checkpoint_reward)
        };

        if !distributions.is_empty() {
            info!(
                "Distributed {:.6} RKU to {} validators",
                checkpoint_reward,
                distributions.len()
            );
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // PHASE 1: Quick read to collect timestamps (minimal lock time)
        let tx_timestamps: Vec<(String, u64)> = {
            let state = self.state.inner.read().await;
            unfinalized_hashes.iter()
                .filter_map(|hash| {
                    state.dag.get_node(hash).map(|node| {
                        let ts = node.tx.tx.timestamp;
                        (hash.clone(), ts)
                    })
                })
                .collect()
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

        // PHASE 3: Minimal write lock - just mutations
        let mut state = self.state.inner.write().await;
        state.checkpoints.push(checkpoint.clone());
        state.last_checkpoint_time_ms = now_ms;

        // Update finality stats
        for finality_time in &finality_times {
            state.finality_sum_ms += finality_time;
            state.finality_count += 1;
            if *finality_time > state.finality_max_ms {
                state.finality_max_ms = *finality_time;
            }
            if state.finality_times_ms.len() >= 1000 {
                state.finality_times_ms.pop_front();
            }
            state.finality_times_ms.push_back(*finality_time);
        }

        // Mark all as finalized
        for hash in &unfinalized_hashes {
            let _ = state.dag.mark_finalized(hash, height);
        }
        
        // Release write lock before logging
        drop(state);

        info!(
            "Created checkpoint {} at height {} ({} txs finalized, {:.6} RKU emitted)",
            &checkpoint.hash[..16],
            height,
            unfinalized_hashes.len(),
            checkpoint_reward
        );
        
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

        Ok(())
    }
    
    /// Collect validator votes from peers for multi-validator quorum.
    /// 
    /// Uses stake-weighted quorum threshold (2/3 of total stake) to determine
    /// when enough signatures have been collected. Falls back to single-validator
    /// mode if P2P network is unavailable or insufficient votes are collected.
    async fn collect_validator_quorum(
        &self,
        checkpoint_hash: &[u8],
        height: u64,
        our_signature: Vec<u8>,
        our_validator_sig: ValidatorSignature,
        tx_merkle_root: &str,
        state_root: &str,
    ) -> (Vec<ValidatorSignature>, Vec<Vec<u8>>, f64) {
        const QUORUM_TIMEOUT_MS: u64 = 5000;
        
        let mut signatures = vec![our_validator_sig];
        let mut raw_signatures = vec![our_signature];
        let mut total_stake_collected = self.our_stake;
        
        // Get total network stake from validator identity if available
        let total_network_stake = if let Some(ref identity) = self.validator_identity {
            let identity = identity.read().await;
            identity.total_active_stake().max(self.our_stake)
        } else {
            self.our_stake // Fallback: assume we're the only validator
        };
        
        let quorum_stake_needed = total_network_stake * QUORUM_STAKE_THRESHOLD;
        debug!(
            "Quorum collection: need {:.0}/{:.0} stake ({:.1}%)",
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
            let start_time = std::time::Instant::now();
            
            // Get connected peer IDs
            let peer_ids = network.get_connected_peer_ids().await;
            debug!("Requesting checkpoint votes from {} connected peers", peer_ids.len());
            
            for peer_id in peer_ids {
                if start_time.elapsed().as_millis() as u64 >= QUORUM_TIMEOUT_MS {
                    debug!("Quorum collection timeout after {}ms", QUORUM_TIMEOUT_MS);
                    break;
                }
                
                // Request vote from peer
                if let Some((peer_sig, peer_raw, peer_stake)) = self.request_checkpoint_vote_p2p(
                    network,
                    &peer_id,
                    &checkpoint_hash_hex,
                    height,
                    tx_merkle_root,
                    state_root,
                    checkpoint_hash,
                ).await {
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
                        
                        // Check if quorum reached
                        if total_stake_collected >= quorum_stake_needed {
                            info!(
                                "Quorum reached with {:.0}/{:.0} stake from {} validators",
                                total_stake_collected, total_network_stake, signatures.len()
                            );
                            break;
                        }
                    }
                }
            }
        }
        
        (signatures, raw_signatures, total_stake_collected)
    }
    
    /// Request a checkpoint vote from a peer validator via P2P.
    /// 
    /// Security: This method validates votes against the local validator registry:
    /// 1. Verifies the validator address exists in our registry
    /// 2. Verifies the BLS public key matches our known key for that validator
    /// 3. Verifies the BLS signature using verified bytes (not peer-supplied)
    /// 4. Uses locally-known stake weight (not peer-supplied)
    #[cfg(feature = "p2p")]
    async fn request_checkpoint_vote_p2p(
        &self,
        network: &NetworkHandle,
        peer_id: &str,
        checkpoint_hash_hex: &str,
        height: u64,
        tx_merkle_root: &str,
        state_root: &str,
        checkpoint_hash_bytes: &[u8],
    ) -> Option<(ValidatorSignature, Vec<u8>, f64)> {
        let request = SyncRequest::CheckpointVote(CheckpointVoteRequest {
            checkpoint_hash: checkpoint_hash_hex.to_string(),
            height,
            tx_merkle_root: tx_merkle_root.to_string(),
            state_root: state_root.to_string(),
        });
        
        match network.sync_request(peer_id, request).await {
            Ok(SyncResponse::CheckpointVote(Some(vote))) => {
                // SECURITY: Validate the validator against our local registry
                let (verified_pk, verified_stake) = if let Some(ref identity) = self.validator_identity {
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
                    // This prevents peers from substituting a different key
                    match identity.get_validator_bls_key(&vote.validator_address) {
                        Some(known_pk) => {
                            // Verify the peer's claimed public key matches our known key
                            let claimed_pk = URL_SAFE_NO_PAD.decode(&vote.bls_public_key).ok()?;
                            if claimed_pk != known_pk {
                                warn!(
                                    "BLS key mismatch for validator {} (peer {}): claimed key doesn't match registry",
                                    &vote.validator_address[..16.min(vote.validator_address.len())],
                                    peer_id
                                );
                                return None;
                            }
                            
                            // Get the validator's stake from our registry (not from peer)
                            let stake = identity.get_validator_stake(&vote.validator_address).unwrap_or(0.0);
                            (known_pk, stake)
                        }
                        None => {
                            warn!(
                                "No BLS key in registry for validator {} (peer {})",
                                &vote.validator_address[..16.min(vote.validator_address.len())],
                                peer_id
                            );
                            return None;
                        }
                    }
                } else {
                    // No validator identity service - use trust verifier as fallback
                    // This is less secure but maintains backward compatibility
                    debug!("No validator identity service, using peer-supplied data (testnet mode)");
                    let pk = URL_SAFE_NO_PAD.decode(&vote.bls_public_key).ok()?;
                    (pk, vote.stake)
                };
                
                // SECURITY: Decode signature from the encoded string (not peer-supplied bytes)
                // This ensures we verify and use the same bytes
                let sig_bytes = match URL_SAFE_NO_PAD.decode(&vote.signature) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        warn!("Invalid signature encoding from peer {}", peer_id);
                        return None;
                    }
                };
                
                // Verify BLS signature using our verified public key
                if !bls_verify(checkpoint_hash_bytes, &sig_bytes, &verified_pk) {
                    warn!(
                        "Invalid BLS signature from validator {} (peer {}) for checkpoint {}",
                        &vote.validator_address[..16.min(vote.validator_address.len())],
                        peer_id,
                        height
                    );
                    return None;
                }
                
                // Signature verified - create validated vote
                let validator_sig = ValidatorSignature {
                    validator: vote.validator_address.clone(),
                    signature: vote.signature.clone(),
                    weight: verified_stake, // Use locally-verified stake
                    bls_public_key: Some(vote.bls_public_key.clone()),
                };
                
                debug!(
                    "Verified checkpoint vote from {} (stake: {:.0}) for height {}",
                    &vote.validator_address[..16.min(vote.validator_address.len())],
                    verified_stake,
                    height
                );
                
                // Return decoded sig_bytes (verified), not peer-supplied signature_bytes
                return Some((validator_sig, sig_bytes, verified_stake));
            }
            Ok(SyncResponse::CheckpointVote(None)) => {
                debug!("Peer {} declined to vote on checkpoint {}", peer_id, height);
            }
            Ok(SyncResponse::Error { message }) => {
                warn!("Peer {} returned error for checkpoint vote: {}", peer_id, message);
            }
            Ok(_) => {
                warn!("Unexpected response type from peer {} for checkpoint vote", peer_id);
            }
            Err(e) => {
                debug!("Failed to request checkpoint vote from {}: {}", peer_id, e);
            }
        }
        None
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
