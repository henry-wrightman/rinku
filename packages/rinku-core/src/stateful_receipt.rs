use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewKeySpec {
    pub key: String,
    pub path: String,
    pub schema_hash: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewKeyValue {
    pub key: String,
    pub value: Value,
    pub leaf_hash: String,
    pub proof: ViewKeyProof,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewKeyProof {
    pub siblings: Vec<String>,
    pub path_bits: Vec<bool>,
    pub root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultiProof {
    pub keys: Vec<String>,
    pub leaf_hashes: Vec<String>,
    pub shared_siblings: Vec<String>,
    pub path_bitmap: Vec<u8>,
    pub root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointFinality {
    pub checkpoint_height: u64,
    pub checkpoint_hash: String,
    pub checkpoint_timestamp: u64,
    pub state_root: String,
    pub receipt_root: String,
    pub bls_aggregated_sig: Option<String>,
    pub bls_signer_bitmap: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatefulReceipt {
    pub version: u32,
    pub tx_hash: String,
    pub chain_id: String,
    pub contract_id: String,
    pub entrypoint: String,
    pub caller: String,
    pub pre_state_root: String,
    pub post_state_root: String,
    pub view_keys: Vec<ViewKeyValue>,
    pub multi_proof: Option<MultiProof>,
    pub finality: CheckpointFinality,
    pub events: Vec<ContractEventCompact>,
    pub gas_used: u64,
    pub status: ReceiptStatus,
    pub timestamp: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_proof_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    Success,
    Revert,
    OutOfGas,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractEventCompact {
    pub name: String,
    pub data: HashMap<String, Value>,
    pub index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "type")]
pub enum VerifiableObject {
    #[serde(rename = "contract_output")]
    ContractOutput {
        receipt: StatefulReceipt,
    },
    #[serde(rename = "account_proof")]
    AccountProof {
        address: String,
        balance_micro: u64,
        nonce: u64,
        staked_micro: u64,
        checkpoint_height: u64,
        checkpoint_hash: String,
        state_root: String,
        merkle_proof: Vec<String>,
        merkle_index: usize,
        bls_aggregated_sig: Option<String>,
        bls_signer_bitmap: Option<String>,
        chain_id: Option<String>,
    },
    #[serde(rename = "tx_finality")]
    TxFinality {
        tx_hash: String,
        tx_signature: String,
        checkpoint_height: u64,
        merkle_proof: crate::merkle::MerkleProof,
        aggregated_signature: String,
        signer_bitmap: Vec<u8>,
        validator_root: String,
        chain_id: Option<String>,
    },
    #[serde(rename = "weight_proof")]
    WeightProof {
        tx_hash: String,
        aggregated_weight: crate::types::AggregatedWeight,
        checkpoint_height: u64,
        checkpoint_hash: String,
        weight_trie_root: String,
        merkle_proof: Vec<String>,
        merkle_index: usize,
        bls_aggregated_sig: Option<String>,
        chain_id: Option<String>,
    },
    #[serde(rename = "custom")]
    Custom {
        schema_id: String,
        payload: Vec<u8>,
        proof_root: String,
        merkle_proof: Vec<String>,
        checkpoint_height: u64,
        chain_id: Option<String>,
    },
}

impl VerifiableObject {
    pub fn checkpoint_height(&self) -> u64 {
        match self {
            VerifiableObject::ContractOutput { receipt } => receipt.finality.checkpoint_height,
            VerifiableObject::AccountProof { checkpoint_height, .. } => *checkpoint_height,
            VerifiableObject::TxFinality { checkpoint_height, .. } => *checkpoint_height,
            VerifiableObject::WeightProof { checkpoint_height, .. } => *checkpoint_height,
            VerifiableObject::Custom { checkpoint_height, .. } => *checkpoint_height,
        }
    }

    pub fn chain_id(&self) -> Option<&str> {
        match self {
            VerifiableObject::ContractOutput { receipt } => Some(&receipt.chain_id),
            VerifiableObject::AccountProof { chain_id, .. } => chain_id.as_deref(),
            VerifiableObject::TxFinality { chain_id, .. } => chain_id.as_deref(),
            VerifiableObject::WeightProof { chain_id, .. } => chain_id.as_deref(),
            VerifiableObject::Custom { chain_id, .. } => chain_id.as_deref(),
        }
    }

    pub fn object_type(&self) -> &'static str {
        match self {
            VerifiableObject::ContractOutput { .. } => "contract_output",
            VerifiableObject::AccountProof { .. } => "account_proof",
            VerifiableObject::TxFinality { .. } => "tx_finality",
            VerifiableObject::WeightProof { .. } => "weight_proof",
            VerifiableObject::Custom { .. } => "custom",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofExpectation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_contract_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_chain_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_checkpoint_height: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_state_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_object_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_view_keys: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofInput {
    pub label: String,
    pub proof: VerifiableObject,
    pub expectation: ProofExpectation,
}

impl ProofInput {
    pub fn validate(&self) -> Result<(), ProofValidationError> {
        let exp = &self.expectation;

        if let Some(ref expected_type) = exp.expected_object_type {
            if self.proof.object_type() != expected_type.as_str() {
                return Err(ProofValidationError::TypeMismatch {
                    expected: expected_type.clone(),
                    got: self.proof.object_type().to_string(),
                });
            }
        }

        if let Some(ref expected_chain) = exp.expected_chain_id {
            if let Some(chain) = self.proof.chain_id() {
                if chain != expected_chain.as_str() {
                    return Err(ProofValidationError::ChainMismatch {
                        expected: expected_chain.clone(),
                        got: chain.to_string(),
                    });
                }
            }
        }

        if let Some(min_height) = exp.min_checkpoint_height {
            if self.proof.checkpoint_height() < min_height {
                return Err(ProofValidationError::StaleProof {
                    min_required: min_height,
                    got: self.proof.checkpoint_height(),
                });
            }
        }

        if let Some(ref expected_contract) = exp.expected_contract_id {
            if let VerifiableObject::ContractOutput { ref receipt } = self.proof {
                if receipt.contract_id != *expected_contract {
                    return Err(ProofValidationError::ContractMismatch {
                        expected: expected_contract.clone(),
                        got: receipt.contract_id.clone(),
                    });
                }
            }
        }

        if let Some(ref required_keys) = exp.required_view_keys {
            if let VerifiableObject::ContractOutput { ref receipt } = self.proof {
                let available: Vec<&str> = receipt.view_keys.iter().map(|vk| vk.key.as_str()).collect();
                for rk in required_keys {
                    if !available.contains(&rk.as_str()) {
                        return Err(ProofValidationError::MissingViewKey {
                            key: rk.clone(),
                        });
                    }
                }
            }
        }

        if let Some(ref expected_root) = exp.expected_state_root {
            match &self.proof {
                VerifiableObject::ContractOutput { receipt } => {
                    if receipt.post_state_root != *expected_root {
                        return Err(ProofValidationError::StateRootMismatch {
                            expected: expected_root.clone(),
                            got: receipt.post_state_root.clone(),
                        });
                    }
                }
                VerifiableObject::AccountProof { state_root, .. } => {
                    if state_root != expected_root {
                        return Err(ProofValidationError::StateRootMismatch {
                            expected: expected_root.clone(),
                            got: state_root.clone(),
                        });
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProofValidationError {
    TypeMismatch { expected: String, got: String },
    ChainMismatch { expected: String, got: String },
    StaleProof { min_required: u64, got: u64 },
    ContractMismatch { expected: String, got: String },
    MissingViewKey { key: String },
    StateRootMismatch { expected: String, got: String },
    InvalidMerkleProof,
    InvalidSignature,
    MalformedProof { reason: String },
}

impl std::fmt::Display for ProofValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProofValidationError::TypeMismatch { expected, got } =>
                write!(f, "proof type mismatch: expected {}, got {}", expected, got),
            ProofValidationError::ChainMismatch { expected, got } =>
                write!(f, "chain mismatch: expected {}, got {}", expected, got),
            ProofValidationError::StaleProof { min_required, got } =>
                write!(f, "stale proof: min checkpoint {} required, got {}", min_required, got),
            ProofValidationError::ContractMismatch { expected, got } =>
                write!(f, "contract mismatch: expected {}, got {}", expected, got),
            ProofValidationError::MissingViewKey { key } =>
                write!(f, "missing required view key: {}", key),
            ProofValidationError::StateRootMismatch { expected, got } =>
                write!(f, "state root mismatch: expected {}, got {}", expected, got),
            ProofValidationError::InvalidMerkleProof =>
                write!(f, "invalid merkle proof"),
            ProofValidationError::InvalidSignature =>
                write!(f, "invalid BLS signature"),
            ProofValidationError::MalformedProof { reason } =>
                write!(f, "malformed proof: {}", reason),
        }
    }
}

impl std::error::Error for ProofValidationError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContractSchema {
    pub contract_id: String,
    pub view_keys: Vec<ViewKeySpec>,
    pub accepted_proof_types: Vec<String>,
    pub schema_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatefulContractDeploy {
    pub contract_id: String,
    pub creator: String,
    pub wasm_base64: String,
    pub init_state: HashMap<String, Value>,
    pub schema: ContractSchema,
    pub ts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatefulContractCall {
    pub contract_id: String,
    pub entrypoint: String,
    pub input: HashMap<String, Value>,
    pub proof_inputs: Vec<ProofInput>,
    pub pre_state_hash: String,
    pub post_state_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidatedProofContext {
    pub label: String,
    pub object_type: String,
    pub checkpoint_height: u64,
    pub extracted_values: HashMap<String, Value>,
}

impl ValidatedProofContext {
    pub fn from_proof_input(input: &ProofInput) -> Self {
        let mut extracted = HashMap::new();

        match &input.proof {
            VerifiableObject::ContractOutput { receipt } => {
                for vk in &receipt.view_keys {
                    extracted.insert(
                        format!("{}.{}", receipt.contract_id, vk.key),
                        vk.value.clone(),
                    );
                }
                extracted.insert("_contract_id".to_string(), Value::String(receipt.contract_id.clone()));
                extracted.insert("_entrypoint".to_string(), Value::String(receipt.entrypoint.clone()));
                extracted.insert("_status".to_string(), serde_json::to_value(&receipt.status).unwrap_or(Value::Null));
            }
            VerifiableObject::AccountProof { address, balance_micro, nonce, staked_micro, .. } => {
                extracted.insert("address".to_string(), Value::String(address.clone()));
                extracted.insert("balance_micro".to_string(), Value::Number(serde_json::Number::from(*balance_micro)));
                extracted.insert("nonce".to_string(), Value::Number(serde_json::Number::from(*nonce)));
                extracted.insert("staked_micro".to_string(), Value::Number(serde_json::Number::from(*staked_micro)));
            }
            VerifiableObject::TxFinality { tx_hash, tx_signature, .. } => {
                extracted.insert("tx_hash".to_string(), Value::String(tx_hash.clone()));
                extracted.insert("tx_signature".to_string(), Value::String(tx_signature.clone()));
            }
            VerifiableObject::WeightProof { tx_hash, aggregated_weight, .. } => {
                extracted.insert("tx_hash".to_string(), Value::String(tx_hash.clone()));
                extracted.insert("trust_score".to_string(), Value::Number(serde_json::Number::from(aggregated_weight.trust_score())));
                extracted.insert("net_weight".to_string(), Value::Number(serde_json::Number::from(aggregated_weight.net_weight)));
            }
            VerifiableObject::Custom { schema_id, .. } => {
                extracted.insert("_schema_id".to_string(), Value::String(schema_id.clone()));
            }
        }

        Self {
            label: input.label.clone(),
            object_type: input.proof.object_type().to_string(),
            checkpoint_height: input.proof.checkpoint_height(),
            extracted_values: extracted,
        }
    }
}

pub fn compute_view_key_leaf_hash(contract_id: &str, key: &str, value: &Value) -> String {
    let canonical = format!("viewkey:{}:{}:{}", contract_id, key, value);
    hex::encode(crate::crypto::sha256(canonical.as_bytes()))
}

pub fn compute_contract_state_root(contract_id: &str, view_keys: &[ViewKeyValue]) -> String {
    if view_keys.is_empty() {
        return hex::encode(crate::crypto::sha256(
            format!("contract_state:{}", contract_id).as_bytes()
        ));
    }

    let leaf_hashes: Vec<[u8; 32]> = view_keys.iter().map(|vk| {
        let bytes = hex::decode(&vk.leaf_hash).unwrap_or_else(|_| vec![0u8; 32]);
        let mut arr = [0u8; 32];
        let len = bytes.len().min(32);
        arr[..len].copy_from_slice(&bytes[..len]);
        arr
    }).collect();

    match crate::merkle::MerkleTree::new(leaf_hashes) {
        Ok(tree) => tree.root(),
        Err(_) => hex::encode(crate::crypto::sha256(
            format!("contract_state:{}", contract_id).as_bytes()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_key_leaf_hash() {
        let hash = compute_view_key_leaf_hash("sc_test", "balance", &Value::Number(serde_json::Number::from(100)));
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_proof_input_validation_type_mismatch() {
        let input = ProofInput {
            label: "test".to_string(),
            proof: VerifiableObject::AccountProof {
                address: "abc".to_string(),
                balance_micro: 100,
                nonce: 1,
                staked_micro: 0,
                checkpoint_height: 10,
                checkpoint_hash: "hash".to_string(),
                state_root: "root".to_string(),
                merkle_proof: vec![],
                merkle_index: 0,
                bls_aggregated_sig: None,
                bls_signer_bitmap: None,
                chain_id: None,
            },
            expectation: ProofExpectation {
                expected_contract_id: None,
                expected_chain_id: None,
                min_checkpoint_height: None,
                expected_state_root: None,
                expected_object_type: Some("contract_output".to_string()),
                required_view_keys: None,
            },
        };

        assert!(input.validate().is_err());
    }

    #[test]
    fn test_proof_input_validation_stale_proof() {
        let input = ProofInput {
            label: "test".to_string(),
            proof: VerifiableObject::AccountProof {
                address: "abc".to_string(),
                balance_micro: 100,
                nonce: 1,
                staked_micro: 0,
                checkpoint_height: 5,
                checkpoint_hash: "hash".to_string(),
                state_root: "root".to_string(),
                merkle_proof: vec![],
                merkle_index: 0,
                bls_aggregated_sig: None,
                bls_signer_bitmap: None,
                chain_id: None,
            },
            expectation: ProofExpectation {
                expected_contract_id: None,
                expected_chain_id: None,
                min_checkpoint_height: Some(10),
                expected_state_root: None,
                expected_object_type: None,
                required_view_keys: None,
            },
        };

        assert!(input.validate().is_err());
    }

    #[test]
    fn test_proof_input_validation_success() {
        let input = ProofInput {
            label: "balance_check".to_string(),
            proof: VerifiableObject::AccountProof {
                address: "abc".to_string(),
                balance_micro: 1_000_000_000,
                nonce: 5,
                staked_micro: 0,
                checkpoint_height: 100,
                checkpoint_hash: "hash".to_string(),
                state_root: "root".to_string(),
                merkle_proof: vec![],
                merkle_index: 0,
                bls_aggregated_sig: None,
                bls_signer_bitmap: None,
                chain_id: None,
            },
            expectation: ProofExpectation {
                expected_contract_id: None,
                expected_chain_id: None,
                min_checkpoint_height: Some(50),
                expected_state_root: None,
                expected_object_type: Some("account_proof".to_string()),
                required_view_keys: None,
            },
        };

        assert!(input.validate().is_ok());
    }

    #[test]
    fn test_validated_proof_context_extraction() {
        let input = ProofInput {
            label: "oracle_price".to_string(),
            proof: VerifiableObject::AccountProof {
                address: "validator_01".to_string(),
                balance_micro: 5_000_000_000,
                nonce: 42,
                staked_micro: 1_000_000_000,
                checkpoint_height: 200,
                checkpoint_hash: "cp_hash".to_string(),
                state_root: "sr".to_string(),
                merkle_proof: vec![],
                merkle_index: 0,
                bls_aggregated_sig: None,
                bls_signer_bitmap: None,
                chain_id: None,
            },
            expectation: ProofExpectation {
                expected_contract_id: None,
                expected_chain_id: None,
                min_checkpoint_height: None,
                expected_state_root: None,
                expected_object_type: None,
                required_view_keys: None,
            },
        };

        let ctx = ValidatedProofContext::from_proof_input(&input);
        assert_eq!(ctx.label, "oracle_price");
        assert_eq!(ctx.object_type, "account_proof");
        assert_eq!(ctx.checkpoint_height, 200);
        assert_eq!(ctx.extracted_values.get("balance_micro").and_then(|v| v.as_u64()), Some(5_000_000_000));
    }

    #[test]
    fn test_verifiable_object_type() {
        let vo = VerifiableObject::AccountProof {
            address: "test".to_string(),
            balance_micro: 0,
            nonce: 0,
            staked_micro: 0,
            checkpoint_height: 0,
            checkpoint_hash: String::new(),
            state_root: String::new(),
            merkle_proof: vec![],
            merkle_index: 0,
            bls_aggregated_sig: None,
            bls_signer_bitmap: None,
            chain_id: None,
        };
        assert_eq!(vo.object_type(), "account_proof");
    }

    #[test]
    fn test_contract_state_root_empty() {
        let root = compute_contract_state_root("sc_test", &[]);
        assert!(!root.is_empty());
        assert_eq!(root.len(), 64);
    }

    #[test]
    fn test_receipt_status_serialization() {
        let s = serde_json::to_string(&ReceiptStatus::Success).unwrap();
        assert_eq!(s, "\"success\"");
    }
}
