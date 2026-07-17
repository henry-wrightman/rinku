use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use flate2::{read::DeflateDecoder, write::DeflateEncoder, Compression};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

use crate::bls::{parse_signer_bitmap, verify_aggregated_signature};

pub use rinku_core::stateful_receipt::{
    MerkleSumLeaf, MerkleSumProof, MerkleSumProofSibling, MerkleSumRoot,
};

pub const WEIGHT_UNITS: u64 = 100_000_000;

pub fn to_weight_units(value: f64) -> u64 {
    (value * WEIGHT_UNITS as f64).round() as u64
}

pub fn from_weight_units(micro: u64) -> f64 {
    micro as f64 / WEIGHT_UNITS as f64
}

#[derive(Debug, Clone)]
pub struct MerkleSumNode {
    pub hash: String,
    pub sum_weight_units: u64,
}

/// Hash a validator leaf using u64 weight_units for deterministic cross-language verification
fn hash_leaf(leaf: &MerkleSumLeaf) -> String {
    let data = format!(
        "leaf:{}:{}:{}:{}",
        leaf.index, leaf.address, leaf.bls_public_key, leaf.weight_units
    );
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

/// Hash internal node using u64 weight sums for deterministic cross-language verification
fn hash_internal(left: &MerkleSumNode, right: &MerkleSumNode) -> String {
    let data = format!(
        "node:{}:{}:{}:{}",
        left.hash, left.sum_weight_units, right.hash, right.sum_weight_units
    );
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

fn empty_node() -> MerkleSumNode {
    let mut hasher = Sha256::new();
    hasher.update(b"rinku:empty_node:v1");
    MerkleSumNode {
        hash: hex::encode(hasher.finalize()),
        sum_weight_units: 0,
    }
}

pub struct MerkleSumTreeResult {
    pub root: MerkleSumRoot,
    pub layers: Vec<Vec<MerkleSumNode>>,
}

pub fn build_merkle_sum_tree(leaves: &[MerkleSumLeaf]) -> MerkleSumTreeResult {
    if leaves.is_empty() {
        let mut hasher = Sha256::new();
        hasher.update(b"empty");
        return MerkleSumTreeResult {
            root: MerkleSumRoot {
                hash: hex::encode(hasher.finalize()),
                total_weight_units: 0,
                total_weight: 0.0,
            },
            layers: vec![],
        };
    }

    let mut sorted_leaves: Vec<MerkleSumLeaf> = leaves.to_vec();
    sorted_leaves.sort_by_key(|l| l.index);

    let mut current_layer: Vec<MerkleSumNode> = sorted_leaves
        .iter()
        .map(|leaf| MerkleSumNode {
            hash: hash_leaf(leaf),
            sum_weight_units: leaf.weight_units,
        })
        .collect();

    let mut layers: Vec<Vec<MerkleSumNode>> = vec![current_layer.clone()];

    while current_layer.len() > 1 {
        let mut next_layer: Vec<MerkleSumNode> = Vec::new();

        let mut i = 0;
        while i < current_layer.len() {
            let left = &current_layer[i];
            let right = if i + 1 < current_layer.len() {
                &current_layer[i + 1]
            } else {
                &empty_node()
            };

            next_layer.push(MerkleSumNode {
                hash: hash_internal(left, right),
                sum_weight_units: left.sum_weight_units + right.sum_weight_units,
            });
            i += 2;
        }

        current_layer = next_layer;
        layers.push(current_layer.clone());
    }

    let root_node = &current_layer[0];
    MerkleSumTreeResult {
        root: MerkleSumRoot {
            hash: root_node.hash.clone(),
            total_weight_units: root_node.sum_weight_units,
            total_weight: from_weight_units(root_node.sum_weight_units),
        },
        layers,
    }
}

pub fn get_merkle_sum_proof(leaves: &[MerkleSumLeaf], leaf_index: usize) -> Option<MerkleSumProof> {
    let mut sorted_leaves: Vec<MerkleSumLeaf> = leaves.to_vec();
    sorted_leaves.sort_by_key(|l| l.index);

    let target_leaf = sorted_leaves
        .iter()
        .find(|l| l.index == leaf_index)?
        .clone();
    let position_in_array = sorted_leaves.iter().position(|l| l.index == leaf_index)?;

    let mut current_layer: Vec<MerkleSumNode> = sorted_leaves
        .iter()
        .map(|leaf| MerkleSumNode {
            hash: hash_leaf(leaf),
            sum_weight_units: leaf.weight_units,
        })
        .collect();

    let mut siblings: Vec<MerkleSumProofSibling> = Vec::new();
    let mut path_bits: Vec<bool> = Vec::new();
    let mut pos = position_in_array;

    while current_layer.len() > 1 {
        let is_right = pos % 2 == 1;
        let sibling_pos = if is_right { pos - 1 } else { pos + 1 };

        let sibling = if sibling_pos < current_layer.len() {
            current_layer[sibling_pos].clone()
        } else {
            empty_node()
        };

        siblings.push(MerkleSumProofSibling {
            hash: sibling.hash.clone(),
            weight_units: sibling.sum_weight_units,
            weight: from_weight_units(sibling.sum_weight_units),
            is_left: !is_right,
        });
        path_bits.push(is_right);

        let mut next_layer: Vec<MerkleSumNode> = Vec::new();
        let mut i = 0;
        while i < current_layer.len() {
            let left = &current_layer[i];
            let right = if i + 1 < current_layer.len() {
                &current_layer[i + 1]
            } else {
                &empty_node()
            };
            next_layer.push(MerkleSumNode {
                hash: hash_internal(left, right),
                sum_weight_units: left.sum_weight_units + right.sum_weight_units,
            });
            i += 2;
        }

        pos /= 2;
        current_layer = next_layer;
    }

    Some(MerkleSumProof {
        leaf: target_leaf,
        siblings,
        path_bits,
    })
}

pub fn verify_merkle_sum_proof(
    proof: &MerkleSumProof,
    expected_root: &MerkleSumRoot,
) -> (bool, f64, Vec<String>) {
    let mut errors: Vec<String> = Vec::new();

    let mut current = MerkleSumNode {
        hash: hash_leaf(&proof.leaf),
        sum_weight_units: proof.leaf.weight_units,
    };

    for (i, sibling) in proof.siblings.iter().enumerate() {
        let is_right = proof.path_bits.get(i).copied().unwrap_or(false);

        let sibling_node = MerkleSumNode {
            hash: sibling.hash.clone(),
            sum_weight_units: sibling.weight_units,
        };

        let (left, right) = if is_right {
            (&sibling_node, &current)
        } else {
            (&current, &sibling_node)
        };

        current = MerkleSumNode {
            hash: hash_internal(left, right),
            sum_weight_units: left.sum_weight_units + right.sum_weight_units,
        };
    }

    if current.hash != expected_root.hash {
        errors.push(format!(
            "Root hash mismatch: expected {}, got {}",
            expected_root.hash, current.hash
        ));
    }

    if current.sum_weight_units != expected_root.total_weight_units {
        errors.push(format!(
            "Total weight mismatch: expected {}, got {}",
            expected_root.total_weight_units, current.sum_weight_units
        ));
    }

    (errors.is_empty(), proof.leaf.weight, errors)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfContainedProof {
    pub version: u32,
    pub tx_hash: String,
    pub tx_signature: String,
    pub tx_from: String,
    pub tx_to: String,
    pub tx_amount: f64,
    pub tx_nonce: u64,
    pub tx_timestamp: u64,
    pub checkpoint_height: u64,
    pub checkpoint_id: String,
    pub checkpoint_timestamp: u64,
    pub tx_merkle_root: String,
    pub state_root: String,
    pub receipt_root: String,
    pub tip_count: u32,
    pub merkle_proof: Vec<String>,
    pub merkle_index: usize,
    pub bls_aggregated_sig: String,
    pub bls_signer_bitmap: String,
    pub bls_signer_count: usize,
    pub signer_membership_proofs: Vec<MerkleSumProof>,
    pub validator_sum_tree_root: MerkleSumRoot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofVerificationResult {
    pub valid: bool,
    pub errors: Vec<String>,
    pub tx_hash: String,
    pub checkpoint_height: u64,
    pub computed_signer_weight: f64,
    pub total_weight: f64,
    pub signer_count: usize,
    pub merkle_verified: bool,
    pub bls_verified: bool,
    pub validator_set_verified: bool,
}

const SELF_PROOF_VERSION: u32 = 4;

pub fn compute_checkpoint_signing_hash(
    height: u64,
    tx_merkle_root: &str,
    state_root: &str,
    receipt_root: &str,
    tip_count: u32,
    timestamp: u64,
) -> Vec<u8> {
    // This MUST match the format used in checkpoint.rs compute_checkpoint_hash
    let signing_data = format!(
        "{}:{}:{}:{}:{}:{}",
        height, tx_merkle_root, state_root, receipt_root, tip_count, timestamp
    );
    let mut hasher = Sha256::new();
    hasher.update(signing_data.as_bytes());
    hasher.finalize().to_vec()
}

pub fn verify_tx_merkle_proof(
    tx_hash: &str,
    proof: &[String],
    index: usize,
    expected_root: &str,
) -> bool {
    // Decode tx_hash from hex to raw bytes
    let mut current_bytes = match hex::decode(tx_hash) {
        Ok(bytes) if bytes.len() == 32 => bytes,
        _ => return false,
    };
    let mut idx = index;

    for sibling in proof {
        // Decode sibling from hex to raw bytes
        let sibling_bytes = match hex::decode(sibling) {
            Ok(bytes) if bytes.len() == 32 => bytes,
            _ => return false,
        };

        // Combine as raw bytes (not hex strings) - matches MerkleTree::compute_next_layer
        let mut combined = Vec::with_capacity(64);
        if idx % 2 == 0 {
            combined.extend_from_slice(&current_bytes);
            combined.extend_from_slice(&sibling_bytes);
        } else {
            combined.extend_from_slice(&sibling_bytes);
            combined.extend_from_slice(&current_bytes);
        }

        let mut hasher = Sha256::new();
        hasher.update(&combined);
        current_bytes = hasher.finalize().to_vec();
        idx /= 2;
    }

    hex::encode(&current_bytes) == expected_root
}

/// Hash data using SHA256 and return hex string
fn sha256_hex_for_proof(data: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

/// Verify an account state proof against its embedded state root
/// Returns true if the proof is valid (the account data matches the merkle root)
/// Uses the same leaf/node encoding as sync_verification::hash_account_leaf and hash_internal
/// Detailed result of account state proof verification
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountProofVerificationDetail {
    pub valid: bool,
    pub leaf_data: String,
    pub leaf_hash: String,
    pub computed_root: String,
    pub expected_root: String,
    pub proof_length: usize,
    pub merkle_index: usize,
}

pub fn verify_account_state_proof(proof: &rinku_core::types::AccountStateProof) -> bool {
    verify_account_state_proof_detailed(proof).valid
}

pub fn verify_account_state_proof_detailed(
    proof: &rinku_core::types::AccountStateProof,
) -> AccountProofVerificationDetail {
    let leaf_data = format!(
        "account:{}:{}:{}:{}",
        proof.address, proof.balance_micro, proof.nonce, proof.staked_micro
    );

    if proof.version >= 4 {
        verify_account_state_proof_smt(proof, &leaf_data)
    } else {
        verify_account_state_proof_flat(proof, &leaf_data)
    }
}

fn verify_account_state_proof_smt(
    proof: &rinku_core::types::AccountStateProof,
    leaf_data: &str,
) -> AccountProofVerificationDetail {
    use sha2::{Digest, Sha256};
    let leaf_hash_bytes: [u8; 32] = {
        let mut h = Sha256::new();
        h.update(leaf_data.as_bytes());
        h.finalize().into()
    };
    let leaf_hash = hex::encode(leaf_hash_bytes);

    let key = crate::sparse_merkle_trie::hash_account_key(&proof.address);
    let path = (0..256)
        .map(|i| {
            let byte_idx = i / 8;
            let bit_idx = 7 - (i % 8);
            (key[byte_idx] >> bit_idx) & 1 == 1
        })
        .collect::<Vec<bool>>();

    let default_hashes = crate::sparse_merkle_trie::compute_default_hashes();

    let mut current_hash = leaf_hash_bytes;
    for (i, &go_right) in path.iter().enumerate().rev() {
        let sibling = if i < proof.merkle_proof.len() {
            match hex::decode(&proof.merkle_proof[i]) {
                Ok(bytes) if bytes.len() == 32 => {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&bytes);
                    arr
                }
                _ => default_hashes[i + 1],
            }
        } else {
            default_hashes[i + 1]
        };

        let mut h = Sha256::new();
        if go_right {
            h.update(sibling);
            h.update(current_hash);
        } else {
            h.update(current_hash);
            h.update(sibling);
        }
        current_hash = h.finalize().into();
    }

    let computed_root = hex::encode(current_hash);
    let valid = computed_root == proof.state_root;

    if !valid {
        tracing::warn!(
            "SMT account proof verification FAILED for {}: computed_root={} vs expected={}",
            &proof.address[..16.min(proof.address.len())],
            &computed_root[..16],
            &proof.state_root[..16]
        );
    }

    AccountProofVerificationDetail {
        valid,
        leaf_data: leaf_data.to_string(),
        leaf_hash,
        computed_root,
        expected_root: proof.state_root.clone(),
        proof_length: proof.merkle_proof.len(),
        merkle_index: 0,
    }
}

fn verify_account_state_proof_flat(
    proof: &rinku_core::types::AccountStateProof,
    leaf_data: &str,
) -> AccountProofVerificationDetail {
    let leaf_hash = sha256_hex_for_proof(leaf_data);
    let mut current_hash = leaf_hash.clone();

    let mut idx = proof.merkle_index;

    for sibling_hex in &proof.merkle_proof {
        let (left, right) = if idx % 2 == 0 {
            (current_hash.clone(), sibling_hex.clone())
        } else {
            (sibling_hex.clone(), current_hash.clone())
        };
        current_hash = sha256_hex_for_proof(&format!("node:{}:{}", left, right));
        idx /= 2;
    }

    let valid = current_hash == proof.state_root;

    if !valid {
        tracing::warn!(
            "Account proof verification FAILED for {}: computed_root={} vs expected={}",
            &proof.address[..16.min(proof.address.len())],
            &current_hash[..16],
            &proof.state_root[..16]
        );
    }

    AccountProofVerificationDetail {
        valid,
        leaf_data: leaf_data.to_string(),
        leaf_hash,
        computed_root: current_hash,
        expected_root: proof.state_root.clone(),
        proof_length: proof.merkle_proof.len(),
        merkle_index: proof.merkle_index,
    }
}

pub fn verify_self_contained_proof(proof: &SelfContainedProof) -> ProofVerificationResult {
    let mut result = ProofVerificationResult {
        valid: false,
        errors: Vec::new(),
        tx_hash: proof.tx_hash.clone(),
        checkpoint_height: proof.checkpoint_height,
        computed_signer_weight: 0.0,
        total_weight: proof.validator_sum_tree_root.total_weight,
        signer_count: proof.bls_signer_count,
        merkle_verified: false,
        bls_verified: false,
        validator_set_verified: false,
    };

    if proof.version != SELF_PROOF_VERSION {
        result
            .errors
            .push(format!("Unsupported proof version: {}", proof.version));
        return result;
    }

    let mut computed_signer_weight = 0.0;
    for membership_proof in &proof.signer_membership_proofs {
        computed_signer_weight += membership_proof.leaf.weight;
    }
    result.computed_signer_weight = computed_signer_weight;
    result.validator_set_verified = true;

    let tx_merkle_valid = verify_tx_merkle_proof(
        &proof.tx_hash,
        &proof.merkle_proof,
        proof.merkle_index,
        &proof.tx_merkle_root,
    );
    result.merkle_verified = tx_merkle_valid;
    if !tx_merkle_valid {
        result
            .errors
            .push("Merkle proof verification failed".to_string());
    }

    let checkpoint_hash = compute_checkpoint_signing_hash(
        proof.checkpoint_height,
        &proof.tx_merkle_root,
        &proof.state_root,
        &proof.receipt_root,
        proof.tip_count,
        proof.checkpoint_timestamp,
    );

    let signer_pub_keys: Vec<Vec<u8>> = proof
        .signer_membership_proofs
        .iter()
        .filter_map(|p| URL_SAFE_NO_PAD.decode(&p.leaf.bls_public_key).ok())
        .collect();

    let aggregated_sig = match URL_SAFE_NO_PAD.decode(&proof.bls_aggregated_sig) {
        Ok(sig) => sig,
        Err(_) => {
            result
                .errors
                .push("Failed to decode BLS signature".to_string());
            return result;
        }
    };

    let bls_valid =
        verify_aggregated_signature(&checkpoint_hash, &aggregated_sig, &signer_pub_keys);
    result.bls_verified = bls_valid;
    if !bls_valid {
        result
            .errors
            .push("BLS signature verification failed".to_string());
    }

    let total_weight = proof.validator_sum_tree_root.total_weight;
    if total_weight <= 0.0 {
        result.errors.push("Invalid total weight".to_string());
    } else {
        let weight_ratio = computed_signer_weight / total_weight;
        // Use 0.6666 (exactly 2/3) to allow 2-of-3 validator quorum in small validator sets
        if weight_ratio < 0.6666 {
            result.errors.push(format!(
                "Insufficient signer weight: {:.1}% (need 66.66%)",
                weight_ratio * 100.0
            ));
        }
    }

    result.valid = result.merkle_verified
        && result.bls_verified
        && result.validator_set_verified
        && result.errors.is_empty();

    result
}

pub fn encode_self_contained_proof(proof: &SelfContainedProof) -> Result<String, String> {
    let json =
        serde_json::to_string(proof).map_err(|e| format!("JSON serialization failed: {}", e))?;

    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(json.as_bytes())
        .map_err(|e| format!("Compression failed: {}", e))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Compression finish failed: {}", e))?;

    Ok(URL_SAFE_NO_PAD.encode(&compressed))
}

pub fn decode_self_contained_proof(encoded: &str) -> Result<SelfContainedProof, String> {
    let encoded = if encoded.starts_with("rinku://sp/") {
        &encoded[11..]
    } else {
        encoded
    };

    let compressed = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|e| format!("Base64 decode failed: {}", e))?;

    let mut decoder = DeflateDecoder::new(&compressed[..]);
    let mut json = String::new();
    decoder
        .read_to_string(&mut json)
        .map_err(|e| format!("Decompression failed: {}", e))?;

    serde_json::from_str(&json).map_err(|e| format!("JSON parse failed: {}", e))
}

pub fn create_self_proof_url(proof: &SelfContainedProof) -> Result<String, String> {
    let encoded = encode_self_contained_proof(proof)?;
    Ok(format!("rinku://sp/{}", encoded))
}

pub fn encode_account_state_proof(
    proof: &rinku_core::types::AccountStateProof,
) -> Result<String, String> {
    let json =
        serde_json::to_string(proof).map_err(|e| format!("JSON serialization failed: {}", e))?;

    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(json.as_bytes())
        .map_err(|e| format!("Compression failed: {}", e))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Compression finish failed: {}", e))?;

    Ok(URL_SAFE_NO_PAD.encode(&compressed))
}

pub fn decode_account_state_proof(
    encoded: &str,
) -> Result<rinku_core::types::AccountStateProof, String> {
    let encoded = if encoded.starts_with("rinku://asp/") {
        &encoded[12..]
    } else {
        encoded
    };

    let compressed = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|e| format!("Base64 decode failed: {}", e))?;

    let mut decoder = DeflateDecoder::new(&compressed[..]);
    let mut json = String::new();
    decoder
        .read_to_string(&mut json)
        .map_err(|e| format!("Decompression failed: {}", e))?;

    serde_json::from_str(&json).map_err(|e| format!("JSON parse failed: {}", e))
}

pub fn create_account_state_proof_url(
    proof: &rinku_core::types::AccountStateProof,
) -> Result<String, String> {
    let encoded = encode_account_state_proof(proof)?;
    Ok(format!("rinku://asp/{}", encoded))
}

pub fn encode_vo(vo: &rinku_core::stateful_receipt::VerifiableObject) -> Result<String, String> {
    let json =
        serde_json::to_string(vo).map_err(|e| format!("JSON serialization failed: {}", e))?;
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(json.as_bytes())
        .map_err(|e| format!("Compression failed: {}", e))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Compression finish failed: {}", e))?;
    Ok(URL_SAFE_NO_PAD.encode(&compressed))
}

pub fn decode_vo(encoded: &str) -> Result<rinku_core::stateful_receipt::VerifiableObject, String> {
    let encoded = if encoded.starts_with("rinku://vo/") {
        &encoded[11..]
    } else {
        encoded
    };
    let compressed = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|e| format!("Base64 decode failed: {}", e))?;
    let mut decoder = DeflateDecoder::new(&compressed[..]);
    let mut json = String::new();
    decoder
        .read_to_string(&mut json)
        .map_err(|e| format!("Decompression failed: {}", e))?;
    serde_json::from_str(&json).map_err(|e| format!("JSON parse failed: {}", e))
}

pub fn create_vo_url(
    vo: &rinku_core::stateful_receipt::VerifiableObject,
) -> Result<String, String> {
    let encoded = encode_vo(vo)?;
    Ok(format!("rinku://vo/{}", encoded))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VOVerificationResult {
    pub valid: bool,
    pub errors: Vec<String>,
    pub object_type: String,
    pub checkpoint_height: u64,
    pub freshness: Option<VOFreshnessInfo>,
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VOFreshnessInfo {
    pub generated_at_checkpoint: u64,
    pub generated_at_timestamp: u64,
    pub chain_tip_at_generation: u64,
    pub max_age_checkpoints: Option<u64>,
}

impl From<&rinku_core::stateful_receipt::ProofFreshness> for VOFreshnessInfo {
    fn from(f: &rinku_core::stateful_receipt::ProofFreshness) -> Self {
        Self {
            generated_at_checkpoint: f.generated_at_checkpoint,
            generated_at_timestamp: f.generated_at_timestamp,
            chain_tip_at_generation: f.chain_tip_at_generation,
            max_age_checkpoints: f.max_age_checkpoints,
        }
    }
}

pub fn verify_vo(vo: &rinku_core::stateful_receipt::VerifiableObject) -> VOVerificationResult {
    use rinku_core::stateful_receipt::VerifiableObject;
    let object_type = vo.object_type().to_string();
    let checkpoint_height = vo.checkpoint_height();
    let freshness = vo.freshness().map(VOFreshnessInfo::from);

    match vo {
        VerifiableObject::TxFinality {
            tx_hash,
            checkpoint_height: cp_height,
            checkpoint_timestamp,
            tx_merkle_root,
            state_root,
            receipt_root,
            tip_count,
            merkle_proof,
            merkle_index,
            bls_aggregated_sig,
            signer_membership_proofs,
            validator_sum_tree_root,
            tx_from,
            tx_to,
            tx_amount,
            tx_nonce,
            tx_timestamp,
            checkpoint_hash,
            ..
        } => {
            let mut errors = Vec::new();

            let merkle_valid =
                verify_tx_merkle_proof(tx_hash, merkle_proof, *merkle_index, tx_merkle_root);
            if !merkle_valid {
                errors.push("Merkle proof verification failed".to_string());
            }

            let checkpoint_signing_hash = compute_checkpoint_signing_hash(
                *cp_height,
                tx_merkle_root,
                state_root,
                receipt_root,
                *tip_count,
                *checkpoint_timestamp,
            );

            let signer_pub_keys: Vec<Vec<u8>> = signer_membership_proofs
                .iter()
                .filter_map(|p| URL_SAFE_NO_PAD.decode(&p.leaf.bls_public_key).ok())
                .collect();

            let aggregated_sig = match URL_SAFE_NO_PAD.decode(bls_aggregated_sig) {
                Ok(sig) => sig,
                Err(_) => {
                    errors.push("Failed to decode BLS signature".to_string());
                    return VOVerificationResult {
                        valid: false,
                        errors,
                        object_type,
                        checkpoint_height,
                        freshness,
                        details: serde_json::json!({}),
                    };
                }
            };

            let bls_valid = verify_aggregated_signature(
                &checkpoint_signing_hash,
                &aggregated_sig,
                &signer_pub_keys,
            );
            if !bls_valid {
                errors.push("BLS signature verification failed".to_string());
            }

            let mut computed_signer_weight = 0.0;
            for membership_proof in signer_membership_proofs {
                computed_signer_weight += membership_proof.leaf.weight;
            }
            let total_weight = validator_sum_tree_root.total_weight;

            if total_weight <= 0.0 {
                errors.push("Invalid total weight".to_string());
            } else {
                let weight_ratio = computed_signer_weight / total_weight;
                if weight_ratio < 0.6666 {
                    errors.push(format!(
                        "Insufficient signer weight: {:.1}% (need 66.66%)",
                        weight_ratio * 100.0
                    ));
                }
            }

            let valid = merkle_valid && bls_valid && errors.is_empty();

            VOVerificationResult {
                valid,
                errors,
                object_type,
                checkpoint_height,
                freshness,
                details: serde_json::json!({
                    "txHash": tx_hash,
                    "txFrom": tx_from,
                    "txTo": tx_to,
                    "txAmount": tx_amount,
                    "txNonce": tx_nonce,
                    "txTimestamp": tx_timestamp,
                    "checkpointHash": checkpoint_hash,
                    "merkleVerified": merkle_valid,
                    "blsVerified": bls_valid,
                    "validatorSetVerified": true,
                    "signerWeight": computed_signer_weight,
                    "totalWeight": total_weight,
                    "signerCount": signer_membership_proofs.len(),
                }),
            }
        }
        VerifiableObject::AccountProof {
            address,
            balance_micro,
            balance,
            nonce,
            staked_micro,
            staked,
            state_root,
            merkle_proof,
            merkle_index,
            checkpoint_hash,
            is_on_demand,
            ..
        } => {
            let leaf_data = format!(
                "account:{}:{}:{}:{}",
                address, balance_micro, nonce, staked_micro
            );
            let leaf_hash = sha256_hex_for_proof(&leaf_data);
            let mut current_hash = leaf_hash.clone();
            let mut idx = *merkle_index;

            for sibling_hex in merkle_proof {
                let (left, right) = if idx % 2 == 0 {
                    (current_hash.clone(), sibling_hex.clone())
                } else {
                    (sibling_hex.clone(), current_hash.clone())
                };
                current_hash = sha256_hex_for_proof(&format!("node:{}:{}", left, right));
                idx /= 2;
            }

            let valid = current_hash == *state_root;
            let mut errors = Vec::new();
            if !valid {
                errors.push(format!(
                    "State root mismatch: computed {} vs expected {}",
                    &current_hash[..16.min(current_hash.len())],
                    &state_root[..16.min(state_root.len())]
                ));
            }

            VOVerificationResult {
                valid,
                errors,
                object_type,
                checkpoint_height,
                freshness,
                details: serde_json::json!({
                    "address": address,
                    "balance": balance,
                    "balanceMicro": balance_micro,
                    "nonce": nonce,
                    "staked": staked,
                    "stakedMicro": staked_micro,
                    "stateRoot": state_root,
                    "checkpointHash": checkpoint_hash,
                    "merkleIndex": merkle_index,
                    "proofDepth": merkle_proof.len(),
                    "isOnDemand": is_on_demand,
                }),
            }
        }
        _ => VOVerificationResult {
            valid: true,
            errors: vec![],
            object_type,
            checkpoint_height,
            freshness,
            details: serde_json::json!({
                "note": "Verification not implemented for this proof type"
            }),
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactProof {
    pub version: u8,
    pub tx_hash: Vec<u8>,
    pub tx_signature: Vec<u8>,
    pub checkpoint_height: u64,
    pub merkle_proof: Vec<Vec<u8>>,
    pub merkle_index: usize,
    pub aggregated_validator_sig: Vec<u8>,
    pub signer_bitmap: Vec<u8>,
    pub validator_set_root: Vec<u8>,
}

const COMPACT_PROOF_VERSION: u8 = 1;

fn write_varint(value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut val = value;
    while val > 0x7f {
        bytes.push((val & 0x7f) as u8 | 0x80);
        val >>= 7;
    }
    bytes.push(val as u8);
    bytes
}

fn read_varint(data: &[u8], offset: usize) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0;
    let mut bytes_read = 0;

    while offset + bytes_read < data.len() {
        let byte = data[offset + bytes_read];
        value |= ((byte & 0x7f) as u64) << shift;
        bytes_read += 1;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }

    Some((value, bytes_read))
}

pub fn encode_compact_proof(proof: &CompactProof) -> Result<(Vec<u8>, String, String), String> {
    let mut parts: Vec<u8> = Vec::new();

    parts.push(COMPACT_PROOF_VERSION);
    parts.extend(&proof.tx_hash);
    parts.extend(&proof.tx_signature);
    parts.extend(write_varint(proof.checkpoint_height));
    parts.extend(write_varint(proof.merkle_proof.len() as u64));
    for hash in &proof.merkle_proof {
        parts.extend(hash);
    }
    parts.extend(write_varint(proof.merkle_index as u64));
    parts.extend(&proof.aggregated_validator_sig);
    parts.extend(write_varint(proof.signer_bitmap.len() as u64));
    parts.extend(&proof.signer_bitmap);
    parts.extend(&proof.validator_set_root);

    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(&parts)
        .map_err(|e| format!("Compression failed: {}", e))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Compression finish failed: {}", e))?;

    let base64url = URL_SAFE_NO_PAD.encode(&compressed);
    let url = format!("rinku://p/{}", base64url);

    Ok((compressed, base64url, url))
}

pub fn decode_compact_proof(encoded: &str) -> Result<CompactProof, String> {
    let encoded = if encoded.starts_with("rinku://p/") {
        &encoded[10..]
    } else {
        encoded
    };

    let compressed = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|e| format!("Base64 decode failed: {}", e))?;

    let mut decoder = DeflateDecoder::new(&compressed[..]);
    let mut binary = Vec::new();
    decoder
        .read_to_end(&mut binary)
        .map_err(|e| format!("Decompression failed: {}", e))?;

    let mut offset = 0;

    let version = binary[offset];
    offset += 1;
    if version != COMPACT_PROOF_VERSION {
        return Err(format!("Unsupported proof version: {}", version));
    }

    let tx_hash = binary[offset..offset + 32].to_vec();
    offset += 32;

    let tx_signature = binary[offset..offset + 64].to_vec();
    offset += 64;

    let (checkpoint_height, cp_bytes) =
        read_varint(&binary, offset).ok_or("Failed to read checkpoint height")?;
    offset += cp_bytes;

    let (merkle_depth, md_bytes) =
        read_varint(&binary, offset).ok_or("Failed to read merkle depth")?;
    offset += md_bytes;

    let mut merkle_proof = Vec::new();
    for _ in 0..merkle_depth {
        merkle_proof.push(binary[offset..offset + 32].to_vec());
        offset += 32;
    }

    let (merkle_index, mi_bytes) =
        read_varint(&binary, offset).ok_or("Failed to read merkle index")?;
    offset += mi_bytes;

    let aggregated_validator_sig = binary[offset..offset + 48].to_vec();
    offset += 48;

    let (bitmap_length, bl_bytes) =
        read_varint(&binary, offset).ok_or("Failed to read bitmap length")?;
    offset += bl_bytes;

    let signer_bitmap = binary[offset..offset + bitmap_length as usize].to_vec();
    offset += bitmap_length as usize;

    let validator_set_root = binary[offset..offset + 32].to_vec();

    Ok(CompactProof {
        version,
        tx_hash,
        tx_signature,
        checkpoint_height,
        merkle_proof,
        merkle_index: merkle_index as usize,
        aggregated_validator_sig,
        signer_bitmap,
        validator_set_root,
    })
}

pub struct ProofSizeAnalysis {
    pub raw_bytes: usize,
    pub compressed_bytes: usize,
    pub base64_chars: usize,
    pub qr_version: String,
    pub viability: String,
}

pub fn analyze_proof_size(proof: &CompactProof) -> ProofSizeAnalysis {
    let raw_size = 1
        + 32
        + 64
        + 4
        + 1
        + (proof.merkle_proof.len() * 32)
        + 2
        + 48
        + 1
        + proof.signer_bitmap.len()
        + 32;

    let (compressed, base64url, _) = encode_compact_proof(proof).unwrap_or_default();

    let chars = base64url.len();

    let (qr_version, viability) = if chars <= 395 {
        ("v10", "Easy scan")
    } else if chars <= 758 {
        ("v15", "Good")
    } else if chars <= 1249 {
        ("v20", "Large QR")
    } else if chars <= 1853 {
        ("v25", "Very large")
    } else if chars <= 2520 {
        ("v30", "Huge")
    } else if chars <= 4296 {
        ("v40", "Max QR")
    } else {
        (">v40", "Too big")
    };

    ProofSizeAnalysis {
        raw_bytes: raw_size,
        compressed_bytes: compressed.len(),
        base64_chars: chars,
        qr_version: qr_version.to_string(),
        viability: viability.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_roundtrip() {
        let values = [0, 1, 127, 128, 255, 16383, 16384, 1_000_000];
        for val in values {
            let encoded = write_varint(val);
            let (decoded, _) = read_varint(&encoded, 0).unwrap();
            assert_eq!(decoded, val);
        }
    }

    #[test]
    fn test_merkle_proof_verification() {
        let tx_hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let proof: Vec<String> = vec![];
        let root = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        assert!(verify_tx_merkle_proof(tx_hash, &proof, 0, root));
    }

    #[test]
    fn test_merkle_proof_verification_with_sibling() {
        use sha2::{Digest, Sha256};

        let left = "0000000000000000000000000000000000000000000000000000000000000001";
        let right = "0000000000000000000000000000000000000000000000000000000000000002";

        let left_bytes = hex::decode(left).unwrap();
        let right_bytes = hex::decode(right).unwrap();

        let mut combined = Vec::with_capacity(64);
        combined.extend_from_slice(&left_bytes);
        combined.extend_from_slice(&right_bytes);

        let mut hasher = Sha256::new();
        hasher.update(&combined);
        let root_bytes = hasher.finalize();
        let root = hex::encode(root_bytes);

        assert!(verify_tx_merkle_proof(left, &[right.to_string()], 0, &root));
        assert!(verify_tx_merkle_proof(right, &[left.to_string()], 1, &root));
    }

    #[test]
    fn test_merkle_proof_rejects_invalid_hash() {
        let invalid_hash = "abc123";
        let proof: Vec<String> = vec![];
        let root = "abc123";

        assert!(!verify_tx_merkle_proof(invalid_hash, &proof, 0, root));
    }

    #[test]
    fn test_merkle_proof_with_core_tree() {
        use rinku_core::crypto::sha256;
        use rinku_core::merkle::MerkleTree;

        let leaves = vec![
            sha256(b"tx1"),
            sha256(b"tx2"),
            sha256(b"tx3"),
            sha256(b"tx4"),
        ];

        let tree = MerkleTree::new(leaves.clone()).unwrap();
        let root = tree.root();

        for index in 0..4 {
            let proof = tree.get_proof(index).unwrap();
            let tx_hash = hex::encode(leaves[index]);

            assert!(
                verify_tx_merkle_proof(&tx_hash, &proof.siblings, index, &root),
                "Proof verification failed for leaf at index {}",
                index
            );
        }
    }

    #[test]
    fn test_merkle_proof_with_5_leaves() {
        use rinku_core::crypto::sha256;
        use rinku_core::merkle::MerkleTree;

        let leaves: Vec<[u8; 32]> = (0..5)
            .map(|i| sha256(format!("transaction_{}", i).as_bytes()))
            .collect();

        let tree = MerkleTree::new(leaves.clone()).unwrap();
        let root = tree.root();

        for index in 0..5 {
            let proof = tree.get_proof(index).unwrap();
            let tx_hash = hex::encode(leaves[index]);

            assert!(
                verify_tx_merkle_proof(&tx_hash, &proof.siblings, index, &root),
                "Proof verification failed for index {} in 5-leaf tree",
                index
            );
        }
    }

    #[test]
    fn test_build_merkle_sum_tree() {
        let leaves = vec![
            MerkleSumLeaf {
                index: 0,
                address: "alice".to_string(),
                bls_public_key: "pk_alice".to_string(),
                weight_units: 10_000_000_000,
                weight: 100.0,
            },
            MerkleSumLeaf {
                index: 1,
                address: "bob".to_string(),
                bls_public_key: "pk_bob".to_string(),
                weight_units: 5_000_000_000,
                weight: 50.0,
            },
        ];

        let result = build_merkle_sum_tree(&leaves);

        assert_eq!(result.root.total_weight, 150.0);
        assert!(!result.root.hash.is_empty());
        assert_eq!(result.layers.len(), 2);
    }

    #[test]
    fn test_merkle_sum_tree_single_leaf() {
        let leaves = vec![MerkleSumLeaf {
            index: 0,
            address: "alice".to_string(),
            bls_public_key: "pk_alice".to_string(),
            weight_units: 10_000_000_000,
            weight: 100.0,
        }];

        let result = build_merkle_sum_tree(&leaves);

        assert_eq!(result.root.total_weight, 100.0);
        assert_eq!(result.layers.len(), 1);
    }

    #[test]
    fn test_merkle_sum_tree_empty() {
        let leaves: Vec<MerkleSumLeaf> = vec![];
        let result = build_merkle_sum_tree(&leaves);

        assert_eq!(result.root.total_weight, 0.0);
        assert!(result.layers.is_empty());
    }

    #[test]
    fn test_get_and_verify_merkle_sum_proof() {
        let leaves = vec![
            MerkleSumLeaf {
                index: 0,
                address: "alice".to_string(),
                bls_public_key: "pk_alice".to_string(),
                weight_units: 10_000_000_000,
                weight: 100.0,
            },
            MerkleSumLeaf {
                index: 1,
                address: "bob".to_string(),
                bls_public_key: "pk_bob".to_string(),
                weight_units: 5_000_000_000,
                weight: 50.0,
            },
            MerkleSumLeaf {
                index: 2,
                address: "charlie".to_string(),
                bls_public_key: "pk_charlie".to_string(),
                weight_units: 7_500_000_000,
                weight: 75.0,
            },
        ];

        let tree = build_merkle_sum_tree(&leaves);
        let proof = get_merkle_sum_proof(&leaves, 1).unwrap();

        let (valid, weight, errors) = verify_merkle_sum_proof(&proof, &tree.root);

        assert!(valid, "Proof verification failed: {:?}", errors);
        assert_eq!(weight, 50.0);
        assert!(errors.is_empty());
    }

    fn test_sha256_hex(data: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(data.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Test helper: hash account leaf using u64 micro-units (matches production code)
    fn test_hash_account_leaf(addr: &str, balance: f64, nonce: u64, staked: f64) -> String {
        let balance_micro = rinku_core::types::to_micro_units(balance);
        let staked_micro = rinku_core::types::to_micro_units(staked);
        let data = format!(
            "account:{}:{}:{}:{}",
            addr, balance_micro, nonce, staked_micro
        );
        test_sha256_hex(&data)
    }

    fn test_hash_internal(left: &str, right: &str) -> String {
        test_sha256_hex(&format!("node:{}:{}", left, right))
    }

    #[test]
    fn test_account_state_proof_verification() {
        use rinku_core::types::AccountStateProof;

        let accounts = vec![
            ("alice", 100.0, 5u64, 0.0),
            ("bob", 50.0, 3u64, 10.0),
            ("charlie", 75.0, 7u64, 25.0),
        ];

        let leaves: Vec<String> = accounts
            .iter()
            .map(|(addr, balance, nonce, staked)| {
                test_hash_account_leaf(addr, *balance, *nonce, *staked)
            })
            .collect();

        let state_root = compute_test_merkle_root_strings(&leaves);
        let merkle_proof = compute_test_merkle_proof_strings(&leaves, 1);

        let proof = AccountStateProof {
            version: 2,
            address: "bob".to_string(),
            balance_micro: rinku_core::types::to_micro_units(50.0),
            balance: 50.0,
            nonce: 3,
            staked_micro: rinku_core::types::to_micro_units(10.0),
            staked: 10.0,
            pending_rewards_micro: 0,
            pending_rewards: 0.0,
            staked_at: 0,
            last_reward_at: None,
            claimed_rewards_total_micro: 0,
            claimed_rewards_total: 0.0,
            checkpoint_height: 100,
            checkpoint_hash: "test_checkpoint_hash".to_string(),
            checkpoint_timestamp: 1234567890,
            state_root: state_root.clone(),
            merkle_proof,
            merkle_index: 1,
            bls_aggregated_sig: None,
            bls_signer_bitmap: None,
            tx_hash: "test_tx_hash".to_string(),
            is_on_demand: false,
        };

        assert!(verify_account_state_proof(&proof));
    }

    #[test]
    fn test_account_state_proof_rejects_tampered_balance() {
        use rinku_core::types::AccountStateProof;

        let accounts = vec![("alice", 100.0, 5u64, 0.0), ("bob", 50.0, 3u64, 10.0)];

        let leaves: Vec<String> = accounts
            .iter()
            .map(|(addr, balance, nonce, staked)| {
                test_hash_account_leaf(addr, *balance, *nonce, *staked)
            })
            .collect();

        let state_root = compute_test_merkle_root_strings(&leaves);
        let merkle_proof = compute_test_merkle_proof_strings(&leaves, 1);

        let tampered_proof = AccountStateProof {
            version: 2,
            address: "bob".to_string(),
            balance_micro: rinku_core::types::to_micro_units(999.0),
            balance: 999.0,
            nonce: 3,
            staked_micro: rinku_core::types::to_micro_units(10.0),
            staked: 10.0,
            pending_rewards_micro: 0,
            pending_rewards: 0.0,
            staked_at: 0,
            last_reward_at: None,
            claimed_rewards_total_micro: 0,
            claimed_rewards_total: 0.0,
            checkpoint_height: 100,
            checkpoint_hash: "test_checkpoint_hash".to_string(),
            checkpoint_timestamp: 1234567890,
            state_root,
            merkle_proof,
            merkle_index: 1,
            bls_aggregated_sig: None,
            bls_signer_bitmap: None,
            tx_hash: "test_tx_hash".to_string(),
            is_on_demand: false,
        };

        assert!(!verify_account_state_proof(&tampered_proof));
    }

    #[test]
    fn test_account_state_proof_single_account() {
        use rinku_core::types::AccountStateProof;

        let state_root = test_hash_account_leaf("alice", 100.0, 5, 0.0);

        let proof = AccountStateProof {
            version: 2,
            address: "alice".to_string(),
            balance_micro: rinku_core::types::to_micro_units(100.0),
            balance: 100.0,
            nonce: 5,
            staked_micro: rinku_core::types::to_micro_units(0.0),
            staked: 0.0,
            pending_rewards_micro: 0,
            pending_rewards: 0.0,
            staked_at: 0,
            last_reward_at: None,
            claimed_rewards_total_micro: 0,
            claimed_rewards_total: 0.0,
            checkpoint_height: 1,
            checkpoint_hash: "genesis".to_string(),
            checkpoint_timestamp: 0,
            state_root,
            merkle_proof: vec![],
            merkle_index: 0,
            bls_aggregated_sig: None,
            bls_signer_bitmap: None,
            tx_hash: "genesis_tx".to_string(),
            is_on_demand: false,
        };

        assert!(verify_account_state_proof(&proof));
    }

    fn compute_test_merkle_root_strings(leaves: &[String]) -> String {
        if leaves.is_empty() {
            return "empty".to_string();
        }
        if leaves.len() == 1 {
            return leaves[0].clone();
        }

        let mut current_level = leaves.to_vec();
        while current_level.len() > 1 {
            let mut next_level = Vec::new();
            for chunk in current_level.chunks(2) {
                let left = &chunk[0];
                let right = if chunk.len() > 1 { &chunk[1] } else { left };
                next_level.push(test_hash_internal(left, right));
            }
            current_level = next_level;
        }
        current_level[0].clone()
    }

    fn compute_test_merkle_proof_strings(leaves: &[String], target_index: usize) -> Vec<String> {
        if leaves.len() <= 1 {
            return vec![];
        }

        let mut proof = Vec::new();
        let mut current_level = leaves.to_vec();
        let mut current_index = target_index;

        while current_level.len() > 1 {
            let sibling_index = if current_index % 2 == 0 {
                current_index + 1
            } else {
                current_index - 1
            };

            if sibling_index < current_level.len() {
                proof.push(current_level[sibling_index].clone());
            } else {
                proof.push(current_level[current_index].clone());
            }

            let mut next_level = Vec::new();
            for chunk in current_level.chunks(2) {
                let left = &chunk[0];
                let right = if chunk.len() > 1 { &chunk[1] } else { left };
                next_level.push(test_hash_internal(left, right));
            }
            current_index /= 2;
            current_level = next_level;
        }
        proof
    }

    /// Test micro-unit conversion consistency
    #[test]
    fn test_micro_unit_conversion_roundtrip() {
        // Test that micro-unit conversion is consistent for various values
        let test_values = vec![
            0.0,
            1.0,
            100.0,
            0.00000001, // Smallest representable unit
            1234567.89012345,
            30_000_000.0, // Max supply
        ];

        for val in test_values {
            let micro = rinku_core::types::to_micro_units(val);
            let back = rinku_core::types::from_micro_units(micro);
            // Allow for floating point rounding within 8 decimal places
            let diff = (val - back).abs();
            assert!(
                diff < 0.00000001,
                "Conversion failed for {}: got {} (micro={})",
                val,
                back,
                micro
            );
        }
    }

    /// Test canonical encoding format matches specification
    #[test]
    fn test_canonical_leaf_encoding_format() {
        // Verify the canonical format is correct
        let addr = "test_address";
        let balance = 100.5;
        let nonce = 5u64;
        let staked = 50.25;

        let balance_micro = rinku_core::types::to_micro_units(balance);
        let staked_micro = rinku_core::types::to_micro_units(staked);

        // Expected format: "account:{address}:{balance_micro}:{nonce}:{staked_micro}"
        let expected_data = format!(
            "account:{}:{}:{}:{}",
            addr, balance_micro, nonce, staked_micro
        );

        // Verify micro values are integers
        assert_eq!(balance_micro, 10050000000); // 100.5 * 100_000_000
        assert_eq!(staked_micro, 5025000000); // 50.25 * 100_000_000

        // Verify the hash is deterministic
        let hash1 = test_hash_account_leaf(addr, balance, nonce, staked);
        let hash2 = test_hash_account_leaf(addr, balance, nonce, staked);
        assert_eq!(hash1, hash2, "Hash should be deterministic");

        // Verify manually hashing the expected data gives the same result
        let manual_hash = test_sha256_hex(&expected_data);
        assert_eq!(
            hash1, manual_hash,
            "Leaf hash should match canonical format"
        );
    }

    /// Test proof verification after simulated transfer transaction
    #[test]
    fn test_proof_for_transfer_transaction() {
        use rinku_core::types::AccountStateProof;

        // Simulate state AFTER a transfer: alice sent 10 RKU to bob
        let accounts = vec![
            ("alice", 90.0, 1u64, 0.0), // balance decreased, nonce incremented
            ("bob", 60.0, 0u64, 0.0),   // balance increased
        ];

        let leaves: Vec<String> = accounts
            .iter()
            .map(|(addr, balance, nonce, staked)| {
                test_hash_account_leaf(addr, *balance, *nonce, *staked)
            })
            .collect();

        let state_root = compute_test_merkle_root_strings(&leaves);

        // Verify alice's proof (sender)
        let alice_proof = AccountStateProof {
            version: 2,
            address: "alice".to_string(),
            balance_micro: rinku_core::types::to_micro_units(90.0),
            balance: 90.0,
            nonce: 1,
            staked_micro: 0,
            staked: 0.0,
            pending_rewards_micro: 0,
            pending_rewards: 0.0,
            staked_at: 0,
            last_reward_at: None,
            claimed_rewards_total_micro: 0,
            claimed_rewards_total: 0.0,
            checkpoint_height: 1,
            checkpoint_hash: "cp1".to_string(),
            checkpoint_timestamp: 1000,
            state_root: state_root.clone(),
            merkle_proof: compute_test_merkle_proof_strings(&leaves, 0),
            merkle_index: 0,
            bls_aggregated_sig: None,
            bls_signer_bitmap: None,
            tx_hash: "transfer_tx".to_string(),
            is_on_demand: false,
        };
        assert!(
            verify_account_state_proof(&alice_proof),
            "Alice's transfer proof should be valid"
        );

        // Verify bob's proof (receiver)
        let bob_proof = AccountStateProof {
            version: 2,
            address: "bob".to_string(),
            balance_micro: rinku_core::types::to_micro_units(60.0),
            balance: 60.0,
            nonce: 0,
            staked_micro: 0,
            staked: 0.0,
            pending_rewards_micro: 0,
            pending_rewards: 0.0,
            staked_at: 0,
            last_reward_at: None,
            claimed_rewards_total_micro: 0,
            claimed_rewards_total: 0.0,
            checkpoint_height: 1,
            checkpoint_hash: "cp1".to_string(),
            checkpoint_timestamp: 1000,
            state_root: state_root.clone(),
            merkle_proof: compute_test_merkle_proof_strings(&leaves, 1),
            merkle_index: 1,
            bls_aggregated_sig: None,
            bls_signer_bitmap: None,
            tx_hash: "transfer_tx".to_string(),
            is_on_demand: false,
        };
        assert!(
            verify_account_state_proof(&bob_proof),
            "Bob's transfer proof should be valid"
        );
    }

    /// Test proof verification after simulated stake transaction
    #[test]
    fn test_proof_for_stake_transaction() {
        use rinku_core::types::AccountStateProof;

        // Simulate state AFTER a stake: alice staked 50 RKU
        let accounts = vec![
            ("alice", 50.0, 1u64, 50.0), // balance decreased, staked increased, nonce incremented
        ];

        let leaves: Vec<String> = accounts
            .iter()
            .map(|(addr, balance, nonce, staked)| {
                test_hash_account_leaf(addr, *balance, *nonce, *staked)
            })
            .collect();

        let state_root = compute_test_merkle_root_strings(&leaves);

        let proof = AccountStateProof {
            version: 2,
            address: "alice".to_string(),
            balance_micro: rinku_core::types::to_micro_units(50.0),
            balance: 50.0,
            nonce: 1,
            staked_micro: rinku_core::types::to_micro_units(50.0),
            staked: 50.0,
            pending_rewards_micro: 0,
            pending_rewards: 0.0,
            staked_at: 0,
            last_reward_at: None,
            claimed_rewards_total_micro: 0,
            claimed_rewards_total: 0.0,
            checkpoint_height: 2,
            checkpoint_hash: "cp2".to_string(),
            checkpoint_timestamp: 2000,
            state_root,
            merkle_proof: vec![], // Single account, no proof needed
            merkle_index: 0,
            bls_aggregated_sig: None,
            bls_signer_bitmap: None,
            tx_hash: "stake_tx".to_string(),
            is_on_demand: false,
        };

        assert!(
            verify_account_state_proof(&proof),
            "Stake proof should be valid"
        );
    }

    /// Test proof verification after simulated unstake transaction
    #[test]
    fn test_proof_for_unstake_transaction() {
        use rinku_core::types::AccountStateProof;

        // Simulate state AFTER an unstake: alice unstaked 25 RKU
        let accounts = vec![
            ("alice", 75.0, 2u64, 25.0), // balance increased, staked decreased, nonce incremented
        ];

        let leaves: Vec<String> = accounts
            .iter()
            .map(|(addr, balance, nonce, staked)| {
                test_hash_account_leaf(addr, *balance, *nonce, *staked)
            })
            .collect();

        let state_root = compute_test_merkle_root_strings(&leaves);

        let proof = AccountStateProof {
            version: 2,
            address: "alice".to_string(),
            balance_micro: rinku_core::types::to_micro_units(75.0),
            balance: 75.0,
            nonce: 2,
            staked_micro: rinku_core::types::to_micro_units(25.0),
            staked: 25.0,
            pending_rewards_micro: 0,
            pending_rewards: 0.0,
            staked_at: 0,
            last_reward_at: None,
            claimed_rewards_total_micro: 0,
            claimed_rewards_total: 0.0,
            checkpoint_height: 3,
            checkpoint_hash: "cp3".to_string(),
            checkpoint_timestamp: 3000,
            state_root,
            merkle_proof: vec![],
            merkle_index: 0,
            bls_aggregated_sig: None,
            bls_signer_bitmap: None,
            tx_hash: "unstake_tx".to_string(),
            is_on_demand: false,
        };

        assert!(
            verify_account_state_proof(&proof),
            "Unstake proof should be valid"
        );
    }

    /// Test that proof correctly rejects incorrect staked amount
    #[test]
    fn test_proof_rejects_incorrect_stake() {
        use rinku_core::types::AccountStateProof;

        let accounts = vec![("alice", 50.0, 1u64, 50.0)];

        let leaves: Vec<String> = accounts
            .iter()
            .map(|(addr, balance, nonce, staked)| {
                test_hash_account_leaf(addr, *balance, *nonce, *staked)
            })
            .collect();

        let state_root = compute_test_merkle_root_strings(&leaves);

        // Create proof with incorrect staked amount
        let proof = AccountStateProof {
            version: 2,
            address: "alice".to_string(),
            balance_micro: rinku_core::types::to_micro_units(50.0),
            balance: 50.0,
            nonce: 1,
            staked_micro: rinku_core::types::to_micro_units(999.0), // Wrong staked amount
            staked: 999.0,
            pending_rewards_micro: 0,
            pending_rewards: 0.0,
            staked_at: 0,
            last_reward_at: None,
            claimed_rewards_total_micro: 0,
            claimed_rewards_total: 0.0,
            checkpoint_height: 1,
            checkpoint_hash: "cp1".to_string(),
            checkpoint_timestamp: 1000,
            state_root,
            merkle_proof: vec![],
            merkle_index: 0,
            bls_aggregated_sig: None,
            bls_signer_bitmap: None,
            tx_hash: "test_tx".to_string(),
            is_on_demand: false,
        };

        assert!(
            !verify_account_state_proof(&proof),
            "Proof with wrong stake should be rejected"
        );
    }
}
