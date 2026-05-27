# Agora Coin Technical Specification v1.0 Implementation Notes

This codebase implements the prototype scope of the supplied v1.0 specification:

- Wallet ID, operator ID, validator ID, transaction ID, Merkle roots, and PoH hashes use BLAKE3.
- Operator and user signatures use Ed25519.
- Wallet creation and charge are operator-signed; transfers and burns are user-signed.
- Every accepted state transition is persisted and anchored in a sequential PoH entry.
- Iterations apply logarithmic penalties, adaptive burn-rate control, activity-score redistribution, faucet accounting, Gini metrics, and JSON snapshots.
- The v1 operator registry is bootstrapped with one genesis operator from `AGORA_OPERATOR_SECRET`/`AGORA_OPERATOR_PUBKEY`.
- Multi-operator changes are exposed as an admin endpoint but disabled for v1.

The full user-provided mathematical and API specification is treated as the canonical product document for the next hardening phases.
