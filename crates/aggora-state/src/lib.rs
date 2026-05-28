//! Authoritative state machine for the chain.
//!
//! [`CoinState`] is the only component that mutates persistent state. It owns:
//!
//! - a [`CoinStorage`] handle (sled trees) for durable data,
//! - an async `RwLock<SystemState>` whose write half is the *single serialization point* for
//!   every mutating transaction, so PoH-tick ordering and balance updates can never race,
//! - an async `RwLock<SystemParameters>` so `/admin/parameters` can hot-swap the economic
//!   configuration,
//! - in-memory caches that the REST layer hits on the hot path:
//!     * `seen_operator_sigs` — replay guard for non-idempotent operator requests,
//!     * `rate_wallet_create` and `rate_wallet_tx` — sliding-window rate limiters.
//!
//! All public mutating methods follow the same pattern:
//! 1. **Outside the lock**: deserialize, normalize, compute `tx_id`, verify Ed25519
//!    signatures, and check rate limits. These are the expensive CPU operations and they
//!    don't need the lock.
//! 2. **Acquire `system.write()`**: from here on every read/validate/mutate is atomic. The
//!    wallet read, nonce/balance check, mutation, and write all happen inside the lock so
//!    concurrent transactions cannot double-spend.
//! 3. **Append the PoH entry** via [`append_tx_locked`], which bumps `current_tick`,
//!    rebuilds the chain hash, persists the transaction + secondary index + PoH entry, and
//!    leaves the updated `SystemState` for the caller to flush.
//!
//! See [`docs/features.md`](../../docs/features.md) for the end-to-end map of which REST
//! endpoint invokes which method here and which sled tree it ultimately writes to.

use aggora_crypto::{
    canonical_request_message, hash_serializable, normalize_bytes_to_hex, now_ms, operator_id_from_public_key,
    public_key_from_secret_hex, tx_signing_message, validator_id_from_public_key, verify_ed25519, wallet_id_from_public_key,
    ZERO_HASH_HEX,
};
use aggora_economy::{compute_gini, execute_iteration, micro_seed};
use aggora_poh::build_entry;
use aggora_storage::CoinStorage;
use aggora_types::*;
use anyhow::Result;
use serde::Serialize;
use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    env,
    net::IpAddr,
    sync::Arc,
};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

const DEV_OPERATOR_SECRET: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

