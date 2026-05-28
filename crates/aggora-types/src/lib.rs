//! Wire types and configuration schema shared by every crate in the workspace.
//!
//! Everything here is dependency-free (only `serde` + `std`) on purpose so it can be pulled
//! into clients, the simulator, or future SDK crates without dragging in sled, ed25519, or
//! axum. The convention is:
//!
//! - Primitive crypto types are *hex-encoded `String`s* on the wire (`WalletId`, `Signature`,
//!   `Hash`, `PublicKey`). The decoding helpers live in [`aggora_crypto`].
//! - Numeric balances are unsigned micro-AGC (`MicroAgc` = `u64`); 1 AGC = 1 000 000 µAGC.
//! - Transactions are an externally-tagged enum (`kind: "transfer" | "mint" | …`) so the
//!   on-disk JSON is self-describing.
//! - Every configurable parameter has a `Default` impl that is the production-recommended
//!   starting value — see [`docs/parameter-tuning.md`](../../docs/parameter-tuning.md) for the
//!   simulation evidence behind the picks.
//!
//! [`aggora_crypto`]: ../aggora_crypto/index.html

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// 32-byte BLAKE3(public_key), hex-encoded. Used as primary key in the `wallets` sled tree.
pub type WalletId = String;
/// BLAKE3(operator_public_key), same derivation as `WalletId`.
pub type OperatorId = String;
/// BLAKE3(validator_public_key), same derivation as `WalletId`.
pub type ValidatorId = String;
/// 32-byte Ed25519 public key, hex-encoded.
pub type PublicKey = String;
/// 64-byte Ed25519 signature, hex-encoded.
pub type Signature = String;
/// 32-byte BLAKE3 digest, hex-encoded.
pub type Hash = String;
/// Atomic balance unit. 1 AGC = `MICRO_AGC_PER_AGC` µAGC, stored as `u64` everywhere.
pub type MicroAgc = u64;
/// Monotonic PoH tick index assigned by the state machine on every accepted transaction.
pub type Tick = u64;
/// Monotonic iteration counter. 0 at genesis, +1 per economic iteration.
pub type IterationId = u64;
/// Per-wallet (or per-operator-request) monotonic counter; the next expected value is stored
/// on the wallet itself so the state machine can reject replays without a global view.
pub type Nonce = u64;
/// Unix milliseconds. Used for both client-supplied timestamps and operator-signature drift.
pub type Timestamp = i64;

