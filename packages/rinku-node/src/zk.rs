#![cfg(feature = "zk")]

use anyhow::{anyhow, Result};
use ark_bn254::{Bn254, Fr, G1Affine, G2Affine};
use ark_ff::{BigInteger, Field, PrimeField};
use ark_groth16::{Groth16, Proof, VerifyingKey};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use light_poseidon::{Poseidon, PoseidonBytesHasher, PoseidonHasher};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

pub const ZK_URL_VERSION: u8 = 1;
pub const CHAIN_ID_MAINNET: &str = "rinku-mainnet";
pub const CHAIN_ID_TESTNET: &str = "rinku-testnet";
pub const MERKLE_DEPTH: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZkProofInput {
    pub tx_hash: String,
    pub sender_pub_key_x: String,
    pub sender_pub_key_y: String,
    pub merkle_path_elements: Vec<String>,
    pub merkle_path_indices: Vec<u8>,
    pub amount: String,
    pub amount_blinding: String,
    pub checkpoint_height: u64,
    pub chain_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZkPublicInputs {
    pub cp_root: String,
    pub nullifier: String,
    pub amount_commitment: String,
    pub chain_id_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZkProofPayload {
    pub v: u8,
    pub chain_id: String,
    pub cp_height: u64,
    pub proof: String,
    pub public_inputs: ZkPublicInputs,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_memo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aux_data: Option<ZkAuxData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZkAuxData {
    pub validator_root: String,
    pub total_weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZkVerifyResult {
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cp_height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_commitment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MerkleWitness {
    pub tx_hash: String,
    pub checkpoint_height: u64,
    pub merkle_path_elements: Vec<String>,
    pub merkle_path_indices: Vec<u8>,
    pub checkpoint_root: String,
}

pub fn poseidon_hash(inputs: &[Fr]) -> Fr {
    use light_poseidon::parameters::bn254_x5;

    if inputs.is_empty() {
        return Fr::from(0u64);
    }

    if inputs.len() > 12 {
        let first = poseidon_hash(&inputs[0..12]);
        let rest = &inputs[12..];
        let mut combined = vec![first];
        combined.extend_from_slice(rest);
        return poseidon_hash(&combined);
    }

    let mut poseidon =
        Poseidon::<Fr>::new_circom(inputs.len()).expect("Failed to create Poseidon hasher");

    poseidon
        .hash(inputs)
        .expect("Failed to compute Poseidon hash")
}

pub fn compute_nullifier(priv_key: &Fr, checkpoint_height: u64, tx_hash: &Fr) -> Fr {
    poseidon_hash(&[*priv_key, Fr::from(checkpoint_height), *tx_hash])
}

pub fn compute_amount_commitment(amount: &Fr, blinding: &Fr) -> Fr {
    poseidon_hash(&[*amount, *blinding])
}

pub fn compute_chain_id_hash(chain_id: &Fr) -> Fr {
    poseidon_hash(&[*chain_id])
}

pub fn compute_merkle_root(tx_hash: &Fr, path_elements: &[Fr], path_indices: &[u8]) -> Fr {
    let mut current = *tx_hash;

    for (i, (element, index)) in path_elements.iter().zip(path_indices.iter()).enumerate() {
        current = if *index == 0 {
            poseidon_hash(&[current, *element])
        } else {
            poseidon_hash(&[*element, current])
        };
        debug!("Merkle step {}: {:?}", i, current);
    }

    current
}

pub fn string_to_fr(s: &str) -> Result<Fr> {
    if s.starts_with("0x") {
        let bytes = hex::decode(&s[2..])?;
        Ok(Fr::from_be_bytes_mod_order(&bytes))
    } else {
        let n: u128 = s
            .parse()
            .map_err(|e| anyhow!("Failed to parse number: {}", e))?;
        Ok(Fr::from(n))
    }
}

pub fn fr_to_string(f: &Fr) -> String {
    let bytes = f.into_bigint().to_bytes_be();
    format!("0x{}", hex::encode(&bytes))
}

pub fn encode_zk_url(payload: &ZkProofPayload) -> Result<String> {
    let json = serde_json::to_vec(payload)?;

    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&json)?;
    let compressed = encoder.finish()?;

    let encoded = URL_SAFE_NO_PAD.encode(&compressed);

    Ok(format!("rinku://zk/{}", encoded))
}

pub fn decode_zk_url(url: &str) -> Result<ZkProofPayload> {
    if !url.starts_with("rinku://zk/") {
        return Err(anyhow!("Invalid ZK URL format"));
    }

    let encoded = &url[11..];
    let compressed = URL_SAFE_NO_PAD.decode(encoded)?;

    let mut decoder = DeflateDecoder::new(&compressed[..]);
    let mut json = Vec::new();
    decoder.read_to_end(&mut json)?;

    let payload: ZkProofPayload = serde_json::from_slice(&json)?;
    Ok(payload)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedProof {
    pub pi_a: [String; 2],
    pub pi_b: [[String; 2]; 2],
    pub pi_c: [String; 2],
}

pub fn serialize_proof(proof: &Proof<Bn254>) -> Result<String> {
    let mut bytes = Vec::new();
    proof.serialize_compressed(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(&bytes))
}

pub fn deserialize_proof(proof_str: &str) -> Result<Proof<Bn254>> {
    if proof_str.starts_with('{') {
        let parsed: SerializedProof = serde_json::from_str(proof_str)?;

        let a = parse_g1_affine(&parsed.pi_a[0], &parsed.pi_a[1])?;
        let b = parse_g2_affine(
            &parsed.pi_b[0][0],
            &parsed.pi_b[0][1],
            &parsed.pi_b[1][0],
            &parsed.pi_b[1][1],
        )?;
        let c = parse_g1_affine(&parsed.pi_c[0], &parsed.pi_c[1])?;

        Ok(Proof { a, b, c })
    } else {
        let bytes = URL_SAFE_NO_PAD.decode(proof_str)?;
        let proof = Proof::<Bn254>::deserialize_compressed(&bytes[..])?;
        Ok(proof)
    }
}

fn parse_g1_affine(x_str: &str, y_str: &str) -> Result<G1Affine> {
    let x = string_to_fr(x_str)?;
    let y = string_to_fr(y_str)?;

    let x_fq = ark_bn254::Fq::from_be_bytes_mod_order(&x.into_bigint().to_bytes_be());
    let y_fq = ark_bn254::Fq::from_be_bytes_mod_order(&y.into_bigint().to_bytes_be());

    Ok(G1Affine::new_unchecked(x_fq, y_fq))
}

fn parse_g2_affine(x0_str: &str, x1_str: &str, y0_str: &str, y1_str: &str) -> Result<G2Affine> {
    let x0 = string_to_fr(x0_str)?;
    let x1 = string_to_fr(x1_str)?;
    let y0 = string_to_fr(y0_str)?;
    let y1 = string_to_fr(y1_str)?;

    let x0_fq = ark_bn254::Fq::from_be_bytes_mod_order(&x0.into_bigint().to_bytes_be());
    let x1_fq = ark_bn254::Fq::from_be_bytes_mod_order(&x1.into_bigint().to_bytes_be());
    let y0_fq = ark_bn254::Fq::from_be_bytes_mod_order(&y0.into_bigint().to_bytes_be());
    let y1_fq = ark_bn254::Fq::from_be_bytes_mod_order(&y1.into_bigint().to_bytes_be());

    let x = ark_bn254::Fq2::new(x0_fq, x1_fq);
    let y = ark_bn254::Fq2::new(y0_fq, y1_fq);

    Ok(G2Affine::new_unchecked(x, y))
}

pub struct NullifierRegistry {
    inner: Arc<RwLock<HashSet<String>>>,
    db: Option<Arc<sled::Db>>,
}

impl NullifierRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashSet::new())),
            db: None,
        }
    }

    pub fn with_persistence(db: Arc<sled::Db>) -> Result<Self> {
        let tree = db.open_tree("nullifiers")?;
        let mut set = HashSet::new();

        for result in tree.iter() {
            if let Ok((key, _)) = result {
                if let Ok(nullifier) = String::from_utf8(key.to_vec()) {
                    set.insert(nullifier);
                }
            }
        }

        info!("Loaded {} nullifiers from persistence", set.len());

        Ok(Self {
            inner: Arc::new(RwLock::new(set)),
            db: Some(db),
        })
    }

    pub async fn has(&self, nullifier: &str) -> bool {
        self.inner.read().await.contains(nullifier)
    }

    pub async fn add(&self, nullifier: String) {
        let mut guard = self.inner.write().await;
        guard.insert(nullifier.clone());

        if let Some(ref db) = self.db {
            if let Ok(tree) = db.open_tree("nullifiers") {
                let _ = tree.insert(nullifier.as_bytes(), &[1u8]);
            }
        }
    }

    pub async fn remove(&self, nullifier: &str) {
        self.inner.write().await.remove(nullifier);

        if let Some(ref db) = self.db {
            if let Ok(tree) = db.open_tree("nullifiers") {
                let _ = tree.remove(nullifier.as_bytes());
            }
        }
    }

    pub async fn clear(&self) {
        self.inner.write().await.clear();

        if let Some(ref db) = self.db {
            if let Ok(tree) = db.open_tree("nullifiers") {
                let _ = tree.clear();
            }
        }
    }

    pub async fn size(&self) -> usize {
        self.inner.read().await.len()
    }
}

