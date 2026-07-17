use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{debug, info, warn};

use crate::events::{EventBus, NodeEvent};

static ACTIVE_CONNECTIONS: AtomicU32 = AtomicU32::new(0);
const MAX_CONNECTIONS: u32 = 100;
const PING_INTERVAL_SECS: u64 = 30;

#[derive(Clone)]
pub struct WsState {
    pub event_bus: Arc<EventBus>,
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(ws_state): State<WsState>,
) -> impl IntoResponse {
    let current = ACTIVE_CONNECTIONS.load(Ordering::Relaxed);
    if current >= MAX_CONNECTIONS {
        warn!(
            "WebSocket connection rejected: {} active (max {})",
            current, MAX_CONNECTIONS
        );
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    }

    ws.on_upgrade(move |socket| handle_socket(socket, ws_state))
        .into_response()
}

async fn handle_socket(socket: WebSocket, ws_state: WsState) {
    let conn_id = ACTIVE_CONNECTIONS.fetch_add(1, Ordering::Relaxed) + 1;
    info!("WebSocket connected (active: {})", conn_id);

    let (mut sender, mut receiver) = socket.split();
    let mut event_rx = ws_state.event_bus.subscribe();
    let mut ping_interval = interval(Duration::from_secs(PING_INTERVAL_SECS));
    ping_interval.tick().await;

    let mut subscribe_filter: Option<Vec<String>> = None;
    let mut account_filter: Option<Vec<String>> = None;

    loop {
        tokio::select! {
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                            if parsed.get("type").and_then(|v| v.as_str()) == Some("ping") {
                                let _ = sender.send(Message::Text(
                                    serde_json::json!({"type": "pong"}).to_string().into()
                                )).await;
                                continue;
                            }
                            if let Some(subs) = parsed.get("subscribe").and_then(|v| v.as_array()) {
                                let mut types = Vec::new();
                                let mut accounts = Vec::new();
                                for s in subs {
                                    if let Some(str_val) = s.as_str() {
                                        if let Some(addr) = str_val.strip_prefix("account:") {
                                            accounts.push(addr.to_string());
                                        } else {
                                            types.push(str_val.to_string());
                                        }
                                    }
                                }
                                if !types.is_empty() { subscribe_filter = Some(types); }
                                if !accounts.is_empty() { account_filter = Some(accounts); }
                                debug!("WebSocket subscription updated: types={:?}, accounts={:?}", subscribe_filter, account_filter);
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
            event = event_rx.recv() => {
                match event {
                    Ok(node_event) => {
                        if !should_send(&node_event, &subscribe_filter, &account_filter) {
                            continue;
                        }
                        if let Ok(json) = serde_json::to_string(&node_event) {
                            if sender.send(Message::Text(json.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        debug!("WebSocket lagged {} events", n);
                    }
                    Err(_) => break,
                }
            }
            _ = ping_interval.tick() => {
                if sender.send(Message::Ping(vec![].into())).await.is_err() {
                    break;
                }
            }
        }
    }

    let remaining = ACTIVE_CONNECTIONS.fetch_sub(1, Ordering::Relaxed) - 1;
    info!("WebSocket disconnected (active: {})", remaining);
}

fn should_send(
    event: &NodeEvent,
    type_filter: &Option<Vec<String>>,
    account_filter: &Option<Vec<String>>,
) -> bool {
    if let Some(ref accounts) = account_filter {
        let matches = match event {
            NodeEvent::AccountUpdated { address, .. } => accounts.iter().any(|a| a == address),
            NodeEvent::NewTransaction { from, to, .. }
            | NodeEvent::FastPathConfirmed { from, to, .. }
            | NodeEvent::FastPathExecuted { from, to, .. } => {
                accounts.iter().any(|a| a == from || a == to)
            }
            _ => false,
        };
        if matches {
            return true;
        }
    }

    if let Some(ref types) = type_filter {
        let event_type = match event {
            NodeEvent::NewTransaction { .. } => "transactions",
            NodeEvent::FastPathConfirmed { .. } => "fast_path",
            NodeEvent::FastPathExecuted { .. } => "fast_path",
            NodeEvent::CheckpointCreated { .. } => "checkpoints",
            NodeEvent::AccountUpdated { .. } => "accounts",
            NodeEvent::PartitionSuspected { .. } => "partition",
            NodeEvent::PartitionConfirmed { .. } => "partition",
            NodeEvent::PartitionHealed { .. } => "partition",
            NodeEvent::MergeStarted { .. } => "merge",
            NodeEvent::MergeCompleted { .. } => "merge",
            NodeEvent::MergeProgress { .. } => "merge",
            NodeEvent::TransactionRolledBack { .. } => "merge",
            NodeEvent::PenaltyAssessed { .. } => "merge",
        };
        return types.iter().any(|t| t == event_type || t == "all");
    }

    if account_filter.is_some() {
        return false;
    }

    true
}
