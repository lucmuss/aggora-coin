# Implementation Status

Aggora Coin currently implements the v1 single-validator prototype foundation from the technical specification. It is not yet the full production/multi-validator system.

## Implemented

- Rust workspace with focused crates for types, crypto, storage, PoH, economy, state, REST, and CLI.
- BLAKE3 wallet/operator/validator IDs, transaction IDs, Merkle roots, and PoH hash chaining.
- Ed25519 signatures for operator-signed requests and user-signed transfer/burn payloads.
- Genesis operator bootstrap from environment secret/public key.
- sled persistence trees for wallets, operators, validators, transactions, PoH log, wallet transaction index, iterations, and system state.
- REST API for wallet creation, charge/mint, transfer, burn, public wallet/transaction/iteration/stats queries, WebSocket stats events, admin parameters, admin snapshot, and simulation iteration trigger.
- Logarithmic penalties, adaptive burn-rate control, activity EMA redistribution, faucet accounting, Gini metric, and per-iteration JSON snapshots.
- Docker image, healthcheck, and compose file.
- Aggora Chat settings integration for operator-signed wallet creation and wallet-state refresh.
- Compose dependency so Aggora Chat `web` waits for Aggora Coin health before starting.

## Partially Implemented Prototype Scope

- Simulation endpoint executes economic iterations, but scripted seed replay and stochastic synthetic traffic generation are intentionally minimal.
- Consensus is single-validator with PoH anchoring; round-robin multi-validator leader rotation is represented in config/types but not networked.
- Captcha uses a development proof contract from Aggora Chat rather than a production Turnstile/hCaptcha verifier.
- Rate-limit fields exist in configuration, but API rate-limiting middleware is not yet implemented in the Rust gateway.
- Admin operator changes are exposed as a v1 disabled endpoint, matching the single-genesis-operator prototype constraint.

## Not Yet Implemented

- libp2p/gRPC validator networking.
- Byzantine fault tolerance, slashing, threshold signatures, or multi-operator governance.
- React/Leptos Coin dashboard with charts.
- Full seed replay engine, all scenario files, and 24-iteration numerical convergence report.
- Criterion benchmarks and >=80% coverage report.
- Production KMS/HSM key handling.

## Verification Performed

- `cargo test --workspace --quiet` in the official Rust Docker image.
- `pytest tests/accounts/test_agc_wallet.py -q` with SQLite test database override.
- `python manage.py check` and `makemigrations --check --dry-run`.
- `ruff check` for changed Django/Python files.
- `docker compose config --quiet` for local and production compose files.
- `docker compose up --build -d` for Aggora Coin standalone image.
- `docker compose -f docker-compose.local.yml up -d aggora-coin web` for integrated stack.
- Live signed wallet creation through Aggora Chat container to `http://aggora-coin:8080`.
- Live public API verification at `http://127.0.0.1:18081/api/v1/stats`.
