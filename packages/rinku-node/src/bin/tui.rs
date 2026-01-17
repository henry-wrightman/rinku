use std::sync::Arc;
use anyhow::Result;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use rinku_node::config::NodeConfig;
use rinku_node::state::NodeState;
use rinku_node::tui::run_tui;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()),
        ))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .init();

    let config = NodeConfig::from_env();
    let node_id = config.node_id.clone();
    
    info!("Loading Rinku Node state for TUI...");
    
    let data_path = std::path::Path::new(&config.data_dir);
    if !data_path.exists() {
        std::fs::create_dir_all(&config.data_dir)?;
    }

    let state = Arc::new(NodeState::new(config).await?);
    
    run_tui(state, node_id).await
}
