use aggora_crypto::{canonical_request_message, operator_id_from_public_key, public_key_from_secret_hex, sign_with_secret_hex};
use aggora_economy::{run_simulation, SimConfig, SimMetrics};
use aggora_rest::serve;
use aggora_state::{CoinState, NodeConfig};
use aggora_types::SystemParameters;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ed25519_dalek::SigningKey;
use rand::{rngs::OsRng, RngCore};
use std::{fs, net::SocketAddr, path::PathBuf};
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Debug, Parser)]
#[command(name = "aggora-node", version, about = "Aggora Coin node, simulator, and admin utility")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the REST gateway and single-validator state machine.
    Node {
        #[arg(long, env = "AGGORA_COIN_BIND", default_value = "0.0.0.0:8080")]
        bind: SocketAddr,
        #[arg(long, env = "AGGORA_COIN_CONFIG", default_value = "config/default.toml")]
        config: PathBuf,
        #[arg(long, env = "AGGORA_COIN_DB_PATH")]
        db_path: Option<String>,
        #[arg(long, env = "AGGORA_COIN_SNAPSHOT_PATH")]
        snapshot_path: Option<String>,
    },
    /// Generate an Ed25519 operator or wallet keypair.
    Keygen,
    /// Sign an API request in the same canonical format used by the REST gateway.
    SignRequest {
        #[arg(long)]
        secret: String,
        #[arg(long)]
        method: String,
        #[arg(long)]
        path: String,
        #[arg(long)]
        timestamp: i64,
        #[arg(long, default_value = "")]
        body: String,
    },
    /// Run N economic iterations against the configured local database and exit.
    Sim {
        #[arg(long, default_value_t = 24)]
        iterations: u64,
        #[arg(long, env = "AGGORA_COIN_CONFIG", default_value = "config/default.toml")]
        config: PathBuf,
    },
    /// Run a deterministic synthetic economic simulation and print per-iteration CSV metrics.
    ///
    /// This drives the real iteration engine over a synthetic population so the economic model
    /// can be validated and tuned. Parameter flags override the loaded config for the run.
    Simulate {
        #[arg(long, env = "AGGORA_COIN_CONFIG", default_value = "config/default.toml")]
        config: PathBuf,
        #[arg(long, default_value_t = 24)]
        iterations: u64,
        #[arg(long, default_value_t = 100)]
        initial_wallets: u64,
        #[arg(long, default_value_t = 0.8)]
        wealth_sigma: f64,
        #[arg(long, default_value_t = 5.0)]
        tx_per_wallet: f64,
        #[arg(long, default_value_t = 0.05)]
        transfer_fraction: f64,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Override economy.growth_factor_per_iteration.
        #[arg(long)]
        growth_factor: Option<f64>,
        /// Override economy.penalty_rate.
        #[arg(long)]
        penalty_rate: Option<f64>,
        /// Override economy.target_penalty_share_of_supply.
        #[arg(long)]
        target_penalty_share: Option<f64>,
        /// Override economy.faucet_share_of_penalty.
        #[arg(long)]
        faucet_share: Option<f64>,
        /// Override economy.burn_base.
        #[arg(long)]
        burn_base: Option<f64>,
        /// Override economy.inverse_balance_weight.
        #[arg(long)]
        inverse_balance_weight: Option<f64>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    fmt().with_env_filter(EnvFilter::from_default_env()).init();
    let cli = Cli::parse();
    match cli.command {
        Command::Node {
            bind,
            config,
            db_path,
            snapshot_path,
        } => {
            let mut parameters = load_parameters(&config)?;
            if let Some(db_path) = db_path {
                parameters.storage.db_path = db_path;
            }
            if let Some(snapshot_path) = snapshot_path {
                parameters.storage.snapshot_path = snapshot_path;
            }
            let state = CoinState::open(NodeConfig::from_parameters(parameters)).await?;
            serve(state, bind).await?;
        }
        Command::Keygen => {
            let mut seed = [0u8; 32];
            OsRng.fill_bytes(&mut seed);
            let signing_key = SigningKey::from_bytes(&seed);
            let secret = hex::encode(seed);
            let public = hex::encode(signing_key.verifying_key().to_bytes());
            let id = operator_id_from_public_key(&public)?;
            println!("secret={secret}");
            println!("public={public}");
            println!("id={id}");
        }
        Command::SignRequest {
            secret,
            method,
            path,
            timestamp,
            body,
        } => {
            let public = public_key_from_secret_hex(&secret)?;
            let id = operator_id_from_public_key(&public)?;
            let message = canonical_request_message(&method, &path, timestamp, body.as_bytes());
            let signature = sign_with_secret_hex(&secret, &message)?;
            println!("X-Operator-Id: {id}");
            println!("X-Operator-Public-Key: {public}");
            println!("X-Operator-Timestamp: {timestamp}");
            println!("X-Operator-Signature: {signature}");
        }
        Command::Sim { iterations, config } => {
            let parameters = load_parameters(&config)?;
            let state = CoinState::open(NodeConfig::from_parameters(parameters)).await?;
            let status = state.run_simulation_iterations(iterations).await?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        Command::Simulate {
            config,
            iterations,
            initial_wallets,
            wealth_sigma,
            tx_per_wallet,
            transfer_fraction,
            seed,
            growth_factor,
            penalty_rate,
            target_penalty_share,
            faucet_share,
            burn_base,
            inverse_balance_weight,
        } => {
            let mut parameters = load_parameters(&config)?;
            if let Some(value) = growth_factor {
                parameters.growth.growth_factor_per_iteration = value;
            }
            if let Some(value) = penalty_rate {
                parameters.economy.penalty_rate = value;
            }
            if let Some(value) = target_penalty_share {
                parameters.economy.target_penalty_share_of_supply = value;
            }
            if let Some(value) = faucet_share {
                parameters.economy.faucet_share_of_penalty = value;
            }
            if let Some(value) = burn_base {
                parameters.economy.burn_base = value;
            }
            if let Some(value) = inverse_balance_weight {
                parameters.economy.inverse_balance_weight = value;
            }
            let sim_config = SimConfig {
                initial_wallets,
                initial_wealth_sigma: wealth_sigma,
                iterations,
                tx_per_wallet_mean: tx_per_wallet,
                transfer_fraction_mean: transfer_fraction,
                rng_seed: seed,
            };
            let metrics = run_simulation(&parameters, &sim_config)?;
            println!("{}", SimMetrics::csv_header());
            for row in &metrics {
                println!("{}", row.to_csv_row());
            }
        }
    }
    Ok(())
}

fn load_parameters(path: &PathBuf) -> Result<SystemParameters> {
    if !path.exists() {
        return Ok(SystemParameters::default());
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parse TOML config {}", path.display()))
}
