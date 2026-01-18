use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionKind {
    Transfer,
    Stake,
    Unstake,
    #[serde(alias = "claimRewards")]
    ClaimRewards,
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
    #[serde(
        default,
        rename = "txSignature",
        skip_serializing_if = "Option::is_none"
    )]
    pub signature: Option<String>,
    /// Previous transaction hash from the same account (for per-account chain)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_account_tx: Option<String>,
    /// Self-provable proof URL for the previous account transaction (enables offline crawling)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_account_proof_url: Option<String>,
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
    /// Hash of the most recent transaction from this account (for per-account chain)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tx_hash: Option<String>,
    /// Self-provable proof URL for the most recent transaction (enables offline history crawling)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tx_proof_url: Option<String>,
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
            last_tx_hash: None,
            last_tx_proof_url: None,
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
    #[serde(default)]
    pub bls_public_key: Option<String>,
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
            max_gas_price: 10.0,         // Match TypeScript GAS_MAX_FEE
            target_txs_per_period: 3000, // 20 TPS × 15s period
            adjustment_factor: 0.125,    // 12.5% max change per period
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

/// A compact entry in a wallet's transaction chain for distributed history sharing
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletChainEntry {
    pub hash: String,
    pub to: String,
    pub amount: f64,
    pub fee: f64,
    pub nonce: u64,
    pub timestamp: u64,
    pub signature: String,
    #[serde(default)]
    pub kind: Option<TransactionKind>,
    /// Previous transaction hash in this account's chain
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_tx: Option<String>,
    /// Self-provable proof URL (rinku://sp/...) for offline verification
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_url: Option<String>,
    /// Checkpoint height where this tx was finalized (for verification)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_height: Option<u64>,
}

/// A wallet's complete transaction chain for distributed history sharing
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalletChain {
    /// The wallet address this chain belongs to
    pub address: String,
    /// Chain entries ordered from newest to oldest
    pub entries: Vec<WalletChainEntry>,
    /// Hash of the most recent transaction (chain head)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_tx: Option<String>,
    /// Current account nonce (for validation)
    pub nonce: u64,
    /// Timestamp when this chain was exported
    pub exported_at: u64,
    /// Node/wallet that exported this chain (for attribution)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exported_by: Option<String>,
}

impl WalletChain {
    pub fn new(address: String, nonce: u64) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            address,
            entries: Vec::new(),
            head_tx: None,
            nonce,
            exported_at: now,
            exported_by: None,
        }
    }

    /// Verify that the chain is internally consistent (prev_tx pointers form a valid chain)
    pub fn verify_chain_links(&self) -> bool {
        if self.entries.is_empty() {
            return true;
        }
        
        // Check that head_tx matches first entry
        if let Some(ref head) = self.head_tx {
            if self.entries.first().map(|e| &e.hash) != Some(head) {
                return false;
            }
        }
        
        // Verify chain links: each entry's prev_tx should match next entry's hash
        for i in 0..self.entries.len() - 1 {
            let current = &self.entries[i];
            let next = &self.entries[i + 1];
            if current.prev_tx.as_ref() != Some(&next.hash) {
                return false;
            }
        }
        
        // Last entry should have prev_tx = None (genesis) or point to unknown older tx
        true
    }

    /// Get the number of transactions in this chain
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
