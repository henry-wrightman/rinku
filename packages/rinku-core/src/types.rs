use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionKind {
    Transfer,
    Stake,
    Unstake,
    Contract,
    Consolidation,
    Reward,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    pub from: String,
    pub to: String,
    pub amount: f64,
    pub nonce: u64,
    pub timestamp: u64,
    pub parents: Vec<String>,
    #[serde(default)]
    pub kind: Option<TransactionKind>,
    #[serde(default)]
    pub gas_limit: Option<u64>,
    #[serde(default)]
    pub gas_price: Option<f64>,
    #[serde(default)]
    pub data: Option<String>,
    #[serde(default, rename = "txSignature", skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedTransaction {
    #[serde(flatten)]
    pub tx: Transaction,
    pub hash: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub address: String,
    pub balance: f64,
    pub nonce: u64,
    pub first_seen: u64,
    #[serde(default)]
    pub staked: f64,
    #[serde(default)]
    pub unbonding: f64,
    #[serde(default)]
    pub unbonding_release: Option<u64>,
}

impl Account {
    pub fn new(address: String, timestamp: u64) -> Self {
        Self {
            address,
            balance: 0.0,
            nonce: 0,
            first_seen: timestamp,
            staked: 0.0,
            unbonding: 0.0,
            unbonding_release: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub height: u64,
    pub hash: String,
    pub previous_hash: Option<String>,
    pub tx_merkle_root: String,
    pub state_root: String,
    pub receipt_root: String,
    pub tip_count: u32,
    pub timestamp: u64,
    pub validator_signatures: Vec<ValidatorSignature>,
    #[serde(default)]
    pub aggregated_signature: Option<String>,
    #[serde(default)]
    pub signer_bitmap: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatorSignature {
    pub validator: String,
    pub signature: String,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Validator {
    pub address: String,
    pub stake: f64,
    pub first_stake_time: u64,
    #[serde(default)]
    pub bls_public_key: Option<String>,
    #[serde(default)]
    pub missed_checkpoints: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DagNode {
    pub hash: String,
    pub tx: SignedTransaction,
    pub parents: Vec<String>,
    pub children: Vec<String>,
    pub weight: f64,
    pub finalized: bool,
    #[serde(default)]
    pub checkpoint_height: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MerkleProof {
    pub leaf_hash: String,
    pub siblings: Vec<String>,
    pub path_bits: Vec<bool>,
    pub root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactProof {
    pub version: u8,
    pub tx_hash: String,
    pub tx_signature: String,
    pub checkpoint_height: u64,
    pub merkle_proof: MerkleProof,
    pub aggregated_signature: String,
    pub signer_bitmap: Vec<u8>,
    pub validator_root: String,
    #[serde(default)]
    pub chain_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GasConfig {
    pub min_gas_price: f64,
    pub max_gas_price: f64,
    pub target_txs_per_period: u32,
    pub adjustment_factor: f64,
    pub period_duration_ms: u64,
}

impl Default for GasConfig {
    fn default() -> Self {
        Self {
            min_gas_price: 0.001,
            max_gas_price: 10.0, // Match TypeScript GAS_MAX_FEE
            target_txs_per_period: 15000, // 1000 TPS × 15s period
            adjustment_factor: 0.125, // 12.5% max change per period
            period_duration_ms: 15000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenomicsConfig {
    pub max_supply: f64,
    pub genesis_allocation: f64,
    pub halving_interval: u64,
    pub initial_emission: f64,
    pub min_emission: f64,
    pub validator_floor: f64,
}

impl Default for TokenomicsConfig {
    fn default() -> Self {
        Self {
            max_supply: 30_000_000.0,
            genesis_allocation: 6_000_000.0,
            halving_interval: 3_150_000,
            initial_emission: 3.934,
            min_emission: 0.123,
            validator_floor: 0.7,
        }
    }
}

pub type AccountMap = HashMap<String, Account>;
pub type TransactionMap = HashMap<String, SignedTransaction>;