impl Default for NullifierRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for NullifierRegistry {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            db: self.db.clone(),
        }
    }
}

pub struct ZkVerifier {
    verification_key: Option<VerifyingKey<Bn254>>,
    nullifier_registry: NullifierRegistry,
    expected_chain_id: String,
}

impl ZkVerifier {
    pub fn new(chain_id: &str) -> Self {
        Self {
            verification_key: None,
            nullifier_registry: NullifierRegistry::new(),
            expected_chain_id: chain_id.to_string(),
        }
    }

    pub fn load_verification_key(&mut self, vkey_json: &str) -> Result<()> {
        let vk = parse_verification_key(vkey_json)?;
        self.verification_key = Some(vk);
        info!("ZK verification key loaded successfully");
        Ok(())
    }

    pub async fn verify_url(&self, url: &str) -> ZkVerifyResult {
        match decode_zk_url(url) {
            Ok(payload) => self.verify_payload(&payload).await,
            Err(e) => ZkVerifyResult {
                valid: false,
                reason: Some(format!("Failed to decode URL: {}", e)),
                cp_height: None,
                amount_commitment: None,
            },
        }
    }

    pub async fn verify_payload(&self, payload: &ZkProofPayload) -> ZkVerifyResult {
        if payload.v != ZK_URL_VERSION {
            return ZkVerifyResult {
                valid: false,
                reason: Some(format!("Unsupported version: {}", payload.v)),
                cp_height: None,
                amount_commitment: None,
            };
        }

        if payload.chain_id != self.expected_chain_id {
            return ZkVerifyResult {
                valid: false,
                reason: Some(format!(
                    "Chain ID mismatch: expected {}, got {}",
                    self.expected_chain_id, payload.chain_id
                )),
                cp_height: None,
                amount_commitment: None,
            };
        }

        if self
            .nullifier_registry
            .has(&payload.public_inputs.nullifier)
            .await
        {
            return ZkVerifyResult {
                valid: false,
                reason: Some("Nullifier already used (double-claim attempt)".to_string()),
                cp_height: None,
                amount_commitment: None,
            };
        }

        let vk = match &self.verification_key {
            Some(vk) => vk,
            None => {
                return ZkVerifyResult {
                    valid: false,
                    reason: Some("Verification key not loaded".to_string()),
                    cp_height: None,
                    amount_commitment: None,
                };
            }
        };

        let proof = match deserialize_proof(&payload.proof) {
            Ok(p) => p,
            Err(e) => {
                return ZkVerifyResult {
                    valid: false,
                    reason: Some(format!("Failed to deserialize proof: {}", e)),
                    cp_height: None,
                    amount_commitment: None,
                };
            }
        };

        let public_inputs = match self.parse_public_inputs(&payload.public_inputs) {
            Ok(inputs) => inputs,
            Err(e) => {
                return ZkVerifyResult {
                    valid: false,
                    reason: Some(format!("Failed to parse public inputs: {}", e)),
                    cp_height: None,
                    amount_commitment: None,
                };
            }
        };

        let pvk = ark_groth16::prepare_verifying_key(vk);
        match Groth16::<Bn254>::verify_proof(&pvk, &proof, &public_inputs) {
            Ok(true) => ZkVerifyResult {
                valid: true,
                reason: None,
                cp_height: Some(payload.cp_height),
                amount_commitment: Some(payload.public_inputs.amount_commitment.clone()),
            },
            Ok(false) => ZkVerifyResult {
                valid: false,
                reason: Some("Groth16 proof verification failed".to_string()),
                cp_height: None,
                amount_commitment: None,
            },
            Err(e) => ZkVerifyResult {
                valid: false,
                reason: Some(format!("Verification error: {}", e)),
                cp_height: None,
                amount_commitment: None,
            },
        }
    }

