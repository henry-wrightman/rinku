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
use tracing::{debug, info, warn};

use crate::bls::{
    aggregate_signatures, bls_sign, bls_verify, create_signer_bitmap, generate_bls_keypair,
};
use crate::config::TrustConfig;
use crate::consensus::ConsensusService;
use crate::dag_pruning::{DagPruningService, PruningConfig};
use crate::leader_election::{LeaderElectionConfig, LeaderElectionService};
#[cfg(feature = "p2p")]
use crate::network::{
    CheckpointData, CheckpointPushData, CheckpointVoteRequest, NetworkHandle, SyncResponse,
    VoteRequest, VoteResponse,
};
use crate::slashing::SlashingService;
use crate::state::NodeState;
use crate::trust::TrustVerifier;
use crate::validator_identity::ValidatorIdentityService;

const DYNAMIC_TX_CAP_MIN: usize = 10;
const DYNAMIC_TX_CAP_MAX: usize = 600;

struct AcceptedVote {
    canonical_stake: u64,
    sig_bytes: Vec<u8>,
}
const DYNAMIC_TX_CAP_DEFAULT: usize = 50;

struct PendingQcc {
    checkpoint: Checkpoint,
    hashes: Vec<String>,
    txs_to_execute: Vec<SignedTransaction>,
    reward_distributions: Vec<(String, u64)>,
    checkpoint_reward: u64,
    finalized_proofs: std::collections::HashMap<String, rinku_core::types::AccountStateProof>,
    fast_path_executed: std::collections::HashSet<String>,
    height: u64,
    now_ms: u64,
    is_partitioned: bool,
    partition_info: crate::state::partition::PartitionState,
    finality_sum: u64,
    finality_count: u64,
    finality_max: u64,
    finality_times: Vec<u64>,
    t_overall: std::time::Instant,
    t_gather_ms: u128,
    t_proof_ms: u128,
    t_weight_ms: u128,
    qcc_handle: tokio::task::JoinHandle<Option<(Vec<ValidatorSignature>, Vec<u8>, Vec<u8>)>>,
    qcc_spawned_at: std::time::Instant,
}

struct QccRetryData {
    checkpoint: Checkpoint,
    hashes: Vec<String>,
    txs_to_execute: Vec<SignedTransaction>,
    reward_distributions: Vec<(String, u64)>,
    checkpoint_reward: u64,
    finalized_proofs: std::collections::HashMap<String, rinku_core::types::AccountStateProof>,
    fast_path_executed: std::collections::HashSet<String>,
    height: u64,
    is_partitioned: bool,
    partition_info: crate::state::partition::PartitionState,
}

#[cfg(feature = "p2p")]
struct DeltaSyncFetchResult {
    delta: crate::network::DeltaData,
    peer_id: String,
    fetch_ms: u128,
    peers_tried: u32,
    peers_timeout: u32,
    peers_error: u32,
}

#[cfg(feature = "p2p")]
struct PendingDeltaSync {
    handle: tokio::task::JoinHandle<Option<DeltaSyncFetchResult>>,
    spawned_at: std::time::Instant,
    from_height: u64,
}

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
    last_delta_sync_catch_up: Option<std::time::Instant>,
    consecutive_gap_one_ticks: u32,
    consecutive_behind_yields: u32,
    consecutive_qcc_failures: u32,
    qcc_failure_height: u64,
    qcc_yielded_height: u64,
    fast_path_yield_start: Option<std::time::Instant>,
    pending_qcc: Option<PendingQcc>,
    qcc_retry_data: Option<QccRetryData>,
    #[cfg(feature = "p2p")]
    pending_delta_sync: Option<PendingDeltaSync>,
    last_checkpoint_applied_at: Option<std::time::Instant>,
    last_own_proposal_time_ms: u64,
    tick_counter: u64,
    total_checkpoints_produced: u64,
    total_qcc_wait_ms: u128,
    total_qcc_pickup_delay_ms: u128,
    last_dashboard_at: std::time::Instant,
}

const FORK_RECOVERY_THRESHOLD: u32 = 3;
const LEADER_SKIP_BASE_TICKS: u32 = 3;
const LEADER_SKIP_STAGGER_TICKS: u32 = 1;
const LEADER_INTENT_EXTENSION_TICKS: u32 = 2;
const MAX_CHECKPOINT_TXS: usize = 1000;
const POST_SYNC_COOLDOWN_MS: u64 = 500;
const LEADER_POST_SYNC_MAX_DEFER_MS: u64 = 500;
const MIN_INTER_CHECKPOINT_MS: u64 = 2000;
const QCC_SELF_YIELD_THRESHOLD: u32 = 2;

// Use centralized constant from config
use crate::config::PROPAGATION_GRACE_MS;

const PARALLEL_QCC_DEADLINE_MS: u64 = 4000;

