# Aggora Coin — Feature Reference

A compact, end-to-end map of what Aggora Coin (v1 prototype) currently does, structured the same
way an integrating service or another AI agent needs to read it: by responsibility, with the
crate that owns it and the spec section it maps to.

## At a glance

Aggora Coin is a single-validator, sled-backed AGC ledger with operator-permissioned wallet
creation, user-signed permissionless transfers, and a periodic redistribution iteration that
reduces wealth inequality via a logarithmic penalty + adaptive burn + activity-weighted reward
loop. A REST/WebSocket gateway exposes the chain; a deterministic simulator drives the same
engine without sled for parameter tuning and convergence proofs.

## Crate-level responsibilities

| Crate | Owns | Spec |
|-------|------|------|
| `aggora-types`   | All on-wire types, system state, configurable parameters, seed schema. | B, F |
| `aggora-crypto`  | BLAKE3 hashing, Ed25519 sign/verify, canonical request/Tx encoding, Merkle root, ID derivation. | C |
| `aggora-storage` | sled trees for wallets, operators, validators, txs, PoH log, iterations, system singletons, JSON snapshots. | H |
| `aggora-poh`     | PoH entry construction and chain verification. | C.1, E |
| `aggora-economy` | Penalty math, adaptive burn, redistribution, Gini, iteration engine, deterministic synthetic simulator. | D, J |
| `aggora-state`   | Transactional state machine on top of storage: wallet creation, charge, transfer, burn, iteration, rate limits, replay guard, signed-request verification. | B, E, I, K |
| `aggora-rest`    | axum REST + WebSocket gateway, per-IP/per-wallet rate limits, error mapping. | G |
| `aggora-cli`     | `aggora-node` binary: `node`, `keygen`, `sign-request`, `sim`, `simulate`, `load-seed`. | M, J |

## Identity & cryptography (spec C)

- Ed25519 keys for operators, validators, and users.
- Wallet/Operator/Validator IDs = `BLAKE3(public_key)` (32 bytes, hex on the wire).
- Transaction IDs = `BLAKE3(canonical_payload_without_signature)`.
- User signatures = Ed25519 over the raw 32-byte `tx_id`.
- Operator signatures = Ed25519 over `METHOD\nPATH\nTIMESTAMP_MS\nBODY`.
- Merkle root = BLAKE3 binary tree with explicit 0x00/0x01 domain tags.

## Wallets & permissions

- **Wallet creation is permissioned**: only the genesis operator (v1 single-operator) can mint
  wallets. The request is operator-signed, captcha-gated (configurable), and per-IP rate-limited
  to `rate_limit_wallet_per_ip_per_day`.
- **Transfers/Burns are permissionless** to anyone holding a wallet key, with per-wallet rate
  limiting `rate_limit_tx_per_wallet_per_minute`.
- **Replay-protected /charge**: an in-memory sliding window of operator signatures rejects any
  identical signed body within `timestamp_drift_ms`.

## State machine (spec B, E)

- All wallet read/validate/mutate steps live inside the same async `RwLock<SystemState>` so
  concurrent transfers cannot double-spend or re-use a nonce (verified by
  [`concurrent_transfers_from_same_wallet_cannot_double_spend`](../crates/aggora-state/tests/end_to_end.rs)).
- Each accepted transaction emits a PoH entry through `append_tx_locked` and writes:
  - the canonical Transaction record (`transactions` tree),
  - a `wallet_id||tick` secondary index (`tx_by_wallet` tree),
  - the new PoH entry (`poh_log` tree),
  - the updated `SystemState` singleton.
- `n_wallets`, `n_active_wallets`, `total_supply`, `last_gini`, `last_inflation` are maintained
  incrementally so `/stats` returns in O(1).
- Per-tx fsync is removed; sled's background flusher (500 ms) and the explicit iteration-commit
  flush handle durability.

## Iteration engine (spec D, I)

Pipeline implemented in [`aggora-economy::execute_iteration`](../crates/aggora-economy/src/lib.rs):

1. Snapshot supply, wallet count, Gini.
2. Compute inflation `(M_end_prev - M_start_prev) / M_start_prev` (spec D.4 — fixed in this
   release; the previous formula was always 0.0 in steady state).
3. Adapt burn rate `β = clip(β₀ + k_β (I - I*), β_min, β_max)`.
4. Logarithmic penalty per wallet, then auto-calibrate to hit `target_penalty_share_of_supply`
   when deviation > 20 %.
5. Allocate the penalty pool in priority order — **faucet → burn → redistribution** — so
   `faucet_from_penalty + burned + redistribution_pool == penalty_total` exactly. This
   guarantees the spec D.11 invariant `post = snapshot - burned + faucet_from_mint`.
6. Update each wallet's activity score (EMA), then distribute the redistribution pool by
   `α_i · (1+balance)^{-γ}` weights (activity + optional inverse-balance tilt).
7. Settle rounding residual to burn or genesis per `residual_policy`.
8. Spawn `growth_factor · N` new faucet wallets (capped), funded by faucet pool + mint.
9. Build and persist the `IterationCommit` transaction; write a JSON snapshot.

`execute_iteration` is O(N), allocates no extra `HashMap` lookups (all per-wallet arrays are
index-aligned with `wallets`), and is exercised by both the unit invariant test and the live
state integration test.

## Simulator (spec J)

[`aggora-economy::run_simulation`](../crates/aggora-economy/src/simulator.rs) drives the real
iteration engine over a synthetic population, completely off the storage path:

- Configurable initial wealth distribution (log-normal with adjustable σ).
- Per-iteration Poisson-count synthetic transfers, log-normal amounts.
- Same seed → identical trajectory (ChaCha20Rng).
- Per-iteration CSV emit: supply, n_wallets, n_active, Gini, top10_share, median, penalty,
  burned, reward, faucet_from_mint, new_wallets, n_txs, burn_rate, inflation.

Exposed as `aggora-node simulate` with per-flag overrides for the key economy parameters; see
[`parameter-tuning.md`](parameter-tuning.md) for the validation study behind the production
defaults.

## REST surface (spec G)

| Method | Path                                  | Auth      |
|--------|---------------------------------------|-----------|
| POST   | `/api/v1/wallet`                      | Operator  |
| POST   | `/api/v1/charge`                      | Operator  |
| POST   | `/api/v1/transaction`                 | User      |
| POST   | `/api/v1/burn`                        | User      |
| GET    | `/api/v1/wallet/{id}`                 | Open      |
| GET    | `/api/v1/wallet/{id}/transactions`    | Open      |
| GET    | `/api/v1/transaction/{id}`            | Open      |
| GET    | `/api/v1/iteration/current`           | Open      |
| GET    | `/api/v1/iteration/{id}`              | Open      |
| GET    | `/api/v1/stats`                       | Open (O(1)) |
| GET    | `/api/v1/stats/gini`                  | Open      |
| GET    | `/api/v1/stats/supply`                | Open      |
| WS     | `/api/v1/events`                      | Open      |
| GET/PUT| `/admin/parameters`                   | Admin (Genesis) |
| POST   | `/admin/simulation/start|stop`        | Admin     |
| GET    | `/admin/simulation/status`            | Admin     |
| POST   | `/admin/snapshot`                     | Admin     |
| POST   | `/admin/operator`                     | Admin (v1: disabled stub) |
| GET    | `/healthz`, `/api/v1/health`          | Open      |
| GET    | `/api/v1/operator/genesis`            | Open      |

Error codes match spec G.4 (`INVALID_SIGNATURE`, `INVALID_NONCE`, `INSUFFICIENT_BALANCE`,
`WALLET_NOT_FOUND`, `OPERATOR_UNAUTHORIZED`, `RATE_LIMIT_EXCEEDED`, `ITERATION_IN_PROGRESS`,
`INTERNAL_ERROR`, `BAD_REQUEST`).

## CLI (`aggora-node`)

| Command          | Purpose |
|------------------|---------|
| `node`           | Run the REST gateway + state machine. |
| `keygen`         | Generate Ed25519 keypair (operator or wallet). |
| `sign-request`   | Print the operator headers for a canonical signed request. |
| `sim`            | Run N iterations of the on-chain engine against the configured DB. |
| `simulate`       | Run the deterministic synthetic economic simulator, emit CSV. |
| `load-seed`      | Bootstrap the local DB from a `seeds/*.json` file. |

## Persistence (spec H)

sled trees:

```
wallets        wallet_id           -> Wallet (JSON)
operators      operator_id         -> Operator
validators     validator_id        -> Validator
transactions   tx_id               -> Transaction (tagged enum)
poh_log        tick (BE u64)       -> PohEntry
tx_by_wallet   wallet_id||tick||id -> tx_id (secondary index)
iterations     iteration_id (BE)   -> IterationCommit
system         "state" / "parameters"
```

Per-iteration JSON snapshots are written to `snapshots/iter_{N}.json` and contain parameters,
system state, all wallets, and the PoH anchor at the time of the commit. Admin can also force a
manual snapshot via `POST /admin/snapshot`.

## Security model (spec K)

- **Sybil**: operator signature + captcha + per-IP daily wallet limit.
- **Replay**:
  - User transactions: per-wallet monotonic nonce + signed `tx_id`.
  - Operator requests: in-memory signature set within `timestamp_drift_ms` window — blocks
    duplicate `/charge` mints (proven by integration test).
- **Double-spend**: serialization under the system lock for the entire read-validate-write
  critical section.
- **DoS**: per-IP wallet limit, per-wallet tx-per-minute limit, both enforced before signature
  verification so attackers cannot cost the validator CPU.
- **Key management**: operator secret is read from env (`AGORA_OPERATOR_SECRET`); a hard-coded
  dev secret is provided strictly for local development.

## Invariants verified by tests

| Test | Crate | Invariant |
|------|-------|-----------|
| `gini_bounds_hold` | aggora-economy | Gini ∈ [0,1]. |
| `iteration_preserves_accounting` | aggora-economy | post_supply == Σ balances. |
| `supply_invariant_holds_under_high_burn_and_growth` | aggora-economy | post = snapshot − burned + faucet_from_mint, exact, under β+φ > 1. |
| `simulation_is_deterministic_and_bounded` | aggora-economy | Same config → same trajectory; burn ∈ [β_min, β_max]. |
| `derives_public_key_and_verifies_signature` | aggora-crypto | Ed25519 sign/verify round-trip. |
| `creates_verifiable_entry` | aggora-poh | PoH hash chain verifies. |
| `boots_with_dev_operator` | aggora-state | Bootstrap from dev secret. |
| `wallet_creation_charge_transfer_burn_round_trip` | aggora-state | End-to-end happy path. |
| `concurrent_transfers_from_same_wallet_cannot_double_spend` | aggora-state | 1/N wins under contention, others get `INVALID_NONCE`. |
| `replay_guard_rejects_duplicate_operator_signature_on_charge` | aggora-state | Second identical /charge fails. |
| `iteration_preserves_total_supply_invariant_with_live_wallets` | aggora-state | Live iteration over real sled state preserves D.11. |
| `duplicate_wallet_id_is_rejected` | aggora-state | Same pubkey twice → second call gets `WALLET_ALREADY_EXISTS`. |
