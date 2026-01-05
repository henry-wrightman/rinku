use blst::min_pk::{AggregateSignature, PublicKey, SecretKey, Signature};
use blst::BLST_ERROR;
use rand::RngCore;
use sha2::{Digest, Sha256};

const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_NUL_";

#[derive(Debug, Clone)]
pub struct BLSKeyPair {
    pub public_key: Vec<u8>,
    pub private_key: Vec<u8>,
    pub fingerprint: String,
}

#[derive(Debug, Clone)]
pub struct AggregatedBLSSignature {
    pub signature: Vec<u8>,
    pub signer_bitmap: Vec<u8>,
    pub signer_count: usize,
}

pub fn generate_bls_keypair() -> BLSKeyPair {
    let mut ikm = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut ikm);

    let sk = SecretKey::key_gen(&ikm, &[]).expect("Failed to generate BLS secret key");
    let pk = sk.sk_to_pk();

    let pk_bytes = pk.compress().to_vec();
    let sk_bytes = sk.to_bytes().to_vec();
    let fingerprint = compute_bls_fingerprint(&pk_bytes);

    BLSKeyPair {
        public_key: pk_bytes,
        private_key: sk_bytes,
        fingerprint,
    }
}

pub fn compute_bls_fingerprint(public_key: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(public_key);
    let hash = hasher.finalize();
    hex::encode(&hash[..20])
}

pub fn bls_get_public_key(private_key: &[u8]) -> Result<Vec<u8>, String> {
    let sk = SecretKey::from_bytes(private_key).map_err(|e| format!("Invalid private key: {:?}", e))?;
    let pk = sk.sk_to_pk();
    Ok(pk.compress().to_vec())
}

pub fn bls_sign(message: &[u8], private_key: &[u8]) -> Result<Vec<u8>, String> {
    let sk = SecretKey::from_bytes(private_key).map_err(|e| format!("Invalid private key: {:?}", e))?;
    let sig = sk.sign(message, DST, &[]);
    Ok(sig.compress().to_vec())
}

pub fn bls_verify(message: &[u8], signature: &[u8], public_key: &[u8]) -> bool {
    let pk = match PublicKey::from_bytes(public_key) {
        Ok(pk) => pk,
        Err(_) => return false,
    };

    let sig = match Signature::from_bytes(signature) {
        Ok(sig) => sig,
        Err(_) => return false,
    };

    sig.verify(true, message, DST, &[], &pk, true) == BLST_ERROR::BLST_SUCCESS
}

pub fn aggregate_signatures(signatures: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if signatures.is_empty() {
        return Err("No signatures to aggregate".to_string());
    }

    let parsed_sigs: Result<Vec<Signature>, _> = signatures.iter().map(|s| Signature::from_bytes(s)).collect();

    let sigs = parsed_sigs.map_err(|e| format!("Invalid signature: {:?}", e))?;
    let sig_refs: Vec<&Signature> = sigs.iter().collect();

    let agg = AggregateSignature::aggregate(&sig_refs, true).map_err(|e| format!("Aggregation failed: {:?}", e))?;

    Ok(agg.to_signature().compress().to_vec())
}

pub fn aggregate_public_keys(public_keys: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if public_keys.is_empty() {
        return Err("No public keys to aggregate".to_string());
    }

    let parsed_pks: Result<Vec<PublicKey>, _> = public_keys.iter().map(|pk| PublicKey::from_bytes(pk)).collect();

    let pks = parsed_pks.map_err(|e| format!("Invalid public key: {:?}", e))?;

    let pk_refs: Vec<&PublicKey> = pks.iter().collect();
    let agg_pk = pk_refs
        .iter()
        .skip(1)
        .fold(pk_refs[0].clone(), |acc, pk| {
            let mut agg = blst::min_pk::AggregatePublicKey::from_public_key(&acc);
            agg.add_public_key(*pk, true).expect("Failed to add public key");
            agg.to_public_key()
        });

    Ok(agg_pk.compress().to_vec())
}

pub fn verify_aggregated_signature(message: &[u8], aggregated_sig: &[u8], public_keys: &[Vec<u8>]) -> bool {
    let agg_pk = match aggregate_public_keys(public_keys) {
        Ok(pk) => pk,
        Err(_) => return false,
    };

    bls_verify(message, aggregated_sig, &agg_pk)
}

