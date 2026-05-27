# Architecture

Aggora Coin is split into small Rust crates so cryptography, economics, persistence, state transition logic, and HTTP wiring can be tested independently.

The v1 runtime is intentionally single-node but replayable:

1. REST receives a signed operator or user request.
2. `aggora-state` verifies semantic invariants: nonce monotonicity, sufficient balance, wallet identity, operator authorization, and iteration locks.
3. The state transition is persisted to sled.
4. The transaction ID is included in a PoH entry with a BLAKE3 Merkle root.
5. Iteration commits snapshot penalties, rewards, burns, faucet accounting, supply, and Gini.

Persistence trees mirror the spec: `wallets`, `operators`, `validators`, `transactions`, `poh_log`, `tx_by_wallet`, `iterations`, and `system`.