/// Conversion factor between user-facing AGC and the on-chain micro-AGC integer balance unit.
pub const MICRO_AGC_PER_AGC: MicroAgc = 1_000_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperatorRole {
    Genesis,
    Standard,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperatorAction {
    Add,
    Revoke,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operator {
    pub id: OperatorId,
    pub pubkey: PublicKey,
    pub role: OperatorRole,
    pub authorized_at: Tick,
    pub authorized_by: OperatorId,
    pub revoked_at: Option<Tick>,
    pub metadata: String,
}

impl Operator {
    pub fn active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// On-chain wallet record stored in the `wallets` sled tree.
///
/// The `iteration_*` fields accumulate activity inside the current iteration; the iteration
/// engine reads them to compute the activity score (EMA over iterations), then resets them.
/// `activity_score` therefore persists across iterations while the tx counters do not.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wallet {
    /// `WalletId = BLAKE3(pubkey)`; also the sled key.
    pub id: WalletId,
    /// Ed25519 public key the wallet signs transactions with.
    pub pubkey: PublicKey,
    /// Current balance in µAGC. Updated atomically under the system lock.
    pub balance: MicroAgc,
    /// Next expected nonce for outgoing transactions. Incremented after every transfer/burn.
    pub nonce: Nonce,
    pub created_at_tick: Tick,
    pub created_at_iteration: IterationId,
    pub created_by_operator: OperatorId,
    /// Number of transactions this wallet was involved in during the current iteration.
    pub iteration_tx_count: u32,
    /// Distinct counterparties this iteration — used by the activity threshold check.
    pub iteration_counterparties: BTreeSet<WalletId>,
    /// Iteration in which this wallet last participated in a transaction.
    pub last_active_iteration: IterationId,
    /// Activity EMA in [0,1]; multiplied into the redistribution weight `α`.
    pub activity_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintTx {
    pub tx_id: Hash,
    pub to: WalletId,
    pub amount: MicroAgc,
    pub eur_amount: u64,
    pub operator_id: OperatorId,
    pub nonce: u64,
    pub timestamp: Timestamp,
    pub operator_sig: Signature,
    pub category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferTx {
    pub tx_id: Hash,
    pub from: WalletId,
    pub to: WalletId,
    pub amount: MicroAgc,
    pub nonce: Nonce,
    pub timestamp: Timestamp,
    pub sender_pubkey: PublicKey,
    pub user_sig: Signature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BurnTx {
    pub tx_id: Hash,
    pub from: WalletId,
    pub amount: MicroAgc,
    pub nonce: Nonce,
    pub timestamp: Timestamp,
    pub sender_pubkey: PublicKey,
    pub user_sig: Signature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationCommit {
    pub tx_id: Hash,
    pub iteration_id: IterationId,
    pub triggered_at_tick: Tick,
    pub snapshot_supply: MicroAgc,
    pub snapshot_n_wallets: u64,
    pub snapshot_gini: f64,
    pub inflation: f64,
    pub burn_rate: f64,
    pub penalties: Vec<(WalletId, MicroAgc)>,
    pub rewards: Vec<(WalletId, MicroAgc)>,
    pub burned: MicroAgc,
    pub faucet_from_penalty: MicroAgc,
    pub faucet_from_mint: MicroAgc,
    pub new_wallets: Vec<WalletId>,
    pub post_supply: MicroAgc,
    pub validator_sigs: Vec<(ValidatorId, Signature)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorChangeTx {
    pub tx_id: Hash,
    pub action: OperatorAction,
    pub target_pubkey: PublicKey,
    pub timestamp: Timestamp,
    pub authorizing_op: OperatorId,
    pub operator_sig: Signature,
}

/// The externally-tagged Transaction enum is the on-chain unit the PoH stream commits to.
/// `kind: "mint" | "transfer" | "burn" | "iteration_commit" | "operator_change"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Transaction {
    /// Operator-signed mint (wallet seed or /charge top-up).
    Mint(MintTx),
    /// User-signed wallet-to-wallet transfer.
    Transfer(TransferTx),
    /// User-signed self-burn.
    Burn(BurnTx),
    /// System-generated commit recording the result of an economic iteration.
    IterationCommit(IterationCommit),
    /// v2 placeholder for adding/revoking operators; rejected in v1.
    OperatorChange(OperatorChangeTx),
}

impl Transaction {
    pub fn tx_id(&self) -> &str {
        match self {
            Transaction::Mint(tx) => &tx.tx_id,
            Transaction::Transfer(tx) => &tx.tx_id,
            Transaction::Burn(tx) => &tx.tx_id,
            Transaction::IterationCommit(tx) => &tx.tx_id,
            Transaction::OperatorChange(tx) => &tx.tx_id,
        }
    }

    pub fn wallet_ids(&self) -> Vec<&str> {
        match self {
            Transaction::Mint(tx) => vec![tx.to.as_str()],
            Transaction::Transfer(tx) => vec![tx.from.as_str(), tx.to.as_str()],
            Transaction::Burn(tx) => vec![tx.from.as_str()],
            Transaction::IterationCommit(tx) => tx
                .penalties
                .iter()
                .map(|(wallet_id, _)| wallet_id.as_str())
                .chain(tx.rewards.iter().map(|(wallet_id, _)| wallet_id.as_str()))
                .chain(tx.new_wallets.iter().map(String::as_str))
                .collect(),
            Transaction::OperatorChange(_) => vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PohEntry {
    pub tick: Tick,
    pub prev_hash: Hash,
    pub hash: Hash,
    pub tx_root: Hash,
    pub tx_ids: Vec<Hash>,
    pub leader_id: ValidatorId,
    pub leader_sig: Signature,
    pub wall_clock: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Validator {
    pub id: ValidatorId,
    pub pubkey: PublicKey,
    pub endpoint: String,
    pub is_genesis: bool,
    pub registered_at: Tick,
    pub active: bool,
    pub last_seen_tick: Tick,
}

/// Live, mutable system-wide state. Persisted as a single sled key (`system/state`) and
/// also held in memory by [`aggora_state::CoinState`] under an `RwLock` for hot reads.
///
/// Every mutating transaction updates `current_tick`, `last_poh_hash`, the wallet counters,
/// and `total_supply` *under the same write-lock acquisition*, which is what makes the chain
/// safe under concurrent REST traffic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemState {
    /// Strictly monotonic PoH tick index assigned by the state machine.
    pub current_tick: Tick,
    /// Iteration counter (0 at genesis, +1 per economic iteration).
    pub current_iteration: IterationId,
    /// Tick at which the current iteration started; used to detect when the next is due.
    pub iteration_started_at: Tick,
    /// Sum of all wallet balances. Maintained incrementally so `/stats` is O(1).
    pub total_supply: MicroAgc,
    /// Cached count of wallet records.
    pub n_wallets: u64,
    /// Wallets that have transacted in `current_iteration`. Reset to the new faucet wallets
    /// at iteration commit, then incremented in transfer/burn when a wallet first transacts.
    pub n_active_wallets: u64,
    pub last_poh_hash: Hash,
    // Supply at the *end* of the previous iteration (post-economics). Together with
    // `previous_iteration_start_supply` this lets the iteration engine measure how much the
    // previous cycle inflated the supply (spec D.4) and steer the adaptive burn rate.
    pub previous_iteration_supply: MicroAgc,
    // Supply at the *start* of the previous iteration (pre-economics snapshot).
    #[serde(default)]
    pub previous_iteration_start_supply: MicroAgc,
    pub last_inflation: f64,
    #[serde(default)]
    pub last_gini: f64,
    pub iteration_lock: bool,
}

impl Default for SystemState {
    fn default() -> Self {
        Self {
            current_tick: 0,
            current_iteration: 0,
            iteration_started_at: 0,
            total_supply: 0,
            n_wallets: 0,
            n_active_wallets: 0,
            last_poh_hash: "00".repeat(32),
            previous_iteration_supply: 0,
            previous_iteration_start_supply: 0,
            last_inflation: 0.0,
            last_gini: 0.0,
            iteration_lock: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemParameters {
    pub economy: EconomyParameters,
    pub growth: GrowthParameters,
    pub consensus: ConsensusParameters,
    pub iteration: IterationParameters,
    pub storage: StorageParameters,
    pub security: SecurityParameters,
    pub simulation: SimulationParameters,
}

impl Default for SystemParameters {
    fn default() -> Self {
        Self {
            economy: EconomyParameters::default(),
            growth: GrowthParameters::default(),
            consensus: ConsensusParameters::default(),
            iteration: IterationParameters::default(),
            storage: StorageParameters::default(),
            security: SecurityParameters::default(),
            simulation: SimulationParameters::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EconomyParameters {
    pub initial_seed_agc: u64,
    pub penalty_rate: f64,
    pub target_penalty_share_of_supply: f64,
    pub burn_base: f64,
    pub burn_sensitivity: f64,
    pub burn_min: f64,
    pub burn_max: f64,
    pub target_inflation_per_iter: f64,
    pub faucet_share_of_penalty: f64,
    pub redistribution_active_min: f64,
    pub redistribution_active_max: f64,
    pub activity_ema_lambda: f64,
    pub activity_min_tx_count: u32,
    pub activity_min_counterparties: u32,
    pub inverse_balance_weight: f64,
    pub residual_policy: ResidualPolicy,
}

impl Default for EconomyParameters {
    fn default() -> Self {
        Self {
            initial_seed_agc: 10,
            penalty_rate: 0.05,
            // Selected empirically over 24-iteration simulations (see docs/parameter-tuning.md):
            // 0.05 keeps the penalty pool large enough that adaptive burn can compensate the
            // faucet mint at sustainable wallet-growth rates, without crushing larger balances.
            target_penalty_share_of_supply: 0.05,
            burn_base: 0.10,
            burn_sensitivity: 0.5,
            burn_min: 0.0,
            burn_max: 0.9,
            target_inflation_per_iter: 0.02,
            faucet_share_of_penalty: 0.20,
            redistribution_active_min: 0.5,
            redistribution_active_max: 1.5,
            activity_ema_lambda: 0.5,
            activity_min_tx_count: 1,
            activity_min_counterparties: 1,
            // Mild inverse-balance tilt accelerates Gini reduction (0.13 -> 0.12 over 24 iter)
            // without breaking the activity-weighted incentive: γ=0.5 stayed stable in all sweeps.
            inverse_balance_weight: 0.5,
            residual_policy: ResidualPolicy::Burn,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResidualPolicy {
    Burn,
    GenesisWallet,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthParameters {
    pub growth_factor_per_iteration: f64,
    pub max_new_wallets_per_iter: u64,
    pub charge_eur_to_agc_ratio: f64,
}

impl Default for GrowthParameters {
    fn default() -> Self {
        Self {
            // 30% wallet growth per iteration is the spec's upper bound; in simulation it caused
            // ~20%-per-iteration hyperinflation because faucet mint scales with N while burn
            // capacity scales with target_penalty_share. 5% keeps inflation below the controller's
            // ability to react and is the production-recommended starting value.
            growth_factor_per_iteration: 0.05,
            max_new_wallets_per_iter: 1000,
            charge_eur_to_agc_ratio: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusParameters {
    pub n_validators: u64,
    pub poh_tick_ms: u64,
    pub ticks_per_slot: u64,
    pub max_txs_per_tick: usize,
    pub leader_rotation: String,
}

impl Default for ConsensusParameters {
    fn default() -> Self {
        Self {
            n_validators: 1,
            poh_tick_ms: 400,
            ticks_per_slot: 64,
            max_txs_per_tick: 256,
            leader_rotation: "round_robin".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IterationParameters {
    pub iterations_per_year: u64,
    pub ticks_per_iteration: u64,
}

impl Default for IterationParameters {
    fn default() -> Self {
        Self {
            iterations_per_year: 12,
            ticks_per_iteration: 6_566_400,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageParameters {
    pub db_path: String,
    pub snapshot_path: String,
    pub snapshot_per_iteration: bool,
    pub seed_path: String,
}

impl Default for StorageParameters {
    fn default() -> Self {
        Self {
            db_path: "./data/agc.sled".to_string(),
            snapshot_path: "./snapshots".to_string(),
            snapshot_per_iteration: true,
            seed_path: "./seeds".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityParameters {
    pub operator_pubkey_env: String,
    pub operator_secret_env: String,
    pub require_captcha_proof: bool,
    pub rate_limit_wallet_per_ip_per_day: u64,
    pub rate_limit_tx_per_wallet_per_minute: u64,
    pub timestamp_drift_ms: i64,
    pub dev_auth_bypass: bool,
}

impl Default for SecurityParameters {
    fn default() -> Self {
        Self {
            operator_pubkey_env: "AGORA_OPERATOR_PUBKEY".to_string(),
            operator_secret_env: "AGORA_OPERATOR_SECRET".to_string(),
            require_captcha_proof: true,
            rate_limit_wallet_per_ip_per_day: 5,
            rate_limit_tx_per_wallet_per_minute: 60,
            timestamp_drift_ms: 60_000,
            dev_auth_bypass: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationParameters {
    pub enabled: bool,
    pub speed_factor: u64,
    pub seed_file: String,
    pub auto_generate_users: bool,
    pub users_per_iteration_mean: f64,
    pub users_per_iteration_stddev: f64,
    pub tx_per_active_wallet_mean: f64,
}

impl Default for SimulationParameters {
    fn default() -> Self {
        Self {
            enabled: false,
            speed_factor: 1000,
            seed_file: "seeds/default_100_nodes.json".to_string(),
            auto_generate_users: true,
            users_per_iteration_mean: 80.0,
            users_per_iteration_stddev: 15.0,
            tx_per_active_wallet_mean: 5.0,
        }
    }
}

/// Schema for the JSON files in `seeds/`. The bootstrap loader walks `initial_wallets` and
/// installs them through the state machine before any operator/user request arrives. The
/// simulator additionally consumes `scripted_events` to inject reproducible population
/// changes and traffic bursts at specific iterations, which is what the spec's "scenario"
/// suite (J.4) needs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SeedFile {
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub initial_wallets: Vec<SeedWallet>,
    #[serde(default)]
    pub scripted_events: Vec<ScriptedEvent>,
}

/// One scripted scenario step. Each variant carries an `at_iteration` index that the
/// simulator dispatches against at the start of that iteration (before synthetic activity).
///
/// Unknown variants fall through `Unknown` so older code can still load forward-compatible
/// seed files without crashing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ScriptedEvent {
    /// Mint `count` additional faucet-style wallets at the given iteration. Each receives
    /// `balance` µAGC (defaults to the configured initial_seed).
    SpawnWallets {
        at_iteration: IterationId,
        count: u64,
        #[serde(default)]
        balance: Option<MicroAgc>,
    },
    /// Inject `n_txs` extra synthetic transfers between random wallets during the named
    /// iteration. Useful for stress-testing the activity-weighted redistribution.
    TransferBurst {
        at_iteration: IterationId,
        n_txs: u64,
        /// Fraction of sender balance moved per transfer. Defaults to the simulator's
        /// configured `transfer_fraction_mean` when omitted.
        #[serde(default)]
        fraction: Option<f64>,
    },
    /// Operator-style charge burst: mint `amount` µAGC total, split evenly across `n_targets`
    /// randomly-chosen existing wallets. Simulates external AGC inflows from /charge.
    ChargeBurst {
        at_iteration: IterationId,
        amount: MicroAgc,
        n_targets: u64,
    },
    /// Move `fraction` of every wallet's balance into a single randomly chosen wallet at the
    /// given iteration. Used to model a sudden wealth-concentration event for stress-testing
    /// the redistribution mechanism.
    WealthShock {
        at_iteration: IterationId,
        fraction: f64,
    },
    /// Forward-compat catch-all so unknown actions don't break old simulators.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedWallet {
    /// Ed25519 public key (hex or base64); the wallet id is derived from it.
    pub public_key: PublicKey,
    /// Initial balance in micro-AGC. If omitted, the configured initial seed is used.
    #[serde(default)]
    pub initial_balance: Option<MicroAgc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCreateRequest {
    pub public_key: PublicKey,
    pub captcha_token: String,
    pub timestamp: Timestamp,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChargeRequest {
    pub to: WalletId,
    pub amount: MicroAgc,
    pub eur_amount: u64,
    pub timestamp: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferRequest {
    pub from: WalletId,
    pub to: WalletId,
    pub amount: MicroAgc,
    pub nonce: Nonce,
    pub timestamp: Timestamp,
    pub sender_pubkey: PublicKey,
    pub user_sig: Signature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BurnRequest {
    pub from: WalletId,
    pub amount: MicroAgc,
    pub nonce: Nonce,
    pub timestamp: Timestamp,
    pub sender_pubkey: PublicKey,
    pub user_sig: Signature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletCreateResponse {
    pub wallet_id: WalletId,
    pub public_key: PublicKey,
    pub balance: MicroAgc,
    pub created_at_tick: Tick,
    pub tx_id: Hash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxAcceptedResponse {
    pub tx_id: Hash,
    pub tick: Tick,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsResponse {
    pub current_tick: Tick,
    pub current_iteration: IterationId,
    pub total_supply: MicroAgc,
    pub n_wallets: u64,
    pub n_active_wallets: u64,
    pub gini: f64,
    pub last_poh_hash: Hash,
    pub last_inflation: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupplyPoint {
    pub iteration: IterationId,
    pub supply: MicroAgc,
    pub burned: MicroAgc,
    pub faucet_from_mint: MicroAgc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GiniPoint {
    pub iteration: IterationId,
    pub gini: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationStatus {
    pub running: bool,
    pub mode: String,
    pub iterations_executed: u64,
}
