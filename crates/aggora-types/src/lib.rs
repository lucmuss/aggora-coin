use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub type WalletId = String;
pub type OperatorId = String;
pub type ValidatorId = String;
pub type PublicKey = String;
pub type Signature = String;
pub type Hash = String;
pub type MicroAgc = u64;
pub type Tick = u64;
pub type IterationId = u64;
pub type Nonce = u64;
pub type Timestamp = i64;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wallet {
    pub id: WalletId,
    pub pubkey: PublicKey,
    pub balance: MicroAgc,
    pub nonce: Nonce,
    pub created_at_tick: Tick,
    pub created_at_iteration: IterationId,
    pub created_by_operator: OperatorId,
    pub iteration_tx_count: u32,
    pub iteration_counterparties: BTreeSet<WalletId>,
    pub last_active_iteration: IterationId,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Transaction {
    Mint(MintTx),
    Transfer(TransferTx),
    Burn(BurnTx),
    IterationCommit(IterationCommit),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemState {
    pub current_tick: Tick,
    pub current_iteration: IterationId,
    pub iteration_started_at: Tick,
    pub total_supply: MicroAgc,
    pub n_wallets: u64,
    pub n_active_wallets: u64,
    pub last_poh_hash: Hash,
    pub previous_iteration_supply: MicroAgc,
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
            target_penalty_share_of_supply: 0.03,
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
            inverse_balance_weight: 0.0,
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
            growth_factor_per_iteration: 0.30,
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
