//! All cryptography and canonical-encoding helpers shared by the chain.
//!
//! The module is split into three groups:
//!
//! 1. **Hashing** — [`blake3_hex`], [`hash_serializable`], [`merkle_root_hex`]. Every on-chain
//!    identifier (wallet/operator/validator IDs, transaction IDs, PoH hashes, snapshot
//!    anchors) ultimately funnels through these so we have a single place to audit.
//! 2. **Encoding** — [`decode_bytes`], [`normalize_bytes_to_hex`], [`fixed_32`], [`fixed_64`].
//!    Inputs from JSON/HTTP can arrive as either hex or base64; these helpers accept both and
//!    enforce length so the rest of the codebase never has to parse keys defensively.
//! 3. **Signing/verification** — [`sign_with_secret_hex`], [`verify_ed25519`],
//!    [`canonical_request_message`], [`tx_signing_message`]. The two `message` helpers define
//!    the *exact* byte sequence operator requests and user transactions sign over, so the CLI
//!    `sign-request` subcommand and the REST gateway can never disagree.
//!
//! There is no global state — every function is pure modulo `now_ms()`, which is the only
//! reason this crate isn't `no_std`-friendly.

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use ed25519_dalek::{Signature as DalekSignature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

pub const ZERO_HASH_HEX: &str = "0000000000000000000000000000000000000000000000000000000000000000";

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn blake3_hex(bytes: impl AsRef<[u8]>) -> String {
    blake3::hash(bytes.as_ref()).to_hex().to_string()
}

pub fn hash_serializable<T: Serialize>(value: &T) -> Result<String> {
    let bytes = serde_json::to_vec(value).context("canonical JSON serialization failed")?;
    Ok(blake3_hex(bytes))
}

pub fn decode_bytes(input: &str) -> Result<Vec<u8>> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("empty bytes"));
    }
    if trimmed.len() % 2 == 0 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return hex::decode(trimmed).context("hex decode failed");
    }
    general_purpose::STANDARD
        .decode(trimmed)
        .or_else(|_| general_purpose::URL_SAFE_NO_PAD.decode(trimmed))
        .context("base64 decode failed")
}

pub fn normalize_bytes_to_hex(input: &str, expected_len: usize) -> Result<String> {
    let bytes = decode_bytes(input)?;
    if bytes.len() != expected_len {
        return Err(anyhow!("expected {expected_len} bytes, got {}", bytes.len()));
    }
    Ok(hex::encode(bytes))
}

pub fn fixed_32(input: &str) -> Result<[u8; 32]> {
    let bytes = decode_bytes(input)?;
    bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| anyhow!("expected 32 bytes, got {}", bytes.len()))
}

pub fn fixed_64(input: &str) -> Result<[u8; 64]> {
    let bytes = decode_bytes(input)?;
    bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| anyhow!("expected 64 bytes, got {}", bytes.len()))
}

pub fn public_key_from_secret_hex(secret: &str) -> Result<String> {
    let secret_bytes = fixed_32(secret)?;
    let signing_key = SigningKey::from_bytes(&secret_bytes);
    Ok(hex::encode(signing_key.verifying_key().to_bytes()))
}

pub fn sign_with_secret_hex(secret: &str, message: &[u8]) -> Result<String> {
    let secret_bytes = fixed_32(secret)?;
    let signing_key = SigningKey::from_bytes(&secret_bytes);
    let sig = signing_key.sign(message);
    Ok(hex::encode(sig.to_bytes()))
}

pub fn verify_ed25519(public_key: &str, message: &[u8], signature: &str) -> Result<bool> {
    let public_bytes = fixed_32(public_key)?;
    let sig_bytes = fixed_64(signature)?;
    let verifying_key = VerifyingKey::from_bytes(&public_bytes).context("invalid Ed25519 public key")?;
    let signature = DalekSignature::from_bytes(&sig_bytes);
    Ok(verifying_key.verify(message, &signature).is_ok())
}

pub fn wallet_id_from_public_key(public_key: &str) -> Result<String> {
    let pubkey_hex = normalize_bytes_to_hex(public_key, 32)?;
    let pubkey_bytes = hex::decode(pubkey_hex)?;
    Ok(blake3_hex(pubkey_bytes))
}

pub fn operator_id_from_public_key(public_key: &str) -> Result<String> {
    wallet_id_from_public_key(public_key)
}

pub fn validator_id_from_public_key(public_key: &str) -> Result<String> {
    wallet_id_from_public_key(public_key)
}

pub fn tx_signing_message(tx_id_hex: &str) -> Result<Vec<u8>> {
    let bytes = decode_bytes(tx_id_hex)?;
    if bytes.len() != 32 {
        return Err(anyhow!("tx id must be 32 bytes"));
    }
    Ok(bytes)
}

pub fn canonical_request_message(method: &str, path: &str, timestamp_ms: i64, body: &[u8]) -> Vec<u8> {
    let mut message = format!("{}\n{}\n{}\n", method.to_ascii_uppercase(), path, timestamp_ms).into_bytes();
    message.extend_from_slice(body);
    message
}

pub fn merkle_root_hex(tx_ids: &[String]) -> Result<String> {
    if tx_ids.is_empty() {
        return Ok(ZERO_HASH_HEX.to_string());
    }
    let mut layer = tx_ids
        .iter()
        .map(|tx_id| {
            let mut leaf = Vec::with_capacity(33);
            leaf.push(0x00);
            leaf.extend_from_slice(&decode_bytes(tx_id)?);
            Ok(blake3::hash(&leaf).as_bytes().to_vec())
        })
        .collect::<Result<Vec<_>>>()?;

    while layer.len() > 1 {
        let mut next = Vec::with_capacity((layer.len() + 1) / 2);
        for pair in layer.chunks(2) {
            let left = &pair[0];
            let right = if pair.len() == 2 { &pair[1] } else { &pair[0] };
            let mut node = Vec::with_capacity(65);
            node.push(0x01);
            node.extend_from_slice(left);
            node.extend_from_slice(right);
            next.push(blake3::hash(&node).as_bytes().to_vec());
        }
        layer = next;
    }
    Ok(hex::encode(&layer[0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_public_key_and_verifies_signature() {
        let secret = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
        let public = public_key_from_secret_hex(secret).unwrap();
        let message = b"aggora";
        let sig = sign_with_secret_hex(secret, message).unwrap();
        assert!(verify_ed25519(&public, message, &sig).unwrap());
        assert!(!verify_ed25519(&public, b"other", &sig).unwrap());
    }

    #[test]
    fn empty_merkle_root_is_zero_hash() {
        assert_eq!(merkle_root_hex(&[]).unwrap(), ZERO_HASH_HEX);
    }
}
