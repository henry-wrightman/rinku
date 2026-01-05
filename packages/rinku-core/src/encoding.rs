use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use flate2::{read::DeflateDecoder, write::DeflateEncoder, Compression};
use serde::{de::DeserializeOwned, Serialize};
use std::io::{Read, Write};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum EncodingError {
    #[error("JSON serialization failed: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("Base64 decoding failed: {0}")]
    Base64Error(#[from] base64::DecodeError),
    #[error("Compression failed: {0}")]
    CompressionError(#[from] std::io::Error),
    #[error("Invalid URL format")]
    InvalidUrlFormat,
}

pub fn encode_to_url<T: Serialize>(data: &T) -> Result<String, EncodingError> {
    let json = serde_json::to_string(data)?;
    let compressed = compress(&json)?;
    let encoded = URL_SAFE_NO_PAD.encode(&compressed);
    Ok(encoded)
}

pub fn decode_from_url<T: DeserializeOwned>(encoded: &str) -> Result<T, EncodingError> {
    let compressed = URL_SAFE_NO_PAD.decode(encoded)?;
    let json = decompress(&compressed)?;
    let data = serde_json::from_str(&json)?;
    Ok(data)
}

pub fn compress(data: &str) -> Result<Vec<u8>, EncodingError> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(data.as_bytes())?;
    Ok(encoder.finish()?)
}

pub fn decompress(data: &[u8]) -> Result<String, EncodingError> {
    let mut decoder = DeflateDecoder::new(data);
    let mut decompressed = String::new();
    decoder.read_to_string(&mut decompressed)?;
    Ok(decompressed)
}

pub fn create_transaction_url(base_url: &str, tx_payload: &str) -> String {
    format!("{}/tx/{}", base_url, tx_payload)
}

pub fn create_proof_url(base_url: &str, proof_payload: &str) -> String {
    format!("{}/txp/{}", base_url, proof_payload)
}

pub fn create_self_proof_url(proof_payload: &str) -> String {
    format!("rinku://sp/{}", proof_payload)
}

pub fn create_zk_proof_url(proof_payload: &str) -> String {
    format!("rinku://zk/{}", proof_payload)
}

pub fn parse_rinku_url(url: &str) -> Result<(String, String), EncodingError> {
    if url.starts_with("rinku://") {
        let rest = &url[8..];
        if let Some(slash_pos) = rest.find('/') {
            let url_type = &rest[..slash_pos];
            let payload = &rest[slash_pos + 1..];
            return Ok((url_type.to_string(), payload.to_string()));
        }
    }

    if url.contains("/tx/") {
        if let Some(pos) = url.find("/tx/") {
            let payload = &url[pos + 4..];
            return Ok(("tx".to_string(), payload.to_string()));
        }
    }

    if url.contains("/txp/") {
        if let Some(pos) = url.find("/txp/") {
            let payload = &url[pos + 5..];
            return Ok(("txp".to_string(), payload.to_string()));
        }
    }

    Err(EncodingError::InvalidUrlFormat)
}

pub fn base64url_encode(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

pub fn base64url_decode(encoded: &str) -> Result<Vec<u8>, EncodingError> {
    Ok(URL_SAFE_NO_PAD.decode(encoded)?)
}

pub fn hex_to_base64url(hex_str: &str) -> Result<String, EncodingError> {
    let bytes = hex::decode(hex_str).map_err(|_| EncodingError::InvalidUrlFormat)?;
    Ok(base64url_encode(&bytes))
}

pub fn base64url_to_hex(b64_str: &str) -> Result<String, EncodingError> {
    let bytes = base64url_decode(b64_str)?;
    Ok(hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Transaction;

    #[test]
    fn test_encode_decode_transaction() {
        let tx = Transaction {
            from: "sender123".to_string(),
            to: "receiver456".to_string(),
            amount: 100.5,
            nonce: 1,
            timestamp: 1234567890,
            parents: vec!["parent1".to_string()],
            kind: None,
            gas_limit: Some(21000),
            gas_price: Some(0.001),
            data: None,
            signature: None,
        };

        let encoded = encode_to_url(&tx).unwrap();
        let decoded: Transaction = decode_from_url(&encoded).unwrap();

        assert_eq!(decoded.from, tx.from);
        assert_eq!(decoded.to, tx.to);
        assert_eq!(decoded.amount, tx.amount);
        assert_eq!(decoded.nonce, tx.nonce);
    }

    #[test]
    fn test_compress_decompress() {
        let original = r#"{"test": "data", "number": 12345}"#;
        let compressed = compress(original).unwrap();
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, original);
    }

    #[test]
    fn test_parse_rinku_url() {
        let (url_type, payload) = parse_rinku_url("rinku://sp/abc123").unwrap();
        assert_eq!(url_type, "sp");
        assert_eq!(payload, "abc123");

        let (url_type, payload) = parse_rinku_url("https://example.com/tx/xyz789").unwrap();
        assert_eq!(url_type, "tx");
        assert_eq!(payload, "xyz789");
    }

    #[test]
    fn test_hex_base64_conversion() {
        let hex_str = "48656c6c6f";
        let b64 = hex_to_base64url(hex_str).unwrap();
        let back = base64url_to_hex(&b64).unwrap();
        assert_eq!(back, hex_str);
    }
}