    pub async fn verify_and_consume_nullifier(&self, payload: &ZkProofPayload) -> ZkVerifyResult {
        let result = self.verify_payload(payload).await;

        if result.valid {
            self.nullifier_registry
                .add(payload.public_inputs.nullifier.clone())
                .await;
        }

        result
    }

    fn parse_public_inputs(&self, inputs: &ZkPublicInputs) -> Result<Vec<Fr>> {
        Ok(vec![
            string_to_fr(&inputs.cp_root)?,
            string_to_fr(&inputs.nullifier)?,
            string_to_fr(&inputs.amount_commitment)?,
            string_to_fr(&inputs.chain_id_hash)?,
        ])
    }

    pub fn nullifier_registry(&self) -> &NullifierRegistry {
        &self.nullifier_registry
    }
}

fn parse_verification_key(vkey_json: &str) -> Result<VerifyingKey<Bn254>> {
    let parsed: serde_json::Value = serde_json::from_str(vkey_json)?;

    let alpha_g1 = parse_g1_from_json(&parsed["vk_alpha_1"])?;
    let beta_g2 = parse_g2_from_json(&parsed["vk_beta_2"])?;
    let gamma_g2 = parse_g2_from_json(&parsed["vk_gamma_2"])?;
    let delta_g2 = parse_g2_from_json(&parsed["vk_delta_2"])?;

    let gamma_abc_g1: Vec<G1Affine> = parsed["IC"]
        .as_array()
        .ok_or_else(|| anyhow!("Missing IC array"))?
        .iter()
        .map(|v| parse_g1_from_json(v))
        .collect::<Result<Vec<_>>>()?;

    Ok(VerifyingKey {
        alpha_g1,
        beta_g2,
        gamma_g2,
        delta_g2,
        gamma_abc_g1,
    })
}

