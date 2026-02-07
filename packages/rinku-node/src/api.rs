use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::{info, warn};

use crate::gossip::{GossipMessage, GossipService};
use crate::network::CheckpointData;
use crate::state::TransactionResult;
use crate::sync_verification::build_account_merkle_root_sorted;

static FAUCET_RATE_LIMIT: std::sync::LazyLock<Mutex<HashMap<String, u64>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
const FAUCET_AMOUNT: f64 = 100.0;
const FAUCET_RATE_LIMIT_MS: u64 = 60_000;

/// Limit concurrent sync requests to prevent API overload under sync storms
static ACTIVE_SYNC_REQUESTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
const MAX_CONCURRENT_SYNC_REQUESTS: u32 = 5;

use crate::config::{DEGRADED_MODE_THRESHOLD, MAX_TIPS_BACKPRESSURE};

/// Guard to automatically decrement active sync request count on drop
struct SyncRequestGuard;
impl Drop for SyncRequestGuard {
    fn drop(&mut self) {
        ACTIVE_SYNC_REQUESTS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

use crate::state::NodeState;

#[derive(Clone)]
pub struct ApiState {
    pub node_state: NodeState,
    pub gossip_service: Option<Arc<GossipService>>,
}

#[derive(Serialize)]
struct ApiError {
    error: String,
    code: u16,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = StatusCode::from_u16(self.code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(self)).into_response()
    }
}

impl ApiError {
    fn not_found(message: impl Into<String>) -> Self {
        Self { error: message.into(), code: 404 }
    }
    
