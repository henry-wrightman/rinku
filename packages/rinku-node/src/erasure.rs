use anyhow::{anyhow, Result};
use reed_solomon_erasure::galois_8::ReedSolomon;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DA_SIZE_THRESHOLD: usize = 262_144; // 256KB — applied to total payload (bodies + proofs)

const MAX_SHARD_SIZE: usize = 4 * 1024 * 1024;
const MAX_TOTAL_SHARDS: usize = 64;
const MAX_ORIGINAL_SIZE: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaCommitment {
    pub merkle_root: String,
    pub data_shard_count: usize,
    pub parity_shard_count: usize,
    pub original_size: usize,
    #[serde(default)]
    pub compressed: bool,
    #[serde(default)]
    pub uncompressed_size: usize,
}

pub fn compress_da_payload(data: &[u8]) -> Result<Vec<u8>> {
    use flate2::write::DeflateEncoder;
    use flate2::Compression;
    use std::io::Write;

    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(data)
        .map_err(|e| anyhow!("DA compression failed: {}", e))?;
    encoder.finish()
        .map_err(|e| anyhow!("DA compression finalize failed: {}", e))
}

const MAX_UNCOMPRESSED_SIZE: usize = 32 * 1024 * 1024;

pub fn decompress_da_payload(data: &[u8], uncompressed_size: usize) -> Result<Vec<u8>> {
    use flate2::read::DeflateDecoder;
    use std::io::Read;

    if uncompressed_size == 0 || uncompressed_size > MAX_UNCOMPRESSED_SIZE {
        return Err(anyhow!(
            "invalid uncompressed_size: {} (max {})",
            uncompressed_size, MAX_UNCOMPRESSED_SIZE
        ));
    }

    let read_limit = uncompressed_size.saturating_mul(2).min(MAX_UNCOMPRESSED_SIZE);
    let decoder = DeflateDecoder::new(data);
    let mut result = Vec::with_capacity(uncompressed_size.min(MAX_UNCOMPRESSED_SIZE));
    decoder.take(read_limit as u64).read_to_end(&mut result)
        .map_err(|e| anyhow!("DA decompression failed: {}", e))?;

    if result.len() != uncompressed_size {
        return Err(anyhow!(
            "DA decompression size mismatch: expected {} bytes, got {}",
            uncompressed_size, result.len()
        ));
    }

    Ok(result)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaShard {
    pub index: usize,
    pub data: String,
    pub merkle_proof: Vec<String>,
}

pub fn compute_shard_params(validator_count: usize) -> (usize, usize) {
    let n = validator_count.max(3);
    let k = (n + 1) / 2;
    let m = n - k;
    (k, m.max(1))
}

pub fn encode(
    data: &[u8],
    data_shards: usize,
    parity_shards: usize,
) -> Result<(DaCommitment, Vec<DaShard>)> {
    if data.is_empty() {
        return Err(anyhow!("cannot erasure-encode empty data"));
    }
    if data.len() > MAX_ORIGINAL_SIZE {
        return Err(anyhow!(
            "data too large for erasure encoding: {} bytes",
            data.len()
        ));
    }
    let total = data_shards + parity_shards;
    if total > MAX_TOTAL_SHARDS {
        return Err(anyhow!("too many shards: {}", total));
    }
    if data_shards == 0 || parity_shards == 0 {
        return Err(anyhow!("need at least 1 data and 1 parity shard"));
    }

    let rs = ReedSolomon::new(data_shards, parity_shards)
        .map_err(|e| anyhow!("reed-solomon init failed: {}", e))?;

    let original_size = data.len();
    let shard_size = (original_size + data_shards - 1) / data_shards;

    if shard_size > MAX_SHARD_SIZE {
        return Err(anyhow!(
            "shard size {} exceeds maximum {}",
            shard_size,
            MAX_SHARD_SIZE
        ));
    }

    let mut shards: Vec<Vec<u8>> = Vec::with_capacity(total);
    for i in 0..data_shards {
        let start = i * shard_size;
        let end = (start + shard_size).min(original_size);
        let mut shard = Vec::with_capacity(shard_size);
        if start < original_size {
            shard.extend_from_slice(&data[start..end]);
        }
        shard.resize(shard_size, 0);
        shards.push(shard);
    }
    for _ in 0..parity_shards {
        shards.push(vec![0u8; shard_size]);
    }

    rs.encode(&mut shards)
        .map_err(|e| anyhow!("reed-solomon encode failed: {}", e))?;

    let shard_hashes: Vec<String> = shards.iter().map(|s| sha256_bytes(s)).collect();

    let merkle_root = compute_merkle_root(&shard_hashes);

    let da_shards: Vec<DaShard> = shards
        .into_iter()
        .enumerate()
        .map(|(i, shard_data)| {
            let proof = compute_merkle_proof(&shard_hashes, i);
            DaShard {
                index: i,
                data: base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &shard_data,
                ),
                merkle_proof: proof,
            }
        })
        .collect();

    let commitment = DaCommitment {
        merkle_root,
        data_shard_count: data_shards,
        parity_shard_count: parity_shards,
        original_size,
        compressed: false,
        uncompressed_size: 0,
    };

    Ok((commitment, da_shards))
}

