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
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FastPathStatus {
    Pending,
    Confirmed,
    Executed,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum ContractTransactionData {
    #[serde(rename = "deploy")]
    Deploy {
        #[serde(alias = "wasmBase64", alias = "wasm_base64")]
        wasm_base64: String,
        #[serde(default, alias = "initState", alias = "init_state")]
        init_state: HashMap<String, serde_json::Value>,
    },
    #[serde(rename = "call")]
    Call {
        #[serde(alias = "contractId", alias = "contract_id")]
        contract_id: String,
        entrypoint: String,
        #[serde(default)]
        input: HashMap<String, serde_json::Value>,
    },
}

impl ContractTransactionData {
    pub fn from_data_field(data: &str) -> Result<Self, String> {
        serde_json::from_str(data).map_err(|e| format!("Invalid contract data: {}", e))
    }

    pub fn to_data_field(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| format!("Failed to serialize contract data: {}", e))
    }

    pub fn is_deploy(&self) -> bool {
        matches!(self, ContractTransactionData::Deploy { .. })
    }

    pub fn is_call(&self) -> bool {
        matches!(self, ContractTransactionData::Call { .. })
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
    /// Merkle root of the transaction weight trie (for offline weight proof verification)
    /// Empty string if no weight attestations exist for this checkpoint
    #[serde(default)]
    pub weight_trie_root: String,
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

// ============================================================================
// TRANSACTION WEIGHT ATTESTATION SYSTEM
// Protocol-level trust scoring for transactions via stake-weighted validator votes
// ============================================================================

/// Vote direction for transaction weight attestations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeightVote {
    /// Boost transaction visibility/trust (+1)
    Boost,
    /// Suppress transaction visibility/trust (-1)
    Suppress,
    /// Neutral / abstain (0)
    Neutral,
}

impl WeightVote {
    pub fn value(&self) -> i64 {
        match self {
            WeightVote::Boost => 1,
            WeightVote::Suppress => -1,
            WeightVote::Neutral => 0,
        }
    }
}

impl Default for WeightVote {
    fn default() -> Self {
        WeightVote::Neutral
    }
}

/// A single validator's attestation for a transaction's weight
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeightAttestation {
    /// Validator's public key (hex-encoded)
    pub validator_pubkey: String,
    /// Validator's stake at the time of attestation (in micro-units)
    pub stake_micro: u64,
    /// The vote: boost, suppress, or neutral
    pub vote: WeightVote,
    /// BLS signature over (tx_hash || vote || checkpoint_height)
    pub bls_signature: String,
    /// Checkpoint height when this attestation was recorded
    pub checkpoint_height: u64,
}

/// Aggregated weight score for a transaction
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AggregatedWeight {
    /// Sum of stakes from validators who voted Boost
    pub boost_stake_micro: u64,
    /// Sum of stakes from validators who voted Suppress
    pub suppress_stake_micro: u64,
    /// Sum of stakes from validators who voted Neutral or didn't vote
    pub neutral_stake_micro: u64,
    /// Net weight score: boost_stake - suppress_stake (can be negative)
    pub net_weight: i64,
    /// Number of unique validators who attested
    pub attestation_count: u32,
    /// Total network stake at checkpoint (denominator for ratios)
    pub total_network_stake_micro: u64,
}

impl AggregatedWeight {
    /// Calculate boost ratio (0.0 to 1.0)
    pub fn boost_ratio(&self) -> f64 {
        if self.total_network_stake_micro == 0 {
            return 0.0;
        }
        self.boost_stake_micro as f64 / self.total_network_stake_micro as f64
    }
    
    /// Calculate suppress ratio (0.0 to 1.0)
    pub fn suppress_ratio(&self) -> f64 {
        if self.total_network_stake_micro == 0 {
            return 0.0;
        }
        self.suppress_stake_micro as f64 / self.total_network_stake_micro as f64
    }
    
    /// Calculate net weight ratio (-1.0 to 1.0)
    pub fn net_weight_ratio(&self) -> f64 {
        if self.total_network_stake_micro == 0 {
            return 0.0;
        }
        self.net_weight as f64 / self.total_network_stake_micro as f64
    }
    
    /// Trust score normalized to 0-100 scale (50 = neutral)
    pub fn trust_score(&self) -> u8 {
        let ratio = self.net_weight_ratio();
        // Map -1.0..1.0 to 0..100, with 50 as neutral
        ((ratio + 1.0) * 50.0).clamp(0.0, 100.0) as u8
    }
}

/// Self-contained proof of a transaction's weight/trust score at a specific checkpoint.
/// This proof can be verified offline without querying any node.
/// 
/// ## Verification Steps:
/// 1. Verify each attestation's BLS signature against known validator set
/// 2. Verify stake amounts match validator registry at checkpoint
/// 3. Walk merkle_proof to verify weight is included in checkpoint's weight_trie_root
/// 4. Compare computed root against checkpoint's signed weight_trie_root
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionWeightProof {
    /// Proof format version (for future upgrades)
    pub version: u32,
    /// Transaction hash this weight proof is for
    pub tx_hash: String,
    /// Individual validator attestations (may be empty if using aggregated sig)
    pub attestations: Vec<WeightAttestation>,
    /// Aggregated weight score
    pub aggregated_weight: AggregatedWeight,
    /// Checkpoint height when this proof was generated
    pub checkpoint_height: u64,
    /// Checkpoint hash for verification context
    pub checkpoint_hash: String,
    /// Timestamp of the checkpoint
    pub checkpoint_timestamp: u64,
    /// Root of the weight trie (included in checkpoint for offline verification)
    pub weight_trie_root: String,
    /// Merkle proof from tx weight leaf to weight_trie_root
    pub merkle_proof: Vec<String>,
    /// Index position in the weight trie
    pub merkle_index: usize,
    /// Aggregated BLS signature from all attesting validators (optional, for compactness)
    pub bls_aggregated_sig: Option<String>,
    /// Bitmap indicating which validators signed (for BLS aggregation)
    pub bls_signer_bitmap: Option<String>,
}

impl TransactionWeightProof {
    /// Create an empty/default weight proof for a transaction with no attestations
    pub fn empty(tx_hash: String, checkpoint_height: u64, checkpoint_hash: String, checkpoint_timestamp: u64) -> Self {
        Self {
            version: 1,
            tx_hash,
            attestations: vec![],
            aggregated_weight: AggregatedWeight::default(),
            checkpoint_height,
            checkpoint_hash,
            checkpoint_timestamp,
            weight_trie_root: String::new(),
            merkle_proof: vec![],
            merkle_index: 0,
            bls_aggregated_sig: None,
            bls_signer_bitmap: None,
        }
    }
}

/// Pending weight vote from a validator (before checkpoint aggregation)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingWeightVote {
    /// Transaction hash being voted on
    pub tx_hash: String,
    /// Validator's address/pubkey
    pub validator_pubkey: String,
    /// The vote
    pub vote: WeightVote,
    /// Timestamp when vote was cast
    pub timestamp_ms: u64,
    /// BLS signature over the vote
    pub bls_signature: Option<String>,
}

