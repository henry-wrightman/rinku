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
use std::sync::Mutex;
use tokio::task::JoinHandle;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::{info, warn};

use crate::gossip::GossipMessage;

static FAUCET_RATE_LIMIT: std::sync::LazyLock<Mutex<HashMap<String, u64>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
const FAUCET_AMOUNT: f64 = 100.0;
const FAUCET_RATE_LIMIT_MS: u64 = 60_000;

use crate::state::NodeState;

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

#[derive(Deserialize)]
struct SubmitTxRequest {
    from: String,
    to: String,
    amount: f64,
    nonce: u64,
    timestamp: u64,
    parents: Vec<String>,
    signature: String,
    hash: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyTxInner {
    from: String,
    to: String,
    amount: f64,
    #[serde(default)]
    fee: f64,
    nonce: u64,
    #[serde(default)]
    tip_urls: Vec<String>,
    sig: String,
    ts: u64,
    hash: String,
}

#[derive(Deserialize)]
struct LegacySubmitTxRequest {
    tx: LegacyTxInner,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BatchTxItem {
    tx: LegacyTxInner,
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
    finalized: bool,
    weight: f64,
    url: String,
}

#[derive(Serialize)]
struct AccountsResponse {
    accounts: Vec<AccountResponse>,
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
struct SyncTxQuery {
    #[serde(default)]
    from_checkpoint: Option<u64>,
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
    let validators = state.get_validator_count().await;
    let total_stake = state.get_total_stake().await;
    let uptime_seconds = state.get_uptime_seconds().await;
    let merkle_root = state.get_dag_merkle_root().await;
    let node_id = std::env::var("NODE_ID").unwrap_or_else(|_| "unknown".to_string());

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
    })
}

async fn post_gossip(
    State(state): State<NodeState>,
    Json(message): Json<GossipMessage>,
) -> impl IntoResponse {
    info!("Received gossip message: {:?}", std::mem::discriminant(&message));
    
    match &message {
        GossipMessage::Transaction { hash, tx } => {
            info!("Gossip: received tx {} from peer", &hash[..16.min(hash.len())]);
            if let Err(e) = state.add_transaction(tx.clone()).await {
                warn!("Failed to add gossiped transaction: {}", e);
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": e.to_string() })),
                ).into_response();
            }
        }
        GossipMessage::TipAnnouncement { dag_size, tips, .. } => {
            info!("Gossip: peer announced {} tips, dag_size={}", tips.len(), dag_size);
        }
        GossipMessage::SyncRequest { from_checkpoint, missing_hashes } => {
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
        GossipMessage::SyncResponse { transactions, checkpoint_height } => {
            info!("Gossip: received sync response with {} txs at height {}", 
                transactions.len(), checkpoint_height);
            for tx in transactions {
                if let Err(e) = state.add_transaction(tx.clone()).await {
                    warn!("Failed to add synced tx {}: {}", &tx.hash[..16.min(tx.hash.len())], e);
                }
            }
        }
        GossipMessage::PeerDiscovery { peers, node_id } => {
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
    }

    Json(serde_json::json!({ "status": "ok" })).into_response()
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

async fn get_snapshot_sync(State(state): State<NodeState>) -> Json<SnapshotSyncResponse> {
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

    Json(SnapshotSyncResponse {
        accounts: snapshot.accounts,
        validators: snapshot.validators,
        checkpoints: snapshot.checkpoints,
        gas_price: snapshot.gas_price,
        total_supply: snapshot.total_supply,
        genesis_time: snapshot.genesis_time,
        dag_transactions: snapshot.dag_transactions,
        total_transactions: snapshot.total_transactions,
        checkpoint_height,
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

async fn get_sync_transactions(
    State(state): State<NodeState>,
    Query(query): Query<SyncTxQuery>,
) -> Json<Vec<rinku_core::types::SignedTransaction>> {
    let from_checkpoint = query.from_checkpoint.unwrap_or(0);
    
    info!("Sync transactions request from checkpoint {}", from_checkpoint);
    
    // Get all transactions since the given checkpoint
    let txs = state.get_txs_since_checkpoint(from_checkpoint, &[]).await;
    
    info!("Returning {} transactions since checkpoint {}", txs.len(), from_checkpoint);
    
    Json(txs)
}

async fn handle_faucet_request(
    State(state): State<NodeState>,
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

    let tips = state.get_tips().await;
    let tip_urls: Vec<String> = tips
        .into_iter()
        .take(2)
        .map(|hash| format!("rinku://tx/h/{}", hash))
        .collect();

    let faucet_account = state.get_account("faucet").await;
    let nonce = faucet_account.map(|a| a.nonce).unwrap_or(0) + 1;

    let inner_tx = rinku_core::types::Transaction {
        from: "faucet".to_string(),
        to: address,
        amount: FAUCET_AMOUNT,
        nonce,
        timestamp: now,
        parents: tip_urls,
        kind: None,
        gas_limit: None,
        gas_price: Some(0.0),
        data: None,
        signature: None,
    };

    let tx = rinku_core::types::SignedTransaction {
        tx: inner_tx,
        hash: String::new(),
        signature: "faucet-signature".to_string(),
    };

    let tx_json = serde_json::to_string(&tx.tx).unwrap_or_default();
    let hash = rinku_core::crypto::hash_transaction(&tx_json);
    let tx = rinku_core::types::SignedTransaction { hash: hash.clone(), ..tx };

    match state.add_transaction(tx).await {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!(FaucetResponse {
                success: true,
                amount: FAUCET_AMOUNT,
                tx_hash: hash,
            })),
        ).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response(),
    }
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
    let tips = state.get_tips().await;
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
        None => (StatusCode::NOT_FOUND, "Account not found").into_response(),
    }
}

async fn submit_transaction(
    State(state): State<NodeState>,
    Json(req): Json<SubmitTxRequest>,
) -> impl IntoResponse {
    let tx = rinku_core::types::SignedTransaction {
        tx: rinku_core::types::Transaction {
            from: req.from,
            to: req.to,
            amount: req.amount,
            nonce: req.nonce,
            timestamp: req.timestamp,
            parents: req.parents,
            kind: None,
            gas_limit: None,
            gas_price: None,
            data: None,
            signature: Some(req.signature.clone()),
        },
        hash: req.hash.clone(),
        signature: req.signature,
    };

    match state.add_transaction(tx).await {
        Ok(()) => (
            StatusCode::OK,
            Json(SubmitTxResponse {
                success: true,
                hash: req.hash,
                error: None,
            }),
        ),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(SubmitTxResponse {
                success: false,
                hash: req.hash,
                error: Some(e.to_string()),
            }),
        ),
    }
}

async fn submit_legacy_transaction(
    State(state): State<NodeState>,
    Json(req): Json<LegacySubmitTxRequest>,
) -> impl IntoResponse {
    let inner = req.tx;
    let tx = rinku_core::types::SignedTransaction {
        tx: rinku_core::types::Transaction {
            from: inner.from,
            to: inner.to,
            amount: inner.amount,
            nonce: inner.nonce,
            timestamp: inner.ts,
            parents: inner.tip_urls,
            kind: None,
            gas_limit: None,
            gas_price: Some(inner.fee),
            data: None,
            signature: Some(inner.sig.clone()),
        },
        hash: inner.hash.clone(),
        signature: inner.sig,
    };

    match state.add_transaction(tx).await {
        Ok(()) => (
            StatusCode::OK,
            Json(SubmitTxResponse {
                success: true,
                hash: inner.hash,
                error: None,
            }),
        ),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(SubmitTxResponse {
                success: false,
                hash: inner.hash,
                error: Some(e.to_string()),
            }),
        ),
    }
}

async fn submit_batch_transaction(
    State(state): State<NodeState>,
    Json(req): Json<BatchSubmitTxRequest>,
) -> Json<BatchSubmitTxResponse> {
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
                    parents: inner.tip_urls,
                    kind: None,
                    gas_limit: None,
                    gas_price: Some(inner.fee),
                    data: None,
                    signature: Some(inner.sig.clone()),
                },
                hash: inner.hash,
                signature: inner.sig,
            }
        })
        .collect();

    // Use optimized batch method - single lock acquisition
    let results = state.add_transactions_batch(txs).await;
    let successful = results.iter().filter(|r| r.is_ok()).count();
    let failed = results.len() - successful;

    Json(BatchSubmitTxResponse {
        successful,
        failed,
        total,
    })
}

