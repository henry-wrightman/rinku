use serde::{Deserialize, Serialize};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, CryptoError>;

#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("Invalid key length: expected {expected}, got {actual}")]
    InvalidKeyLength { expected: usize, actual: usize },

    #[error("Signing error: {0}")]
    SigningError(String),

    #[error("Verification error: {0}")]
    VerificationError(String),

    #[error("Invalid public key: {0}")]
    InvalidPublicKey(String),

    #[error("Invalid signature format")]
    InvalidSignature,

    #[error("Serialization error: {0}")]
    SerializationError(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyPair {
    pub public_key: Vec<u8>,
    pub private_key: Vec<u8>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountState {
    pub fingerprint: String,
    pub balance: f64,
    pub nonce: u64,
    pub first_tx_timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransactionKind {
    User,
    Consolidation,
}

impl Default for TransactionKind {
    fn default() -> Self {
        TransactionKind::User
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub from: String,
    pub to: String,
    pub amount: f64,
    pub fee: f64,
    pub nonce: u64,
    #[serde(rename = "tipUrls")]
    pub tip_urls: Vec<String>,
    pub sig: String,
    pub ts: u64,
    #[serde(default)]
    pub kind: Option<TransactionKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedTransaction {
    #[serde(flatten)]
    pub tx: Transaction,
    pub hash: String,
}

#[derive(Debug, Clone)]
pub struct MerkleNode {
    pub hash: String,
    pub left: Option<Box<MerkleNode>>,
    pub right: Option<Box<MerkleNode>>,
    pub data: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DAGNode {
    pub tx: SignedTransaction,
    pub parent_urls: Vec<String>,
    pub children: Vec<String>,
    pub weight: f64,
    pub confirmed: bool,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalityMetadata {
    pub checkpoint_id: String,
    pub checkpoint_height: u64,
    pub finalized_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Weight {
    pub account_age: f64,
    pub balance: f64,
    pub total: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GasPrice {
    pub current: f64,
    pub min: f64,
    pub max: f64,
    pub avg_last_100: f64,
    pub last_updated: u64,
}
