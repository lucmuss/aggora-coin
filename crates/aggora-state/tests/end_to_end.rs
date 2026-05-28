//! End-to-end integration tests against the live `CoinState` (sled-backed) API.
//!
//! These exercise the same public surface the REST gateway calls, including signature
//! verification, replay protection, the iteration engine, and concurrent writers. They are
//! what proves the bug fixes from the recent refactor (double-spend race, replay guard,
//! supply invariant) actually hold against the real state machine.

use aggora_crypto::{
    canonical_request_message, hash_serializable, normalize_bytes_to_hex, now_ms, public_key_from_secret_hex,
    sign_with_secret_hex, tx_signing_message, wallet_id_from_public_key,
};
use aggora_state::{CoinState, NodeConfig, StateError};
use aggora_types::{
    BurnRequest, ChargeRequest, SystemParameters, TransferRequest, WalletCreateRequest,
};
use ed25519_dalek::SigningKey;
use rand::{rngs::OsRng, RngCore};
use serde::Serialize;

const DEV_OPERATOR_SECRET: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

/// Per-test scratch parameters. Uses unique sled/snapshot paths so tests don't collide.
fn fresh_params(tag: &str) -> SystemParameters {
    let mut params = SystemParameters::default();
    let stamp = now_ms();
    params.storage.db_path = format!("/tmp/aggora-state-it-{tag}-{stamp}.sled");
    params.storage.snapshot_path = format!("/tmp/aggora-state-it-{tag}-{stamp}-snaps");
    params.security.require_captcha_proof = false;
    params.security.dev_auth_bypass = false;
    params
}

/// Mirrors `aggora_rest::operator_auth` so the test can drive the signed-request path the same
/// way the REST gateway does. Returns `(headers_unused, signature)`; the signature is what the
/// state API needs when forwarding it onto the on-chain mint transaction.
fn sign_operator_body<T: Serialize>(method: &str, path: &str, body: &T) -> (i64, String, String) {
    let body_bytes = serde_json::to_vec(body).unwrap();
    let timestamp = now_ms();
    let message = canonical_request_message(method, path, timestamp, &body_bytes);
    let sig = sign_with_secret_hex(DEV_OPERATOR_SECRET, &message).unwrap();
    let pubkey = public_key_from_secret_hex(DEV_OPERATOR_SECRET).unwrap();
    (timestamp, sig, pubkey)
}

/// Mints a wallet via the operator path and returns its keypair + assigned id/balance.
async fn create_wallet(state: &CoinState) -> (SigningKey, String) {
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    let signing_key = SigningKey::from_bytes(&seed);
    let public_key = hex::encode(signing_key.verifying_key().to_bytes());
    let request = WalletCreateRequest {
        public_key: public_key.clone(),
        captcha_token: "test".into(),
        timestamp: now_ms(),
        metadata: serde_json::Value::Null,
    };
    let (ts, sig, op_pub) = sign_operator_body("POST", "/api/v1/wallet", &request);
    let body = serde_json::to_vec(&request).unwrap();
    let operator = state
        .verify_operator_request("POST", "/api/v1/wallet", &body, Some(ts), Some(&sig), None, Some(&op_pub), false)
        .await
        .expect("operator auth");
    let response = state
        .create_wallet(request, &operator, sig)
        .await
        .expect("wallet creation");
    (signing_key, response.wallet_id)
}

#[derive(Serialize)]
struct TransferPayload<'a> {
    from: &'a str,
    to: &'a str,
    amount: u64,
    nonce: u64,
    timestamp: i64,
    sender_pubkey: String,
}

#[derive(Serialize)]
struct BurnPayload<'a> {
    from: &'a str,
    amount: u64,
    nonce: u64,
    timestamp: i64,
    sender_pubkey: String,
}

fn signed_transfer(from_id: &str, to_id: &str, amount: u64, nonce: u64, sender: &SigningKey) -> TransferRequest {
    let sender_pubkey_hex = hex::encode(sender.verifying_key().to_bytes());
    let timestamp = now_ms();
    let tx_id = hash_serializable(&TransferPayload {
        from: from_id,
        to: to_id,
        amount,
        nonce,
        timestamp,
        sender_pubkey: normalize_bytes_to_hex(&sender_pubkey_hex, 32).unwrap(),
    })
    .unwrap();
    let message = tx_signing_message(&tx_id).unwrap();
    let sig = sign_with_secret_hex(&hex::encode(sender.to_bytes()), &message).unwrap();
    TransferRequest {
        from: from_id.into(),
        to: to_id.into(),
        amount,
        nonce,
        timestamp,
        sender_pubkey: sender_pubkey_hex,
        user_sig: sig,
    }
}

