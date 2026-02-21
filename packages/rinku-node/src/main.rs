use std::sync::Arc;
use std::path::PathBuf;
use anyhow::Result;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod api;
mod bls;
#[cfg(feature = "p2p")]
mod cbor_codec;
mod checkpoint;
mod config;
mod consensus;
mod contract_storage;
mod contracts;
mod sparse_merkle_trie;
mod dag_pruning;
#[cfg(feature = "wasm")]
mod wasm_runtime;
mod emission;
mod fast_path;
mod fork_remediation;
mod gas;
mod gossip;
mod leader_election;
mod mempool_cleanup;
#[cfg(feature = "p2p")]
mod network;
mod persistence;
mod proofs;
mod relay;
mod storage;
mod rewards;
mod slashing;
mod state;
mod state_trie;
mod sync_verification;
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
use gossip::{CheckpointVoteSigner, GossipService};
#[cfg(feature = "p2p")]
use network::{HandshakeConfig, NetworkConfig, NetworkService};
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
    let export_key = args.iter().any(|a| a == "--export-key");
    let import_key_flag = args.iter().any(|a| a == "--import-key");
    let import_key_value = args.iter().position(|a| a == "--import-key")
        .and_then(|i| args.get(i + 1).cloned());
    let show_address = args.iter().any(|a| a == "--show-address");

    if import_key_flag && import_key_value.is_none() {
        eprintln!("Error: --import-key requires a private key hex argument.");
        eprintln!("Usage: cargo run -p rinku-node -- --import-key <PRIVATE_KEY_HEX>");
        std::process::exit(1);
    }
    let tui_mode = args.iter().any(|a| a == "--tui" || a == "-t");

    if export_key || import_key_value.is_some() || show_address {
        let config = NodeConfig::from_env();
        let key_password = std::env::var("VALIDATOR_KEY_PASSWORD")
            .unwrap_or_else(|_| "dev-password".to_string());
        if config.mainnet_mode && (key_password.is_empty() || key_password == "dev-password") {
            eprintln!("Error: MAINNET_MODE requires VALIDATOR_KEY_PASSWORD to be set (non-default).");
            std::process::exit(1);
        }
        let mut manager = ValidatorKeyManager::new(&config.data_dir);

        if export_key {
            let key_path = std::path::Path::new(&config.data_dir).join("validator.key");
            if !key_path.exists() {
                eprintln!("No wallet key found at {}/validator.key", config.data_dir);
                eprintln!();
                eprintln!("To create one, either:");
                eprintln!("  1. Start the node normally (a key will be generated automatically)");
                eprintln!("  2. Import an existing key: cargo run -p rinku-node -- --import-key <JSON>");
                std::process::exit(1);
            }
            match manager.load_key(&key_password) {
                Ok(_address) => {
                    let wallet_json = manager.wallet_json()
                        .expect("Key loaded but wallet JSON unavailable");
                    eprintln!("Wallet JSON (KEEP SECRET):");
                    println!("{}", wallet_json);
                    eprintln!();
                    eprintln!("Import this into the Explorer: Settings > Import Private Key > paste the JSON above");
                }
                Err(e) => {
                    eprintln!("Failed to load validator key: {}", e);
                    eprintln!("Check that VALIDATOR_KEY_PASSWORD is correct.");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }

        if let Some(ref key_input) = import_key_value {
            match manager.load_from_any_format(key_input) {
                Ok(address) => {
                    if let Err(e) = manager.save_key(&key_password) {
                        eprintln!("Failed to save imported key: {}", e);
                        std::process::exit(1);
                    }
                    eprintln!("Successfully imported wallet key.");
                    eprintln!("Validator wallet address: {}", address);
                    eprintln!("Key saved to: {}/validator.key (encrypted)", config.data_dir);
                    eprintln!();
                    eprintln!("Start the node normally to use this wallet.");
                }
                Err(e) => {
                    eprintln!("Failed to import key: {}", e);
                    eprintln!("Accepted formats: wallet JSON, PKCS8 DER hex, or raw 32-byte private key hex.");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }

        if show_address {
            let key_path = std::path::Path::new(&config.data_dir).join("validator.key");
            if !key_path.exists() {
                eprintln!("No wallet key found at {}/validator.key", config.data_dir);
                eprintln!();
                eprintln!("To create one, either:");
                eprintln!("  1. Start the node normally (a key will be generated automatically)");
                eprintln!("  2. Import an existing key: cargo run -p rinku-node -- --import-key <HEX>");
                std::process::exit(1);
            }
            match manager.load_key(&key_password) {
                Ok(address) => {
                    println!("{}", address);
                }
                Err(e) => {
                    eprintln!("Failed to load validator key: {}", e);
                    eprintln!("Check that VALIDATOR_KEY_PASSWORD is correct.");
                    std::process::exit(1);
                }
            }
            return Ok(());
        }
    }

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

    if config.mainnet_mode {
        info!("MAINNET_MODE enabled: enforcing strict startup requirements");
        let allow_untrusted_genesis = std::env::var("ALLOW_UNTRUSTED_GENESIS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        let has_genesis_validators = !config.trust.genesis_validators.is_empty();
        let has_trust_checkpoint = config.trust.trust_checkpoint_hash.is_some();
        info!(
            "MAINNET_MODE checks: is_genesis_node={}, allow_untrusted_genesis={}, genesis_validators={}, trusted_checkpoint={}",
            config.is_genesis_node,
            allow_untrusted_genesis,
            config.trust.genesis_validators.len(),
            has_trust_checkpoint
        );
        if !has_genesis_validators && !has_trust_checkpoint {
            if config.is_genesis_node && allow_untrusted_genesis {
                warn!("MAINNET_MODE: allowing untrusted genesis bootstrap (ALLOW_UNTRUSTED_GENESIS=true)");
            } else {
                return Err(anyhow::anyhow!(
                    "MAINNET_MODE requires GENESIS_VALIDATORS or TRUST_CHECKPOINT_HASH"
                ));
            }
        }
        if config.public_url.is_none() {
            return Err(anyhow::anyhow!(
                "MAINNET_MODE requires PUBLIC_URL for leader election"
            ));
        }
        if !config.p2p.enabled {
            return Err(anyhow::anyhow!("MAINNET_MODE requires P2P_ENABLED"));
        }
        if !config.is_genesis_node && config.p2p.bootstrap_peers.is_empty() {
            return Err(anyhow::anyhow!(
                "MAINNET_MODE validator requires P2P_BOOTSTRAP_PEERS"
            ));
        }
        if config.p2p.enable_mdns {
            warn!("MAINNET_MODE: P2P_MDNS is enabled; consider disabling for production");
        }
    }

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

    // FRESH_START: Wipe persistent storage to force a clean genesis
    let fresh_start = std::env::var("FRESH_START")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    
    // Check if data directory exists and log contents
    let data_path = std::path::Path::new(&config.data_dir);
    if fresh_start && data_path.exists() {
        warn!("FRESH_START=true: Wiping data directory {}", config.data_dir);
        let redb_path = data_path.join("redb.db");
        if redb_path.exists() {
            if let Err(e) = std::fs::remove_file(&redb_path) {
                warn!("Could not remove redb.db: {}", e);
            } else {
                info!("Removed redb.db for fresh start");
            }
        }
        let vi_path = data_path.join("validator-identity");
        if vi_path.exists() {
            if let Err(e) = std::fs::remove_dir_all(&vi_path) {
                warn!("Could not remove validator-identity dir: {}", e);
            } else {
                info!("Removed validator-identity dir for fresh start");
            }
        }
        let sled_path = data_path.join("sled-db");
        if sled_path.exists() {
            if let Err(e) = std::fs::remove_dir_all(&sled_path) {
                warn!("Could not remove sled-db dir: {}", e);
            } else {
                info!("Removed sled-db dir for fresh start");
            }
        }
    } else if data_path.exists() {
        info!("Data directory exists, checking for stale locks...");
        let sled_path = data_path.join("sled-db");
        if sled_path.exists() {
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

    let key_password = std::env::var("VALIDATOR_KEY_PASSWORD").unwrap_or_else(|_| "dev-password".to_string());
    let key_path = std::env::var("VALIDATOR_KEY_PATH").ok();
    let key_hex = std::env::var("VALIDATOR_KEY_HEX").ok();

    if config.mainnet_mode {
        if key_password.is_empty() || key_password == "dev-password" {
            return Err(anyhow::anyhow!(
                "MAINNET_MODE requires VALIDATOR_KEY_PASSWORD (non-default)"
            ));
        }
    }

    let mut validator_manager = ValidatorKeyManager::new(&config.data_dir);
    if let Some(path) = key_path {
        validator_manager.set_key_path(PathBuf::from(path));
    }

    let validator_address = if let Some(ref hex) = key_hex {
        validator_manager.load_from_hex(hex).ok()
    } else {
        validator_manager.load_or_generate(&key_password).ok()
    };

    if config.mainnet_mode && key_hex.is_none() {
        if let Err(e) = validator_manager.validate_key_permissions() {
            return Err(anyhow::anyhow!("Validator key file permissions invalid: {}", e));
        }
    }
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

    // CRITICAL: When GENESIS_VALIDATORS is set, it becomes the authoritative source
    // of truth for the validator set. Both registries must be replaced to prevent
    // stale validators from persisting across restarts/redeployments.
    if !config.trust.genesis_validators.is_empty() {
        let genesis_seed: Vec<(String, Vec<u8>)> = config
            .trust
            .genesis_validators
            .iter()
            .map(|v| (v.address.clone(), v.bls_public_key.clone()))
            .collect();
        
        // Replace state.validators first (used for checkpoint signature verification)
        state.replace_validators_with_genesis(&genesis_seed).await;
        
        // Replace ValidatorIdentityService.active_validators (used for leader election)
        if let Some(ref vi) = validator_identity {
            let mut vi_guard = vi.write().await;
            vi_guard.seed_genesis_validators(&genesis_seed);
        }
        
        let genesis_addresses: std::collections::HashSet<String> = genesis_seed.iter().map(|(a, _)| a.clone()).collect();
        {
            use crate::validator_identity::MIN_VALIDATOR_STAKE;
            let mut rewards = state.rewards.write().await;
            let existing_stakes: Vec<String> = rewards.get_all_stakes().iter().map(|s| s.staker.clone()).collect();
            let mut removed = 0;
            for staker in &existing_stakes {
                if !genesis_addresses.contains(staker) {
                    rewards.remove_stake(staker);
                    removed += 1;
                }
            }
            if removed > 0 {
                info!("Removed {} stale stake(s) from rewards service (not in genesis set)", removed);
            }
            let mut registered = 0;
            for (address, _) in &genesis_seed {
                if rewards.get_stake(address).is_none() {
                    if let Ok(_) = rewards.stake(address, MIN_VALIDATOR_STAKE) {
                        registered += 1;
                    }
                }
            }
            if registered > 0 {
                info!(
                    "Registered {} genesis validator stake(s) in rewards service ({} RKU each)",
                    registered, MIN_VALIDATOR_STAKE
                );
            }
        }
        
        info!(
            "Seeded {} genesis validator(s) into validator registry",
            genesis_seed.len()
        );
        
        // Clean up ghost accounts: remove accounts that are not genesis validators,
        // not the faucet, and have 0 balance + 0 staked (leftover from old snapshots)
        state.cleanup_stale_accounts(&genesis_addresses).await;
    }
    
    // Sync stakes to accounts AFTER genesis validator replacement
    // This ensures only current stakers get accounts, not stale ones from old snapshots
    state.sync_stakes_to_accounts().await;

    let mut checkpoint_service = CheckpointService::new(
        state.clone(),
        config.checkpoint_interval_ms,
        validator_address.clone(),
        config.peers.clone(),
        config.trust.clone(),
        config.mainnet_mode,
    )
    .with_local_url(config.public_url.clone());
    
    if let Some(ref vi) = validator_identity {
        let vi_guard = vi.read().await;
        if let (Some(private_key), Some(public_key)) = (
            vi_guard.local_bls_private_key().map(|k| k.to_vec()),
            vi_guard.local_bls_public_key().map(|k| k.to_vec()),
        ) {
            checkpoint_service = checkpoint_service.with_bls_keypair(private_key, public_key);
            info!("Loaded persistent BLS keypair from validator identity service");
        }
        checkpoint_service = checkpoint_service.with_validator_identity(vi.clone());
    }
    let checkpoint_vote_signer = CheckpointVoteSigner {
        validator_address: checkpoint_service.validator_address(),
        bls_private_key: checkpoint_service.bls_private_key_bytes(),
        bls_public_key: checkpoint_service.bls_public_key_bytes(),
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
    // When GENESIS_VALIDATORS is set, it's authoritative - don't allow auto-registration
    // of validators that aren't in the genesis set. Otherwise, allow auto-registration
    // for non-production or single-node setups.
    let has_genesis_validators = !config.trust.genesis_validators.is_empty();
    state.set_validator_info(
        validator_address.clone(), 
        Some(bls_public_key), 
        !has_genesis_validators  // allow_auto_register = false when GENESIS_VALIDATORS is set
    ).await;

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
            data_dir: Some(config.data_dir.clone()),
        };
        
        match NetworkService::new(network_config) {
            Ok((mut network_service, handle)) => {
                let shared_handle = Arc::new(tokio::sync::Mutex::new(handle));
                let mut handshake_config = HandshakeConfig::default();
                handshake_config.chain_id = config.chain_id.clone();
                handshake_config.network_id = config.network_id.clone();
                if config.mainnet_mode {
                    handshake_config.required_chain_id = Some(config.chain_id.clone());
                    handshake_config.required_network_id = Some(config.network_id.clone());
                }
                network_service.set_handshake_config(handshake_config);

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
                
                Some(shared_handle)
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

    #[cfg(feature = "p2p")]
    if let Some(ref handle) = network_handle {
        checkpoint_service = checkpoint_service.with_network_handle(handle.clone());
    }

    // Create GossipService BEFORE spawning CheckpointService (for checkpoint broadcast)
    let gossip_service = if config.gossip_enabled {
        let mut service = GossipService::new(
            state.clone(),
            config.peers.clone(),
            config.gossip_interval_ms,
            config.trust.clone(),
            config.sync_verify_strict,
        );
        service.set_checkpoint_vote_signer(checkpoint_vote_signer.clone());
        
        // Wire up validator identity service for syncing validator registry
        if let Some(ref vi) = validator_identity {
            service.set_validator_identity(vi.clone());
        }
        
        // Wire up the libp2p network handle if available
        #[cfg(feature = "p2p")]
        if let Some(ref handle) = network_handle {
            service.set_network_handle(handle.clone());
            info!("GossipService connected to libp2p network");
        }
        
        Some(std::sync::Arc::new(service))
    } else {
        None
    };
    
    // Wire up GossipService to CheckpointService for immediate checkpoint broadcast
    if let Some(ref gs) = gossip_service {
        checkpoint_service = checkpoint_service.with_gossip_service(gs.clone());
    }

    let checkpoint_handle = tokio::spawn(async move {
        if let Err(e) = checkpoint_service.start().await {
            tracing::error!("Checkpoint service error: {}", e);
        }
    });
    info!(
        "Checkpoint service started ({}ms interval)",
        config.checkpoint_interval_ms
    );
    
    // Start GossipService background task
    if let Some(ref gs) = gossip_service {
        let service_for_task = gs.clone();
        tokio::spawn(async move {
            if let Err(e) = service_for_task.start().await {
                tracing::error!("Gossip service error: {}", e);
            }
        });
        info!(
            "Gossip service started ({}ms interval)",
            config.gossip_interval_ms
        );
    }

    let tip_consolidator = TipConsolidator::new(state.clone(), validator_address);
    let tip_consolidator = if let Some(ref gs) = gossip_service {
        tip_consolidator.with_gossip_service(gs.clone())
    } else {
        tip_consolidator
    };
    let tip_handle = tokio::spawn(async move {
        if let Err(e) = tip_consolidator.start().await {
            tracing::error!("Tip consolidator error: {}", e);
        }
    });
    info!("Tip consolidation service started");

    // Mempool cleanup service - prune expired pending transactions every 30s
    let mempool_cleanup = mempool_cleanup::MempoolCleanupService::new(state.clone());
    tokio::spawn(async move {
        mempool_cleanup.start().await;
    });
    info!("Mempool cleanup service started (TTL: 120s, interval: 30s)");

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
