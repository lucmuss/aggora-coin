# CLAUDE.md

This file orients Claude Code (and any other AI coding assistant) working in this repository.
Keep this file short and current; deep references go to `docs/`.

> Workspace policies in [`/srv/projects/CLAUDE.md`](/srv/projects/CLAUDE.md) still apply (uv,
> just, ruff, etc.). This file overrides them only where the rules differ for a pure-Rust
> crate (e.g. `cargo` instead of `uv`).

## What this is

Aggora Coin (AGC) — a Rust workspace implementing the v1 single-validator prototype of a
semi-decentralized token system with permissioned wallet creation, permissionless transfers,
and a periodic redistribution iteration that reduces wealth inequality. It is the chain layer
behind Aggora Chat; the chat application creates wallets here via the operator-signed REST
path.

The full feature map is in [`docs/features.md`](docs/features.md). The parameter-tuning
evidence is in [`docs/parameter-tuning.md`](docs/parameter-tuning.md). Spec is
[`docs/spec.md`](docs/spec.md), architecture is [`docs/architecture.md`](docs/architecture.md),
status is [`docs/implementation-status.md`](docs/implementation-status.md).

## Crate map

```
crates/aggora-types     wire types, configuration schema, seed schema   (no deps beyond serde+std)
crates/aggora-crypto    BLAKE3, Ed25519, canonical request encoding, Merkle
crates/aggora-storage   sled trees + JSON snapshots (spec H)
crates/aggora-poh       Proof-of-History entry + chain verification
crates/aggora-economy   penalty/burn/redistribution math, iteration engine, deterministic simulator
crates/aggora-state     authoritative state machine on top of storage
crates/aggora-rest      axum REST + WebSocket gateway, rate-limit middleware
crates/aggora-cli       aggora-node binary (node | keygen | sign-request | sim | simulate | load-seed)
```

`aggora-state::CoinState` is the only thing that mutates persistent state. The REST layer is
a thin authentication + rate-limit + serialization layer around it.

## How to run things

This repo is Rust-only; the workspace policy `just`/`uv` does not apply.

```sh
# Tests (host needs cargo, or run inside the rust:1-slim-bookworm container)
cargo test --workspace
docker run --rm -v "$PWD":/app -w /app -e CARGO_TARGET_DIR=/tmp/agct \
    rust:1-slim-bookworm cargo test --workspace

# Local node + REST
cargo run --release -p aggora-cli -- node --config config/default.toml

# Docker
docker compose up --build      # exposes http://127.0.0.1:18081

# Deterministic economic simulator (parameter tuning)
cargo run --release -p aggora-cli -- simulate \
    --iterations 24 --initial-wallets 100 --seed 42 \
    --growth-factor 0.05 --target-penalty-share 0.05 --inverse-balance-weight 0.5

# Bootstrap initial wallets from a seed JSON
cargo run --release -p aggora-cli -- load-seed --file seeds/default_100_nodes.json

# Sign an operator request the same way the REST gateway verifies it
cargo run --release -p aggora-cli -- sign-request \
    --secret "$AGORA_OPERATOR_SECRET" --method POST \
    --path /api/v1/wallet --timestamp "$(date +%s%3N)" --body '{}'
```

The Aggora Chat container reaches this service at `http://aggora-coin:8080` over the
`apps-shared` Docker network; see [`docker-compose.yml`](docker-compose.yml).

## Conventions and invariants — please don't regress these

1. **Single-lock state mutation.** Every mutating method on `CoinState` *must* acquire
   `system.write()` before reading the wallet store. The earlier double-spend bug came from
   reading before locking. Keep signature verification and `tx_id` computation outside the
   lock (they're the expensive parts), and keep storage read + validate + write strictly
   inside it. Tests under [`crates/aggora-state/tests/end_to_end.rs`](crates/aggora-state/tests/end_to_end.rs)
   will catch regressions, including a concurrent-transfer race.

2. **Supply invariant.** Spec D.11 says
   `post_supply == snapshot_supply − burned + faucet_from_mint` exactly. The economy
   allocates the penalty pool in priority order (faucet → burn → redistribute); never let the
   three slices over-allocate. The `supply_invariant_holds_under_high_burn_and_growth` test
   pins this even when `β + φ > 1`.

3. **Inflation metric.** Per spec D.4 `I = (M_end_prev − M_start_prev) / M_start_prev` — the
   *previous* iteration's internal change. Comparing the current iteration's start against
   the previous iteration's end is always ~0 in steady state and silently breaks the adaptive
   burn. `SystemState` tracks both `previous_iteration_supply` and
   `previous_iteration_start_supply`; update both at every iteration commit.

4. **Replay protection.** Operator requests are authenticated over (method, path, timestamp,
   body); the same signed body is replayable within the drift window. `/charge` is non-
   idempotent, so `CoinState::guard_operator_replay` *must* be called before applying it.
   `/wallet` is naturally idempotent (`WalletAlreadyExists`) but still rate-limited.

5. **/stats is O(1).** Do not regress to scanning all wallets per request. New stats fields
   belong in `SystemState` (incremental) or `last_gini`-style cached values updated at
   iteration commit. The WebSocket pushes `/stats` every two seconds; an O(N) recomputation
   here would dominate the hot path.

6. **Per-tx fsync is OFF.** sled flushes its log every 500 ms by default; iteration commits
   flush explicitly. Don't add `flush()` to a hot-path handler "just to be safe" — it caps
   throughput at hundreds of TX/s.

7. **Tuning parameters need simulation evidence.** If you change any default in
   `config/default.toml` or the `Default` impls in `aggora-types`, re-run a 24-iteration sweep
   and update [`docs/parameter-tuning.md`](docs/parameter-tuning.md). Stable starting point:
   supply ≤ 3× over 24 iter, inflation in [0, 0.05], Gini monotonically decreasing or stable.

## Where things live

| Need | File |
|------|------|
| Add a wire field           | `crates/aggora-types/src/lib.rs` (mind serde defaults) |
| Add an Ed25519/BLAKE3 helper | `crates/aggora-crypto/src/lib.rs` |
| Change the economic math   | `crates/aggora-economy/src/lib.rs`; validate via simulator |
| Add a state-mutating method | `crates/aggora-state/src/lib.rs` (read + sig outside lock, mutate inside) |
| Add a REST endpoint        | `crates/aggora-rest/src/lib.rs`; map errors via `state_error` |
| Add a CLI subcommand       | `crates/aggora-cli/src/main.rs` |
| Add a sled tree            | `crates/aggora-storage/src/lib.rs`; mirror in spec H |
| Adjust adaptive-burn params | `config/default.toml` *and* `EconomyParameters::default()` |

## Local dev secrets

The dev operator secret hardcoded in `aggora-state` is
`000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f`. Used only when neither
`AGORA_OPERATOR_SECRET` nor `AGGORA_COIN_OPERATOR_SECRET` is set. Production deployments
must override via environment + a secret manager — see `[security]` in `config/default.toml`.

## What is *not* in scope right now

Tracked in [`docs/implementation-status.md`](docs/implementation-status.md): libp2p networking,
BFT/threshold signatures, the Coin dashboard, scripted-event simulator, criterion benchmarks
for the ≥10 000 TX/s claim, KMS/HSM key handling. If asked to add any of these, treat it as a
v2 milestone and discuss with the user first.
