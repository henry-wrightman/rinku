use std::collections::BTreeMap;

use crate::crypto::{compute_fingerprint, hash, hash_transaction, hex_to_array, extract_private_key_scalar_from_pkcs8, sign, verify};
use crate::merkle::get_merkle_root;
use crate::types::{AccountState, Transaction};

#[cfg(test)]
mod cross_lang_tests {
    use super::*;

    #[test]
    fn test_hash_empty_matches_typescript() {
        let result = hash("");
        assert_eq!(
            result,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "Empty string hash must match TypeScript"
        );
    }

    #[test]
    fn test_hash_hello_rinku_matches_typescript() {
        let result = hash("Hello, Rinku!");
        assert_eq!(
            result,
            "e2d6c45f1ee517481b6adf19680e5a0c99df318402ad116edf203e2c4887310b",
            "Hello, Rinku! hash must match TypeScript"
        );
    }

    #[test]
    fn test_hash_test_matches_typescript() {
        let result = hash("test");
        assert_eq!(
            result,
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "'test' hash must match TypeScript"
        );
    }

    #[test]
    fn test_hash_abc_matches_typescript() {
        let result = hash("abc");
        assert_eq!(
            result,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "'abc' hash must match TypeScript"
        );
    }

    #[test]
    fn test_hash_json_matches_typescript() {
        let result = hash("{\"key\":\"value\"}");
        assert_eq!(
            result,
            "e43abcf3375244839c012f9633f95862d232a95b00d5bc7348b3098b9fed7f32",
            "JSON hash must match TypeScript"
        );
    }

    #[test]
    fn test_fingerprint_matches_typescript() {
        let public_key_hex = "04efc918f027ae293ce881dcb32ddea8eda78615cfd9f0a3f53fb30cca084720a58204f0da12b189401818390b3c967c1411a4443624c4ff75409fca2230af0a3f";
        let public_key = hex_to_array(public_key_hex);
        let result = compute_fingerprint(&public_key);
        assert_eq!(
            result,
            "1ed679715cd3069a1b17ae18dedc3d042a02d3c4",
            "Fingerprint must match TypeScript"
        );
    }

    #[test]
    fn test_merkle_single_account_matches_typescript() {
        let mut accounts = BTreeMap::new();
        accounts.insert(
            "account1".to_string(),
            AccountState {
                fingerprint: "account1".to_string(),
                balance: 1000.0,
                nonce: 1,
                first_tx_timestamp: 0,
            },
        );
        let root = get_merkle_root(&accounts);
        assert_eq!(
            root,
            "0e6f7b2ee76b4e8c1da2a7a10379d864cd8c7248a72ddf99f0f02b29d3888a09",
            "Single account Merkle root must match TypeScript"
        );
    }

    #[test]
    fn test_merkle_two_accounts_matches_typescript() {
        let mut accounts = BTreeMap::new();
        accounts.insert(
            "a".to_string(),
            AccountState {
                fingerprint: "a".to_string(),
                balance: 100.0,
                nonce: 1,
                first_tx_timestamp: 0,
            },
        );
        accounts.insert(
            "b".to_string(),
            AccountState {
                fingerprint: "b".to_string(),
                balance: 200.0,
                nonce: 2,
                first_tx_timestamp: 0,
            },
        );
        let root = get_merkle_root(&accounts);
        assert_eq!(
            root,
            "e77a703ade2a260105019cfca9c2d841893e1723185ad471dc76395ea334d703",
            "Two accounts Merkle root must match TypeScript"
        );
    }

    #[test]
    fn test_merkle_three_accounts_matches_typescript() {
        let mut accounts = BTreeMap::new();
        accounts.insert(
            "alice".to_string(),
            AccountState {
                fingerprint: "alice".to_string(),
                balance: 1000.0,
                nonce: 5,
                first_tx_timestamp: 0,
            },
        );
        accounts.insert(
            "bob".to_string(),
            AccountState {
                fingerprint: "bob".to_string(),
                balance: 2000.0,
                nonce: 3,
                first_tx_timestamp: 0,
            },
        );
        accounts.insert(
            "charlie".to_string(),
            AccountState {
                fingerprint: "charlie".to_string(),
                balance: 500.0,
                nonce: 1,
                first_tx_timestamp: 0,
            },
        );
        let root = get_merkle_root(&accounts);
        assert_eq!(
            root,
            "3381af47d74047ff64e508ac51baa69e853fe449a388587b461feda6cbf0ff53",
            "Three accounts Merkle root must match TypeScript"
        );
    }

