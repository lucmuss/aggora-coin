use aggora_crypto::{canonical_request_message, operator_id_from_public_key, public_key_from_secret_hex, sign_with_secret_hex};
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
