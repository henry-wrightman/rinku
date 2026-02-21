use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use sha2::Digest;
use tracing::debug;

pub fn verify_intent_signature_with_pubkey(
    canonical_message: &str,
    signature_hex: &str,
    signer_address: &str,
    public_key_hex: &str,
) -> Result<(), String> {
    let pubkey_bytes = hex::decode(public_key_hex)
        .map_err(|e| format!("Invalid public key hex: {}", e))?;

    let fingerprint = hex::encode(&sha2::Sha256::digest(&pubkey_bytes)[..20]);
    if fingerprint != signer_address {
        return Err(format!(
            "Public key fingerprint {} does not match signer address {}",
            &fingerprint[..16], &signer_address[..16.min(signer_address.len())]
        ));
    }

    let sig_bytes = hex::decode(signature_hex)
        .map_err(|e| format!("Invalid signature hex: {}", e))?;

    let signature = parse_p256_signature(&sig_bytes)?;

    let verifying_key = VerifyingKey::from_sec1_bytes(&pubkey_bytes)
        .map_err(|e| format!("Invalid ECDSA public key: {}", e))?;

    verifying_key.verify(canonical_message.as_bytes(), &signature)
        .map_err(|e| {
            debug!(
                "ECDSA verification failed for {}: {}",
                &signer_address[..16.min(signer_address.len())],
                e
            );
            format!("Signature verification failed: {}", e)
        })?;

    Ok(())
}

fn parse_p256_signature(sig_bytes: &[u8]) -> Result<Signature, String> {
    if let Ok(sig) = Signature::from_der(sig_bytes) {
        return Ok(sig);
    }

    if sig_bytes.len() == 64 {
        let mut r_bytes = [0u8; 32];
        let mut s_bytes = [0u8; 32];
        r_bytes.copy_from_slice(&sig_bytes[0..32]);
        s_bytes.copy_from_slice(&sig_bytes[32..64]);
        return Signature::from_scalars(r_bytes, s_bytes)
            .map_err(|e| format!("Invalid raw signature: {}", e));
    }

    Err(format!("Cannot parse signature ({} bytes): not DER or raw r||s format", sig_bytes.len()))
}
