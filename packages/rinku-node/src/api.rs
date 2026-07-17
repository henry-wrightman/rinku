use axum::{
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderValue, Method, StatusCode},
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
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::{info, warn};

use crate::gossip::GossipService;
use crate::http_rate_limit::HttpRateLimiters;
use crate::network::CheckpointData;
use crate::state::TransactionResult;
use crate::sync_verification::build_account_merkle_root_sorted;
use rinku_core::types::{from_micro_units, to_micro_units};

static FAUCET_RATE_LIMIT: std::sync::LazyLock<Mutex<HashMap<String, u64>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
const FAUCET_AMOUNT: u64 = 10_000_000_000;
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
    pub event_bus: Arc<crate::events::EventBus>,
    pub rate_limits: Arc<HttpRateLimiters>,
    pub faucet_enabled: bool,
}

fn client_key(addr: Option<ConnectInfo<SocketAddr>>) -> String {
    addr.map(|ConnectInfo(a)| a.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn rate_limited_response(kind: &str, max: usize, window_secs: u64) -> axum::response::Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(serde_json::json!({
            "error": format!(
                "Rate limited ({kind}): max {max} requests per {window_secs}s"
            ),
            "success": false,
        })),
    )
        .into_response()
}

fn build_read_cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::HEAD, Method::OPTIONS])
        .allow_headers(Any)
}

fn build_write_cors(origins: &[String]) -> CorsLayer {
    let allow_any = origins.iter().any(|o| o.trim() == "*");
    let mut layer = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(Any);

    if allow_any {
        layer = layer.allow_origin(Any);
    } else {
        let parsed: Vec<HeaderValue> = origins
            .iter()
            .filter_map(|o| HeaderValue::from_str(o.trim()).ok())
            .collect();
        if parsed.is_empty() {
            // No origins configured — deny browser cross-origin writes (non-browser clients unaffected).
            layer = layer.allow_origin(AllowOrigin::list(Vec::<HeaderValue>::new()));
        } else {
            layer = layer.allow_origin(AllowOrigin::list(parsed));
        }
    }
    layer
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
        Self {
            error: message.into(),
            code: 404,
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
            code: 400,
        }
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
    effective_nonce: u64,
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
    lane: String,
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
    #[serde(default)]
    data: Option<String>,
}

#[derive(Deserialize)]
struct SubmitTxRequest {
    tx: TxInner,
    #[serde(default, alias = "publicKey")]
    public_key: Option<PublicKeyField>,
}

/// Accepts either a byte array (wallet) or hex string (explorer).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PublicKeyField {
    Bytes(Vec<u8>),
    Hex(String),
}

impl PublicKeyField {
    fn to_hex(&self) -> Result<String, String> {
        match self {
            PublicKeyField::Bytes(b) => {
                if b.is_empty() {
                    return Err("empty publicKey".into());
                }
                Ok(hex::encode(b))
            }
            PublicKeyField::Hex(h) => {
                let cleaned = h.trim().strip_prefix("0x").unwrap_or(h.trim());
                hex::decode(cleaned).map_err(|_| "invalid publicKey hex".to_string())?;
                Ok(cleaned.to_lowercase())
            }
        }
    }
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
    sig: String,
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
    lane: String,
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
    tps_short: f64,
    tps_long: f64,
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
    node_version: String,
    protocol_version: String,
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
    node_version: String,
    protocol_version: String,
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
        version: crate::versioning::NODE_VERSION.to_string(),
    })
}

async fn get_sync_status(State(state): State<NodeState>) -> Json<SyncStatusResponse> {
    let tips = state.get_tips().await;
    let (dag_size, _, _) = state.get_dag_stats().await;
    let checkpoint_height = state.get_checkpoint_height();
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
        total_stake: from_micro_units(total_stake),
        uptime_seconds,
        is_syncing: false,
        faucet_balance: from_micro_units(faucet_balance),
        node_version: crate::versioning::NODE_VERSION.to_string(),
        protocol_version: crate::versioning::PROTOCOL_VERSION.to_string(),
    })
}

async fn get_bootstrap_info(State(state): State<NodeState>) -> Json<BootstrapInfoResponse> {
    let (peer_id, listen_addr, validator_address, bls_public_key) =
        state.get_bootstrap_info().await;

    // Build bootstrap multiaddr for P2P_BOOTSTRAP_PEERS env var
    let bootstrap_multiaddr = match (&peer_id, &listen_addr) {
        (Some(pid), Some(addr)) => {
            // Parse listen_addr to extract port, use placeholder for external IP
            let port = addr
                .split("/tcp/")
                .nth(1)
                .and_then(|s| s.split('/').next())
                .unwrap_or("4001");
            Some(format!("/ip4/<PUBLIC_IP>/tcp/{}/p2p/{}", port, pid))
        }
        _ => None,
    };

    // Build GENESIS_VALIDATORS env var format: address:bls_base64url
    // Only include if both validator address and valid BLS key exist
    let genesis_validator_env = match (&validator_address, &bls_public_key) {
        (Some(addr), Some(bls_key)) if !bls_key.is_empty() => Some(format!("{}:{}", addr, bls_key)),
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
    let checkpoint_height = state.get_checkpoint_height();
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
        total_stake: from_micro_units(total_stake),
        uptime_seconds,
        node_version: crate::versioning::NODE_VERSION.to_string(),
        protocol_version: crate::versioning::PROTOCOL_VERSION.to_string(),
    })
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
    if !api_state
        .node_state
        .verify_slashing_evidence(&evidence)
        .await
    {
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

    info!(
        "Bootstrap request: from_checkpoint={}, limit={}",
        from_checkpoint, limit
    );

    let all_txs = state.get_txs_since_checkpoint(from_checkpoint, &[]).await;
    let total_available = all_txs.len();
    let checkpoint_height = state.get_checkpoint_height();

    let transactions: Vec<_> = all_txs.into_iter().take(limit).collect();
    let has_more = total_available > limit;

    info!(
        "Bootstrap response: {} txs (total={}, has_more={})",
        transactions.len(),
        total_available,
        has_more
    );

    Json(BootstrapResponse {
        transactions,
        checkpoint_height,
        total_available,
        has_more,
    })
}

