# Changelog

All notable changes to Aggora Coin are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the workspace itself uses a single
0.x version across all crates while the protocol is still pre-stable.

## [Unreleased]

## [0.3.0] — 2026-05-28

### Added
- **Adaptive-burn parameter tuning** with empirical evidence in
  [`docs/parameter-tuning.md`](docs/parameter-tuning.md). Eight 24-iteration sweeps establish the
  production-recommended starting parameters and demonstrate Gini reduction from 0.37 → 0.13
  while supply stays bounded (~2× over 24 iterations).
- **Rate-limit middleware**: per-IP `wallet`-create daily window and per-wallet outgoing-tx
  per-minute window. Returns `RATE_LIMIT_EXCEEDED` / HTTP 429 per spec G.4 and is enforced
  before Ed25519 verification so attackers can't burn validator CPU on bogus signatures.
- **Seed-replay loader**: `SeedFile` schema in `aggora-types`, `CoinState::install_seed`, and
  the `aggora-node load-seed` CLI subcommand. Idempotent across restarts so bootstrap can be
  re-run safely.
- **End-to-end integration tests** in [`crates/aggora-state/tests/end_to_end.rs`](crates/aggora-state/tests/end_to_end.rs)
  — five tests covering the happy-path round trip, the concurrent-transfer race that proves
  the double-spend fix from 0.2.0, the operator-replay guard, the live iteration supply
  invariant, and the duplicate-wallet path.
- Repository-wide documentation: [`docs/features.md`](docs/features.md) is the crate-by-crate
  responsibility map; [`docs/parameter-tuning.md`](docs/parameter-tuning.md) is the simulation
  study; [`CLAUDE.md`](CLAUDE.md) orients any AI coding assistant continuing work here.
- Module-level rustdoc on every crate and doc comments on the major public types so a reader
  can navigate data flow without diving into call sites.

### Changed
- **Defaults**: `growth_factor_per_iteration` 0.30 → **0.05**, `target_penalty_share_of_supply`
  0.03 → **0.05**, `inverse_balance_weight` 0.0 → **0.5**. The previous defaults caused ~80×
  supply growth over 24 iterations in simulation; the new ones keep inflation tracking the
  configured 2 % target. Updated in `config/default.toml` and in the Rust `Default` impls.
- `SystemState` now carries both `previous_iteration_supply` and
  `previous_iteration_start_supply`; both ends of each cycle are advanced at iteration commit.

### Fixed
- **Adaptive burn was inert.** The inflation metric compared this iteration's start supply to
  the previous iteration's end supply; in steady state these are equal (transfers are
  zero-sum), so the controller never saw the faucet-driven growth and stayed pinned at
  `burn_base`. Reworked to use the previous iteration's *internal* delta per spec D.4 — the
  burn rate now actually responds and is the linchpin of the parameter-tuning results.

## [0.2.0] — 2026-05-27

### Added
- **Deterministic economic simulator** (`aggora-economy::simulator`) driving the real
  iteration engine over a synthetic ChaCha20-seeded population. Exposed as
  `aggora-node simulate` with per-flag overrides and per-iteration CSV output of supply, Gini,
  burn rate, inflation, top-10 share, etc.
- Supply-invariant unit test asserting `post == snapshot − burned + faucet_from_mint` exactly,
  even under stress conditions where `burn_max + faucet_share > 1`.
- `CoinState::live_gini()` diagnostic for the exact (O(N)) Gini over the live wallet set;
  `/stats` continues to return the cached value for hot-path use.

### Changed
- Penalty pool allocation rewritten to use a priority order (faucet → burn → redistribution)
  so the three slices add up to exactly the pool. The previous code could under- or
  over-allocate when `β + φ > 1`.
- `execute_iteration` rewritten to use index-aligned vectors instead of nested `Vec::find`,
  taking the per-wallet loops from O(N²) back to O(N).
- `/stats` now O(1): served from `last_gini` and incremental counters in `SystemState`. The
  previous implementation scanned every wallet and recomputed Gini on every request and every
  WebSocket tick.
- Per-transaction `flush()` removed; sled's 500 ms background flush plus the explicit
  iteration-commit flush handle durability. This is the largest single throughput win.

### Fixed
- **Double-spend / TOCTOU race**: `transfer`, `burn`, `charge`, and `create_wallet` now read
  the wallet and validate balance/nonce *inside* the `system.write()` lock. Previously the
  read happened before the lock, so two concurrent transfers from the same wallet could both
  pass validation against the same stale snapshot and double-spend / re-use a nonce.
- **/charge replay**: operator requests are authenticated only over (method, path, timestamp,
  body), so the same signed body was replayable inside the timestamp-drift window. Added an
  in-memory sliding-window guard that prunes expired entries and rejects duplicates.
- Gini is now clamped to `[0,1]`; float rounding could push the formula a hair outside.

## [0.1.0] — 2026-05-27

### Added
- Initial workspace and v1 single-validator prototype: crates for types, crypto, storage,
  PoH, economy, state, REST, and CLI.
- BLAKE3 IDs and hashes, Ed25519 signatures, Merkle roots, PoH chaining.
- sled persistence for wallets, operators, validators, transactions, PoH log, secondary
  indexes, iteration commits, and system singletons.
- REST + WebSocket gateway implementing the spec G.2 endpoints (wallet create, charge,
  transfer, burn, queries, stats, admin parameters, simulation control).
- Iteration engine implementing the spec D economic model: logarithmic penalty, adaptive
  burn rate, activity-weighted redistribution, Gini metric, JSON snapshots.
- Docker image, healthcheck, and `docker-compose.yml`.
- Aggora Chat integration: operator-signed wallet creation from the chat UI and wallet-state
  refresh, with the chat `web` service depending on Aggora Coin's healthcheck.

[Unreleased]: https://github.com/lucmuss/aggora-coin/compare/main...HEAD
[0.3.0]: https://github.com/lucmuss/aggora-coin/compare/4b62d44...50010f8
[0.2.0]: https://github.com/lucmuss/aggora-coin/compare/594af8b...4b62d44
[0.1.0]: https://github.com/lucmuss/aggora-coin/commit/594af8b
