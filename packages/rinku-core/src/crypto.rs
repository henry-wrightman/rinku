use p256::{
    ecdsa::{
        signature::{Signer, Verifier},
        Signature, SigningKey, VerifyingKey,
    },
    SecretKey,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("Invalid private key")]
    InvalidPrivateKey,
    #[error("Invalid public key")]
    InvalidPublicKey,
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Signature verification failed")]
    VerificationFailed,
    #[error("Key generation failed: {0}")]
    KeyGenerationFailed(String),
}

#[derive(Clone)]
pub struct KeyPair {
    signing_key: SigningKey,
    verifying_key: VerifyingKey,
}

impl KeyPair {
    pub fn generate() -> Result<Self, CryptoError> {
        let signing_key = SigningKey::random(&mut rand::thread_rng());
        let verifying_key = *signing_key.verifying_key();
        Ok(Self {
            signing_key,
            verifying_key,
        })
    }

    pub fn from_private_key_hex(hex_key: &str) -> Result<Self, CryptoError> {
        let bytes = hex::decode(hex_key).map_err(|_| CryptoError::InvalidPrivateKey)?;
        let secret_key =
            SecretKey::from_slice(&bytes).map_err(|_| CryptoError::InvalidPrivateKey)?;
        let signing_key = SigningKey::from(secret_key);
        let verifying_key = *signing_key.verifying_key();
        Ok(Self {
            signing_key,
            verifying_key,
        })
    }

    pub fn private_key_hex(&self) -> String {
        hex::encode(self.signing_key.to_bytes())
    }

    pub fn public_key_hex(&self) -> String {
        let point = self.verifying_key.to_encoded_point(false);
        hex::encode(point.as_bytes())
    }

    pub fn address(&self) -> String {
        let pubkey_hex = self.public_key_hex();
        let hash = sha256_hex(&pubkey_hex);
        hash[..40].to_string()
    }

    pub fn sign(&self, message: &[u8]) -> Result<String, CryptoError> {
        let signature: Signature = self.signing_key.sign(message);
        Ok(hex::encode(signature.to_bytes()))
    }

    pub fn sign_hex(&self, hex_message: &str) -> Result<String, CryptoError> {
        let bytes = hex::decode(hex_message).map_err(|_| CryptoError::InvalidSignature)?;
        self.sign(&bytes)
    }
}

pub fn verify_signature(
    public_key_hex: &str,
    message: &[u8],
    signature_hex: &str,
) -> Result<bool, CryptoError> {
    let pubkey_bytes = hex::decode(public_key_hex).map_err(|_| CryptoError::InvalidPublicKey)?;
    let verifying_key = VerifyingKey::from_sec1_bytes(&pubkey_bytes)
        .map_err(|_| CryptoError::InvalidPublicKey)?;

    let sig_bytes = hex::decode(signature_hex).map_err(|_| CryptoError::InvalidSignature)?;
    let signature =
        Signature::from_slice(&sig_bytes).map_err(|_| CryptoError::InvalidSignature)?;

    Ok(verifying_key.verify(message, &signature).is_ok())
}

pub fn verify_signature_hex(
    public_key_hex: &str,
    message_hex: &str,
    signature_hex: &str,
) -> Result<bool, CryptoError> {
    let message = hex::decode(message_hex).map_err(|_| CryptoError::InvalidSignature)?;
    verify_signature(public_key_hex, &message, signature_hex)
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

pub fn sha256_hex(data: &str) -> String {
    hex::encode(sha256(data.as_bytes()))
}

pub fn double_sha256(data: &[u8]) -> [u8; 32] {
    sha256(&sha256(data))
}

pub fn hash_transaction(tx_json: &str) -> String {
    sha256_hex(tx_json)
}

pub fn recover_address_from_signature(
    message_hash_hex: &str,
    signature_hex: &str,
) -> Result<String, CryptoError> {
    let _msg_bytes = hex::decode(message_hash_hex).map_err(|_| CryptoError::InvalidSignature)?;
    let _sig_bytes = hex::decode(signature_hex).map_err(|_| CryptoError::InvalidSignature)?;
    
    Err(CryptoError::KeyGenerationFailed(
        "ECDSA recovery not implemented - use public key verification".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_generation() {
        let keypair = KeyPair::generate().unwrap();
        assert!(!keypair.public_key_hex().is_empty());
        assert!(!keypair.private_key_hex().is_empty());
        assert_eq!(keypair.address().len(), 40);
    }

    #[test]
    fn test_sign_and_verify() {
        let keypair = KeyPair::generate().unwrap();
        let message = b"test message";
        let signature = keypair.sign(message).unwrap();

        let result = verify_signature(&keypair.public_key_hex(), message, &signature).unwrap();
        assert!(result);
    }

    #[test]
    fn test_sha256() {
        let hash = sha256_hex("hello");
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_keypair_from_private_key() {
        let keypair1 = KeyPair::generate().unwrap();
        let private_hex = keypair1.private_key_hex();

        let keypair2 = KeyPair::from_private_key_hex(&private_hex).unwrap();
        assert_eq!(keypair1.public_key_hex(), keypair2.public_key_hex());
        assert_eq!(keypair1.address(), keypair2.address());
    }
}