#[derive(Debug, Error)]
pub enum StateError {
    #[error("invalid signature")]
    InvalidSignature,
    #[error("invalid nonce")]
    InvalidNonce,
    #[error("insufficient balance")]
    InsufficientBalance,
    #[error("wallet not found")]
    WalletNotFound,
    #[error("operator unauthorized")]
    OperatorUnauthorized,
    #[error("wallet already exists")]
    WalletAlreadyExists,
    #[error("iteration in progress")]
    IterationInProgress,
    #[error("rate limit exceeded")]
    RateLimitExceeded,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl StateError {
    pub fn code(&self) -> &'static str {
        match self {
            StateError::InvalidSignature => "INVALID_SIGNATURE",
            StateError::InvalidNonce => "INVALID_NONCE",
            StateError::InsufficientBalance => "INSUFFICIENT_BALANCE",
            StateError::WalletNotFound => "WALLET_NOT_FOUND",
            StateError::OperatorUnauthorized => "OPERATOR_UNAUTHORIZED",
            StateError::WalletAlreadyExists => "WALLET_ALREADY_EXISTS",
            StateError::IterationInProgress => "ITERATION_IN_PROGRESS",
            StateError::RateLimitExceeded => "RATE_LIMIT_EXCEEDED",
            StateError::BadRequest(_) => "BAD_REQUEST",
            StateError::Internal(_) => "INTERNAL_ERROR",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub parameters: SystemParameters,
    pub db_path: String,
    pub snapshot_path: String,
    pub operator_secret: Option<String>,
    pub operator_public_key: Option<String>,
    pub validator_endpoint: String,
}

impl NodeConfig {
    pub fn from_parameters(parameters: SystemParameters) -> Self {
        Self {
            db_path: parameters.storage.db_path.clone(),
            snapshot_path: parameters.storage.snapshot_path.clone(),
            parameters,
            operator_secret: None,
            operator_public_key: None,
            validator_endpoint: "127.0.0.1:8080".to_string(),
        }
    }
}

#[derive(Clone)]
pub struct CoinState {
    inner: Arc<Inner>,
}

struct Inner {
    storage: CoinStorage,
    parameters: RwLock<SystemParameters>,
    system: RwLock<SystemState>,
    operator_secret: Option<String>,
    operator_public_key: String,
    operator_id: String,
    validator_id: String,
    simulation: Mutex<SimulationStatus>,
    seen_operator_sigs: Mutex<HashMap<String, i64>>,
    // Sliding-window rate-limit state: per-IP wallet-create timestamps (day window) and
    // per-wallet outgoing-transfer timestamps (minute window). Bounded by the limits themselves
    // because old entries are pruned on every check.
    rate_wallet_create: Mutex<HashMap<IpAddr, VecDeque<i64>>>,
    rate_wallet_tx: Mutex<HashMap<String, VecDeque<i64>>>,
}

impl CoinState {
    pub async fn open(config: NodeConfig) -> Result<Self> {
        let storage = CoinStorage::open(&config.db_path, &config.snapshot_path)?;
        let parameters = storage.load_parameters()?.unwrap_or(config.parameters);
        storage.store_parameters(&parameters)?;

        let operator_secret = config
            .operator_secret
            .or_else(|| env::var(&parameters.security.operator_secret_env).ok())
            .or_else(|| env::var("AGGORA_COIN_OPERATOR_SECRET").ok())
            .or_else(|| Some(DEV_OPERATOR_SECRET.to_string()));
        let operator_public_key = config
            .operator_public_key
            .or_else(|| env::var(&parameters.security.operator_pubkey_env).ok())
            .or_else(|| env::var("AGGORA_COIN_OPERATOR_PUBKEY").ok())
            .or_else(|| operator_secret.as_deref().and_then(|secret| public_key_from_secret_hex(secret).ok()))
            .ok_or_else(|| anyhow::anyhow!("operator public key could not be derived"))?;
        let operator_public_key = normalize_bytes_to_hex(&operator_public_key, 32)?;
        let operator_id = operator_id_from_public_key(&operator_public_key)?;
        let validator_id = validator_id_from_public_key(&operator_public_key)?;

        let mut system = storage.load_state()?.unwrap_or_default();
        if system.last_poh_hash.is_empty() {
            system.last_poh_hash = ZERO_HASH_HEX.to_string();
        }
        storage.store_state(&system)?;

        let genesis_operator = Operator {
            id: operator_id.clone(),
            pubkey: operator_public_key.clone(),
            role: OperatorRole::Genesis,
            authorized_at: 0,
            authorized_by: operator_id.clone(),
            revoked_at: None,
            metadata: "Aggora Coin Genesis Operator".to_string(),
        };
        storage.put_operator(&genesis_operator)?;
        storage.put_validator(&Validator {
            id: validator_id.clone(),
            pubkey: operator_public_key.clone(),
            endpoint: config.validator_endpoint,
            is_genesis: true,
            registered_at: 0,
            active: true,
            last_seen_tick: system.current_tick,
        })?;
        storage.flush()?;

        Ok(Self {
            inner: Arc::new(Inner {
                storage,
                parameters: RwLock::new(parameters),
                system: RwLock::new(system),
                operator_secret,
                operator_public_key,
                operator_id,
                validator_id,
                simulation: Mutex::new(SimulationStatus {
                    running: false,
                    mode: "idle".to_string(),
                    iterations_executed: 0,
                }),
                seen_operator_sigs: Mutex::new(HashMap::new()),
                rate_wallet_create: Mutex::new(HashMap::new()),
                rate_wallet_tx: Mutex::new(HashMap::new()),
            }),
        })
    }

    pub fn storage(&self) -> &CoinStorage {
        &self.inner.storage
    }

    pub fn operator_public_key(&self) -> &str {
        &self.inner.operator_public_key
    }

    pub fn operator_id(&self) -> &str {
        &self.inner.operator_id
    }

    pub async fn parameters(&self) -> SystemParameters {
        self.inner.parameters.read().await.clone()
    }

    pub async fn system(&self) -> SystemState {
        self.inner.system.read().await.clone()
    }

    pub async fn update_parameters(&self, parameters: SystemParameters) -> Result<(), StateError> {
        self.inner.storage.store_parameters(&parameters)?;
        *self.inner.parameters.write().await = parameters;
        Ok(())
    }

    pub async fn verify_operator_request(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        timestamp: Option<i64>,
        signature: Option<&str>,
        operator_id_header: Option<&str>,
        operator_public_header: Option<&str>,
        admin_only: bool,
    ) -> Result<Operator, StateError> {
        let parameters = self.parameters().await;
        if parameters.security.dev_auth_bypass && signature.is_none() {
            return self
                .inner
                .storage
                .get_operator(&self.inner.operator_id)?
                .ok_or(StateError::OperatorUnauthorized);
        }

        let timestamp = timestamp.ok_or(StateError::InvalidSignature)?;
        if (now_ms() - timestamp).abs() > parameters.security.timestamp_drift_ms {
            return Err(StateError::InvalidSignature);
        }
        let signature = signature.ok_or(StateError::InvalidSignature)?;
        let public_key = if let Some(public_key) = operator_public_header {
            normalize_bytes_to_hex(public_key, 32).map_err(|_| StateError::InvalidSignature)?
        } else if let Some(operator_id) = operator_id_header {
            self.inner
                .storage
                .get_operator(operator_id)?
                .ok_or(StateError::OperatorUnauthorized)?
                .pubkey
        } else {
            return Err(StateError::OperatorUnauthorized);
        };
        let operator_id = operator_id_from_public_key(&public_key).map_err(|_| StateError::InvalidSignature)?;
        if let Some(header_id) = operator_id_header {
            if header_id != operator_id {
                return Err(StateError::OperatorUnauthorized);
            }
        }
        let operator = self
            .inner
            .storage
            .get_operator(&operator_id)?
            .ok_or(StateError::OperatorUnauthorized)?;
        if !operator.active() || operator.pubkey != public_key {
            return Err(StateError::OperatorUnauthorized);
        }
        if admin_only && operator.role != OperatorRole::Genesis {
            return Err(StateError::OperatorUnauthorized);
        }
        let message = canonical_request_message(method, path, timestamp, body);
        let valid = verify_ed25519(&public_key, &message, signature).map_err(|_| StateError::InvalidSignature)?;
        if !valid {
            return Err(StateError::InvalidSignature);
        }
        Ok(operator)
    }

    /// Sliding-window per-IP rate limit for wallet creation (spec K.1: anti-Sybil).
    ///
    /// Records the current timestamp and rejects if more than `limit` creates have occurred in
    /// the trailing 24h window. `None` for `ip` (e.g. unix sockets) skips the limit.
    pub async fn check_wallet_rate_limit(&self, ip: Option<IpAddr>) -> Result<(), StateError> {
        let Some(ip) = ip else { return Ok(()) };
        let limit = self.parameters().await.security.rate_limit_wallet_per_ip_per_day;
        if limit == 0 {
            return Ok(());
        }
        let window_ms: i64 = 24 * 60 * 60 * 1000;
        let now = now_ms();
        let mut guard = self.inner.rate_wallet_create.lock().await;
        let entry = guard.entry(ip).or_default();
        while entry.front().map_or(false, |ts| now - *ts > window_ms) {
            entry.pop_front();
        }
        if entry.len() as u64 >= limit {
            return Err(StateError::RateLimitExceeded);
        }
        entry.push_back(now);
        Ok(())
    }

    /// Sliding-window per-wallet rate limit for outgoing transfers.
    pub async fn check_transfer_rate_limit(&self, wallet_id: &str) -> Result<(), StateError> {
        let limit = self.parameters().await.security.rate_limit_tx_per_wallet_per_minute;
        if limit == 0 {
            return Ok(());
        }
        let window_ms: i64 = 60 * 1000;
        let now = now_ms();
        let mut guard = self.inner.rate_wallet_tx.lock().await;
        let entry = guard.entry(wallet_id.to_string()).or_default();
        while entry.front().map_or(false, |ts| now - *ts > window_ms) {
            entry.pop_front();
        }
        if entry.len() as u64 >= limit {
            return Err(StateError::RateLimitExceeded);
        }
        entry.push_back(now);
        Ok(())
    }

    /// Rejects a replayed operator signature within the timestamp-drift window.
    ///
    /// Operator requests are authenticated over (method, path, timestamp, body) only, so an
    /// identical signed request is byte-for-byte replayable until its timestamp drifts out of
    /// the accepted window. For non-idempotent mints (`/charge`) that would mean a double mint,
    /// so we keep a short-lived in-memory set of seen signatures and prune expired entries.
    async fn guard_operator_replay(&self, signature: &str, timestamp: i64) -> Result<(), StateError> {
        let drift = self.parameters().await.security.timestamp_drift_ms.max(0);
        let now = now_ms();
        let mut seen = self.inner.seen_operator_sigs.lock().await;
        seen.retain(|_, ts| (now - *ts).abs() <= drift);
        if seen.contains_key(signature) {
            return Err(StateError::InvalidSignature);
        }
        seen.insert(signature.to_string(), timestamp);
        Ok(())
    }

    pub async fn create_wallet(
        &self,
        request: WalletCreateRequest,
        operator: &Operator,
        operator_sig: String,
    ) -> Result<WalletCreateResponse, StateError> {
        let parameters = self.parameters().await;
        if parameters.security.require_captcha_proof && request.captcha_token.trim().is_empty() {
            return Err(StateError::BadRequest("captcha_token is required".to_string()));
        }
        let public_key = normalize_bytes_to_hex(&request.public_key, 32).map_err(|err| StateError::BadRequest(err.to_string()))?;
        let wallet_id = wallet_id_from_public_key(&public_key).map_err(|err| StateError::BadRequest(err.to_string()))?;

        let mut system = self.inner.system.write().await;
        if system.iteration_lock {
            return Err(StateError::IterationInProgress);
        }
        // Existence check must hold the same lock as the insert, otherwise two concurrent
        // creates for the same public key could both pass the check (TOCTOU).
        if self.inner.storage.get_wallet(&wallet_id)?.is_some() {
            return Err(StateError::WalletAlreadyExists);
        }
        let tick = system.current_tick + 1;
        let amount = micro_seed(&parameters);
        let wallet = Wallet {
            id: wallet_id.clone(),
            pubkey: public_key.clone(),
            balance: amount,
            nonce: 0,
            created_at_tick: tick,
            created_at_iteration: system.current_iteration,
            created_by_operator: operator.id.clone(),
            iteration_tx_count: 0,
            iteration_counterparties: BTreeSet::new(),
            last_active_iteration: system.current_iteration,
            activity_score: 1.0,
        };
        let tx_id = mint_tx_id(&wallet_id, amount, 0, &operator.id, tick, request.timestamp, "wallet_seed")?;
        let tx = Transaction::Mint(MintTx {
            tx_id: tx_id.clone(),
            to: wallet_id.clone(),
            amount,
            eur_amount: 0,
            operator_id: operator.id.clone(),
            nonce: tick,
            timestamp: request.timestamp,
            operator_sig,
            category: "wallet_seed".to_string(),
        });
        self.inner.storage.put_wallet(&wallet)?;
        system.total_supply = system.total_supply.saturating_add(amount);
        system.n_wallets = system.n_wallets.saturating_add(1);
        system.n_active_wallets = system.n_active_wallets.saturating_add(1);
        self.append_tx_locked(&mut system, &tx)?;
        self.inner.storage.store_state(&system)?;
        Ok(WalletCreateResponse {
            wallet_id,
            public_key,
            balance: amount,
            created_at_tick: tick,
            tx_id,
        })
    }

    pub async fn charge(
        &self,
        request: ChargeRequest,
        operator: &Operator,
        operator_sig: String,
    ) -> Result<TxAcceptedResponse, StateError> {
        if request.amount == 0 {
            return Err(StateError::BadRequest("amount must be greater than zero".to_string()));
        }
        // Minting is non-idempotent: reject a replayed operator signature before applying it.
        self.guard_operator_replay(&operator_sig, request.timestamp).await?;
        let mut system = self.inner.system.write().await;
        if system.iteration_lock {
            return Err(StateError::IterationInProgress);
        }
        let mut wallet = self
            .inner
            .storage
            .get_wallet(&request.to)?
            .ok_or(StateError::WalletNotFound)?;
        let tick = system.current_tick + 1;
        wallet.balance = wallet.balance.saturating_add(request.amount);
        let tx_id = mint_tx_id(
            &request.to,
            request.amount,
            request.eur_amount,
            &operator.id,
            tick,
            request.timestamp,
            "charge",
        )?;
        let tx = Transaction::Mint(MintTx {
            tx_id: tx_id.clone(),
            to: request.to,
            amount: request.amount,
            eur_amount: request.eur_amount,
            operator_id: operator.id.clone(),
            nonce: tick,
            timestamp: request.timestamp,
            operator_sig,
            category: "charge".to_string(),
        });
        self.inner.storage.put_wallet(&wallet)?;
        system.total_supply = system.total_supply.saturating_add(request.amount);
        self.append_tx_locked(&mut system, &tx)?;
        self.inner.storage.store_state(&system)?;
        Ok(TxAcceptedResponse { tx_id, tick })
    }

    pub async fn transfer(&self, request: TransferRequest) -> Result<TxAcceptedResponse, StateError> {
        if request.from == request.to {
            return Err(StateError::BadRequest("from and to must differ".to_string()));
        }
        if request.amount == 0 {
            return Err(StateError::BadRequest("amount must be greater than zero".to_string()));
        }
        let public_wallet_id = wallet_id_from_public_key(&request.sender_pubkey).map_err(|_| StateError::InvalidSignature)?;
        if public_wallet_id != request.from {
            return Err(StateError::InvalidSignature);
        }
        let tx_id = transfer_tx_id(&request)?;
        let sender_pubkey = normalize_bytes_to_hex(&request.sender_pubkey, 32).map_err(|_| StateError::InvalidSignature)?;
        self.verify_user_signature(&request.sender_pubkey, &tx_id, &request.user_sig).await?;

        // Read-validate-mutate must all happen under the same lock: reading balance/nonce
        // before locking would let two concurrent transfers from one wallet both pass the
        // checks against the same stale snapshot and double-spend.
        let mut system = self.inner.system.write().await;
        if system.iteration_lock {
            return Err(StateError::IterationInProgress);
        }
        let mut from_wallet = self
            .inner
            .storage
            .get_wallet(&request.from)?
            .ok_or(StateError::WalletNotFound)?;
        let mut to_wallet = self
            .inner
            .storage
            .get_wallet(&request.to)?
            .ok_or(StateError::WalletNotFound)?;
        if from_wallet.nonce != request.nonce {
            return Err(StateError::InvalidNonce);
        }
        if from_wallet.balance < request.amount {
            return Err(StateError::InsufficientBalance);
        }
        let tick = system.current_tick + 1;
        let iteration = system.current_iteration;
        let mut newly_active = 0u64;
        if from_wallet.last_active_iteration != iteration {
            newly_active += 1;
        }
        if to_wallet.last_active_iteration != iteration {
            newly_active += 1;
        }
        from_wallet.balance -= request.amount;
        from_wallet.nonce += 1;
        from_wallet.iteration_tx_count += 1;
        from_wallet.iteration_counterparties.insert(request.to.clone());
        from_wallet.last_active_iteration = iteration;
        to_wallet.balance = to_wallet.balance.saturating_add(request.amount);
        to_wallet.iteration_tx_count += 1;
        to_wallet.iteration_counterparties.insert(request.from.clone());
        to_wallet.last_active_iteration = iteration;

        let tx = Transaction::Transfer(TransferTx {
            tx_id: tx_id.clone(),
            from: request.from,
            to: request.to,
            amount: request.amount,
            nonce: request.nonce,
            timestamp: request.timestamp,
            sender_pubkey,
            user_sig: request.user_sig,
        });
        self.inner.storage.put_wallet(&from_wallet)?;
        self.inner.storage.put_wallet(&to_wallet)?;
        self.append_tx_locked(&mut system, &tx)?;
        system.n_active_wallets = system.n_active_wallets.saturating_add(newly_active);
        self.inner.storage.store_state(&system)?;
        Ok(TxAcceptedResponse { tx_id, tick })
    }

    pub async fn burn(&self, request: BurnRequest) -> Result<TxAcceptedResponse, StateError> {
        if request.amount == 0 {
            return Err(StateError::BadRequest("amount must be greater than zero".to_string()));
        }
        let public_wallet_id = wallet_id_from_public_key(&request.sender_pubkey).map_err(|_| StateError::InvalidSignature)?;
        if public_wallet_id != request.from {
            return Err(StateError::InvalidSignature);
        }
        let tx_id = burn_tx_id(&request)?;
        let sender_pubkey = normalize_bytes_to_hex(&request.sender_pubkey, 32).map_err(|_| StateError::InvalidSignature)?;
        self.verify_user_signature(&request.sender_pubkey, &tx_id, &request.user_sig).await?;

        let mut system = self.inner.system.write().await;
        if system.iteration_lock {
            return Err(StateError::IterationInProgress);
        }
        let mut wallet = self
            .inner
            .storage
            .get_wallet(&request.from)?
            .ok_or(StateError::WalletNotFound)?;
        if wallet.nonce != request.nonce {
            return Err(StateError::InvalidNonce);
        }
        if wallet.balance < request.amount {
            return Err(StateError::InsufficientBalance);
        }
        let tick = system.current_tick + 1;
        let iteration = system.current_iteration;
        let newly_active = if wallet.last_active_iteration != iteration { 1 } else { 0 };
        wallet.balance -= request.amount;
        wallet.nonce += 1;
        wallet.iteration_tx_count += 1;
        wallet.last_active_iteration = iteration;
        let tx = Transaction::Burn(BurnTx {
            tx_id: tx_id.clone(),
            from: request.from,
            amount: request.amount,
            nonce: request.nonce,
            timestamp: request.timestamp,
            sender_pubkey,
            user_sig: request.user_sig,
        });
        self.inner.storage.put_wallet(&wallet)?;
        system.total_supply = system.total_supply.saturating_sub(request.amount);
        self.append_tx_locked(&mut system, &tx)?;
        system.n_active_wallets = system.n_active_wallets.saturating_add(newly_active);
        self.inner.storage.store_state(&system)?;
        Ok(TxAcceptedResponse { tx_id, tick })
    }

    pub async fn execute_iteration_now(&self) -> Result<IterationCommit, StateError> {
        let parameters = self.parameters().await;
        let mut system = self.inner.system.write().await;
        if system.iteration_lock {
            return Err(StateError::IterationInProgress);
        }
        system.iteration_lock = true;
        self.inner.storage.store_state(&system)?;

        let wallets = self.inner.storage.list_wallets()?;
        let result = execute_iteration(wallets, &parameters, &system, &self.inner.operator_id)?;
        for wallet in &result.wallets {
            self.inner.storage.put_wallet(wallet)?;
        }
        let tx = Transaction::IterationCommit(result.commit.clone());
        system.total_supply = result.commit.post_supply;
        system.n_wallets = result.wallets.len() as u64;
        // Activity counters reset each iteration; the only wallets already "active" in the new
        // iteration are the freshly faucet-seeded ones (created_at_iteration == new iteration).
        system.n_active_wallets = result.commit.new_wallets.len() as u64;
        system.current_iteration = result.commit.iteration_id;
        system.iteration_started_at = system.current_tick;
        // Record both ends of this cycle so the next iteration can measure the inflation it
        // produced (spec D.4): start = pre-economics snapshot, end = post-economics supply.
        system.previous_iteration_start_supply = result.commit.snapshot_supply;
        system.previous_iteration_supply = result.commit.post_supply;
        system.last_inflation = result.commit.inflation;
        system.last_gini = compute_gini(&result.wallets);
        system.iteration_lock = false;
        self.append_tx_locked(&mut system, &tx)?;
        self.inner.storage.put_iteration(&result.commit)?;
        self.inner.storage.store_state(&system)?;
        if parameters.storage.snapshot_per_iteration {
            let snapshot = serde_json::json!({
                "iteration_id": result.commit.iteration_id,
                "tick": system.current_tick,
                "timestamp": now_ms(),
                "parameters": parameters,
                "system_state": &*system,
                "wallets": result.wallets,
                "poh_anchor": system.last_poh_hash,
            });
            self.inner.storage.write_snapshot(result.commit.iteration_id, &snapshot)?;
        }
        self.inner.storage.flush()?;
        Ok(result.commit)
    }

    /// Bootstraps the chain from a seed file: every `SeedWallet` is installed as if it had been
    /// minted by the genesis operator. Skips wallets whose ids already exist so the call is
    /// idempotent across restarts. Returns the number of newly created wallets.
    ///
    /// This is the admin bootstrap path documented under "scripted seed replay" in the spec;
    /// it deliberately bypasses operator-signature verification because the seed file itself
    /// is the authority (it has to be trusted before any operator request can be served).
    pub async fn install_seed(&self, seed: SeedFile) -> Result<usize, StateError> {
        let parameters = self.parameters().await;
        let default_seed = micro_seed(&parameters);
        let mut system = self.inner.system.write().await;
        if system.iteration_lock {
            return Err(StateError::IterationInProgress);
        }
        let mut installed = 0usize;
        for entry in seed.initial_wallets {
            let public_key = normalize_bytes_to_hex(&entry.public_key, 32)
                .map_err(|err| StateError::BadRequest(err.to_string()))?;
            let wallet_id = wallet_id_from_public_key(&public_key)
                .map_err(|err| StateError::BadRequest(err.to_string()))?;
            if self.inner.storage.get_wallet(&wallet_id)?.is_some() {
                continue;
            }
            let amount = entry.initial_balance.unwrap_or(default_seed);
            let tick = system.current_tick + 1;
            let wallet = Wallet {
                id: wallet_id.clone(),
                pubkey: public_key,
                balance: amount,
                nonce: 0,
                created_at_tick: tick,
                created_at_iteration: system.current_iteration,
                created_by_operator: self.inner.operator_id.clone(),
                iteration_tx_count: 0,
                iteration_counterparties: BTreeSet::new(),
                last_active_iteration: system.current_iteration,
                activity_score: 1.0,
            };
            let tx_id = mint_tx_id(
                &wallet_id,
                amount,
                0,
                &self.inner.operator_id,
                tick,
                now_ms(),
                "seed_bootstrap",
            )?;
            let tx = Transaction::Mint(MintTx {
                tx_id,
                to: wallet_id,
                amount,
                eur_amount: 0,
                operator_id: self.inner.operator_id.clone(),
                nonce: tick,
                timestamp: now_ms(),
                operator_sig: String::new(),
                category: "seed_bootstrap".to_string(),
            });
            self.inner.storage.put_wallet(&wallet)?;
            system.total_supply = system.total_supply.saturating_add(amount);
            system.n_wallets = system.n_wallets.saturating_add(1);
            system.n_active_wallets = system.n_active_wallets.saturating_add(1);
            self.append_tx_locked(&mut system, &tx)?;
            installed += 1;
        }
        self.inner.storage.store_state(&system)?;
        self.inner.storage.flush()?;
        Ok(installed)
    }

    pub async fn create_manual_snapshot(&self) -> Result<serde_json::Value, StateError> {
        let system = self.system().await;
        let parameters = self.parameters().await;
        let wallets = self.inner.storage.list_wallets()?;
        let payload = serde_json::json!({
            "iteration_id": system.current_iteration,
            "tick": system.current_tick,
            "timestamp": now_ms(),
            "parameters": parameters,
            "system_state": system,
            "wallets": wallets,
        });
        self.inner.storage.write_snapshot(system.current_iteration, &payload)?;
        Ok(payload)
    }

    /// O(1) stats served entirely from the in-memory system state. The Gini value is the one
    /// computed at the last iteration commit; recomputing it live would require a full wallet
    /// scan on every `/stats` call and every WebSocket tick, which does not scale.
    pub async fn stats(&self) -> Result<StatsResponse, StateError> {
        let system = self.system().await;
        Ok(StatsResponse {
            current_tick: system.current_tick,
            current_iteration: system.current_iteration,
            total_supply: system.total_supply,
            n_wallets: system.n_wallets,
            n_active_wallets: system.n_active_wallets,
            gini: system.last_gini,
            last_poh_hash: system.last_poh_hash,
            last_inflation: system.last_inflation,
        })
    }

    /// Exact Gini over the live wallet set (full scan). Use for diagnostics, not hot paths.
    pub async fn live_gini(&self) -> Result<f64, StateError> {
        Ok(compute_gini(&self.inner.storage.list_wallets()?))
    }

    pub async fn simulation_status(&self) -> SimulationStatus {
        self.inner.simulation.lock().await.clone()
    }

    pub async fn set_simulation_running(&self, running: bool, mode: &str) -> SimulationStatus {
        let mut status = self.inner.simulation.lock().await;
        status.running = running;
        status.mode = mode.to_string();
        status.clone()
    }

    pub async fn run_simulation_iterations(&self, iterations: u64) -> Result<SimulationStatus, StateError> {
        {
            let mut status = self.inner.simulation.lock().await;
            status.running = true;
            status.mode = "manual".to_string();
        }
        for _ in 0..iterations {
            self.execute_iteration_now().await?;
            let mut status = self.inner.simulation.lock().await;
            status.iterations_executed += 1;
        }
        let mut status = self.inner.simulation.lock().await;
        status.running = false;
        Ok(status.clone())
    }

    async fn verify_user_signature(&self, public_key: &str, tx_id: &str, signature: &str) -> Result<(), StateError> {
        let parameters = self.parameters().await;
        if parameters.security.dev_auth_bypass && signature.trim().is_empty() {
            return Ok(());
        }
        let message = tx_signing_message(tx_id).map_err(|_| StateError::InvalidSignature)?;
        let valid = verify_ed25519(public_key, &message, signature).map_err(|_| StateError::InvalidSignature)?;
        if !valid {
            return Err(StateError::InvalidSignature);
        }
        Ok(())
    }

    fn append_tx_locked(&self, system: &mut SystemState, tx: &Transaction) -> Result<(), StateError> {
        system.current_tick = system.current_tick.saturating_add(1);
        let entry = build_entry(
            system.current_tick,
            system.last_poh_hash.clone(),
            vec![tx.tx_id().to_string()],
            self.inner.validator_id.clone(),
            self.inner.operator_secret.as_deref(),
        )?;
        system.last_poh_hash = entry.hash.clone();
        self.inner.storage.put_transaction(tx, system.current_tick)?;
        self.inner.storage.put_poh_entry(&entry)?;
        Ok(())
    }
}

fn mint_tx_id(
    to: &str,
    amount: MicroAgc,
    eur_amount: u64,
    operator_id: &str,
    nonce: u64,
    timestamp: Timestamp,
    category: &str,
) -> Result<String, StateError> {
    #[derive(Serialize)]
    struct Payload<'a> {
        to: &'a str,
        amount: MicroAgc,
        eur_amount: u64,
        operator_id: &'a str,
        nonce: u64,
        timestamp: Timestamp,
        category: &'a str,
    }
    Ok(hash_serializable(&Payload {
        to,
        amount,
        eur_amount,
        operator_id,
        nonce,
        timestamp,
        category,
    })?)
}

fn transfer_tx_id(request: &TransferRequest) -> Result<String, StateError> {
    #[derive(Serialize)]
    struct Payload<'a> {
        from: &'a str,
        to: &'a str,
        amount: MicroAgc,
        nonce: Nonce,
        timestamp: Timestamp,
        sender_pubkey: String,
    }
    Ok(hash_serializable(&Payload {
        from: &request.from,
        to: &request.to,
        amount: request.amount,
        nonce: request.nonce,
        timestamp: request.timestamp,
        sender_pubkey: normalize_bytes_to_hex(&request.sender_pubkey, 32).map_err(|_| StateError::InvalidSignature)?,
    })?)
}

fn burn_tx_id(request: &BurnRequest) -> Result<String, StateError> {
    #[derive(Serialize)]
    struct Payload<'a> {
        from: &'a str,
        amount: MicroAgc,
        nonce: Nonce,
        timestamp: Timestamp,
        sender_pubkey: String,
    }
    Ok(hash_serializable(&Payload {
        from: &request.from,
        amount: request.amount,
        nonce: request.nonce,
        timestamp: request.timestamp,
        sender_pubkey: normalize_bytes_to_hex(&request.sender_pubkey, 32).map_err(|_| StateError::InvalidSignature)?,
    })?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn boots_with_dev_operator() {
        let mut params = SystemParameters::default();
        params.storage.db_path = format!("/tmp/aggora-coin-test-{}", now_ms());
        params.storage.snapshot_path = format!("/tmp/aggora-coin-snapshots-{}", now_ms());
        params.security.require_captcha_proof = false;
        let state = CoinState::open(NodeConfig::from_parameters(params)).await.unwrap();
        assert!(!state.operator_id().is_empty());
    }
}