fn signed_burn(from_id: &str, amount: u64, nonce: u64, sender: &SigningKey) -> BurnRequest {
    let sender_pubkey_hex = hex::encode(sender.verifying_key().to_bytes());
    let timestamp = now_ms();
    let tx_id = hash_serializable(&BurnPayload {
        from: from_id,
        amount,
        nonce,
        timestamp,
        sender_pubkey: normalize_bytes_to_hex(&sender_pubkey_hex, 32).unwrap(),
    })
    .unwrap();
    let message = tx_signing_message(&tx_id).unwrap();
    let sig = sign_with_secret_hex(&hex::encode(sender.to_bytes()), &message).unwrap();
    BurnRequest {
        from: from_id.into(),
        amount,
        nonce,
        timestamp,
        sender_pubkey: sender_pubkey_hex,
        user_sig: sig,
    }
}

#[tokio::test]
async fn wallet_creation_charge_transfer_burn_round_trip() {
    let params = fresh_params("happy");
    let state = CoinState::open(NodeConfig::from_parameters(params)).await.unwrap();

    let (alice_key, alice_id) = create_wallet(&state).await;
    let (_bob_key, bob_id) = create_wallet(&state).await;

    // Top alice up with a charge from the operator.
    let charge = ChargeRequest {
        to: alice_id.clone(),
        amount: 50_000_000,
        eur_amount: 50,
        timestamp: now_ms(),
    };
    let (ts, sig, op_pub) = sign_operator_body("POST", "/api/v1/charge", &charge);
    let body = serde_json::to_vec(&charge).unwrap();
    let operator = state
        .verify_operator_request("POST", "/api/v1/charge", &body, Some(ts), Some(&sig), None, Some(&op_pub), false)
        .await
        .unwrap();
    state.charge(charge, &operator, sig).await.unwrap();

    // Alice transfers half to Bob.
    let transfer = signed_transfer(&alice_id, &bob_id, 25_000_000, 0, &alice_key);
    state.transfer(transfer).await.unwrap();

    // Alice burns a tiny amount.
    let burn = signed_burn(&alice_id, 1_000_000, 1, &alice_key);
    state.burn(burn).await.unwrap();

    let alice = state.storage().get_wallet(&alice_id).unwrap().unwrap();
    let bob = state.storage().get_wallet(&bob_id).unwrap().unwrap();
    // Alice: 10 AGC seed + 50 AGC charge - 25 AGC transfer - 1 AGC burn = 34 AGC
    assert_eq!(alice.balance, 34_000_000);
    assert_eq!(alice.nonce, 2);
    // Bob: 10 AGC seed + 25 AGC received
    assert_eq!(bob.balance, 35_000_000);

    let stats = state.stats().await.unwrap();
    assert_eq!(stats.n_wallets, 2);
}

#[tokio::test]
async fn concurrent_transfers_from_same_wallet_cannot_double_spend() {
    // Pre-fix this test failed: two transfers reading the same balance/nonce snapshot would both
    // pass validation before either acquired the system lock, allowing double-spend / nonce reuse.
    let params = fresh_params("race");
    let state = CoinState::open(NodeConfig::from_parameters(params)).await.unwrap();
    let (alice_key, alice_id) = create_wallet(&state).await;
    let (_bob_key, bob_id) = create_wallet(&state).await;

    // Top Alice up so she could technically afford all attempts if the race were exploitable.
    let charge = ChargeRequest {
        to: alice_id.clone(),
        amount: 100_000_000,
        eur_amount: 100,
        timestamp: now_ms(),
    };
    let (ts, sig, op_pub) = sign_operator_body("POST", "/api/v1/charge", &charge);
    let body = serde_json::to_vec(&charge).unwrap();
    let operator = state
        .verify_operator_request("POST", "/api/v1/charge", &body, Some(ts), Some(&sig), None, Some(&op_pub), false)
        .await
        .unwrap();
    state.charge(charge, &operator, sig).await.unwrap();

    let starting_balance = state.storage().get_wallet(&alice_id).unwrap().unwrap().balance;
    let attempts = 8u32;
    let amount = 5_000_000u64; // 5 AGC each, well within balance

    // Fire `attempts` transfers concurrently, all signed with the SAME nonce. Only one may win.
    let mut handles = Vec::new();
    for _ in 0..attempts {
        let state = state.clone();
        let alice_key = alice_key.clone();
        let alice_id = alice_id.clone();
        let bob_id = bob_id.clone();
        handles.push(tokio::spawn(async move {
            let tx = signed_transfer(&alice_id, &bob_id, amount, 0, &alice_key);
            state.transfer(tx).await
        }));
    }

    let mut accepted = 0u32;
    let mut nonce_rejects = 0u32;
    for handle in handles {
        match handle.await.unwrap() {
            Ok(_) => accepted += 1,
            Err(StateError::InvalidNonce) => nonce_rejects += 1,
            Err(other) => panic!("unexpected transfer error: {other:?}"),
        }
    }
    assert_eq!(accepted, 1, "exactly one transfer must win the race");
    assert_eq!(nonce_rejects, attempts - 1, "all other attempts must be rejected as stale nonces");

    let alice = state.storage().get_wallet(&alice_id).unwrap().unwrap();
    assert_eq!(alice.balance, starting_balance - amount);
    assert_eq!(alice.nonce, 1);
}

