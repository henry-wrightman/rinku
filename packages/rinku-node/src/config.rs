use rinku_core::types::{GasConfig, TokenomicsConfig};
use std::env;

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
pub struct NodeConfig {
    pub node_id: String,
    pub data_dir: String,
    pub api_port: u16,
    pub gossip_port: u16,
    pub peers: Vec<String>,
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
}

impl NodeConfig {
    pub fn from_env() -> Self {
        let node_id = env::var("NODE_ID")
            .unwrap_or_else(|_| hex::encode(&rand::random::<[u8; 8]>()));

        let peers: Vec<String> = env::var("NODE_PEERS")
            .map(|p| p.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())  // Filter out empty strings
                .collect())
            .unwrap_or_default();

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
                    if let Ok(pubkey_bytes) = hex::decode(parts[1]) {
                        genesis_validators.push(GenesisValidator {
                            address: parts[0].to_string(),
                            bls_public_key: pubkey_bytes,
                        });
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