fn parse_g1_from_json(value: &serde_json::Value) -> Result<G1Affine> {
    let arr = value.as_array().ok_or_else(|| anyhow!("Expected array"))?;
    if arr.len() < 2 {
        return Err(anyhow!("G1 point needs at least 2 coordinates"));
    }

    let x_str = arr[0].as_str().ok_or_else(|| anyhow!("Expected string"))?;
    let y_str = arr[1].as_str().ok_or_else(|| anyhow!("Expected string"))?;

    parse_g1_affine(x_str, y_str)
}

fn parse_g2_from_json(value: &serde_json::Value) -> Result<G2Affine> {
    let arr = value.as_array().ok_or_else(|| anyhow!("Expected array"))?;
    if arr.len() < 2 {
        return Err(anyhow!("G2 point needs at least 2 coordinates"));
    }

    let x_arr = arr[0].as_array().ok_or_else(|| anyhow!("Expected array"))?;
    let y_arr = arr[1].as_array().ok_or_else(|| anyhow!("Expected array"))?;

    if x_arr.len() < 2 || y_arr.len() < 2 {
        return Err(anyhow!("G2 coordinate needs 2 elements"));
    }

    let x0_str = x_arr[0]
        .as_str()
        .ok_or_else(|| anyhow!("Expected string"))?;
    let x1_str = x_arr[1]
        .as_str()
        .ok_or_else(|| anyhow!("Expected string"))?;
    let y0_str = y_arr[0]
        .as_str()
        .ok_or_else(|| anyhow!("Expected string"))?;
    let y1_str = y_arr[1]
        .as_str()
        .ok_or_else(|| anyhow!("Expected string"))?;

    parse_g2_affine(x0_str, x1_str, y0_str, y1_str)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZkStats {
    pub proofs_verified: u64,
    pub proofs_valid: u64,
    pub proofs_invalid: u64,
    pub nullifiers_registered: usize,
}

pub struct ZkService {
    verifier: Arc<RwLock<ZkVerifier>>,
    stats: Arc<RwLock<ZkStats>>,
}

impl ZkService {
    pub fn new(chain_id: &str) -> Self {
        Self {
            verifier: Arc::new(RwLock::new(ZkVerifier::new(chain_id))),
            stats: Arc::new(RwLock::new(ZkStats {
                proofs_verified: 0,
                proofs_valid: 0,
                proofs_invalid: 0,
                nullifiers_registered: 0,
            })),
        }
    }

    pub async fn load_verification_key(&self, vkey_json: &str) -> Result<()> {
        self.verifier.write().await.load_verification_key(vkey_json)
    }

    pub async fn verify_url(&self, url: &str) -> ZkVerifyResult {
        let result = self.verifier.read().await.verify_url(url).await;

        let mut stats = self.stats.write().await;
        stats.proofs_verified += 1;
        if result.valid {
            stats.proofs_valid += 1;
        } else {
            stats.proofs_invalid += 1;
        }

        result
    }

    pub async fn verify_and_consume(&self, url: &str) -> ZkVerifyResult {
        let payload = match decode_zk_url(url) {
            Ok(p) => p,
            Err(e) => {
                return ZkVerifyResult {
                    valid: false,
                    reason: Some(format!("Failed to decode URL: {}", e)),
                    cp_height: None,
                    amount_commitment: None,
                };
            }
        };

        let result = self
            .verifier
            .read()
            .await
            .verify_and_consume_nullifier(&payload)
            .await;

        let mut stats = self.stats.write().await;
        stats.proofs_verified += 1;
        if result.valid {
            stats.proofs_valid += 1;
            stats.nullifiers_registered =
                self.verifier.read().await.nullifier_registry().size().await;
        } else {
            stats.proofs_invalid += 1;
        }

        result
    }

    pub async fn get_stats(&self) -> ZkStats {
        let mut stats = self.stats.read().await.clone();
        stats.nullifiers_registered = self.verifier.read().await.nullifier_registry().size().await;
        stats
    }

    pub async fn has_nullifier(&self, nullifier: &str) -> bool {
        self.verifier
            .read()
            .await
            .nullifier_registry()
            .has(nullifier)
            .await
    }
}

impl Clone for ZkService {
    fn clone(&self) -> Self {
        Self {
            verifier: self.verifier.clone(),
            stats: self.stats.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_poseidon_hash_single() {
        let input = Fr::from(123u64);
        let hash = poseidon_hash(&[input]);
        assert_ne!(hash, Fr::from(0u64));
    }

    #[test]
    fn test_poseidon_hash_deterministic() {
        let inputs = vec![Fr::from(1u64), Fr::from(2u64)];
        let hash1 = poseidon_hash(&inputs);
        let hash2 = poseidon_hash(&inputs);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_nullifier_computation() {
        let priv_key = Fr::from(12345u64);
        let checkpoint_height = 100u64;
        let tx_hash = Fr::from(67890u64);

        let nullifier = compute_nullifier(&priv_key, checkpoint_height, &tx_hash);
        assert_ne!(nullifier, Fr::from(0u64));

        let nullifier2 = compute_nullifier(&priv_key, checkpoint_height, &tx_hash);
        assert_eq!(nullifier, nullifier2);
    }

    #[test]
    fn test_amount_commitment() {
        let amount = Fr::from(1000u64);
        let blinding = Fr::from(99999u64);

        let commitment = compute_amount_commitment(&amount, &blinding);
        assert_ne!(commitment, Fr::from(0u64));
    }

    #[test]
    fn test_merkle_root_computation() {
        let tx_hash = Fr::from(12345u64);
        let path_elements = vec![Fr::from(1u64), Fr::from(2u64)];
        let path_indices = vec![0u8, 1u8];

        let root = compute_merkle_root(&tx_hash, &path_elements, &path_indices);
        assert_ne!(root, Fr::from(0u64));
    }

    #[test]
    fn test_zk_url_encoding_decoding() {
        let payload = ZkProofPayload {
            v: ZK_URL_VERSION,
            chain_id: CHAIN_ID_TESTNET.to_string(),
            cp_height: 100,
            proof: "test_proof".to_string(),
            public_inputs: ZkPublicInputs {
                cp_root: "0x1234".to_string(),
                nullifier: "0x5678".to_string(),
                amount_commitment: "0x9abc".to_string(),
                chain_id_hash: "0xdef0".to_string(),
            },
            encrypted_memo: None,
            aux_data: None,
        };

        let url = encode_zk_url(&payload).unwrap();
        assert!(url.starts_with("rinku://zk/"));

        let decoded = decode_zk_url(&url).unwrap();
        assert_eq!(decoded.v, payload.v);
        assert_eq!(decoded.chain_id, payload.chain_id);
        assert_eq!(decoded.cp_height, payload.cp_height);
    }

    #[tokio::test]
    async fn test_nullifier_registry() {
        let registry = NullifierRegistry::new();

        assert!(!registry.has("test_nullifier").await);
        assert_eq!(registry.size().await, 0);

        registry.add("test_nullifier".to_string()).await;
        assert!(registry.has("test_nullifier").await);
        assert_eq!(registry.size().await, 1);

        registry.remove("test_nullifier").await;
        assert!(!registry.has("test_nullifier").await);
        assert_eq!(registry.size().await, 0);
    }
}
