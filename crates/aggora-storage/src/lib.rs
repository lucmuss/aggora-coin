use aggora_types::{
    GiniPoint, IterationCommit, Operator, PohEntry, SupplyPoint, SystemParameters, SystemState, Transaction, Validator, Wallet,
};
use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use sled::{Db, Tree};
use std::{fs, path::PathBuf};

#[derive(Clone)]
pub struct CoinStorage {
    db: Db,
    wallets: Tree,
    operators: Tree,
    validators: Tree,
    transactions: Tree,
    poh_log: Tree,
    tx_by_wallet: Tree,
    iterations: Tree,
    system: Tree,
    snapshot_path: PathBuf,
}

impl CoinStorage {
    pub fn open(db_path: impl AsRef<str>, snapshot_path: impl Into<PathBuf>) -> Result<Self> {
        let db = sled::open(db_path.as_ref()).with_context(|| format!("open sled db at {}", db_path.as_ref()))?;
        let storage = Self {
            wallets: db.open_tree("wallets")?,
            operators: db.open_tree("operators")?,
            validators: db.open_tree("validators")?,
            transactions: db.open_tree("transactions")?,
            poh_log: db.open_tree("poh_log")?,
            tx_by_wallet: db.open_tree("tx_by_wallet")?,
            iterations: db.open_tree("iterations")?,
            system: db.open_tree("system")?,
            db,
            snapshot_path: snapshot_path.into(),
        };
        fs::create_dir_all(&storage.snapshot_path)?;
        Ok(storage)
    }

    pub fn flush(&self) -> Result<()> {
        self.db.flush()?;
        Ok(())
    }

    pub fn get_wallet(&self, wallet_id: &str) -> Result<Option<Wallet>> {
        get_json(&self.wallets, wallet_id.as_bytes())
    }

    pub fn put_wallet(&self, wallet: &Wallet) -> Result<()> {
        put_json(&self.wallets, wallet.id.as_bytes(), wallet)
    }

    pub fn remove_wallet(&self, wallet_id: &str) -> Result<()> {
        self.wallets.remove(wallet_id.as_bytes())?;
        Ok(())
    }

    pub fn list_wallets(&self) -> Result<Vec<Wallet>> {
        scan_values(&self.wallets)
    }

    pub fn put_operator(&self, operator: &Operator) -> Result<()> {
        put_json(&self.operators, operator.id.as_bytes(), operator)
    }

    pub fn get_operator(&self, operator_id: &str) -> Result<Option<Operator>> {
        get_json(&self.operators, operator_id.as_bytes())
    }

    pub fn list_operators(&self) -> Result<Vec<Operator>> {
        scan_values(&self.operators)
    }

    pub fn put_validator(&self, validator: &Validator) -> Result<()> {
        put_json(&self.validators, validator.id.as_bytes(), validator)
    }

    pub fn list_validators(&self) -> Result<Vec<Validator>> {
        scan_values(&self.validators)
    }

    pub fn put_transaction(&self, tx: &Transaction, tick: u64) -> Result<()> {
        put_json(&self.transactions, tx.tx_id().as_bytes(), tx)?;
        for wallet_id in tx.wallet_ids() {
            let key = format!("{}:{:020}:{}", wallet_id, tick, tx.tx_id());
            self.tx_by_wallet.insert(key.as_bytes(), tx.tx_id().as_bytes())?;
        }
        Ok(())
    }

    pub fn get_transaction(&self, tx_id: &str) -> Result<Option<Transaction>> {
        get_json(&self.transactions, tx_id.as_bytes())
    }

    pub fn list_transactions_for_wallet(&self, wallet_id: &str, limit: usize) -> Result<Vec<Transaction>> {
        let prefix = format!("{}:", wallet_id);
        let mut tx_ids = Vec::new();
        for item in self.tx_by_wallet.scan_prefix(prefix.as_bytes()) {
            let (_, value) = item?;
            tx_ids.push(String::from_utf8(value.to_vec())?);
        }
        tx_ids.sort();
        tx_ids.reverse();
        tx_ids.truncate(limit);
        let mut txs = Vec::with_capacity(tx_ids.len());
        for tx_id in tx_ids {
            if let Some(tx) = self.get_transaction(&tx_id)? {
                txs.push(tx);
            }
        }
        Ok(txs)
    }

    pub fn put_poh_entry(&self, entry: &PohEntry) -> Result<()> {
        put_json(&self.poh_log, &entry.tick.to_be_bytes(), entry)
    }

    pub fn latest_poh_entries(&self, limit: usize) -> Result<Vec<PohEntry>> {
        let mut entries = Vec::new();
        for item in self.poh_log.iter().rev().take(limit) {
            let (_, value) = item?;
            entries.push(serde_json::from_slice(&value)?);
        }
        Ok(entries)
    }

    pub fn put_iteration(&self, commit: &IterationCommit) -> Result<()> {
        put_json(&self.iterations, &commit.iteration_id.to_be_bytes(), commit)
    }

    pub fn get_iteration(&self, iteration_id: u64) -> Result<Option<IterationCommit>> {
        get_json(&self.iterations, &iteration_id.to_be_bytes())
    }

    pub fn list_iterations(&self) -> Result<Vec<IterationCommit>> {
        scan_values(&self.iterations)
    }

    pub fn load_state(&self) -> Result<Option<SystemState>> {
        get_json(&self.system, b"state")
    }

    pub fn store_state(&self, state: &SystemState) -> Result<()> {
        put_json(&self.system, b"state", state)
    }

    pub fn load_parameters(&self) -> Result<Option<SystemParameters>> {
        get_json(&self.system, b"parameters")
    }

    pub fn store_parameters(&self, parameters: &SystemParameters) -> Result<()> {
        put_json(&self.system, b"parameters", parameters)
    }

    pub fn supply_history(&self) -> Result<Vec<SupplyPoint>> {
        let mut points = Vec::new();
        for commit in self.list_iterations()? {
            points.push(SupplyPoint {
                iteration: commit.iteration_id,
                supply: commit.post_supply,
                burned: commit.burned,
                faucet_from_mint: commit.faucet_from_mint,
            });
        }
        Ok(points)
    }

    pub fn gini_history(&self) -> Result<Vec<GiniPoint>> {
        let mut points = Vec::new();
        for commit in self.list_iterations()? {
            points.push(GiniPoint {
                iteration: commit.iteration_id,
                gini: commit.snapshot_gini,
            });
        }
        Ok(points)
    }

    pub fn write_snapshot(&self, iteration_id: u64, payload: &serde_json::Value) -> Result<PathBuf> {
        fs::create_dir_all(&self.snapshot_path)?;
        let path = self.snapshot_path.join(format!("iter_{iteration_id}.json"));
        fs::write(&path, serde_json::to_vec_pretty(payload)?)?;
        Ok(path)
    }
}

fn put_json<T: Serialize>(tree: &Tree, key: &[u8], value: &T) -> Result<()> {
    tree.insert(key, serde_json::to_vec(value)?)?;
    Ok(())
}

fn get_json<T: DeserializeOwned>(tree: &Tree, key: &[u8]) -> Result<Option<T>> {
    tree.get(key)?
        .map(|value| serde_json::from_slice(&value).context("decode sled JSON value"))
        .transpose()
}

fn scan_values<T: DeserializeOwned>(tree: &Tree) -> Result<Vec<T>> {
    let mut out = Vec::new();
    for item in tree.iter() {
        let (_, value) = item?;
        out.push(serde_json::from_slice(&value)?);
    }
    Ok(out)
}
