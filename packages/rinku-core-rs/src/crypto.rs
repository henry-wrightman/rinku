use p256::ecdsa::{signature::Signer, signature::Verifier, Signature, SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

use crate::types::{CryptoError, KeyPair, Result, Transaction};

pub fn hash(data: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data.as_bytes());
    let result = hasher.finalize();
    array_to_hex(&result)
}

pub fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    array_to_hex(&result)
}

pub fn compute_fingerprint(public_key: &[u8]) -> String {
    let hash = hash_bytes(public_key);
    hash[..40].to_string()
}

pub fn generate_keypair() -> Result<KeyPair> {
    let signing_key = SigningKey::random(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    let public_key_bytes = verifying_key.to_encoded_point(false);
    let public_key = public_key_bytes.as_bytes().to_vec();

    let private_key = signing_key.to_bytes().to_vec();

    let fingerprint = compute_fingerprint(&public_key);

    Ok(KeyPair {
        public_key,
        private_key,
        fingerprint,
    })
}

pub fn sign(message: &str, private_key: &[u8]) -> Result<String> {
    if private_key.len() != 32 {
        return Err(CryptoError::InvalidKeyLength {
            expected: 32,
            actual: private_key.len(),
        });
    }

    let signing_key = SigningKey::from_bytes(private_key.into())
        .map_err(|e| CryptoError::SigningError(e.to_string()))?;

    let signature: Signature = signing_key.sign(message.as_bytes());
    Ok(array_to_hex(&signature.to_bytes()))
}

pub fn verify(message: &str, signature_hex: &str, public_key: &[u8]) -> Result<bool> {
    let signature_bytes = hex_to_array(signature_hex);
    if signature_bytes.len() != 64 {
        return Ok(false);
    }

    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|e| CryptoError::VerificationError(e.to_string()))?;

    let verifying_key = VerifyingKey::from_sec1_bytes(public_key)
        .map_err(|e| CryptoError::InvalidPublicKey(e.to_string()))?;

    match verifying_key.verify(message.as_bytes(), &signature) {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

pub fn array_to_hex(arr: &[u8]) -> String {
    arr.iter().map(|b| format!("{:02x}", b)).collect()
}

pub fn hex_to_array(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .filter_map(|i| {
            if i + 2 <= hex.len() {
                u8::from_str_radix(&hex[i..i + 2], 16).ok()
            } else {
                None
            }
        })
        .collect()
}

pub fn hash_transaction(tx: &Transaction) -> String {
    let tx_data = serde_json::json!({
        "from": tx.from,
        "to": tx.to,
        "amount": tx.amount,
        "fee": tx.fee,
        "nonce": tx.nonce,
        "tipUrls": tx.tip_urls,
        "ts": tx.ts
    });
    hash(&tx_data.to_string())
}

pub fn extract_private_key_scalar_from_pkcs8(pkcs8_der: &[u8]) -> Result<Vec<u8>> {
    if pkcs8_der.len() < 138 {
        return Err(CryptoError::InvalidKeyLength {
            expected: 138,
            actual: pkcs8_der.len(),
        });
    }
    let scalar_offset = 36;
    let scalar_length = 32;
    if pkcs8_der.len() < scalar_offset + scalar_length {
        return Err(CryptoError::InvalidKeyLength {
            expected: scalar_offset + scalar_length,
            actual: pkcs8_der.len(),
        });
    }
    Ok(pkcs8_der[scalar_offset..scalar_offset + scalar_length].to_vec())
}

pub fn sign_with_pkcs8(message: &str, pkcs8_private_key: &[u8]) -> Result<String> {
    let scalar = extract_private_key_scalar_from_pkcs8(pkcs8_private_key)?;
    sign(message, &scalar)
}

#[cfg(test)]
mod crypto_tests {
    use super::*;

    #[test]
    fn test_hash_known_value() {
        let result = hash("");
        assert_eq!(
            result,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_array_to_hex() {
        assert_eq!(array_to_hex(&[0x00, 0x01, 0x0a, 0xff]), "00010aff");
        assert_eq!(array_to_hex(&[]), "");
    }

    #[test]
    fn test_hex_to_array() {
        assert_eq!(hex_to_array("00010aff"), vec![0x00, 0x01, 0x0a, 0xff]);
        assert_eq!(hex_to_array(""), Vec::<u8>::new());
    }

    #[test]
    fn test_fingerprint_length() {
        let kp = generate_keypair().unwrap();
        assert_eq!(kp.fingerprint.len(), 40);
    }

    #[test]
    fn test_fingerprint_deterministic() {
        let kp = generate_keypair().unwrap();
        let fp1 = compute_fingerprint(&kp.public_key);
        let fp2 = compute_fingerprint(&kp.public_key);
        assert_eq!(fp1, fp2);
    }
}