    #[test]
    fn test_merkle_four_accounts_matches_typescript() {
        let mut accounts = BTreeMap::new();
        accounts.insert(
            "a".to_string(),
            AccountState {
                fingerprint: "a".to_string(),
                balance: 100.0,
                nonce: 1,
                first_tx_timestamp: 0,
            },
        );
        accounts.insert(
            "b".to_string(),
            AccountState {
                fingerprint: "b".to_string(),
                balance: 200.0,
                nonce: 2,
                first_tx_timestamp: 0,
            },
        );
        accounts.insert(
            "c".to_string(),
            AccountState {
                fingerprint: "c".to_string(),
                balance: 300.0,
                nonce: 3,
                first_tx_timestamp: 0,
            },
        );
        accounts.insert(
            "d".to_string(),
            AccountState {
                fingerprint: "d".to_string(),
                balance: 400.0,
                nonce: 4,
                first_tx_timestamp: 0,
            },
        );
        let root = get_merkle_root(&accounts);
        assert_eq!(
            root,
            "8b4d3df0e89c74c39d73e523af3af8c72b6528a55b9a889297a3bbb3e1279425",
            "Four accounts Merkle root must match TypeScript"
        );
    }

    #[test]
    fn test_leaf_hash_matches_typescript() {
        let leaf_hash = hash("account1:1000:1");
        assert_eq!(
            leaf_hash,
            "0e6f7b2ee76b4e8c1da2a7a10379d864cd8c7248a72ddf99f0f02b29d3888a09",
            "Leaf hash must match TypeScript"
        );
    }

    #[test]
    fn test_leaf_hash_a_matches_typescript() {
        let leaf_hash = hash("a:100:1");
        assert_eq!(
            leaf_hash,
            "f9b4f040d2353c5dde72aedc6b006807b5399d762b94f8bc51cc8682629dfa0e",
            "Leaf hash 'a:100:1' must match TypeScript"
        );
    }

    #[test]
    fn test_leaf_hash_b_matches_typescript() {
        let leaf_hash = hash("b:200:2");
        assert_eq!(
            leaf_hash,
            "776c7b1905dae9f4f34740ec5801bb13b0cdc7390458ab40b8ad15f191095801",
            "Leaf hash 'b:200:2' must match TypeScript"
        );
    }

    #[test]
    fn test_pkcs8_extraction() {
        let pkcs8_hex = "308187020100301306072a8648ce3d020106082a8648ce3d030107046d306b020101042004490ca38837f471986cfe3c6f33d649395fcefdd0c13f2fce3f6e11b650bf7da14403420004dc281510bb1d1cdf340c84aa9d3abef38fad2a76de52f16d3965365661014794ed785393e96950dfbfd42989f175f256c188a0914b27468269022cc6c3c0c5b4";
        let pkcs8_bytes = hex_to_array(pkcs8_hex);
        let scalar = extract_private_key_scalar_from_pkcs8(&pkcs8_bytes).expect("Should extract scalar");
        assert_eq!(scalar.len(), 32, "Scalar should be 32 bytes");
    }

    #[test]
    fn test_pkcs8_sign_verify() {
        let pkcs8_hex = "308187020100301306072a8648ce3d020106082a8648ce3d030107046d306b020101042004490ca38837f471986cfe3c6f33d649395fcefdd0c13f2fce3f6e11b650bf7da14403420004dc281510bb1d1cdf340c84aa9d3abef38fad2a76de52f16d3965365661014794ed785393e96950dfbfd42989f175f256c188a0914b27468269022cc6c3c0c5b4";
        let public_key_hex = "04dc281510bb1d1cdf340c84aa9d3abef38fad2a76de52f16d3965365661014794ed785393e96950dfbfd42989f175f256c188a0914b27468269022cc6c3c0c5b4";
        
        let pkcs8_bytes = hex_to_array(pkcs8_hex);
        let public_key = hex_to_array(public_key_hex);
        
        let scalar = extract_private_key_scalar_from_pkcs8(&pkcs8_bytes).expect("Should extract scalar");
        let message = "test message";
        let signature = sign(message, &scalar).expect("Should sign");
        let valid = verify(message, &signature, &public_key).expect("Should verify");
        
        assert!(valid, "Signature should be valid");
    }
}
