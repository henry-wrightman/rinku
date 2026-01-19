use std::sync::Arc;
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
#[cfg(feature = "p2p")]
mod network;
mod persistence;
mod proofs;
mod rewards;
mod slashing;
mod state;
mod state_trie;
mod tip_consolidator;
mod trust;
mod validator;
mod validator_identity;
mod versioning;
#[cfg(feature = "zk")]
mod zk;
#[cfg(feature = "tui")]
pub mod tui;

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
    let args: Vec<String> = std::env::args().collect();
    let tui_mode = args.iter().any(|a| a == "--tui" || a == "-t");
    
    if tui_mode {
        #[cfg(feature = "tui")]
        {
            // TUI mode: write logs to a file to prevent corrupting the TUI display
            // Logs can be viewed in .rinku-data/tui.log
            std::fs::create_dir_all(".rinku-data").ok();
            let file_appender = tracing_appender::rolling::never(".rinku-data", "tui.log");
            tracing_subscriber::registry()
                .with(tracing_subscriber::EnvFilter::new(
                    std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
                ))
                .with(tracing_subscriber::fmt::layer()
                    .with_writer(file_appender)
                    .with_ansi(false))
                .init();
        }
        #[cfg(not(feature = "tui"))]
        {
            eprintln!("TUI feature not enabled. Rebuild with: cargo build --features tui");
            std::process::exit(1);
        }
    } else {
        tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new(
                std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
            ))
            .with(tracing_subscriber::fmt::layer())
            .init();
    }

    info!("Starting Rinku Node v0.1.0");
    info!("Process PID: {}", std::process::id());

    let config = NodeConfig::from_env();
    info!("Node ID: {}", config.node_id);
    info!("Data dir: {}", config.data_dir);

    // Log trust configuration
    if !config.trust.genesis_validators.is_empty() {
        info!(
            "Trust config: {} genesis validator(s), quorum threshold: {:.0}%",
            config.trust.genesis_validators.len(),
            config.trust.checkpoint_quorum_threshold * 100.0
        );
        for gv in &config.trust.genesis_validators {
            info!(
                "  Genesis validator: {}...",
                &gv.address[..16.min(gv.address.len())]
            );
        }
    } else {
        info!("Trust config: TESTNET MODE (no genesis validators, signatures not verified)");
    }
    if let Some(ref trusted_hash) = config.trust.trust_checkpoint_hash {
        info!(
            "Weak subjectivity checkpoint: {}...",
            &trusted_hash[..16.min(trusted_hash.len())]
        );
    }

    // Check if data directory exists and log contents
    let data_path = std::path::Path::new(&config.data_dir);
    if data_path.exists() {
        info!("Data directory exists, checking for stale locks...");
        let sled_path = data_path.join("sled-db");
        if sled_path.exists() {
            // Try to remove any stale lock files
            let lock_path = sled_path.join("lock");
            if lock_path.exists() {
                info!("Found lock file, attempting to remove stale lock...");
                if let Err(e) = std::fs::remove_file(&lock_path) {
                    info!("Could not remove lock file (may be in use): {}", e);
                } else {
                    info!("Removed stale lock file");
                }
            }
        }
    } else {
        info!("Data directory does not exist, will create");
        std::fs::create_dir_all(&config.data_dir)?;
    }

    info!("Initializing node state...");
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
        config.peers.clone(),
        config.trust.clone(),
    );
    let bls_public_key = checkpoint_service.bls_public_key_base64();
    info!(
        "BLS public key: {}...",
        &bls_public_key[..32]
    );
    state.set_validator_info(validator_address.clone(), Some(bls_public_key)).await;
    let checkpoint_handle = tokio::spawn(async move {
        if let Err(e) = checkpoint_service.start().await {
            tracing::error!("Checkpoint service error: {}", e);
        }
    });
    info!(
        "Checkpoint service started ({}ms interval)",
        config.checkpoint_interval_ms
    );

    let fork_service = ForkRemediationService::new(state.clone());
    let fork_handle = tokio::spawn(async move {
        if let Err(e) = fork_service.start().await {
            tracing::error!("Fork remediation service error: {}", e);
        }
    });
    info!("Fork remediation service started");

    // Create GossipService - shared between background task and API
    let gossip_service = if config.gossip_enabled {
        let service = GossipService::new(
            state.clone(),
            config.peers.clone(),
            config.gossip_interval_ms,
            config.trust.clone(),
        );
        let service_for_task = service.clone();
        tokio::spawn(async move {
            if let Err(e) = service_for_task.start().await {
                tracing::error!("Gossip service error: {}", e);
            }
        });
        info!(
            "Gossip service started ({}ms interval)",
            config.gossip_interval_ms
        );
        Some(service)
    } else {
        None
    };

    let tip_consolidator = TipConsolidator::new(state.clone(), validator_address);
    let tip_handle = tokio::spawn(async move {
        if let Err(e) = tip_consolidator.start().await {
            tracing::error!("Tip consolidator error: {}", e);
        }
    });
    info!("Tip consolidation service started");

    // Periodic snapshot saving (every 60 seconds)
    let snapshot_state = state.clone();
    let snapshot_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Err(e) = snapshot_state.save_snapshot().await {
                tracing::error!("Failed to save snapshot: {}", e);
            } else {
                tracing::debug!("Snapshot saved successfully");
            }
        }
    });
    info!("Snapshot persistence started (60s interval)");

    let static_dir = config.static_dir.as_ref().map(std::path::PathBuf::from);
    info!("STATIC_DIR config: {:?}", config.static_dir);
    let api_handle = api::start_api_server(state.clone(), gossip_service.clone(), config.api_port, static_dir).await?;

    info!("Rinku Node running on port {}", config.api_port);
    info!("API available at http://0.0.0.0:{}/api", config.api_port);
    if let Some(ref dir) = config.static_dir {
        info!("Static files enabled from: {}", dir);
    } else {
        info!("No STATIC_DIR set, API-only mode");
    }

    // Setup signal handler for graceful shutdown
    let shutdown_state = state.clone();
    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                info!("Received shutdown signal, saving state...");
                if let Err(e) = shutdown_state.save_snapshot().await {
                    tracing::error!("Failed to save final snapshot: {}", e);
                } else {
                    info!("Final snapshot saved successfully");
                }
                info!("Shutting down gracefully");
                std::process::exit(0);
            }
            Err(e) => {
                tracing::error!("Error waiting for shutdown signal: {}", e);
            }
        }
    });

    // TUI mode: run the terminal interface instead of waiting on background tasks
    #[cfg(feature = "tui")]
    if tui_mode {
        info!("Starting TUI mode...");
        let tui_state = Arc::new(state);
        let node_id = config.node_id.clone();
        return tui::run_tui(tui_state, gossip_service, node_id).await;
    }

    // Headless mode: wait on all background services
    tokio::select! {
        _ = api_handle => info!("API server stopped"),
        _ = checkpoint_handle => info!("Checkpoint service stopped"),
        _ = fork_handle => info!("Fork remediation stopped"),
        _ = tip_handle => info!("Tip consolidation stopped"),
        _ = snapshot_handle => info!("Snapshot persistence stopped"),
    }

    Ok(())
}
