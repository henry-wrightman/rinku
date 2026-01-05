pub mod crypto;
pub mod merkle;
pub mod types;

#[cfg(test)]
mod cross_lang_tests;

pub use crypto::*;
pub use merkle::*;
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_empty_string() {
        let result = hash("");
        assert_eq!(
            result,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_hash_deterministic() {
        let hash1 = hash("Hello, Rinku!");
        let hash2 = hash("Hello, Rinku!");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_different_inputs() {
        let hash1 = hash("input1");
        let hash2 = hash("input2");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_hex_roundtrip() {
        let original = vec![0x00, 0x01, 0x0a, 0xff];
        let hex_str = array_to_hex(&original);
        let restored = hex_to_array(&hex_str);
        assert_eq!(original, restored);
    }

    #[test]
    fn test_keypair_generation() {
        let kp = generate_keypair().expect("Failed to generate keypair");
        assert_eq!(kp.public_key.len(), 65);
        assert_eq!(kp.fingerprint.len(), 40);
    }

    #[test]
    fn test_sign_verify() {
        let kp = generate_keypair().expect("Failed to generate keypair");
        let message = "Hello, Rinku!";
        let signature = sign(message, &kp.private_key).expect("Failed to sign");
        let valid = verify(message, &signature, &kp.public_key).expect("Failed to verify");
        assert!(valid);
    }

    #[test]
    fn test_sign_verify_wrong_key() {
        let kp1 = generate_keypair().expect("Failed to generate keypair");
        let kp2 = generate_keypair().expect("Failed to generate keypair");
        let message = "Hello, Rinku!";
        let signature = sign(message, &kp1.private_key).expect("Failed to sign");
        let valid = verify(message, &signature, &kp2.public_key).unwrap_or(false);
        assert!(!valid);
    }

    #[test]
    fn test_sign_verify_tampered_message() {
        let kp = generate_keypair().expect("Failed to generate keypair");
        let message = "Hello, Rinku!";
        let signature = sign(message, &kp.private_key).expect("Failed to sign");
        let valid = verify("Tampered message", &signature, &kp.public_key).unwrap_or(false);
        assert!(!valid);
    }
}
