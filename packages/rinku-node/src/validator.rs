use anyhow::Result;
use rinku_core::crypto::KeyPair;
use std::path::Path;
use thiserror::Error;
use tracing::info;

#[derive(Error, Debug)]
pub enum ValidatorError {
    #[error("Key not found")]
    KeyNotFound,
    #[error("Invalid password")]
    InvalidPassword,
    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

pub struct ValidatorKeyManager {
    keypair: Option<KeyPair>,
    data_dir: String,
}

impl ValidatorKeyManager {
    pub fn new(data_dir: &str) -> Self {
        Self {
            keypair: None,
            data_dir: data_dir.to_string(),
        }
    }

    pub fn generate_key(&mut self) -> Result<String, ValidatorError> {
        let keypair =
            KeyPair::generate().map_err(|e| ValidatorError::EncryptionFailed(e.to_string()))?;
        let address = keypair.address();
        self.keypair = Some(keypair);
        info!("Generated new validator key: {}...", &address);
        Ok(address)
    }

    pub fn load_or_generate(&mut self, password: &str) -> Result<String, ValidatorError> {
        let key_path = Path::new(&self.data_dir).join("validator.key");

        if key_path.exists() {
            self.load_key(password)
        } else {
            let address = self.generate_key()?;
            self.save_key(password)?;
            Ok(address)
        }
    }

    pub fn load_key(&mut self, _password: &str) -> Result<String, ValidatorError> {
        let key_path = Path::new(&self.data_dir).join("validator.key");
        let key_hex = std::fs::read_to_string(key_path)?;
        let keypair = KeyPair::from_private_key_hex(key_hex.trim())
            .map_err(|e| ValidatorError::EncryptionFailed(e.to_string()))?;
        let address = keypair.address();
        self.keypair = Some(keypair);
        info!("Loaded validator key: {}...", &address[..16]);
        Ok(address)
    }

    pub fn save_key(&self, _password: &str) -> Result<(), ValidatorError> {
        let keypair = self.keypair.as_ref().ok_or(ValidatorError::KeyNotFound)?;
        let key_path = Path::new(&self.data_dir).join("validator.key");
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::write(key_path, keypair.private_key_hex())?;
        Ok(())
    }

    pub fn address(&self) -> Option<String> {
        self.keypair.as_ref().map(|k| k.address())
    }

    pub fn sign(&self, message: &[u8]) -> Result<String, ValidatorError> {
        let keypair = self.keypair.as_ref().ok_or(ValidatorError::KeyNotFound)?;
        keypair
            .sign(message)
            .map_err(|e| ValidatorError::EncryptionFailed(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_generate_key() {
        let mut manager = ValidatorKeyManager::new("/tmp/test-validator");
        let address = manager.generate_key().unwrap();
        assert_eq!(address.len(), 40);
    }

    #[test]
    fn test_sign_message() {
        let mut manager = ValidatorKeyManager::new("/tmp/test-validator");
        manager.generate_key().unwrap();
        let signature = manager.sign(b"test message").unwrap();
        assert!(!signature.is_empty());
    }
}
