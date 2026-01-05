use anyhow::Result;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod api;
mod bls;
mod checkpoint;
mod config;
mod consensus;
mod contracts;
mod emission;
mod fork_remediation;
mod gas;
mod gossip;
mod persistence;
mod proofs;
mod rewards;
mod slashing;
mod state;
mod tip_consolidator;
mod validator;
mod zk;

use checkpoint::CheckpointService;
use config::NodeConfig;
use emission::EmissionService;
use fork_remediation::ForkRemediationService;
use gas::{GasConfig, GasService};
use gossip::GossipService;
use rewards::{RewardConfig, RewardsService};
use slashing::SlashingService;
use tip_consolidator::TipConsolidator;
use validator::ValidatorKeyManager;

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

    let mut validator_manager = ValidatorKeyManager::new(&config.data_dir);
    let validator_address = validator_manager.load_or_generate("dev-password").ok();
    if let Some(ref addr) = validator_address {
        info!("Validator key loaded: {}...", &addr[..16.min(addr.len())]);
    }

    let _gas_service = GasService::new(GasConfig::default());
    info!("Gas service initialized");

    let _rewards_service = RewardsService::new(RewardConfig::default());
    info!("Rewards service initialized");

    let _emission_service = EmissionService::new();
    info!("Emission service initialized (30M max supply, halving every 3.15M checkpoints)");

    let _slashing_service = SlashingService::new();
    info!("Slashing service initialized");

    let checkpoint_service = CheckpointService::new(
        state.clone(),
        config.checkpoint_interval_ms,
        validator_address.clone(),
    );
    info!("BLS public key: {}...", &checkpoint_service.bls_public_key_base64()[..32]);
    let checkpoint_handle = tokio::spawn(async move {
        if let Err(e) = checkpoint_service.start().await {
            tracing::error!("Checkpoint service error: {}", e);
        }
    });
    info!("Checkpoint service started ({}ms interval)", config.checkpoint_interval_ms);

    let fork_service = ForkRemediationService::new(state.clone());
    let fork_handle = tokio::spawn(async move {
        if let Err(e) = fork_service.start().await {
            tracing::error!("Fork remediation service error: {}", e);
        }
    });
    info!("Fork remediation service started");

    if config.gossip_enabled {
        let gossip_service = GossipService::new(
            state.clone(),
            config.peers.clone(),
            config.gossip_interval_ms,
        );
        tokio::spawn(async move {
            if let Err(e) = gossip_service.start().await {
                tracing::error!("Gossip service error: {}", e);
            }
        });
        info!("Gossip service started ({}ms interval)", config.gossip_interval_ms);
    }

    let tip_consolidator = TipConsolidator::new(state.clone(), validator_address);
    let tip_handle = tokio::spawn(async move {
        if let Err(e) = tip_consolidator.start().await {
            tracing::error!("Tip consolidator error: {}", e);
        }
    });
    info!("Tip consolidation service started");

    let api_handle = api::start_api_server(state.clone(), config.api_port).await?;

    info!("Rinku Node running on port {}", config.api_port);
    info!("API available at http://0.0.0.0:{}/api", config.api_port);

    tokio::select! {
        _ = api_handle => info!("API server stopped"),
        _ = checkpoint_handle => info!("Checkpoint service stopped"),
        _ = fork_handle => info!("Fork remediation stopped"),
        _ = tip_handle => info!("Tip consolidation stopped"),
    }

    Ok(())
}
