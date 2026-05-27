# Aggora Coin

Aggora Coin (AGC) is a Rust/Axum prototype for the permissioned wallet-creation and permissionless transaction layer used by Aggora Chat.

This repository implements the v1 single-validator foundation:

- Ed25519 operator and user signature verification.
- BLAKE3 wallet IDs, transaction IDs, Merkle roots, and PoH entries.
- sled-backed wallets, operators, validators, transactions, PoH log, iterations, state, and parameters.
- REST endpoints for wallet creation, charge/mint, transfer, burn, query, stats, WebSocket stats events, admin parameters, snapshots, and simulation iteration runs.
- Logarithmic penalty, adaptive burn, activity-weighted redistribution, faucet mint accounting, Gini metrics, and JSON snapshots.
- Docker image with healthcheck and persistent volumes.

## Run Locally With Docker

```sh
docker compose up --build
curl http://127.0.0.1:18081/healthz
curl http://127.0.0.1:18081/api/v1/stats
```

Default development operator secret:

```text
000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f
```

Override it in production with `AGORA_OPERATOR_SECRET` and store it in a secret manager.

## Create A Signed Wallet Request

The REST gateway verifies operator headers over this canonical message:

```text
METHOD\nPATH\nTIMESTAMP_MS\nJSON_BODY
```

The helper command prints all required headers:

```sh
BODY='{"public_key":"<hex-32-byte-public-key>","captcha_token":"dev","timestamp":1730000000000}'
TS=$(date +%s%3N)
aggora-node sign-request \
  --secret "$AGORA_OPERATOR_SECRET" \
  --method POST \
  --path /api/v1/wallet \
  --timestamp "$TS" \
  --body "$BODY"
```

## API Surface

- `POST /api/v1/wallet` operator-signed wallet creation.
- `POST /api/v1/charge` operator-signed AGC mint/charge.
- `POST /api/v1/transaction` user-signed transfer.
- `POST /api/v1/burn` user-signed burn.
- `GET /api/v1/wallet/{id}` wallet state.
- `GET /api/v1/wallet/{id}/transactions` wallet transaction archive.
- `GET /api/v1/transaction/{id}` transaction detail.
- `GET /api/v1/iteration/current` current iteration detail.
- `GET /api/v1/iteration/{id}` iteration commit detail.
- `GET /api/v1/stats`, `/stats/gini`, `/stats/supply` public metrics.
- `GET /api/v1/events` WebSocket stats stream.
- `GET|PUT /admin/parameters` genesis-operator admin parameters.
- `POST /admin/simulation/start|stop`, `GET /admin/simulation/status` simulation controls.
- `POST /admin/snapshot` JSON snapshot.

## Workspace

```text
crates/aggora-types     shared primitives and DTOs
crates/aggora-crypto    BLAKE3, Ed25519, canonical request helpers, Merkle roots
crates/aggora-storage   sled schema wrapper and snapshots
crates/aggora-poh       PoH entry generation and verification
crates/aggora-economy   penalty, burn, redistribution, Gini, iteration math
crates/aggora-state     state machine and transaction application
crates/aggora-rest      Axum REST/WebSocket API
crates/aggora-cli       aggora-node binary
```

## Implementation Status

See [`docs/implementation-status.md`](docs/implementation-status.md) for the exact implemented, partial, and not-yet-implemented scope.

## Production Notes

This is the v1 prototype from the technical specification: single genesis operator, single validator by default, crash-tolerant rather than Byzantine-tolerant. Multi-validator networking and BFT/threshold signatures remain v2 hardening items.
