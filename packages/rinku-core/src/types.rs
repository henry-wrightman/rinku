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
    #[serde(alias = "dataOnly")]
    DataOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FastPathStatus {
    Pending,
    Confirmed,
    Finalized,
}

impl Default for FastPathStatus {
    fn default() -> Self {
        FastPathStatus::Pending
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FastPathAck {
    pub tx_hash: String,
    pub validator_address: String,
    pub validator_stake: f64,
    pub bls_signature: Option<String>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FastPathFinality {
    pub tx_hash: String,
    pub status: FastPathStatus,
    pub acks: Vec<FastPathAck>,
    pub total_stake_acked: f64,
    pub quorum_stake_required: f64,
    pub registered_at_ms: u64,
    pub confirmed_at_ms: Option<u64>,
    pub checkpoint_height: Option<u64>,
}

impl FastPathFinality {
    pub fn finality_time_ms(&self) -> Option<u64> {
        self.confirmed_at_ms.map(|confirmed| confirmed.saturating_sub(self.registered_at_ms))
    }
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
    /// Optional memo/message content (max 256 bytes for messaging apps)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// Optional references to other transaction hashes (for threading/chaining messages)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedTransaction {
    #[serde(flatten)]
    pub tx: Transaction,
    pub hash: String,
    pub signature: String,
}

impl Transaction {
    /// All transactions are now fast-path eligible for sub-500ms finality.
    /// Balance/nonce validation is performed by validators before ACKing.
    pub fn is_fast_path_eligible(&self) -> bool {
        // All transaction types can use fast-path finality
        // Validators will only ACK if the transaction is valid (correct balance, nonce, etc.)
        true
    }
    
    pub fn is_data_only(&self) -> bool {
        self.amount == 0.0 && 
        (self.memo.is_some() || self.references.is_some()) &&
        !matches!(self.kind, Some(TransactionKind::Stake) | 
                            Some(TransactionKind::Unstake) | 
                            Some(TransactionKind::ClaimRewards) |
                            Some(TransactionKind::Contract))
    }
}

impl SignedTransaction {
    pub fn is_fast_path_eligible(&self) -> bool {
        self.tx.is_fast_path_eligible()
    }
    
    pub fn is_data_only(&self) -> bool {
        self.tx.is_data_only()
    }
}

/// Micro-units multiplier: 1 RKU = 100,000,000 micro-RKU (8 decimal places)
/// This matches the precision used in hash_account_leaf_for_proof
pub const MICRO_UNITS: u64 = 100_000_000;

/// Convert f64 balance to u64 micro-units with proper rounding
pub fn to_micro_units(value: f64) -> u64 {
    (value * MICRO_UNITS as f64).round() as u64
}

/// Convert u64 micro-units back to f64 for display
pub fn from_micro_units(micro: u64) -> f64 {
    micro as f64 / MICRO_UNITS as f64
}

/// Self-contained proof of account state at a specific checkpoint
/// This proof can be verified offline without querying any node
/// 
/// ## Canonical Leaf Encoding (version 2+)
/// Account leaves are hashed using the format:
/// `SHA256("account:{address}:{balance_micro}:{nonce}:{staked_micro}")`
/// 
/// Where:
/// - `address`: lowercase hex account address (40 chars)
/// - `balance_micro`: u64 micro-units (1 RKU = 100,000,000 micro-RKU)
/// - `nonce`: u64 transaction counter
/// - `staked_micro`: u64 micro-units of staked balance
/// 
/// Example: `"account:abc123...def:1000000000:5:500000000"`
/// represents 10.0 RKU balance, nonce 5, 5.0 RKU staked
/// 
/// Internal nodes are hashed using: `SHA256("node:{left_hash}:{right_hash}")`
/// 
/// ## Verification Steps
/// 1. Reconstruct leaf hash: `SHA256("account:{address}:{balance_micro}:{nonce}:{staked_micro}")`
/// 2. Walk merkle_proof siblings from leaf to root
/// 3. Compare computed root against state_root
/// 4. Optionally verify checkpoint BLS signature against validator set
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStateProof {
    /// Proof format version. Version 2+ uses u64 micro-units for deterministic encoding.
    /// Version 3+ includes complete reward state for deterministic cross-node sync.
    #[serde(default)]
    pub version: u32,
    pub address: String,
    /// Balance in micro-units (u64). 1 RKU = 100,000,000 micro-RKU.
    /// This is the canonical value used for merkle leaf hashing.
    /// Defaults to 0 for backward compatibility with v1 proofs.
    #[serde(default)]
    pub balance_micro: u64,
    /// Balance in RKU (f64) for display convenience. Derived from balance_micro.
    #[serde(default)]
    pub balance: f64,
    pub nonce: u64,
    /// Staked amount in micro-units (u64).
    /// Defaults to 0 for backward compatibility with v1 proofs.
    #[serde(default)]
    pub staked_micro: u64,
    /// Staked amount in RKU (f64) for display convenience.
    #[serde(default)]
    pub staked: f64,
    
    // === REWARD STATE (v3+) - For deterministic cross-node synchronization ===
    /// Pending rewards in micro-units (u64). Used for direct sync instead of inference.
    #[serde(default)]
    pub pending_rewards_micro: u64,
    /// Pending rewards in RKU (f64) for display convenience.
    #[serde(default)]
    pub pending_rewards: f64,
    /// Timestamp (ms) when stake was created. 0 if not staking.
    #[serde(default)]
    pub staked_at: u64,
    /// Timestamp (ms) of last reward distribution. None if never received rewards.
    #[serde(default)]
    pub last_reward_at: Option<u64>,
    /// Total claimed rewards in micro-units (lifetime). Used for validation.
    #[serde(default)]
    pub claimed_rewards_total_micro: u64,
    /// Total claimed rewards in RKU (f64) for display convenience.
    #[serde(default)]
    pub claimed_rewards_total: f64,
    
    pub checkpoint_height: u64,
    pub checkpoint_hash: String,
    pub checkpoint_timestamp: u64,
    /// For activity-based proofs: this is the checkpoint's state_root (BLS-signed).
    /// For on-demand proofs: this is computed from current state (may differ from checkpoint).
    pub state_root: String,
    pub merkle_proof: Vec<String>,
    pub merkle_index: usize,
    pub bls_aggregated_sig: Option<String>,
    pub bls_signer_bitmap: Option<String>,
    pub tx_hash: String,
    /// True if this proof was generated on-demand from current state.
    /// On-demand proofs compute state_root from live state, which may differ
    /// from the checkpoint's committed state_root if account activity occurred
    /// after the last checkpoint finalization.
    #[serde(default)]
    pub is_on_demand: bool,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_balance_proof: Option<AccountStateProof>,
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
            latest_balance_proof: None,
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
    #[serde(default)]
    pub finalized_tx_hashes: Vec<String>,
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
    #[serde(default)]
    pub received_at_ms: Option<u64>,
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
