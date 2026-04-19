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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        freshness: Option<ProofFreshness>,
    },
    #[serde(rename = "account_proof")]
    AccountProof {
        address: String,
        balance_micro: u64,
        #[serde(default)]
        balance: f64,
        nonce: u64,
        staked_micro: u64,
        #[serde(default)]
        staked: f64,
        checkpoint_height: u64,
        checkpoint_hash: String,
        #[serde(default)]
        checkpoint_timestamp: u64,
        state_root: String,
        merkle_proof: Vec<String>,
        merkle_index: usize,
        bls_aggregated_sig: Option<String>,
        bls_signer_bitmap: Option<String>,
        #[serde(default)]
        is_on_demand: bool,
        chain_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        freshness: Option<ProofFreshness>,
    },
    #[serde(rename = "tx_finality")]
    TxFinality {
        tx_hash: String,
        tx_signature: String,
        tx_from: String,
        tx_to: String,
        tx_amount: f64,
        tx_nonce: u64,
        tx_timestamp: u64,
        checkpoint_height: u64,
        checkpoint_hash: String,
        checkpoint_timestamp: u64,
        tx_merkle_root: String,
        state_root: String,
        receipt_root: String,
        tip_count: u32,
        merkle_proof: Vec<String>,
        merkle_index: usize,
        bls_aggregated_sig: String,
        bls_signer_bitmap: String,
        signer_count: usize,
        signer_membership_proofs: Vec<MerkleSumProof>,
        validator_sum_tree_root: MerkleSumRoot,
        chain_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        freshness: Option<ProofFreshness>,
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        freshness: Option<ProofFreshness>,
    },
    #[serde(rename = "custom")]
    Custom {
        schema_id: String,
        payload: Vec<u8>,
        proof_root: String,
        merkle_proof: Vec<String>,
        checkpoint_height: u64,
        chain_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        freshness: Option<ProofFreshness>,
    },
    #[serde(rename = "batch_proof")]
    BatchProof {
        finality: CheckpointFinality,
        tx_hashes: Vec<String>,
        multiproof: crate::merkle::MerkleMultiProof,
        receipts: Option<Vec<StatefulReceipt>>,
        chain_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        freshness: Option<ProofFreshness>,
    },
    #[serde(rename = "fast_path_proof")]
    FastPathProof {
        tx_hash: String,
        tx_from: String,
        tx_to: String,
        tx_amount: f64,
        tx_nonce: u64,
        write_set_hash: String,
        micro_checkpoint_seq: u64,
        state_root: String,
        merkle_proof: Vec<String>,
        merkle_index: usize,
        finality_ms: u64,
        confirmed_validators: Vec<String>,
        chain_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        freshness: Option<ProofFreshness>,
    },
    #[serde(rename = "state_witness")]
    StateWitness {
        contract_id: Option<String>,
        entries: Vec<StateWitnessEntry>,
        state_root: String,
        checkpoint_height: u64,
        checkpoint_hash: String,
        bls_aggregated_sig: Option<String>,
        bls_signer_bitmap: Option<String>,
        chain_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        freshness: Option<ProofFreshness>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofFreshness {
    pub generated_at_checkpoint: u64,
    pub generated_at_timestamp: u64,
    pub chain_tip_at_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_checkpoints: Option<u64>,
}

impl ProofFreshness {
    pub fn age(&self, current_checkpoint: u64) -> u64 {
        current_checkpoint.saturating_sub(self.generated_at_checkpoint)
    }

    pub fn is_fresh(&self, current_checkpoint: u64) -> bool {
        match self.max_age_checkpoints {
            Some(max_age) => self.age(current_checkpoint) <= max_age,
            None => true,
        }
    }

    pub fn new(checkpoint_height: u64, timestamp: u64, chain_tip: u64, max_age: Option<u64>) -> Self {
        Self {
            generated_at_checkpoint: checkpoint_height,
            generated_at_timestamp: timestamp,
            chain_tip_at_generation: chain_tip,
            max_age_checkpoints: max_age,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MerkleSumLeaf {
    pub index: usize,
    pub address: String,
    pub bls_public_key: String,
    pub weight_units: u64,
    #[serde(default)]
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MerkleSumRoot {
    pub hash: String,
    pub total_weight_units: u64,
    #[serde(default)]
    pub total_weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MerkleSumProofSibling {
    pub hash: String,
    pub weight_units: u64,
    #[serde(default)]
    pub weight: f64,
    pub is_left: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MerkleSumProof {
    pub leaf: MerkleSumLeaf,
    pub siblings: Vec<MerkleSumProofSibling>,
    #[serde(default)]
    pub path_bits: Vec<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateWitnessEntry {
    pub key: String,
    pub value: Option<Value>,
    pub proof_key: String,
    pub proof_siblings: Vec<String>,
}

impl VerifiableObject {
    pub fn checkpoint_height(&self) -> u64 {
        match self {
            VerifiableObject::ContractOutput { receipt, .. } => receipt.finality.checkpoint_height,
            VerifiableObject::AccountProof { checkpoint_height, .. } => *checkpoint_height,
            VerifiableObject::TxFinality { checkpoint_height, .. } => *checkpoint_height,
            VerifiableObject::WeightProof { checkpoint_height, .. } => *checkpoint_height,
            VerifiableObject::Custom { checkpoint_height, .. } => *checkpoint_height,
            VerifiableObject::BatchProof { finality, .. } => finality.checkpoint_height,
            VerifiableObject::StateWitness { checkpoint_height, .. } => *checkpoint_height,
            VerifiableObject::FastPathProof { micro_checkpoint_seq, .. } => *micro_checkpoint_seq,
        }
    }

    pub fn chain_id(&self) -> Option<&str> {
        match self {
            VerifiableObject::ContractOutput { receipt, .. } => Some(&receipt.chain_id),
            VerifiableObject::AccountProof { chain_id, .. } => chain_id.as_deref(),
            VerifiableObject::TxFinality { chain_id, .. } => chain_id.as_deref(),
            VerifiableObject::WeightProof { chain_id, .. } => chain_id.as_deref(),
            VerifiableObject::Custom { chain_id, .. } => chain_id.as_deref(),
            VerifiableObject::BatchProof { chain_id, .. } => chain_id.as_deref(),
            VerifiableObject::StateWitness { chain_id, .. } => chain_id.as_deref(),
            VerifiableObject::FastPathProof { chain_id, .. } => chain_id.as_deref(),
        }
    }

    pub fn object_type(&self) -> &'static str {
        match self {
            VerifiableObject::ContractOutput { .. } => "contract_output",
            VerifiableObject::AccountProof { .. } => "account_proof",
            VerifiableObject::TxFinality { .. } => "tx_finality",
            VerifiableObject::WeightProof { .. } => "weight_proof",
            VerifiableObject::Custom { .. } => "custom",
            VerifiableObject::BatchProof { .. } => "batch_proof",
            VerifiableObject::StateWitness { .. } => "state_witness",
            VerifiableObject::FastPathProof { .. } => "fast_path_proof",
        }
    }

    pub fn freshness(&self) -> Option<&ProofFreshness> {
        match self {
            VerifiableObject::ContractOutput { freshness, .. } => freshness.as_ref(),
            VerifiableObject::AccountProof { freshness, .. } => freshness.as_ref(),
            VerifiableObject::TxFinality { freshness, .. } => freshness.as_ref(),
            VerifiableObject::WeightProof { freshness, .. } => freshness.as_ref(),
            VerifiableObject::Custom { freshness, .. } => freshness.as_ref(),
            VerifiableObject::BatchProof { freshness, .. } => freshness.as_ref(),
            VerifiableObject::StateWitness { freshness, .. } => freshness.as_ref(),
            VerifiableObject::FastPathProof { freshness, .. } => freshness.as_ref(),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_checkpoints: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_checkpoint_height: Option<u64>,
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
            if let VerifiableObject::ContractOutput { ref receipt, .. } = self.proof {
                if receipt.contract_id != *expected_contract {
                    return Err(ProofValidationError::ContractMismatch {
                        expected: expected_contract.clone(),
                        got: receipt.contract_id.clone(),
                    });
                }
            }
        }

        if let Some(ref required_keys) = exp.required_view_keys {
            if let VerifiableObject::ContractOutput { ref receipt, .. } = self.proof {
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

        if let (Some(max_age), Some(current_height)) = (exp.max_age_checkpoints, exp.current_checkpoint_height) {
            if let Some(freshness) = self.proof.freshness() {
                let actual_age = freshness.age(current_height);
                if actual_age > max_age {
                    return Err(ProofValidationError::ProofTooOld {
                        max_age,
                        actual_age,
                        proof_checkpoint: freshness.generated_at_checkpoint,
                        current_checkpoint: current_height,
                    });
                }
            } else {
                let proof_height = self.proof.checkpoint_height();
                let age = current_height.saturating_sub(proof_height);
                if age > max_age {
                    return Err(ProofValidationError::ProofTooOld {
                        max_age,
                        actual_age: age,
                        proof_checkpoint: proof_height,
                        current_checkpoint: current_height,
                    });
                }
            }
        }

        if let Some(ref expected_root) = exp.expected_state_root {
            match &self.proof {
                VerifiableObject::ContractOutput { receipt, .. } => {
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
                VerifiableObject::BatchProof { finality, .. } => {
                    if finality.state_root != *expected_root {
                        return Err(ProofValidationError::StateRootMismatch {
                            expected: expected_root.clone(),
                            got: finality.state_root.clone(),
                        });
                    }
                }
                VerifiableObject::StateWitness { state_root, .. } => {
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
    ProofTooOld { max_age: u64, actual_age: u64, proof_checkpoint: u64, current_checkpoint: u64 },
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
            ProofValidationError::ProofTooOld { max_age, actual_age, proof_checkpoint, current_checkpoint } =>
                write!(f, "proof too old: max age {} checkpoints, actual age {} (proof at {}, chain at {})", max_age, actual_age, proof_checkpoint, current_checkpoint),
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
            VerifiableObject::ContractOutput { receipt, .. } => {
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
            VerifiableObject::BatchProof { tx_hashes, finality, .. } => {
                extracted.insert("tx_hashes".to_string(), Value::Array(
                    tx_hashes.iter().map(|h| Value::String(h.clone())).collect()
                ));
                extracted.insert("tx_count".to_string(), Value::Number(serde_json::Number::from(tx_hashes.len())));
                extracted.insert("checkpoint_hash".to_string(), Value::String(finality.checkpoint_hash.clone()));
            }
            VerifiableObject::StateWitness { contract_id, entries, state_root, .. } => {
                if let Some(cid) = contract_id {
                    extracted.insert("_contract_id".to_string(), Value::String(cid.clone()));
                }
                extracted.insert("state_root".to_string(), Value::String(state_root.clone()));
                extracted.insert("entry_count".to_string(), Value::Number(serde_json::Number::from(entries.len())));
                for entry in entries {
                    let prefix = contract_id.as_deref().unwrap_or("account");
                    extracted.insert(
                        format!("{}.{}", prefix, entry.key),
                        entry.value.clone().unwrap_or(Value::Null),
                    );
                }
            }
            VerifiableObject::FastPathProof { tx_hash, write_set_hash, micro_checkpoint_seq, state_root, finality_ms, .. } => {
                extracted.insert("tx_hash".to_string(), Value::String(tx_hash.clone()));
                extracted.insert("write_set_hash".to_string(), Value::String(write_set_hash.clone()));
                extracted.insert("micro_checkpoint_seq".to_string(), Value::Number(serde_json::Number::from(*micro_checkpoint_seq)));
                extracted.insert("state_root".to_string(), Value::String(state_root.clone()));
                extracted.insert("finality_ms".to_string(), Value::Number(serde_json::Number::from(*finality_ms)));
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
                balance: 0.000001,
                nonce: 1,
                staked_micro: 0,
                staked: 0.0,
                checkpoint_height: 10,
                checkpoint_hash: "hash".to_string(),
                checkpoint_timestamp: 0,
                state_root: "root".to_string(),
                merkle_proof: vec![],
                merkle_index: 0,
                bls_aggregated_sig: None,
                bls_signer_bitmap: None,
                is_on_demand: false,
                chain_id: None,
                freshness: None,
            },
            expectation: ProofExpectation {
                expected_contract_id: None,
                expected_chain_id: None,
                min_checkpoint_height: None,
                expected_state_root: None,
                expected_object_type: Some("contract_output".to_string()),
                required_view_keys: None,
                max_age_checkpoints: None,
                current_checkpoint_height: None,
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
                balance: 0.000001,
                nonce: 1,
                staked_micro: 0,
                staked: 0.0,
                checkpoint_height: 5,
                checkpoint_hash: "hash".to_string(),
                checkpoint_timestamp: 0,
                state_root: "root".to_string(),
                merkle_proof: vec![],
                merkle_index: 0,
                bls_aggregated_sig: None,
                bls_signer_bitmap: None,
                is_on_demand: false,
                chain_id: None,
                freshness: None,
            },
            expectation: ProofExpectation {
                expected_contract_id: None,
                expected_chain_id: None,
                min_checkpoint_height: Some(10),
                expected_state_root: None,
                expected_object_type: None,
                required_view_keys: None,
                max_age_checkpoints: None,
                current_checkpoint_height: None,
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
                balance: 10.0,
                nonce: 5,
                staked_micro: 0,
                staked: 0.0,
                checkpoint_height: 100,
                checkpoint_hash: "hash".to_string(),
                checkpoint_timestamp: 0,
                state_root: "root".to_string(),
                merkle_proof: vec![],
                merkle_index: 0,
                bls_aggregated_sig: None,
                bls_signer_bitmap: None,
                is_on_demand: false,
                chain_id: None,
                freshness: None,
            },
            expectation: ProofExpectation {
                expected_contract_id: None,
                expected_chain_id: None,
                min_checkpoint_height: Some(50),
                expected_state_root: None,
                expected_object_type: Some("account_proof".to_string()),
                required_view_keys: None,
                max_age_checkpoints: None,
                current_checkpoint_height: None,
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
                balance: 50.0,
                nonce: 42,
                staked_micro: 1_000_000_000,
                staked: 10.0,
                checkpoint_height: 200,
                checkpoint_hash: "cp_hash".to_string(),
                checkpoint_timestamp: 0,
                state_root: "sr".to_string(),
                merkle_proof: vec![],
                merkle_index: 0,
                bls_aggregated_sig: None,
                bls_signer_bitmap: None,
                is_on_demand: false,
                chain_id: None,
                freshness: None,
            },
            expectation: ProofExpectation {
                expected_contract_id: None,
                expected_chain_id: None,
                min_checkpoint_height: None,
                expected_state_root: None,
                expected_object_type: None,
                required_view_keys: None,
                max_age_checkpoints: None,
                current_checkpoint_height: None,
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
            balance: 0.0,
            nonce: 0,
            staked_micro: 0,
            staked: 0.0,
            checkpoint_height: 0,
            checkpoint_hash: String::new(),
            checkpoint_timestamp: 0,
            state_root: String::new(),
            merkle_proof: vec![],
            merkle_index: 0,
            bls_aggregated_sig: None,
            bls_signer_bitmap: None,
            is_on_demand: false,
            chain_id: None,
            freshness: None,
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

    #[test]
    fn test_batch_proof_object_type_and_checkpoint() {
        let vo = VerifiableObject::BatchProof {
            finality: CheckpointFinality {
                checkpoint_height: 42,
                checkpoint_hash: "cp_hash".to_string(),
                checkpoint_timestamp: 1000,
                state_root: "sr".to_string(),
                receipt_root: "rr".to_string(),
                bls_aggregated_sig: None,
                bls_signer_bitmap: None,
            },
            tx_hashes: vec!["tx1".to_string(), "tx2".to_string()],
            multiproof: crate::merkle::MerkleMultiProof {
                leaf_hashes: vec![],
                leaf_indices: vec![],
                helper_hashes: vec![],
                helper_indices: vec![],
                num_leaves: 0,
                root: String::new(),
            },
            receipts: None,
            chain_id: Some("rinku-testnet".to_string()),
            freshness: None,
        };
        assert_eq!(vo.object_type(), "batch_proof");
        assert_eq!(vo.checkpoint_height(), 42);
        assert_eq!(vo.chain_id(), Some("rinku-testnet"));
    }

    #[test]
    fn test_state_witness_object_type_and_checkpoint() {
        let vo = VerifiableObject::StateWitness {
            contract_id: Some("sc_chat".to_string()),
            entries: vec![
                StateWitnessEntry {
                    key: "count".to_string(),
                    value: Some(Value::Number(serde_json::Number::from(5))),
                    proof_key: "abcd".to_string(),
                    proof_siblings: vec!["s1".to_string()],
                },
            ],
            state_root: "root123".to_string(),
            checkpoint_height: 99,
            checkpoint_hash: "cp99".to_string(),
            bls_aggregated_sig: None,
            bls_signer_bitmap: None,
            chain_id: None,
            freshness: None,
        };
        assert_eq!(vo.object_type(), "state_witness");
        assert_eq!(vo.checkpoint_height(), 99);
        assert_eq!(vo.chain_id(), None);
    }

    #[test]
    fn test_batch_proof_serialization_roundtrip() {
        let vo = VerifiableObject::BatchProof {
            finality: CheckpointFinality {
                checkpoint_height: 10,
                checkpoint_hash: "hash10".to_string(),
                checkpoint_timestamp: 500,
                state_root: "sr10".to_string(),
                receipt_root: "rr10".to_string(),
                bls_aggregated_sig: Some("sig".to_string()),
                bls_signer_bitmap: Some("bitmap".to_string()),
            },
            tx_hashes: vec!["txA".to_string(), "txB".to_string(), "txC".to_string()],
            multiproof: crate::merkle::MerkleMultiProof {
                leaf_hashes: vec!["lh1".to_string()],
                leaf_indices: vec![0],
                helper_hashes: vec!["hh1".to_string()],
                helper_indices: vec![(0, 1)],
                num_leaves: 2,
                root: "mroot".to_string(),
            },
            receipts: None,
            chain_id: Some("test-chain".to_string()),
            freshness: None,
        };

        let json = serde_json::to_string(&vo).unwrap();
        let deserialized: VerifiableObject = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.object_type(), "batch_proof");
        assert_eq!(deserialized.checkpoint_height(), 10);
        assert_eq!(deserialized.chain_id(), Some("test-chain"));
    }

    #[test]
    fn test_state_witness_serialization_roundtrip() {
        let vo = VerifiableObject::StateWitness {
            contract_id: Some("sc_test".to_string()),
            entries: vec![
                StateWitnessEntry {
                    key: "balance".to_string(),
                    value: Some(Value::Number(serde_json::Number::from(1000))),
                    proof_key: "pk1".to_string(),
                    proof_siblings: vec!["sib1".to_string(), "sib2".to_string()],
                },
                StateWitnessEntry {
                    key: "owner".to_string(),
                    value: Some(Value::String("alice".to_string())),
                    proof_key: "pk2".to_string(),
                    proof_siblings: vec![],
                },
            ],
            state_root: "witness_root".to_string(),
            checkpoint_height: 77,
            checkpoint_hash: "cp77".to_string(),
            bls_aggregated_sig: None,
            bls_signer_bitmap: None,
            chain_id: Some("mainnet".to_string()),
            freshness: None,
        };

        let json = serde_json::to_string(&vo).unwrap();
        let deserialized: VerifiableObject = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.object_type(), "state_witness");
        assert_eq!(deserialized.checkpoint_height(), 77);
        assert_eq!(deserialized.chain_id(), Some("mainnet"));
    }

    #[test]
    fn test_batch_proof_validation_state_root() {
        let input = ProofInput {
            label: "batch".to_string(),
            proof: VerifiableObject::BatchProof {
                finality: CheckpointFinality {
                    checkpoint_height: 5,
                    checkpoint_hash: "h".to_string(),
                    checkpoint_timestamp: 0,
                    state_root: "actual_root".to_string(),
                    receipt_root: "rr".to_string(),
                    bls_aggregated_sig: None,
                    bls_signer_bitmap: None,
                },
                tx_hashes: vec![],
                multiproof: crate::merkle::MerkleMultiProof {
                    leaf_hashes: vec![],
                    leaf_indices: vec![],
                    helper_hashes: vec![],
                    helper_indices: vec![],
                    num_leaves: 0,
                    root: String::new(),
                },
                receipts: None,
                chain_id: None,
                freshness: None,
            },
            expectation: ProofExpectation {
                expected_contract_id: None,
                expected_chain_id: None,
                min_checkpoint_height: None,
                expected_state_root: Some("expected_root".to_string()),
                expected_object_type: None,
                required_view_keys: None,
                max_age_checkpoints: None,
                current_checkpoint_height: None,
            },
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn test_state_witness_validation_state_root() {
        let input = ProofInput {
            label: "witness".to_string(),
            proof: VerifiableObject::StateWitness {
                contract_id: None,
                entries: vec![],
                state_root: "wrong_root".to_string(),
                checkpoint_height: 10,
                checkpoint_hash: "ch".to_string(),
                bls_aggregated_sig: None,
                bls_signer_bitmap: None,
                chain_id: None,
                freshness: None,
            },
            expectation: ProofExpectation {
                expected_contract_id: None,
                expected_chain_id: None,
                min_checkpoint_height: None,
                expected_state_root: Some("correct_root".to_string()),
                expected_object_type: None,
                required_view_keys: None,
                max_age_checkpoints: None,
                current_checkpoint_height: None,
            },
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn test_batch_proof_context_extraction() {
        let input = ProofInput {
            label: "batch_ctx".to_string(),
            proof: VerifiableObject::BatchProof {
                finality: CheckpointFinality {
                    checkpoint_height: 20,
                    checkpoint_hash: "cp20".to_string(),
                    checkpoint_timestamp: 100,
                    state_root: "sr".to_string(),
                    receipt_root: "rr".to_string(),
                    bls_aggregated_sig: None,
                    bls_signer_bitmap: None,
                },
                tx_hashes: vec!["t1".to_string(), "t2".to_string()],
                multiproof: crate::merkle::MerkleMultiProof {
                    leaf_hashes: vec![],
                    leaf_indices: vec![],
                    helper_hashes: vec![],
                    helper_indices: vec![],
                    num_leaves: 0,
                    root: String::new(),
                },
                receipts: None,
                chain_id: None,
                freshness: None,
            },
            expectation: ProofExpectation {
                expected_contract_id: None,
                expected_chain_id: None,
                min_checkpoint_height: None,
                expected_state_root: None,
                expected_object_type: None,
                required_view_keys: None,
                max_age_checkpoints: None,
                current_checkpoint_height: None,
            },
        };

        let ctx = ValidatedProofContext::from_proof_input(&input);
        assert_eq!(ctx.object_type, "batch_proof");
        assert_eq!(ctx.checkpoint_height, 20);
        assert_eq!(ctx.extracted_values.get("tx_count").and_then(|v| v.as_u64()), Some(2));
    }

    #[test]
    fn test_state_witness_context_extraction() {
        let input = ProofInput {
            label: "sw_ctx".to_string(),
            proof: VerifiableObject::StateWitness {
                contract_id: Some("sc_x".to_string()),
                entries: vec![
                    StateWitnessEntry {
                        key: "val".to_string(),
                        value: Some(Value::Number(serde_json::Number::from(42))),
                        proof_key: "pk".to_string(),
                        proof_siblings: vec![],
                    },
                ],
                state_root: "root".to_string(),
                checkpoint_height: 50,
                checkpoint_hash: "cp50".to_string(),
                bls_aggregated_sig: None,
                bls_signer_bitmap: None,
                chain_id: None,
                freshness: None,
            },
            expectation: ProofExpectation {
                expected_contract_id: None,
                expected_chain_id: None,
                min_checkpoint_height: None,
                expected_state_root: None,
                expected_object_type: None,
                required_view_keys: None,
                max_age_checkpoints: None,
                current_checkpoint_height: None,
            },
        };

        let ctx = ValidatedProofContext::from_proof_input(&input);
        assert_eq!(ctx.object_type, "state_witness");
        assert_eq!(ctx.checkpoint_height, 50);
        assert_eq!(ctx.extracted_values.get("sc_x.val").and_then(|v| v.as_u64()), Some(42));
        assert_eq!(ctx.extracted_values.get("entry_count").and_then(|v| v.as_u64()), Some(1));
    }

    #[test]
    fn test_proof_freshness_is_fresh() {
        let freshness = ProofFreshness {
            generated_at_checkpoint: 100,
            generated_at_timestamp: 1000000,
            chain_tip_at_generation: 100,
            max_age_checkpoints: Some(10),
        };
        assert!(freshness.is_fresh(105));
        assert!(freshness.is_fresh(110));
        assert!(!freshness.is_fresh(111));
    }

    #[test]
    fn test_proof_freshness_no_max_age() {
        let freshness = ProofFreshness {
            generated_at_checkpoint: 50,
            generated_at_timestamp: 500000,
            chain_tip_at_generation: 50,
            max_age_checkpoints: None,
        };
        assert!(freshness.is_fresh(1000));
    }

    #[test]
    fn test_proof_too_old_validation() {
        let input = ProofInput {
            label: "stale_test".to_string(),
            proof: VerifiableObject::AccountProof {
                address: "abc".to_string(),
                balance_micro: 100,
                balance: 0.000001,
                nonce: 1,
                staked_micro: 0,
                staked: 0.0,
                checkpoint_height: 50,
                checkpoint_hash: "hash".to_string(),
                checkpoint_timestamp: 0,
                state_root: "root".to_string(),
                merkle_proof: vec![],
                merkle_index: 0,
                bls_aggregated_sig: None,
                bls_signer_bitmap: None,
                is_on_demand: false,
                chain_id: None,
                freshness: Some(ProofFreshness {
                    generated_at_checkpoint: 50,
                    generated_at_timestamp: 500000,
                    chain_tip_at_generation: 50,
                    max_age_checkpoints: None,
                }),
            },
            expectation: ProofExpectation {
                expected_contract_id: None,
                expected_chain_id: None,
                min_checkpoint_height: None,
                expected_state_root: None,
                expected_object_type: None,
                required_view_keys: None,
                max_age_checkpoints: Some(5),
                current_checkpoint_height: Some(100),
            },
        };
        assert!(input.validate().is_err());
    }
}
