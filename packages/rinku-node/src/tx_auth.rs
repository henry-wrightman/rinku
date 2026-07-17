//! User transaction authenticity (ECDSA P-256).
//!
//! Non-system txs must present a public key whose fingerprint equals `tx.from`,
//! and an ECDSA signature over the UTF-8 bytes of `tx.hash` (wallet-compatible).

use anyhow::{anyhow, Result};
use rinku_core::{fingerprint_from_public_key_hex, types::SignedTransaction, verify_tx_signature};

pub fn is_system_transaction(tx: &SignedTransaction) -> bool {
    matches!(
        tx.tx.kind,
        Some(rinku_core::types::TransactionKind::Consolidation)
    ) || tx.signature.starts_with("anchor-")
        || tx.tx.from == "faucet"
        || tx.tx.from == "genesis"
}

/// Normalize a public key provided as raw SEC1 bytes or hex string into lowercase hex.
pub fn normalize_public_key_hex(bytes: Option<&[u8]>, hex: Option<&str>) -> Result<Option<String>> {
    if let Some(b) = bytes {
        if !b.is_empty() {
            return Ok(Some(hex::encode(b)));
        }
    }
    if let Some(h) = hex {
        let trimmed = h.trim();
        if !trimmed.is_empty() {
            let cleaned = trimmed.strip_prefix("0x").unwrap_or(trimmed);
            // Validate hex
            hex::decode(cleaned).map_err(|_| anyhow!("Invalid publicKey hex"))?;
            return Ok(Some(cleaned.to_lowercase()));
        }
    }
    Ok(None)
}

/// Verify ECDSA authenticity for a user transaction.
///
/// `provided_pubkey_hex` comes from the submit/gossip payload (preferred on first sighting).
/// `known_pubkey_hex` is the key already bound to the account (if any).
///
/// Returns the canonical pubkey hex that should be bound to the account.
pub fn verify_user_tx_authenticity(
    tx: &SignedTransaction,
    provided_pubkey_hex: Option<&str>,
    known_pubkey_hex: Option<&str>,
) -> Result<String> {
    if is_system_transaction(tx) {
        return Err(anyhow!(
            "internal: verify_user_tx_authenticity called for system transaction"
        ));
    }

    if tx.signature.is_empty() || tx.signature.starts_with("faucet-") {
        return Err(anyhow!("Missing or invalid transaction signature"));
    }

    let pubkey = match (provided_pubkey_hex, known_pubkey_hex) {
        (Some(p), Some(k)) => {
            if p.to_lowercase() != k.to_lowercase() {
                return Err(anyhow!(
                    "publicKey does not match key already bound to this account"
                ));
            }
            p.to_string()
        }
        (Some(p), None) => p.to_string(),
        (None, Some(k)) => k.to_string(),
        (None, None) => {
            return Err(anyhow!("publicKey required for authenticated transactions"));
        }
    };

    let fingerprint = fingerprint_from_public_key_hex(&pubkey)
        .map_err(|e| anyhow!("Invalid publicKey: {}", e))?;

    if fingerprint != tx.tx.from {
        return Err(anyhow!(
            "publicKey fingerprint does not match transaction from address"
        ));
    }

    verify_tx_signature(&pubkey, &tx.hash, &tx.signature)
        .map_err(|_| anyhow!("Invalid transaction signature"))?;

    Ok(pubkey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rinku_core::{types::Transaction, KeyPair};

    fn signed_transfer(kp: &KeyPair, to: &str, amount: u64, nonce: u64) -> SignedTransaction {
        let mut tx = Transaction {
            from: kp.address(),
            to: to.to_string(),
            amount,
            nonce,
            timestamp: 1_700_000_000_000,
            parents: vec![],
            kind: None,
            gas_limit: None,
            gas_price: Some(1_000),
            data: None,
            signature: None,
            memo: None,
            references: None,
        };
        let hash = rinku_core::hash_transaction(&serde_json::to_string(&tx).unwrap());
        let signature = kp.sign(hash.as_bytes()).unwrap();
        tx.signature = Some(signature.clone());
        SignedTransaction {
            tx,
            hash,
            signature,
        }
    }

    #[test]
    fn valid_signature_accepted() {
        let kp = KeyPair::generate().unwrap();
        let tx = signed_transfer(&kp, "deadbeef", 1_000_000, 0);
        let pk = kp.public_key_hex();
        let bound = verify_user_tx_authenticity(&tx, Some(&pk), None).unwrap();
        assert_eq!(bound, pk);
    }

    #[test]
    fn wrong_key_rejected() {
        let kp = KeyPair::generate().unwrap();
        let other = KeyPair::generate().unwrap();
        let tx = signed_transfer(&kp, "deadbeef", 1_000_000, 0);
        let err =
            verify_user_tx_authenticity(&tx, Some(&other.public_key_hex()), None).unwrap_err();
        assert!(
            err.to_string().contains("fingerprint") || err.to_string().contains("signature"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn missing_pubkey_rejected() {
        let kp = KeyPair::generate().unwrap();
        let tx = signed_transfer(&kp, "deadbeef", 1_000_000, 0);
        let err = verify_user_tx_authenticity(&tx, None, None).unwrap_err();
        assert!(err.to_string().contains("publicKey required"));
    }

    #[test]
    fn forged_from_address_rejected() {
        let kp = KeyPair::generate().unwrap();
        let mut tx = signed_transfer(&kp, "deadbeef", 1_000_000, 0);
        tx.tx.from = "0000000000000000000000000000000000000000".to_string();
        let err = verify_user_tx_authenticity(&tx, Some(&kp.public_key_hex()), None).unwrap_err();
        assert!(err.to_string().contains("fingerprint"));
    }

    #[test]
    fn system_faucet_detected() {
        let tx = SignedTransaction {
            tx: Transaction {
                from: "faucet".to_string(),
                to: "abc".to_string(),
                amount: 1,
                nonce: 0,
                timestamp: 0,
                parents: vec![],
                kind: None,
                gas_limit: None,
                gas_price: Some(0),
                data: None,
                signature: None,
                memo: None,
                references: None,
            },
            hash: "x".to_string(),
            signature: "faucet-signature".to_string(),
        };
        assert!(is_system_transaction(&tx));
    }

    #[test]
    fn known_pubkey_allows_omitting_provided() {
        let kp = KeyPair::generate().unwrap();
        let tx = signed_transfer(&kp, "deadbeef", 1_000_000, 0);
        let pk = kp.public_key_hex();
        verify_user_tx_authenticity(&tx, None, Some(&pk)).unwrap();
    }
}
