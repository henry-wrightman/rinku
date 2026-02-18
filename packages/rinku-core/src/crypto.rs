use p256::{
    ecdsa::{
        signature::{Signer, Verifier},
        Signature, SigningKey, VerifyingKey,
    },
    SecretKey,
};
use sha2::{Digest, Sha256};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use argon2::Argon2;
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use chacha20poly1305::aead::{Aead, KeyInit};
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
    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),
    #[error("Decryption failed: {0}")]
    DecryptionFailed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedPrivateKey {
    pub version: u8,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
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

    pub fn private_key_pkcs8_der_hex(&self) -> String {
        let priv_bytes = self.signing_key.to_bytes();
        let pub_point = self.verifying_key.to_encoded_point(false);
        let pub_bytes = pub_point.as_bytes();

        let mut der = Vec::with_capacity(138);
        der.extend_from_slice(&[0x30, 0x81, 0x87]);
        der.extend_from_slice(&[0x02, 0x01, 0x00]);
        der.extend_from_slice(&[0x30, 0x13]);
        der.extend_from_slice(&[0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01]);
        der.extend_from_slice(&[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07]);
        der.extend_from_slice(&[0x04, 0x6d]);
        der.extend_from_slice(&[0x30, 0x6b]);
        der.extend_from_slice(&[0x02, 0x01, 0x01]);
        der.extend_from_slice(&[0x04, 0x20]);
        der.extend_from_slice(&priv_bytes);
        der.extend_from_slice(&[0xa1, 0x44, 0x03, 0x42, 0x00]);
        der.extend_from_slice(pub_bytes);

        hex::encode(der)
    }

    pub fn fingerprint(&self) -> String {
        let pub_point = self.verifying_key.to_encoded_point(false);
        let pub_bytes = pub_point.as_bytes();
        let hash = sha256(pub_bytes);
        hex::encode(hash)[..40].to_string()
    }

    pub fn wallet_json(&self) -> String {
        serde_json::json!({
            "publicKey": self.public_key_hex(),
            "privateKey": self.private_key_pkcs8_der_hex(),
            "fingerprint": self.fingerprint()
        }).to_string()
    }

    pub fn from_pkcs8_der_hex(hex_key: &str) -> Result<Self, CryptoError> {
        let bytes = hex::decode(hex_key).map_err(|_| CryptoError::InvalidPrivateKey)?;
        if bytes.len() < 68 || bytes[0] != 0x30 {
            return Err(CryptoError::InvalidPrivateKey);
        }
        if bytes[34] != 0x04 || bytes[35] != 0x20 {
            return Err(CryptoError::InvalidPrivateKey);
        }
        let raw_key = &bytes[36..68];
        let secret_key = SecretKey::from_slice(raw_key).map_err(|_| CryptoError::InvalidPrivateKey)?;
        let signing_key = SigningKey::from(secret_key);
        let verifying_key = *signing_key.verifying_key();
        Ok(Self { signing_key, verifying_key })
    }

    pub fn from_wallet_json(json: &str) -> Result<Self, CryptoError> {
        let parsed: serde_json::Value = serde_json::from_str(json).map_err(|_| CryptoError::InvalidPrivateKey)?;
        if let Some(pk) = parsed.get("privateKey").and_then(|v| v.as_str()) {
            return Self::from_pkcs8_der_hex(pk);
        }
        Err(CryptoError::InvalidPrivateKey)
    }

    pub fn from_any_key_format(input: &str) -> Result<Self, CryptoError> {
        let trimmed = input.trim();
        if trimmed.starts_with('{') {
            return Self::from_wallet_json(trimmed);
        }
        if trimmed.len() == 64 {
            return Self::from_private_key_hex(trimmed);
        }
        if trimmed.len() > 64 {
            return Self::from_pkcs8_der_hex(trimmed);
        }
        Self::from_private_key_hex(trimmed)
    }

    pub fn public_key_hex(&self) -> String {
        let point = self.verifying_key.to_encoded_point(false);
        hex::encode(point.as_bytes())
    }

    pub fn address(&self) -> String {
        self.fingerprint()
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

fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32], CryptoError> {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;
    Ok(key)
}

pub fn encrypt_private_key_hex(private_key_hex: &str, password: &str) -> Result<String, CryptoError> {
    if password.is_empty() {
        return Err(CryptoError::EncryptionFailed("Empty password".to_string()));
    }
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let key = derive_key(password, &salt)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));

    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, private_key_hex.as_bytes())
        .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))?;

    let payload = EncryptedPrivateKey {
        version: 1,
        salt: URL_SAFE_NO_PAD.encode(salt),
        nonce: URL_SAFE_NO_PAD.encode(nonce_bytes),
        ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
    };

    serde_json::to_string(&payload)
        .map_err(|e| CryptoError::EncryptionFailed(e.to_string()))
}

pub fn decrypt_private_key_hex(encrypted: &str, password: &str) -> Result<String, CryptoError> {
    if password.is_empty() {
        return Err(CryptoError::DecryptionFailed("Empty password".to_string()));
    }
    let payload: EncryptedPrivateKey = serde_json::from_str(encrypted)
        .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))?;

    let salt = URL_SAFE_NO_PAD
        .decode(payload.salt)
        .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))?;
    let nonce = URL_SAFE_NO_PAD
        .decode(payload.nonce)
        .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))?;
    let ciphertext = URL_SAFE_NO_PAD
        .decode(payload.ciphertext)
        .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))?;

    let key = derive_key(password, &salt)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))?;

    String::from_utf8(plaintext)
        .map_err(|e| CryptoError::DecryptionFailed(e.to_string()))
}

pub fn parse_encrypted_private_key(data: &str) -> Option<EncryptedPrivateKey> {
    serde_json::from_str(data).ok()
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
    fn test_encrypt_decrypt_private_key() {
        let keypair = KeyPair::generate().unwrap();
        let private_hex = keypair.private_key_hex();
        let encrypted = encrypt_private_key_hex(&private_hex, "test-password").unwrap();
        let decrypted = decrypt_private_key_hex(&encrypted, "test-password").unwrap();
        assert_eq!(private_hex, decrypted);
    }

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
