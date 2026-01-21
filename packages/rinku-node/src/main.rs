use std::sync::Arc;
use anyhow::Result;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod api;
mod bls;
mod checkpoint;
mod config;
mod consensus;
mod contracts;
mod dag_pruning;
mod emission;
mod fork_remediation;
mod gas;
mod gossip;
mod leader_election;
#[cfg(feature = "p2p")]
mod network;
mod persistence;
mod proofs;
mod storage;
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
#[cfg(feature = "p2p")]
use network::{NetworkConfig, NetworkService};
use rewards::{RewardConfig, RewardsService};
use consensus::ConsensusService;
use slashing::SlashingService;
use tip_consolidator::TipConsolidator;
use tokio::sync::RwLock;
use validator::ValidatorKeyManager;
use validator_identity::ValidatorIdentityService;

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

    // Log genesis node status
    if config.is_genesis_node {
        info!("Node role: GENESIS NODE (can create new chain)");
    } else {
        info!("Node role: VALIDATOR (must sync from network, {} bootstrap peers)", 
              config.p2p.bootstrap_peers.len());
    }
    
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

    let slashing_service = Arc::new(RwLock::new(SlashingService::new()));
    info!("Slashing service initialized");
    
    let consensus_service = Arc::new(RwLock::new(
        ConsensusService::new(state.clone())
            .with_slashing_service(slashing_service.clone())
    ));
    info!("Consensus service initialized with slashing integration");

    let validator_identity_path = format!("{}/validator-identity", config.data_dir);
    let validator_identity = match ValidatorIdentityService::new(&validator_identity_path) {
        Ok(service) => {
            info!("Validator identity service initialized (epoch: {})", service.current_epoch());
            Some(Arc::new(RwLock::new(service)))
        }
        Err(e) => {
            warn!("Could not initialize validator identity service: {}", e);
            None
        }
    };

    let checkpoint_service = CheckpointService::new(
        state.clone(),
        config.checkpoint_interval_ms,
        validator_address.clone(),
        config.peers.clone(),
        config.trust.clone(),
    )
    .with_local_url(config.public_url.clone());
    
    let checkpoint_service = if let Some(ref vi) = validator_identity {
        checkpoint_service.with_validator_identity(vi.clone())
    } else {
        checkpoint_service
    };
    
    if config.public_url.is_some() {
        info!("Leader election enabled for checkpoint creation");
    } else {
        info!("Leader election disabled (no PUBLIC_URL set) - this node will create checkpoints independently");
    }
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

    // Start libp2p NetworkService for production P2P (before GossipService so we can pass the handle)
    #[cfg(feature = "p2p")]
    let network_handle = if config.p2p.enabled {
        let network_config = NetworkConfig {
            listen_addr: config.p2p.listen_addr.clone(),
            bootstrap_peers: config.p2p.bootstrap_peers.clone(),
            enable_mdns: config.p2p.enable_mdns,
        };
        
        match NetworkService::new(network_config) {
            Ok((mut network_service, handle)) => {
                let peer_id = network_service.local_peer_id();
                info!("P2P network started with peer ID: {}", peer_id);
                info!("P2P listening on: {}", config.p2p.listen_addr);
                
                // Store peer info for bootstrap endpoint
                state.set_peer_info(peer_id.to_string(), config.p2p.listen_addr.clone()).await;
                
                if !config.p2p.bootstrap_peers.is_empty() {
                    info!("P2P bootstrap peers: {:?}", config.p2p.bootstrap_peers);
                }
                if config.p2p.enable_mdns {
                    info!("P2P mDNS discovery enabled (LAN peers)");
                }
                
                tokio::spawn(async move {
                    if let Err(e) = network_service.start().await {
                        tracing::error!("P2P network service error: {}", e);
                    }
                });
                
                Some(handle)
            }
            Err(e) => {
                warn!("Failed to start P2P network: {}", e);
                None
            }
        }
    } else {
        info!("P2P networking disabled (using HTTP gossip only)");
        None
    };

    // Create GossipService - shared between background task and API
    let gossip_service = if config.gossip_enabled {
        let mut service = GossipService::new(
            state.clone(),
            config.peers.clone(),
            config.gossip_interval_ms,
            config.trust.clone(),
        );
        
        // Wire up the libp2p network handle if available
        #[cfg(feature = "p2p")]
        if let Some(handle) = network_handle {
            service.set_network_handle(handle);
            info!("GossipService connected to libp2p network");
        }
        
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
