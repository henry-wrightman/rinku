use anyhow::Result;
use rinku_core::crypto::{KeyPair, encrypt_private_key_hex, decrypt_private_key_hex, parse_encrypted_private_key};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::info;

#[derive(Error, Debug)]
pub enum ValidatorError {
    #[error("Key not found")]
    KeyNotFound,
    #[error("Invalid password")]
    InvalidPassword,
    #[error("Insecure key file permissions")]
    InsecurePermissions,
    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

pub struct ValidatorKeyManager {
    keypair: Option<KeyPair>,
    data_dir: String,
    key_path: PathBuf,
}

impl ValidatorKeyManager {
    pub fn new(data_dir: &str) -> Self {
        let key_path = Path::new(data_dir).join("validator.key");
        Self {
            keypair: None,
            data_dir: data_dir.to_string(),
            key_path,
        }
    }

    pub fn set_key_path(&mut self, path: PathBuf) {
        self.key_path = path;
    }

    pub fn generate_key(&mut self) -> Result<String, ValidatorError> {
        let keypair =
            KeyPair::generate().map_err(|e| ValidatorError::EncryptionFailed(e.to_string()))?;
        let address = keypair.address();
        self.keypair = Some(keypair);
        info!("Generated new validator key: {}...", &address[..16]);
        Ok(address)
    }

    pub fn load_or_generate(&mut self, password: &str) -> Result<String, ValidatorError> {
        if password.is_empty() {
            return Err(ValidatorError::InvalidPassword);
        }
        if self.key_path.exists() {
            self.load_key(password)
        } else {
            let address = self.generate_key()?;
            self.save_key(password)?;
            Ok(address)
        }
    }

    pub fn load_key(&mut self, password: &str) -> Result<String, ValidatorError> {
        let key_data = std::fs::read_to_string(&self.key_path)?;
        let trimmed = key_data.trim();
        let key_hex = if parse_encrypted_private_key(trimmed).is_some() {
            let decrypted = decrypt_private_key_hex(trimmed, password)
                .map_err(|e| ValidatorError::EncryptionFailed(e.to_string()))?;
            decrypted
        } else {
            trimmed.to_string()
        };
        let keypair = KeyPair::from_private_key_hex(key_hex.trim())
            .map_err(|e| ValidatorError::EncryptionFailed(e.to_string()))?;
        let address = keypair.address();
        self.keypair = Some(keypair);
        info!("Loaded validator key: {}...", &address[..16]);
        if parse_encrypted_private_key(trimmed).is_none() && !password.is_empty() {
            let _ = self.save_key(password);
        }
        Ok(address)
    }

    pub fn save_key(&self, password: &str) -> Result<(), ValidatorError> {
        if password.is_empty() {
            return Err(ValidatorError::InvalidPassword);
        }
        let keypair = self.keypair.as_ref().ok_or(ValidatorError::KeyNotFound)?;
        std::fs::create_dir_all(&self.data_dir)?;
        let encrypted = encrypt_private_key_hex(&keypair.private_key_hex(), password)
            .map_err(|e| ValidatorError::EncryptionFailed(e.to_string()))?;
        std::fs::write(&self.key_path, encrypted)?;
        Ok(())
    }

    pub fn load_from_hex(&mut self, key_hex: &str) -> Result<String, ValidatorError> {
        let keypair = KeyPair::from_private_key_hex(key_hex.trim())
            .map_err(|e| ValidatorError::EncryptionFailed(e.to_string()))?;
        let address = keypair.address();
        self.keypair = Some(keypair);
        info!("Loaded validator key from hex: {}...", &address[..16]);
        Ok(address)
    }

    pub fn validate_key_permissions(&self) -> Result<(), ValidatorError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = std::fs::metadata(&self.key_path)?;
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(ValidatorError::InsecurePermissions);
            }
        }
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
    use tempfile::tempdir;

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

    #[test]
    fn test_load_from_hex() {
        let keypair = KeyPair::generate().unwrap();
        let key_hex = keypair.private_key_hex();
        let mut manager = ValidatorKeyManager::new("/tmp/test-validator");
        let address = manager.load_from_hex(&key_hex).unwrap();
        assert_eq!(address, keypair.address());
    }

    #[test]
    fn test_encrypted_key_storage_roundtrip() {
        let dir = tempdir().unwrap();
        let data_dir = dir.path().to_str().unwrap();
        let mut manager = ValidatorKeyManager::new(data_dir);
        let address = manager.generate_key().unwrap();
        manager.save_key("test-password").unwrap();

        let key_path = dir.path().join("validator.key");
        let stored = fs::read_to_string(key_path).unwrap();
        assert!(stored.trim_start().starts_with('{'));

        let mut manager2 = ValidatorKeyManager::new(data_dir);
        let loaded = manager2.load_key("test-password").unwrap();
        assert_eq!(address, loaded);
    }
}