async fn get_snapshot_sync(
    State(state): State<NodeState>,
) -> Result<Json<SnapshotSyncResponse>, (StatusCode, String)> {
    // Concurrency limit to prevent API overload during sync storms
    let current = ACTIVE_SYNC_REQUESTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if current >= MAX_CONCURRENT_SYNC_REQUESTS {
        ACTIVE_SYNC_REQUESTS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        warn!(
            "Sync request rejected: too many concurrent requests ({}/{})",
            current + 1,
            MAX_CONCURRENT_SYNC_REQUESTS
        );
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Too many sync requests, try again later".to_string(),
        ));
    }

    // Decrement on scope exit
    let _guard = SyncRequestGuard;

    info!("Snapshot sync request received");

    let snapshot = state.get_sync_snapshot().await;
    let checkpoint_height = state.get_checkpoint_height();

    info!(
        "Snapshot sync response: {} accounts, {} validators, {} checkpoints, {} dag txs",
        snapshot.accounts.len(),
        snapshot.validators.len(),
        snapshot.checkpoints.len(),
        snapshot.dag_transactions.len()
    );

    let mut account_data: Vec<crate::network::AccountData> = snapshot
        .accounts
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
        gas_price: from_micro_units(snapshot.gas_price),
        total_supply: from_micro_units(snapshot.total_supply),
        genesis_time: snapshot.genesis_time,
        dag_transactions: snapshot.dag_transactions,
        total_transactions: snapshot.total_transactions,
        checkpoint_height,
        contracts: snapshot.contracts,
        accounts_merkle_root,
        rewards_snapshot: snapshot.rewards_snapshot,
        emission_snapshot: snapshot.emission_snapshot,
        slashing_snapshot: snapshot.slashing_snapshot,
        total_burned: from_micro_units(snapshot.total_burned),
        total_to_validators: from_micro_units(snapshot.total_to_validators),
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
    info!(
        "Merge accounts request: {} accounts from peer",
        req.accounts.len()
    );

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
        warn!(
            "Delta sync request rejected: too many concurrent requests ({}/{})",
            current + 1,
            MAX_CONCURRENT_SYNC_REQUESTS
        );
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Too many sync requests, try again later".to_string(),
        ));
    }

    // Decrement on scope exit
    let _guard = SyncRequestGuard;

    let from_checkpoint = query.from_checkpoint.unwrap_or(0);
    let (new_checkpoints, tx_checkpoint_heights, to_checkpoint) = {
        let state_guard = state.inner.read().await;
        let mut tx_checkpoint_heights = std::collections::HashMap::new();
        let mut tx_count_by_checkpoint: std::collections::HashMap<u64, u64> =
            std::collections::HashMap::new();
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
                finalized_tx_hashes: cp.finalized_tx_hashes.clone(),
                state_root: Some(cp.state_root.clone()),
                receipt_root: Some(cp.receipt_root.clone()),
                tip_count: Some(cp.tip_count),
                validator_signatures: cp.validator_signatures.clone(),
                signer_bitmap: cp.signer_bitmap.clone(),
            })
            .collect();
        let to_checkpoint = state_guard
            .checkpoints
            .last()
            .map(|cp| cp.height)
            .unwrap_or(0);
        (new_checkpoints, tx_checkpoint_heights, to_checkpoint)
    };

    // Get all transactions since the given checkpoint
    let all_txs = state.get_txs_since_checkpoint(from_checkpoint, &[]).await;

    let offset = query.offset.unwrap_or(0);
    let limit = query
        .limit
        .unwrap_or(DEFAULT_SYNC_LIMIT)
        .min(MAX_SYNC_LIMIT);
    let total = all_txs.len();

    info!(
        "Sync delta request: checkpoint={}, offset={}, limit={}",
        from_checkpoint, offset, limit
    );

    // Apply pagination
    let transactions: Vec<_> = all_txs.into_iter().skip(offset).take(limit).collect();

    let returned = transactions.len();
    let has_more = offset + returned < total;

    // Collect current nonces for all unique senders in these transactions (backwards compat)
    let mut account_nonces = std::collections::HashMap::new();
    // Collect FULL account states for authoritative sync (balance + stake + nonce)
    let mut account_states = std::collections::HashMap::new();

    // Get all accounts involved in these transactions (both senders and receivers)
    let mut involved_addresses: std::collections::HashSet<String> =
        std::collections::HashSet::new();
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

    info!(
        "Returning {} of {} transactions with {} account states (offset={}, has_more={})",
        returned,
        total,
        account_states.len(),
        offset,
        has_more
    );

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
            info!(
                "Imported {} validators from delta sync requester",
                merged_count
            );
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
        warn!(
            "Delta sync request rejected: too many concurrent requests ({}/{})",
            current + 1,
            MAX_CONCURRENT_SYNC_REQUESTS
        );
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Too many sync requests, try again later".to_string(),
        ));
    }

    // Decrement on scope exit
    let _guard = SyncRequestGuard;

    let from_checkpoint = req.from_checkpoint.unwrap_or(0);
    let (new_checkpoints, tx_checkpoint_heights, to_checkpoint) = {
        let state_guard = state.inner.read().await;
        let mut tx_checkpoint_heights = std::collections::HashMap::new();
        let mut tx_count_by_checkpoint: std::collections::HashMap<u64, u64> =
            std::collections::HashMap::new();
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
                finalized_tx_hashes: cp.finalized_tx_hashes.clone(),
                state_root: Some(cp.state_root.clone()),
                receipt_root: Some(cp.receipt_root.clone()),
                tip_count: Some(cp.tip_count),
                validator_signatures: cp.validator_signatures.clone(),
                signer_bitmap: cp.signer_bitmap.clone(),
            })
            .collect();
        let to_checkpoint = state_guard
            .checkpoints
            .last()
            .map(|cp| cp.height)
            .unwrap_or(0);
        (new_checkpoints, tx_checkpoint_heights, to_checkpoint)
    };

    let all_txs = state.get_txs_since_checkpoint(from_checkpoint, &[]).await;

    let offset = req.offset.unwrap_or(0);
    let limit = req.limit.unwrap_or(DEFAULT_SYNC_LIMIT).min(MAX_SYNC_LIMIT);
    let total = all_txs.len();

    info!(
        "POST Sync delta request: checkpoint={}, offset={}, limit={}",
        from_checkpoint, offset, limit
    );

    let transactions: Vec<_> = all_txs.into_iter().skip(offset).take(limit).collect();

    let returned = transactions.len();
    let has_more = offset + returned < total;

    let mut account_nonces = std::collections::HashMap::new();
    let mut account_states = std::collections::HashMap::new();

    let mut involved_addresses: std::collections::HashSet<String> =
        std::collections::HashSet::new();
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

    info!(
        "Returning {} of {} transactions with {} account states (offset={}, has_more={})",
        returned,
        total,
        account_states.len(),
        offset,
        has_more
    );

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
    addr: Option<ConnectInfo<SocketAddr>>,
    Json(req): Json<FaucetRequest>,
) -> impl IntoResponse {
    if !api_state.faucet_enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "Faucet is disabled on this node (set FAUCET_ENABLED=true to enable)"
            })),
        )
            .into_response();
    }

    let key = client_key(addr);
    if !api_state
        .rate_limits
        .general
        .check_and_record(&format!("faucet:{key}"))
    {
        return rate_limited_response(
            "faucet",
            api_state.rate_limits.general.max(),
            api_state.rate_limits.general.window_secs(),
        );
    }

    let address = req.address.trim().to_string();

    if address.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Address required" })),
        )
            .into_response();
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
                )
                    .into_response();
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

    let nonce = state.get_effective_nonce_for("faucet").await;

    let inner_tx = rinku_core::types::Transaction {
        from: "faucet".to_string(),
        to: address.clone(),
        amount: FAUCET_AMOUNT,
        nonce,
        timestamp: now,
        parents: tip_urls,
        kind: None,
        gas_limit: None,
        gas_price: Some(0),
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
    let tx = rinku_core::types::SignedTransaction {
        hash: hash.clone(),
        ..tx
    };

    match state.add_transaction(tx.clone()).await {
        Ok(TransactionResult::Accepted) => {
            api_state
                .event_bus
                .publish(crate::events::NodeEvent::NewTransaction {
                    hash: hash.clone(),
                    from: "faucet".to_string(),
                    to: address.clone(),
                    amount: from_micro_units(FAUCET_AMOUNT),
                    kind: None,
                });
            // Broadcast via fast-path for sub-second finality
            if let Some(ref gossip) = api_state.gossip_service {
                let (validator_addr, validator_stake) = state.get_validator_info().await;
                if let (Some(addr), Some(_)) = (validator_addr, validator_stake) {
                    let stake = state.get_validator_stake(&addr).await.unwrap_or(0);
                    if stake > 0 {
                        gossip
                            .broadcast_fast_path_transaction(tx.clone(), &addr, stake)
                            .await;
                        info!(
                            "Faucet tx {} to {} broadcast via FAST-PATH",
                            &hash[..16.min(hash.len())],
                            &address[..12.min(address.len())]
                        );
                    } else {
                        gossip.broadcast_transaction(tx).await;
                        info!(
                            "Faucet tx {} to {} broadcast to peers",
                            &hash[..16.min(hash.len())],
                            &address[..12.min(address.len())]
                        );
                    }
                } else {
                    gossip.broadcast_transaction(tx).await;
                    info!(
                        "Faucet tx {} to {} broadcast to peers",
                        &hash[..16.min(hash.len())],
                        &address[..12.min(address.len())]
                    );
                }
            }
            (
                StatusCode::OK,
                Json(serde_json::json!(FaucetResponse {
                    success: true,
                    amount: from_micro_units(FAUCET_AMOUNT),
                    tx_hash: hash,
                })),
            )
                .into_response()
        }
        Ok(TransactionResult::Buffered) => {
            // Faucet transactions should never be buffered (controlled nonce)
            warn!(
                "Faucet tx {} unexpectedly buffered",
                &hash[..16.min(hash.len())]
            );
            (
                StatusCode::OK,
                Json(serde_json::json!(FaucetResponse {
                    success: true,
                    amount: from_micro_units(FAUCET_AMOUNT),
                    tx_hash: hash,
                })),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
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
    enabled: bool,
}

async fn get_faucet_stats(State(state): State<NodeState>) -> Json<FaucetStatsResponse> {
    let rate_limit_entries = {
        let rate_limit = FAUCET_RATE_LIMIT.lock().unwrap();
        rate_limit.len()
    };

    let faucet_account = state.get_account("faucet").await;
    let current_balance = from_micro_units(faucet_account.map(|a| a.balance).unwrap_or(0));
    let genesis_allocation = 1_000_000.0;
    let total_distributed = genesis_allocation - current_balance;

    Json(FaucetStatsResponse {
        rate_limit_entries,
        max_entries: 10000,
        node_url: "local".to_string(),
        genesis_allocation,
        current_balance,
        total_distributed,
        drop_amount: from_micro_units(FAUCET_AMOUNT),
        enabled: state.faucet_enabled(),
    })
}

async fn get_stats(State(state): State<NodeState>) -> Json<StatsResponse> {
    let (dag_nodes, tips, accounts) = state.get_dag_stats().await;
    let checkpoint_height = state.get_checkpoint_height();
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
        gas_price: from_micro_units(gas_price),
        total_supply: from_micro_units(total_supply),
        validators,
        total_stake: from_micro_units(total_stake),
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
        Some(account) => {
            let effective_nonce = state.get_effective_nonce_for(&account.address).await;
            let effective_balance = state.get_effective_balance_for(&account.address).await;
            (
                StatusCode::OK,
                Json(AccountResponse {
                    fingerprint: account.address,
                    balance: from_micro_units(effective_balance),
                    nonce: account.nonce,
                    effective_nonce,
                    staked: from_micro_units(account.staked),
                }),
            )
                .into_response()
        }
        None => ApiError::not_found("Account not found").into_response(),
    }
}

async fn get_account_transactions_with_fast_path(
    State(api_state): State<ApiState>,
    Path(address): Path<String>,
) -> Json<AccountTransactionsResponse> {
    let txs = api_state
        .node_state
        .get_transactions_by_address(&address, 100)
        .await;

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
                            (
                                Some(status.to_string()),
                                fp.confirmed_at_ms,
                                fp.finality_time_ms(),
                            )
                        }
                        None => {
                            if finalized {
                                (Some("finalized".to_string()), None, None)
                            } else if gossip
                                .get_all_fast_path_executed()
                                .await
                                .contains(&stx.hash)
                            {
                                (Some("confirmed".to_string()), None, None)
                            } else {
                                (None, None, None)
                            }
                        }
                    }
                } else {
                    if finalized {
                        (Some("finalized".to_string()), None, None)
                    } else {
                        (None, None, None)
                    }
                };

            let lane_str = match stx.tx.classify_lane() {
                rinku_core::types::TransactionLane::FastPath => "fast_path",
                rinku_core::types::TransactionLane::Checkpoint => "checkpoint",
            };
            result.push(AccountTransactionItem {
                hash: stx.hash.clone(),
                from: stx.tx.from.clone(),
                to: stx.tx.to.clone(),
                amount: from_micro_units(stx.tx.amount),
                timestamp: stx.tx.timestamp,
                direction,
                finalized,
                memo: stx.tx.memo.clone(),
                references: stx.tx.references.clone(),
                fast_path_status,
                fast_path_confirmed_at_ms,
                fast_path_finality_ms,
                lane: lane_str.to_string(),
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

fn account_state_proof_to_vo(
    proof: &rinku_core::types::AccountStateProof,
    is_on_demand: bool,
) -> rinku_core::stateful_receipt::VerifiableObject {
    use rinku_core::stateful_receipt::{ProofFreshness, VerifiableObject};

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    VerifiableObject::AccountProof {
        address: proof.address.clone(),
        balance_micro: proof.balance_micro,
        balance: proof.balance,
        nonce: proof.nonce,
        staked_micro: proof.staked_micro,
        staked: proof.staked,
        checkpoint_height: proof.checkpoint_height,
        checkpoint_hash: proof.checkpoint_hash.clone(),
        checkpoint_timestamp: proof.checkpoint_timestamp,
        state_root: proof.state_root.clone(),
        merkle_proof: proof.merkle_proof.clone(),
        merkle_index: proof.merkle_index,
        bls_aggregated_sig: proof.bls_aggregated_sig.clone(),
        bls_signer_bitmap: proof.bls_signer_bitmap.clone(),
        is_on_demand,
        chain_id: None,
        freshness: Some(ProofFreshness::new(
            proof.checkpoint_height,
            now_ms,
            proof.checkpoint_height,
            None,
        )),
    }
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
        let vo = account_state_proof_to_vo(&proof, false);
        let verified = crate::proofs::verify_vo(&vo).valid;
        let proof_url = crate::proofs::create_vo_url(&vo).ok();
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
                error: Some(
                    "No proof available - account may not have finalized transactions".to_string(),
                ),
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

    if let Some(proof) = state.generate_account_state_proof_on_demand(&address).await {
        let vo = account_state_proof_to_vo(&proof, true);
        let verified = crate::proofs::verify_vo(&vo).valid;
        let proof_url = crate::proofs::create_vo_url(&vo).ok();
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
                    (
                        Some(cp.height),
                        Some(cp.hash.clone()),
                        Some(cp.state_root.clone()),
                    )
                } else {
                    let latest = state.get_latest_checkpoint().await;
                    latest
                        .map(|cp| {
                            (
                                Some(cp.height),
                                Some(cp.hash.clone()),
                                Some(cp.state_root.clone()),
                            )
                        })
                        .unwrap_or((None, None, None))
                }
            } else {
                let latest = state.get_latest_checkpoint().await;
                latest
                    .map(|cp| {
                        (
                            Some(cp.height),
                            Some(cp.hash.clone()),
                            Some(cp.state_root.clone()),
                        )
                    })
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
                amount: from_micro_units(tx.tx.amount),
                fee: from_micro_units(tx.tx.gas_price.unwrap_or(0)),
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
    addr: Option<ConnectInfo<SocketAddr>>,
    Json(req): Json<SubmitTxRequest>,
) -> impl IntoResponse {
    let key = client_key(addr);
    if !api_state.rate_limits.tx.check_and_record(&key) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(SubmitTxResponse {
                success: false,
                hash: String::new(),
                error: Some(format!(
                    "Rate limited (tx): max {} requests per {}s",
                    api_state.rate_limits.tx.max(),
                    api_state.rate_limits.tx.window_secs()
                )),
                fast_path_eligible: None,
                fast_path_status: None,
            }),
        );
    }

    let tip_count = api_state.node_state.get_tip_count().await;
    let inner = &req.tx;

    // Check if this is a system/validator transaction that bypasses degraded mode
    let is_system_tx = inner.sig.starts_with("anchor-")
        || inner.from == "faucet"
        || inner.from == "genesis"
        || matches!(
            inner.kind,
            Some(rinku_core::types::TransactionKind::Consolidation)
        );

    // Check if sender is a validator (validators can submit during degraded mode)
    let is_validator_tx = api_state.node_state.is_validator(&inner.from).await;

    // Hard backpressure: reject ALL transactions when tips exceed hard limit
    if tip_count > MAX_TIPS_BACKPRESSURE {
        warn!(
            "Transaction rejected: DAG tips ({}) exceed hard backpressure threshold ({})",
            tip_count, MAX_TIPS_BACKPRESSURE
        );
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SubmitTxResponse {
                success: false,
                hash: String::new(),
                error: Some(format!(
                    "System overloaded: {} tips pending. All transactions paused. Try again later.",
                    tip_count
                )),
                fast_path_eligible: None,
                fast_path_status: None,
            }),
        );
    }

    // Graceful degradation: when tips > threshold, only allow validator/system transactions
    if tip_count > DEGRADED_MODE_THRESHOLD && !is_system_tx && !is_validator_tx {
        warn!(
            "Transaction rejected: degraded mode active ({} tips), only validator txs allowed",
            tip_count
        );
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
    let provided_pubkey = match req.public_key {
        Some(pk) => match pk.to_hex() {
            Ok(h) => Some(h),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(SubmitTxResponse {
                        success: false,
                        hash: String::new(),
                        error: Some(e),
                        fast_path_eligible: None,
                        fast_path_status: None,
                    }),
                );
            }
        },
        None => None,
    };
    let tx = rinku_core::types::SignedTransaction {
        tx: rinku_core::types::Transaction {
            from: inner.from,
            to: inner.to,
            amount: to_micro_units(inner.amount),
            nonce: inner.nonce,
            timestamp: inner.ts,
            parents: inner.parents.clone(),
            kind: inner.kind.clone(),
            gas_limit: None,
            gas_price: Some(to_micro_units(inner.fee)),
            data: inner.data.clone(),
            signature: Some(inner.sig.clone()),
            memo: inner.memo.clone(),
            references: inner.references.clone(),
        },
        hash: inner.hash.clone(),
        signature: inner.sig.clone(),
    };

    let is_fast_path_eligible = tx.is_fast_path_eligible();

    match api_state
        .node_state
        .add_transaction_authenticated(tx.clone(), provided_pubkey)
        .await
    {
        Ok(TransactionResult::Accepted) => {
            api_state
                .event_bus
                .publish(crate::events::NodeEvent::NewTransaction {
                    hash: inner.hash.clone(),
                    from: tx.tx.from.clone(),
                    to: tx.tx.to.clone(),
                    amount: from_micro_units(tx.tx.amount),
                    kind: tx.tx.kind.as_ref().map(|k| format!("{:?}", k)),
                });
            // Broadcast to peers after successful local add
            if let Some(ref gossip) = api_state.gossip_service {
                let (validator_addr, _) = api_state.node_state.get_validator_info().await;
                let validator_stake = if let Some(ref addr) = validator_addr {
                    api_state
                        .node_state
                        .get_validator_stake(addr)
                        .await
                        .unwrap_or(0)
                } else {
                    0
                };

                if let Some(addr) = validator_addr {
                    gossip
                        .broadcast_fast_path_transaction(tx.clone(), &addr, validator_stake)
                        .await;
                    info!(
                        "Transaction {} broadcast via fast-path protocol",
                        &inner.hash[..16.min(inner.hash.len())]
                    );
                } else {
                    gossip.broadcast_transaction(tx).await;
                    info!(
                        "Transaction {} broadcast to peers (no validator identity)",
                        &inner.hash[..16.min(inner.hash.len())]
                    );
                }
            }
            (
                StatusCode::OK,
                Json(SubmitTxResponse {
                    success: true,
                    hash: inner.hash,
                    error: None,
                    fast_path_eligible: Some(is_fast_path_eligible),
                    fast_path_status: if is_fast_path_eligible {
                        Some("pending".to_string())
                    } else {
                        None
                    },
                }),
            )
        }
        Ok(TransactionResult::Buffered) => {
            // Transaction was buffered - still return success but don't broadcast yet
            info!(
                "Transaction {} buffered (future nonce)",
                &inner.hash[..16.min(inner.hash.len())]
            );
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
    addr: Option<ConnectInfo<SocketAddr>>,
    Json(req): Json<SubmitTxRequest>,
) -> impl IntoResponse {
    let key = client_key(addr);
    if !api_state.rate_limits.tx.check_and_record(&key) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(FastPathTxResponse {
                success: false,
                hash: String::new(),
                fast_path_eligible: false,
                fast_path_status: None,
                estimated_finality_ms: None,
                error: Some(format!(
                    "Rate limited (tx): max {} requests per {}s",
                    api_state.rate_limits.tx.max(),
                    api_state.rate_limits.tx.window_secs()
                )),
            }),
        );
    }

    let provided_pubkey = match req.public_key {
        Some(pk) => match pk.to_hex() {
            Ok(h) => Some(h),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(FastPathTxResponse {
                        success: false,
                        hash: String::new(),
                        fast_path_eligible: false,
                        fast_path_status: None,
                        estimated_finality_ms: None,
                        error: Some(e),
                    }),
                );
            }
        },
        None => None,
    };
    let inner = req.tx;
    let tx = rinku_core::types::SignedTransaction {
        tx: rinku_core::types::Transaction {
            from: inner.from.clone(),
            to: inner.to.clone(),
            amount: to_micro_units(inner.amount),
            nonce: inner.nonce,
            timestamp: inner.ts,
            parents: inner.parents.clone(),
            kind: inner.kind.clone(),
            gas_limit: None,
            gas_price: Some(to_micro_units(inner.fee)),
            data: inner.data.clone(),
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
                error: Some(
                    "Transaction not eligible for fast-path (must be data-only with amount=0)"
                        .to_string(),
                ),
            }),
        );
    }

    match api_state
        .node_state
        .add_transaction_authenticated(tx.clone(), provided_pubkey)
        .await
    {
        Ok(TransactionResult::Accepted) | Ok(TransactionResult::Buffered) => {
            if let Some(ref gossip) = api_state.gossip_service {
                let (validator_addr, _) = api_state.node_state.get_validator_info().await;
                let validator_stake = if let Some(ref addr) = validator_addr {
                    api_state
                        .node_state
                        .get_validator_stake(addr)
                        .await
                        .unwrap_or(0)
                } else {
                    0
                };

                if let Some(addr) = validator_addr {
                    gossip
                        .broadcast_fast_path_transaction(tx.clone(), &addr, validator_stake)
                        .await;
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
            let quorum_percent = if finality.quorum_stake_required > 0 {
                (finality.total_stake_acked * 100 / finality.quorum_stake_required) as u32
            } else {
                0
            };
            let finality_time = finality.finality_time_ms();
            return (
                StatusCode::OK,
                Json(FastPathStatusResponse {
                    hash: finality.tx_hash,
                    status: format!("{:?}", finality.status).to_lowercase(),
                    aggregated_stake: from_micro_units(finality.total_stake_acked),
                    quorum_threshold: from_micro_units(finality.quorum_stake_required),
                    quorum_percent,
                    ack_count: finality.acks.len(),
                    finality_time_ms: finality_time,
                }),
            );
        }

        // Fallback: check fast-path executed set and DAG finalization
        let fp_executed = gossip.get_all_fast_path_executed().await.contains(&hash);
        let is_finalized = {
            let state = api_state.node_state.inner.read().await;
            state
                .dag
                .get_node(&hash)
                .map(|n| n.finalized)
                .unwrap_or(false)
        };
        let derived_status = if is_finalized {
            "finalized"
        } else if fp_executed {
            "confirmed"
        } else {
            "pending"
        };
        return (
            StatusCode::OK,
            Json(FastPathStatusResponse {
                hash,
                status: derived_status.to_string(),
                aggregated_stake: 0.0,
                quorum_threshold: 0.0,
                quorum_percent: 0,
                ack_count: 0,
                finality_time_ms: None,
            }),
        );
    }

    // No gossip service - derive from DAG state only
    let is_finalized = {
        let state = api_state.node_state.inner.read().await;
        state
            .dag
            .get_node(&hash)
            .map(|n| n.finalized)
            .unwrap_or(false)
    };
    (
        StatusCode::OK,
        Json(FastPathStatusResponse {
            hash,
            status: if is_finalized {
                "finalized".to_string()
            } else {
                "pending".to_string()
            },
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
    addr: Option<ConnectInfo<SocketAddr>>,
    Json(req): Json<BatchSubmitTxRequest>,
) -> Result<Json<BatchSubmitTxResponse>, (StatusCode, String)> {
    let key = client_key(addr);
    if !api_state.rate_limits.tx.check_and_record(&key) {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "Rate limited (tx): max {} requests per {}s",
                api_state.rate_limits.tx.max(),
                api_state.rate_limits.tx.window_secs()
            ),
        ));
    }

    let tip_count = api_state.node_state.get_tip_count().await;

    // Hard backpressure: reject ALL batch transactions when tips exceed hard limit
    if tip_count > MAX_TIPS_BACKPRESSURE {
        warn!(
            "Batch transaction rejected: DAG tips ({}) exceed hard backpressure threshold ({})",
            tip_count, MAX_TIPS_BACKPRESSURE
        );
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "System overloaded: {} tips pending. All transactions paused. Try again later.",
                tip_count
            ),
        ));
    }

    // Graceful degradation: batch transactions are typically from regular users, reject in degraded mode
    if tip_count > DEGRADED_MODE_THRESHOLD {
        warn!(
            "Batch transaction rejected: degraded mode active ({} tips)",
            tip_count
        );
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "Network congested: {} tips pending. Batch submissions paused. Try again in 30s.",
                tip_count
            ),
        ));
    }

    let total = req.transactions.len();

    // Pre-convert all transactions outside of any locks
    let mut successful = 0usize;
    let mut failed = 0usize;
    let mut txs_for_broadcast = Vec::new();

    for item in req.transactions {
        let pubkey = item.public_key.as_ref().map(|b| hex::encode(b));
        let inner = item.tx;
        let tx = rinku_core::types::SignedTransaction {
            tx: rinku_core::types::Transaction {
                from: inner.from,
                to: inner.to,
                amount: to_micro_units(inner.amount),
                nonce: inner.nonce,
                timestamp: inner.ts,
                parents: inner.parents,
                kind: inner.kind,
                gas_limit: None,
                gas_price: Some(to_micro_units(inner.fee)),
                data: inner.data,
                signature: Some(inner.sig.clone()),
                memo: inner.memo,
                references: inner.references,
            },
            hash: inner.hash,
            signature: inner.sig,
        };

        match api_state
            .node_state
            .add_transaction_authenticated(tx.clone(), pubkey)
            .await
        {
            Ok(_) => {
                successful += 1;
                txs_for_broadcast.push(tx);
            }
            Err(_) => {
                failed += 1;
            }
        }
    }

    if let Some(ref gossip) = api_state.gossip_service {
        let (validator_addr, _) = api_state.node_state.get_validator_info().await;
        let validator_stake = if let Some(ref addr) = validator_addr {
            api_state
                .node_state
                .get_validator_stake(addr)
                .await
                .unwrap_or(0)
        } else {
            0
        };

        for tx in &txs_for_broadcast {
            if let Some(ref addr) = validator_addr {
                if validator_stake > 0 {
                    gossip
                        .broadcast_fast_path_transaction(tx.clone(), addr, validator_stake)
                        .await;
                } else {
                    gossip.broadcast_transaction(tx.clone()).await;
                }
            } else {
                gossip.broadcast_transaction(tx.clone()).await;
            }
        }
        if successful > 0 {
            info!(
                "Broadcast {} transactions to peers via fast-path",
                successful
            );
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

    // Get fast-path executed set for fallback status derivation
    let fp_executed: std::collections::HashSet<String> =
        if let Some(ref gossip) = api_state.gossip_service {
            gossip.get_all_fast_path_executed().await
        } else {
            std::collections::HashSet::new()
        };

    // Batch lookup trust scores from weight_trie
    let trust_scores: std::collections::HashMap<String, (u8, u32)> = {
        let inner = state.inner.read().await;
        let mut scores = std::collections::HashMap::new();
        if let Some(ref weight_trie) = inner.weight_trie {
            for hash in &hashes {
                if let Some(weight) = weight_trie.get_weight(hash) {
                    scores.insert(
                        hash.clone(),
                        (weight.trust_score(), weight.attestation_count),
                    );
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
            let parents: Vec<String> = n
                .parents
                .iter()
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
            // Priority: gossip tracking > finalized flag > fast-path executed set > pending
            let (fast_path_status, fast_path_confirmed_at_ms, fast_path_finality_ms) =
                if let Some(finality) = fast_path_statuses.get(&n.hash) {
                    (
                        Some(format!("{:?}", finality.status).to_lowercase()),
                        finality.confirmed_at_ms,
                        finality.finality_time_ms(),
                    )
                } else if n.finalized {
                    (Some("finalized".to_string()), None, None)
                } else if fp_executed.contains(&n.hash) {
                    (Some("confirmed".to_string()), None, None)
                } else {
                    (Some("pending".to_string()), None, None)
                };

            // Get trust score and attestation count
            let (trust_score, attestation_count) = trust_scores
                .get(&n.hash)
                .map(|(score, count)| (Some(*score), Some(*count)))
                .unwrap_or((None, None));

            let lane = if let Some(ref k) = n.kind {
                match k {
                    rinku_core::types::TransactionKind::Contract => "checkpoint",
                    _ => "fast_path",
                }
            } else {
                "fast_path"
            };
            DagNodeResponse {
                hash: n.hash,
                from: n.from,
                to: n.to,
                amount: from_micro_units(n.amount),
                fee: from_micro_units(n.fee),
                nonce: n.nonce,
                ts: n.ts,
                parent_count: parents.len(),
                parents,
                finalized: n.finalized,
                weight: n.weight,
                url,
                sig: n.sig,
                kind: n.kind,
                fast_path_status,
                fast_path_confirmed_at_ms,
                fast_path_finality_ms,
                trust_score,
                attestation_count,
                lane: lane.to_string(),
            }
        })
        .collect();
    Json(DagResponse { has_more, nodes })
}

async fn get_accounts(State(state): State<NodeState>) -> Json<AccountsResponse> {
    let accounts = state.get_all_accounts_with_effective().await;
    Json(AccountsResponse {
        accounts: accounts
            .into_iter()
            .map(|(a, eff_nonce, eff_balance)| AccountResponse {
                fingerprint: a.address.clone(),
                balance: from_micro_units(eff_balance),
                effective_nonce: eff_nonce,
                nonce: a.nonce,
                staked: from_micro_units(a.staked),
            })
            .collect(),
    })
}

async fn get_network_partition(State(state): State<NodeState>) -> Json<serde_json::Value> {
    let ps = state.get_partition_state().await;
    Json(serde_json::json!({
        "status": format!("{:?}", ps.status),
        "current_epoch": ps.current_epoch,
        "epoch_start_checkpoint": ps.epoch_start_checkpoint,
        "epoch_start_timestamp": ps.epoch_start_timestamp,
        "visible_validators": ps.visible_validators,
        "visible_stake_pct": ps.visible_stake_pct,
        "suspected_since": ps.suspected_since,
        "total_epochs": ps.total_epochs,
    }))
}

async fn get_merge_report_latest(State(state): State<NodeState>) -> Json<serde_json::Value> {
    match state.get_latest_merge_report().await {
        Some(report) => Json(serde_json::to_value(report).unwrap_or(serde_json::Value::Null)),
        None => Json(serde_json::json!({"status": "no_merge_report"})),
    }
}

async fn get_merge_report_by_epoch(
    State(state): State<NodeState>,
    axum::extract::Path(epoch): axum::extract::Path<u64>,
) -> Json<serde_json::Value> {
    match state.get_merge_report_by_epoch(epoch).await {
        Some(report) => Json(serde_json::to_value(report).unwrap_or(serde_json::Value::Null)),
        None => Json(serde_json::json!({"status": "not_found", "epoch": epoch})),
    }
}

async fn get_merge_history(State(state): State<NodeState>) -> Json<serde_json::Value> {
    let history = state.get_merge_history().await;
    Json(serde_json::json!({
        "merge_count": history.len(),
        "merges": history,
    }))
}

async fn post_partition_merge(
    State(api_state): State<ApiState>,
    axum::extract::Json(body): axum::extract::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let fork_point = body.get("fork_point").and_then(|v| v.as_u64()).unwrap_or(0);

    if let Some(ref gossip) = api_state.gossip_service {
        gossip.send_merge_payload(fork_point).await;
        Json(serde_json::json!({
            "status": "merge_triggered",
            "fork_point": fork_point,
        }))
    } else {
        Json(serde_json::json!({
            "status": "error",
            "message": "gossip service not available",
        }))
    }
}

async fn post_partition_budget(
    State(state): State<NodeState>,
    axum::extract::Json(body): axum::extract::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let address = body.get("address").and_then(|v| v.as_str()).unwrap_or("");
    if address.is_empty() {
        return Json(serde_json::json!({"status": "error", "message": "address is required"}));
    }

    let budget = body
        .get("budget")
        .and_then(|v| v.as_f64())
        .map(|b| rinku_core::types::to_micro_units(b));

    if state.set_partition_budget(address, budget).await {
        Json(serde_json::json!({
            "status": "ok",
            "address": address,
            "partition_budget": budget,
        }))
    } else {
        Json(serde_json::json!({
            "status": "error",
            "message": "account not found",
        }))
    }
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
    let (tps, tps_short, tps_long) = state.get_dynamic_tps().await;
    Json(NetworkStatsResponse {
        tps,
        tps_short,
        tps_long,
        total_transactions_processed: stats.total_transactions as usize,
        finalized_count: stats.finalized_count,
        unfinalized_count: stats.unfinalized_count,
        finality_ratio,
        checkpoint_count: stats.checkpoint_height,
        latest_checkpoint_height: stats.checkpoint_height,
        latest_checkpoint_id: stats.latest_checkpoint_id,
        total_staked: from_micro_units(total_stake),
        validator_count: validators,
        network_age: elapsed_secs as u64,
    })
}

async fn get_gas_price(State(state): State<NodeState>) -> Json<GasPriceResponse> {
    let (current, total_burned, _, avg) = state.get_gas_stats().await;
    Json(GasPriceResponse {
        current: from_micro_units(current),
        min: 0.001,
        max: 10.0,
        avg_last_100: from_micro_units(avg),
        total_burned: from_micro_units(total_burned),
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
        total_burned: from_micro_units(total_burned),
        total_to_validators: from_micro_units(total_to_validators),
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
    let tx_throughput = if total_transactions > 0 {
        (total_transactions as f64) / 60.0
    } else {
        0.0
    };

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
    sig: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<rinku_core::types::TransactionKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
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
    lane: String,
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
        let tip_urls: Vec<String> = tx
            .tx
            .parents
            .iter()
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
                    let _is_confirmed = matches!(
                        fp.status,
                        rinku_core::types::FastPathStatus::Confirmed
                            | rinku_core::types::FastPathStatus::Finalized
                    );
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

        let redacted_data = tx.tx.data.clone();

        let lane_str = match tx.tx.classify_lane() {
            rinku_core::types::TransactionLane::FastPath => "fast_path",
            rinku_core::types::TransactionLane::Checkpoint => "checkpoint",
        };
        Ok(Json(TransactionResponse {
            hash: tx.hash.clone(),
            from: tx.tx.from.clone(),
            to: tx.tx.to.clone(),
            amount: from_micro_units(tx.tx.amount),
            fee: from_micro_units(tx.tx.gas_price.unwrap_or(0)),
            nonce: tx.tx.nonce,
            ts: tx.tx.timestamp,
            tip_urls,
            finalized,
            weight,
            url: format!("/tx/h/{}", tx.hash),
            sig: tx.signature.clone(),
            kind: tx.tx.kind.clone(),
            data: redacted_data,
            memo: tx.tx.memo.clone(),
            references: tx.tx.references.clone(),
            fast_path_status,
            fast_path_confirmed_at_ms,
            fast_path_finality_ms,
            lane: lane_str.to_string(),
        }))
    } else {
        // Debug: log lookup failure with DAG stats
        let (dag_size, _, _) = state.get_dag_stats().await;
        tracing::warn!(
            "Transaction {} not found in DAG (DAG size: {})",
            hash,
            dag_size
        );
        Err(ApiError::not_found(format!(
            "Transaction {} not found (may have been pruned after finalization)",
            hash
        )))
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
    let reply_data: Vec<(
        String,
        String,
        String,
        f64,
        f64,
        u64,
        u64,
        Vec<String>,
        bool,
        f64,
        Option<String>,
        Option<Vec<String>>,
        String,
        Option<rinku_core::types::TransactionKind>,
        Option<String>,
    )> = {
        let inner = state.inner.read().await;
        let all_txs = inner.dag.all_transactions();

        all_txs
            .iter()
            .filter(|tx| {
                tx.tx
                    .references
                    .as_ref()
                    .map_or(false, |refs| refs.contains(&hash))
            })
            .map(|tx| {
                let (finalized, weight) = inner
                    .dag
                    .get_node(&tx.hash)
                    .map(|n| (n.finalized, n.weight))
                    .unwrap_or((false, 1.0));

                let tip_urls: Vec<String> = tx
                    .tx
                    .parents
                    .iter()
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
                    from_micro_units(tx.tx.amount),
                    from_micro_units(tx.tx.gas_price.unwrap_or(0)),
                    tx.tx.nonce,
                    tx.tx.timestamp,
                    tip_urls,
                    finalized,
                    weight,
                    tx.tx.memo.clone(),
                    tx.tx.references.clone(),
                    tx.signature.clone(),
                    tx.tx.kind.clone(),
                    tx.tx.data.clone(),
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

    let mut replies: Vec<TransactionResponse> = reply_data
        .into_iter()
        .map(
            |(
                tx_hash,
                from,
                to,
                amount,
                fee,
                nonce,
                ts,
                tip_urls,
                finalized,
                weight,
                memo,
                references,
                sig,
                kind,
                data,
            )| {
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

                let redacted_data = data;

                let lane_str = if let Some(ref k) = kind {
                    match k {
                        rinku_core::types::TransactionKind::Contract => "checkpoint",
                        _ => "fast_path",
                    }
                } else {
                    "fast_path"
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
                    sig,
                    kind,
                    data: redacted_data,
                    memo,
                    references,
                    fast_path_status,
                    fast_path_confirmed_at_ms,
                    fast_path_finality_ms,
                    lane: lane_str.to_string(),
                }
            },
        )
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
    let tx = state.get_transaction(&hash).await.ok_or_else(|| {
        ApiError::not_found(format!(
            "Transaction {} not found (may have been pruned)",
            hash
        ))
    })?;

    let (finalized, checkpoint_height) = state.get_finalization_info(&hash).await;

    let (merkle_proof, merkle_index, checkpoint_data, proof_url) = if finalized {
        if let Some(cp_height) = checkpoint_height {
            if let Some((proof, index, checkpoint)) = state.get_merkle_proof(&hash, cp_height).await
            {
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
        amount: from_micro_units(tx.tx.amount),
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

    let stakers: Vec<StakerInfo> = active_validators
        .iter()
        .map(|v| StakerInfo {
            staker: v.staker.clone(),
            amount: from_micro_units(v.amount),
            staked_at: v.staked_at,
        })
        .collect();

    let mut top_stakers = stakers.clone();
    top_stakers.sort_by(|a, b| {
        b.amount
            .partial_cmp(&a.amount)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    top_stakers.truncate(10);

    Json(StakingResponse {
        total_staked: from_micro_units(total_staked),
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
    let checkpoint_height = state.get_checkpoint_height();
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
        circulating_supply: from_micro_units(total_supply),
        total_emitted: from_micro_units(emission_total_emitted),
        total_burned: from_micro_units(gas_burned),
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
    let checkpoint_height = state.get_checkpoint_height();
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
        current_reward: from_micro_units(stats.current_reward),
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
            amount: from_micro_units(e.amount),
            percent_slashed: e.percent_slashed,
            checkpoint_height: e.checkpoint_height,
            timestamp: e.timestamp,
        })
        .collect();

    let unbonding_queue: Vec<UnbondingEntryResponse> = queue
        .iter()
        .map(|e| UnbondingEntryResponse {
            validator: e.validator.clone(),
            amount: from_micro_units(e.amount),
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
        total_slashed: from_micro_units(slashing.get_total_slashed()),
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
    let checkpoints: Vec<CheckpointInfo> = inner
        .checkpoints
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

    Json(CheckpointsResponse { total, checkpoints })
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChainTipResponse {
    checkpoint_height: u64,
    checkpoint_hash: String,
    checkpoint_timestamp: u64,
    state_root: String,
}

async fn get_chain_tip(State(state): State<NodeState>) -> Json<ChainTipResponse> {
    let inner = state.inner.read().await;
    if let Some(cp) = inner.checkpoints.last() {
        Json(ChainTipResponse {
            checkpoint_height: cp.height,
            checkpoint_hash: cp.hash.clone(),
            checkpoint_timestamp: cp.timestamp,
            state_root: cp.state_root.clone(),
        })
    } else {
        Json(ChainTipResponse {
            checkpoint_height: 0,
            checkpoint_hash: "genesis".to_string(),
            checkpoint_timestamp: inner.genesis_time,
            state_root: String::new(),
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
        Err(ApiError::not_found(format!(
            "Checkpoint at height {} not found",
            height
        )))
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
    peers: Vec<crate::network::PeerStats>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    http_peers: Vec<crate::gossip::PeerInfo>,
    peer_count: usize,
}

async fn get_gossip_stats(State(api_state): State<ApiState>) -> Json<GossipStatsResponse> {
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

async fn get_peers(State(api_state): State<ApiState>) -> Json<PeersResponse> {
    if let Some(ref gossip_service) = api_state.gossip_service {
        let p2p_peers = gossip_service.get_p2p_peers().await;
        let http_peers = gossip_service.get_http_peers().await;
        let peer_count = p2p_peers.len();
        Json(PeersResponse {
            peers: p2p_peers,
            http_peers,
            peer_count,
        })
    } else {
        Json(PeersResponse {
            peers: Vec::new(),
            http_peers: Vec::new(),
            peer_count: 0,
        })
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
    State(state): State<NodeState>,
    Json(req): Json<VerifyProofRequest>,
) -> impl IntoResponse {
    use crate::proofs::{
        decode_account_state_proof, decode_self_contained_proof, decode_vo,
        verify_account_state_proof_detailed, verify_self_contained_proof, verify_vo,
    };

    let proof_url = req.proof_url.trim();
    let current_checkpoint_height = state.get_checkpoint_height() as u64;

    if proof_url.starts_with("rinku://vo/") {
        match decode_vo(proof_url) {
            Ok(vo) => {
                let result = verify_vo(&vo);
                let mut freshness_json =
                    serde_json::to_value(&result.freshness).unwrap_or(serde_json::json!(null));
                if let Some(obj) = freshness_json.as_object_mut() {
                    obj.insert(
                        "currentChainTip".to_string(),
                        serde_json::json!(current_checkpoint_height),
                    );
                }
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "proofType": result.object_type,
                        "valid": result.valid,
                        "errors": result.errors,
                        "checkpointHeight": result.checkpoint_height,
                        "freshness": freshness_json,
                        "details": result.details,
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "proofType": "unknown",
                        "valid": false,
                        "error": format!("Failed to decode VerifiableObject: {}", e)
                    })),
                )
                    .into_response();
            }
        }
    }

    if proof_url.starts_with("rinku://asp/") {
        match decode_account_state_proof(proof_url) {
            Ok(proof) => {
                let vo = account_state_proof_to_vo(&proof, false);
                let result = verify_vo(&vo);
                let mut freshness_json =
                    serde_json::to_value(&result.freshness).unwrap_or(serde_json::json!(null));
                if let Some(obj) = freshness_json.as_object_mut() {
                    obj.insert(
                        "currentChainTip".to_string(),
                        serde_json::json!(current_checkpoint_height),
                    );
                }
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "proofType": result.object_type,
                        "valid": result.valid,
                        "errors": result.errors,
                        "checkpointHeight": result.checkpoint_height,
                        "freshness": freshness_json,
                        "details": result.details,
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "proofType": "account_state",
                        "valid": false,
                        "error": format!("Failed to decode account state proof: {}", e)
                    })),
                )
                    .into_response();
            }
        }
    }

    if proof_url.starts_with("rinku://sp/") {
        match decode_self_contained_proof(proof_url) {
            Ok(proof) => {
                let result = verify_self_contained_proof(&proof);
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({
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
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "proofType": "transaction",
                        "valid": false,
                        "error": format!("Failed to decode proof: {}", e)
                    })),
                )
                    .into_response();
            }
        }
    }

    (StatusCode::BAD_REQUEST, Json(serde_json::json!({
        "valid": false,
        "error": "Unrecognized proof URL scheme. Expected rinku://vo/, rinku://sp/, or rinku://asp/"
    }))).into_response()
}

async fn generate_transaction_proof(
    State(state): State<NodeState>,
    Path(hash): Path<String>,
) -> Result<Json<TransactionProofResponse>, ApiError> {
    use crate::proofs::{
        build_merkle_sum_tree, create_vo_url, get_merkle_sum_proof, MerkleSumLeaf,
    };
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use rinku_core::stateful_receipt::{ProofFreshness, VerifiableObject};

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

    let (merkle_proof, merkle_index, checkpoint) =
        match state.get_merkle_proof(&hash, cp_height).await {
            Some(data) => data,
            None => {
                return Ok(Json(TransactionProofResponse {
                    tx_hash: hash,
                    finalized: true,
                    proof_url: None,
                    proof_size_bytes: None,
                    qr_viable: None,
                    error: Some(
                        "Could not generate merkle proof (transaction may have been pruned)"
                            .to_string(),
                    ),
                }));
            }
        };

    let fast_path_cert_data = state.get_fast_path_cert(&hash).await;

    let (validator_leaves, acked_indices): (Vec<MerkleSumLeaf>, Vec<usize>) = {
        let leaves: Vec<MerkleSumLeaf> = checkpoint
            .validator_signatures
            .iter()
            .filter(|sig| sig.bls_public_key.is_some())
            .enumerate()
            .map(|(i, sig)| MerkleSumLeaf {
                index: i,
                address: sig.validator.clone(),
                bls_public_key: sig.bls_public_key.clone().unwrap_or_default(),
                weight_units: sig.weight,
                weight: from_micro_units(sig.weight),
            })
            .collect();
        let indices: Vec<usize> = (0..leaves.len()).collect();
        (leaves, indices)
    };

    if validator_leaves.is_empty() {
        return Ok(Json(TransactionProofResponse {
            tx_hash: hash,
            finalized: true,
            proof_url: None,
            proof_size_bytes: None,
            qr_viable: None,
            error: Some("No validators available for proof generation".to_string()),
        }));
    }

    let validator_tree = build_merkle_sum_tree(&validator_leaves);

    let membership_proofs: Vec<_> = acked_indices
        .iter()
        .filter_map(|&idx| get_merkle_sum_proof(&validator_leaves, idx))
        .collect();

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let freshness = ProofFreshness::new(checkpoint.height, now_ms, checkpoint.height, None);

    let vo = VerifiableObject::TxFinality {
        tx_hash: tx.hash.clone(),
        tx_signature: tx.signature.clone(),
        tx_from: tx.tx.from.clone(),
        tx_to: tx.tx.to.clone(),
        tx_amount: from_micro_units(tx.tx.amount),
        tx_nonce: tx.tx.nonce,
        tx_timestamp: tx.tx.timestamp,
        checkpoint_height: checkpoint.height,
        checkpoint_hash: checkpoint.hash.clone(),
        checkpoint_timestamp: checkpoint.timestamp,
        tx_merkle_root: checkpoint.tx_merkle_root.clone(),
        state_root: checkpoint.state_root.clone(),
        receipt_root: checkpoint.receipt_root.clone(),
        tip_count: checkpoint.tip_count,
        merkle_proof,
        merkle_index,
        bls_aggregated_sig: checkpoint.aggregated_signature.clone().unwrap_or_default(),
        bls_signer_bitmap: checkpoint
            .signer_bitmap
            .as_ref()
            .map(|b| URL_SAFE_NO_PAD.encode(b))
            .unwrap_or_default(),
        signer_count: checkpoint.validator_signatures.len(),
        signer_membership_proofs: membership_proofs,
        validator_sum_tree_root: validator_tree.root,
        chain_id: None,
        freshness: Some(freshness),
    };

    match create_vo_url(&vo) {
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchProofRequest {
    tx_hashes: Vec<String>,
    #[serde(default)]
    include_receipts: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchProofResponse {
    success: bool,
    proof: Option<rinku_core::stateful_receipt::VerifiableObject>,
    tx_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn generate_batch_proof(
    State(state): State<NodeState>,
    Json(req): Json<BatchProofRequest>,
) -> impl IntoResponse {
    use rinku_core::merkle::MerkleTree;
    use rinku_core::stateful_receipt::{CheckpointFinality, VerifiableObject};

    if req.tx_hashes.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(BatchProofResponse {
                success: false,
                proof: None,
                tx_count: 0,
                error: Some("tx_hashes must not be empty".to_string()),
            }),
        );
    }

    if req.tx_hashes.len() > 500 {
        return (
            StatusCode::BAD_REQUEST,
            Json(BatchProofResponse {
                success: false,
                proof: None,
                tx_count: 0,
                error: Some("Maximum 500 transactions per batch proof".to_string()),
            }),
        );
    }

    let mut checkpoint_height: Option<u64> = None;
    for hash in &req.tx_hashes {
        let (finalized, cp_h) = state.get_finalization_info(hash).await;
        if !finalized {
            return (
                StatusCode::BAD_REQUEST,
                Json(BatchProofResponse {
                    success: false,
                    proof: None,
                    tx_count: 0,
                    error: Some(format!("Transaction {} is not yet finalized", hash)),
                }),
            );
        }
        match (checkpoint_height, cp_h) {
            (None, Some(h)) => checkpoint_height = Some(h),
            (Some(existing), Some(h)) if existing != h => {
                return (StatusCode::BAD_REQUEST, Json(BatchProofResponse {
                    success: false,
                    proof: None,
                    tx_count: 0,
                    error: Some(format!(
                        "Transactions span multiple checkpoints ({} and {}). All must be in the same checkpoint.",
                        existing, h
                    )),
                }));
            }
            _ => {}
        }
    }

    let cp_height = match checkpoint_height {
        Some(h) => h,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(BatchProofResponse {
                    success: false,
                    proof: None,
                    tx_count: 0,
                    error: Some("Could not determine checkpoint height".to_string()),
                }),
            );
        }
    };

    let checkpoint = match state.get_checkpoint_by_height(cp_height).await {
        Some(cp) => cp,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(BatchProofResponse {
                    success: false,
                    proof: None,
                    tx_count: 0,
                    error: Some(format!(
                        "Checkpoint {} not found (may have been pruned)",
                        cp_height
                    )),
                }),
            );
        }
    };

    let finalized_hashes = if !checkpoint.finalized_tx_hashes.is_empty() {
        let mut h = checkpoint.finalized_tx_hashes.clone();
        h.sort();
        h
    } else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(BatchProofResponse {
                success: false,
                proof: None,
                tx_count: 0,
                error: Some("Checkpoint has no finalized tx hashes".to_string()),
            }),
        );
    };

    let tree = match MerkleTree::from_hex_leaves(&finalized_hashes) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(BatchProofResponse {
                    success: false,
                    proof: None,
                    tx_count: 0,
                    error: Some(format!("Failed to build merkle tree: {}", e)),
                }),
            );
        }
    };

    let mut leaf_indices = Vec::with_capacity(req.tx_hashes.len());
    for hash in &req.tx_hashes {
        match finalized_hashes.iter().position(|h| h == hash) {
            Some(idx) => leaf_indices.push(idx),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(BatchProofResponse {
                        success: false,
                        proof: None,
                        tx_count: 0,
                        error: Some(format!(
                            "Transaction {} not found in checkpoint {}",
                            hash, cp_height
                        )),
                    }),
                );
            }
        }
    }

    let multiproof = match tree.get_multiproof(&leaf_indices) {
        Ok(mp) => mp,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(BatchProofResponse {
                    success: false,
                    proof: None,
                    tx_count: 0,
                    error: Some(format!("Failed to generate multiproof: {}", e)),
                }),
            );
        }
    };

    let finality = CheckpointFinality {
        checkpoint_height: checkpoint.height,
        checkpoint_hash: checkpoint.hash.clone(),
        checkpoint_timestamp: checkpoint.timestamp,
        state_root: checkpoint.state_root.clone(),
        receipt_root: checkpoint.receipt_root.clone(),
        bls_aggregated_sig: checkpoint.aggregated_signature.clone(),
        bls_signer_bitmap: checkpoint.signer_bitmap.as_ref().map(|b| {
            use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
            URL_SAFE_NO_PAD.encode(b)
        }),
    };

    let chain_id = std::env::var("CHAIN_ID").ok();

    let freshness = rinku_core::stateful_receipt::ProofFreshness {
        generated_at_checkpoint: checkpoint.height,
        generated_at_timestamp: checkpoint.timestamp,
        chain_tip_at_generation: checkpoint.height,
        max_age_checkpoints: None,
    };

    let vo = VerifiableObject::BatchProof {
        finality,
        tx_hashes: req.tx_hashes.clone(),
        multiproof,
        receipts: None,
        chain_id,
        freshness: Some(freshness),
    };

    (
        StatusCode::OK,
        Json(BatchProofResponse {
            success: true,
            proof: Some(vo),
            tx_count: req.tx_hashes.len(),
            error: None,
        }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StateWitnessRequest {
    contract_id: String,
    keys: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StateWitnessResponse {
    success: bool,
    witness: Option<rinku_core::stateful_receipt::VerifiableObject>,
    key_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn generate_state_witness(
    State(state): State<NodeState>,
    Json(req): Json<StateWitnessRequest>,
) -> impl IntoResponse {
    use rinku_core::stateful_receipt::{StateWitnessEntry, VerifiableObject};

    if req.keys.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(StateWitnessResponse {
                success: false,
                witness: None,
                key_count: 0,
                error: Some("keys must not be empty".to_string()),
            }),
        );
    }

    if req.keys.len() > 100 {
        return (
            StatusCode::BAD_REQUEST,
            Json(StateWitnessResponse {
                success: false,
                witness: None,
                key_count: 0,
                error: Some("Maximum 100 keys per state witness".to_string()),
            }),
        );
    }

    let checkpoint = match state.get_latest_checkpoint().await {
        Some(cp) => cp,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(StateWitnessResponse {
                    success: false,
                    witness: None,
                    key_count: 0,
                    error: Some("No checkpoint available yet".to_string()),
                }),
            );
        }
    };

    let mut entries = Vec::with_capacity(req.keys.len());

    for key in &req.keys {
        let trie_key = crate::sparse_merkle_trie::hash_contract_key(&req.contract_id, key);
        let value = state
            .get_contract_storage_value(&req.contract_id, key)
            .await;
        let proof = state
            .get_contract_storage_proof(&req.contract_id, key)
            .await;

        let (proof_key_hex, proof_siblings) = match proof {
            Some(p) => {
                let key_hex = hex::encode(p.key);
                let siblings: Vec<String> = p.siblings.iter().map(hex::encode).collect();
                (key_hex, siblings)
            }
            None => (hex::encode(trie_key), vec![]),
        };

        entries.push(StateWitnessEntry {
            key: key.clone(),
            value,
            proof_key: proof_key_hex,
            proof_siblings,
        });
    }

    let chain_id = std::env::var("CHAIN_ID").ok();

    let freshness = rinku_core::stateful_receipt::ProofFreshness {
        generated_at_checkpoint: checkpoint.height,
        generated_at_timestamp: checkpoint.timestamp,
        chain_tip_at_generation: checkpoint.height,
        max_age_checkpoints: None,
    };

    let vo = VerifiableObject::StateWitness {
        contract_id: Some(req.contract_id),
        entries,
        state_root: checkpoint.state_root.clone(),
        checkpoint_height: checkpoint.height,
        checkpoint_hash: checkpoint.hash.clone(),
        bls_aggregated_sig: checkpoint.aggregated_signature.clone(),
        bls_signer_bitmap: checkpoint.signer_bitmap.as_ref().map(|b| {
            use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
            URL_SAFE_NO_PAD.encode(b)
        }),
        chain_id,
        freshness: Some(freshness),
    };

    (
        StatusCode::OK,
        Json(StateWitnessResponse {
            success: true,
            witness: Some(vo),
            key_count: req.keys.len(),
            error: None,
        }),
    )
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
        tip_rewards: from_micro_units(summary.tip_rewards),
        stake_rewards: from_micro_units(summary.stake_rewards),
        witness_rewards: from_micro_units(summary.witness_rewards),
        total_rewards: from_micro_units(summary.total_rewards),
        pending_rewards: from_micro_units(summary.pending_rewards),
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

async fn post_reconcile_stakes(State(state): State<NodeState>) -> Json<ReconcileStakesResponse> {
    let (count, changes) = state.reconcile_stakes().await;

    Json(ReconcileStakesResponse {
        reconciled_count: count,
        changes: changes
            .into_iter()
            .map(|(addr, old, new)| StakeReconcileChange {
                address: addr,
                old_staked: from_micro_units(old),
                new_staked: from_micro_units(new),
            })
            .collect(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StakingAddressResponse {
    address: String,
    staked_amount: f64,
    is_validator: bool,
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
    let is_validator = state.is_validator(&address).await;

    Json(StakingAddressResponse {
        address: status.address,
        staked_amount: from_micro_units(status.position.as_ref().map(|p| p.amount).unwrap_or(0)),
        is_validator,
        staked_at: status.position.as_ref().map(|p| p.staked_at),
        can_unstake: status.can_unstake,
        cooldown_remaining_ms: status.cooldown_remaining_ms,
        stake_rewards_total: from_micro_units(status.stake_rewards_total),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContractListResponse {
    contracts: Vec<crate::contracts::ContractState>,
    count: usize,
}

async fn get_contracts(State(state): State<NodeState>) -> Json<ContractListResponse> {
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeployContractRequest {
    creator: String,
    wasm_base64: String,
    #[serde(default)]
    init_state: HashMap<String, serde_json::Value>,
    #[serde(default)]
    nonce: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeployContractResponse {
    success: bool,
    contract_id: String,
    deploy_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn deploy_contract(
    State(api_state): State<ApiState>,
    addr: Option<ConnectInfo<SocketAddr>>,
    Json(req): Json<DeployContractRequest>,
) -> impl IntoResponse {
    let key = client_key(addr);
    if !api_state.rate_limits.contract.check_and_record(&key) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(DeployContractResponse {
                success: false,
                contract_id: String::new(),
                deploy_url: String::new(),
                error: Some(format!(
                    "Rate limited (contract): max {} requests per {}s",
                    api_state.rate_limits.contract.max(),
                    api_state.rate_limits.contract.window_secs()
                )),
            }),
        )
            .into_response();
    }

    const MAX_WASM_SIZE: usize = 2 * 1024 * 1024;

    if req.creator.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(DeployContractResponse {
                success: false,
                contract_id: String::new(),
                deploy_url: String::new(),
                error: Some("Creator address is required".to_string()),
            }),
        )
            .into_response();
    }

    let wasm_bytes_len = req.wasm_base64.len() * 3 / 4;
    if wasm_bytes_len > MAX_WASM_SIZE {
        return (
            StatusCode::BAD_REQUEST,
            Json(DeployContractResponse {
                success: false,
                contract_id: String::new(),
                deploy_url: String::new(),
                error: Some(format!(
                    "WASM binary too large: {} bytes (max {})",
                    wasm_bytes_len, MAX_WASM_SIZE
                )),
            }),
        )
            .into_response();
    }

    use base64::Engine as _;
    let wasm_valid = base64::engine::general_purpose::STANDARD
        .decode(&req.wasm_base64)
        .map(|bytes| bytes.len() >= 8 && bytes[0..4] == [0x00, 0x61, 0x73, 0x6d])
        .unwrap_or(false);

    if !wasm_valid {
        return (
            StatusCode::BAD_REQUEST,
            Json(DeployContractResponse {
                success: false,
                contract_id: String::new(),
                deploy_url: String::new(),
                error: Some("Invalid WASM binary (bad magic bytes or base64 encoding)".to_string()),
            }),
        )
            .into_response();
    }

    let nonce = req.nonce.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    });
    let contract_id = crate::contracts::create_contract_id(&req.creator, nonce);
    let deploy_url = format!("rinku://contract/{}", contract_id);

    info!(
        "Contract deploy preview: {} (submit as signed transaction to finalize)",
        contract_id
    );
    (
        StatusCode::OK,
        Json(DeployContractResponse {
            success: true,
            contract_id,
            deploy_url,
            error: None,
        }),
    )
        .into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallContractRequest {
    caller: String,
    entrypoint: String,
    #[serde(default)]
    input: HashMap<String, serde_json::Value>,
    #[serde(default)]
    gas_limit: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CallContractResponse {
    success: bool,
    gas_used: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_fee: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_message: Option<String>,
    logs: Vec<String>,
    events: Vec<crate::contracts::ContractEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state_diff: Option<crate::contracts::StateDiff>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_data: Option<String>,
    new_state: Option<HashMap<String, serde_json::Value>>,
    new_state_hash: Option<String>,
    new_height: Option<u64>,
}

async fn call_contract(
    State(api_state): State<ApiState>,
    addr: Option<ConnectInfo<SocketAddr>>,
    Path(contract_id): Path<String>,
    Json(req): Json<CallContractRequest>,
) -> impl IntoResponse {
    let key = client_key(addr);
    if !api_state.rate_limits.contract.check_and_record(&key) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(CallContractResponse {
                success: false,
                gas_used: 0,
                estimated_fee: None,
                error: Some(format!(
                    "Rate limited (contract): max {} requests per {}s",
                    api_state.rate_limits.contract.max(),
                    api_state.rate_limits.contract.window_secs()
                )),
                error_message: None,
                logs: vec![],
                events: vec![],
                state_diff: None,
                return_data: None,
                new_state: None,
                new_state_hash: None,
                new_height: None,
            }),
        );
    }

    let state = &api_state.node_state;
    let contract = match state.get_contract(&contract_id).await {
        Some(c) => c,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(CallContractResponse {
                    success: false,
                    gas_used: 0,
                    estimated_fee: None,
                    error: Some(format!("Contract not found: {}", contract_id)),
                    error_message: None,
                    logs: Vec::new(),
                    events: Vec::new(),
                    state_diff: None,
                    return_data: None,
                    new_state: None,
                    new_state_hash: None,
                    new_height: None,
                }),
            );
        }
    };

    let current_gas_price = state.get_gas_price().await;
    let runtime = crate::contracts::ContractRuntime::new();
    let result = runtime.execute(
        &contract_id,
        &contract.wasm_base64,
        &req.entrypoint,
        &req.input,
        &contract.state,
        contract.height + 1,
        req.gas_limit,
    );

    let base_fee = from_micro_units(current_gas_price);
    let additional_gas = result
        .gas_used
        .saturating_sub(crate::wasm_runtime::BASE_TX_GAS);
    let execution_fee =
        (additional_gas as f64 / crate::wasm_runtime::BASE_TX_GAS as f64) * base_fee;
    let total_estimated_fee = base_fee + execution_fee;

    if result.success {
        let mut new_state = contract.state.clone();
        let new_height = contract.height + 1;

        if let Some(ref diff) = result.state_diff {
            for change in &diff.changes {
                if let Some(ref new_value) = change.new_value {
                    new_state.insert(change.key.clone(), new_value.clone());
                } else {
                    new_state.remove(&change.key);
                }
            }
        }

        let new_state_hash = crate::contracts::compute_state_hash(&new_state);

        (
            StatusCode::OK,
            Json(CallContractResponse {
                success: true,
                gas_used: result.gas_used,
                estimated_fee: Some(total_estimated_fee),
                error: None,
                error_message: None,
                logs: result.logs,
                events: result.events,
                state_diff: result.state_diff,
                return_data: None,
                new_state: Some(new_state),
                new_state_hash: Some(new_state_hash),
                new_height: Some(new_height),
            }),
        )
    } else {
        (
            StatusCode::OK,
            Json(CallContractResponse {
                success: false,
                gas_used: result.gas_used,
                estimated_fee: Some(total_estimated_fee),
                error: result.error.clone(),
                error_message: result.error,
                logs: result.logs,
                events: result.events,
                state_diff: None,
                return_data: None,
                new_state: None,
                new_state_hash: None,
                new_height: None,
            }),
        )
    }
}

async fn get_version(State(state): State<NodeState>) -> Json<VersionResponse> {
    let (chain_id, network_id) = state.get_chain_info().await;
    Json(VersionResponse {
        protocol_version: crate::versioning::PROTOCOL_VERSION.to_string(),
        node_version: crate::versioning::NODE_VERSION.to_string(),
        chain_id,
        network_id,
        features: vec![
            "dag-consensus".to_string(),
            "url-native".to_string(),
            "redb-persistence".to_string(),
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
    use sysinfo::{Pid, System};

    let (dag_nodes, tips, accounts) = state.get_dag_stats().await;
    let checkpoint_height = state.get_checkpoint_height();
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
    let global_cpu_percent: f32 =
        sys.cpus().iter().map(|c| c.cpu_usage()).sum::<f32>() / cpu_count as f32;

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
        from_micro_units(gas_price),
        validators,
        from_micro_units(total_stake),
        from_micro_units(total_supply),
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
    use rinku_core::types::{PendingWeightVote, WeightVote};

    let state = &api_state.node_state;

    info!(
        "Received vote request for tx {}: vote={}, validator={:?}",
        hash, payload.vote, payload.validator_pubkey
    );

    let vote = match payload.vote.to_lowercase().as_str() {
        "boost" => WeightVote::Boost,
        "suppress" => WeightVote::Suppress,
        "neutral" => WeightVote::Neutral,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(WeightVoteResponse {
                    success: false,
                    tx_hash: hash,
                    vote: payload.vote,
                    message: "Invalid vote type. Use 'boost', 'suppress', or 'neutral'".to_string(),
                }),
            );
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
            info!(
                "Vote registered for tx {}: vote={:?}, validator={}",
                hash, pending_vote.vote, pending_vote.validator_pubkey
            );
        } else {
            warn!(
                "Weight trie not available - vote not registered for tx {}",
                hash
            );
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(WeightVoteResponse {
                    success: false,
                    tx_hash: hash,
                    vote: payload.vote,
                    message: "Weight attestation system not enabled on this node".to_string(),
                }),
            );
        }
    }

    // Broadcast vote to peers so all nodes have it for checkpoint aggregation
    if let Some(ref gossip_service) = api_state.gossip_service {
        gossip_service
            .broadcast_weight_vote(hash.clone(), validator_pubkey, vote_str, now_ms, bls_sig)
            .await;
    }

    (
        StatusCode::OK,
        Json(WeightVoteResponse {
            success: true,
            tx_hash: hash,
            vote: payload.vote,
            message:
                "Vote registered and broadcast to network. Will be aggregated at next checkpoint."
                    .to_string(),
        }),
    )
}

async fn get_weight_proof(
    State(state): State<NodeState>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    let inner = state.inner.read().await;

    let weight_trie = match &inner.weight_trie {
        Some(wt) => wt.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "Weight attestation system not enabled"
                })),
            )
                .into_response();
        }
    };

    let weight = match weight_trie.get_weight(&hash) {
        Some(w) => w.clone(),
        None => {
            return (
                StatusCode::OK,
                Json(WeightProofResponse {
                    tx_hash: hash,
                    aggregated_weight: rinku_core::types::AggregatedWeight::default(),
                    trust_score: 50,
                    boost_ratio: 0.0,
                    suppress_ratio: 0.0,
                    checkpoint_height: inner.checkpoints.last().map(|c| c.height),
                    weight_trie_root: String::new(),
                    merkle_proof: vec![],
                    merkle_index: 0,
                }),
            )
                .into_response();
        }
    };

    let mut wt = weight_trie.clone();
    let (proof, index, _leaf) = wt.generate_proof(&hash).unwrap_or((
        vec![],
        0,
        rinku_core::types::WeightTrieLeaf {
            tx_hash: hash.clone(),
            boost_stake_micro: 0,
            suppress_stake_micro: 0,
            neutral_stake_micro: 0,
            total_network_stake_micro: 0,
            attestation_count: 0,
        },
    ));
    let root = wt.compute_root();

    (
        StatusCode::OK,
        Json(WeightProofResponse {
            tx_hash: hash,
            trust_score: weight.trust_score(),
            boost_ratio: weight.boost_ratio(),
            suppress_ratio: weight.suppress_ratio(),
            aggregated_weight: weight,
            checkpoint_height: inner.checkpoints.last().map(|c| c.height),
            weight_trie_root: root,
            merkle_proof: proof,
            merkle_index: index,
        }),
    )
        .into_response()
}

