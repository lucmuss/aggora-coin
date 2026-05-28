# Implementation Status

Aggora Coin currently implements the v1 single-validator prototype foundation from the
technical specification, with the bug-fix and tuning work documented in
[`features.md`](features.md) and [`parameter-tuning.md`](parameter-tuning.md).

## Implemented

- Rust workspace with focused crates for types, crypto, storage, PoH, economy, state, REST, and CLI.
- BLAKE3 wallet/operator/validator IDs, transaction IDs, Merkle roots, and PoH hash chaining.
- Ed25519 signatures for operator-signed requests and user-signed transfer/burn payloads.
- Genesis operator bootstrap from environment secret/public key.
- sled persistence trees for wallets, operators, validators, transactions, PoH log, wallet
  transaction index, iterations, and system state.
- REST API for wallet creation, charge/mint, transfer, burn, public wallet/transaction/iteration/
  stats queries, WebSocket stats events, admin parameters, admin snapshot, and simulation
  iteration trigger.
- Logarithmic penalties, adaptive burn-rate control (with the inflation-metric fix that makes
  it actually responsive to faucet growth), activity EMA redistribution with optional
  inverse-balance tilt, faucet accounting, Gini metric (clamped to [0,1]), and per-iteration
  JSON snapshots.
- Docker image, healthcheck, and compose file.
- Aggora Chat settings integration for operator-signed wallet creation and wallet-state refresh.
- Compose dependency so Aggora Chat `web` waits for Aggora Coin health before starting.
- **Concurrency-safe state machine**: read/validate/mutate is serialized under the system lock;
  concurrent transfers from the same wallet cannot double-spend (proven by integration test).
- **Exact supply accounting**: penalty pool is allocated `faucet → burn → redistribute` so the
  spec D.11 invariant `post = snapshot − burned + faucet_from_mint` holds to the µAGC.
- **Operator-signature replay guard**: in-memory sliding window blocks duplicate /charge mints
  inside the timestamp-drift window.
- **Rate-limit middleware**: per-IP daily wallet-create limit and per-wallet per-minute transfer
  limit, enforced before signature verification; `RATE_LIMIT_EXCEEDED` (HTTP 429) per spec G.4.
- **Deterministic synthetic simulator** (`aggora-node simulate`) emitting per-iteration CSV
  metrics and used to tune the production-recommended starting parameters.
- **Seed-replay loader** (`aggora-node load-seed`) that bootstraps a sled DB from the
  `seeds/*.json` initial_wallets list, idempotent across restarts.
- O(1) `/stats` (cached gini/n_wallets/n_active in system state) and O(N) iteration engine
  (index-aligned vectors, no nested finds).
- Per-tx fsync removed — sled's 500 ms background flush + iteration-commit flush handle
  durability without serializing every transaction on disk.

## Partially Implemented Prototype Scope

- `aggora-node sim` runs N economic iterations against the configured DB but only does
  scripted seed replay through `load-seed`; the live "scripted_events" simulator from the spec
  (transfer bursts, attack scenarios) is still minimal.
- Consensus is single-validator with PoH anchoring; round-robin multi-validator leader rotation
  is represented in config/types but not networked.
- Captcha uses a development proof contract from Aggora Chat rather than a production
  Turnstile/hCaptcha verifier.
- Admin operator changes are exposed as a v1 disabled endpoint, matching the single-genesis-
  operator prototype constraint.

## Not Yet Implemented

- libp2p / gRPC validator networking.
- Byzantine fault tolerance, slashing, threshold signatures, or multi-operator governance.
- React/Leptos Coin dashboard with charts.
- Full seed replay engine for `scripted_events` (spawn_wallets bursts, transfer_burst, etc.).
- Criterion benchmarks for the >=10 000 TX/s claim.
- Production KMS/HSM key handling.

## Verification Performed

- `cargo test --workspace` covers 13 tests, including 5 new end-to-end integration tests that
  exercise the concurrent-transfer race, the replay guard, the live supply invariant, and the
  duplicate-wallet path.
- 24-iteration deterministic simulations across 8 parameter sweeps (see
  [`parameter-tuning.md`](parameter-tuning.md)) confirm supply bounded and Gini reduction at
  the recommended defaults.
- `docker compose config --quiet` for local and production compose files.
- `docker compose up --build -d` for Aggora Coin standalone image.
- Live signed wallet creation through Aggora Chat container to `http://aggora-coin:8080`.
- Live public API verification at `http://127.0.0.1:18081/api/v1/stats`.
