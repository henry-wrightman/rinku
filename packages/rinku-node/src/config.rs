use rinku_core::types::{GasConfig, TokenomicsConfig};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::env;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct GenesisValidator {
    pub address: String,
    pub bls_public_key: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TrustConfig {
    pub genesis_validators: Vec<GenesisValidator>,
    pub checkpoint_quorum_threshold: f64,
    pub trust_checkpoint_hash: Option<String>,
}

impl Default for TrustConfig {
    fn default() -> Self {
        Self {
            genesis_validators: Vec::new(),
            checkpoint_quorum_threshold: 0.67,
            trust_checkpoint_hash: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct P2pConfig {
    pub enabled: bool,
    pub listen_addr: String,
    pub bootstrap_peers: Vec<String>,
    pub enable_mdns: bool,
    pub max_peers: usize,
    pub connection_timeout_secs: u64,
}

impl Default for P2pConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen_addr: "/ip4/0.0.0.0/tcp/4001".to_string(),
            bootstrap_peers: Vec::new(),
            enable_mdns: true,
            max_peers: 50,
            connection_timeout_secs: 60,
        }
    }
}

impl P2pConfig {
    pub fn from_env() -> Self {
        let enabled = std::env::var("P2P_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true);
        
        let port = std::env::var("P2P_PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(4001);
        
        let listen_addr = format!("/ip4/0.0.0.0/tcp/{}", port);
        
        let bootstrap_peers: Vec<String> = std::env::var("P2P_BOOTSTRAP_PEERS")
            .map(|p| p.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect())
            .unwrap_or_default();
        
        let enable_mdns = std::env::var("P2P_MDNS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true);
        
        let max_peers = std::env::var("P2P_MAX_PEERS")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(50);
        
        let connection_timeout_secs = std::env::var("P2P_CONNECTION_TIMEOUT")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(60);
        
        Self {
            enabled,
            listen_addr,
            bootstrap_peers,
            enable_mdns,
            max_peers,
            connection_timeout_secs,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub node_id: String,
    pub data_dir: String,
    pub api_port: u16,
    pub gossip_port: u16,
    pub peers: Vec<String>,
    pub chain_id: String,
    pub network_id: String,
    pub mainnet_mode: bool,
    pub sync_verify_strict: bool,
    pub max_dag_nodes: usize,
    pub max_tips: usize,
    pub checkpoint_interval_ms: u64,
    pub gossip_interval_ms: u64,
    pub crypto_workers: usize,
    pub gas: GasConfig,
    pub tokenomics: TokenomicsConfig,
    pub gossip_enabled: bool,
    pub rate_limit_tx_max: u32,
    pub rate_limit_contract_max: u32,
    pub rate_limit_general_max: u32,
    pub static_dir: Option<String>,
    pub trust: TrustConfig,
    pub public_url: Option<String>,
    pub p2p: P2pConfig,
    pub is_genesis_node: bool,
}

impl NodeConfig {
    pub fn from_env() -> Self {
        let node_id = env::var("NODE_ID")
            .unwrap_or_else(|_| hex::encode(&rand::random::<[u8; 8]>()));

        let peers: Vec<String> = env::var("NODE_PEERS")
            .map(|p| p.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect())
            .unwrap_or_default();

        let chain_id = env::var("CHAIN_ID").unwrap_or_else(|_| "rinku-mainnet".to_string());
        let network_id = env::var("NETWORK_ID").unwrap_or_else(|_| "mainnet".to_string());

        let mainnet_mode = env::var("MAINNET_MODE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let allow_unverified_sync = env::var("ALLOW_UNVERIFIED_SYNC")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        let sync_verify_strict = env::var("STRICT_SYNC_VERIFY")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(!allow_unverified_sync);

        Self {
            node_id,
            data_dir: env::var("DATA_DIR").unwrap_or_else(|_| ".rinku-data".to_string()),
            api_port: env::var("API_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3001),
            gossip_port: env::var("GOSSIP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(4001),
            peers,
            chain_id,
            network_id,
            mainnet_mode,
            sync_verify_strict,
            max_dag_nodes: env::var("MAX_DAG_NODES")
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or(300),
            max_tips: env::var("MAX_TIPS")
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or(15),
            checkpoint_interval_ms: env::var("CHECKPOINT_INTERVAL_MS")
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or(15000),
            gossip_interval_ms: env::var("GOSSIP_INTERVAL_MS")
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or(200),
            crypto_workers: env::var("CRYPTO_WORKERS")
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| num_cpus::get().saturating_sub(1).max(1)),
            gas: GasConfig::default(),
            tokenomics: TokenomicsConfig::default(),
            gossip_enabled: env::var("GOSSIP_ENABLED")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            rate_limit_tx_max: env::var("RATE_LIMIT_TX_MAX")
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or(30),
            rate_limit_contract_max: env::var("RATE_LIMIT_CONTRACT_MAX")
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or(20),
            rate_limit_general_max: env::var("RATE_LIMIT_GENERAL_MAX")
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or(100),
            static_dir: env::var("STATIC_DIR").ok(),
            trust: TrustConfig::from_env(),
            public_url: env::var("PUBLIC_URL").ok(),
            p2p: P2pConfig::from_env(),
            is_genesis_node: env::var("IS_GENESIS_NODE")
                .map(|v| v == "true" || v == "1")
                .unwrap_or_else(|_| {
                    // Auto-detect: If no bootstrap peers configured, assume genesis node
                    env::var("P2P_BOOTSTRAP_PEERS")
                        .map(|p| p.trim().is_empty())
                        .unwrap_or(true)
                }),
        }
    }
}

impl TrustConfig {
    pub fn from_env() -> Self {
        let mut genesis_validators = Vec::new();
        
        if let Ok(genesis_data) = env::var("GENESIS_VALIDATORS") {
            for entry in genesis_data.split(';') {
                let parts: Vec<&str> = entry.split(':').collect();
                if parts.len() == 2 {
                    let mut decoded_from = None;
                    let pubkey_bytes = URL_SAFE_NO_PAD.decode(parts[1])
                        .ok()
                        .map(|b| {
                            decoded_from = Some("base64url");
                            b
                        })
                        .or_else(|| {
                            hex::decode(parts[1]).ok().map(|b| {
                                decoded_from = Some("hex");
                                b
                            })
                        });
                    if let Some(pubkey_bytes) = pubkey_bytes {
                        if let Some(source) = decoded_from {
                            info!(
                                "Parsed GENESIS_VALIDATORS entry for {}... (format={})",
                                &parts[0][..16.min(parts[0].len())],
                                source
                            );
                        }
                        genesis_validators.push(GenesisValidator {
                            address: parts[0].to_string(),
                            bls_public_key: pubkey_bytes,
                        });
                    } else {
                        warn!(
                            "Failed to parse GENESIS_VALIDATORS entry for {}... (expected base64url or hex)",
                            &parts[0][..16.min(parts[0].len())]
                        );
                    }
                }
            }
        }
        
        let checkpoint_quorum_threshold = env::var("CHECKPOINT_QUORUM_THRESHOLD")
            .ok()
            .and_then(|t| t.parse().ok())
            .unwrap_or(0.67);
        
        let trust_checkpoint_hash = env::var("TRUST_CHECKPOINT_HASH").ok();
        
        Self {
            genesis_validators,
            checkpoint_quorum_threshold,
            trust_checkpoint_hash,
        }
    }
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self::from_env()
    }
}