pub fn decode(commitment: &DaCommitment, received_shards: &[Option<DaShard>]) -> Result<Vec<u8>> {
    let total = commitment.data_shard_count + commitment.parity_shard_count;
    if total > MAX_TOTAL_SHARDS {
        return Err(anyhow!("invalid commitment: too many shards {}", total));
    }
    if commitment.original_size > MAX_ORIGINAL_SIZE {
        return Err(anyhow!(
            "invalid commitment: original size too large {}",
            commitment.original_size
        ));
    }
    if commitment.data_shard_count == 0 || commitment.parity_shard_count == 0 {
        return Err(anyhow!("invalid commitment: zero shard counts"));
    }
    if received_shards.len() != total {
        return Err(anyhow!(
            "shard count mismatch: expected {}, got {}",
            total,
            received_shards.len()
        ));
    }

    let mut present_count = 0usize;
    for shard in received_shards {
        if shard.is_some() {
            present_count += 1;
        }
    }
    if present_count < commitment.data_shard_count {
        return Err(anyhow!(
            "insufficient shards for reconstruction: have {}, need {}",
            present_count,
            commitment.data_shard_count
        ));
    }

    let shard_size =
        (commitment.original_size + commitment.data_shard_count - 1) / commitment.data_shard_count;

    let mut shard_buffers: Vec<Option<Vec<u8>>> = Vec::with_capacity(total);
    for shard_opt in received_shards {
        match shard_opt {
            Some(shard) => {
                if shard.index >= total {
                    return Err(anyhow!(
                        "shard index {} out of range (total {})",
                        shard.index,
                        total
                    ));
                }

                let decoded =
                    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &shard.data)
                        .map_err(|e| {
                            anyhow!("base64 decode failed for shard {}: {}", shard.index, e)
                        })?;

                if decoded.len() != shard_size {
                    return Err(anyhow!(
                        "shard {} size mismatch: expected {}, got {}",
                        shard.index,
                        shard_size,
                        decoded.len()
                    ));
                }

                let shard_hash = sha256_bytes(&decoded);
                if !verify_merkle_proof(
                    &commitment.merkle_root,
                    &shard_hash,
                    shard.index,
                    &shard.merkle_proof,
                    total,
                ) {
                    return Err(anyhow!(
                        "merkle proof verification failed for shard {}",
                        shard.index
                    ));
                }

                shard_buffers.push(Some(decoded));
            }
            None => {
                shard_buffers.push(None);
            }
        }
    }

    let rs = ReedSolomon::new(commitment.data_shard_count, commitment.parity_shard_count)
        .map_err(|e| anyhow!("reed-solomon init failed: {}", e))?;

    let mut shards_for_rs: Vec<Option<Vec<u8>>> = shard_buffers;
    rs.reconstruct(&mut shards_for_rs)
        .map_err(|e| anyhow!("reed-solomon reconstruction failed: {}", e))?;

    let mut result = Vec::with_capacity(commitment.original_size);
    for i in 0..commitment.data_shard_count {
        if let Some(ref shard_data) = shards_for_rs[i] {
            result.extend_from_slice(shard_data);
        } else {
            return Err(anyhow!("data shard {} missing after reconstruction", i));
        }
    }

    result.truncate(commitment.original_size);
    Ok(result)
}

fn sha256_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn sha256_pair(a: &str, b: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(a.as_bytes());
    hasher.update(b.as_bytes());
    hex::encode(hasher.finalize())
}

fn compute_merkle_root(leaves: &[String]) -> String {
    if leaves.is_empty() {
        return sha256_bytes(b"empty");
    }
    if leaves.len() == 1 {
        return leaves[0].clone();
    }

    let next_pow2 = leaves.len().next_power_of_two();
    let mut layer: Vec<String> = leaves.to_vec();
    while layer.len() < next_pow2 {
        layer.push(sha256_bytes(b"padding"));
    }

    while layer.len() > 1 {
        let mut next_layer = Vec::with_capacity(layer.len() / 2);
        for i in (0..layer.len()).step_by(2) {
            next_layer.push(sha256_pair(&layer[i], &layer[i + 1]));
        }
        layer = next_layer;
    }

    layer[0].clone()
}

fn compute_merkle_proof(leaves: &[String], index: usize) -> Vec<String> {
    if leaves.len() <= 1 {
        return Vec::new();
    }

    let next_pow2 = leaves.len().next_power_of_two();
    let mut padded: Vec<String> = leaves.to_vec();
    while padded.len() < next_pow2 {
        padded.push(sha256_bytes(b"padding"));
    }

    let mut proof = Vec::new();
    let mut idx = index;
    let mut layer = padded;

    while layer.len() > 1 {
        let sibling = if idx % 2 == 0 { idx + 1 } else { idx - 1 };
        if sibling < layer.len() {
            proof.push(layer[sibling].clone());
        }

        let mut next_layer = Vec::with_capacity(layer.len() / 2);
        for i in (0..layer.len()).step_by(2) {
            next_layer.push(sha256_pair(&layer[i], &layer[i + 1]));
        }
        layer = next_layer;
        idx /= 2;
    }

    proof
}