async fn get_dag_summary(State(state): State<NodeState>) -> Json<DagSummaryResponse> {
    let (total_nodes, tip_count, account_count) = state.get_dag_stats().await;
    let checkpoint_height = state.get_checkpoint_height().await;
    let tips = state.get_tips().await;
    let (finalized_count, _) = state.get_finalized_stats().await;
    Json(DagSummaryResponse {
        total_nodes,
        tip_count,
        checkpoint_height,
        finalized_count,
        tips,
        merkle_root: "".to_string(),
        account_count,
    })
}

async fn get_dag(State(state): State<NodeState>) -> Json<DagResponse> {
    let mut nodes_data = state.get_all_dag_nodes().await;
    // Sort by timestamp descending (newest first)
    nodes_data.sort_by(|a, b| b.ts.cmp(&a.ts));
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
            DagNodeResponse {
                hash: n.hash,
                from: n.from,
                to: n.to,
                amount: n.amount,
                fee: n.fee,
                nonce: n.nonce,
                ts: n.ts,
                parents,
                finalized: n.finalized,
                weight: n.weight,
                url,
            }
        })
        .collect();
    Json(DagResponse {
        has_more: false,
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
    let (dag_nodes, _, _) = state.get_dag_stats().await;
    let total_transactions = state.get_total_transactions().await as usize;
    let checkpoint_height = state.get_checkpoint_height().await;
    let rewards = state.rewards.read().await;
    let total_stake = rewards.get_total_staked();
    let validators = rewards.get_active_validators().len();
    drop(rewards);
    let (finalized_count, unfinalized_count) = state.get_finalized_stats().await;
    let finality_ratio = if dag_nodes > 0 {
        finalized_count as f64 / dag_nodes as f64
    } else {
        0.0
    };
    let elapsed_secs = state.get_elapsed_seconds();
    let tps = if elapsed_secs > 0.0 && total_transactions > 0 {
        (total_transactions as f64) / elapsed_secs
    } else {
        0.0
    };
    Json(NetworkStatsResponse {
        tps,
        total_transactions_processed: total_transactions,
        finalized_count,
        unfinalized_count,
        finality_ratio,
        checkpoint_count: checkpoint_height,
        latest_checkpoint_height: checkpoint_height,
        latest_checkpoint_id: None,
        total_staked: total_stake,
        validator_count: validators,
        network_age: 0,
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

async fn get_finality_metrics(State(state): State<NodeState>) -> Json<FinalityMetricsResponse> {
    let (dag_nodes, _, _) = state.get_dag_stats().await;
    let total_transactions = state.get_total_transactions().await as usize;
    let (finalized_count, pending_count) = state.get_finalized_stats().await;
    let (avg_ms, median_ms, p95_ms, last_checkpoint_age_ms, checkpoints_per_min) = 
        state.get_finality_timing().await;
    
    let finality_rate = if dag_nodes > 0 {
        finalized_count as f64 / dag_nodes as f64
    } else {
        1.0
    };
    let tx_throughput = if total_transactions > 0 { (total_transactions as f64) / 60.0 } else { 0.0 };
    
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
}

async fn get_transaction(
    State(state): State<NodeState>,
    Path(hash): Path<String>,
) -> Result<Json<TransactionResponse>, (StatusCode, String)> {
    if let Some(tx) = state.get_transaction(&hash).await {
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
            weight: 1.0,
            url: format!("/tx/h/{}", tx.hash),
        }))
    } else {
        Err((StatusCode::NOT_FOUND, format!("Transaction {} not found", hash)))
    }
}

async fn get_self_provable_tx(
    State(state): State<NodeState>,
    Path(hash): Path<String>,
) -> Result<Json<SelfProvableTransactionResponse>, (StatusCode, String)> {
    let tx = state
        .get_transaction(&hash)
        .await
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Transaction {} not found", hash)))?;

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
    
    let max_supply = 30_000_000.0;
    let genesis_allocation = 6_000_000.0;
    
    Json(TokenomicsSupplyResponse {
        max_supply,
        genesis_allocation,
        circulating_supply: total_supply,
        total_emitted: 0.0,
        total_burned: 0.0,
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

async fn get_gossip_stats() -> Json<GossipStatsResponse> {
    Json(GossipStatsResponse {
        peers_connected: 0,
        messages_sent: 0,
        messages_received: 0,
        last_gossip_at: 0,
    })
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

#[derive(Deserialize)]
struct StakeRequest {
    address: String,
    amount: f64,
}

#[derive(Serialize)]
struct StakeResponse {
    success: bool,
    address: String,
    amount: f64,
    error: Option<String>,
}

async fn post_stake(
    State(state): State<NodeState>,
    Json(req): Json<StakeRequest>,
) -> Json<StakeResponse> {
    let mut rewards = state.rewards.write().await;
    
    match rewards.stake(&req.address, req.amount) {
        Ok(_position) => Json(StakeResponse {
            success: true,
            address: req.address,
            amount: req.amount,
            error: None,
        }),
        Err(e) => Json(StakeResponse {
            success: false,
            address: req.address,
            amount: req.amount,
            error: Some(e),
        }),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContractDeployRequest {
    creator: String,
    wasm_base64: String,
    init_state: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ContractDeployResponse {
    success: bool,
    contract_id: Option<String>,
    deploy_url: Option<String>,
    error: Option<String>,
}

async fn post_contract_deploy(
    Json(req): Json<ContractDeployRequest>,
) -> Json<ContractDeployResponse> {
    let contract_id = format!("contract-{}", &rinku_core::crypto::sha256_hex(&req.creator)[..16]);
    let deploy_url = format!("rinku://contract/{}", contract_id);
    
    Json(ContractDeployResponse {
        success: true,
        contract_id: Some(contract_id),
        deploy_url: Some(deploy_url),
        error: None,
    })
}

#[derive(Deserialize)]
struct ContractCallRequest {
    caller: String,
    entrypoint: String,
    input: serde_json::Value,
}

#[derive(Serialize)]
struct ContractCallResponse {
    success: bool,
    result: Option<serde_json::Value>,
    gas_used: u64,
    error: Option<String>,
}

async fn post_contract_call(
    Path(contract_id): Path<String>,
    Json(req): Json<ContractCallRequest>,
) -> Json<ContractCallResponse> {
    Json(ContractCallResponse {
        success: true,
        result: Some(serde_json::json!({
            "contract_id": contract_id,
            "caller": req.caller,
            "entrypoint": req.entrypoint,
        })),
        gas_used: 1000,
        error: None,
    })
}

async fn get_version() -> Json<VersionResponse> {
    Json(VersionResponse {
        protocol_version: "1.0.0".to_string(),
        node_version: env!("CARGO_PKG_VERSION").to_string(),
        chain_id: "rinku-mainnet".to_string(),
        network_id: "rinku".to_string(),
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
    let validators = state.get_validator_count().await;
    let total_stake = state.get_total_stake().await;
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

pub async fn start_api_server(
    state: NodeState,
    port: u16,
    static_dir: Option<PathBuf>,
) -> anyhow::Result<JoinHandle<()>> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let api_routes = Router::new()
        .route("/health", get(health))
        .route("/api/stats", get(get_stats))
        .route("/api/tips", get(get_tips))
        .route("/api/tipUrls", get(get_tip_urls))
        .route("/api/request", post(handle_faucet_request))
        .route("/api/account/:address", get(get_account))
        .route("/api/tx", post(submit_legacy_transaction))
        .route("/api/tx/batch", post(submit_batch_transaction))
        .route("/api/tx/:hash", get(get_transaction))
        .route("/api/txp/:hash", get(get_self_provable_tx))
        .route("/api/dag", get(get_dag))
        .route("/api/dag/summary", get(get_dag_summary))
        .route("/api/accounts", get(get_accounts))
        .route("/api/stats/network", get(get_network_stats))
        .route("/api/gas/price", get(get_gas_price))
        .route("/api/gas/stats", get(get_gas_stats))
        .route("/api/finality/metrics", get(get_finality_metrics))
        .route("/api/version", get(get_version))
        .route("/api/staking", get(get_staking))
        .route("/api/staking/stake", post(post_stake))
        .route("/api/staking/:address", get(get_staking_address))
        .route("/api/contracts/deploy", post(post_contract_deploy))
        .route("/api/contracts/:contract_id/call", post(post_contract_call))
        .route("/api/tokenomics/supply", get(get_tokenomics_supply))
        .route("/api/tokenomics/emission", get(get_tokenomics_emission))
        .route("/api/tokenomics/slashing", get(get_tokenomics_slashing))
        .route("/api/rewards/config", get(get_rewards_config))
        .route("/api/rewards/:address", get(get_rewards_address))
        .route("/api/checkpoints", get(get_checkpoints))
        .route("/api/checkpoints/latest", get(get_checkpoints_latest))
        .route("/api/fork/stats", get(get_fork_stats))
        .route("/api/gossip", post(post_gossip))
        .route("/api/gossip/stats", get(get_gossip_stats))
        .route("/api/sync/status", get(get_sync_status))
        .route("/api/sync/bootstrap", post(post_bootstrap))
        .route("/api/sync/snapshot", get(get_snapshot_sync))
        .route("/api/sync/transactions", get(get_batch_transactions))
        .route("/api/sync/delta", get(get_sync_transactions))
        .route("/api/tip-consolidator/stats", get(get_tip_consolidator_stats))
        .route("/metrics", get(get_metrics))
        .layer(cors.clone())
        .with_state(state);

    let app = if let Some(static_path) = static_dir {
        if static_path.exists() {
            let index_path = static_path.join("index.html");
            let serve_dir = ServeDir::new(&static_path)
                .not_found_service(ServeFile::new(&index_path));
            info!("Serving static files from {:?}", static_path);
            api_routes.fallback_service(serve_dir)
        } else {
            info!("Static directory {:?} not found, API-only mode", static_path);
            api_routes
        }
    } else {
        api_routes
    };

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("API server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    let handle = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service()).await.unwrap();
    });

    Ok(handle)
}