async fn get_tx_weight(
    State(state): State<NodeState>,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    let inner = state.inner.read().await;

    let weight = inner
        .weight_trie
        .as_ref()
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
    event_bus: Arc<crate::events::EventBus>,
) -> anyhow::Result<JoinHandle<()>> {
    let (tx_max, contract_max, general_max) = state.rate_limit_config();
    let rate_limits = Arc::new(HttpRateLimiters::from_config(
        tx_max,
        contract_max,
        general_max,
    ));
    let faucet_enabled = state.faucet_enabled();
    let cors_origins = state.cors_allow_origins().to_vec();

    let read_cors = build_read_cors();
    let write_cors = build_write_cors(&cors_origins);

    info!(
        "HTTP abuse controls: tx={}/min contract={}/min general={}/min faucet_enabled={} write_cors={:?}",
        tx_max, contract_max, general_max, faucet_enabled, cors_origins
    );

    let api_state = ApiState {
        node_state: state.clone(),
        gossip_service,
        event_bus: event_bus.clone(),
        rate_limits,
        faucet_enabled,
    };

    let ws_state = crate::websocket::WsState {
        event_bus: event_bus.clone(),
    };
    let ws_routes = Router::new()
        .route("/api/ws", get(crate::websocket::ws_handler))
        .with_state(ws_state);

    // Write routes: tightened CORS + rate-limited handlers
    let write_routes = Router::new()
        .route("/api/slashing/evidence", post(post_slashing_evidence))
        .route("/api/tx", post(submit_transaction))
        .route("/api/tx/fast", post(submit_fast_path_transaction))
        .route("/api/tx/batch", post(submit_batch_transaction))
        .route("/api/request", post(handle_faucet_request))
        .route("/api/faucet/request", post(handle_faucet_request))
        .route("/api/sync/delta", post(post_sync_delta))
        .route("/api/tx/:hash/vote", post(post_weight_vote))
        .route("/api/partition/merge", post(post_partition_merge))
        .route("/api/contracts/deploy", post(deploy_contract))
        .route("/api/contracts/:contract_id/call", post(call_contract))
        .layer(write_cors)
        .with_state(api_state.clone());

    // Read-ish gossip/API routes that still need ApiState (open CORS)
    let gossip_read_routes = Router::new()
        .route("/api/gossip/stats", get(get_gossip_stats))
        .route("/api/peers", get(get_peers))
        .route("/api/tx/fast/:hash", get(get_fast_path_status))
        .route("/api/dag", get(get_dag))
        .route("/api/tx/:hash", get(get_transaction))
        .route("/api/tx/:hash/replies", get(get_transaction_replies))
        .route(
            "/api/account/:address/transactions",
            get(get_account_transactions_with_fast_path),
        )
        .route("/api/finality/metrics", get(get_finality_metrics))
        .layer(read_cors.clone())
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
        .route(
            "/api/account/:address/proof/current",
            get(get_account_proof_current),
        )
        .route("/api/tx/:hash/receipt", get(get_transaction_receipt))
        .route("/api/txp/:hash", get(get_self_provable_tx))
        .route("/api/tx/:hash/proof", get(generate_transaction_proof))
        .route("/api/tx/:hash/weight", get(get_tx_weight))
        .route("/api/tx/:hash/weight-proof", get(get_weight_proof))
        .route("/api/dag/summary", get(get_dag_summary))
        .route("/api/accounts", get(get_accounts))
        .route("/api/network/stats", get(get_network_stats))
        .route("/api/network/partition", get(get_network_partition))
        .route("/api/partition/budget", post(post_partition_budget))
        .route("/api/merge/report/latest", get(get_merge_report_latest))
        .route("/api/merge/report/:epoch", get(get_merge_report_by_epoch))
        .route("/api/merge/history", get(get_merge_history))
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
        .route("/api/proof/batch", post(generate_batch_proof))
        .route("/api/state/witness", post(generate_state_witness))
        .route("/api/chain/tip", get(get_chain_tip))
        .route(
            "/api/tip-consolidator/stats",
            get(get_tip_consolidator_stats),
        )
        .route("/metrics", get(get_metrics))
        .layer(read_cors)
        .with_state(state);

    // Merge all routers
    let api_routes = ws_routes
        .merge(write_routes)
        .merge(gossip_read_routes)
        .merge(node_routes);

    // Root health check handler for API-only mode
    async fn root_health() -> impl IntoResponse {
        Json(serde_json::json!({ "status": "ok", "mode": "api-only" }))
    }

    let app = if let Some(static_path) = static_dir {
        if static_path.exists() {
            let index_path = static_path.join("index.html");
            let serve_dir =
                ServeDir::new(&static_path).not_found_service(ServeFile::new(&index_path));
            info!(
                "Serving static files from {:?} with SPA routing fallback to {:?}",
                static_path, index_path
            );
            api_routes.fallback_service(serve_dir)
        } else {
            info!(
                "Static directory {:?} not found, API-only mode with root health check",
                static_path
            );
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
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    Ok(handle)
}
