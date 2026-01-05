use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use tokio::task::JoinHandle;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

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
    address: String,
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
struct VersionResponse {
    protocol: String,
    node: String,
    features: Vec<String>,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn get_stats(State(state): State<NodeState>) -> Json<StatsResponse> {
    let (dag_nodes, tips, accounts) = state.get_dag_stats().await;
    let checkpoint_height = state.get_checkpoint_height().await;
    let gas_price = state.get_gas_price().await;
    let total_supply = state.get_total_supply().await;
    let validators = state.get_validator_count().await;
    let total_stake = state.get_total_stake().await;

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
                address: account.address,
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

async fn get_dag_summary(State(state): State<NodeState>) -> Json<DagSummaryResponse> {
    let (total_nodes, tip_count, account_count) = state.get_dag_stats().await;
    let checkpoint_height = state.get_checkpoint_height().await;
    let tips = state.get_tips().await;
    Json(DagSummaryResponse {
        total_nodes,
        tip_count,
        checkpoint_height,
        finalized_count: 0,
        tips,
        merkle_root: "".to_string(),
        account_count,
    })
}

async fn get_dag(State(state): State<NodeState>) -> Json<DagResponse> {
    let nodes_data = state.get_all_dag_nodes().await;
    let nodes: Vec<DagNodeResponse> = nodes_data
        .into_iter()
        .map(|n| {
            let url = format!("rinku://tx/{}", &n.hash);
            DagNodeResponse {
                hash: n.hash,
                from: n.from,
                to: n.to,
                amount: n.amount,
                nonce: n.nonce,
                ts: n.ts,
                parents: n.parents,
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
                address: a.address,
                balance: a.balance,
                nonce: a.nonce,
                staked: a.staked,
            })
            .collect(),
    })
}

async fn get_network_stats(State(state): State<NodeState>) -> Json<NetworkStatsResponse> {
    let (total_nodes, _, _) = state.get_dag_stats().await;
    let checkpoint_height = state.get_checkpoint_height().await;
    let total_stake = state.get_total_stake().await;
    let validators = state.get_validator_count().await;
    Json(NetworkStatsResponse {
        tps: 0.0,
        total_transactions_processed: total_nodes,
        finalized_count: 0,
        unfinalized_count: total_nodes,
        finality_ratio: 0.0,
        checkpoint_count: checkpoint_height,
        latest_checkpoint_height: checkpoint_height,
        latest_checkpoint_id: None,
        total_staked: total_stake,
        validator_count: validators,
        network_age: 0,
    })
}

async fn get_gas_price(State(state): State<NodeState>) -> Json<GasPriceResponse> {
    let current = state.get_gas_price().await;
    Json(GasPriceResponse {
        current,
        min: 0.001,
        max: 100.0,
        avg_last_100: current,
        total_burned: 0.0,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GasStatsResponse {
    total_burned: f64,
    total_collected: f64,
}

async fn get_gas_stats() -> Json<GasStatsResponse> {
    Json(GasStatsResponse {
        total_burned: 0.0,
        total_collected: 0.0,
    })
}

async fn get_finality_metrics() -> Json<FinalityMetricsResponse> {
    Json(FinalityMetricsResponse {
        avg_time_to_finality: 15000.0,
        median_time_to_finality: 15000.0,
        p95_time_to_finality: 20000.0,
        pending_count: 0,
        finalized_count: 0,
        finality_rate: 1.0,
        checkpoint_latency: 15000.0,
        checkpoints_per_minute: 4.0,
        last_checkpoint_age: 0,
        tx_throughput: 0.0,
    })
}

async fn get_version() -> Json<VersionResponse> {
    Json(VersionResponse {
        protocol: "1.0.0".to_string(),
        node: env!("CARGO_PKG_VERSION").to_string(),
        features: vec![
            "dag-consensus".to_string(),
            "url-native".to_string(),
            "sled-persistence".to_string(),
        ],
    })
}

async fn get_metrics(State(state): State<NodeState>) -> String {
    let (dag_nodes, tips, accounts) = state.get_dag_stats().await;
    let checkpoint_height = state.get_checkpoint_height().await;
    let gas_price = state.get_gas_price().await;
    let validators = state.get_validator_count().await;
    let total_stake = state.get_total_stake().await;
    let total_supply = state.get_total_supply().await;

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
"#,
        dag_nodes,
        tips,
        accounts,
        checkpoint_height,
        gas_price,
        validators,
        total_stake,
        total_supply,
    )
}

pub async fn start_api_server(state: NodeState, port: u16) -> anyhow::Result<JoinHandle<()>> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/stats", get(get_stats))
        .route("/api/tips", get(get_tips))
        .route("/api/tipUrls", get(get_tip_urls))
        .route("/api/account/:address", get(get_account))
        .route("/api/tx", post(submit_legacy_transaction))
        .route("/api/dag", get(get_dag))
        .route("/api/dag/summary", get(get_dag_summary))
        .route("/api/accounts", get(get_accounts))
        .route("/api/stats/network", get(get_network_stats))
        .route("/api/gas/price", get(get_gas_price))
        .route("/api/gas/stats", get(get_gas_stats))
        .route("/api/finality/metrics", get(get_finality_metrics))
        .route("/api/version", get(get_version))
        .route("/metrics", get(get_metrics))
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("API server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    Ok(handle)
}