#[cfg(feature = "p2p")]
async fn collect_qcc_standalone(
    checkpoint: Checkpoint,
    finalized_tx_hashes: &[String],
    gossip_service: Option<Arc<crate::gossip::GossipService>>,
    network_handle: Option<Arc<NetworkHandle>>,
    validator_identity: Option<Arc<RwLock<ValidatorIdentityService>>>,
    validator_address: &str,
    checkpoint_height_cache: Arc<std::sync::atomic::AtomicU64>,
) -> Option<(Vec<ValidatorSignature>, Vec<u8>, Vec<u8>)> {
    let total_stake = if let Some(ref identity) = validator_identity {
        let identity_guard = identity.read().await;
        identity_guard
            .active_validators()
            .iter()
            .map(|(_, v)| v.effective_stake)
            .sum::<u64>()
    } else {
        return None;
    };

    if total_stake == 0 {
        return None;
    }

    let quorum_threshold = (total_stake * 2 + 2) / 3;

    let mut collected_sigs: Vec<ValidatorSignature> = checkpoint.validator_signatures.clone();
    let mut sig_bytes_list: Vec<Vec<u8>> = Vec::new();
    let mut signer_addresses: Vec<String> = vec![validator_address.to_string()];

    let our_canonical_stake = if let Some(ref identity) = validator_identity {
        let identity_guard = identity.read().await;
        identity_guard
            .active_validators()
            .iter()
            .find(|(addr, _)| *addr == validator_address)
            .map(|(_, v)| v.effective_stake)
            .unwrap_or(0)
    } else {
        0
    };
    let mut collected_stake: u64 = our_canonical_stake;

    for sig in &checkpoint.validator_signatures {
        if let Ok(decoded) = URL_SAFE_NO_PAD.decode(&sig.signature) {
            sig_bytes_list.push(decoded);
        }
    }

    info!(
        "QCC-PIPELINE: Collecting votes for checkpoint {} at height {} (our_canonical_stake={}, quorum={}/{})",
        &checkpoint.hash[..16.min(checkpoint.hash.len())],
        checkpoint.height, our_canonical_stake, quorum_threshold, total_stake
    );

    if let Some(ref gossip) = gossip_service {
        let (vote_tx, mut vote_rx) = tokio::sync::mpsc::channel::<crate::gossip::QccGossipVote>(32);
        gossip.set_qcc_vote_channel(vote_tx).await;

        gossip
            .broadcast_qcc_vote_request(
                checkpoint.height,
                &checkpoint.hash,
                &checkpoint.tx_merkle_root,
                &checkpoint.state_root,
                validator_address,
                finalized_tx_hashes,
            )
            .await;

        let mut fallback_futs: Option<FuturesUnordered<_>> = {
            if let Some(ref network_handle) = network_handle {
                let peer_ids = network_handle.get_connected_peer_ids().await;
                if !peer_ids.is_empty() {
                    let vote_request = CheckpointVoteRequest {
                        checkpoint_hash: checkpoint.hash.clone(),
                        height: checkpoint.height,
                        tx_merkle_root: checkpoint.tx_merkle_root.clone(),
                        state_root: checkpoint.state_root.clone(),
                        finalized_tx_hashes: finalized_tx_hashes.to_vec(),
                        finalized_transactions: vec![],
                    };
                    let timeout_dur = std::time::Duration::from_millis(
                        PARALLEL_QCC_DEADLINE_MS.saturating_sub(200),
                    );
                    let futs: FuturesUnordered<_> = peer_ids
                        .iter()
                        .map(|peer_id| {
                            let pid = peer_id.clone();
                            let nh = Arc::clone(network_handle);
                            let req = VoteRequest::CheckpointVote(vote_request.clone());
                            let td = timeout_dur;
                            async move {
                                match tokio::time::timeout(td, nh.vote_request(&pid, req)).await {
                                    Ok(Ok(response)) => (pid, Some(response)),
                                    Ok(Err(e)) => {
                                        warn!(
                                            "QCC-RR: Vote request to {} failed: {}",
                                            &pid[..16.min(pid.len())],
                                            e
                                        );
                                        (pid, None)
                                    }
                                    Err(_) => {
                                        warn!(
                                            "QCC-RR: Vote request to {} timed out",
                                            &pid[..16.min(pid.len())]
                                        );
                                        (pid, None)
                                    }
                                }
                            }
                        })
                        .collect();
                    info!(
                        "QCC-PIPELINE-PARALLEL: Launched gossip + request-response to {} peers (deadline={}ms)",
                        peer_ids.len(), PARALLEL_QCC_DEADLINE_MS
                    );
                    Some(futs)
                } else {
                    None
                }
            } else {
                None
            }
        };

        let deadline = tokio::time::Instant::now()
            + tokio::time::Duration::from_millis(PARALLEL_QCC_DEADLINE_MS);

        let mut gossip_votes = 0u32;
        let mut rr_votes = 0u32;
        let qcc_start = std::time::Instant::now();

        loop {
            if collected_stake >= quorum_threshold {
                let quorum_ms = qcc_start.elapsed().as_millis();
                let voter_list: Vec<String> = signer_addresses
                    .iter()
                    .map(|a| {
                        format!(
                            "{}({})",
                            &a[..12.min(a.len())],
                            collected_sigs
                                .iter()
                                .find(|s| &s.validator == a)
                                .map(|s| s.weight)
                                .unwrap_or(0)
                        )
                    })
                    .collect();
                info!(
                    "QCC-PIPELINE: Quorum reached for height {} ({}/{} stake, {} sigs, gossip={}, rr={}) in {}ms voters=[{}]",
                    checkpoint.height, collected_stake, total_stake, collected_sigs.len(),
                    gossip_votes, rr_votes, quorum_ms, voter_list.join(", ")
                );
                break;
            }

            let current_height = checkpoint_height_cache.load(std::sync::atomic::Ordering::Relaxed);
            if current_height >= checkpoint.height {
                gossip.clear_qcc_vote_channel().await;
                info!(
                    "QCC-PIPELINE: Aborting — height {} already committed",
                    checkpoint.height
                );
                return None;
            }

            tokio::select! {
                result = vote_rx.recv() => {
                    match result {
                        Some(vote) => {
                            if vote.height != checkpoint.height || vote.checkpoint_hash != checkpoint.hash {
                                continue;
                            }
                            if signer_addresses.contains(&vote.validator_address) {
                                continue;
                            }
                            if let Some(accepted) = verify_vote_standalone(
                                &vote.validator_address,
                                &vote.bls_public_key,
                                &vote.signature,
                                &checkpoint.hash,
                                &validator_identity,
                            ).await {
                                collected_stake += accepted.canonical_stake;
                                signer_addresses.push(vote.validator_address.clone());
                                sig_bytes_list.push(accepted.sig_bytes);
                                collected_sigs.push(ValidatorSignature {
                                    validator: vote.validator_address,
                                    signature: vote.signature,
                                    weight: accepted.canonical_stake,
                                    bls_public_key: Some(vote.bls_public_key),
                                });
                                gossip_votes += 1;
                                info!(
                                    "QCC-PIPELINE-GOSSIP: Valid vote (canonical_stake={}, accumulated={}/{})",
                                    accepted.canonical_stake, collected_stake, quorum_threshold
                                );
                            }
                        }
                        None => {}
                    }
                }

                result = async {
                    if let Some(ref mut futs) = fallback_futs {
                        futs.next().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    if let Some((peer_id, maybe_response)) = result {
                        if let Some(response) = maybe_response {
                            match response {
                                VoteResponse::CheckpointVote(Some(vote)) => {
                                    if signer_addresses.contains(&vote.validator_address) {
                                        continue;
                                    }
                                    if let Some(accepted) = verify_vote_standalone(
                                        &vote.validator_address,
                                        &vote.bls_public_key,
                                        &vote.signature,
                                        &checkpoint.hash,
                                        &validator_identity,
                                    ).await {
                                        collected_stake += accepted.canonical_stake;
                                        signer_addresses.push(vote.validator_address.clone());
                                        sig_bytes_list.push(accepted.sig_bytes);
                                        collected_sigs.push(ValidatorSignature {
                                            validator: vote.validator_address,
                                            signature: vote.signature,
                                            weight: accepted.canonical_stake,
                                            bls_public_key: Some(vote.bls_public_key),
                                        });
                                        rr_votes += 1;
                                        info!(
                                            "QCC-PIPELINE-RR: Valid vote from {} (canonical_stake={}, accumulated={}/{})",
                                            &peer_id[..16.min(peer_id.len())],
                                            accepted.canonical_stake, collected_stake, quorum_threshold
                                        );
                                    }
                                }
                                VoteResponse::CheckpointVote(None) => {}
                                VoteResponse::Error { message } => {
                                    warn!("QCC-RR: Vote error from {}: {}", &peer_id[..16.min(peer_id.len())], message);
                                }
                            }
                        }
                    } else {
                        fallback_futs = None;
                    }
                }

                _ = tokio::time::sleep_until(deadline) => {
                    info!(
                        "QCC-PIPELINE: Deadline reached ({}ms) — collected {}/{} stake (gossip={}, rr={})",
                        PARALLEL_QCC_DEADLINE_MS, collected_stake, quorum_threshold,
                        gossip_votes, rr_votes
                    );
                    break;
                }
            }
        }

        gossip.clear_qcc_vote_channel().await;

        if collected_stake < quorum_threshold {
            if let Some(ref nh) = network_handle {
                let peers = nh.get_connected_peers().await;
                let mut heights: Vec<u64> = peers
                    .iter()
                    .filter(|p| p.handshake_validated)
                    .filter_map(|p| p.handshake_info.as_ref())
                    .map(|h| h.checkpoint_height)
                    .collect();
                if !heights.is_empty() {
                    heights.sort_unstable();
                    let median = heights[heights.len() / 2];
                    if median > checkpoint.height {
                        warn!(
                            "QCC-PIPELINE: Aborting — behind peers (network={}, our checkpoint={})",
                            median, checkpoint.height
                        );
                        return None;
                    }
                }
            }
        }
    }

    if collected_stake < quorum_threshold {
        let voter_list: Vec<String> = signer_addresses
            .iter()
            .map(|a| {
                format!(
                    "{}({})",
                    &a[..12.min(a.len())],
                    collected_sigs
                        .iter()
                        .find(|s| &s.validator == a)
                        .map(|s| s.weight)
                        .unwrap_or(0)
                )
            })
            .collect();
        warn!(
            "QCC-PIPELINE: Failed to reach quorum for height {} ({}/{} stake, need {}, got {} votes) voters=[{}]",
            checkpoint.height, collected_stake, total_stake, quorum_threshold, collected_sigs.len(),
            voter_list.join(", ")
        );
        return None;
    }

    let aggregated_sig = match aggregate_signatures(&sig_bytes_list) {
        Ok(agg) => agg,
        Err(e) => {
            warn!(
                "QCC-PIPELINE: BLS aggregation failed for height {}: {}",
                checkpoint.height, e
            );
            return None;
        }
    };

    let sorted_validators: Vec<String> = if let Some(ref identity) = validator_identity {
        let identity_guard = identity.read().await;
        let mut addrs: Vec<String> = identity_guard
            .active_validators()
            .iter()
            .filter(|(_, v)| !v.bls_public_key.is_empty())
            .map(|(addr, _)| addr.clone())
            .collect();
        addrs.sort();
        addrs
    } else {
        signer_addresses.clone()
    };

    let total_validators = sorted_validators.len();
    let signer_indices: Vec<usize> = signer_addresses
        .iter()
        .filter_map(|addr| sorted_validators.iter().position(|a| a == addr))
        .collect();

    let bitmap = create_signer_bitmap(&signer_indices, total_validators);

    Some((collected_sigs, aggregated_sig, bitmap))
}

#[cfg(feature = "p2p")]
async fn verify_vote_standalone(
    validator_address: &str,
    bls_public_key: &str,
    signature: &str,
    checkpoint_hash: &str,
    validator_identity: &Option<Arc<RwLock<ValidatorIdentityService>>>,
) -> Option<AcceptedVote> {
    let checkpoint_hash_bytes = match hex::decode(checkpoint_hash) {
        Ok(b) => b,
        Err(_) => return None,
    };
    let bls_pub_bytes = match URL_SAFE_NO_PAD.decode(bls_public_key) {
        Ok(b) => b,
        Err(_) => {
            warn!(
                "QCC: Invalid BLS public key from {}",
                &validator_address[..16.min(validator_address.len())]
            );
            return None;
        }
    };
    let sig_bytes = match URL_SAFE_NO_PAD.decode(signature) {
        Ok(b) => b,
        Err(_) => return None,
    };

    if !bls_verify(&checkpoint_hash_bytes, &sig_bytes, &bls_pub_bytes) {
        warn!(
            "QCC: Invalid BLS signature from {} — rejecting",
            &validator_address[..16.min(validator_address.len())]
        );
        return None;
    }

    let (canonical_stake, canonical_bls_key) = if let Some(ref identity) = validator_identity {
        let identity_guard = identity.read().await;
        identity_guard
            .active_validators()
            .iter()
            .find(|(addr, _)| *addr == validator_address)
            .map(|(_, v)| (v.effective_stake, v.bls_public_key_base64()))
            .unwrap_or((0, String::new()))
    } else {
        (0, String::new())
    };

    if !canonical_bls_key.is_empty() && canonical_bls_key != bls_public_key {
        warn!(
            "QCC: Vote from {} has BLS key mismatch — rejecting",
            &validator_address[..16.min(validator_address.len())]
        );
        return None;
    }
    if canonical_stake == 0 {
        warn!(
            "QCC: Vote from {} has zero canonical stake — ignoring",
            &validator_address[..16.min(validator_address.len())]
        );
        return None;
    }

    Some(AcceptedVote {
        canonical_stake,
        sig_bytes,
    })
}

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
            pruning_service: Some(Arc::new(tokio::sync::Mutex::new(DagPruningService::new(
                PruningConfig::default(),
            )))),
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
            last_delta_sync_catch_up: None,
            consecutive_gap_one_ticks: 0,
            consecutive_behind_yields: 0,
            consecutive_qcc_failures: 0,
            qcc_failure_height: 0,
            qcc_yielded_height: 0,
            fast_path_yield_start: None,
            pending_qcc: None,
            qcc_retry_data: None,
            #[cfg(feature = "p2p")]
            pending_delta_sync: None,
            last_checkpoint_applied_at: None,
            last_own_proposal_time_ms: 0,
            tick_counter: 0,
            total_checkpoints_produced: 0,
            total_qcc_wait_ms: 0,
            total_qcc_pickup_delay_ms: 0,
            last_dashboard_at: std::time::Instant::now(),
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

    pub fn with_validator_identity(
        mut self,
        identity: Arc<RwLock<ValidatorIdentityService>>,
    ) -> Self {
        self.validator_identity = Some(identity);
        self
    }

    pub fn with_pruning_config(mut self, config: PruningConfig) -> Self {
        self.pruning_service = Some(Arc::new(tokio::sync::Mutex::new(DagPruningService::new(
            config,
        ))));
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
        let initial_height = self.state.get_checkpoint_height();
        handle.update_checkpoint_height(initial_height);
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

        let tick_duration = tokio::time::Duration::from_millis(self.interval_ms);
        let qcc_poll_duration = tokio::time::Duration::from_millis(50);
        let mut next_full_tick = tokio::time::Instant::now() + tick_duration;

        loop {
            let qcc_already_finished = self
                .pending_qcc
                .as_ref()
                .map(|p| p.qcc_handle.is_finished())
                .unwrap_or(false);

            if qcc_already_finished {
                let qcc_pickup_start = std::time::Instant::now();
                if let Err(e) = self.check_pending_qcc().await {
                    tracing::warn!("QCC early-pickup failed: {}", e);
                }
                let pickup_ms = qcc_pickup_start.elapsed().as_millis();
                info!(
                    "RCC-QCC-WAKEUP: QCC picked up in {}ms (next full tick in {}ms)",
                    pickup_ms,
                    next_full_tick
                        .saturating_duration_since(tokio::time::Instant::now())
                        .as_millis()
                );
            }

            #[cfg(feature = "p2p")]
            let delta_pending = {
                let finished = self
                    .pending_delta_sync
                    .as_ref()
                    .map(|p| p.handle.is_finished())
                    .unwrap_or(false);
                if finished {
                    self.pickup_async_delta_sync().await;
                }
                self.pending_delta_sync.is_some()
            };
            #[cfg(not(feature = "p2p"))]
            let delta_pending = false;

            let qcc_pending_unfinished = self
                .pending_qcc
                .as_ref()
                .map(|p| !p.qcc_handle.is_finished())
                .unwrap_or(false);

            if qcc_pending_unfinished || delta_pending {
                let wait = std::cmp::min(
                    qcc_poll_duration,
                    next_full_tick.saturating_duration_since(tokio::time::Instant::now()),
                );
                tokio::time::sleep(wait).await;
            } else {
                tokio::time::sleep_until(next_full_tick).await;
            }

            let now = tokio::time::Instant::now();
            let is_full_tick = now >= next_full_tick;
            if !is_full_tick {
                continue;
            }
            next_full_tick = now + tick_duration;

            self.tick_counter += 1;
            let tick_start = std::time::Instant::now();

            if let Some(ref validator_identity) = self.validator_identity {
                let result = validator_identity.write().await.process_epoch_transition();
                if result.new_epoch > result.old_epoch {
                    info!(
                        "Epoch transition: {} -> {} (activated: {}, exited: {})",
                        result.old_epoch,
                        result.new_epoch,
                        result.activated.len(),
                        result.exited.len()
                    );
                }
            }

            let had_pending_qcc = self.pending_qcc.is_some();
            let pending_height = self.pending_qcc.as_ref().map(|p| p.height).unwrap_or(0);
            let pending_qcc_finished = self
                .pending_qcc
                .as_ref()
                .map(|p| p.qcc_handle.is_finished())
                .unwrap_or(false);
            let pending_qcc_wait = self
                .pending_qcc
                .as_ref()
                .map(|p| p.qcc_spawned_at.elapsed().as_millis())
                .unwrap_or(0);

            if let Err(e) = self.create_state_snapshot().await {
                tracing::warn!("State snapshot failed: {}", e);
            }

            let tick_elapsed = tick_start.elapsed().as_millis();
            let new_pending = self.pending_qcc.is_some();
            let current_height = self.state.get_checkpoint_height();

            if tick_elapsed > 50 || had_pending_qcc {
                let fast_path_pool_size = {
                    let state = self.state.inner.read().await;
                    state.fast_path_finalized_txs.len()
                };
                info!(
                    "RCC-TICK #{}: {}ms | height={} | pending_qcc: before={} after={} | qcc_h={} qcc_finished={} qcc_age={}ms | fast_path_pool={} | produced={}",
                    self.tick_counter, tick_elapsed, current_height,
                    had_pending_qcc, new_pending, pending_height,
                    pending_qcc_finished, pending_qcc_wait,
                    fast_path_pool_size, self.total_checkpoints_produced
                );
            }

            const DASHBOARD_INTERVAL_SECS: u64 = 15;
            if self.last_dashboard_at.elapsed().as_secs() >= DASHBOARD_INTERVAL_SECS {
                let (_tps, tps_short, tps_long) = self.state.get_dynamic_tps().await;
                let (fast_path_pool_size, accounts_count) = {
                    let state = self.state.inner.read().await;
                    (
                        state.fast_path_finalized_txs.len(),
                        state.accounts.len(),
                    )
                };
                let gossip_stats = if let Some(ref gossip) = self.gossip_service {
                    let stats = gossip.get_fast_path_stats().await;
                    format!(
                        "pending={} confirmed={}",
                        stats.pending_count, stats.confirmed_count
                    )
                } else {
                    "no-gossip".to_string()
                };
                let avg_qcc_ms = if self.total_checkpoints_produced > 0 {
                    self.total_qcc_wait_ms / self.total_checkpoints_produced as u128
                } else {
                    0
                };
                let avg_pickup_delay = if self.total_checkpoints_produced > 0 {
                    self.total_qcc_pickup_delay_ms / self.total_checkpoints_produced as u128
                } else {
                    0
                };
                let cadence = self
                    .last_checkpoint_applied_at
                    .map(|t| format!("{}ms ago", t.elapsed().as_millis()))
                    .unwrap_or_else(|| "never".to_string());
                let peer_health_summary = if let Some(ref gossip) = self.gossip_service {
                    gossip.get_peer_health_summary().await
                } else {
                    "no-gossip".to_string()
                };
                info!(
                    "RCC-DASHBOARD: height={} tps_15s={:.1} tps_60s={:.1} | checkpoints_produced={} avg_qcc={}ms avg_pickup_delay={}ms qcc_failures={} | fast_path_pool={} accounts={} {} | {} | last_checkpoint={} | tick_interval={}ms yielded_h={}",
                    current_height, tps_short, tps_long,
                    self.total_checkpoints_produced, avg_qcc_ms, avg_pickup_delay,
                    self.consecutive_qcc_failures,
                    fast_path_pool_size, accounts_count, gossip_stats, peer_health_summary, cadence,
                    self.interval_ms, self.qcc_yielded_height
                );
                self.last_dashboard_at = std::time::Instant::now();
            }

            let prune_count = self
                .pruning_counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            const PRUNE_EVERY_N_CHECKPOINTS: u32 = 10;
            const IN_MEMORY_RETENTION: u64 = 10;

            if prune_count > 0 && prune_count % PRUNE_EVERY_N_CHECKPOINTS == 0 {
                let current_height = self.state.get_checkpoint_height();

                if current_height > 50 {
                    let finalized_hashes: std::collections::HashSet<String> = {
                        let state_guard = self.state.inner.read().await;
                        state_guard
                            .dag
                            .nodes()
                            .filter(|n| n.finalized)
                            .map(|n| n.hash.clone())
                            .collect()
                    };

                    if let Some(ref pruning_arc) = self.pruning_service {
                        let pruning = Arc::clone(pruning_arc);
                        let storage = Arc::clone(self.state.storage());
                        tokio::spawn(async move {
                            let mut pruning_guard = pruning.lock().await;
                            match pruning_guard.prune_dag(
                                &storage,
                                current_height,
                                &finalized_hashes,
                            ) {
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
                    let has_pruneable = {
                        let state_guard = self.state.inner.read().await;
                        state_guard
                            .dag
                            .has_finalized_before(current_height - IN_MEMORY_RETENTION)
                    };
                    if has_pruneable {
                        let mut state_guard = self.state.inner.write().await;
                        let pruned = state_guard
                            .dag
                            .prune_finalized_before(current_height - IN_MEMORY_RETENTION);
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
    }

    /// Sign the genesis checkpoint (height 0) if it exists but has no BLS signatures
    /// Note: We keep the original hash to ensure all nodes have the same genesis checkpoint hash
    async fn sign_genesis_checkpoint(&self) {
        let mut state = self.state.inner.write().await;

        let my_stake = state
            .validators
            .get(&self.validator_address)
            .map(|v| v.stake)
            .unwrap_or(0);

        // Find genesis checkpoint (height 0)
        if let Some(genesis_cp) = state.checkpoints.iter_mut().find(|cp| cp.height == 0) {
            // Check if it already has valid signatures with BLS keys
            let has_bls_keys = genesis_cp
                .validator_signatures
                .iter()
                .any(|s| s.bls_public_key.is_some());

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

                    info!(
                        "Genesis checkpoint signed: {}",
                        &genesis_cp.hash[..16.min(genesis_cp.hash.len())]
                    );
                }
            }
        }
    }

    #[cfg(feature = "p2p")]
    async fn get_network_consensus_height(&self) -> Option<u64> {
        let mut best: Option<u64> = None;

        if let Some(ref gossip) = self.gossip_service {
            let live = gossip.get_highest_seen_peer_height();
            if live > 0 {
                best = Some(live);
            }
        }

        if let Some(ref network_handle) = self.network_handle {
            let peers = network_handle.get_connected_peers().await;
            let mut heights: Vec<u64> = peers
                .iter()
                .filter(|p| p.handshake_validated)
                .filter_map(|p| p.handshake_info.as_ref())
                .map(|h| h.checkpoint_height)
                .filter(|h| *h > 0)
                .collect();
            if !heights.is_empty() {
                heights.sort_unstable();
                let median_idx = heights.len() / 2;
                let handshake_median = heights[median_idx];
                best = Some(best.map_or(handshake_median, |b| b.max(handshake_median)));
            }
        }

        best
    }

    /// Fetch and apply a peer checkpoint at the given height via P2P delta sync.
    /// Tries each peer sequentially, returning as soon as one provides useful data.
    async fn fetch_and_apply_peer_checkpoint(&self, _height: u64) -> bool {
        self.fetch_and_apply_peer_checkpoint_with_timeout(_height, 3000)
            .await
    }

    async fn fetch_and_apply_peer_checkpoint_medium(&self, _height: u64) -> bool {
        self.fetch_and_apply_peer_checkpoint_with_timeout(_height, 2200)
            .await
    }

    async fn fetch_and_apply_peer_checkpoint_fast(&self, _height: u64) -> bool {
        self.fetch_and_apply_peer_checkpoint_with_timeout(_height, 1500)
            .await
    }

    #[cfg(feature = "p2p")]
    async fn collect_checkpoint_votes(
        &self,
        checkpoint: &Checkpoint,
        finalized_tx_hashes: &[String],
        _finalized_transactions: &[SignedTransaction],
    ) -> Option<(Vec<ValidatorSignature>, Vec<u8>, Vec<u8>)> {
        let total_stake = if let Some(ref identity) = self.validator_identity {
            let identity_guard = identity.read().await;
            identity_guard
                .active_validators()
                .iter()
                .map(|(_, v)| v.effective_stake)
                .sum::<u64>()
        } else {
            return None;
        };

        if total_stake == 0 {
            return None;
        }

        let quorum_threshold = (total_stake * 2 + 2) / 3;

        let mut collected_sigs: Vec<ValidatorSignature> = checkpoint.validator_signatures.clone();
        let mut sig_bytes_list: Vec<Vec<u8>> = Vec::new();
        let mut signer_addresses: Vec<String> = vec![self.validator_address.clone()];

        let our_canonical_stake = if let Some(ref identity) = self.validator_identity {
            let identity_guard = identity.read().await;
            identity_guard
                .active_validators()
                .iter()
                .find(|(addr, _)| *addr == &self.validator_address)
                .map(|(_, v)| v.effective_stake)
                .unwrap_or(0)
        } else {
            self.our_stake
        };
        let mut collected_stake: u64 = our_canonical_stake;

        for sig in &checkpoint.validator_signatures {
            if let Ok(decoded) = URL_SAFE_NO_PAD.decode(&sig.signature) {
                sig_bytes_list.push(decoded);
            }
        }

        info!(
            "QCC: Collecting votes for checkpoint {} at height {} (our_canonical_stake={}, quorum={}/{}, gossip-first)",
            &checkpoint.hash[..16.min(checkpoint.hash.len())],
            checkpoint.height, our_canonical_stake, quorum_threshold, total_stake
        );

        const PARALLEL_QCC_DEADLINE_MS: u64 = 4000;

        if let Some(ref gossip) = self.gossip_service {
            let (vote_tx, mut vote_rx) =
                tokio::sync::mpsc::channel::<crate::gossip::QccGossipVote>(32);
            gossip.set_qcc_vote_channel(vote_tx).await;

            gossip
                .broadcast_qcc_vote_request(
                    checkpoint.height,
                    &checkpoint.hash,
                    &checkpoint.tx_merkle_root,
                    &checkpoint.state_root,
                    &self.validator_address,
                    finalized_tx_hashes,
                )
                .await;

            #[cfg(feature = "p2p")]
            let mut fallback_futs: Option<FuturesUnordered<_>> = {
                if let Some(ref network_handle) = self.network_handle {
                    let peer_ids = network_handle.get_connected_peer_ids().await;
                    if !peer_ids.is_empty() {
                        let vote_request = CheckpointVoteRequest {
                            checkpoint_hash: checkpoint.hash.clone(),
                            height: checkpoint.height,
                            tx_merkle_root: checkpoint.tx_merkle_root.clone(),
                            state_root: checkpoint.state_root.clone(),
                            finalized_tx_hashes: finalized_tx_hashes.to_vec(),
                            finalized_transactions: vec![],
                        };
                        let timeout_dur = std::time::Duration::from_millis(
                            PARALLEL_QCC_DEADLINE_MS.saturating_sub(200),
                        );
                        let futs: FuturesUnordered<_> = peer_ids
                            .iter()
                            .map(|peer_id| {
                                let pid = peer_id.clone();
                                let nh = Arc::clone(network_handle);
                                let req = VoteRequest::CheckpointVote(vote_request.clone());
                                let td = timeout_dur;
                                async move {
                                    match tokio::time::timeout(td, nh.vote_request(&pid, req)).await
                                    {
                                        Ok(Ok(response)) => (pid, Some(response)),
                                        Ok(Err(e)) => {
                                            warn!(
                                                "QCC-RR: Vote request to {} failed: {}",
                                                &pid[..16.min(pid.len())],
                                                e
                                            );
                                            (pid, None)
                                        }
                                        Err(_) => {
                                            warn!(
                                                "QCC-RR: Vote request to {} timed out",
                                                &pid[..16.min(pid.len())]
                                            );
                                            (pid, None)
                                        }
                                    }
                                }
                            })
                            .collect();
                        info!(
                            "QCC-PARALLEL: Launched gossip + request-response to {} peers simultaneously (deadline={}ms)",
                            peer_ids.len(), PARALLEL_QCC_DEADLINE_MS
                        );
                        Some(futs)
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            #[cfg(not(feature = "p2p"))]
            let mut fallback_futs: Option<
                FuturesUnordered<std::future::Pending<(String, Option<VoteResponse>)>>,
            > = None;

            let deadline = tokio::time::Instant::now()
                + tokio::time::Duration::from_millis(PARALLEL_QCC_DEADLINE_MS);

            let mut gossip_votes = 0u32;
            let mut rr_votes = 0u32;

            loop {
                if collected_stake >= quorum_threshold {
                    let voter_list: Vec<String> = signer_addresses
                        .iter()
                        .map(|a| {
                            format!(
                                "{}({})",
                                &a[..12.min(a.len())],
                                collected_sigs
                                    .iter()
                                    .find(|s| &s.validator == a)
                                    .map(|s| s.weight)
                                    .unwrap_or(0)
                            )
                        })
                        .collect();
                    info!(
                        "QCC-PARALLEL: Quorum reached for height {} ({}/{} stake, {} sigs, gossip={}, rr={}) voters=[{}]",
                        checkpoint.height, collected_stake, total_stake, collected_sigs.len(),
                        gossip_votes, rr_votes, voter_list.join(", ")
                    );
                    break;
                }

                let new_height = self.state.get_checkpoint_height();
                if new_height >= checkpoint.height {
                    gossip.clear_qcc_vote_channel().await;
                    info!(
                        "QCC: Aborting — height {} already committed",
                        checkpoint.height
                    );
                    return None;
                }

                tokio::select! {
                    result = vote_rx.recv() => {
                        match result {
                            Some(vote) => {
                                if vote.height != checkpoint.height || vote.checkpoint_hash != checkpoint.hash {
                                    continue;
                                }
                                if signer_addresses.contains(&vote.validator_address) {
                                    continue;
                                }
                                if let Some(accepted) = self.verify_and_accept_vote(
                                    &vote.validator_address,
                                    &vote.bls_public_key,
                                    &vote.signature,
                                    &checkpoint.hash,
                                    quorum_threshold,
                                    collected_stake,
                                ).await {
                                    collected_stake += accepted.canonical_stake;
                                    signer_addresses.push(vote.validator_address.clone());
                                    sig_bytes_list.push(accepted.sig_bytes);
                                    collected_sigs.push(ValidatorSignature {
                                        validator: vote.validator_address,
                                        signature: vote.signature,
                                        weight: accepted.canonical_stake,
                                        bls_public_key: Some(vote.bls_public_key),
                                    });
                                    gossip_votes += 1;
                                    info!(
                                        "QCC-GOSSIP: Valid vote (canonical_stake={}, accumulated={}/{})",
                                        accepted.canonical_stake, collected_stake, quorum_threshold
                                    );
                                }
                            }
                            None => {}
                        }
                    }

                    result = async {
                        if let Some(ref mut futs) = fallback_futs {
                            futs.next().await
                        } else {
                            std::future::pending().await
                        }
                    } => {
                        if let Some((peer_id, maybe_response)) = result {
                            if let Some(response) = maybe_response {
                                match response {
                                    VoteResponse::CheckpointVote(Some(vote)) => {
                                        if signer_addresses.contains(&vote.validator_address) {
                                            continue;
                                        }
                                        if let Some(accepted) = self.verify_and_accept_vote(
                                            &vote.validator_address,
                                            &vote.bls_public_key,
                                            &vote.signature,
                                            &checkpoint.hash,
                                            quorum_threshold,
                                            collected_stake,
                                        ).await {
                                            collected_stake += accepted.canonical_stake;
                                            signer_addresses.push(vote.validator_address.clone());
                                            sig_bytes_list.push(accepted.sig_bytes);
                                            collected_sigs.push(ValidatorSignature {
                                                validator: vote.validator_address,
                                                signature: vote.signature,
                                                weight: accepted.canonical_stake,
                                                bls_public_key: Some(vote.bls_public_key),
                                            });
                                            rr_votes += 1;
                                            info!(
                                                "QCC-RR: Valid vote from {} (accumulated={}/{})",
                                                &peer_id[..16.min(peer_id.len())], collected_stake, quorum_threshold
                                            );
                                        }
                                    }
                                    VoteResponse::CheckpointVote(None) => {}
                                    VoteResponse::Error { message } => {
                                        warn!("QCC-RR: Vote error from {}: {}", &peer_id[..16.min(peer_id.len())], message);
                                    }
                                }
                            }
                        } else {
                            fallback_futs = None;
                        }
                    }

                    _ = tokio::time::sleep_until(deadline) => {
                        info!(
                            "QCC-PARALLEL: Deadline reached ({}ms) — collected {}/{} stake (gossip={}, rr={})",
                            PARALLEL_QCC_DEADLINE_MS, collected_stake, quorum_threshold,
                            gossip_votes, rr_votes
                        );
                        break;
                    }
                }
            }

            gossip.clear_qcc_vote_channel().await;

            #[cfg(feature = "p2p")]
            if collected_stake < quorum_threshold {
                if let Some(network_best) = self.get_network_consensus_height().await {
                    if network_best > checkpoint.height {
                        warn!(
                            "QCC-PARALLEL: Aborting — behind peers (network={}, our checkpoint={})",
                            network_best, checkpoint.height
                        );
                        return None;
                    }
                }
            }
        }

        if collected_stake < quorum_threshold {
            let voter_list: Vec<String> = signer_addresses
                .iter()
                .map(|a| {
                    format!(
                        "{}({})",
                        &a[..12.min(a.len())],
                        collected_sigs
                            .iter()
                            .find(|s| &s.validator == a)
                            .map(|s| s.weight)
                            .unwrap_or(0)
                    )
                })
                .collect();
            warn!(
                "QCC: Failed to reach quorum for height {} ({}/{} stake, need {}, got {} votes) voters=[{}]",
                checkpoint.height, collected_stake, total_stake, quorum_threshold, collected_sigs.len(),
                voter_list.join(", ")
            );
            return None;
        }

        let aggregated_sig = match aggregate_signatures(&sig_bytes_list) {
            Ok(agg) => agg,
            Err(e) => {
                warn!(
                    "QCC: BLS aggregation failed for height {}: {}",
                    checkpoint.height, e
                );
                return None;
            }
        };

        let sorted_validators: Vec<String> = if let Some(ref identity) = self.validator_identity {
            let identity_guard = identity.read().await;
            let mut addrs: Vec<String> = identity_guard
                .active_validators()
                .iter()
                .filter(|(_, v)| !v.bls_public_key.is_empty())
                .map(|(addr, _)| addr.clone())
                .collect();
            addrs.sort();
            addrs
        } else {
            signer_addresses.clone()
        };

        let total_validators = sorted_validators.len();
        let signer_indices: Vec<usize> = signer_addresses
            .iter()
            .filter_map(|addr| sorted_validators.iter().position(|a| a == addr))
            .collect();

        let bitmap = create_signer_bitmap(&signer_indices, total_validators);

        Some((collected_sigs, aggregated_sig, bitmap))
    }

    async fn check_pending_qcc(&mut self) -> Result<bool> {
        let pending = match self.pending_qcc.take() {
            Some(p) => p,
            None => return Ok(false),
        };

        if pending.qcc_handle.is_finished() {
            let qcc_actual_ms = pending.qcc_spawned_at.elapsed().as_millis();
            let qcc_result = match pending.qcc_handle.await {
                Ok(result) => result,
                Err(e) => {
                    warn!("QCC task panicked for height {}: {}", pending.height, e);
                    None
                }
            };
            let t_qcc_ms = pending
                .t_overall
                .elapsed()
                .as_millis()
                .saturating_sub(pending.t_gather_ms + pending.t_proof_ms + pending.t_weight_ms);
            let pickup_delay_ms = t_qcc_ms.saturating_sub(qcc_actual_ms);

            match qcc_result {
                Some((sigs, agg_sig, bitmap)) => {
                    self.consecutive_qcc_failures = 0;
                    self.qcc_retry_data = None;
                    self.qcc_yielded_height = 0;
                    self.fast_path_yield_start = None;
                    let checkpoint = Checkpoint {
                        validator_signatures: sigs,
                        aggregated_signature: Some(URL_SAFE_NO_PAD.encode(&agg_sig)),
                        signer_bitmap: Some(bitmap),
                        ..pending.checkpoint
                    };
                    info!(
                        "QCC-PIPELINE: Checkpoint {} certified with {} validator signatures (qcc={}ms, actual={}ms, pickup_delay={}ms)",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        checkpoint.validator_signatures.len(),
                        t_qcc_ms, qcc_actual_ms, pickup_delay_ms
                    );
                    self.apply_certified_checkpoint(
                        checkpoint,
                        pending.hashes,
                        pending.txs_to_execute,
                        pending.reward_distributions,
                        pending.checkpoint_reward,
                        pending.finalized_proofs,
                        pending.fast_path_executed,
                        pending.height,
                        pending.now_ms,
                        pending.is_partitioned,
                        pending.partition_info,
                        pending.finality_sum,
                        pending.finality_count,
                        pending.finality_max,
                        pending.finality_times,
                        pending.t_overall,
                        pending.t_gather_ms,
                        pending.t_proof_ms,
                        pending.t_weight_ms,
                        t_qcc_ms,
                        qcc_actual_ms,
                    )
                    .await?;
                    return Ok(true);
                }
                None => {
                    let new_height = self.state.get_checkpoint_height();
                    if new_height >= pending.height {
                        info!(
                            "QCC-PIPELINE: Vote collection aborted — height {} already committed",
                            pending.height
                        );
                    } else {
                        self.consecutive_qcc_failures += 1;
                        warn!(
                            "QCC-PIPELINE: No quorum for checkpoint at height {} — rolling back emission (failure {}/{})",
                            pending.height, self.consecutive_qcc_failures, QCC_SELF_YIELD_THRESHOLD
                        );
                        if self.consecutive_qcc_failures < QCC_SELF_YIELD_THRESHOLD {
                            warn!(
                                "QCC-RETRY: Caching checkpoint {} at height {} for idempotent retry (hash={})",
                                pending.height,
                                self.consecutive_qcc_failures,
                                &pending.checkpoint.hash[..16.min(pending.checkpoint.hash.len())]
                            );
                            self.qcc_retry_data = Some(QccRetryData {
                                checkpoint: pending.checkpoint.clone(),
                                hashes: pending.hashes.clone(),
                                txs_to_execute: pending.txs_to_execute.clone(),
                                reward_distributions: pending.reward_distributions.clone(),
                                checkpoint_reward: pending.checkpoint_reward,
                                finalized_proofs: pending.finalized_proofs.clone(),
                                fast_path_executed: pending.fast_path_executed.clone(),
                                height: pending.height,
                                is_partitioned: pending.is_partitioned,
                                partition_info: pending.partition_info.clone(),
                            });
                        } else {
                            self.qcc_retry_data = None;
                        }
                        #[cfg(feature = "p2p")]
                        if let Some(ref gossip) = self.gossip_service {
                            gossip.clear_vote_lock_for_height(pending.height).await;
                            warn!(
                                "QCC-PIPELINE: Cleared vote lock at height {} after QCC failure — allowing new proposals",
                                pending.height
                            );
                        }
                        {
                            let mut emission = self.state.emission.write().await;
                            emission.rollback_to_height(pending.height.saturating_sub(1));
                        }
                        if !pending.reward_distributions.is_empty() {
                            let mut rewards = self.state.rewards.write().await;
                            for (addr, amount) in &pending.reward_distributions {
                                rewards.reverse_reward(addr, *amount);
                            }
                            info!(
                                "QCC-PIPELINE: Reversed {} reward distributions for aborted height {}",
                                pending.reward_distributions.len(), pending.height
                            );
                        }
                        if self.consecutive_qcc_failures >= QCC_SELF_YIELD_THRESHOLD {
                            warn!(
                                "QCC-PIPELINE SELF-YIELD: {} consecutive failures at height {} — broadcasting timeout",
                                self.consecutive_qcc_failures, pending.height
                            );
                            self.qcc_yielded_height = pending.height;
                            if let Some(ref gossip) = self.gossip_service {
                                let our_stake = {
                                    let state = self.state.inner.read().await;
                                    state
                                        .validators
                                        .get(&self.validator_address)
                                        .map(|v| v.stake)
                                        .unwrap_or(0)
                                };
                                gossip
                                    .broadcast_view_change(
                                        pending.height,
                                        1,
                                        &self.validator_address,
                                        our_stake,
                                        rinku_core::types::ViewChangeReason::LeaderTimeout,
                                    )
                                    .await;
                            }
                        }
                    }
                    return Ok(true);
                }
            }
        } else {
            let elapsed = pending.t_overall.elapsed().as_millis();
            if elapsed > 8000 {
                warn!(
                    "QCC-PIPELINE: Pending QCC for height {} has been running for {}ms — aborting",
                    pending.height, elapsed
                );
                pending.qcc_handle.abort();
                self.consecutive_qcc_failures += 1;
                warn!(
                    "QCC-PIPELINE TIMEOUT: Aborting QCC for height {} after {}ms (failure {}/{})",
                    pending.height,
                    elapsed,
                    self.consecutive_qcc_failures,
                    QCC_SELF_YIELD_THRESHOLD
                );
                if self.consecutive_qcc_failures < QCC_SELF_YIELD_THRESHOLD {
                    warn!(
                        "QCC-RETRY: Caching checkpoint (timeout) at height {} for idempotent retry (hash={})",
                        pending.height,
                        &pending.checkpoint.hash[..16.min(pending.checkpoint.hash.len())]
                    );
                    self.qcc_retry_data = Some(QccRetryData {
                        checkpoint: pending.checkpoint.clone(),
                        hashes: pending.hashes.clone(),
                        txs_to_execute: pending.txs_to_execute.clone(),
                        reward_distributions: pending.reward_distributions.clone(),
                        checkpoint_reward: pending.checkpoint_reward,
                        finalized_proofs: pending.finalized_proofs.clone(),
                        fast_path_executed: pending.fast_path_executed.clone(),
                        height: pending.height,
                        is_partitioned: pending.is_partitioned,
                        partition_info: pending.partition_info.clone(),
                    });
                } else {
                    self.qcc_retry_data = None;
                }
                #[cfg(feature = "p2p")]
                if let Some(ref gossip) = self.gossip_service {
                    gossip.clear_vote_lock_for_height(pending.height).await;
                    warn!(
                        "QCC-PIPELINE: Cleared vote lock at height {} after QCC timeout — allowing new proposals",
                        pending.height
                    );
                }
                {
                    let mut emission = self.state.emission.write().await;
                    emission.rollback_to_height(pending.height.saturating_sub(1));
                }
                if !pending.reward_distributions.is_empty() {
                    let mut rewards = self.state.rewards.write().await;
                    for (addr, amount) in &pending.reward_distributions {
                        rewards.reverse_reward(addr, *amount);
                    }
                }
                if self.consecutive_qcc_failures >= QCC_SELF_YIELD_THRESHOLD {
                    warn!(
                        "QCC-PIPELINE SELF-YIELD (timeout): {} consecutive failures at height {} — broadcasting timeout",
                        self.consecutive_qcc_failures, pending.height
                    );
                    self.qcc_yielded_height = pending.height;
                    if let Some(ref gossip) = self.gossip_service {
                        let our_stake = {
                            let state = self.state.inner.read().await;
                            state
                                .validators
                                .get(&self.validator_address)
                                .map(|v| v.stake)
                                .unwrap_or(0)
                        };
                        gossip
                            .broadcast_view_change(
                                pending.height,
                                1,
                                &self.validator_address,
                                our_stake,
                                rinku_core::types::ViewChangeReason::LeaderTimeout,
                            )
                            .await;
                    }
                }
                return Ok(true);
            }
            self.pending_qcc = Some(pending);
            return Ok(false);
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply_certified_checkpoint(
        &mut self,
        checkpoint: Checkpoint,
        hashes: Vec<String>,
        txs_to_execute: Vec<SignedTransaction>,
        reward_distributions: Vec<(String, u64)>,
        checkpoint_reward: u64,
        finalized_proofs: std::collections::HashMap<String, rinku_core::types::AccountStateProof>,
        _fast_path_executed: std::collections::HashSet<String>,
        height: u64,
        now_ms: u64,
        is_partitioned: bool,
        partition_info: crate::state::partition::PartitionState,
        finality_sum: u64,
        finality_count: u64,
        finality_max: u64,
        finality_times: Vec<u64>,
        t_overall: std::time::Instant,
        t_gather_ms: u128,
        t_proof_ms: u128,
        t_weight_ms: u128,
        t_qcc_ms: u128,
        qcc_actual_ms: u128,
    ) -> Result<()> {
        let lock_start = std::time::Instant::now();
        let mut state = self.state.inner.write().await;
        let lock_wait_ms = lock_start.elapsed().as_millis();
        if lock_wait_ms > 5 {
            info!(
                "RCC-LOCK: write lock acquired in {}ms for checkpoint h={}",
                lock_wait_ms, height
            );
        }
        let current_tip = state.checkpoints.last().map(|c| c.height).unwrap_or(0);
        if current_tip + 1 != height {
            drop(state);
            info!(
                "QCC-PIPELINE ABORT: Local tip advanced to {} while QCC ran for checkpoint {} — another node produced it first",
                current_tip, height
            );
            return Ok(());
        }

        let fast_path_already_finalized: std::collections::HashSet<String> = hashes
            .iter()
            .filter(|h| state.fast_path_finalized_txs.contains_key(h.as_str()))
            .cloned()
            .collect();

        let mut contract_lane_txs: Vec<SignedTransaction> = txs_to_execute
            .iter()
            .filter(|tx| !fast_path_already_finalized.contains(&tx.hash))
            .cloned()
            .collect();
        contract_lane_txs.sort_by(|a, b| {
            a.tx.from
                .cmp(&b.tx.from)
                .then(a.tx.nonce.cmp(&b.tx.nonce))
                .then(a.hash.cmp(&b.hash))
        });

        let available_nonces: std::collections::HashMap<String, std::collections::BTreeSet<u64>> = {
            let mut map: std::collections::HashMap<String, std::collections::BTreeSet<u64>> =
                std::collections::HashMap::new();
            for tx in &contract_lane_txs {
                if !matches!(
                    tx.tx.kind,
                    Some(rinku_core::types::TransactionKind::Consolidation)
                ) {
                    map.entry(tx.tx.from.clone())
                        .or_default()
                        .insert(tx.tx.nonce);
                }
            }
            map
        };

        if !fast_path_already_finalized.is_empty() {
            info!(
                "Proposer checkpoint h={}: {} fast-path TXs (already applied), {} contract-lane TXs to execute",
                height, fast_path_already_finalized.len(), contract_lane_txs.len()
            );
        }

        let pre_snapshot: std::collections::HashMap<String, (u64, u64, u64)> = state
            .accounts
            .iter()
            .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
            .collect();
        state.pre_checkpoint_accounts_snapshot = Some((height, pre_snapshot));
        state.checkpoints.push(checkpoint.clone());
        state.last_checkpoint_time_ms = now_ms;
        self.last_own_proposal_time_ms = now_ms;
        self.state
            .checkpoint_height_cache
            .store(checkpoint.height, std::sync::atomic::Ordering::Relaxed);
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

        let batch_result =
            crate::state::NodeState::execute_batch_inline(&mut state, &contract_lane_txs, &available_nonces);

        {
            let cleared = state.clear_checkpoint_finalized_txs(&fast_path_already_finalized);
            if cleared > 0 {
                tracing::info!(
                    "Proposer checkpoint h={}: cleared {} fast-path finalized entries",
                    height, cleared
                );
            }
        }

        let cleanup_hashes: Vec<String> = hashes
            .iter()
            .filter(|h| {
                batch_result.executed_hashes.contains(h.as_str())
                    || fast_path_already_finalized.contains(h.as_str())
            })
            .cloned()
            .collect();
        if !cleanup_hashes.is_empty() {
            state.dag.cleanup_sender_unfinalized_batch(&cleanup_hashes);
        }

        {
            let snapshot: std::collections::HashMap<String, (u64, u64, u64)> = state
                .accounts
                .iter()
                .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
                .collect();
            state.checkpoint_accounts_snapshot = Some((height, snapshot));
        }

        {
            let compact_start = std::time::Instant::now();
            let old_dirty = state.state_trie.dirty_node_count();
            state.state_trie = crate::state::StateInner::build_state_trie_from_accounts(&state.accounts);
            let compact_ms = compact_start.elapsed().as_millis();
            if compact_ms > 2 || old_dirty > 5000 {
                tracing::info!(
                    "SMT compaction at h={}: rebuilt trie from {} accounts (was {} dirty nodes, took {}ms)",
                    height, state.accounts.len(), old_dirty, compact_ms
                );
            }
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
            self.state
                .record_finalized_batch(snapshot_finalized_count)
                .await;
        }

        self.total_checkpoints_produced += 1;
        self.total_qcc_wait_ms += qcc_actual_ms;
        self.total_qcc_pickup_delay_ms += t_qcc_ms.saturating_sub(qcc_actual_ms);
        let cadence_ms = self
            .last_checkpoint_applied_at
            .map(|t| t.elapsed().as_millis())
            .unwrap_or(0);
        self.last_checkpoint_applied_at = Some(std::time::Instant::now());

        let total_ms = t_overall.elapsed().as_millis();
        info!(
            "Created checkpoint {} h={} ({} txs, {:.6} RKU, {}ms total) | gather={}ms proof={}ms weight={}ms qcc={}ms finalize={}ms lock={}ms cadence={}ms [PIPELINED]",
            &checkpoint.hash[..16],
            height,
            hashes.len(),
            rinku_core::types::from_micro_units(checkpoint_reward),
            total_ms,
            t_gather_ms,
            t_proof_ms,
            t_weight_ms,
            t_qcc_ms,
            total_ms.saturating_sub(t_gather_ms + t_proof_ms + t_weight_ms + t_qcc_ms),
            lock_wait_ms,
            cadence_ms
        );

        #[cfg(feature = "p2p")]
        if let Some(ref nh) = self.network_handle {
            nh.update_checkpoint_height(height);
        }

        if let Some(ref eb) = self.event_bus {
            let vr: Vec<(String, f64)> = reward_distributions
                .iter()
                .map(|(addr, amt)| (addr.clone(), rinku_core::types::from_micro_units(*amt)))
                .collect();
            eb.publish(crate::events::NodeEvent::CheckpointCreated {
                hash: checkpoint.hash.clone(),
                height,
                txs_finalized: hashes.len(),
                reward: rinku_core::types::from_micro_units(checkpoint_reward),
                validator_rewards: vr,
            });
        }

        let proof_tx_hash = hashes
            .first()
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

        self.state
            .store_batch_deferred(batch_result.new_deferred, std::collections::HashMap::new())
            .await;

        self.state
            .process_batch_special_txs_with_skip(
                &batch_result.special_txs,
                &fast_path_already_finalized,
            )
            .await;

        {
            let state = self.state.inner.read().await;
            let mut rewards = self.state.rewards.write().await;
            for (addr, account) in &state.accounts {
                if account.staked > 0 {
                    rewards.sync_stake_amount(addr, account.staked);
                }
            }
        }

        self.state
            .process_batch_reward_infos(&contract_lane_txs, &batch_result.executed_hashes)
            .await;

        tracing::info!(
            "Proposer checkpoint h={} batch executed {}/{} contract-lane txs, {} fast-path (pre-finalized), {} gap-skipped senders",
            height,
            batch_result.executed_count, contract_lane_txs.len(),
            fast_path_already_finalized.len(), batch_result.gap_skipped_senders.len()
        );

        self.state.store_precomputed_proofs(&final_proofs).await;

        if let Some(ref consensus) = self.consensus_service {
            let participating_validators: Vec<String> = checkpoint
                .validator_signatures
                .iter()
                .map(|sig| sig.validator.clone())
                .collect();
            let mut consensus_guard = consensus.write().await;
            consensus_guard
                .track_liveness(height, &participating_validators)
                .await;
        }

        if let Some(ref gossip) = self.gossip_service {
            let proofs_vec: Vec<rinku_core::types::AccountStateProof> =
                final_proofs.values().cloned().collect();
            gossip
                .broadcast_checkpoint(
                    checkpoint.clone(),
                    hashes.clone(),
                    txs_to_execute.clone(),
                    proofs_vec.clone(),
                )
                .await;

            #[cfg(feature = "p2p")]
            {
                const BACKFILL_COUNT: usize = 3;
                let backfill = gossip.get_recent_checkpoints(height, BACKFILL_COUNT).await;
                let push_data = CheckpointPushData {
                    checkpoint,
                    finalized_tx_hashes: hashes.clone(),
                    finalized_transactions: txs_to_execute,
                    precomputed_proofs: proofs_vec,
                    backfill_checkpoints: backfill,
                };
                gossip.push_checkpoint_to_peers(push_data);
            }

            gossip.cleanup_finalized_full(&hashes, height).await;
        }

        Ok(())
    }

    async fn verify_and_accept_vote(
        &self,
        validator_address: &str,
        bls_public_key: &str,
        signature: &str,
        checkpoint_hash: &str,
        _quorum_threshold: u64,
        _collected_stake: u64,
    ) -> Option<AcceptedVote> {
        let checkpoint_hash_bytes = match hex::decode(checkpoint_hash) {
            Ok(b) => b,
            Err(_) => return None,
        };
        let bls_pub_bytes = match URL_SAFE_NO_PAD.decode(bls_public_key) {
            Ok(b) => b,
            Err(_) => {
                warn!(
                    "QCC: Invalid BLS public key from {}",
                    &validator_address[..16.min(validator_address.len())]
                );
                return None;
            }
        };
        let sig_bytes = match URL_SAFE_NO_PAD.decode(signature) {
            Ok(b) => b,
            Err(_) => return None,
        };

        if !bls_verify(&checkpoint_hash_bytes, &sig_bytes, &bls_pub_bytes) {
            warn!(
                "QCC: Invalid BLS signature from {} — rejecting",
                &validator_address[..16.min(validator_address.len())]
            );
            return None;
        }

        let (canonical_stake, canonical_bls_key) =
            if let Some(ref identity) = self.validator_identity {
                let identity_guard = identity.read().await;
                identity_guard
                    .active_validators()
                    .iter()
                    .find(|(addr, _)| *addr == validator_address)
                    .map(|(_, v)| (v.effective_stake, v.bls_public_key_base64()))
                    .unwrap_or((0, String::new()))
            } else {
                (0, String::new())
            };

        if !canonical_bls_key.is_empty() && canonical_bls_key != bls_public_key {
            warn!(
                "QCC: Vote from {} has BLS key mismatch — rejecting",
                &validator_address[..16.min(validator_address.len())]
            );
            return None;
        }
        if canonical_stake == 0 {
            warn!(
                "QCC: Vote from {} has zero canonical stake — ignoring",
                &validator_address[..16.min(validator_address.len())]
            );
            return None;
        }

        Some(AcceptedVote {
            canonical_stake,
            sig_bytes,
        })
    }

    async fn fetch_and_apply_peer_checkpoint_with_timeout(
        &self,
        _height: u64,
        timeout_ms: u64,
    ) -> bool {
        #[cfg(feature = "p2p")]
        {
            let network_handle = match self.network_handle.as_ref() {
                Some(h) => h,
                None => return false,
            };
            let peers = network_handle.get_connected_peers().await;

            let local_height = self.state.get_checkpoint_height();
            let from_cp = local_height;

            use futures::stream::{FuturesUnordered, StreamExt};
            let mut futs: FuturesUnordered<_> = peers
                .iter()
                .map(|peer| {
                    let peer_id = peer.peer_id.clone();
                    let nh = Arc::clone(network_handle);
                    let t_ms = timeout_ms;
                    async move {
                        let result = tokio::time::timeout(
                            std::time::Duration::from_millis(t_ms),
                            nh.request_delta(&peer_id, from_cp),
                        )
                        .await;
                        match result {
                            Ok(r) => (peer_id, Some(r), false),
                            Err(_) => {
                                warn!(
                                    "Delta sync request to peer {} timed out after {}ms",
                                    &peer_id[..16.min(peer_id.len())],
                                    t_ms
                                );
                                (peer_id, None, true)
                            }
                        }
                    }
                })
                .collect();

            let fetch_start = std::time::Instant::now();
            let mut peers_tried = 0u32;
            let mut peers_timeout = 0u32;
            let mut peers_error = 0u32;

            while let Some((peer_id, maybe_result, timed_out)) = futs.next().await {
                peers_tried += 1;
                if timed_out {
                    peers_timeout += 1;
                    if let Some(ref gossip) = self.gossip_service {
                        gossip
                            .record_peer_sync_failure(&peer_id[..16.min(peer_id.len())])
                            .await;
                    }
                    continue;
                }
                let result = match maybe_result {
                    Some(r) => r,
                    None => continue,
                };
                match result {
                    Ok(SyncResponse::Delta(delta)) => {
                        if let Some(ref gossip) = self.gossip_service {
                            gossip
                                .record_peer_sync_success(&peer_id[..16.min(peer_id.len())])
                                .await;
                        }
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

                        let delta_tx_map: std::collections::HashMap<String, SignedTransaction> =
                            delta
                                .transactions
                                .iter()
                                .map(|td| {
                                    let stx = SignedTransaction {
                                        tx: rinku_core::types::Transaction {
                                            from: td.from.clone(),
                                            to: td.to.clone(),
                                            amount: td.amount,
                                            nonce: td.nonce,
                                            timestamp: td.timestamp,
                                            parents: td.parents.clone(),
                                            kind: None,
                                            gas_limit: None,
                                            gas_price: Some(td.gas_price),
                                            data: None,
                                            signature: Some(td.signature.clone()),
                                            memo: td.memo.clone(),
                                            references: td.references.clone(),
                                        },
                                        hash: td.hash.clone(),
                                        signature: td.signature.clone(),
                                    };
                                    (td.hash.clone(), stx)
                                })
                                .collect();

                        let mut sorted_cps: Vec<&CheckpointData> =
                            delta.new_checkpoints.iter().collect();
                        sorted_cps.sort_by_key(|c| c.height);

                        let mut applied_count = 0u64;
                        for cp_data in &sorted_cps {
                            let current = self.state.get_checkpoint_height();
                            if cp_data.height <= current {
                                continue;
                            }
                            if cp_data.height != current + 1 {
                                if let Some(ref gossip) = self.gossip_service {
                                    let checkpoint = self.checkpoint_data_to_checkpoint(
                                        cp_data,
                                        &delta.new_checkpoints,
                                    );
                                    let mut buffer = gossip.checkpoint_buffer.lock().await;
                                    if !buffer.contains_key(&checkpoint.height) {
                                        info!(
                                            "Delta sync: buffering checkpoint {} at height {} (current: {}) for later",
                                            &checkpoint.hash[..16.min(checkpoint.hash.len())],
                                            checkpoint.height,
                                            current
                                        );
                                        buffer.insert(
                                            checkpoint.height,
                                            crate::gossip::BufferedCheckpoint {
                                                checkpoint,
                                                finalized_tx_hashes: cp_data
                                                    .finalized_tx_hashes
                                                    .clone(),
                                                finalized_transactions: Vec::new(),
                                                precomputed_proofs: Vec::new(),
                                                source: format!(
                                                    "delta-{}",
                                                    &peer_id[..16.min(peer_id.len())]
                                                ),
                                            },
                                        );
                                    }
                                }
                                continue;
                            }

                            let checkpoint =
                                self.checkpoint_data_to_checkpoint(cp_data, &delta.new_checkpoints);

                            {
                                let mut emission = self.state.emission.write().await;
                                let reward = emission.get_checkpoint_reward(checkpoint.height);
                                if emission.record_emission_for_height(checkpoint.height, reward) {
                                    let mut rewards = self.state.rewards.write().await;
                                    rewards.distribute_checkpoint_rewards(reward);
                                }
                            }

                            let finalized_tx_hashes = checkpoint.finalized_tx_hashes.clone();
                            match self
                                .state
                                .apply_checkpoint_catching_up(
                                    checkpoint.clone(),
                                    finalized_tx_hashes,
                                )
                                .await
                            {
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
                                        gossip
                                            .remove_finalized_from_fast_path(
                                                &checkpoint.finalized_tx_hashes,
                                            )
                                            .await;
                                        let cp_txs: Vec<SignedTransaction> = checkpoint
                                            .finalized_tx_hashes
                                            .iter()
                                            .filter_map(|h| delta_tx_map.get(h).cloned())
                                            .collect();
                                        let cp_proofs: Vec<rinku_core::types::AccountStateProof> =
                                            delta
                                                .precomputed_proofs
                                                .iter()
                                                .filter(|p| {
                                                    p.checkpoint_height == checkpoint.height
                                                })
                                                .cloned()
                                                .collect();
                                        gossip
                                            .cache_checkpoint_data(
                                                checkpoint.height,
                                                crate::gossip::CachedCheckpointData {
                                                    checkpoint: checkpoint.clone(),
                                                    finalized_tx_hashes: checkpoint
                                                        .finalized_tx_hashes
                                                        .clone(),
                                                    finalized_transactions: cp_txs,
                                                    precomputed_proofs: cp_proofs,
                                                },
                                            )
                                            .await;
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
                                            validator_rewards: vec![],
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
                            let new_height = self.state.get_checkpoint_height();
                            info!(
                                "Delta sync from peer {}: applied {} checkpoints (now at h={}), draining buffer",
                                &peer_id[..16.min(peer_id.len())],
                                applied_count,
                                new_height
                            );
                            if let Some(ref nh) = self.network_handle {
                                nh.update_checkpoint_height(new_height);
                            }
                            if let Some(ref gossip) = self.gossip_service {
                                gossip.drain_checkpoint_buffer().await;
                            }
                            let fetch_ms = fetch_start.elapsed().as_millis();
                            info!(
                                "DELTA-FETCH: success in {}ms from peer {} | peers: tried={} timeout={} error={} | applied {} checkpoints to h={}",
                                fetch_ms, &peer_id[..16.min(peer_id.len())], peers_tried, peers_timeout, peers_error, applied_count, new_height
                            );
                            return true;
                        }
                    }
                    Ok(_) => {
                        peers_error += 1;
                        warn!(
                            "P2P peer {} returned unexpected response for delta",
                            &peer_id[..16.min(peer_id.len())]
                        );
                    }
                    Err(e) => {
                        peers_error += 1;
                        warn!(
                            "Failed to request delta from p2p peer {}: {}",
                            &peer_id[..16.min(peer_id.len())],
                            e
                        );
                    }
                }
            }
            let fetch_ms = fetch_start.elapsed().as_millis();
            info!(
                "DELTA-FETCH: completed in {}ms | peers: tried={} timeout={} error={} | no checkpoint applied",
                fetch_ms, peers_tried, peers_timeout, peers_error
            );
        }
        false
    }

    #[cfg(feature = "p2p")]
    fn spawn_async_delta_fetch(&mut self, from_height: u64, timeout_ms: u64) {
        if self.pending_delta_sync.is_some() {
            return;
        }
        let network_handle = match self.network_handle.as_ref() {
            Some(h) => Arc::clone(h),
            None => return,
        };
        let gossip = self.gossip_service.clone();

        info!(
            "ASYNC-DELTA-SYNC: spawning background fetch from height {} (timeout={}ms)",
            from_height, timeout_ms
        );

        let handle = tokio::spawn(async move {
            use crate::network::SyncResponse;
            let peers = network_handle.get_connected_peers().await;
            if peers.is_empty() {
                info!("ASYNC-DELTA-FETCH: no connected peers");
                return None;
            }

            let fetch_start = std::time::Instant::now();
            let mut peers_tried = 0u32;
            let mut peers_timeout = 0u32;
            let mut peers_error = 0u32;

            let mut futs: futures::stream::FuturesUnordered<_> = peers
                .iter()
                .map(|peer| {
                    let peer_id = peer.peer_id.clone();
                    let nh = Arc::clone(&network_handle);
                    let t_ms = timeout_ms;
                    async move {
                        let result = tokio::time::timeout(
                            std::time::Duration::from_millis(t_ms),
                            nh.request_delta(&peer_id, from_height),
                        )
                        .await;
                        match result {
                            Ok(r) => (peer_id, Some(r), false),
                            Err(_) => (peer_id, None, true),
                        }
                    }
                })
                .collect();

            use futures::stream::StreamExt;
            while let Some((peer_id, maybe_result, timed_out)) = futs.next().await {
                peers_tried += 1;
                let short_peer = peer_id[..16.min(peer_id.len())].to_string();
                if timed_out {
                    peers_timeout += 1;
                    warn!(
                        "ASYNC-DELTA-FETCH: peer {} timed out after {}ms",
                        short_peer, timeout_ms
                    );
                    if let Some(ref g) = gossip {
                        g.record_peer_sync_failure(&short_peer).await;
                    }
                    continue;
                }
                let result = match maybe_result {
                    Some(r) => r,
                    None => continue,
                };
                match result {
                    Ok(SyncResponse::Delta(delta)) => {
                        if let Some(ref g) = gossip {
                            g.record_peer_sync_success(&short_peer).await;
                        }
                        let fetch_ms = fetch_start.elapsed().as_millis();
                        if delta.new_checkpoints.is_empty() && delta.transactions.is_empty() {
                            tracing::debug!(
                                "ASYNC-DELTA-FETCH: peer {} returned empty delta in {}ms — trying next peer",
                                short_peer, fetch_ms
                            );
                            continue;
                        }
                        info!(
                            "ASYNC-DELTA-FETCH: success from peer {} in {}ms ({} txs, {} checkpoints) | tried={} timeout={} error={}",
                            short_peer, fetch_ms, delta.transactions.len(), delta.new_checkpoints.len(),
                            peers_tried, peers_timeout, peers_error
                        );
                        return Some(DeltaSyncFetchResult {
                            delta,
                            peer_id: short_peer,
                            fetch_ms,
                            peers_tried,
                            peers_timeout,
                            peers_error,
                        });
                    }
                    Ok(_) => {
                        peers_error += 1;
                    }
                    Err(e) => {
                        peers_error += 1;
                        warn!("ASYNC-DELTA-FETCH: peer {} error: {}", short_peer, e);
                    }
                }
            }

            let fetch_ms = fetch_start.elapsed().as_millis();
            info!(
                "ASYNC-DELTA-FETCH: all peers failed in {}ms | tried={} timeout={} error={}",
                fetch_ms, peers_tried, peers_timeout, peers_error
            );
            None
        });

        self.pending_delta_sync = Some(PendingDeltaSync {
            handle,
            spawned_at: std::time::Instant::now(),
            from_height,
        });
    }

    #[cfg(feature = "p2p")]
    async fn pickup_async_delta_sync(&mut self) {
        let pending = match self.pending_delta_sync.take() {
            Some(p) => p,
            None => return,
        };
        let elapsed_ms = pending.spawned_at.elapsed().as_millis();

        match pending.handle.await {
            Ok(Some(result)) => {
                let applied = self.apply_delta_sync_result(result).await;
                if applied {
                    self.last_delta_sync_catch_up = Some(std::time::Instant::now());
                    self.consecutive_gap_one_ticks = 0;
                    self.consecutive_behind_yields = 0;
                    let new_height = self.state.get_checkpoint_height();
                    info!(
                        "ASYNC-DELTA-SYNC: pickup applied checkpoints — now at height {} (total pickup time: {}ms)",
                        new_height, elapsed_ms
                    );
                    if let Some(ref gossip) = self.gossip_service {
                        gossip.drain_checkpoint_buffer().await;
                    }
                    let post_drain_height = self.state.get_checkpoint_height();
                    if post_drain_height > new_height {
                        info!(
                            "ASYNC-DELTA-SYNC: buffer drain advanced {} -> {} after delta apply",
                            new_height, post_drain_height
                        );
                    }

                    if let Some(network_best) = self.get_network_consensus_height().await {
                        let gap = network_best.saturating_sub(post_drain_height);
                        if gap >= 1 {
                            let timeout = if gap == 1 { 500 } else { 1500 };
                            self.spawn_async_delta_fetch(post_drain_height, timeout);
                        }
                    }
                } else {
                    info!(
                        "ASYNC-DELTA-SYNC: pickup received data but no checkpoints applied ({}ms)",
                        elapsed_ms
                    );
                }
            }
            Ok(None) => {
                info!(
                    "ASYNC-DELTA-SYNC: pickup — fetch returned no data ({}ms)",
                    elapsed_ms
                );
                if let Some(ref gossip) = self.gossip_service {
                    gossip.drain_checkpoint_buffer().await;
                    let post_drain = self.state.get_checkpoint_height();
                    if post_drain > pending.from_height {
                        self.last_delta_sync_catch_up = Some(std::time::Instant::now());
                        info!(
                            "ASYNC-DELTA-SYNC: fetch failed but buffer drain recovered {} -> {}",
                            pending.from_height, post_drain
                        );
                    }
                }
            }
            Err(e) => {
                warn!("ASYNC-DELTA-SYNC: background task panicked: {}", e);
            }
        }
    }

    #[cfg(feature = "p2p")]
    async fn apply_delta_sync_result(&mut self, result: DeltaSyncFetchResult) -> bool {
        let delta = result.delta;
        let peer_id = result.peer_id;

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
                    "ASYNC-DELTA-APPLY: ingested {} peer txs (peer {}, {} checkpoints available)",
                    ingested,
                    peer_id,
                    delta.new_checkpoints.len()
                );
            }
        }

        let delta_tx_map: std::collections::HashMap<String, SignedTransaction> = delta
            .transactions
            .iter()
            .map(|td| {
                let stx = SignedTransaction {
                    tx: rinku_core::types::Transaction {
                        from: td.from.clone(),
                        to: td.to.clone(),
                        amount: td.amount,
                        nonce: td.nonce,
                        timestamp: td.timestamp,
                        parents: td.parents.clone(),
                        kind: None,
                        gas_limit: None,
                        gas_price: Some(td.gas_price),
                        data: None,
                        signature: Some(td.signature.clone()),
                        memo: td.memo.clone(),
                        references: td.references.clone(),
                    },
                    hash: td.hash.clone(),
                    signature: td.signature.clone(),
                };
                (td.hash.clone(), stx)
            })
            .collect();

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
                    let checkpoint =
                        self.checkpoint_data_to_checkpoint(cp_data, &delta.new_checkpoints);
                    let mut buffer = gossip.checkpoint_buffer.lock().await;
                    if !buffer.contains_key(&checkpoint.height) {
                        info!(
                            "ASYNC-DELTA-APPLY: buffering checkpoint {} at height {} (current: {}) for later",
                            &checkpoint.hash[..16.min(checkpoint.hash.len())],
                            checkpoint.height,
                            current
                        );
                        buffer.insert(
                            checkpoint.height,
                            crate::gossip::BufferedCheckpoint {
                                checkpoint,
                                finalized_tx_hashes: cp_data.finalized_tx_hashes.clone(),
                                finalized_transactions: Vec::new(),
                                precomputed_proofs: Vec::new(),
                                source: format!("async-delta-{}", peer_id),
                            },
                        );
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
            let cp_proofs_for_apply: Vec<rinku_core::types::AccountStateProof> = delta
                .precomputed_proofs
                .iter()
                .filter(|p| p.checkpoint_height == checkpoint.height)
                .cloned()
                .collect();
            let apply_result = if !cp_proofs_for_apply.is_empty() {
                let proof_result = self
                    .state
                    .apply_checkpoint_proof_verified(
                        checkpoint.clone(),
                        finalized_tx_hashes.clone(),
                        &cp_proofs_for_apply,
                    )
                    .await;
                match proof_result {
                    Ok(count) => Ok(count),
                    Err(e) => {
                        warn!(
                            "ASYNC-DELTA proof-verified failed for h={}, falling back to execution: {}",
                            checkpoint.height, e
                        );
                        self.state
                            .apply_checkpoint_catching_up(checkpoint.clone(), finalized_tx_hashes)
                            .await
                    }
                }
            } else {
                self.state
                    .apply_checkpoint_catching_up(checkpoint.clone(), finalized_tx_hashes)
                    .await
            };
            match apply_result {
                Ok(missing_tx_count) => {
                    if missing_tx_count > 0 {
                        warn!(
                            "ASYNC-DELTA-APPLY: {} txs missing after checkpoint {} at height {}",
                            missing_tx_count,
                            &checkpoint.hash[..16.min(checkpoint.hash.len())],
                            checkpoint.height
                        );
                    }
                    applied_count += 1;
                    if let Some(ref gossip) = self.gossip_service {
                        gossip
                            .remove_finalized_from_fast_path(&checkpoint.finalized_tx_hashes)
                            .await;
                        let cp_txs: Vec<SignedTransaction> = checkpoint
                            .finalized_tx_hashes
                            .iter()
                            .filter_map(|h| delta_tx_map.get(h).cloned())
                            .collect();
                        let cp_proofs: Vec<rinku_core::types::AccountStateProof> = delta
                            .precomputed_proofs
                            .iter()
                            .filter(|p| p.checkpoint_height == checkpoint.height)
                            .cloned()
                            .collect();
                        gossip
                            .cache_checkpoint_data(
                                checkpoint.height,
                                crate::gossip::CachedCheckpointData {
                                    checkpoint: checkpoint.clone(),
                                    finalized_tx_hashes: checkpoint.finalized_tx_hashes.clone(),
                                    finalized_transactions: cp_txs,
                                    precomputed_proofs: cp_proofs,
                                },
                            )
                            .await;
                    }
                    info!(
                        "ASYNC-DELTA-APPLY: applied checkpoint {} at height {} from async delta sync",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        checkpoint.height
                    );
                    if let Some(ref eb) = self.event_bus {
                        eb.publish(crate::events::NodeEvent::CheckpointCreated {
                            hash: checkpoint.hash.clone(),
                            height: checkpoint.height,
                            txs_finalized: checkpoint.finalized_tx_hashes.len(),
                            reward: 0.0,
                            validator_rewards: vec![],
                        });
                    }
                }
                Err(e) => {
                    warn!(
                        "ASYNC-DELTA-APPLY: failed to apply checkpoint {} at height {}: {}",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        checkpoint.height,
                        e
                    );
                    break;
                }
            }
        }

        if applied_count > 0 {
            let new_height = self.state.get_checkpoint_height();
            if let Some(ref nh) = self.network_handle {
                nh.update_checkpoint_height(new_height);
            }
            info!(
                "ASYNC-DELTA-APPLY: applied {} checkpoints (now at h={})",
                applied_count, new_height
            );
            return true;
        }
        false
    }

    #[cfg(feature = "p2p")]
    fn checkpoint_data_to_checkpoint(
        &self,
        cp_data: &CheckpointData,
        all_cps: &[CheckpointData],
    ) -> Checkpoint {
        let previous_hash = all_cps
            .iter()
            .find(|c| c.height + 1 == cp_data.height)
            .and_then(|c| c.hash.clone());

        Checkpoint {
            height: cp_data.height,
            hash: cp_data
                .hash
                .clone()
                .unwrap_or_else(|| rinku_core::sha256_hex(&format!("cp:{}", cp_data.height))),
            previous_hash: previous_hash.or_else(|| cp_data.previous_hash.clone()),
            tx_merkle_root: cp_data.merkle_root.clone(),
            state_root: cp_data
                .state_root
                .clone()
                .unwrap_or_else(|| cp_data.merkle_root.clone()),
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
            view_change_certificate: None,
            view: 0,
        }
    }

    /// Sync missing transactions from peers via P2P delta sync
    async fn sync_missing_transactions(&self, target_height: u64) -> Result<()> {
        #[cfg(feature = "p2p")]
        {
            let network_handle = self
                .network_handle
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No P2P network handle available"))?;

            let from_checkpoint = {
                let state = self.state.inner.read().await;
                state.checkpoints.last().map(|cp| cp.height).unwrap_or(0)
            };

            let peers = network_handle.get_connected_peers().await;

            for peer in &peers {
                let peer_id = peer.peer_id.clone();
                let result = network_handle
                    .request_delta(&peer_id, from_checkpoint)
                    .await;
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
                                    .as_millis()
                                    as u64;
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
                                    fast_path_cert: None,
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
                        debug!(
                            "P2P peer {} returned unexpected response for delta",
                            &peer_id[..16.min(peer_id.len())]
                        );
                    }
                    Err(e) => {
                        debug!(
                            "Failed to sync delta from p2p peer {}: {}",
                            &peer_id[..16.min(peer_id.len())],
                            e
                        );
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
        unfinalized_hashes: &[String],
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
            let result = self
                .trust_verifier
                .verify_checkpoint(&peer_checkpoint, &validators);
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
            unfinalized_hashes
                .iter()
                .filter_map(|hash| {
                    state.dag.get_node(hash).and_then(|node| {
                        if node.finalized {
                            None // Skip already-finalized transactions
                        } else {
                            Some(node.tx.clone())
                        }
                    })
                })
                .collect()
        };

        txs_to_execute.sort_by(|a, b| {
            a.tx.from
                .cmp(&b.tx.from)
                .then(a.tx.nonce.cmp(&b.tx.nonce))
                .then(a.hash.cmp(&b.hash))
        });

        let mut state = self.state.inner.write().await;
        let pre_snapshot: std::collections::HashMap<String, (u64, u64, u64)> = state
            .accounts
            .iter()
            .map(|(addr, acc)| (addr.clone(), (acc.balance, acc.nonce, acc.staked)))
            .collect();
        state.pre_checkpoint_accounts_snapshot = Some((height, pre_snapshot));
        state.checkpoints.push(peer_checkpoint);
        state.last_checkpoint_time_ms = now_ms;
        self.state
            .checkpoint_height_cache
            .store(height, std::sync::atomic::Ordering::Relaxed);

        // Mark transactions as finalized (count only newly-finalized)
        let mut newly_finalized = 0u64;
        for hash in unfinalized_hashes {
            let was_finalized = state
                .dag
                .get_node(hash)
                .map(|n| n.finalized)
                .unwrap_or(true);
            if !was_finalized {
                let _ = state.dag.mark_finalized(hash, height);
                newly_finalized += 1;
            }
        }
        let finalized_count = newly_finalized;

        drop(state);

        if finalized_count > 0 {
            self.state.record_finalized_batch(finalized_count).await;
        }

        info!(
            "Adopted peer checkpoint {} at height {} ({} txs newly finalized of {} in leader list, {:.6} RKU emitted)",
            &checkpoint_hash[..16.min(checkpoint_hash.len())],
            height,
            newly_finalized,
            unfinalized_hashes.len(),
            rinku_core::types::from_micro_units(checkpoint_reward)
        );

        let fp_executed: std::collections::HashSet<String> = {
            let state_guard = self.state.inner.read().await;
            state_guard
                .fast_path_finalized_txs
                .keys()
                .cloned()
                .collect()
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
            let vr: Vec<(String, f64)> = distributions
                .iter()
                .map(|(addr, amt)| (addr.clone(), rinku_core::types::from_micro_units(*amt)))
                .collect();
            eb.publish(crate::events::NodeEvent::CheckpointCreated {
                hash: checkpoint_hash.clone(),
                height,
                txs_finalized: newly_finalized as usize,
                reward: rinku_core::types::from_micro_units(checkpoint_reward),
                validator_rewards: vr,
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
            let network_handle = self.network_handle.as_ref().ok_or_else(|| {
                anyhow::anyhow!("No P2P network handle available for chain recovery")
            })?;

            let peers = network_handle.get_connected_peers().await;

            if peers.is_empty() {
                return Err(anyhow::anyhow!("No P2P peers available for chain recovery"));
            }

            for peer in &peers {
                let peer_id = peer.peer_id.clone();
                let peer_id_short = &peer_id[..16.min(peer_id.len())];
                info!(
                    "[ForkRecovery] Requesting full snapshot sync from p2p peer {}",
                    peer_id_short
                );

                let result = network_handle.request_snapshot(&peer_id).await;
                match result {
                    Ok(SyncResponse::Snapshot(snapshot_data)) => {
                        use crate::state::presync::convert_snapshot_data_to_sync_snapshot;
                        let sync_snapshot = convert_snapshot_data_to_sync_snapshot(snapshot_data);

                        let mut linkage_valid = true;
                        for i in 1..sync_snapshot.checkpoints.len() {
                            let expected_prev = &sync_snapshot.checkpoints[i - 1].hash;
                            if sync_snapshot.checkpoints[i].previous_hash.as_deref()
                                != Some(expected_prev)
                            {
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
                                if checkpoint.validator_signatures.is_empty()
                                    && checkpoint.height > 1
                                {
                                    continue;
                                }
                                for sig in &checkpoint.validator_signatures {
                                    if let Ok(sig_bytes) = URL_SAFE_NO_PAD.decode(&sig.signature) {
                                        if sig_bytes.len() < 96
                                            || blst::min_pk::Signature::from_bytes(&sig_bytes)
                                                .is_err()
                                        {
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
                        let latest_height = sync_snapshot
                            .checkpoints
                            .last()
                            .map(|c| c.height)
                            .unwrap_or(0);

                        {
                            let mut state = self.state.inner.write().await;

                            state.checkpoints = sync_snapshot.checkpoints;
                            let sync_cp_height =
                                state.checkpoints.last().map(|cp| cp.height).unwrap_or(0);
                            self.state
                                .checkpoint_height_cache
                                .store(sync_cp_height, std::sync::atomic::Ordering::Relaxed);

                            state.accounts.clear();
                            for (fingerprint, account) in sync_snapshot.accounts {
                                state.accounts.insert(fingerprint, account);
                            }
                            state.state_trie =
                                crate::state::StateInner::build_state_trie_from_accounts(
                                    &state.accounts,
                                );

                            state.validators.clear();
                            for (addr, validator) in sync_snapshot.validators {
                                state.validators.insert(addr, validator);
                            }

                            let max_nodes = state.dag.node_count().max(10000);
                            state.dag = rinku_core::dag::Dag::new(max_nodes);

                            for tx in sync_snapshot.dag_transactions {
                                let parents = tx.tx.parents.clone();
                                let timestamp_ms =
                                    crate::config::normalize_timestamp_to_ms(tx.tx.timestamp);
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
                                    fast_path_cert: None,
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

                        self.consecutive_fork_failures
                            .store(0, std::sync::atomic::Ordering::SeqCst);

                        return Ok(true);
                    }
                    Ok(_) => {
                        debug!(
                            "[ForkRecovery] P2P peer {} returned unexpected response for snapshot",
                            peer_id_short
                        );
                    }
                    Err(e) => {
                        debug!(
                            "[ForkRecovery] Failed to reach p2p peer {}: {}",
                            peer_id_short, e
                        );
                    }
                }
            }
        }

        Err(anyhow::anyhow!("No peer had a valid snapshot for recovery"))
    }

    /// Record a fork failure (previous_hash mismatch) and potentially trigger recovery
    fn record_fork_failure(&self) -> bool {
        let failures = self
            .consecutive_fork_failures
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;

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
        self.consecutive_fork_failures
            .store(0, std::sync::atomic::Ordering::SeqCst);
    }

    fn should_use_single_validator(quorum_reached: bool, mainnet_mode: bool) -> bool {
        !quorum_reached && !mainnet_mode
    }

    async fn create_state_snapshot(&mut self) -> Result<()> {
        if self.pending_qcc.is_some() {
            let resolved = self.check_pending_qcc().await?;
            if !resolved {
                if let Some(ref gossip) = self.gossip_service {
                    gossip.drain_checkpoint_buffer().await;
                }
                return Ok(());
            }
        }

        let (height, previous_hash, mut local_checkpoint_height) = {
            let state = self.state.inner.read().await;
            let current_height = state.checkpoints.last().map(|c| c.height).unwrap_or(0);
            let height = current_height + 1;
            let previous_hash = state.checkpoints.last().map(|c| c.hash.clone());
            (height, previous_hash, current_height)
        };

        if self.last_own_proposal_time_ms > 0 {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let elapsed = now_ms.saturating_sub(self.last_own_proposal_time_ms);
            if elapsed < MIN_INTER_CHECKPOINT_MS {
                debug!(
                    "Inter-checkpoint cooldown: {}ms since last own proposal (min {}ms) — skipping height {}",
                    elapsed, MIN_INTER_CHECKPOINT_MS, height
                );
                return Ok(());
            }
        }

        #[cfg(feature = "p2p")]
        {
            if let Some(ref gossip) = self.gossip_service {
                gossip.drain_checkpoint_buffer().await;
                gossip.drain_push_retries().await;
                let buffered_height = self.state.get_checkpoint_height();
                if buffered_height > local_checkpoint_height {
                    self.last_delta_sync_catch_up = Some(std::time::Instant::now());
                    info!(
                        "Proactive buffer drain advanced height {} -> {} before behind-peers check",
                        local_checkpoint_height, buffered_height
                    );
                    local_checkpoint_height = buffered_height;
                }
            }

            if let Some(network_median) = self.get_network_consensus_height().await {
                let mut behind = network_median.saturating_sub(local_checkpoint_height);
                if behind >= 1 {
                    if behind >= 2 {
                        {
                            let mut state = self.state.inner.write().await;
                            let fp_size = state.fast_path_finalized_txs.len();
                            if fp_size > 0 {
                                state.fast_path_finalized_txs.clear();
                                state.fast_path_finalized_order.clear();
                                info!(
                                    "BEHIND PEERS: flushed {} fast-path finalized entries — checkpoint sync will rebuild",
                                    fp_size
                                );
                            }
                        }

                        let we_are_leader = self.is_snapshot_proposer(height).await;
                        if we_are_leader {
                            self.consecutive_behind_yields += 1;
                            if let Some(ref gossip) = self.gossip_service {
                                let our_stake = {
                                    let state = self.state.inner.read().await;
                                    state
                                        .validators
                                        .get(&self.validator_address)
                                        .map(|v| v.stake)
                                        .unwrap_or(0)
                                };
                                if self.consecutive_behind_yields <= 2 {
                                    info!(
                                        "BEHIND PEERS SELF-YIELD: we are elected leader for height {} but behind by {} (yield {}) — broadcasting leader timeout so backups can skip immediately",
                                        height, behind, self.consecutive_behind_yields
                                    );
                                    gossip
                                        .broadcast_view_change(
                                            height,
                                            1,
                                            &self.validator_address,
                                            our_stake,
                                            rinku_core::types::ViewChangeReason::LeaderBehind,
                                        )
                                        .await;
                                } else {
                                    info!(
                                        "BEHIND PEERS SELF-YIELD: we are elected leader for height {} but behind by {} (yield {}) — suppressing timeout spam, async delta sync in progress",
                                        height, behind, self.consecutive_behind_yields
                                    );
                                }
                            }
                        } else {
                            self.consecutive_behind_yields = 0;
                        }
                    }

                    if let Some(ref gossip) = self.gossip_service {
                        gossip.drain_checkpoint_buffer().await;
                        let new_height = self.state.get_checkpoint_height();
                        if new_height > local_checkpoint_height {
                            self.last_delta_sync_catch_up = Some(std::time::Instant::now());
                            self.consecutive_behind_yields = 0;
                            behind = network_median.saturating_sub(new_height);
                            info!(
                                "BEHIND PEERS: buffer drain advanced {} -> {} (gap closed to {})",
                                local_checkpoint_height, new_height, behind
                            );
                            local_checkpoint_height = new_height;
                            if behind == 0 {
                                self.consecutive_gap_one_ticks = 0;
                                return Ok(());
                            }
                        }
                    }

                    if behind == 1 {
                        self.consecutive_gap_one_ticks += 1;
                        const GAP_ONE_FALLBACK_TICKS: u32 = 3;
                        if self.consecutive_gap_one_ticks >= GAP_ONE_FALLBACK_TICKS {
                            if let Some(ref gossip) = self.gossip_service {
                                let (accumulated, total, has_quorum) =
                                    gossip.get_leader_timeout_info(height).await;
                                if has_quorum {
                                    warn!(
                                        "BEHIND PEERS FALLBACK: gap=1 async fetch failed {} consecutive ticks AND LeaderTimeout quorum exists ({}/{}) for height {} — leader yielded, proceeding to backup production",
                                        self.consecutive_gap_one_ticks, accumulated, total, height
                                    );
                                    self.consecutive_gap_one_ticks = 0;
                                    self.consecutive_behind_yields = 0;
                                    behind = 0;
                                }
                            }
                        }
                    }

                    if behind >= 1 && self.pending_delta_sync.is_none() {
                        let timeout = if behind == 1 { 500 } else { 1500 };
                        info!(
                            "BEHIND PEERS: local height {} vs network best {} (gap={}) — spawning async delta sync (timeout={}ms)",
                            local_checkpoint_height, network_median, behind, timeout
                        );
                        self.spawn_async_delta_fetch(local_checkpoint_height, timeout);
                        return Ok(());
                    } else if behind >= 1 && self.pending_delta_sync.is_some() {
                        let age_ms = self
                            .pending_delta_sync
                            .as_ref()
                            .map(|p| p.spawned_at.elapsed().as_millis())
                            .unwrap_or(0);
                        info!(
                            "BEHIND PEERS: local height {} vs network best {} (gap={}) — async delta sync already in flight (age={}ms)",
                            local_checkpoint_height, network_median, behind, age_ms
                        );
                        return Ok(());
                    }

                    if behind == 0 {
                        self.consecutive_gap_one_ticks = 0;
                    }
                } else {
                    self.consecutive_gap_one_ticks = 0;
                    self.consecutive_behind_yields = 0;
                }
            }
        }

        if height != self.last_seen_height {
            self.last_seen_height = height;
            self.stuck_iterations = 0;
        } else {
            self.stuck_iterations += 1;
        }

        if self.stuck_iterations >= 1 {
            if let Some(ref gossip) = self.gossip_service {
                gossip.drain_checkpoint_buffer().await;
                let new_height = self.state.get_checkpoint_height();
                if new_height > local_checkpoint_height {
                    self.last_delta_sync_catch_up = Some(std::time::Instant::now());
                    info!(
                        "Snapshot sync: buffer drain advanced height from {} to {} — resuming (suppression set)",
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
            } else if unfinalized_count > 0 && !is_proposer_for_reset && self.stuck_iterations >= 2
            {
                if self.pending_delta_sync.is_none() {
                    info!(
                            "Proactive height probe: spawning async delta fetch (not proposer, {} unfinalized txs, stuck {} ticks)",
                            unfinalized_count, self.stuck_iterations
                        );
                    self.spawn_async_delta_fetch(local_checkpoint_height, 500);
                }
            } else if unfinalized_count == 0 {
                if self.stuck_iterations >= 1 && self.pending_delta_sync.is_none() {
                    let timeout = match self.stuck_iterations {
                        1 => 500,
                        2 => 1500,
                        _ => 2500,
                    };
                    info!(
                        "Snapshot sync: spawning async delta fetch at height {} (stuck_iter={}, timeout={}ms)",
                        height, self.stuck_iterations, timeout
                    );
                    self.spawn_async_delta_fetch(local_checkpoint_height, timeout);
                }
                return Ok(());
            }
        }

        if height != self.qcc_failure_height {
            self.consecutive_qcc_failures = 0;
            self.qcc_failure_height = height;
        }

        let is_proposer = self.is_snapshot_proposer(height).await;

        if is_proposer {
            if self.qcc_yielded_height == height {
                const FAST_PATH_YIELD_MAX_MS: u64 = 3000;
                let yield_expired = self
                    .fast_path_yield_start
                    .map(|t| t.elapsed().as_millis() as u64 >= FAST_PATH_YIELD_MAX_MS)
                    .unwrap_or(false);

                if yield_expired {
                    info!(
                        "YIELD-RECOVERY: fast-path yield expired ({}ms) at height {} — clearing yield state and proceeding",
                        self.fast_path_yield_start.map(|t| t.elapsed().as_millis()).unwrap_or(0), height
                    );
                    self.qcc_yielded_height = 0;
                    self.fast_path_yield_start = None;
                    self.consecutive_qcc_failures = 0;
                } else {
                    if let Some(ref gossip) = self.gossip_service {
                        gossip.drain_checkpoint_buffer().await;
                        let new_height = self.state.get_checkpoint_height();
                        if new_height > local_checkpoint_height {
                            info!(
                                "YIELD-RECOVERY: adopted buffered checkpoint, height {} -> {} — clearing yield state",
                                local_checkpoint_height, new_height
                            );
                            self.qcc_yielded_height = 0;
                            self.fast_path_yield_start = None;
                            self.consecutive_qcc_failures = 0;
                            self.last_delta_sync_catch_up = Some(std::time::Instant::now());
                            return Ok(());
                        }
                    }

                    if self.pending_delta_sync.is_none() {
                        info!(
                            "YIELD-RECOVERY: spawning async delta fetch for height {} (yield recovery)",
                            height
                        );
                        self.spawn_async_delta_fetch(local_checkpoint_height, 1500);
                    }
                    {
                        let new_height = self.state.get_checkpoint_height();
                        if new_height > local_checkpoint_height {
                            info!(
                                "YIELD-RECOVERY: height advanced to {} (was stuck at yield height {}) — clearing yield state",
                                new_height, height
                            );
                            self.qcc_yielded_height = 0;
                            self.fast_path_yield_start = None;
                            self.consecutive_qcc_failures = 0;
                            self.last_delta_sync_catch_up = Some(std::time::Instant::now());
                        }
                    }
                    return Ok(());
                }
            }

            if let Some(ref gossip) = self.gossip_service {
                gossip.drain_checkpoint_buffer().await;
                let new_height = self.state.get_checkpoint_height();
                if new_height > local_checkpoint_height {
                    info!(
                        "PROPOSER: adopted buffered checkpoint before creating (height {} -> {})",
                        local_checkpoint_height, new_height
                    );
                    self.last_delta_sync_catch_up = Some(std::time::Instant::now());
                    self.consecutive_qcc_failures = 0;
                    self.qcc_yielded_height = 0;
                    self.fast_path_yield_start = None;
                    return Ok(());
                }
                if gossip.has_buffered_checkpoint(height).await {
                    info!(
                        "PROPOSER: buffered checkpoint exists at height {} but couldn't apply yet — deferring creation",
                        height
                    );
                    return Ok(());
                }

                gossip
                    .broadcast_checkpoint_intent(height, &self.validator_address)
                    .await;
            }
            let recently_caught_up = self
                .last_delta_sync_catch_up
                .map(|t| {
                    t.elapsed() <= std::time::Duration::from_millis(LEADER_POST_SYNC_MAX_DEFER_MS)
                })
                .unwrap_or(false);
            if recently_caught_up {
                info!(
                    "PROPOSER: recently caught up ({}ms ago, cooldown {}ms) — deferring height {} (QCC will gate commit)",
                    self.last_delta_sync_catch_up.map(|t| t.elapsed().as_millis()).unwrap_or(0),
                    LEADER_POST_SYNC_MAX_DEFER_MS,
                    height
                );
                return Ok(());
            }

            {
                let fast_path_pool_size = {
                    let state = self.state.inner.read().await;
                    state.fast_path_finalized_txs.len()
                };
                if fast_path_pool_size == 0 {
                    let has_pending_fast_path = if let Some(ref gossip) = self.gossip_service {
                        let stats = gossip.get_fast_path_stats().await;
                        stats.confirmed_count > 50
                    } else {
                        false
                    };
                    let caught_up_recently = self
                        .last_delta_sync_catch_up
                        .map(|t| t.elapsed() <= std::time::Duration::from_secs(2))
                        .unwrap_or(false);
                    if has_pending_fast_path && caught_up_recently {
                        if let Some(ref gossip) = self.gossip_service {
                            let our_stake = {
                                let state = self.state.inner.read().await;
                                state
                                    .validators
                                    .get(&self.validator_address)
                                    .map(|v| v.stake)
                                    .unwrap_or(0)
                            };
                            info!(
                                "PROPOSER FAST-PATH-YIELD: fast_path_pool=0 but pending_fast_path={} caught_up_recently={} — self-yielding height {} to avoid QCC mismatch",
                                has_pending_fast_path, caught_up_recently, height
                            );
                            gossip
                                .broadcast_view_change(
                                    height,
                                    1,
                                    &self.validator_address,
                                    our_stake,
                                    rinku_core::types::ViewChangeReason::LeaderTimeout,
                                )
                                .await;
                            self.qcc_yielded_height = height;
                            self.fast_path_yield_start = Some(std::time::Instant::now());
                        }
                        return Ok(());
                    }
                }
            }

            #[cfg(feature = "p2p")]
            {
                if let Some(network_median) = self.get_network_consensus_height().await {
                    if network_median > local_checkpoint_height {
                        let peer_gap = network_median.saturating_sub(local_checkpoint_height);
                        if peer_gap >= 1 {
                            if let Some(ref gossip) = self.gossip_service {
                                let our_stake = {
                                    let state = self.state.inner.read().await;
                                    state
                                        .validators
                                        .get(&self.validator_address)
                                        .map(|v| v.stake)
                                        .unwrap_or(0)
                                };
                                info!(
                                    "PROPOSER PRE-CHECK SELF-YIELD: behind by {} (local={}, network={}) — broadcasting leader timeout instead of slow sync",
                                    peer_gap, local_checkpoint_height, network_median
                                );
                                gossip
                                    .broadcast_view_change(
                                        height,
                                        1,
                                        &self.validator_address,
                                        our_stake,
                                        rinku_core::types::ViewChangeReason::LeaderBehind,
                                    )
                                    .await;
                            }
                            return Ok(());
                        }
                    }
                }
            }
        }

        if !is_proposer {
            if let Some(ref gossip) = self.gossip_service {
                gossip.drain_checkpoint_buffer().await;
                let new_height = self.state.get_checkpoint_height();
                if new_height > local_checkpoint_height {
                    self.last_delta_sync_catch_up = Some(std::time::Instant::now());
                    self.last_checkpoint_applied_at = Some(std::time::Instant::now());
                    self.leader_wait_ticks = 0;
                    self.leader_wait_height = 0;
                    return Ok(());
                }
            }

            if self.stuck_iterations >= 1 {
                if self.pending_delta_sync.is_none() {
                    let timeout = match self.stuck_iterations {
                        1 => 500,
                        2 => 1500,
                        _ => 2500,
                    };
                    info!(
                        "Non-proposer: spawning async delta fetch at height {} (stuck_iter={}, timeout={}ms)",
                        height, self.stuck_iterations, timeout
                    );
                    self.spawn_async_delta_fetch(local_checkpoint_height, timeout);
                }
                let new_height = self.state.get_checkpoint_height();
                if new_height > local_checkpoint_height {
                    self.last_delta_sync_catch_up = Some(std::time::Instant::now());
                    self.last_checkpoint_applied_at = Some(std::time::Instant::now());
                    info!(
                        "Non-proposer recovered checkpoint at height {} from async delta sync (stuck_iter={})",
                        new_height, self.stuck_iterations
                    );
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
                    state
                        .checkpoints
                        .last()
                        .map(|c| c.hash.clone())
                        .unwrap_or_else(|| "genesis".to_string())
                };
                let validator_addresses_with_stakes: Vec<(String, u64)> =
                    if let Some(ref identity) = self.validator_identity {
                        let identity_guard = identity.read().await;
                        identity_guard
                            .active_validators()
                            .iter()
                            .map(|(addr, v)| (addr.clone(), v.effective_stake))
                            .collect()
                    } else {
                        vec![(self.validator_address.clone(), 1)]
                    };
                leader_election
                    .get_backup_rank_from_validators(
                        height,
                        &prev_hash,
                        &validator_addresses_with_stakes,
                        &self.validator_address,
                    )
                    .unwrap_or(0)
            } else {
                0
            };

            let vote_threshold = LEADER_SKIP_BASE_TICKS;
            let has_valid_intent = if let Some(ref gossip) = self.gossip_service {
                gossip
                    .has_valid_leader_intent(height, self.interval_ms)
                    .await
            } else {
                false
            };
            let effective_vote_threshold = if has_valid_intent {
                if self.leader_wait_ticks == vote_threshold {
                    info!(
                        "Leader intent active for height {} — extending timeout vote from {} to {} ticks",
                        height, vote_threshold, vote_threshold + LEADER_INTENT_EXTENSION_TICKS
                    );
                }
                vote_threshold + LEADER_INTENT_EXTENSION_TICKS
            } else {
                vote_threshold
            };

            if self.leader_wait_ticks >= effective_vote_threshold {
                if let Some(catch_up_time) = self.last_delta_sync_catch_up {
                    let suppression_window =
                        std::time::Duration::from_millis(POST_SYNC_COOLDOWN_MS);
                    if catch_up_time.elapsed() <= suppression_window {
                        return Ok(());
                    }
                }

                if let Some(ref gossip) = self.gossip_service {
                    gossip.drain_checkpoint_buffer().await;
                    let new_height = self.state.get_checkpoint_height();
                    if new_height > local_checkpoint_height {
                        self.leader_wait_ticks = 0;
                        self.leader_wait_height = 0;
                        return Ok(());
                    }

                    let our_stake = {
                        let state = self.state.inner.read().await;
                        state
                            .validators
                            .get(&self.validator_address)
                            .map(|v| v.stake)
                            .unwrap_or(0)
                    };
                    gossip
                        .broadcast_view_change(
                            height,
                            1,
                            &self.validator_address,
                            our_stake,
                            rinku_core::types::ViewChangeReason::LeaderTimeout,
                        )
                        .await;
                    gossip.clear_vote_lock_for_height(height).await;

                    let production_threshold =
                        effective_vote_threshold + (backup_rank * LEADER_SKIP_STAGGER_TICKS);
                    if self.leader_wait_ticks < production_threshold {
                        return Ok(());
                    }

                    let (accumulated, total, has_quorum) =
                        gossip.get_leader_timeout_info(height).await;
                    if !has_quorum {
                        if self.leader_wait_ticks == production_threshold {
                            info!(
                                "CONSENSUS SKIP: waiting for timeout quorum at height {} (accumulated={}/{}, need >2/3, {} ticks)",
                                height, accumulated, total, self.leader_wait_ticks
                            );
                        }
                        return Ok(());
                    }

                    gossip.drain_checkpoint_buffer().await;
                    let new_height = self.state.get_checkpoint_height();
                    if new_height > local_checkpoint_height {
                        info!(
                            "CONSENSUS SKIP aborted: checkpoint arrived at height {} while preparing skip for height {}",
                            new_height, height
                        );
                        self.leader_wait_ticks = 0;
                        self.leader_wait_height = 0;
                        return Ok(());
                    }

                    if self.pending_delta_sync.is_none() {
                        info!(
                            "CONSENSUS SKIP: spawning async delta fetch at height {} before skip attempt",
                            height
                        );
                        self.spawn_async_delta_fetch(local_checkpoint_height, 1500);
                    }
                    let new_height = self.state.get_checkpoint_height();
                    if new_height > local_checkpoint_height {
                        self.last_delta_sync_catch_up = Some(std::time::Instant::now());
                        info!(
                            "CONSENSUS SKIP aborted: recovered leader's checkpoint at height {} from async delta sync",
                            new_height
                        );
                        self.leader_wait_ticks = 0;
                        self.leader_wait_height = 0;
                        return Ok(());
                    }

                    info!(
                        "CONSENSUS LEADER SKIP: Timeout quorum reached for height {} — {}/{} stake voted timeout after {} ticks (~{}s) (backup rank {}, {} unfinalized txs)",
                        height, accumulated, total,
                        self.leader_wait_ticks, self.leader_wait_ticks as u64 * self.interval_ms / 1000,
                        backup_rank, unfinalized_count
                    );
                    self.leader_wait_ticks = 0;
                    self.leader_wait_height = 0;
                } else {
                    return Ok(());
                }
            } else {
                return Ok(());
            }
        } else {
            self.leader_wait_ticks = 0;
            self.leader_wait_height = 0;
        }

        if let Some(retry) = self.qcc_retry_data.take() {
            if retry.height == height {
                warn!(
                    "QCC-RETRY: Re-using cached checkpoint for height {} (hash={}) — idempotent retry to prevent equivocation",
                    height, &retry.checkpoint.hash[..16.min(retry.checkpoint.hash.len())]
                );

                #[cfg(feature = "p2p")]
                {
                    if let Some(ref gossip) = self.gossip_service {
                        match gossip
                            .try_lock_proposer_vote(height, &retry.checkpoint.hash)
                            .await
                        {
                            Ok(()) => {}
                            Err(reason) => {
                                warn!(
                                    "QCC-RETRY EQUIVOCATION GUARD: Aborting retry for height {} — {}",
                                    height, reason
                                );
                                return Ok(());
                            }
                        }
                    }
                }

                {
                    let mut emission = self.state.emission.write().await;
                    let reward = emission.get_checkpoint_reward(height);
                    let is_new = emission.record_emission_for_height(height, reward);
                    if is_new {
                        let mut rewards = self.state.rewards.write().await;
                        rewards.distribute_checkpoint_rewards(reward);
                    }
                }

                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;

                #[cfg(feature = "p2p")]
                {
                    let gossip_for_qcc = self.gossip_service.clone();
                    let network_for_qcc = self.network_handle.clone();
                    let identity_for_qcc = self.validator_identity.clone();
                    let addr_for_qcc = self.validator_address.clone();
                    let cp_height_cache = self.state.checkpoint_height_cache.clone();
                    let cp_for_qcc = retry.checkpoint.clone();
                    let hashes_for_qcc = retry.hashes.clone();

                    info!(
                        "QCC-RETRY: Spawning vote collection for cached checkpoint {} at height {} ({} txs)",
                        &retry.checkpoint.hash[..16.min(retry.checkpoint.hash.len())],
                        height,
                        retry.hashes.len()
                    );

                    let t_retry = std::time::Instant::now();
                    let qcc_handle = tokio::spawn(async move {
                        collect_qcc_standalone(
                            cp_for_qcc,
                            &hashes_for_qcc,
                            gossip_for_qcc,
                            network_for_qcc,
                            identity_for_qcc,
                            &addr_for_qcc,
                            cp_height_cache,
                        )
                        .await
                    });

                    self.pending_qcc = Some(PendingQcc {
                        checkpoint: retry.checkpoint,
                        hashes: retry.hashes,
                        txs_to_execute: retry.txs_to_execute,
                        reward_distributions: retry.reward_distributions,
                        checkpoint_reward: retry.checkpoint_reward,
                        finalized_proofs: retry.finalized_proofs,
                        fast_path_executed: retry.fast_path_executed,
                        height: retry.height,
                        now_ms,
                        is_partitioned: retry.is_partitioned,
                        partition_info: retry.partition_info,
                        finality_sum: 0,
                        finality_count: 0,
                        finality_max: 0,
                        finality_times: Vec::new(),
                        t_overall: t_retry,
                        t_gather_ms: 0,
                        t_proof_ms: 0,
                        t_weight_ms: 0,
                        qcc_handle,
                        qcc_spawned_at: std::time::Instant::now(),
                    });
                }

                return Ok(());
            } else {
                warn!(
                    "QCC-RETRY: Discarding stale retry data for height {} (current height {})",
                    retry.height, height
                );
            }
        }

        self.state.purge_zombie_dag_txs().await;

        let t_overall = std::time::Instant::now();

        let (mut hashes, txs, _initial_merkle_root) = self
            .gather_unfinalized_txs(height, true, &previous_hash)
            .await?;
        let t_gather_ms = t_overall.elapsed().as_millis();

        if hashes.is_empty() {
            return Ok(());
        }

        if let Some(ref gossip) = self.gossip_service {
            gossip
                .broadcast_checkpoint_intent(height, &self.validator_address)
                .await;
        }

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

        {
            let state_guard = self.state.inner.read().await;
            let mut any_executable = false;
            for tx in &txs {
                if matches!(tx.tx.kind, Some(rinku_core::TransactionKind::Consolidation)) {
                    any_executable = true;
                    break;
                }
                if let Some(account) = state_guard.accounts.get(&tx.tx.from) {
                    let fp_effective_nonce = state_guard.fast_path_finalized_txs.values()
                        .filter(|e| e.from == tx.tx.from)
                        .map(|e| e.nonce + 1)
                        .max()
                        .unwrap_or(account.nonce);
                    let effective_nonce = fp_effective_nonce.max(account.nonce);
                    if tx.tx.nonce <= effective_nonce {
                        any_executable = true;
                        break;
                    }
                } else {
                    any_executable = true;
                    break;
                }
            }
            if !any_executable {
                tracing::warn!(
                    "Checkpoint h={}: pre-check found 0/{} executable TXs (all nonce gaps) — skipping proof generation",
                    height, txs.len()
                );
                return Ok(());
            }
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();

        let (checkpoint_reward, reward_distributions) = {
            let mut emission = self.state.emission.write().await;
            let reward = emission.get_checkpoint_reward(height);
            let is_new = emission.record_emission_for_height(height, reward);

            let dists = if is_new {
                let mut rewards = self.state.rewards.write().await;
                let distributions = rewards.distribute_checkpoint_rewards(reward);
                if !distributions.is_empty() {
                    info!(
                        "Pre-distributed {:.6} RKU to {} validators before state root computation",
                        rinku_core::types::from_micro_units(reward),
                        distributions.len()
                    );
                }
                distributions
            } else {
                vec![]
            };

            (reward, dists)
        };

        let (affected_vec, fast_path_executed, contract_lane_txs) = {
            let state_guard = self.state.inner.read().await;
            let mut affected: std::collections::HashSet<String> = std::collections::HashSet::new();
            for tx in &txs {
                affected.insert(tx.tx.from.clone());
                if !tx.tx.to.is_empty() {
                    affected.insert(tx.tx.to.clone());
                }
            }
            if let Some(ref v) = state_guard.node_validator_address {
                affected.insert(v.clone());
            }
            for entry in state_guard.fast_path_finalized_txs.values() {
                affected.insert(entry.from.clone());
                if !entry.to.is_empty() {
                    affected.insert(entry.to.clone());
                }
            }
            let accounts: Vec<String> = affected.into_iter().collect();
            let hash_set: std::collections::HashSet<String> = state_guard
                .fast_path_finalized_txs
                .keys()
                .cloned()
                .collect();
            let cl_txs: Vec<rinku_core::SignedTransaction> = txs.iter()
                .filter(|tx| !hash_set.contains(&tx.hash))
                .cloned()
                .collect();
            (accounts, hash_set, cl_txs)
        };

        let fp_count = fast_path_executed.len();
        let cl_count = contract_lane_txs.len();

        let t_proof_start = std::time::Instant::now();
        let proofs_result = self
            .state
            .compute_state_root_and_proofs_at_height(&contract_lane_txs, &affected_vec, height)
            .await;
        let t_proof_ms = t_proof_start.elapsed().as_millis();
        let state_root = proofs_result.state_root.clone();
        let finalized_proofs = proofs_result.proofs;

        let pre_filter_count = hashes.len();
        hashes.retain(|h| proofs_result.executed_tx_hashes.contains(h) || fast_path_executed.contains(h));
        let filtered_count = pre_filter_count - hashes.len();
        if filtered_count > 0 {
            tracing::warn!(
                "Checkpoint h={}: filtered {} non-executable TXs from finalized list ({} -> {} TXs, {} fast-path, {} contract-lane)",
                height, filtered_count, pre_filter_count, hashes.len(), fp_count, cl_count
            );
        }

        if hashes.is_empty() {
            tracing::warn!(
                "Checkpoint h={}: all {} TXs were non-executable — skipping checkpoint",
                height,
                pre_filter_count
            );
            return Ok(());
        }

        hashes.sort();
        let merkle_root = {
            let hashes_clone = hashes.clone();
            let tree =
                tokio::task::spawn_blocking(move || MerkleTree::from_hex_leaves(&hashes_clone))
                    .await
                    .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {}", e))??;
            tree.root()
        };

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

        let my_stake = self
            .state
            .get_validator_stake(&self.validator_address)
            .await
            .unwrap_or(0);
        let proposer_sig = ValidatorSignature {
            validator: self.validator_address.clone(),
            signature: URL_SAFE_NO_PAD.encode(&signature),
            weight: my_stake,
            bls_public_key: Some(self.bls_public_key_base64()),
        };

        let proposer_bitmap = if let Some(ref identity) = self.validator_identity {
            let identity_guard = identity.read().await;
            let mut sorted_addrs: Vec<&String> = identity_guard
                .active_validators()
                .iter()
                .filter(|(_, v)| !v.bls_public_key.is_empty())
                .map(|(addr, _)| addr)
                .collect();
            sorted_addrs.sort();
            let total_validators = sorted_addrs.len();
            if let Some(my_index) = sorted_addrs
                .iter()
                .position(|a| **a == self.validator_address)
            {
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
        let is_partitioned =
            partition_info.status == crate::state::partition::PartitionStatus::Partitioned;

        let t_weight_start = std::time::Instant::now();
        let weight_trie_root =
            {
                const WEIGHT_RETENTION_CHECKPOINTS: u64 = 20;
                let (all_stakes, total_network_stake, tx_checkpoints): (
                    std::collections::HashMap<String, u64>,
                    u64,
                    Option<(u64, std::collections::HashMap<String, u64>)>,
                ) = {
                    let state = self.state.inner.read().await;
                    let mut stakes: std::collections::HashMap<String, u64> =
                        std::collections::HashMap::new();
                    for (addr, v) in state.validators.iter() {
                        if v.stake > 0 {
                            stakes.insert(addr.clone(), v.stake);
                        }
                    }
                    for (addr, account) in state.accounts.iter() {
                        if account.staked > 0 {
                            let stake_micro = account.staked;
                            stakes
                                .entry(addr.clone())
                                .and_modify(|s| *s = (*s).max(stake_micro))
                                .or_insert(stake_micro);
                        }
                    }
                    let total: u64 = stakes.values().sum();
                    let tx_cp = if height > WEIGHT_RETENTION_CHECKPOINTS {
                        let min_cp = height - WEIGHT_RETENTION_CHECKPOINTS;
                        let mut map: std::collections::HashMap<String, u64> =
                            std::collections::HashMap::new();
                        for cp in &state.checkpoints {
                            if cp.height >= min_cp {
                                for h in &cp.finalized_tx_hashes {
                                    map.insert(h.clone(), cp.height);
                                }
                            }
                        }
                        for node in state.dag.get_unfinalized_nodes() {
                            map.entry(node.hash.clone()).or_insert(height);
                        }
                        Some((min_cp, map))
                    } else {
                        None
                    };
                    (stakes, total, tx_cp)
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

                    if let Some((min_cp, ref tx_cp_map)) = tx_checkpoints {
                        let before_count = weight_trie.all_weights().len();
                        weight_trie.prune_before_checkpoint(min_cp, tx_cp_map);
                        let after_count = weight_trie.all_weights().len();
                        if before_count > after_count {
                            info!(
                                "Weight trie pruned: {} -> {} entries (retention >= checkpoint {})",
                                before_count, after_count, min_cp
                            );
                        }
                    }

                    weight_trie.compute_root()
                } else {
                    String::new()
                }
            };
        let t_weight_ms = t_weight_start.elapsed().as_millis();

        let (vc_cert, vc_view) = if !is_proposer {
            if let Some(ref gossip) = self.gossip_service {
                let view = gossip.get_current_view(height).await.max(1);
                let cert = gossip.get_view_change_certificate(height, view).await;
                if cert.is_some() {
                    info!(
                        "Backup checkpoint h={} includes ViewChangeCertificate for view {}",
                        height, view
                    );
                }
                (cert, view)
            } else {
                (None, 0)
            }
        } else {
            (None, 0)
        };

        let mut checkpoint = Checkpoint {
            height,
            hash: hex::encode(&checkpoint_hash_bytes),
            previous_hash,
            tx_merkle_root: merkle_root,
            state_root,
            receipt_root: "0".repeat(64),
            tip_count,
            timestamp,
            validator_signatures: vec![proposer_sig],
            aggregated_signature: if proposer_bitmap.is_some() {
                Some(URL_SAFE_NO_PAD.encode(&signature))
            } else {
                None
            },
            signer_bitmap: proposer_bitmap,
            finalized_tx_hashes: hashes.clone(),
            weight_trie_root,
            provisional: is_partitioned,
            partition_epoch: if is_partitioned {
                partition_info.current_epoch
            } else {
                None
            },
            visible_stake_pct: if is_partitioned {
                Some(partition_info.visible_stake_pct)
            } else {
                None
            },
            merge_report_hash: None,
            view_change_certificate: vc_cert,
            view: vc_view,
        };

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut txs_to_execute: Vec<SignedTransaction> = {
            let state = self.state.inner.read().await;
            hashes
                .iter()
                .filter_map(|hash| {
                    state.dag.get_node(hash).and_then(|node| {
                        if node.finalized {
                            None
                        } else {
                            Some(node.tx.clone())
                        }
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

        #[cfg(feature = "p2p")]
        {
            if let Some(ref gossip) = self.gossip_service {
                match gossip
                    .try_lock_proposer_vote(height, &checkpoint.hash)
                    .await
                {
                    Ok(()) => {}
                    Err(reason) => {
                        warn!(
                            "EQUIVOCATION GUARD: Aborting checkpoint {} at height {} — {} (we already voted for a competing proposal)",
                            &checkpoint.hash[..16.min(checkpoint.hash.len())],
                            height,
                            reason
                        );
                        return Ok(());
                    }
                }
            }

            let gossip_for_qcc = self.gossip_service.clone();
            let network_for_qcc = self.network_handle.clone();
            let identity_for_qcc = self.validator_identity.clone();
            let addr_for_qcc = self.validator_address.clone();
            let cp_height_cache = self.state.checkpoint_height_cache.clone();
            let cp_for_qcc = checkpoint.clone();
            let hashes_for_qcc = hashes.clone();

            info!(
                "QCC-PIPELINE: Spawning vote collection for checkpoint {} at height {} ({} txs) — tick loop continues",
                &checkpoint.hash[..16.min(checkpoint.hash.len())],
                height,
                hashes.len()
            );

            let qcc_handle = tokio::spawn(async move {
                collect_qcc_standalone(
                    cp_for_qcc,
                    &hashes_for_qcc,
                    gossip_for_qcc,
                    network_for_qcc,
                    identity_for_qcc,
                    &addr_for_qcc,
                    cp_height_cache,
                )
                .await
            });

            self.pending_qcc = Some(PendingQcc {
                checkpoint,
                hashes,
                txs_to_execute,
                reward_distributions,
                checkpoint_reward,
                finalized_proofs,
                fast_path_executed,
                height,
                now_ms,
                is_partitioned,
                partition_info,
                finality_sum,
                finality_count,
                finality_max,
                finality_times,
                t_overall,
                t_gather_ms,
                t_proof_ms,
                t_weight_ms,
                qcc_handle,
                qcc_spawned_at: std::time::Instant::now(),
            });
        }

        #[cfg(not(feature = "p2p"))]
        {
            self.apply_certified_checkpoint(
                checkpoint,
                hashes,
                txs_to_execute,
                reward_distributions,
                checkpoint_reward,
                finalized_proofs,
                fast_path_executed,
                height,
                now_ms,
                is_partitioned,
                partition_info,
                finality_sum,
                finality_count,
                finality_max,
                finality_times,
                t_overall,
                t_gather_ms,
                t_proof_ms,
                t_weight_ms,
                0,
                0,
            )
            .await?;
        }

        Ok(())
    }

    async fn is_snapshot_proposer(&self, height: u64) -> bool {
        if let Some(ref leader_election) = self.leader_election {
            let prev_hash = {
                let state = self.state.inner.read().await;
                state
                    .checkpoints
                    .last()
                    .map(|c| c.hash.clone())
                    .unwrap_or_else(|| "genesis".to_string())
            };

            let validator_addresses_with_stakes: Vec<(String, u64)> =
                if let Some(ref identity) = self.validator_identity {
                    let identity_guard = identity.read().await;
                    identity_guard
                        .active_validators()
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

            let mut zombie_count = 0usize;
            let mut unfinalized_nodes: Vec<_> = all_unfinalized
                .iter()
                .filter(|n| n.tx.tx.timestamp <= cutoff_time)
                .filter(|n| Self::is_valid_hex_hash(&n.hash))
                .filter(|n| {
                    let account_nonce = state
                        .accounts
                        .get(&n.tx.tx.from)
                        .map(|a| a.nonce)
                        .unwrap_or(0);
                    if n.tx.tx.nonce < account_nonce
                        && !state.fast_path_finalized_txs.contains_key(&n.hash)
                    {
                        zombie_count += 1;
                        false
                    } else {
                        true
                    }
                })
                .collect();

            if zombie_count > 0 {
                tracing::info!(
                    "Gather: filtered {} zombie txs (stale nonce, not fast-path-tracked)",
                    zombie_count
                );
            }

            let too_new = total - unfinalized_nodes.len() - zombie_count;
            let eligible = unfinalized_nodes.len();

            unfinalized_nodes.sort_by(|a, b| {
                a.tx.tx
                    .from
                    .cmp(&b.tx.tx.from)
                    .then_with(|| a.tx.tx.nonce.cmp(&b.tx.tx.nonce))
                    .then_with(|| a.hash.cmp(&b.hash))
            });

            if eligible > tx_cap {
                use std::collections::BTreeMap as GatherMap;

                let mut sender_groups: GatherMap<&str, Vec<usize>> = GatherMap::new();
                for (i, n) in unfinalized_nodes.iter().enumerate() {
                    sender_groups.entry(&n.tx.tx.from).or_default().push(i);
                }

                let mut sender_chains: Vec<(&str, Vec<usize>, u64)> = sender_groups
                    .iter()
                    .map(|(sender, indices)| {
                        let mut contiguous: Vec<usize> = Vec::new();
                        if let Some(&first_idx) = indices.first() {
                            let mut expected_nonce = unfinalized_nodes[first_idx].tx.tx.nonce;
                            for &i in indices {
                                let tx_nonce = unfinalized_nodes[i].tx.tx.nonce;
                                if tx_nonce == expected_nonce {
                                    contiguous.push(i);
                                    expected_nonce += 1;
                                } else {
                                    break;
                                }
                            }
                        }
                        let best_gas = contiguous
                            .iter()
                            .map(|&i| unfinalized_nodes[i].tx.tx.gas_price.unwrap_or(0))
                            .max()
                            .unwrap_or(0);
                        (*sender, contiguous, best_gas)
                    })
                    .collect();
                sender_chains.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));

                let mut selected_indices: Vec<usize> = Vec::with_capacity(tx_cap);
                for (_, chain, _) in &sender_chains {
                    if selected_indices.len() >= tx_cap {
                        break;
                    }
                    let remaining = tx_cap - selected_indices.len();
                    let take = chain.len().min(remaining);
                    selected_indices.extend_from_slice(&chain[..take]);
                }

                selected_indices.sort();
                let selected_set: std::collections::HashSet<usize> =
                    selected_indices.iter().cloned().collect();
                let mut keep_idx = 0;
                unfinalized_nodes.retain(|_| {
                    let keep = selected_set.contains(&keep_idx);
                    keep_idx += 1;
                    keep
                });
            }

            let hashes: Vec<String> = unfinalized_nodes.iter().map(|n| n.hash.clone()).collect();

            let txs: Vec<SignedTransaction> =
                unfinalized_nodes.iter().map(|n| n.tx.clone()).collect();

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

        if !is_leader && unfinalized.is_empty() {
            return Ok((vec![], vec![], String::new()));
        }

        let needs_peer_sync = is_leader && eligible_count < tx_cap / 2;
        if needs_peer_sync {
            #[cfg(feature = "p2p")]
            {
                if let Some(ref network_handle) = self.network_handle {
                    info!(
                        "Leader mempool sync for checkpoint {} — local eligible: {} txs (below half of cap {})",
                        height, eligible_count, tx_cap
                    );
                    let peers = network_handle.get_connected_peers().await;
                    let local_cp_height = self.state.get_checkpoint_height();

                    let mut peer_futures = Vec::new();
                    for peer in &peers {
                        let peer_id = peer.peer_id.clone();
                        let nh = network_handle.clone();
                        peer_futures.push(async move {
                            let result = tokio::time::timeout(
                                std::time::Duration::from_millis(400),
                                nh.request_delta(&peer_id, local_cp_height),
                            )
                            .await;
                            (peer_id, result)
                        });
                    }

                    let results = futures::future::join_all(peer_futures).await;
                    let mut total_ingested = 0u64;

                    for (peer_id, result) in results {
                        let result = match result {
                            Ok(r) => r,
                            Err(_) => {
                                debug!(
                                    "Pre-checkpoint sync to peer {} timed out",
                                    &peer_id[..16.min(peer_id.len())]
                                );
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
                            .filter(|n| {
                                let account_nonce = state
                                    .accounts
                                    .get(&n.tx.tx.from)
                                    .map(|a| a.nonce)
                                    .unwrap_or(0);
                                n.tx.tx.nonce >= account_nonce
                                    || state.fast_path_finalized_txs.contains_key(&n.hash)
                            })
                            .collect();

                        if !eligible.is_empty() {
                            eligible.sort_by(|a, b| {
                                a.tx.tx
                                    .from
                                    .cmp(&b.tx.tx.from)
                                    .then_with(|| a.tx.tx.nonce.cmp(&b.tx.tx.nonce))
                                    .then_with(|| a.hash.cmp(&b.hash))
                            });

                            if eligible.len() > tx_cap {
                                use std::collections::BTreeMap as SyncGatherMap;

                                let mut sender_groups: SyncGatherMap<&str, Vec<usize>> =
                                    SyncGatherMap::new();
                                for (i, n) in eligible.iter().enumerate() {
                                    sender_groups.entry(&n.tx.tx.from).or_default().push(i);
                                }
                                let mut sender_chains: Vec<(&str, Vec<usize>, u64)> = sender_groups
                                    .iter()
                                    .map(|(sender, indices)| {
                                        let mut contiguous: Vec<usize> = Vec::new();
                                        if let Some(&first_idx) = indices.first() {
                                            let mut expected_nonce =
                                                eligible[first_idx].tx.tx.nonce;
                                            for &i in indices {
                                                if eligible[i].tx.tx.nonce == expected_nonce {
                                                    contiguous.push(i);
                                                    expected_nonce += 1;
                                                } else {
                                                    break;
                                                }
                                            }
                                        }
                                        let best_gas = contiguous
                                            .iter()
                                            .map(|&i| eligible[i].tx.tx.gas_price.unwrap_or(0))
                                            .max()
                                            .unwrap_or(0);
                                        (*sender, contiguous, best_gas)
                                    })
                                    .collect();
                                sender_chains
                                    .sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
                                let mut selected: Vec<usize> = Vec::with_capacity(tx_cap);
                                for (_, chain, _) in &sender_chains {
                                    if selected.len() >= tx_cap {
                                        break;
                                    }
                                    let remaining = tx_cap - selected.len();
                                    let take = chain.len().min(remaining);
                                    selected.extend_from_slice(&chain[..take]);
                                }
                                selected.sort();
                                let selected_set: std::collections::HashSet<usize> =
                                    selected.iter().cloned().collect();
                                let mut idx = 0;
                                eligible.retain(|_| {
                                    let keep = selected_set.contains(&idx);
                                    idx += 1;
                                    keep
                                });
                            }
                            info!(
                                "Peer sync recovered {} eligible txs for checkpoint {} (was {})",
                                eligible.len(),
                                height,
                                unfinalized.len()
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

        if unfinalized.len() > MAX_CHECKPOINT_TXS {
            info!(
                "Checkpoint {} hard-capped: {} eligible txs truncated to {} (remaining deferred to next checkpoint)",
                height, unfinalized.len(), MAX_CHECKPOINT_TXS
            );
            unfinalized.sort();
            unfinalized.truncate(MAX_CHECKPOINT_TXS);
            let capped_set: std::collections::HashSet<&str> =
                unfinalized.iter().map(|s| s.as_str()).collect();
            unfinalized_txs.retain(|tx| capped_set.contains(tx.hash.as_str()));
        }

        unfinalized.sort();

        let tx_merkle_root = if unfinalized.is_empty() {
            "0".repeat(64)
        } else {
            let hashes_clone = unfinalized.clone();
            let tree =
                tokio::task::spawn_blocking(move || MerkleTree::from_hex_leaves(&hashes_clone))
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