#[tokio::test]
async fn replay_guard_rejects_duplicate_operator_signature_on_charge() {
    // /charge is non-idempotent; replaying the same signed body within the timestamp window
    // would otherwise double-mint. The in-memory replay guard must reject the second attempt.
    let params = fresh_params("replay");
    let state = CoinState::open(NodeConfig::from_parameters(params)).await.unwrap();
    let (_alice_key, alice_id) = create_wallet(&state).await;

    let charge = ChargeRequest {
        to: alice_id.clone(),
        amount: 25_000_000,
        eur_amount: 25,
        timestamp: now_ms(),
    };
    let (ts, sig, op_pub) = sign_operator_body("POST", "/api/v1/charge", &charge);
    let body = serde_json::to_vec(&charge).unwrap();
    let operator = state
        .verify_operator_request("POST", "/api/v1/charge", &body, Some(ts), Some(&sig), None, Some(&op_pub), false)
        .await
        .unwrap();
    state.charge(charge.clone(), &operator, sig.clone()).await.unwrap();
    let err = state
        .charge(charge.clone(), &operator, sig.clone())
        .await
        .expect_err("second identical charge must be rejected");
    matches!(err, StateError::InvalidSignature);

    let alice = state.storage().get_wallet(&alice_id).unwrap().unwrap();
    assert_eq!(alice.balance, 10_000_000 + 25_000_000);
}

#[tokio::test]
async fn iteration_preserves_total_supply_invariant_with_live_wallets() {
    let params = fresh_params("iter-invariant");
    let state = CoinState::open(NodeConfig::from_parameters(params)).await.unwrap();

    // Seed a small population so the iteration engine has something to redistribute.
    let mut signers = Vec::new();
    for _ in 0..6 {
        let (key, id) = create_wallet(&state).await;
        signers.push((key, id));
    }

    // Drive a few transfers so activity scores reflect real movement.
    for round in 0..3u64 {
        for i in 0..signers.len() {
            let (sender_key, sender_id) = (signers[i].0.clone(), signers[i].1.clone());
            let (_, recipient_id) = signers[(i + 1) % signers.len()].clone();
            let tx = signed_transfer(&sender_id, &recipient_id, 100_000, round, &sender_key);
            state.transfer(tx).await.unwrap();
        }
    }

    let pre_supply = state.system().await.total_supply;
    let commit = state.execute_iteration_now().await.unwrap();
    let post_supply = state.system().await.total_supply;

    assert_eq!(commit.snapshot_supply, pre_supply);
    // Spec D.11: post = snapshot - burned + faucet_from_mint (exactly).
    assert_eq!(
        commit.post_supply,
        commit.snapshot_supply - commit.burned + commit.faucet_from_mint
    );
    assert_eq!(post_supply, commit.post_supply);

    // The recomputed live Gini must match the cached one (used by /stats).
    let cached = state.system().await.last_gini;
    let live = state.live_gini().await.unwrap();
    assert!((cached - live).abs() < 1e-9, "cached Gini drifted from live");
}

#[tokio::test]
async fn duplicate_wallet_id_is_rejected() {
    // Two attempts to create the same pubkey must serialize through the lock so only the first
    // one inserts the wallet; the second sees the existing record.
    let params = fresh_params("dup-wallet");
    let state = CoinState::open(NodeConfig::from_parameters(params)).await.unwrap();

    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    let signing_key = SigningKey::from_bytes(&seed);
    let public_key = hex::encode(signing_key.verifying_key().to_bytes());
    let wallet_id = wallet_id_from_public_key(&public_key).unwrap();

    for attempt in 0i64..2 {
        let request = WalletCreateRequest {
            public_key: public_key.clone(),
            captcha_token: "test".into(),
            timestamp: now_ms() + attempt,
            metadata: serde_json::Value::Null,
        };
        let (ts, sig, op_pub) = sign_operator_body("POST", "/api/v1/wallet", &request);
        let body = serde_json::to_vec(&request).unwrap();
        let operator = state
            .verify_operator_request("POST", "/api/v1/wallet", &body, Some(ts), Some(&sig), None, Some(&op_pub), false)
            .await
            .unwrap();
        let result = state.create_wallet(request, &operator, sig).await;
        if attempt == 0 {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(StateError::WalletAlreadyExists)));
        }
    }

    assert!(state.storage().get_wallet(&wallet_id).unwrap().is_some());
}