    fn bad_request(message: impl Into<String>) -> Self {
        Self { error: message.into(), code: 400 }
    }
}

#[derive(Serialize)]
struct StatsResponse {
    dag_nodes: usize,
    tips: usize,
    accounts: usize,
    checkpoint_height: u64,
    gas_price: f64,
    total_supply: f64,
    validators: usize,
    total_stake: f64,
}

#[derive(Serialize)]
struct TipsResponse {
    tips: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TipUrlsResponse {
    tip_urls: Vec<String>,
}

#[derive(Serialize)]
struct AccountResponse {
    fingerprint: String,
    balance: f64,
    nonce: u64,
    staked: f64,
}

#[derive(Serialize)]
struct AccountTransactionItem {
    hash: String,
    from: String,
    to: String,
    amount: f64,
    timestamp: u64,
    direction: String,
    finalized: bool,
    memo: Option<String>,
    references: Option<Vec<String>>,
    fast_path_status: Option<String>,
    fast_path_confirmed_at_ms: Option<u64>,
    fast_path_finality_ms: Option<u64>,
}

#[derive(Serialize)]
struct AccountTransactionsResponse {
    address: String,
    transactions: Vec<AccountTransactionItem>,
    total: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TxInner {
    from: String,
    to: String,
    amount: f64,
    #[serde(default)]
    fee: f64,
    nonce: u64,
    #[serde(default, alias = "tipUrls")]
    parents: Vec<String>,
    sig: String,
    ts: u64,
    hash: String,
    #[serde(default)]
    kind: Option<rinku_core::types::TransactionKind>,
    #[serde(default)]
    memo: Option<String>,
    #[serde(default)]
    references: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct SubmitTxRequest {
    tx: TxInner,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchTxItem {
    tx: TxInner,
    #[serde(default)]
    public_key: Option<Vec<u8>>,
}

#[derive(Deserialize)]
struct BatchSubmitTxRequest {
    transactions: Vec<BatchTxItem>,
}

#[derive(Serialize)]
struct BatchSubmitTxResponse {
    successful: usize,
    failed: usize,
    total: usize,
}

#[derive(Serialize)]
struct SubmitTxResponse {
    success: bool,
    hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fast_path_eligible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fast_path_status: Option<String>,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
}

#[derive(Deserialize)]
struct FaucetRequest {
    address: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubmitSlashingEvidenceRequest {
    validator: String,
    checkpoint_height: u64,
    hash1: String,
    hash2: String,
    signature1: String,
    #[serde(default)]
    signature2: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SubmitSlashingEvidenceResponse {
    success: bool,
    accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FaucetResponse {
    success: bool,
    amount: f64,
    tx_hash: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DagSummaryResponse {
    total_nodes: usize,
    tip_count: usize,
    checkpoint_height: u64,
    finalized_count: usize,
    tips: Vec<String>,
    merkle_root: String,
    account_count: usize,
}

#[derive(Serialize)]
struct DagResponse {
    nodes: Vec<DagNodeResponse>,
    #[serde(rename = "hasMore")]
    has_more: bool,
}

#[derive(Serialize)]
struct DagNodeResponse {
    hash: String,
    from: String,
    to: String,
    amount: f64,
    fee: f64,
    nonce: u64,
    ts: u64,
    parents: Vec<String>,
    #[serde(rename = "parentCount")]
    parent_count: usize,
    finalized: bool,
    weight: f64,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<rinku_core::types::TransactionKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fast_path_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fast_path_confirmed_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fast_path_finality_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trust_score: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attestation_count: Option<u32>,
}

#[derive(Serialize)]
struct AccountsResponse {
    accounts: Vec<AccountResponse>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapInfoResponse {
    peer_id: Option<String>,
    listen_addr: Option<String>,
    validator_address: Option<String>,
    bls_public_key: Option<String>,
    bootstrap_multiaddr: Option<String>,
    genesis_validator_env: Option<String>,
    genesis_hash: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NetworkStatsResponse {
    tps: f64,
    total_transactions_processed: usize,
    finalized_count: usize,
    unfinalized_count: usize,
    finality_ratio: f64,
    checkpoint_count: u64,
    latest_checkpoint_height: u64,
    latest_checkpoint_id: Option<String>,
    total_staked: f64,
    validator_count: usize,
    network_age: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GasPriceResponse {
    current: f64,
    min: f64,
    max: f64,
    avg_last_100: f64,
    total_burned: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FinalityMetricsResponse {
    avg_time_to_finality: f64,
    median_time_to_finality: f64,
    p95_time_to_finality: f64,
    pending_count: usize,
    finalized_count: usize,
    finality_rate: f64,
    checkpoint_latency: f64,
    checkpoints_per_minute: f64,
    last_checkpoint_age: u64,
    tx_throughput: f64,
    avg_confirmation_ms: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VersionResponse {
    protocol_version: String,
    node_version: String,
    chain_id: String,
    network_id: String,
    features: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SelfProvableTransactionResponse {
    tx_hash: String,
    from: String,
    to: String,
    amount: f64,
    nonce: u64,
    timestamp: u64,
    signature: String,
    parents: Vec<String>,
    finalized: bool,
    checkpoint_height: Option<u64>,
    merkle_proof: Option<Vec<String>>,
    merkle_index: Option<usize>,
    checkpoint: Option<CheckpointProofData>,
    proof_url: Option<String>,
    self_contained_proof_url: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TransactionProofResponse {
    tx_hash: String,
    finalized: bool,
    proof_url: Option<String>,
    proof_size_bytes: Option<usize>,
    qr_viable: Option<bool>,
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionReceipt {
    pub tx_hash: String,
    pub from: String,
    pub to: String,
    pub amount: f64,
    pub fee: f64,
    pub nonce: u64,
    pub timestamp: u64,
    pub status: TransactionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merkle_proof: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merkle_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionStatus {
    Pending,
    Finalized,
    Expired,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountProofResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof: Option<rinku_core::types::AccountStateProof>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    verified: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckpointProofData {
    height: u64,
    hash: String,
    tx_merkle_root: String,
    state_root: String,
    receipt_root: String,
    tip_count: u32,
    timestamp: u64,
    aggregated_signature: Option<String>,
    signer_bitmap: Option<Vec<u8>>,
    validator_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncStatusResponse {
    node_id: String,
    checkpoint_height: u64,
    dag_size: usize,
    tip_count: usize,
    tips: Vec<String>,
    merkle_root: Option<String>,
    total_transactions: u64,
    validators: usize,
    total_stake: f64,
    uptime_seconds: u64,
    is_syncing: bool,
    faucet_balance: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeStatusResponse {
    node_id: String,
    validator_address: Option<String>,
    bls_public_key: Option<String>,
    checkpoint_height: u64,
    dag_size: usize,
    tip_count: usize,
    total_transactions: u64,
    validators: usize,
    total_stake: f64,
    uptime_seconds: u64,
    version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapRequest {
    from_checkpoint: Option<u64>,
    limit: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapResponse {
    transactions: Vec<rinku_core::types::SignedTransaction>,
    checkpoint_height: u64,
    total_available: usize,
    has_more: bool,
}

#[derive(Deserialize)]
struct BatchTxQuery {
    #[serde(default)]
    hashes: String,
}

#[derive(Deserialize)]
struct DagPageQuery {
    #[serde(default)]
    page: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct SyncTxQuery {
    #[serde(default)]
    from_checkpoint: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncDeltaRequest {
    #[serde(default)]
    from_checkpoint: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    validators: std::collections::HashMap<String, rinku_core::types::Validator>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncDeltaResponse {
    transactions: Vec<rinku_core::types::SignedTransaction>,
    account_nonces: std::collections::HashMap<String, u64>,
    account_states: std::collections::HashMap<String, rinku_core::types::Account>,
    total: usize,
    offset: usize,
    limit: usize,
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
    validators: std::collections::HashMap<String, rinku_core::types::Validator>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchTxResponse {
    transactions: Vec<rinku_core::types::SignedTransaction>,
    found: usize,
    missing: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotSyncResponse {
    accounts: std::collections::HashMap<String, rinku_core::types::Account>,
    validators: std::collections::HashMap<String, rinku_core::types::Validator>,
    checkpoints: Vec<rinku_core::types::Checkpoint>,
    gas_price: f64,
    total_supply: f64,
    genesis_time: u64,
    dag_transactions: Vec<rinku_core::types::SignedTransaction>,
    total_transactions: u64,
    checkpoint_height: u64,
    contracts: std::collections::HashMap<String, crate::contracts::ContractState>,
    accounts_merkle_root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    rewards_snapshot: Option<crate::rewards::RewardsSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    emission_snapshot: Option<crate::emission::EmissionSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slashing_snapshot: Option<crate::slashing::SlashingSnapshot>,
    total_burned: f64,
    total_to_validators: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    genesis_hash: Option<String>,
    #[serde(default)]
    finalized_tx_hashes: Vec<String>,
    #[serde(default)]
    tx_checkpoint_heights: std::collections::HashMap<String, u64>,
    #[serde(default)]
    weight_scores: std::collections::HashMap<String, rinku_core::types::AggregatedWeight>,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn get_sync_status(State(state): State<NodeState>) -> Json<SyncStatusResponse> {
    let tips = state.get_tips().await;
    let (dag_size, _, _) = state.get_dag_stats().await;
    let checkpoint_height = state.get_checkpoint_height().await;
    let total_transactions = state.get_total_transactions().await;
    // Use rewards service for accurate staking data
    let rewards = state.rewards.read().await;
    let validators = rewards.get_active_validators().len();
    let total_stake = rewards.get_total_staked();
    drop(rewards);
    let uptime_seconds = state.get_uptime_seconds().await;
    let merkle_root = state.get_dag_merkle_root().await;
    let node_id = std::env::var("NODE_ID").unwrap_or_else(|_| "unknown".to_string());
    let faucet_balance = state.get_faucet_balance().await;

    Json(SyncStatusResponse {
        node_id,
        checkpoint_height,
        dag_size,
        tip_count: tips.len(),
        tips,
        merkle_root,
        total_transactions,
        validators,
        total_stake,
        uptime_seconds,
        is_syncing: false,
        faucet_balance,
    })
}

async fn get_bootstrap_info(State(state): State<NodeState>) -> Json<BootstrapInfoResponse> {
    let (peer_id, listen_addr, validator_address, bls_public_key) = state.get_bootstrap_info().await;
    
    // Build bootstrap multiaddr for P2P_BOOTSTRAP_PEERS env var
    let bootstrap_multiaddr = match (&peer_id, &listen_addr) {
        (Some(pid), Some(addr)) => {
            // Parse listen_addr to extract port, use placeholder for external IP
            let port = addr.split("/tcp/").nth(1).and_then(|s| s.split('/').next()).unwrap_or("4001");
            Some(format!("/ip4/<PUBLIC_IP>/tcp/{}/p2p/{}", port, pid))
        }
        _ => None,
    };
    
    // Build GENESIS_VALIDATORS env var format: address:bls_base64url
    // Only include if both validator address and valid BLS key exist
    let genesis_validator_env = match (&validator_address, &bls_public_key) {
        (Some(addr), Some(bls_key)) if !bls_key.is_empty() => {
            Some(format!("{}:{}", addr, bls_key))
        }
        _ => None,
    };
    
    let genesis_hash = state.get_genesis_hash().await;
    
    Json(BootstrapInfoResponse {
        peer_id,
        listen_addr,
        validator_address,
        bls_public_key,
        bootstrap_multiaddr,
        genesis_validator_env,
        genesis_hash,
    })
}

async fn get_node_status(State(state): State<NodeState>) -> Json<NodeStatusResponse> {
    let tips = state.get_tips().await;
    let (dag_size, _, _) = state.get_dag_stats().await;
    let checkpoint_height = state.get_checkpoint_height().await;
    let total_transactions = state.get_total_transactions().await;
    // Use rewards service for accurate staking data
    let rewards = state.rewards.read().await;
    let validators = rewards.get_active_validators().len();
    let total_stake = rewards.get_total_staked();
    drop(rewards);
    let uptime_seconds = state.get_uptime_seconds().await;
    let node_id = std::env::var("NODE_ID").unwrap_or_else(|_| "unknown".to_string());
    let (validator_address, bls_public_key) = state.get_validator_info().await;

    Json(NodeStatusResponse {
        node_id,
        validator_address,
        bls_public_key,
        checkpoint_height,
        dag_size,
        tip_count: tips.len(),
        total_transactions,
        validators,
        total_stake,
        uptime_seconds,
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn post_gossip(
    State(api_state): State<ApiState>,
    Json(message): Json<GossipMessage>,
) -> impl IntoResponse {
    // Only log at debug level to avoid log spam from frequent gossip rounds
    tracing::debug!("Received gossip message: {:?}", std::mem::discriminant(&message));
    
    // Use GossipService.handle_message if available (for peer discovery)
    if let Some(ref gossip_service) = api_state.gossip_service {
        match gossip_service.handle_message(message.clone()).await {
            Ok(Some(response)) => {
                return Json(serde_json::json!(response)).into_response();
            }
            Ok(None) => {
                // Message handled, continue to return ok
            }
            Err(e) => {
                warn!("Gossip handle_message error: {}", e);
            }
        }
    } else {
        // Fallback: process directly without GossipService
        let state = &api_state.node_state;
        match &message {
            GossipMessage::Transaction { hash, tx, sender_url } => {
                if let Some(url) = sender_url {
                    info!("Gossip: received tx {} from {}", &hash[..16.min(hash.len())], url);
                } else {
                    info!("Gossip: received tx {} from peer", &hash[..16.min(hash.len())]);
                }
                match state.add_transaction(tx.clone()).await {
                    Err(e) => {
                        warn!("Failed to add gossiped transaction: {}", e);
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({ "error": e.to_string() })),
                        ).into_response();
                    }
                    Ok(TransactionResult::Buffered) => {
                        info!("Gossiped transaction {} buffered (future nonce)", &hash[..16.min(hash.len())]);
                    }
                    Ok(TransactionResult::Accepted) => {
                        // Transaction fully processed
                    }
                }
            }
            GossipMessage::TipAnnouncement { dag_size, tips, .. } => {
                info!("Gossip: peer announced {} tips, dag_size={}", tips.len(), dag_size);
            }
            GossipMessage::SyncRequest { from_checkpoint, missing_hashes, .. } => {
                info!("Gossip: sync request from checkpoint {} for {} hashes", 
                    from_checkpoint, missing_hashes.len());
                let txs = state.get_txs_since_checkpoint(*from_checkpoint, missing_hashes).await;
                let checkpoint_height = state.get_checkpoint_height().await;
                info!("Gossip: responding with {} transactions", txs.len());
                return Json(serde_json::json!({
                    "type": "sync_response",
                    "transactions": txs,
                    "checkpoint_height": checkpoint_height
                })).into_response();
            }
            GossipMessage::SyncResponse { transactions, checkpoint_height, .. } => {
                info!("Gossip: received sync response with {} txs at height {}", 
                    transactions.len(), checkpoint_height);
                for tx in transactions {
                    match state.add_transaction(tx.clone()).await {
                        Err(e) => warn!("Failed to add synced tx {}: {}", &tx.hash[..16.min(tx.hash.len())], e),
                        Ok(TransactionResult::Buffered) => info!("Synced tx {} buffered (future nonce)", &tx.hash[..16.min(tx.hash.len())]),
                        Ok(TransactionResult::Accepted) => {}
                    }
                }
            }
            GossipMessage::PeerDiscovery { peers, node_id, .. } => {
                info!("Gossip: peer {} announced {} peers", node_id, peers.len());
            }
            GossipMessage::ConflictResolution { winner_hash, .. } => {
                info!("Gossip: conflict resolution, winner: {}", &winner_hash[..16.min(winner_hash.len())]);
            }
            GossipMessage::CheckpointSignature { checkpoint_id, validator_address, .. } => {
                info!("Gossip: checkpoint sig for {} from validator {}", 
                    &checkpoint_id[..16.min(checkpoint_id.len())], 
                    &validator_address[..16.min(validator_address.len())]);
            }
            GossipMessage::BloomAnnouncement { filter, checkpoint_height, tip_count, .. } => {
                info!("Gossip: bloom filter with {} items, checkpoint height {}, {} tips",
                    filter.item_count(), checkpoint_height, tip_count);
            }
            GossipMessage::SlashingEvidence { evidence, .. } => {
                info!(
                    "Gossip: slashing evidence for {} at height {}",
                    &evidence.validator[..16.min(evidence.validator.len())],
                    evidence.checkpoint_height
                );
                if state.verify_slashing_evidence(&evidence).await {
                    let mut slashing = state.slashing.write().await;
                    let _ = slashing.submit_double_sign_evidence(
                        evidence.validator.clone(),
                        evidence.checkpoint_height,
                        evidence.hash1.clone(),
                        evidence.hash2.clone(),
                        evidence.signature1.clone(),
                        evidence.signature2.clone(),
                    );
                } else {
                    warn!("Gossip: invalid slashing evidence rejected");
                }
            }
            GossipMessage::WeightVote { tx_hash, validator_pubkey, vote, timestamp_ms, bls_signature, .. } => {
                use rinku_core::types::{WeightVote as WV, PendingWeightVote};
                
                info!("Gossip: weight vote for tx {} from {}", 
                    &tx_hash[..16.min(tx_hash.len())],
                    &validator_pubkey[..16.min(validator_pubkey.len())]);
                
                if let Ok(vote_type) = match vote.to_lowercase().as_str() {
                    "boost" => Ok(WV::Boost),
                    "suppress" => Ok(WV::Suppress),
                    "neutral" => Ok(WV::Neutral),
                    _ => Err(()),
                } {
                    let pending_vote = PendingWeightVote {
                        tx_hash: tx_hash.clone(),
                        validator_pubkey: validator_pubkey.clone(),
                        vote: vote_type,
                        timestamp_ms: *timestamp_ms,
                        bls_signature: bls_signature.clone(),
                    };
                    
                    let mut inner = state.inner.write().await;
                    if let Some(ref mut wt) = inner.weight_trie {
                        wt.add_vote(pending_vote);
                    }
                }
            }
            GossipMessage::CheckpointAnnouncement { checkpoint, .. } => {
                let local_height = state.get_checkpoint_height().await;
                let expected_height = local_height + 1;
                
                info!(
                    "Gossip: checkpoint announcement {} at height {} (local: {})",
                    &checkpoint.hash[..16.min(checkpoint.hash.len())],
                    checkpoint.height,
                    local_height
                );
                
                if checkpoint.height == expected_height {
                    // Apply the checkpoint if it's the next expected one
                    if let Err(e) = state.apply_checkpoint(checkpoint.clone()).await {
                        warn!("Failed to apply announced checkpoint: {}", e);
                    }
                } else if checkpoint.height > expected_height {
                    // We're behind - the normal gossip sync will catch us up
                    // Log this for visibility but don't error
                    info!(
                        "Checkpoint {} is ahead (height {} > local {}), sync will catch up",
                        &checkpoint.hash[..16.min(checkpoint.hash.len())],
                        checkpoint.height,
                        local_height
                    );
                }
                // If checkpoint.height < expected_height, it's an old checkpoint we already have - ignore
            }
            GossipMessage::FastPathBroadcast { tx, sender_validator, sender_stake, timestamp_ms, .. } => {
                info!(
                    "Gossip: fast-path broadcast {} from {} (stake: {:.2})",
                    &tx.hash[..16.min(tx.hash.len())],
                    &sender_validator[..16.min(sender_validator.len())],
                    sender_stake
                );
                if tx.is_fast_path_eligible() {
                    match state.add_transaction(tx.clone()).await {
                        Err(e) => warn!("Failed to add fast-path tx: {}", e),
                        Ok(_) => {}
                    }
                }
            }
            GossipMessage::FastPathAck { tx_hash, validator_address, validator_stake, .. } => {
                info!(
                    "Gossip: fast-path ack for {} from {} (stake: {:.2})",
                    &tx_hash[..16.min(tx_hash.len())],
                    &validator_address[..16.min(validator_address.len())],
                    validator_stake
                );
            }
        }
    }

    Json(serde_json::json!({ "status": "ok" })).into_response()
}

async fn post_slashing_evidence(
    State(api_state): State<ApiState>,
    Json(request): Json<SubmitSlashingEvidenceRequest>,
) -> Json<SubmitSlashingEvidenceResponse> {
    let evidence = crate::slashing::DoubleSignEvidence {
        validator: request.validator.clone(),
        checkpoint_height: request.checkpoint_height,
        hash1: request.hash1.clone(),
        hash2: request.hash2.clone(),
        signature1: request.signature1.clone(),
        signature2: request.signature2.clone(),
        timestamp: 0,
        processed: false,
    };
    if !api_state.node_state.verify_slashing_evidence(&evidence).await {
        return Json(SubmitSlashingEvidenceResponse {
            success: false,
            accepted: false,
            error: Some("Invalid evidence signature".to_string()),
        });
    }
    let mut slashing = api_state.node_state.slashing.write().await;
    let evidence = slashing.submit_double_sign_evidence(
        request.validator.clone(),
        request.checkpoint_height,
        request.hash1.clone(),
        request.hash2.clone(),
        request.signature1.clone(),
        request.signature2.clone(),
    );
    drop(slashing);

    if let Some(evidence) = evidence {
        if let Some(ref gossip_service) = api_state.gossip_service {
            gossip_service.broadcast_slashing_evidence(evidence).await;
        }
        Json(SubmitSlashingEvidenceResponse {
            success: true,
            accepted: true,
            error: None,
        })
    } else {
        Json(SubmitSlashingEvidenceResponse {
            success: true,
            accepted: false,
            error: Some("Evidence rejected (duplicate or invalid)".to_string()),
        })
    }
}

async fn post_bootstrap(
    State(state): State<NodeState>,
    Json(req): Json<BootstrapRequest>,
) -> Json<BootstrapResponse> {
    let from_checkpoint = req.from_checkpoint.unwrap_or(0);
    let limit = req.limit.unwrap_or(500).min(1000);
    
    info!("Bootstrap request: from_checkpoint={}, limit={}", from_checkpoint, limit);

    let all_txs = state.get_txs_since_checkpoint(from_checkpoint, &[]).await;
    let total_available = all_txs.len();
    let checkpoint_height = state.get_checkpoint_height().await;
    
    let transactions: Vec<_> = all_txs.into_iter().take(limit).collect();
    let has_more = total_available > limit;
    
    info!("Bootstrap response: {} txs (total={}, has_more={})", 
        transactions.len(), total_available, has_more);

    Json(BootstrapResponse {
        transactions,
        checkpoint_height,
        total_available,
        has_more,
    })
}

async fn get_snapshot_sync(State(state): State<NodeState>) -> Result<Json<SnapshotSyncResponse>, (StatusCode, String)> {
    // Concurrency limit to prevent API overload during sync storms
    let current = ACTIVE_SYNC_REQUESTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if current >= MAX_CONCURRENT_SYNC_REQUESTS {
        ACTIVE_SYNC_REQUESTS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        warn!("Sync request rejected: too many concurrent requests ({}/{})", current + 1, MAX_CONCURRENT_SYNC_REQUESTS);
        return Err((StatusCode::SERVICE_UNAVAILABLE, "Too many sync requests, try again later".to_string()));
    }
    
    // Decrement on scope exit
    let _guard = SyncRequestGuard;
    
    info!("Snapshot sync request received");
    
    let snapshot = state.get_sync_snapshot().await;
    let checkpoint_height = state.get_checkpoint_height().await;
    
    info!(
        "Snapshot sync response: {} accounts, {} validators, {} checkpoints, {} dag txs",
        snapshot.accounts.len(),
        snapshot.validators.len(),
        snapshot.checkpoints.len(),
        snapshot.dag_transactions.len()
    );

    let mut account_data: Vec<crate::network::AccountData> = snapshot.accounts
        .iter()
        .map(|(address, account)| crate::network::AccountData {
            address: address.clone(),
            balance: account.balance,
            nonce: account.nonce,
            stake: account.staked,
        })
        .collect();
    account_data.sort_by(|a, b| a.address.cmp(&b.address));
    let accounts_merkle_root = build_account_merkle_root_sorted(&account_data);

    Ok(Json(SnapshotSyncResponse {
        accounts: snapshot.accounts,
        validators: snapshot.validators,
        checkpoints: snapshot.checkpoints,
        gas_price: snapshot.gas_price,
        total_supply: snapshot.total_supply,
        genesis_time: snapshot.genesis_time,
        dag_transactions: snapshot.dag_transactions,
        total_transactions: snapshot.total_transactions,
        checkpoint_height,
        contracts: snapshot.contracts,
        accounts_merkle_root,
        rewards_snapshot: snapshot.rewards_snapshot,
        emission_snapshot: snapshot.emission_snapshot,
        slashing_snapshot: snapshot.slashing_snapshot,
        total_burned: snapshot.total_burned,
        total_to_validators: snapshot.total_to_validators,
        genesis_hash: snapshot.genesis_hash,
        finalized_tx_hashes: snapshot.finalized_tx_hashes,
        tx_checkpoint_heights: snapshot.tx_checkpoint_heights,
        weight_scores: snapshot.weight_scores,
    }))
}

#[derive(Deserialize)]
struct MergeAccountsRequest {
    accounts: std::collections::HashMap<String, rinku_core::types::Account>,
}

#[derive(Serialize)]
struct MergeAccountsResponse {
    added: usize,
    updated: usize,
    balance_fixed: usize,
    total: usize,
}

async fn post_merge_accounts(
    State(state): State<NodeState>,
    Json(req): Json<MergeAccountsRequest>,
) -> Json<MergeAccountsResponse> {
    info!("Merge accounts request: {} accounts from peer", req.accounts.len());
    
    let (added, updated, balance_fixed) = state.merge_accounts_from_peer(req.accounts).await;
    let total = state.get_account_count().await;
    
    Json(MergeAccountsResponse {
        added,
        updated,
        balance_fixed,
        total,
    })
}

async fn get_batch_transactions(
    State(state): State<NodeState>,
    Query(query): Query<BatchTxQuery>,
) -> Json<BatchTxResponse> {
    let hashes: Vec<String> = query
        .hashes
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    
    info!("Batch transaction request for {} hashes", hashes.len());
    
    let mut transactions = Vec::new();
    let mut missing = Vec::new();
    
    for hash in &hashes {
        if let Some(tx) = state.get_transaction(hash).await {
            transactions.push(tx);
        } else {
            missing.push(hash.clone());
        }
    }
    
    let found = transactions.len();
    info!("Batch response: found={}, missing={}", found, missing.len());
    
    Json(BatchTxResponse {
        transactions,
        found,
        missing,
    })
}

// Increased from 500 to reduce round trips during high-volume sync
// With 6000+ transactions, 500/page = 12 round trips, but 1000/page = 6 round trips
const DEFAULT_SYNC_LIMIT: usize = 1000;
const MAX_SYNC_LIMIT: usize = 3000;

async fn get_sync_transactions(
    State(state): State<NodeState>,
    Query(query): Query<SyncTxQuery>,
) -> Result<Json<SyncDeltaResponse>, (StatusCode, String)> {
    // Concurrency limit to prevent API overload during sync storms
    let current = ACTIVE_SYNC_REQUESTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if current >= MAX_CONCURRENT_SYNC_REQUESTS {
        ACTIVE_SYNC_REQUESTS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        warn!("Delta sync request rejected: too many concurrent requests ({}/{})", current + 1, MAX_CONCURRENT_SYNC_REQUESTS);
        return Err((StatusCode::SERVICE_UNAVAILABLE, "Too many sync requests, try again later".to_string()));
    }
    
    // Decrement on scope exit
    let _guard = SyncRequestGuard;
    
    let from_checkpoint = query.from_checkpoint.unwrap_or(0);
    let (new_checkpoints, tx_checkpoint_heights, to_checkpoint) = {
        let state_guard = state.inner.read().await;
        let mut tx_checkpoint_heights = std::collections::HashMap::new();
        let mut tx_count_by_checkpoint: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
        for node in state_guard.dag.get_all_nodes() {
            if let Some(height) = node.checkpoint_height {
                tx_checkpoint_heights.insert(node.hash.clone(), height);
                *tx_count_by_checkpoint.entry(height).or_insert(0) += 1;
            }
        }
        let new_checkpoints = state_guard
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
        (new_checkpoints, tx_checkpoint_heights, to_checkpoint)
    };
    
    // Get all transactions since the given checkpoint
    let all_txs = state.get_txs_since_checkpoint(from_checkpoint, &[]).await;
    
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(DEFAULT_SYNC_LIMIT).min(MAX_SYNC_LIMIT);
    let total = all_txs.len();
    
    info!("Sync delta request: checkpoint={}, offset={}, limit={}", 
          from_checkpoint, offset, limit);
    
    // Apply pagination
    let transactions: Vec<_> = all_txs
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect();
    
    let returned = transactions.len();
    let has_more = offset + returned < total;
    
    // Collect current nonces for all unique senders in these transactions (backwards compat)
    let mut account_nonces = std::collections::HashMap::new();
    // Collect FULL account states for authoritative sync (balance + stake + nonce)
    let mut account_states = std::collections::HashMap::new();
    
    // Get all accounts involved in these transactions (both senders and receivers)
    let mut involved_addresses: std::collections::HashSet<String> = std::collections::HashSet::new();
    for tx in &transactions {
        involved_addresses.insert(tx.tx.from.clone());
        involved_addresses.insert(tx.tx.to.clone());
    }
    
    // Fetch full account state for each involved address
    for address in involved_addresses {
        if let Some(account) = state.get_account(&address).await {
            account_nonces.insert(address.clone(), account.nonce);
            account_states.insert(address, account);
        }
    }
    
    info!("Returning {} of {} transactions with {} account states (offset={}, has_more={})", 
          returned, total, account_states.len(), offset, has_more);
    
    let mut page_checkpoint_heights = std::collections::HashMap::new();
    for tx in &transactions {
        if let Some(height) = tx_checkpoint_heights.get(&tx.hash) {
            page_checkpoint_heights.insert(tx.hash.clone(), *height);
        }
    }
    
    let validators = state.get_validators_map().await;
    
    Ok(Json(SyncDeltaResponse {
        transactions,
        account_nonces,
        account_states,
        total,
        offset,
        limit,
        has_more,
        new_checkpoints,
        tx_checkpoint_heights: page_checkpoint_heights,
        from_checkpoint,
        to_checkpoint,
        validators,
    }))
}

/// POST version of delta sync that accepts validators from the requester
/// This enables bidirectional validator sync - the node with more data
/// can still learn about validators from the node requesting sync
async fn post_sync_delta(
    State(api_state): State<ApiState>,
    Json(req): Json<SyncDeltaRequest>,
) -> Result<Json<SyncDeltaResponse>, (StatusCode, String)> {
    let state = &api_state.node_state;
    
    // First, import validators from the requester (bidirectional sync)
    if !req.validators.is_empty() {
        let merged_count = state.merge_validators_from_peer(&req.validators).await;
        if merged_count > 0 {
            info!("Imported {} validators from delta sync requester", merged_count);
            // CRITICAL: Sync to ValidatorIdentityService for vote validation
            // Without this, votes will fail BLS key verification
            if let Some(ref gossip) = api_state.gossip_service {
                gossip.sync_validator_identity_from_state().await;
            }
        }
    }
    
    // Concurrency limit to prevent API overload during sync storms
    let current = ACTIVE_SYNC_REQUESTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if current >= MAX_CONCURRENT_SYNC_REQUESTS {
        ACTIVE_SYNC_REQUESTS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        warn!("Delta sync request rejected: too many concurrent requests ({}/{})", current + 1, MAX_CONCURRENT_SYNC_REQUESTS);
        return Err((StatusCode::SERVICE_UNAVAILABLE, "Too many sync requests, try again later".to_string()));
    }
    
    // Decrement on scope exit
    let _guard = SyncRequestGuard;
    
    let from_checkpoint = req.from_checkpoint.unwrap_or(0);
    let (new_checkpoints, tx_checkpoint_heights, to_checkpoint) = {
        let state_guard = state.inner.read().await;
        let mut tx_checkpoint_heights = std::collections::HashMap::new();
        let mut tx_count_by_checkpoint: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
        for node in state_guard.dag.get_all_nodes() {
            if let Some(height) = node.checkpoint_height {
                tx_checkpoint_heights.insert(node.hash.clone(), height);
                *tx_count_by_checkpoint.entry(height).or_insert(0) += 1;
            }
        }
        let new_checkpoints = state_guard
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
        (new_checkpoints, tx_checkpoint_heights, to_checkpoint)
    };
    
    let all_txs = state.get_txs_since_checkpoint(from_checkpoint, &[]).await;
    
    let offset = req.offset.unwrap_or(0);
    let limit = req.limit.unwrap_or(DEFAULT_SYNC_LIMIT).min(MAX_SYNC_LIMIT);
    let total = all_txs.len();
    
    info!("POST Sync delta request: checkpoint={}, offset={}, limit={}", 
          from_checkpoint, offset, limit);
    
    let transactions: Vec<_> = all_txs
        .into_iter()
        .skip(offset)
        .take(limit)
        .collect();
    
    let returned = transactions.len();
    let has_more = offset + returned < total;
    
    let mut account_nonces = std::collections::HashMap::new();
    let mut account_states = std::collections::HashMap::new();
    
    let mut involved_addresses: std::collections::HashSet<String> = std::collections::HashSet::new();
    for tx in &transactions {
        involved_addresses.insert(tx.tx.from.clone());
        involved_addresses.insert(tx.tx.to.clone());
    }
    
    for address in involved_addresses {
        if let Some(account) = state.get_account(&address).await {
            account_nonces.insert(address.clone(), account.nonce);
            account_states.insert(address, account);
        }
    }
    
    info!("Returning {} of {} transactions with {} account states (offset={}, has_more={})", 
          returned, total, account_states.len(), offset, has_more);
    
    let mut page_checkpoint_heights = std::collections::HashMap::new();
    for tx in &transactions {
        if let Some(height) = tx_checkpoint_heights.get(&tx.hash) {
            page_checkpoint_heights.insert(tx.hash.clone(), *height);
        }
    }
    
    let validators = state.get_validators_map().await;
    
    Ok(Json(SyncDeltaResponse {
        transactions,
        account_nonces,
        account_states,
        total,
        offset,
        limit,
        has_more,
        new_checkpoints,
        tx_checkpoint_heights: page_checkpoint_heights,
        from_checkpoint,
        to_checkpoint,
        validators,
    }))
}

async fn handle_faucet_request(
    State(api_state): State<ApiState>,
    Json(req): Json<FaucetRequest>,
) -> impl IntoResponse {
    let address = req.address.trim().to_string();
    
    if address.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Address required" })),
        ).into_response();
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;

    {
        let mut rate_limit = FAUCET_RATE_LIMIT.lock().unwrap();
        if let Some(&last_request) = rate_limit.get(&address) {
            if now - last_request < FAUCET_RATE_LIMIT_MS {
                let wait_time = (FAUCET_RATE_LIMIT_MS - (now - last_request)) / 1000;
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(serde_json::json!({ 
                        "error": format!("Rate limited. Try again in {} seconds", wait_time) 
                    })),
                ).into_response();
            }
        }
        rate_limit.insert(address.clone(), now);
    }

    let state = &api_state.node_state;
    let tips = state.get_tips().await;
    let tip_urls: Vec<String> = tips
        .into_iter()
        .take(2)
        .map(|hash| format!("rinku://tx/h/{}", hash))
        .collect();

    let faucet_account = state.get_account("faucet").await;
    // Use current nonce (not +1) - add_transaction will increment it
    let nonce = faucet_account.map(|a| a.nonce).unwrap_or(0);

    let inner_tx = rinku_core::types::Transaction {
        from: "faucet".to_string(),
        to: address.clone(),
        amount: FAUCET_AMOUNT,
        nonce,
        timestamp: now,
        parents: tip_urls,
        kind: None,
        gas_limit: None,
        gas_price: Some(0.0),
        data: None,
        signature: None,
        memo: None,
        references: None,
    };

    let tx = rinku_core::types::SignedTransaction {
        tx: inner_tx,
        hash: String::new(),
        signature: "faucet-signature".to_string(),
    };

    let tx_json = serde_json::to_string(&tx.tx).unwrap_or_default();
    let hash = rinku_core::crypto::hash_transaction(&tx_json);
    let tx = rinku_core::types::SignedTransaction { hash: hash.clone(), ..tx };

    match state.add_transaction(tx.clone()).await {
        Ok(TransactionResult::Accepted) => {
            // Broadcast via fast-path for sub-second finality
            if let Some(ref gossip) = api_state.gossip_service {
                let (validator_addr, validator_stake) = state.get_validator_info().await;
                if let (Some(addr), Some(_)) = (validator_addr, validator_stake) {
                    let stake = state.get_validator_stake(&addr).await.unwrap_or(0.0);
                    if stake > 0.0 {
                        gossip.broadcast_fast_path_transaction(tx.clone(), &addr, stake).await;
                        info!("Faucet tx {} to {} broadcast via FAST-PATH", &hash[..16.min(hash.len())], &address[..12.min(address.len())]);
                    } else {
                        gossip.broadcast_transaction(tx).await;
                        info!("Faucet tx {} to {} broadcast to peers", &hash[..16.min(hash.len())], &address[..12.min(address.len())]);
                    }
                } else {
                    gossip.broadcast_transaction(tx).await;
                    info!("Faucet tx {} to {} broadcast to peers", &hash[..16.min(hash.len())], &address[..12.min(address.len())]);
                }
            }
            (
                StatusCode::OK,
                Json(serde_json::json!(FaucetResponse {
                    success: true,
                    amount: FAUCET_AMOUNT,
                    tx_hash: hash,
                })),
            ).into_response()
        }
        Ok(TransactionResult::Buffered) => {
            // Faucet transactions should never be buffered (controlled nonce)
            warn!("Faucet tx {} unexpectedly buffered", &hash[..16.min(hash.len())]);
            (
                StatusCode::OK,
                Json(serde_json::json!(FaucetResponse {
                    success: true,
                    amount: FAUCET_AMOUNT,
                    tx_hash: hash,
                })),
            ).into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FaucetStatsResponse {
    rate_limit_entries: usize,
    max_entries: usize,
    node_url: String,
    genesis_allocation: f64,
    current_balance: f64,
    total_distributed: f64,
    drop_amount: f64,
}

async fn get_faucet_stats(State(state): State<NodeState>) -> Json<FaucetStatsResponse> {
    let rate_limit_entries = {
        let rate_limit = FAUCET_RATE_LIMIT.lock().unwrap();
        rate_limit.len()
    };
    
    let faucet_account = state.get_account("faucet").await;
    let current_balance = faucet_account.map(|a| a.balance).unwrap_or(0.0);
    let genesis_allocation = 1_000_000.0;
    let total_distributed = genesis_allocation - current_balance;
    
    Json(FaucetStatsResponse {
        rate_limit_entries,
        max_entries: 10000,
        node_url: "local".to_string(),
        genesis_allocation,
        current_balance,
        total_distributed,
        drop_amount: FAUCET_AMOUNT,
    })
}

async fn get_stats(State(state): State<NodeState>) -> Json<StatsResponse> {
    let (dag_nodes, tips, accounts) = state.get_dag_stats().await;
    let checkpoint_height = state.get_checkpoint_height().await;
    let gas_price = state.get_gas_price().await;
    let total_supply = state.get_total_supply().await;
    let rewards = state.rewards.read().await;
    let validators = rewards.get_active_validators().len();
    let total_stake = rewards.get_total_staked();
    drop(rewards);

    Json(StatsResponse {
        dag_nodes,
        tips,
        accounts,
        checkpoint_height,
        gas_price,
        total_supply,
        validators,
        total_stake,
    })
}

async fn get_tips(State(state): State<NodeState>) -> Json<TipsResponse> {
    let tips = state.get_tips().await;
    Json(TipsResponse { tips })
}

async fn get_tip_urls(State(state): State<NodeState>) -> Json<TipUrlsResponse> {
    // Use sparse DAG sampling to return at most 16 tips
    // This prevents tip explosion and bounds transaction parent counts
    let tips = state.get_sampled_tips().await;
    let tip_urls: Vec<String> = tips
        .into_iter()
        .map(|hash| format!("rinku://tx/h/{}", hash))
        .collect();
    Json(TipUrlsResponse { tip_urls })
}

async fn get_account(
    State(state): State<NodeState>,
    Path(address): Path<String>,
) -> impl IntoResponse {
    match state.get_account(&address).await {
        Some(account) => (
            StatusCode::OK,
            Json(AccountResponse {
                fingerprint: account.address,
                balance: account.balance,
                nonce: account.nonce,
                staked: account.staked,
            }),
        )
            .into_response(),
        None => ApiError::not_found("Account not found").into_response(),
    }
}

async fn get_account_transactions_with_fast_path(
    State(api_state): State<ApiState>,
    Path(address): Path<String>,
) -> Json<AccountTransactionsResponse> {
    let txs = api_state.node_state.get_transactions_by_address(&address, 100).await;
    
    let transactions: Vec<AccountTransactionItem> = {
        let mut result = Vec::new();
        for (stx, finalized) in txs {
            let direction = if stx.tx.from == address {
                "sent".to_string()
            } else {
                "received".to_string()
            };
            
            let (fast_path_status, fast_path_confirmed_at_ms, fast_path_finality_ms) = 
                if let Some(gossip) = &api_state.gossip_service {
                    match gossip.get_fast_path_status(&stx.hash).await {
                        Some(fp) => {
                            let status = match fp.status {
                                rinku_core::types::FastPathStatus::Pending => "pending",
                                rinku_core::types::FastPathStatus::Confirmed => "confirmed",
                                rinku_core::types::FastPathStatus::Finalized => "finalized",
                            };
                            (Some(status.to_string()), fp.confirmed_at_ms, fp.finality_time_ms())
                        }
                        None => (None, None, None),
                    }
                } else {
                    (None, None, None)
                };
            
            result.push(AccountTransactionItem {
                hash: stx.hash.clone(),
                from: stx.tx.from.clone(),
                to: stx.tx.to.clone(),
                amount: stx.tx.amount,
                timestamp: stx.tx.timestamp,
                direction,
                finalized,
                memo: stx.tx.memo.clone(),
                references: stx.tx.references.clone(),
                fast_path_status,
                fast_path_confirmed_at_ms,
                fast_path_finality_ms,
            });
        }
        result
    };
    
    let total = transactions.len();
    
    Json(AccountTransactionsResponse {
        address,
        transactions,
        total,
    })
}

async fn get_account_proof(
    State(state): State<NodeState>,
    Path(address): Path<String>,
) -> impl IntoResponse {
    let account = match state.get_account(&address).await {
        Some(acc) => acc,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(AccountProofResponse {
                    success: false,
                    proof: None,
                    proof_url: None,
                    verified: None,
                    error: Some("Account not found".to_string()),
                }),
            )
                .into_response();
        }
    };

    if let Some(proof) = account.latest_balance_proof {
        let verified = crate::proofs::verify_account_state_proof(&proof);
        let proof_url = crate::proofs::create_account_state_proof_url(&proof).ok();
        (
            StatusCode::OK,
            Json(AccountProofResponse {
                success: true,
                proof: Some(proof),
                proof_url,
                verified: Some(verified),
                error: None,
            }),
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            Json(AccountProofResponse {
                success: false,
                proof: None,
                proof_url: None,
                verified: None,
                error: Some("No proof available - account may not have finalized transactions".to_string()),
            }),
        )
            .into_response()
    }
}

async fn get_account_proof_current(
    State(state): State<NodeState>,
    Path(address): Path<String>,
) -> impl IntoResponse {
    // Check if account exists
    if state.get_account(&address).await.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(AccountProofResponse {
                success: false,
                proof: None,
                proof_url: None,
                verified: None,
                error: Some("Account not found".to_string()),
            }),
        )
            .into_response();
    }

    // Generate fresh on-demand proof at current checkpoint
    if let Some(proof) = state.generate_account_state_proof_on_demand(&address).await {
        let verified = crate::proofs::verify_account_state_proof(&proof);
        let proof_url = crate::proofs::create_account_state_proof_url(&proof).ok();
        (
            StatusCode::OK,
            Json(AccountProofResponse {
                success: true,
                proof: Some(proof),
                proof_url,
                verified: Some(verified),
                error: None,
            }),
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            Json(AccountProofResponse {
                success: false,
                proof: None,
                proof_url: None,
                verified: None,
                error: Some("No checkpoint available - wait for network to finalize".to_string()),
            }),
        )
            .into_response()
    }
}

async fn get_transaction_receipt(
    State(state): State<NodeState>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    if let Some((tx, _weight)) = state.get_transaction_with_weight(&hash).await {
        let finalized = state.is_finalized(&hash).await;
        
        let (checkpoint_height, checkpoint_hash, state_root) = if finalized {
            if let Some(cp_height) = state.get_tx_checkpoint_height(&hash).await {
                if let Some(cp) = state.get_checkpoint_by_height(cp_height).await {
                    (Some(cp.height), Some(cp.hash.clone()), Some(cp.state_root.clone()))
                } else {
                    let latest = state.get_latest_checkpoint().await;
                    latest.map(|cp| (Some(cp.height), Some(cp.hash.clone()), Some(cp.state_root.clone())))
                        .unwrap_or((None, None, None))
                }
            } else {
                let latest = state.get_latest_checkpoint().await;
                latest.map(|cp| (Some(cp.height), Some(cp.hash.clone()), Some(cp.state_root.clone())))
                    .unwrap_or((None, None, None))
            }
        } else {
            (None, None, None)
        };

        let status = if finalized {
            TransactionStatus::Finalized
        } else {
            TransactionStatus::Pending
        };

        (
            StatusCode::OK,
            Json(TransactionReceipt {
                tx_hash: tx.hash.clone(),
                from: tx.tx.from.clone(),
                to: tx.tx.to.clone(),
                amount: tx.tx.amount,
                fee: tx.tx.gas_price.unwrap_or(0.0),
                nonce: tx.tx.nonce,
                timestamp: tx.tx.timestamp,
                status,
                checkpoint_height,
                checkpoint_hash,
                merkle_proof: None,
                merkle_index: None,
                state_root,
                memo: tx.tx.memo.clone(),
                references: tx.tx.references.clone(),
            }),
        )
            .into_response()
    } else {
        ApiError::not_found("Transaction not found").into_response()
    }
}

async fn submit_transaction(
    State(api_state): State<ApiState>,
    Json(req): Json<SubmitTxRequest>,
) -> impl IntoResponse {
    let tip_count = api_state.node_state.get_tip_count().await;
    let inner = &req.tx;
    
    // Check if this is a system/validator transaction that bypasses degraded mode
    let is_system_tx = inner.sig.starts_with("anchor-")
        || inner.from == "faucet"
        || inner.from == "genesis"
        || matches!(inner.kind, Some(rinku_core::types::TransactionKind::Consolidation));
    
    // Check if sender is a validator (validators can submit during degraded mode)
    let is_validator_tx = api_state.node_state.is_validator(&inner.from).await;
    
    // Hard backpressure: reject ALL transactions when tips exceed hard limit
    if tip_count > MAX_TIPS_BACKPRESSURE {
        warn!("Transaction rejected: DAG tips ({}) exceed hard backpressure threshold ({})", tip_count, MAX_TIPS_BACKPRESSURE);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SubmitTxResponse {
                success: false,
                hash: String::new(),
                error: Some(format!("System overloaded: {} tips pending. All transactions paused. Try again later.", tip_count)),
                fast_path_eligible: None,
                fast_path_status: None,
            }),
        );
    }
    
    // Graceful degradation: when tips > threshold, only allow validator/system transactions
    if tip_count > DEGRADED_MODE_THRESHOLD && !is_system_tx && !is_validator_tx {
        warn!("Transaction rejected: degraded mode active ({} tips), only validator txs allowed", tip_count);
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SubmitTxResponse {
                success: false,
                hash: String::new(),
                error: Some(format!("Network congested: {} tips pending. User transactions paused. Try again in 30s.", tip_count)),
                fast_path_eligible: None,
                fast_path_status: None,
            }),
        );
    }
    
    let inner = req.tx;
    let tx = rinku_core::types::SignedTransaction {
        tx: rinku_core::types::Transaction {
            from: inner.from,
            to: inner.to,
            amount: inner.amount,
            nonce: inner.nonce,
            timestamp: inner.ts,
            parents: inner.parents.clone(),
            kind: inner.kind.clone(),
            gas_limit: None,
            gas_price: Some(inner.fee),
            data: None,
            signature: Some(inner.sig.clone()),
            memo: inner.memo.clone(),
            references: inner.references.clone(),
        },
        hash: inner.hash.clone(),
        signature: inner.sig.clone(),
    };

    let is_fast_path_eligible = tx.is_fast_path_eligible();
    
    match api_state.node_state.add_transaction(tx.clone()).await {
        Ok(TransactionResult::Accepted) => {
            // Broadcast to peers after successful local add
            if let Some(ref gossip) = api_state.gossip_service {
                if is_fast_path_eligible {
                    // Auto-detect: use fast-path for eligible transactions
                    let (validator_addr, _) = api_state.node_state.get_validator_info().await;
                    let validator_stake = if let Some(ref addr) = validator_addr {
                        api_state.node_state.get_validator_stake(addr).await.unwrap_or(0.0)
                    } else {
                        0.0
                    };
                    
                    if let Some(addr) = validator_addr {
                        gossip.broadcast_fast_path_transaction(tx.clone(), &addr, validator_stake).await;
                        info!("Transaction {} broadcast via FAST-PATH", &inner.hash[..16.min(inner.hash.len())]);
                    } else {
                        // No validator identity, fall back to regular broadcast
                        gossip.broadcast_transaction(tx).await;
                        info!("Transaction {} broadcast to peers (no validator for fast-path)", &inner.hash[..16.min(inner.hash.len())]);
                    }
                } else {
                    gossip.broadcast_transaction(tx).await;
                    info!("Transaction {} broadcast to peers", &inner.hash[..16.min(inner.hash.len())]);
                }
            }
            (
                StatusCode::OK,
                Json(SubmitTxResponse {
                    success: true,
                    hash: inner.hash,
                    error: None,
                    fast_path_eligible: Some(is_fast_path_eligible),
                    fast_path_status: if is_fast_path_eligible { Some("pending".to_string()) } else { None },
                }),
            )
        }
        Ok(TransactionResult::Buffered) => {
            // Transaction was buffered - still return success but don't broadcast yet
            info!("Transaction {} buffered (future nonce)", &inner.hash[..16.min(inner.hash.len())]);
            (
                StatusCode::OK,
                Json(SubmitTxResponse {
                    success: true,
                    hash: inner.hash,
                    error: None,
                    fast_path_eligible: Some(is_fast_path_eligible),
                    fast_path_status: None,
                }),
            )
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(SubmitTxResponse {
                success: false,
                hash: inner.hash,
                error: Some(e.to_string()),
                fast_path_eligible: None,
                fast_path_status: None,
            }),
        ),
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FastPathTxResponse {
    success: bool,
    hash: String,
    fast_path_eligible: bool,
    fast_path_status: Option<String>,
    estimated_finality_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn submit_fast_path_transaction(
    State(api_state): State<ApiState>,
    Json(req): Json<SubmitTxRequest>,
) -> impl IntoResponse {
    let inner = req.tx;
    let tx = rinku_core::types::SignedTransaction {
        tx: rinku_core::types::Transaction {
            from: inner.from.clone(),
            to: inner.to.clone(),
            amount: inner.amount,
            nonce: inner.nonce,
            timestamp: inner.ts,
            parents: inner.parents.clone(),
            kind: inner.kind.clone(),
            gas_limit: None,
            gas_price: Some(inner.fee),
            data: None,
            signature: Some(inner.sig.clone()),
            memo: inner.memo.clone(),
            references: inner.references.clone(),
        },
        hash: inner.hash.clone(),
        signature: inner.sig.clone(),
    };

    let is_fast_path_eligible = tx.is_fast_path_eligible();
    
    if !is_fast_path_eligible {
        return (
            StatusCode::BAD_REQUEST,
            Json(FastPathTxResponse {
                success: false,
                hash: inner.hash,
                fast_path_eligible: false,
                fast_path_status: None,
                estimated_finality_ms: None,
                error: Some("Transaction not eligible for fast-path (must be data-only with amount=0)".to_string()),
            }),
        );
    }

    match api_state.node_state.add_transaction(tx.clone()).await {
        Ok(TransactionResult::Accepted) | Ok(TransactionResult::Buffered) => {
            if let Some(ref gossip) = api_state.gossip_service {
                let (validator_addr, _) = api_state.node_state.get_validator_info().await;
                let validator_stake = if let Some(ref addr) = validator_addr {
                    api_state.node_state.get_validator_stake(addr).await.unwrap_or(0.0)
                } else {
                    0.0
                };
                
                if let Some(addr) = validator_addr {
                    gossip.broadcast_fast_path_transaction(tx.clone(), &addr, validator_stake).await;
                } else {
                    gossip.broadcast_transaction(tx.clone()).await;
                }
                
                info!(
                    "Fast-path tx {} submitted and broadcast",
                    &inner.hash[..16.min(inner.hash.len())]
                );
            }
            
            (
                StatusCode::OK,
                Json(FastPathTxResponse {
                    success: true,
                    hash: inner.hash,
                    fast_path_eligible: true,
                    fast_path_status: Some("pending".to_string()),
                    estimated_finality_ms: Some(200),
                    error: None,
                }),
            )
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(FastPathTxResponse {
                success: false,
                hash: inner.hash,
                fast_path_eligible: true,
                fast_path_status: None,
                estimated_finality_ms: None,
                error: Some(e.to_string()),
            }),
        ),
    }
}

#[derive(Serialize)]
struct FastPathStatusResponse {
    hash: String,
    status: String,
    aggregated_stake: f64,
    quorum_threshold: f64,
    quorum_percent: u32,
    ack_count: usize,
    finality_time_ms: Option<u64>,
}

async fn get_fast_path_status(
    State(api_state): State<ApiState>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    if let Some(ref gossip) = api_state.gossip_service {
        if let Some(finality) = gossip.get_fast_path_status(&hash).await {
            let quorum_percent = if finality.quorum_stake_required > 0.0 {
                (finality.total_stake_acked / finality.quorum_stake_required * 100.0) as u32
            } else {
                0
            };
            let finality_time = finality.finality_time_ms();
            return (
                StatusCode::OK,
                Json(FastPathStatusResponse {
                    hash: finality.tx_hash,
                    status: format!("{:?}", finality.status).to_lowercase(),
                    aggregated_stake: finality.total_stake_acked,
                    quorum_threshold: finality.quorum_stake_required,
                    quorum_percent,
                    ack_count: finality.acks.len(),
                    finality_time_ms: finality_time,
                }),
            );
        }
    }
    
    (
        StatusCode::NOT_FOUND,
        Json(FastPathStatusResponse {
            hash,
            status: "unknown".to_string(),
            aggregated_stake: 0.0,
            quorum_threshold: 0.0,
            quorum_percent: 0,
            ack_count: 0,
            finality_time_ms: None,
        }),
    )
}

async fn submit_batch_transaction(
    State(api_state): State<ApiState>,
    Json(req): Json<BatchSubmitTxRequest>,
) -> Result<Json<BatchSubmitTxResponse>, (StatusCode, String)> {
    let tip_count = api_state.node_state.get_tip_count().await;
    
    // Hard backpressure: reject ALL batch transactions when tips exceed hard limit
    if tip_count > MAX_TIPS_BACKPRESSURE {
        warn!("Batch transaction rejected: DAG tips ({}) exceed hard backpressure threshold ({})", tip_count, MAX_TIPS_BACKPRESSURE);
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            format!("System overloaded: {} tips pending. All transactions paused. Try again later.", tip_count)
        ));
    }
    
    // Graceful degradation: batch transactions are typically from regular users, reject in degraded mode
    if tip_count > DEGRADED_MODE_THRESHOLD {
        warn!("Batch transaction rejected: degraded mode active ({} tips)", tip_count);
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            format!("Network congested: {} tips pending. Batch submissions paused. Try again in 30s.", tip_count)
        ));
    }
    
    let total = req.transactions.len();

    // Pre-convert all transactions outside of any locks
    let txs: Vec<rinku_core::types::SignedTransaction> = req.transactions
        .into_iter()
        .map(|item| {
            let inner = item.tx;
            rinku_core::types::SignedTransaction {
                tx: rinku_core::types::Transaction {
                    from: inner.from,
                    to: inner.to,
                    amount: inner.amount,
                    nonce: inner.nonce,
                    timestamp: inner.ts,
                    parents: inner.parents,
                    kind: inner.kind,
                    gas_limit: None,
                    gas_price: Some(inner.fee),
                    data: None,
                    signature: Some(inner.sig.clone()),
                    memo: inner.memo,
                    references: inner.references,
                },
                hash: inner.hash,
                signature: inner.sig,
            }
        })
        .collect();

    // Clone txs for broadcasting after successful add
    let txs_for_broadcast = txs.clone();

    // Use optimized batch method - single lock acquisition
    let results = api_state.node_state.add_transactions_batch(txs).await;
    let successful = results.iter().filter(|r| r.is_ok()).count();
    let failed = results.len() - successful;

    // Broadcast successful transactions to peers
    if let Some(ref gossip) = api_state.gossip_service {
        for (i, result) in results.iter().enumerate() {
            if result.is_ok() {
                if let Some(tx) = txs_for_broadcast.get(i) {
                    gossip.broadcast_transaction(tx.clone()).await;
                }
            }
        }
        if successful > 0 {
            info!("Broadcast {} transactions to peers", successful);
        }
    }

    Ok(Json(BatchSubmitTxResponse {
        successful,
        failed,
        total,
    }))
}

async fn get_dag_summary(State(state): State<NodeState>) -> Json<DagSummaryResponse> {
    // Use combined stats query - single lock acquisition
    let stats = state.get_dashboard_stats().await;
    Json(DagSummaryResponse {
        total_nodes: stats.dag_nodes,
        tip_count: stats.tip_count,
        checkpoint_height: stats.checkpoint_height,
        finalized_count: stats.finalized_count,
        tips: stats.tips,
        merkle_root: "".to_string(),
        account_count: stats.account_count,
    })
}

async fn get_dag(
    State(api_state): State<ApiState>,
    Query(query): Query<DagPageQuery>,
) -> Json<DagResponse> {
    let state = &api_state.node_state;
    let page = query.page.unwrap_or(0);
    let limit = query.limit.unwrap_or(50).min(200); // Default 50, max 200 per page
    
    // Use paginated method - much more efficient for large DAGs
    let (nodes_data, _total, has_more) = state.get_dag_nodes_paginated(page, limit).await;
    
    // Collect all hashes for batch fast-path lookup
    let hashes: Vec<String> = nodes_data.iter().map(|n| n.hash.clone()).collect();
    
    // Batch lookup fast-path status for all transactions
    let fast_path_statuses: std::collections::HashMap<String, rinku_core::types::FastPathFinality> = 
        if let Some(ref gossip) = api_state.gossip_service {
            let mut statuses = std::collections::HashMap::new();
            for hash in &hashes {
                if let Some(finality) = gossip.get_fast_path_status(hash).await {
                    statuses.insert(hash.clone(), finality);
                }
            }
            statuses
        } else {
            std::collections::HashMap::new()
        };
    
    // Batch lookup trust scores from weight_trie
    let trust_scores: std::collections::HashMap<String, (u8, u32)> = {
        let inner = state.inner.read().await;
        let mut scores = std::collections::HashMap::new();
        if let Some(ref weight_trie) = inner.weight_trie {
            for hash in &hashes {
                if let Some(weight) = weight_trie.get_weight(hash) {
                    scores.insert(hash.clone(), (weight.trust_score(), weight.attestation_count));
                }
            }
        }
        scores
    };
    
    let nodes: Vec<DagNodeResponse> = nodes_data
        .into_iter()
        .map(|n| {
            // Use /tx/h/{hash} format for explorer hash-based routing
            let url = format!("/tx/h/{}", &n.hash);
            // Normalize parents to /tx/h/{hash} format for explorer traversal
            let parents: Vec<String> = n.parents.iter()
                .map(|p| {
                    let h = if p.starts_with("rinku://tx/h/") {
                        p.strip_prefix("rinku://tx/h/").unwrap_or(p)
                    } else if p.starts_with("rinku://tx/") {
                        p.strip_prefix("rinku://tx/").unwrap_or(p)
                    } else if p.starts_with("/tx/h/") {
                        return p.clone(); // Already in correct format
                    } else {
                        p.as_str()
                    };
                    format!("/tx/h/{}", h)
                })
                .collect();
            
            // Get fast-path status for this transaction
            let (fast_path_status, fast_path_confirmed_at_ms, fast_path_finality_ms) = 
                if let Some(finality) = fast_path_statuses.get(&n.hash) {
                    (
                        Some(format!("{:?}", finality.status).to_lowercase()),
                        finality.confirmed_at_ms,
                        finality.finality_time_ms(),
                    )
                } else {
                    (Some("pending".to_string()), None, None)
                };
            
            // Get trust score and attestation count
            let (trust_score, attestation_count) = trust_scores.get(&n.hash)
                .map(|(score, count)| (Some(*score), Some(*count)))
                .unwrap_or((None, None));
            
            DagNodeResponse {
                hash: n.hash,
                from: n.from,
                to: n.to,
                amount: n.amount,
                fee: n.fee,
                nonce: n.nonce,
                ts: n.ts,
                parent_count: parents.len(),
                parents,
                finalized: n.finalized,
                weight: n.weight,
                url,
                kind: n.kind,
                fast_path_status,
                fast_path_confirmed_at_ms,
                fast_path_finality_ms,
                trust_score,
                attestation_count,
            }
        })
        .collect();
    Json(DagResponse {
        has_more,
        nodes,
    })
}

async fn get_accounts(State(state): State<NodeState>) -> Json<AccountsResponse> {
    let accounts = state.get_all_accounts().await;
    Json(AccountsResponse {
        accounts: accounts
            .into_iter()
            .map(|a| AccountResponse {
                fingerprint: a.address,
                balance: a.balance,
                nonce: a.nonce,
                staked: a.staked,
            })
            .collect(),
    })
}

async fn get_network_stats(State(state): State<NodeState>) -> Json<NetworkStatsResponse> {
    // Use combined stats query - single lock acquisition for main state
    let stats = state.get_dashboard_stats().await;
    
    // Get rewards info with separate lock (could be combined later)
    let rewards = state.rewards.read().await;
    let total_stake = rewards.get_total_staked();
    let validators = rewards.get_active_validators().len();
    drop(rewards);
    
    // Finality ratio based on total transactions (not just current DAG nodes)
    // This accounts for pruned finalized nodes correctly
    let total_txs = stats.total_transactions as u64;
    let finality_ratio = if total_txs > 0 {
        // finality_count tracks all transactions ever finalized
        // unfinalized_count is current pending transactions
        let finalized = total_txs.saturating_sub(stats.unfinalized_count as u64);
        finalized as f64 / total_txs as f64
    } else {
        1.0 // No transactions = 100% finalized (nothing pending)
    };
    
    let elapsed_secs = state.get_elapsed_seconds();
    let tps = if elapsed_secs > 0.0 && stats.total_transactions > 0 {
        (stats.total_transactions as f64) / elapsed_secs
    } else {
        0.0
    };
    Json(NetworkStatsResponse {
        tps,
        total_transactions_processed: stats.total_transactions as usize,
        finalized_count: stats.finalized_count,
        unfinalized_count: stats.unfinalized_count,
        finality_ratio,
        checkpoint_count: stats.checkpoint_height,
        latest_checkpoint_height: stats.checkpoint_height,
        latest_checkpoint_id: stats.latest_checkpoint_id,
        total_staked: total_stake,
        validator_count: validators,
        network_age: elapsed_secs as u64,
    })
}

async fn get_gas_price(State(state): State<NodeState>) -> Json<GasPriceResponse> {
    let (current, total_burned, _, avg) = state.get_gas_stats().await;
    Json(GasPriceResponse {
        current,
        min: 0.001,
        max: 10.0, // Match TypeScript GAS_MAX_FEE
        avg_last_100: avg,
        total_burned,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GasStatsResponse {
    total_burned: f64,
    total_to_validators: f64,
}

async fn get_gas_stats(State(state): State<NodeState>) -> Json<GasStatsResponse> {
    let (_, total_burned, total_to_validators, _) = state.get_gas_stats().await;
    Json(GasStatsResponse {
        total_burned,
        total_to_validators,
    })
}

async fn get_finality_metrics(State(api_state): State<ApiState>) -> Json<FinalityMetricsResponse> {
    let state = &api_state.node_state;
    let total_transactions = state.get_total_transactions().await as usize;
    let (finalized_count, pending_count) = state.get_finalized_stats().await;
    let (avg_ms, median_ms, p95_ms, last_checkpoint_age_ms, checkpoints_per_min) = 
        state.get_finality_timing().await;
    
    // Use total_transactions as denominator (not dag_nodes which shrinks after pruning)
    // finality_rate = (total - pending) / total
    let finality_rate = if total_transactions > 0 {
        let finalized = total_transactions.saturating_sub(pending_count);
        finalized as f64 / total_transactions as f64
    } else {
        1.0 // No transactions = 100% finalized
    };
    let tx_throughput = if total_transactions > 0 { (total_transactions as f64) / 60.0 } else { 0.0 };
    
    // Get fast-path confirmation stats if available
    let avg_confirmation_ms = if let Some(ref gossip) = api_state.gossip_service {
        gossip.get_fast_path_stats().await.avg_confirmation_ms
    } else {
        None
    };
    
    Json(FinalityMetricsResponse {
        avg_time_to_finality: avg_ms,
        median_time_to_finality: median_ms,
        p95_time_to_finality: p95_ms,
        pending_count,
        finalized_count,
        finality_rate,
        checkpoint_latency: avg_ms,
        checkpoints_per_minute: checkpoints_per_min,
        last_checkpoint_age: last_checkpoint_age_ms,
        tx_throughput,
        avg_confirmation_ms,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TransactionResponse {
    hash: String,
    from: String,
    to: String,
    amount: f64,
    fee: f64,
    nonce: u64,
    ts: u64,
    #[serde(rename = "tipUrls")]
    tip_urls: Vec<String>,
    finalized: bool,
    weight: f64,
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    memo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    references: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fast_path_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fast_path_confirmed_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fast_path_finality_ms: Option<u64>,
}

async fn get_transaction(
    State(api_state): State<ApiState>,
    Path(hash): Path<String>,
) -> Result<Json<TransactionResponse>, ApiError> {
    let state = &api_state.node_state;
    // Debug: log lookup attempt
    tracing::debug!("Looking up transaction hash: {}", hash);
    
    // Get transaction with weight from DAG node
    if let Some((tx, weight)) = state.get_transaction_with_weight(&hash).await {
        tracing::debug!("Found transaction {} in DAG", hash);
        let finalized = state.is_finalized(&hash).await;
        // Normalize parents to /tx/h/{hash} format for explorer navigation
        let tip_urls: Vec<String> = tx.tx.parents.iter()
            .map(|p| {
                let h = if p.starts_with("rinku://tx/h/") {
                    p.strip_prefix("rinku://tx/h/").unwrap_or(p)
                } else if p.starts_with("rinku://tx/") {
                    p.strip_prefix("rinku://tx/").unwrap_or(p)
                } else if p.starts_with("/tx/h/") {
                    p.strip_prefix("/tx/h/").unwrap_or(p)
                } else {
                    p.as_str()
                };
                format!("/tx/h/{}", h)
            })
            .collect();
        
        // Query fast-path status from GossipService
        let (fast_path_status, fast_path_confirmed_at_ms, fast_path_finality_ms) = 
            if let Some(ref gossip) = api_state.gossip_service {
                if let Some(fp) = gossip.get_fast_path_status(&hash).await {
                    let is_confirmed = matches!(fp.status, rinku_core::types::FastPathStatus::Confirmed | rinku_core::types::FastPathStatus::Finalized);
                    let status = match fp.status {
                        rinku_core::types::FastPathStatus::Confirmed => "confirmed",
                        rinku_core::types::FastPathStatus::Finalized => "finalized",
                        rinku_core::types::FastPathStatus::Pending => "pending",
                    };
                    let confirmed_at = fp.confirmed_at_ms;
                    let finality_ms = fp.finality_time_ms();
                    (Some(status.to_string()), confirmed_at, finality_ms)
                } else {
                    (None, None, None)
                }
            } else {
                (None, None, None)
            };
        
        Ok(Json(TransactionResponse {
            hash: tx.hash.clone(),
            from: tx.tx.from.clone(),
            to: tx.tx.to.clone(),
            amount: tx.tx.amount,
            fee: tx.tx.gas_price.unwrap_or(0.0),
            nonce: tx.tx.nonce,
            ts: tx.tx.timestamp,
            tip_urls,
            finalized,
            weight,
            url: format!("/tx/h/{}", tx.hash),
            memo: tx.tx.memo.clone(),
            references: tx.tx.references.clone(),
            fast_path_status,
            fast_path_confirmed_at_ms,
            fast_path_finality_ms,
        }))
    } else {
        // Debug: log lookup failure with DAG stats
        let (dag_size, _, _) = state.get_dag_stats().await;
        tracing::warn!("Transaction {} not found in DAG (DAG size: {})", hash, dag_size);
        Err(ApiError::not_found(format!("Transaction {} not found (may have been pruned after finalization)", hash)))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadResponse {
    parent_hash: String,
    replies: Vec<TransactionResponse>,
    total_replies: usize,
}

async fn get_transaction_replies(
    State(api_state): State<ApiState>,
    Path(hash): Path<String>,
) -> Result<Json<ThreadResponse>, ApiError> {
    let state = &api_state.node_state;
    
    // Collect reply data from DAG first, then release lock before async gossip calls
    let reply_data: Vec<(String, String, String, f64, f64, u64, u64, Vec<String>, bool, f64, Option<String>, Option<Vec<String>>)> = {
        let inner = state.inner.read().await;
        let all_txs = inner.dag.all_transactions();
        
        all_txs.iter()
            .filter(|tx| tx.tx.references.as_ref().map_or(false, |refs| refs.contains(&hash)))
            .map(|tx| {
                let (finalized, weight) = inner.dag.get_node(&tx.hash)
                    .map(|n| (n.finalized, n.weight))
                    .unwrap_or((false, 1.0));
                
                let tip_urls: Vec<String> = tx.tx.parents.iter()
                    .map(|p| {
                        let h = if p.starts_with("rinku://tx/h/") {
                            p.strip_prefix("rinku://tx/h/").unwrap_or(p)
                        } else if p.starts_with("rinku://tx/") {
                            p.strip_prefix("rinku://tx/").unwrap_or(p)
                        } else if p.starts_with("/tx/h/") {
                            p.strip_prefix("/tx/h/").unwrap_or(p)
                        } else {
                            p.as_str()
                        };
                        format!("/tx/h/{}", h)
                    })
                    .collect();
                
                (
                    tx.hash.clone(),
                    tx.tx.from.clone(),
                    tx.tx.to.clone(),
                    tx.tx.amount,
                    tx.tx.gas_price.unwrap_or(0.0),
                    tx.tx.nonce,
                    tx.tx.timestamp,
                    tip_urls,
                    finalized,
                    weight,
                    tx.tx.memo.clone(),
                    tx.tx.references.clone(),
                )
            })
            .collect()
    };
    // Lock released here
    
    // Batch lookup fast-path status for all replies (after releasing DAG lock)
    let reply_hashes: Vec<String> = reply_data.iter().map(|d| d.0.clone()).collect();
    let fast_path_statuses: std::collections::HashMap<String, rinku_core::types::FastPathFinality> = 
        if let Some(ref gossip) = api_state.gossip_service {
            let mut statuses = std::collections::HashMap::new();
            for h in &reply_hashes {
                if let Some(finality) = gossip.get_fast_path_status(h).await {
                    statuses.insert(h.clone(), finality);
                }
            }
            statuses
        } else {
            std::collections::HashMap::new()
        };
    
    let mut replies: Vec<TransactionResponse> = reply_data.into_iter()
        .map(|(tx_hash, from, to, amount, fee, nonce, ts, tip_urls, finalized, weight, memo, references)| {
            // Get fast-path status for this reply
            let (fast_path_status, fast_path_confirmed_at_ms, fast_path_finality_ms) = 
                if let Some(fp) = fast_path_statuses.get(&tx_hash) {
                    let status = match fp.status {
                        rinku_core::types::FastPathStatus::Confirmed => "confirmed",
                        rinku_core::types::FastPathStatus::Finalized => "finalized",
                        rinku_core::types::FastPathStatus::Pending => "pending",
                    };
                    let confirmed_at = fp.confirmed_at_ms;
                    let finality_ms = fp.finality_time_ms();
                    (Some(status.to_string()), confirmed_at, finality_ms)
                } else {
                    (None, None, None)
                };
            
            TransactionResponse {
                hash: tx_hash.clone(),
                from,
                to,
                amount,
                fee,
                nonce,
                ts,
                tip_urls,
                finalized,
                weight,
                url: format!("/tx/h/{}", tx_hash),
                memo,
                references,
                fast_path_status,
                fast_path_confirmed_at_ms,
                fast_path_finality_ms,
            }
        })
        .collect();
    
    replies.sort_by(|a, b| a.ts.cmp(&b.ts));
    let total_replies = replies.len();
    
    Ok(Json(ThreadResponse {
        parent_hash: hash,
        replies,
        total_replies,
    }))
}

async fn get_self_provable_tx(
    State(state): State<NodeState>,
    Path(hash): Path<String>,
) -> Result<Json<SelfProvableTransactionResponse>, ApiError> {
    let tx = state
        .get_transaction(&hash)
        .await
        .ok_or_else(|| ApiError::not_found(format!("Transaction {} not found (may have been pruned)", hash)))?;

    let (finalized, checkpoint_height) = state.get_finalization_info(&hash).await;

    let (merkle_proof, merkle_index, checkpoint_data, proof_url) = if finalized {
        if let Some(cp_height) = checkpoint_height {
            if let Some((proof, index, checkpoint)) = state.get_merkle_proof(&hash, cp_height).await {
                let cp_data = CheckpointProofData {
                    height: checkpoint.height,
                    hash: checkpoint.hash.clone(),
                    tx_merkle_root: checkpoint.tx_merkle_root.clone(),
                    state_root: checkpoint.state_root.clone(),
                    receipt_root: checkpoint.receipt_root.clone(),
                    tip_count: checkpoint.tip_count,
                    timestamp: checkpoint.timestamp,
                    aggregated_signature: checkpoint.aggregated_signature.clone(),
                    signer_bitmap: checkpoint.signer_bitmap.clone(),
                    validator_count: checkpoint.validator_signatures.len(),
                };

                let proof_url = format!(
                    "rinku://txp/{}?cp={}&idx={}&proof={}",
                    hash,
                    cp_height,
                    index,
                    proof.join(",")
                );

                (Some(proof), Some(index), Some(cp_data), Some(proof_url))
            } else {
                (None, None, None, None)
            }
        } else {
            (None, None, None, None)
        }
    } else {
        (None, None, None, None)
    };

    let self_contained_proof_url = if finalized && proof_url.is_some() {
        Some(format!("/api/tx/{}/proof", tx.hash))
    } else {
        None
    };

    Ok(Json(SelfProvableTransactionResponse {
        tx_hash: tx.hash.clone(),
        from: tx.tx.from.clone(),
        to: tx.tx.to.clone(),
        amount: tx.tx.amount,
        nonce: tx.tx.nonce,
        timestamp: tx.tx.timestamp,
        signature: tx.signature.clone(),
        parents: tx.tx.parents.clone(),
        finalized,
        checkpoint_height,
        merkle_proof,
        merkle_index,
        checkpoint: checkpoint_data,
        proof_url,
        self_contained_proof_url,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StakingConfig {
    tip_reward_rate: f64,
    stake_reward_rate: f64,
    witness_reward_rate: f64,
    min_stake_amount: f64,
    unstake_cooldown_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StakingResponse {
    total_staked: f64,
    validators: Vec<StakerInfo>,
    top_stakers: Vec<StakerInfo>,
    config: StakingConfig,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StakerInfo {
    staker: String,
    amount: f64,
    staked_at: u64,
}

async fn get_staking(State(state): State<NodeState>) -> Json<StakingResponse> {
    let rewards = state.rewards.read().await;
    let total_staked = rewards.get_total_staked();
    let active_validators = rewards.get_active_validators();
    
    let stakers: Vec<StakerInfo> = active_validators.iter().map(|v| StakerInfo {
        staker: v.staker.clone(),
        amount: v.amount,
        staked_at: v.staked_at,
    }).collect();
    
    let mut top_stakers = stakers.clone();
    top_stakers.sort_by(|a, b| b.amount.partial_cmp(&a.amount).unwrap_or(std::cmp::Ordering::Equal));
    top_stakers.truncate(10);
    
    Json(StakingResponse {
        total_staked,
        validators: stakers,
        top_stakers,
        config: StakingConfig {
            tip_reward_rate: 0.30,
            stake_reward_rate: 0.50,
            witness_reward_rate: 0.20,
            min_stake_amount: 100.0,
            unstake_cooldown_ms: 7 * 24 * 60 * 60 * 1000,
        },
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TokenomicsSupplyResponse {
    max_supply: f64,
    genesis_allocation: f64,
    circulating_supply: f64,
    total_emitted: f64,
    total_burned: f64,
    remaining_to_emit: f64,
    current_reward: f64,
    halving_epoch: u32,
    next_halving_at: u64,
    halving_interval: u64,
    checkpoint_height: u64,
}

async fn get_tokenomics_supply(State(state): State<NodeState>) -> Json<TokenomicsSupplyResponse> {
    let total_supply = state.get_total_supply().await;
    let checkpoint_height = state.get_checkpoint_height().await;
    let (_, gas_burned, _, _) = state.get_gas_stats().await;
    
    // Get actual emission stats from emission service
    let (emission_total_emitted, _) = state.get_emission_stats().await;
    
    let max_supply = 30_000_000.0;
    let genesis_allocation = 6_000_000.0;
    
    // total_emitted = RKU emitted through checkpoint rewards (from emission service)
    // total_burned = gas fees burned (deflationary pressure)
    Json(TokenomicsSupplyResponse {
        max_supply,
        genesis_allocation,
        circulating_supply: total_supply,
        total_emitted: emission_total_emitted,
        total_burned: gas_burned,
        remaining_to_emit: max_supply - genesis_allocation,
        current_reward: 12.5,
        halving_epoch: 0,
        next_halving_at: 350000,
        halving_interval: 350000,
        checkpoint_height,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RewardsConfigResponse {
    tip_reward_percent: f64,
    stake_reward_percent: f64,
    witness_reward_percent: f64,
    min_stake_for_rewards: f64,
}

async fn get_rewards_config() -> Json<RewardsConfigResponse> {
    Json(RewardsConfigResponse {
        tip_reward_percent: 30.0,
        stake_reward_percent: 50.0,
        witness_reward_percent: 20.0,
        min_stake_for_rewards: 100.0,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmissionScheduleItem {
    epoch: u32,
    start_height: u64,
    reward: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmissionResponse {
    current_epoch: u32,
    current_reward: f64,
    halving_interval: u64,
    total_halvings: u32,
    min_reward: f64,
    schedule: Vec<EmissionScheduleItem>,
    stake_weight_percent: f64,
    age_weight_percent: f64,
}

async fn get_tokenomics_emission(State(state): State<NodeState>) -> Json<EmissionResponse> {
    let checkpoint_height = state.get_checkpoint_height().await;
    let emission = state.emission.read().await;
    let stats = emission.get_stats(checkpoint_height);
    
    let halving_interval: u64 = 3_150_000;
    let mut schedule = Vec::new();
    let mut reward = 12.5;
    for epoch in 0..10 {
        schedule.push(EmissionScheduleItem {
            epoch,
            start_height: epoch as u64 * halving_interval,
            reward,
        });
        reward /= 2.0;
    }
    
    Json(EmissionResponse {
        current_epoch: stats.halving_epoch,
        current_reward: stats.current_reward,
        halving_interval,
        total_halvings: 10,
        min_reward: 0.01,
        schedule,
        stake_weight_percent: 70.0,
        age_weight_percent: 30.0,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SlashingConfigResponse {
    double_sign_percent: f64,
    invalid_checkpoint_percent: f64,
    liveness_percent: f64,
    liveness_repeat_percent: f64,
    liveness_miss_threshold: u32,
    unbonding_period_days: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SlashingResponse {
    config: SlashingConfigResponse,
    events: Vec<SlashEventResponse>,
    total_slashed: f64,
    unbonding_queue: Vec<UnbondingEntryResponse>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SlashEventResponse {
    id: String,
    validator: String,
    reason: String,
    amount: f64,
    percent_slashed: f64,
    checkpoint_height: u64,
    timestamp: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UnbondingEntryResponse {
    validator: String,
    amount: f64,
    started_at: u64,
    available_at: u64,
    slashable: bool,
}

async fn get_tokenomics_slashing(State(state): State<NodeState>) -> Json<SlashingResponse> {
    let slashing = state.slashing.read().await;
    let events = slashing.get_slash_events(100);
    let queue = slashing.get_unbonding_queue();
    
    let slash_events: Vec<SlashEventResponse> = events
        .iter()
        .map(|e| SlashEventResponse {
            id: e.id.clone(),
            validator: e.validator.clone(),
            reason: format!("{:?}", e.reason).to_lowercase(),
            amount: e.amount,
            percent_slashed: e.percent_slashed,
            checkpoint_height: e.checkpoint_height,
            timestamp: e.timestamp,
        })
        .collect();
    
    let unbonding_queue: Vec<UnbondingEntryResponse> = queue
        .iter()
        .map(|e| UnbondingEntryResponse {
            validator: e.validator.clone(),
            amount: e.amount,
            started_at: e.started_at,
            available_at: e.available_at,
            slashable: e.slashable,
        })
        .collect();
    
    Json(SlashingResponse {
        config: SlashingConfigResponse {
            double_sign_percent: 15.0,
            invalid_checkpoint_percent: 25.0,
            liveness_percent: 5.0,
            liveness_repeat_percent: 10.0,
            liveness_miss_threshold: 3,
            unbonding_period_days: 14,
        },
        events: slash_events,
        total_slashed: slashing.get_total_slashed(),
        unbonding_queue,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckpointsResponse {
    checkpoints: Vec<CheckpointInfo>,
    total: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckpointInfo {
    height: u64,
    merkle_root: String,
    tx_count: usize,
    timestamp: u64,
    validators: usize,
}

async fn get_checkpoints(State(state): State<NodeState>) -> Json<CheckpointsResponse> {
    let inner = state.inner.read().await;
    let checkpoints: Vec<CheckpointInfo> = inner.checkpoints
        .iter()
        .rev()
        .take(20)
        .map(|cp| CheckpointInfo {
            height: cp.height,
            merkle_root: cp.tx_merkle_root.clone(),
            tx_count: cp.tip_count as usize,
            timestamp: cp.timestamp,
            validators: cp.validator_signatures.len(),
        })
        .collect();
    
    let total = inner.checkpoints.len();
    drop(inner);
    
    Json(CheckpointsResponse {
        total,
        checkpoints,
    })
}

async fn get_checkpoints_latest(State(state): State<NodeState>) -> Json<CheckpointInfo> {
    let inner = state.inner.read().await;
    if let Some(cp) = inner.checkpoints.last() {
        Json(CheckpointInfo {
            height: cp.height,
            merkle_root: cp.tx_merkle_root.clone(),
            tx_count: cp.tip_count as usize,
            timestamp: cp.timestamp,
            validators: cp.validator_signatures.len(),
        })
    } else {
        Json(CheckpointInfo {
            height: 0,
            merkle_root: "genesis".to_string(),
            tx_count: 0,
            timestamp: inner.genesis_time,
            validators: 0,
        })
    }
}

/// Get checkpoint by height - used for peer checkpoint verification
async fn get_checkpoint_by_height(
    State(state): State<NodeState>,
    Path(height): Path<u64>,
) -> Result<Json<rinku_core::types::Checkpoint>, ApiError> {
    let inner = state.inner.read().await;
    
    // Find checkpoint at the requested height
    if let Some(cp) = inner.checkpoints.iter().find(|c| c.height == height) {
        Ok(Json(cp.clone()))
    } else {
        Err(ApiError::not_found(format!("Checkpoint at height {} not found", height)))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ForkStatsResponse {
    detected_forks: usize,
    resolved_forks: usize,
    double_spends_detected: usize,
    double_spends_resolved: usize,
}

async fn get_fork_stats() -> Json<ForkStatsResponse> {
    Json(ForkStatsResponse {
        detected_forks: 0,
        resolved_forks: 0,
        double_spends_detected: 0,
        double_spends_resolved: 0,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GossipStatsResponse {
    peers_connected: usize,
    messages_sent: u64,
    messages_received: u64,
    last_gossip_at: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PeersResponse {
    http_peers: Vec<crate::gossip::PeerInfo>,
    p2p_peers: Vec<crate::network::PeerStats>,
}

async fn get_gossip_stats(
    State(api_state): State<ApiState>,
) -> Json<GossipStatsResponse> {
    if let Some(ref gossip_service) = api_state.gossip_service {
        let stats = gossip_service.get_stats().await;
        let peer_count = gossip_service.get_peer_count().await;
        Json(GossipStatsResponse {
            peers_connected: peer_count,
            messages_sent: stats.txs_propagated,
            messages_received: stats.txs_received,
            last_gossip_at: stats.sync_requests,
        })
    } else {
        Json(GossipStatsResponse {
            peers_connected: 0,
            messages_sent: 0,
            messages_received: 0,
            last_gossip_at: 0,
        })
    }
}

async fn get_peers(
    State(api_state): State<ApiState>,
) -> Json<PeersResponse> {
    if let Some(ref gossip_service) = api_state.gossip_service {
        let http_peers = gossip_service.get_http_peers().await;
        let p2p_peers = gossip_service.get_p2p_peers().await;
        Json(PeersResponse { http_peers, p2p_peers })
    } else {
        Json(PeersResponse { http_peers: Vec::new(), p2p_peers: Vec::new() })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TipConsolidatorStatsResponse {
    consolidations_created: u64,
    tips_reduced: u64,
    enabled: bool,
}

async fn get_tip_consolidator_stats() -> Json<TipConsolidatorStatsResponse> {
    Json(TipConsolidatorStatsResponse {
        consolidations_created: 0,
        tips_reduced: 0,
        enabled: true,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyProofRequest {
    proof_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifyProofResponse {
    valid: bool,
    errors: Vec<String>,
    tx_hash: String,
    tx_from: String,
    tx_to: String,
    tx_amount: f64,
    tx_nonce: u64,
    tx_timestamp: u64,
    checkpoint_height: u64,
    checkpoint_id: String,
    merkle_verified: bool,
    bls_verified: bool,
    validator_set_verified: bool,
    signer_weight: f64,
    total_weight: f64,
    signer_count: usize,
}

async fn verify_proof_endpoint(
    Json(req): Json<VerifyProofRequest>,
) -> impl IntoResponse {
    use crate::proofs::{decode_self_contained_proof, verify_self_contained_proof, decode_account_state_proof, verify_account_state_proof_detailed};
    
    let proof_url = req.proof_url.trim();
    
    // Handle account state proofs (rinku://asp/...)
    if proof_url.starts_with("rinku://asp/") {
        match decode_account_state_proof(proof_url) {
            Ok(proof) => {
                let detail = verify_account_state_proof_detailed(&proof);
                if !detail.valid {
                    tracing::warn!(
                        "Account state proof verification FAILED for {}: leaf_data='{}', leaf_hash={}, computed_root={}, expected_root={}, merkle_index={}, proof_len={}",
                        &proof.address[..16.min(proof.address.len())],
                        detail.leaf_data,
                        &detail.leaf_hash[..16.min(detail.leaf_hash.len())],
                        &detail.computed_root[..16.min(detail.computed_root.len())],
                        &detail.expected_root[..16.min(detail.expected_root.len())],
                        detail.merkle_index,
                        detail.proof_length
                    );
                }
                return (StatusCode::OK, Json(serde_json::json!({
                    "proofType": "account_state",
                    "valid": detail.valid,
                    "address": proof.address,
                    "balance": proof.balance,
                    "nonce": proof.nonce,
                    "staked": proof.staked,
                    "checkpointHeight": proof.checkpoint_height,
                    "stateRoot": proof.state_root,
                    "merkleIndex": proof.merkle_index,
                    "merkleProof": proof.merkle_proof,
                    "debug": if !detail.valid { Some(serde_json::json!({
                        "leafData": detail.leaf_data,
                        "leafHash": detail.leaf_hash,
                        "computedRoot": detail.computed_root,
                        "expectedRoot": detail.expected_root
                    })) } else { None }
                }))).into_response();
            }
            Err(e) => {
                return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                    "proofType": "account_state",
                    "valid": false,
                    "error": format!("Failed to decode account state proof: {}", e)
                }))).into_response();
            }
        }
    }
    
    // Handle transaction proofs (rinku://sp/...)
    match decode_self_contained_proof(proof_url) {
        Ok(proof) => {
            let result = verify_self_contained_proof(&proof);
            
            (StatusCode::OK, Json(serde_json::json!({
                "proofType": "transaction",
                "valid": result.valid,
                "errors": result.errors,
                "txHash": result.tx_hash,
                "txFrom": proof.tx_from,
                "txTo": proof.tx_to,
                "txAmount": proof.tx_amount,
                "txNonce": proof.tx_nonce,
                "txTimestamp": proof.tx_timestamp,
                "checkpointHeight": result.checkpoint_height,
                "checkpointId": proof.checkpoint_id,
                "merkleVerified": result.merkle_verified,
                "blsVerified": result.bls_verified,
                "validatorSetVerified": result.validator_set_verified,
                "signerWeight": result.computed_signer_weight,
                "totalWeight": result.total_weight,
                "signerCount": result.signer_count,
            }))).into_response()
        }
        Err(e) => {
            (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "proofType": "transaction",
                "valid": false,
                "error": format!("Failed to decode proof: {}", e)
            }))).into_response()
        }
    }
}

async fn generate_transaction_proof(
    State(state): State<NodeState>,
    Path(hash): Path<String>,
) -> Result<Json<TransactionProofResponse>, ApiError> {
    use crate::proofs::{
        create_self_proof_url, build_merkle_sum_tree, get_merkle_sum_proof,
        MerkleSumLeaf, SelfContainedProof,
    };
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    let tx = state
        .get_transaction(&hash)
        .await
        .ok_or_else(|| ApiError::not_found(format!("Transaction {} not found", hash)))?;

    let (finalized, checkpoint_height) = state.get_finalization_info(&hash).await;

    if !finalized {
        return Ok(Json(TransactionProofResponse {
            tx_hash: hash,
            finalized: false,
            proof_url: None,
            proof_size_bytes: None,
            qr_viable: None,
            error: Some("Transaction not yet finalized".to_string()),
        }));
    }

    let cp_height = match checkpoint_height {
        Some(h) => h,
        None => {
            return Ok(Json(TransactionProofResponse {
                tx_hash: hash,
                finalized: true,
                proof_url: None,
                proof_size_bytes: None,
                qr_viable: None,
                error: Some("Checkpoint height not found".to_string()),
            }));
        }
    };

    let (merkle_proof, merkle_index, checkpoint) = match state.get_merkle_proof(&hash, cp_height).await {
        Some(data) => data,
        None => {
            return Ok(Json(TransactionProofResponse {
                tx_hash: hash,
                finalized: true,
                proof_url: None,
                proof_size_bytes: None,
                qr_viable: None,
                error: Some("Could not generate merkle proof (transaction may have been pruned)".to_string()),
            }));
        }
    };

    // Build validator leaves from checkpoint's validator signatures (not global state)
    let validator_leaves: Vec<MerkleSumLeaf> = checkpoint
        .validator_signatures
        .iter()
        .filter(|sig| sig.bls_public_key.is_some())
        .enumerate()
        .map(|(i, sig)| {
            let weight_units = crate::proofs::to_weight_units(sig.weight);
            MerkleSumLeaf {
                index: i,
                address: sig.validator.clone(),
                bls_public_key: sig.bls_public_key.clone().unwrap_or_default(),
                weight_units,
                weight: sig.weight,
            }
        })
        .collect();

    if validator_leaves.is_empty() {
        return Ok(Json(TransactionProofResponse {
            tx_hash: hash,
            finalized: true,
            proof_url: None,
            proof_size_bytes: None,
            qr_viable: None,
            error: Some("Checkpoint has no validators with BLS public keys".to_string()),
        }));
    }

    let validator_tree = build_merkle_sum_tree(&validator_leaves);

    // Generate membership proofs for all signers
    let membership_proofs: Vec<_> = (0..validator_leaves.len())
        .filter_map(|idx| get_merkle_sum_proof(&validator_leaves, idx))
        .collect();

    let self_proof = SelfContainedProof {
        version: 4,
        tx_hash: tx.hash.clone(),
        tx_signature: tx.signature.clone(),
        tx_from: tx.tx.from.clone(),
        tx_to: tx.tx.to.clone(),
        tx_amount: tx.tx.amount,
        tx_nonce: tx.tx.nonce,
        tx_timestamp: tx.tx.timestamp,
        checkpoint_height: checkpoint.height,
        checkpoint_id: checkpoint.hash.clone(),
        checkpoint_timestamp: checkpoint.timestamp,
        tx_merkle_root: checkpoint.tx_merkle_root.clone(),
        state_root: checkpoint.state_root.clone(),
        receipt_root: checkpoint.receipt_root.clone(),
        tip_count: checkpoint.tip_count,
        merkle_proof,
        merkle_index,
        bls_aggregated_sig: checkpoint.aggregated_signature.clone().unwrap_or_default(),
        bls_signer_bitmap: checkpoint.signer_bitmap
            .as_ref()
            .map(|b| URL_SAFE_NO_PAD.encode(b))
            .unwrap_or_default(),
        bls_signer_count: checkpoint.validator_signatures.len(),
        signer_membership_proofs: membership_proofs,
        validator_sum_tree_root: validator_tree.root,
    };

    match create_self_proof_url(&self_proof) {
        Ok(url) => {
            let size = url.len();
            let qr_viable = size <= 2953;

            Ok(Json(TransactionProofResponse {
                tx_hash: hash,
                finalized: true,
                proof_url: Some(url),
                proof_size_bytes: Some(size),
                qr_viable: Some(qr_viable),
                error: None,
            }))
        }
        Err(e) => Ok(Json(TransactionProofResponse {
            tx_hash: hash,
            finalized: true,
            proof_url: None,
            proof_size_bytes: None,
            qr_viable: None,
            error: Some(format!("Failed to encode proof: {}", e)),
        })),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RewardsAddressResponse {
    address: String,
    tip_rewards: f64,
    stake_rewards: f64,
    witness_rewards: f64,
    total_rewards: f64,
    pending_rewards: f64,
}

async fn get_rewards_address(
    State(state): State<NodeState>,
    Path(address): Path<String>,
) -> Json<RewardsAddressResponse> {
    let rewards = state.rewards.read().await;
    let summary = rewards.get_rewards_summary(&address);
    
    Json(RewardsAddressResponse {
        address: summary.address,
        tip_rewards: summary.tip_rewards,
        stake_rewards: summary.stake_rewards,
        witness_rewards: summary.witness_rewards,
        total_rewards: summary.total_rewards,
        pending_rewards: summary.pending_rewards,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReconcileStakesResponse {
    reconciled_count: usize,
    changes: Vec<StakeReconcileChange>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StakeReconcileChange {
    address: String,
    old_staked: f64,
    new_staked: f64,
}

async fn post_reconcile_stakes(
    State(state): State<NodeState>,
) -> Json<ReconcileStakesResponse> {
    let (count, changes) = state.reconcile_stakes().await;
    
    Json(ReconcileStakesResponse {
        reconciled_count: count,
        changes: changes.into_iter().map(|(addr, old, new)| StakeReconcileChange {
            address: addr,
            old_staked: old,
            new_staked: new,
        }).collect(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StakingAddressResponse {
    address: String,
    staked_amount: f64,
    staked_at: Option<u64>,
    can_unstake: bool,
    cooldown_remaining_ms: u64,
    stake_rewards_total: f64,
}

async fn get_staking_address(
    State(state): State<NodeState>,
    Path(address): Path<String>,
) -> Json<StakingAddressResponse> {
    let rewards = state.rewards.read().await;
    let status = rewards.get_staking_status(&address);
    
    Json(StakingAddressResponse {
        address: status.address,
        staked_amount: status.position.as_ref().map(|p| p.amount).unwrap_or(0.0),
        staked_at: status.position.as_ref().map(|p| p.staked_at),
        can_unstake: status.can_unstake,
        cooldown_remaining_ms: status.cooldown_remaining_ms,
        stake_rewards_total: status.stake_rewards_total,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContractListResponse {
    contracts: Vec<crate::contracts::ContractState>,
    count: usize,
}

async fn get_contracts(
    State(state): State<NodeState>,
) -> Json<ContractListResponse> {
    let contracts = state.get_all_contracts().await;
    let count = contracts.len();
    Json(ContractListResponse { contracts, count })
}

async fn get_contract(
    State(state): State<NodeState>,
    Path(contract_id): Path<String>,
) -> Result<Json<crate::contracts::ContractState>, StatusCode> {
    match state.get_contract(&contract_id).await {
        Some(contract) => Ok(Json(contract)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn get_version(State(state): State<NodeState>) -> Json<VersionResponse> {
    let (chain_id, network_id) = state.get_chain_info().await;
    Json(VersionResponse {
        protocol_version: "1.0.0".to_string(),
        node_version: env!("CARGO_PKG_VERSION").to_string(),
        chain_id,
        network_id,
        features: vec![
            "dag-consensus".to_string(),
            "url-native".to_string(),
            "sled-persistence".to_string(),
            "finality-proofs".to_string(),
            "merkle-sum-tree".to_string(),
            "bls-aggregation".to_string(),
            "dynamic-gas".to_string(),
            "smart-contracts".to_string(),
            "tip-consolidation".to_string(),
            "fork-remediation".to_string(),
            "zk-privacy".to_string(),
        ],
    })
}

async fn get_metrics(State(state): State<NodeState>) -> String {
    use sysinfo::{System, Pid};
    
    let (dag_nodes, tips, accounts) = state.get_dag_stats().await;
    let checkpoint_height = state.get_checkpoint_height().await;
    let gas_price = state.get_gas_price().await;
    // Use rewards service for accurate staking data (not deprecated state.validators)
    let rewards = state.rewards.read().await;
    let validators = rewards.get_active_validators().len();
    let total_stake = rewards.get_total_staked();
    drop(rewards);
    let total_supply = state.get_total_supply().await;
    let total_transactions = state.get_total_transactions().await;
    let (finalized, unfinalized) = state.get_finalized_stats().await;
    
    let mut sys = System::new_all();
    sys.refresh_all();
    
    let pid = Pid::from_u32(std::process::id());
    let (process_memory_bytes, process_cpu_percent, process_threads) = 
        if let Some(process) = sys.process(pid) {
            (process.memory(), process.cpu_usage(), 0u64)
        } else {
            (0, 0.0, 0)
        };
    
    let total_memory = sys.total_memory();
    let used_memory = sys.used_memory();
    let cpu_count = sys.cpus().len();
    let global_cpu_percent: f32 = sys.cpus().iter().map(|c| c.cpu_usage()).sum::<f32>() / cpu_count as f32;
    
    let uptime_seconds = state.get_uptime_seconds().await;

    format!(
        r#"# HELP rinku_dag_nodes_total Total number of nodes in the DAG
# TYPE rinku_dag_nodes_total gauge
rinku_dag_nodes_total {}

# HELP rinku_dag_tips_total Current number of tips in the DAG
# TYPE rinku_dag_tips_total gauge
rinku_dag_tips_total {}

# HELP rinku_accounts_total Total number of accounts
# TYPE rinku_accounts_total gauge
rinku_accounts_total {}

# HELP rinku_transactions_total Total transactions processed
# TYPE rinku_transactions_total counter
rinku_transactions_total {}

# HELP rinku_transactions_finalized Total finalized transactions
# TYPE rinku_transactions_finalized gauge
rinku_transactions_finalized {}

# HELP rinku_transactions_unfinalized Pending unfinalized transactions
# TYPE rinku_transactions_unfinalized gauge
rinku_transactions_unfinalized {}

# HELP rinku_checkpoint_height Current checkpoint height
# TYPE rinku_checkpoint_height gauge
rinku_checkpoint_height {}

# HELP rinku_gas_price_current Current gas price in RKU
# TYPE rinku_gas_price_current gauge
rinku_gas_price_current {}

# HELP rinku_validators_active Number of active validators
# TYPE rinku_validators_active gauge
rinku_validators_active {}

# HELP rinku_stake_total Total staked RKU
# TYPE rinku_stake_total gauge
rinku_stake_total {}

# HELP rinku_supply_total Total RKU supply
# TYPE rinku_supply_total gauge
rinku_supply_total {}

# HELP process_resident_memory_bytes Process memory usage in bytes
# TYPE process_resident_memory_bytes gauge
process_resident_memory_bytes {}

# HELP process_cpu_percent Process CPU usage percentage
# TYPE process_cpu_percent gauge
process_cpu_percent {}

# HELP process_threads_total Number of threads in the process
# TYPE process_threads_total gauge
process_threads_total {}

# HELP system_memory_total_bytes Total system memory in bytes
# TYPE system_memory_total_bytes gauge
system_memory_total_bytes {}

# HELP system_memory_used_bytes Used system memory in bytes
# TYPE system_memory_used_bytes gauge
system_memory_used_bytes {}

# HELP system_cpu_count Number of CPU cores
# TYPE system_cpu_count gauge
system_cpu_count {}

# HELP system_cpu_percent_avg Average system CPU usage percentage
# TYPE system_cpu_percent_avg gauge
system_cpu_percent_avg {}

# HELP rinku_uptime_seconds Node uptime in seconds
# TYPE rinku_uptime_seconds counter
rinku_uptime_seconds {}
"#,
        dag_nodes,
        tips,
        accounts,
        total_transactions,
        finalized,
        unfinalized,
        checkpoint_height,
        gas_price,
        validators,
        total_stake,
        total_supply,
        process_memory_bytes,
        process_cpu_percent,
        process_threads,
        total_memory,
        used_memory,
        cpu_count,
        global_cpu_percent,
        uptime_seconds,
    )
}

// ============================================================================
// WEIGHT ATTESTATION ENDPOINTS
// Protocol-level trust scoring for transactions via stake-weighted validator votes
// ============================================================================

#[derive(Deserialize)]
struct WeightVoteRequest {
    vote: String, // "boost", "suppress", or "neutral"
    validator_pubkey: Option<String>,
    bls_signature: Option<String>,
}

#[derive(Serialize)]
struct WeightVoteResponse {
    success: bool,
    tx_hash: String,
    vote: String,
    message: String,
}

#[derive(Serialize)]
struct WeightProofResponse {
    tx_hash: String,
    aggregated_weight: rinku_core::types::AggregatedWeight,
    trust_score: u8,
    boost_ratio: f64,
    suppress_ratio: f64,
    checkpoint_height: Option<u64>,
    weight_trie_root: String,
    merkle_proof: Vec<String>,
    merkle_index: usize,
}

async fn post_weight_vote(
    State(api_state): State<ApiState>,
    Path(hash): Path<String>,
    Json(payload): Json<WeightVoteRequest>,
) -> impl IntoResponse {
    use rinku_core::types::{WeightVote, PendingWeightVote};
    
    let state = &api_state.node_state;
    
    info!("Received vote request for tx {}: vote={}, validator={:?}", 
        hash, payload.vote, payload.validator_pubkey);
    
    let vote = match payload.vote.to_lowercase().as_str() {
        "boost" => WeightVote::Boost,
        "suppress" => WeightVote::Suppress,
        "neutral" => WeightVote::Neutral,
        _ => {
            return (StatusCode::BAD_REQUEST, Json(WeightVoteResponse {
                success: false,
                tx_hash: hash,
                vote: payload.vote,
                message: "Invalid vote type. Use 'boost', 'suppress', or 'neutral'".to_string(),
            }));
        }
    };
    
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    
    let validator_pubkey = match payload.validator_pubkey.clone() {
        Some(pk) => pk,
        None => {
            let inner = state.inner.read().await;
            inner.node_validator_address.clone().unwrap_or_default()
        }
    };
    
    let vote_str = payload.vote.to_lowercase();
    let bls_sig = payload.bls_signature.clone();
    
    let pending_vote = PendingWeightVote {
        tx_hash: hash.clone(),
        validator_pubkey: validator_pubkey.clone(),
        vote: vote.clone(),
        timestamp_ms: now_ms,
        bls_signature: bls_sig.clone(),
    };
    
    {
        let mut inner = state.inner.write().await;
        if let Some(ref mut wt) = inner.weight_trie {
            wt.add_vote(pending_vote.clone());
            info!("Vote registered for tx {}: vote={:?}, validator={}", 
                hash, pending_vote.vote, pending_vote.validator_pubkey);
        } else {
            warn!("Weight trie not available - vote not registered for tx {}", hash);
            return (StatusCode::SERVICE_UNAVAILABLE, Json(WeightVoteResponse {
                success: false,
                tx_hash: hash,
                vote: payload.vote,
                message: "Weight attestation system not enabled on this node".to_string(),
            }));
        }
    }
    
    // Broadcast vote to peers so all nodes have it for checkpoint aggregation
    if let Some(ref gossip_service) = api_state.gossip_service {
        gossip_service.broadcast_weight_vote(
            hash.clone(),
            validator_pubkey,
            vote_str,
            now_ms,
            bls_sig,
        ).await;
    }
    
    (StatusCode::OK, Json(WeightVoteResponse {
        success: true,
        tx_hash: hash,
        vote: payload.vote,
        message: "Vote registered and broadcast to network. Will be aggregated at next checkpoint.".to_string(),
    }))
}

async fn get_weight_proof(
    State(state): State<NodeState>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    let inner = state.inner.read().await;
    
    let weight_trie = match &inner.weight_trie {
        Some(wt) => wt.clone(),
        None => {
            return (StatusCode::NOT_FOUND, Json(serde_json::json!({
                "error": "Weight attestation system not enabled"
            }))).into_response();
        }
    };
    
    let weight = match weight_trie.get_weight(&hash) {
        Some(w) => w.clone(),
        None => {
            return (StatusCode::OK, Json(WeightProofResponse {
                tx_hash: hash,
                aggregated_weight: rinku_core::types::AggregatedWeight::default(),
                trust_score: 50,
                boost_ratio: 0.0,
                suppress_ratio: 0.0,
                checkpoint_height: inner.checkpoints.last().map(|c| c.height),
                weight_trie_root: String::new(),
                merkle_proof: vec![],
                merkle_index: 0,
            })).into_response();
        }
    };
    
    let mut wt = weight_trie.clone();
    let (proof, index, _leaf) = wt.generate_proof(&hash).unwrap_or((vec![], 0, rinku_core::types::WeightTrieLeaf {
        tx_hash: hash.clone(),
        boost_stake_micro: 0,
        suppress_stake_micro: 0,
        neutral_stake_micro: 0,
        total_network_stake_micro: 0,
        attestation_count: 0,
    }));
    let root = wt.compute_root();
    
    (StatusCode::OK, Json(WeightProofResponse {
        tx_hash: hash,
        trust_score: weight.trust_score(),
        boost_ratio: weight.boost_ratio(),
        suppress_ratio: weight.suppress_ratio(),
        aggregated_weight: weight,
        checkpoint_height: inner.checkpoints.last().map(|c| c.height),
        weight_trie_root: root,
        merkle_proof: proof,
        merkle_index: index,
    })).into_response()
}

async fn get_tx_weight(
    State(state): State<NodeState>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    let inner = state.inner.read().await;
    
    let weight = inner.weight_trie.as_ref()
        .and_then(|wt| wt.get_weight(&hash).cloned())
        .unwrap_or_default();
    
    Json(serde_json::json!({
        "tx_hash": hash,
        "trust_score": weight.trust_score(),
        "boost_ratio": weight.boost_ratio(),
        "suppress_ratio": weight.suppress_ratio(),
        "boost_stake": weight.boost_stake_micro as f64 / 100_000_000.0,
        "suppress_stake": weight.suppress_stake_micro as f64 / 100_000_000.0,
        "net_weight": weight.net_weight,
        "attestation_count": weight.attestation_count,
    }))
}

pub async fn start_api_server(
    state: NodeState,
    gossip_service: Option<Arc<GossipService>>,
    port: u16,
    static_dir: Option<PathBuf>,
) -> anyhow::Result<JoinHandle<()>> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Create ApiState for gossip routes
    let api_state = ApiState {
        node_state: state.clone(),
        gossip_service,
    };

    // Routes that need ApiState (gossip + transaction submission + faucet for broadcasting)
    let gossip_routes = Router::new()
        .route("/api/gossip", post(post_gossip))
        .route("/api/gossip/stats", get(get_gossip_stats))
        .route("/api/peers", get(get_peers))
        .route("/api/slashing/evidence", post(post_slashing_evidence))
        .route("/api/tx", post(submit_transaction))
        .route("/api/tx/fast", post(submit_fast_path_transaction))
        .route("/api/tx/fast/:hash", get(get_fast_path_status))
        .route("/api/tx/batch", post(submit_batch_transaction))
        .route("/api/request", post(handle_faucet_request))
        .route("/api/faucet/request", post(handle_faucet_request))
        .route("/api/sync/delta", post(post_sync_delta))
        .route("/api/dag", get(get_dag))
        .route("/api/tx/:hash", get(get_transaction))
        .route("/api/tx/:hash/replies", get(get_transaction_replies))
        .route("/api/account/:address/transactions", get(get_account_transactions_with_fast_path))
        .route("/api/finality/metrics", get(get_finality_metrics))
        .route("/api/tx/:hash/vote", post(post_weight_vote))
        .layer(cors.clone())
        .with_state(api_state);

    // Routes that use NodeState
    let node_routes = Router::new()
        .route("/health", get(health))
        .route("/api/status", get(get_node_status))
        .route("/api/stats", get(get_stats))
        .route("/api/faucet/stats", get(get_faucet_stats))
        .route("/api/tips", get(get_tips))
        .route("/api/tipUrls", get(get_tip_urls))
        .route("/api/account/:address", get(get_account))
        .route("/api/account/:address/proof", get(get_account_proof))
        .route("/api/account/:address/proof/current", get(get_account_proof_current))
        .route("/api/tx/:hash/receipt", get(get_transaction_receipt))
        .route("/api/txp/:hash", get(get_self_provable_tx))
        .route("/api/tx/:hash/proof", get(generate_transaction_proof))
        .route("/api/tx/:hash/weight", get(get_tx_weight))
        .route("/api/tx/:hash/weight-proof", get(get_weight_proof))
        .route("/api/dag/summary", get(get_dag_summary))
        .route("/api/accounts", get(get_accounts))
        .route("/api/network/stats", get(get_network_stats))
        .route("/api/gas/price", get(get_gas_price))
        .route("/api/gas/stats", get(get_gas_stats))
        .route("/api/version", get(get_version))
        .route("/api/staking", get(get_staking))
        .route("/api/staking/:address", get(get_staking_address))
        .route("/api/contracts", get(get_contracts))
        .route("/api/contracts/:contract_id", get(get_contract))
        .route("/api/tokenomics/supply", get(get_tokenomics_supply))
        .route("/api/tokenomics/emission", get(get_tokenomics_emission))
        .route("/api/tokenomics/slashing", get(get_tokenomics_slashing))
        .route("/api/rewards/config", get(get_rewards_config))
        .route("/api/rewards/:address", get(get_rewards_address))
        .route("/api/admin/reconcile-stakes", post(post_reconcile_stakes))
        .route("/api/checkpoints", get(get_checkpoints))
        .route("/api/checkpoints/latest", get(get_checkpoints_latest))
        .route("/api/checkpoints/:height", get(get_checkpoint_by_height))
        .route("/api/fork/stats", get(get_fork_stats))
        .route("/api/sync/status", get(get_sync_status))
        .route("/api/bootstrap", get(get_bootstrap_info))
        .route("/api/sync/bootstrap", post(post_bootstrap))
        .route("/api/sync/snapshot", get(get_snapshot_sync))
        .route("/api/sync/merge-accounts", post(post_merge_accounts))
        .route("/api/sync/transactions", get(get_batch_transactions))
        .route("/api/sync/delta", get(get_sync_transactions))
        .route("/api/verify-proof", post(verify_proof_endpoint))
        .route("/api/tip-consolidator/stats", get(get_tip_consolidator_stats))
        .route("/metrics", get(get_metrics))
        .layer(cors.clone())
        .with_state(state);

    // Merge both routers
    let api_routes = gossip_routes.merge(node_routes);

    // Root health check handler for API-only mode
    async fn root_health() -> impl IntoResponse {
        Json(serde_json::json!({ "status": "ok", "mode": "api-only" }))
    }

    let app = if let Some(static_path) = static_dir {
        if static_path.exists() {
            let index_path = static_path.join("index.html");
            let serve_dir = ServeDir::new(&static_path)
                .not_found_service(ServeFile::new(&index_path));
            info!("Serving static files from {:?} with SPA routing fallback to {:?}", static_path, index_path);
            api_routes.fallback_service(serve_dir)
        } else {
            info!("Static directory {:?} not found, API-only mode with root health check", static_path);
            api_routes.route("/", get(root_health))
        }
    } else {
        info!("No STATIC_DIR configured, API-only mode with root health check");
        api_routes.route("/", get(root_health))
    };

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("API server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    let handle = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service()).await.unwrap();
    });

    Ok(handle)
}