pub fn verify_merkle_proof(
    root: &str,
    leaf_hash: &str,
    index: usize,
    proof: &[String],
    total_leaves: usize,
) -> bool {
    if total_leaves == 0 {
        return false;
    }
    if total_leaves == 1 {
        return leaf_hash == root;
    }

    let next_pow2 = total_leaves.next_power_of_two();
    let depth = (next_pow2 as f64).log2() as usize;

    if proof.len() != depth {
        return false;
    }

    let mut current = leaf_hash.to_string();
    let mut idx = index;

    for sibling in proof {
        current = if idx % 2 == 0 {
            sha256_pair(&current, sibling)
        } else {
            sha256_pair(sibling, &current)
        };
        idx /= 2;
    }

    current == root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_roundtrip() {
        let data = b"Hello, Rinku erasure coding! This is test data for the DA layer.";
        let (commitment, shards) = encode(data, 2, 1).unwrap();

        assert_eq!(commitment.data_shard_count, 2);
        assert_eq!(commitment.parity_shard_count, 1);
        assert_eq!(commitment.original_size, data.len());
        assert_eq!(shards.len(), 3);

        let all_shards: Vec<Option<DaShard>> = shards.into_iter().map(Some).collect();
        let recovered = decode(&commitment, &all_shards).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn test_decode_with_missing_parity() {
        let data = b"Testing reconstruction with missing parity shard.";
        let (commitment, shards) = encode(data, 2, 1).unwrap();

        let partial: Vec<Option<DaShard>> =
            vec![Some(shards[0].clone()), Some(shards[1].clone()), None];
        let recovered = decode(&commitment, &partial).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn test_decode_with_missing_data_shard() {
        let data = b"Testing reconstruction with one data shard missing.";
        let (commitment, shards) = encode(data, 2, 1).unwrap();

        let partial: Vec<Option<DaShard>> =
            vec![None, Some(shards[1].clone()), Some(shards[2].clone())];
        let recovered = decode(&commitment, &partial).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn test_insufficient_shards_fails() {
        let data = b"Not enough shards to reconstruct.";
        let (commitment, shards) = encode(data, 2, 1).unwrap();

        let partial: Vec<Option<DaShard>> = vec![Some(shards[0].clone()), None, None];
        assert!(decode(&commitment, &partial).is_err());
    }

    #[test]
    fn test_merkle_proof_verification() {
        let leaves: Vec<String> = (0..4)
            .map(|i| sha256_bytes(format!("leaf_{}", i).as_bytes()))
            .collect();
        let root = compute_merkle_root(&leaves);

        for (i, leaf) in leaves.iter().enumerate() {
            let proof = compute_merkle_proof(&leaves, i);
            assert!(verify_merkle_proof(&root, leaf, i, &proof, leaves.len()));
        }

        let bad_leaf = sha256_bytes(b"bad");
        let proof = compute_merkle_proof(&leaves, 0);
        assert!(!verify_merkle_proof(
            &root,
            &bad_leaf,
            0,
            &proof,
            leaves.len()
        ));
    }

    #[test]
    fn test_shard_params() {
        assert_eq!(compute_shard_params(3), (2, 1));
        assert_eq!(compute_shard_params(5), (3, 2));
        assert_eq!(compute_shard_params(7), (4, 3));
    }

    #[test]
    fn test_tampered_shard_rejected() {
        let data = b"Tamper detection test data for merkle verification.";
        let (commitment, mut shards) = encode(data, 2, 1).unwrap();

        shards[0].data = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            b"tampered data that is padded out to correct size!!",
        );

        let all_shards: Vec<Option<DaShard>> = shards.into_iter().map(Some).collect();
        assert!(decode(&commitment, &all_shards).is_err());
    }

    #[test]
    fn test_large_payload() {
        let data: Vec<u8> = (0..250_000u32).flat_map(|i| i.to_le_bytes()).collect();
        let (commitment, shards) = encode(&data, 2, 1).unwrap();
        assert!(commitment.original_size == data.len());

        let partial: Vec<Option<DaShard>> =
            vec![Some(shards[0].clone()), None, Some(shards[2].clone())];
        let recovered = decode(&commitment, &partial).unwrap();
        assert_eq!(recovered, data);
    }

    #[test]
    fn test_empty_data_rejected() {
        assert!(encode(b"", 2, 1).is_err());
    }

    #[test]
    fn test_five_validator_setup() {
        let data = b"Five validator erasure coding test with 3+2 shards.";
        let (k, m) = compute_shard_params(5);
        assert_eq!(k, 3);
        assert_eq!(m, 2);

        let (commitment, shards) = encode(data, k, m).unwrap();
        assert_eq!(shards.len(), 5);

        let partial: Vec<Option<DaShard>> = vec![
            Some(shards[0].clone()),
            None,
            Some(shards[2].clone()),
            None,
            Some(shards[4].clone()),
        ];
        let recovered = decode(&commitment, &partial).unwrap();
        assert_eq!(recovered, data);
    }
}