pub fn create_signer_bitmap(signer_indices: &[usize], total_validators: usize) -> Vec<u8> {
    let byte_count = (total_validators + 7) / 8;
    let mut bitmap = vec![0u8; byte_count];

    for &idx in signer_indices {
        if idx < total_validators {
            let byte_idx = idx / 8;
            let bit_idx = idx % 8;
            bitmap[byte_idx] |= 1 << bit_idx;
        }
    }

    bitmap
}

pub fn parse_signer_bitmap(bitmap: &[u8], total_validators: usize) -> Vec<usize> {
    let mut indices = Vec::new();

    for i in 0..total_validators {
        let byte_idx = i / 8;
        let bit_idx = i % 8;

        if byte_idx < bitmap.len() && (bitmap[byte_idx] & (1 << bit_idx)) != 0 {
            indices.push(i);
        }
    }

    indices
}

pub fn create_aggregated_checkpoint_signature(
    checkpoint_hash: &[u8],
    validators: &[(usize, Vec<u8>)],
) -> Result<AggregatedBLSSignature, String> {
    if validators.is_empty() {
        return Err("No validators to sign".to_string());
    }

    let mut signatures = Vec::new();
    let mut signer_indices = Vec::new();

    for (index, private_key) in validators {
        let sig = bls_sign(checkpoint_hash, private_key)?;
        signatures.push(sig);
        signer_indices.push(*index);
    }

    let aggregated_sig = aggregate_signatures(&signatures)?;
    let max_index = *signer_indices.iter().max().unwrap() + 1;
    let signer_bitmap = create_signer_bitmap(&signer_indices, max_index);

    Ok(AggregatedBLSSignature {
        signature: aggregated_sig,
        signer_bitmap,
        signer_count: validators.len(),
    })
}

pub fn verify_aggregated_checkpoint_signature(
    checkpoint_hash: &[u8],
    aggregated_sig: &[u8],
    signer_bitmap: &[u8],
    validator_public_keys: &[Vec<u8>],
) -> bool {
    let signer_indices = parse_signer_bitmap(signer_bitmap, validator_public_keys.len());

    if signer_indices.is_empty() {
        return false;
    }

    let signer_pub_keys: Vec<Vec<u8>> = signer_indices
        .iter()
        .filter_map(|&i| validator_public_keys.get(i).cloned())
        .collect();

    verify_aggregated_signature(checkpoint_hash, aggregated_sig, &signer_pub_keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_generation() {
        let keypair = generate_bls_keypair();
        assert_eq!(keypair.public_key.len(), 48);
        assert_eq!(keypair.private_key.len(), 32);
        assert_eq!(keypair.fingerprint.len(), 40);
    }

    #[test]
    fn test_sign_and_verify() {
        let keypair = generate_bls_keypair();
        let message = b"Hello, Rinku!";

        let signature = bls_sign(message, &keypair.private_key).unwrap();
        assert!(bls_verify(message, &signature, &keypair.public_key));

        let wrong_message = b"Wrong message";
        assert!(!bls_verify(wrong_message, &signature, &keypair.public_key));
    }

    #[test]
    fn test_signature_aggregation() {
        let keypair1 = generate_bls_keypair();
        let keypair2 = generate_bls_keypair();
        let keypair3 = generate_bls_keypair();

        let message = b"Checkpoint data";

        let sig1 = bls_sign(message, &keypair1.private_key).unwrap();
        let sig2 = bls_sign(message, &keypair2.private_key).unwrap();
        let sig3 = bls_sign(message, &keypair3.private_key).unwrap();

        let agg_sig = aggregate_signatures(&[sig1, sig2, sig3]).unwrap();

        let pub_keys = vec![
            keypair1.public_key.clone(),
            keypair2.public_key.clone(),
            keypair3.public_key.clone(),
        ];

        assert!(verify_aggregated_signature(message, &agg_sig, &pub_keys));
    }

    #[test]
    fn test_signer_bitmap() {
        let indices = vec![0, 2, 5, 7];
        let total = 10;

        let bitmap = create_signer_bitmap(&indices, total);
        let parsed = parse_signer_bitmap(&bitmap, total);

        assert_eq!(parsed, indices);
    }
}
