use rinku_core::types::{GasConfig, TokenomicsConfig};
use std::env;

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
}

impl NodeConfig {
    pub fn from_env() -> Self {
        let node_id = env::var("NODE_ID")
            .unwrap_or_else(|_| hex::encode(&rand::random::<[u8; 8]>()));

        let peers: Vec<String> = env::var("NODE_PEERS")
            .map(|p| p.split(',').map(|s| s.trim().to_string()).collect())
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
        }
    }
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self::from_env()
    }
}
