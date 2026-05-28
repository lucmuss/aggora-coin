//! Proof-of-History entry construction and chain verification.
//!
//! A PoH entry binds the previous chain hash, the current tick number, the Merkle root of the
//! transactions accepted at this tick, and the leader validator id into a BLAKE3 digest. Tick
//! N's hash depends on tick N-1, so the sequence is a verifiable total order (spec section C.1
//! and E). [`build_entry`] also signs over `(tick, hash, tx_root)` with the leader's Ed25519
//! key so followers can authenticate the entry without re-running consensus.
//!
//! In the single-validator prototype the state machine calls [`build_entry`] per accepted
//! transaction; in a multi-validator deployment the leader would batch up to
//! `max_txs_per_tick` transactions per call.

use aggora_crypto::{blake3_hex, merkle_root_hex, now_ms, sign_with_secret_hex};
use aggora_types::{Hash, PohEntry, Tick, ValidatorId};
use anyhow::Result;

pub fn poh_hash(prev_hash: &str, tick: Tick, tx_root: &str, leader_id: &str) -> String {
    let mut bytes = Vec::with_capacity(32 + 8 + 32 + 32);
    bytes.extend_from_slice(prev_hash.as_bytes());
    bytes.extend_from_slice(&tick.to_le_bytes());
    bytes.extend_from_slice(tx_root.as_bytes());
    bytes.extend_from_slice(leader_id.as_bytes());
    blake3_hex(bytes)
}

pub fn build_entry(
    tick: Tick,
    prev_hash: Hash,
    tx_ids: Vec<Hash>,
    leader_id: ValidatorId,
    leader_secret_hex: Option<&str>,
) -> Result<PohEntry> {
    let tx_root = merkle_root_hex(&tx_ids)?;
    let hash = poh_hash(&prev_hash, tick, &tx_root, &leader_id);
    let signing_payload = format!("{}:{}:{}", tick, hash, tx_root);
    let leader_sig = match leader_secret_hex {
        Some(secret) => sign_with_secret_hex(secret, signing_payload.as_bytes())?,
        None => String::new(),
    };
    Ok(PohEntry {
        tick,
        prev_hash,
        hash,
        tx_root,
        tx_ids,
        leader_id,
        leader_sig,
        wall_clock: now_ms(),
    })
}

pub fn verify_chain(prev_hash: &str, entry: &PohEntry) -> Result<bool> {
    if entry.prev_hash != prev_hash {
        return Ok(false);
    }
    let expected_root = merkle_root_hex(&entry.tx_ids)?;
    if expected_root != entry.tx_root {
        return Ok(false);
    }
    Ok(poh_hash(&entry.prev_hash, entry.tick, &entry.tx_root, &entry.leader_id) == entry.hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aggora_crypto::ZERO_HASH_HEX;

    #[test]
    fn creates_verifiable_entry() {
        let entry = build_entry(1, ZERO_HASH_HEX.to_string(), vec![], "leader".into(), None).unwrap();
        assert!(verify_chain(ZERO_HASH_HEX, &entry).unwrap());
    }
}
