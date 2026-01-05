use anyhow::Result;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod api;
mod checkpoint;
mod config;
mod consensus;
mod fork_remediation;
mod gas;
mod gossip;
mod state;
mod tip_consolidator;
mod validator;

use config::NodeConfig;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting Rinku Node (Rust)...");

    let config = NodeConfig::from_env();
    info!("Node ID: {}", config.node_id);
    info!("Data dir: {}", config.data_dir);

    let state = state::NodeState::new(config.clone()).await?;

    let api_handle = api::start_api_server(state.clone(), config.api_port).await?;

    info!("Rinku Node running on port {}", config.api_port);
    info!("API available at http://0.0.0.0:{}/api", config.api_port);

    api_handle.await?;

    Ok(())
}
