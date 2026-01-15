use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use flate2::{read::DeflateDecoder, write::DeflateEncoder, Compression};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

use crate::bls::{parse_signer_bitmap, verify_aggregated_signature};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MerkleSumLeaf {
    pub index: usize,
    pub address: String,
    pub bls_public_key: String,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MerkleSumRoot {
    pub hash: String,
    pub total_weight: f64,
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
pub struct MerkleSumProofSibling {
    pub hash: String,
    pub weight: f64,
    pub is_left: bool,
}

#[derive(Debug, Clone)]
pub struct MerkleSumNode {
    pub hash: String,
    pub sum_weight: f64,
}

fn hash_leaf(leaf: &MerkleSumLeaf) -> String {
    let data = format!(
        "leaf:{}:{}:{}:{}",
        leaf.index, leaf.address, leaf.bls_public_key, leaf.weight
    );
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

fn hash_internal(left: &MerkleSumNode, right: &MerkleSumNode) -> String {
    let data = format!(
        "node:{}:{}:{}:{}",
        left.hash, left.sum_weight, right.hash, right.sum_weight
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
        sum_weight: 0.0,
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
            sum_weight: leaf.weight,
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
                sum_weight: left.sum_weight + right.sum_weight,
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
            total_weight: root_node.sum_weight,
        },
        layers,
    }
}

pub fn get_merkle_sum_proof(leaves: &[MerkleSumLeaf], leaf_index: usize) -> Option<MerkleSumProof> {
    let mut sorted_leaves: Vec<MerkleSumLeaf> = leaves.to_vec();
    sorted_leaves.sort_by_key(|l| l.index);

    let target_leaf = sorted_leaves.iter().find(|l| l.index == leaf_index)?.clone();
    let position_in_array = sorted_leaves.iter().position(|l| l.index == leaf_index)?;

    let mut current_layer: Vec<MerkleSumNode> = sorted_leaves
        .iter()
        .map(|leaf| MerkleSumNode {
            hash: hash_leaf(leaf),
            sum_weight: leaf.weight,
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
            hash: sibling.hash,
            weight: sibling.sum_weight,
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
                sum_weight: left.sum_weight + right.sum_weight,
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
        sum_weight: proof.leaf.weight,
    };

    for (i, sibling) in proof.siblings.iter().enumerate() {
        let is_right = proof.path_bits.get(i).copied().unwrap_or(false);

        let sibling_node = MerkleSumNode {
            hash: sibling.hash.clone(),
            sum_weight: sibling.weight,
        };

        let (left, right) = if is_right {
            (&sibling_node, &current)
        } else {
            (&current, &sibling_node)
        };

        current = MerkleSumNode {
            hash: hash_internal(left, right),
            sum_weight: left.sum_weight + right.sum_weight,
        };
    }

    if current.hash != expected_root.hash {
        errors.push(format!(
            "Root hash mismatch: expected {}, got {}",
            expected_root.hash, current.hash
        ));
    }

    if (current.sum_weight - expected_root.total_weight).abs() > 0.0001 {
        errors.push(format!(
            "Total weight mismatch: expected {}, got {}",
            expected_root.total_weight, current.sum_weight
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
        height,
        tx_merkle_root,
        state_root,
        receipt_root,
        tip_count,
        timestamp
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

    let bls_valid = verify_aggregated_signature(&checkpoint_hash, &aggregated_sig, &signer_pub_keys);
    result.bls_verified = bls_valid;
    if !bls_valid {
        result.errors.push("BLS signature verification failed".to_string());
    }

    let total_weight = proof.validator_sum_tree_root.total_weight;
    if total_weight <= 0.0 {
        result.errors.push("Invalid total weight".to_string());
    } else {
        let weight_ratio = computed_signer_weight / total_weight;
        if weight_ratio < 0.67 {
            result.errors.push(format!(
                "Insufficient signer weight: {:.1}% (need 67%)",
                weight_ratio * 100.0
            ));
        }
    }

    result.valid =
        result.merkle_verified && result.bls_verified && result.validator_set_verified && result.errors.is_empty();

    result
}

pub fn encode_self_contained_proof(proof: &SelfContainedProof) -> Result<String, String> {
    let json = serde_json::to_string(proof).map_err(|e| format!("JSON serialization failed: {}", e))?;

    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(json.as_bytes())
        .map_err(|e| format!("Compression failed: {}", e))?;
    let compressed = encoder.finish().map_err(|e| format!("Compression finish failed: {}", e))?;

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
    let compressed = encoder.finish().map_err(|e| format!("Compression finish failed: {}", e))?;

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

    let (merkle_depth, md_bytes) = read_varint(&binary, offset).ok_or("Failed to read merkle depth")?;
    offset += md_bytes;

    let mut merkle_proof = Vec::new();
    for _ in 0..merkle_depth {
        merkle_proof.push(binary[offset..offset + 32].to_vec());
        offset += 32;
    }

    let (merkle_index, mi_bytes) = read_varint(&binary, offset).ok_or("Failed to read merkle index")?;
    offset += mi_bytes;

    let aggregated_validator_sig = binary[offset..offset + 48].to_vec();
    offset += 48;

    let (bitmap_length, bl_bytes) = read_varint(&binary, offset).ok_or("Failed to read bitmap length")?;
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
        let tx_hash = "abc123";
        let proof: Vec<String> = vec![];
        let root = "abc123";

        assert!(verify_tx_merkle_proof(tx_hash, &proof, 0, root));
    }

    #[test]
    fn test_build_merkle_sum_tree() {
        let leaves = vec![
            MerkleSumLeaf {
                index: 0,
                address: "alice".to_string(),
                bls_public_key: "pk_alice".to_string(),
                weight: 100.0,
            },
            MerkleSumLeaf {
                index: 1,
                address: "bob".to_string(),
                bls_public_key: "pk_bob".to_string(),
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
                weight: 100.0,
            },
            MerkleSumLeaf {
                index: 1,
                address: "bob".to_string(),
                bls_public_key: "pk_bob".to_string(),
                weight: 50.0,
            },
            MerkleSumLeaf {
                index: 2,
                address: "charlie".to_string(),
                bls_public_key: "pk_charlie".to_string(),
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
}