/// Weight trie leaf data (what gets hashed for Merkle inclusion)
/// Includes all fields needed for deterministic AggregatedWeight reconstruction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightTrieLeaf {
    pub tx_hash: String,
    pub boost_stake_micro: u64,
    pub suppress_stake_micro: u64,
    pub neutral_stake_micro: u64,
    pub total_network_stake_micro: u64,
    pub attestation_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayIntent {
    pub from: String,
    pub to: String,
    pub amount: f64,
    pub nonce: u64,
    pub kind: Option<TransactionKind>,
    #[serde(default)]
    pub memo: Option<String>,
    #[serde(default)]
    pub references: Option<Vec<String>>,
    #[serde(default)]
    pub data: Option<String>,
    pub max_gas_price: f64,
    pub expiry_ms: u64,
    pub public_key: String,
    pub intent_hash: String,
    pub intent_signature: String,
}

impl RelayIntent {
    fn js_compatible_number(v: f64) -> serde_json::Value {
        if v.fract() == 0.0 && v.abs() < (i64::MAX as f64) {
            serde_json::Value::Number(serde_json::Number::from(v as i64))
        } else {
            serde_json::json!(v)
        }
    }

    pub fn canonical_fields(&self) -> String {
        let mut obj = serde_json::Map::new();
        obj.insert("amount".into(), Self::js_compatible_number(self.amount));
        if let Some(ref data) = self.data {
            obj.insert("data".into(), serde_json::Value::String(data.clone()));
        }
        obj.insert("expiryMs".into(), serde_json::json!(self.expiry_ms));
        obj.insert("from".into(), serde_json::Value::String(self.from.clone()));
        if let Some(ref kind) = self.kind {
            obj.insert("kind".into(), serde_json::to_value(kind).unwrap_or_default());
        }
        obj.insert("maxGasPrice".into(), Self::js_compatible_number(self.max_gas_price));
        if let Some(ref memo) = self.memo {
            obj.insert("memo".into(), serde_json::Value::String(memo.clone()));
        }
        obj.insert("nonce".into(), serde_json::json!(self.nonce));
        if let Some(ref refs) = self.references {
            obj.insert("references".into(), serde_json::to_value(refs).unwrap_or_default());
        }
        obj.insert("to".into(), serde_json::Value::String(self.to.clone()));
        serde_json::to_string(&obj).unwrap_or_default()
    }

    pub fn is_expired(&self) -> bool {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now_ms > self.expiry_ms
    }
}
