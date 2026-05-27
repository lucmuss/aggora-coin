//! Deterministic, in-memory economic simulator.
//!
//! This drives the real [`execute_iteration`](crate::execute_iteration) engine over a synthetic
//! population so the economic model can be validated and tuned without the storage/REST stack.
//! Everything is seeded from a `ChaCha20Rng`, so a given `SimConfig` always produces identical
//! metrics — which is exactly what the spec's "numerical simulation over >= 24 iterations"
//! convergence requirement (D.13 / N.4) needs.

use crate::{compute_gini_from_balances, execute_iteration, micro_seed};
use aggora_crypto::blake3_hex;
use aggora_types::{SystemParameters, SystemState, Wallet};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone)]
pub struct SimConfig {
    pub initial_wallets: u64,
    /// Initial wealth spread. `0.0` = everyone holds exactly the seed; higher values widen the
    /// starting log-normal distribution (used to stress Gini reduction).
    pub initial_wealth_sigma: f64,
    pub iterations: u64,
    /// Mean number of outgoing transfers per active wallet per iteration (Poisson).
    pub tx_per_wallet_mean: f64,
    /// Fraction of the sender balance moved per transfer (mean of a clamped log-normal).
    pub transfer_fraction_mean: f64,
    pub rng_seed: u64,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            initial_wallets: 100,
            initial_wealth_sigma: 0.8,
            iterations: 24,
            tx_per_wallet_mean: 5.0,
            transfer_fraction_mean: 0.05,
            rng_seed: 0xA66_0_C0_1Eu64,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SimMetrics {
    pub iteration: u64,
    pub supply: u64,
    pub n_wallets: u64,
    pub n_active: u64,
    pub gini: f64,
    pub top10_share: f64,
    pub median_balance: u64,
    pub penalty_total: u64,
    pub burned: u64,
    pub reward_total: u64,
    pub faucet_from_mint: u64,
    pub new_wallets: u64,
    pub n_txs: u64,
    pub burn_rate: f64,
    pub inflation: f64,
}

impl SimMetrics {
    pub fn csv_header() -> &'static str {
        "iteration,supply,n_wallets,n_active,gini,top10_share,median_balance,penalty_total,burned,reward_total,faucet_from_mint,new_wallets,n_txs,burn_rate,inflation"
    }

    pub fn to_csv_row(&self) -> String {
        format!(
            "{},{},{},{},{:.6},{:.6},{},{},{},{},{},{},{},{:.6},{:.6}",
            self.iteration,
            self.supply,
            self.n_wallets,
            self.n_active,
            self.gini,
            self.top10_share,
            self.median_balance,
            self.penalty_total,
            self.burned,
            self.reward_total,
            self.faucet_from_mint,
            self.new_wallets,
            self.n_txs,
            self.burn_rate,
            self.inflation,
        )
    }
}

/// Knuth's algorithm for sampling a Poisson-distributed count.
fn sample_poisson(rng: &mut ChaCha20Rng, lambda: f64) -> u64 {
    if lambda <= 0.0 {
        return 0;
    }
    let l = (-lambda).exp();
    let mut k = 0u64;
    let mut p = 1.0;
    loop {
        k += 1;
        p *= rng.gen::<f64>();
        if p <= l {
            return k - 1;
        }
    }
}

/// Standard-normal sample via Box-Muller.
fn sample_standard_normal(rng: &mut ChaCha20Rng) -> f64 {
    let u1: f64 = rng.gen::<f64>().max(f64::MIN_POSITIVE);
    let u2: f64 = rng.gen::<f64>();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

fn sim_wallet(idx: u64, balance: u64) -> Wallet {
    let id = blake3_hex(format!("aggora-sim-wallet:{idx}"));
    Wallet {
        pubkey: id.clone(),
        id,
        balance,
        nonce: 0,
        created_at_tick: 0,
        created_at_iteration: 0,
        created_by_operator: "sim".to_string(),
        iteration_tx_count: 0,
        iteration_counterparties: BTreeSet::new(),
        last_active_iteration: 0,
        activity_score: 1.0,
    }
}

fn top10_share(sorted_desc: &[u64], total: u128) -> f64 {
    if total == 0 || sorted_desc.is_empty() {
        return 0.0;
    }
    let cut = (sorted_desc.len() as f64 * 0.10).ceil() as usize;
    let top: u128 = sorted_desc.iter().take(cut.max(1)).map(|v| *v as u128).sum();
    (top as f64) / (total as f64)
}

/// Run the synthetic simulation and return per-iteration metrics (one row per executed iteration).
pub fn run_simulation(parameters: &SystemParameters, config: &SimConfig) -> anyhow::Result<Vec<SimMetrics>> {
    let mut rng = ChaCha20Rng::seed_from_u64(config.rng_seed);
    let seed = micro_seed(parameters);

    let mut wallets: Vec<Wallet> = (0..config.initial_wallets)
        .map(|i| {
            let balance = if config.initial_wealth_sigma > 0.0 {
                let factor = (config.initial_wealth_sigma * sample_standard_normal(&mut rng)).exp();
                ((seed as f64 * factor).round() as u64).max(1)
            } else {
                seed
            };
            sim_wallet(i, balance)
        })
        .collect();
    let mut next_wallet_idx = config.initial_wallets;

    let mut state = SystemState::default();
    let mut metrics = Vec::with_capacity(config.iterations as usize);

    for _ in 0..config.iterations {
        // 1. Synthetic activity: move value between wallets so the redistribution/activity
        //    machinery has something to work with.
        let n = wallets.len();
        let mut n_txs = 0u64;
        if n >= 2 {
            for sender_idx in 0..n {
                let k = sample_poisson(&mut rng, config.tx_per_wallet_mean);
                for _ in 0..k {
                    if wallets[sender_idx].balance == 0 {
                        break;
                    }
                    let mut recipient_idx = rng.gen_range(0..n);
                    if recipient_idx == sender_idx {
                        recipient_idx = (recipient_idx + 1) % n;
                    }
                    let fraction = (config.transfer_fraction_mean
                        * (0.5 * sample_standard_normal(&mut rng)).exp())
                    .clamp(0.0001, 0.95);
                    let amount = ((wallets[sender_idx].balance as f64 * fraction).floor() as u64)
                        .min(wallets[sender_idx].balance);
                    if amount == 0 {
                        continue;
                    }
                    let recipient_id = wallets[recipient_idx].id.clone();
                    let sender_id = wallets[sender_idx].id.clone();
                    {
                        let sender = &mut wallets[sender_idx];
                        sender.balance -= amount;
                        sender.nonce += 1;
                        sender.iteration_tx_count += 1;
                        sender.iteration_counterparties.insert(recipient_id);
                        sender.last_active_iteration = state.current_iteration;
                    }
                    {
                        let recipient = &mut wallets[recipient_idx];
                        recipient.balance = recipient.balance.saturating_add(amount);
                        recipient.iteration_tx_count += 1;
                        recipient.iteration_counterparties.insert(sender_id);
                        recipient.last_active_iteration = state.current_iteration;
                    }
                    n_txs += 1;
                }
            }
        }

        // 2. Apply the real economic iteration.
        let result = execute_iteration(wallets, parameters, &state, &"sim-operator".to_string())?;
        let commit = result.commit;
        wallets = result.wallets;

        // Re-id the freshly minted faucet wallets so future iterations keep unique ids that do
        // not collide with the deterministic faucet ids minted inside `execute_iteration`.
        for wallet in wallets.iter_mut() {
            if wallet.created_at_iteration == commit.iteration_id {
                *wallet = sim_wallet(next_wallet_idx, wallet.balance);
                next_wallet_idx += 1;
            }
        }

        // 3. Advance the local system state for the next inflation computation.
        state.current_iteration = commit.iteration_id;
        state.total_supply = commit.post_supply;
        state.previous_iteration_supply = commit.post_supply;
        state.last_inflation = commit.inflation;
        state.n_wallets = wallets.len() as u64;

        let mut balances: Vec<u64> = wallets.iter().map(|w| w.balance).collect();
        balances.sort_unstable();
        let total: u128 = balances.iter().map(|v| *v as u128).sum();
        let median_balance = balances.get(balances.len() / 2).copied().unwrap_or(0);
        let mut desc = balances.clone();
        desc.reverse();
        let n_active = wallets.iter().filter(|w| w.activity_score >= 0.999).count() as u64;

        metrics.push(SimMetrics {
            iteration: commit.iteration_id,
            supply: commit.post_supply,
            n_wallets: wallets.len() as u64,
            n_active,
            gini: compute_gini_from_balances(&balances),
            top10_share: top10_share(&desc, total),
            median_balance,
            penalty_total: commit.penalties.iter().map(|(_, p)| *p).sum(),
            burned: commit.burned,
            reward_total: commit.rewards.iter().map(|(_, r)| *r).sum(),
            faucet_from_mint: commit.faucet_from_mint,
            new_wallets: commit.new_wallets.len() as u64,
            n_txs,
            burn_rate: commit.burn_rate,
            inflation: commit.inflation,
        });
    }

    Ok(metrics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulation_is_deterministic_and_bounded() {
        let params = SystemParameters::default();
        let config = SimConfig {
            iterations: 12,
            ..SimConfig::default()
        };
        let a = run_simulation(&params, &config).unwrap();
        let b = run_simulation(&params, &config).unwrap();
        assert_eq!(a.len(), 12);
        // Determinism: identical config -> identical trajectory.
        assert_eq!(a.last().unwrap().supply, b.last().unwrap().supply);
        assert_eq!(a.last().unwrap().gini, b.last().unwrap().gini);
        for row in &a {
            assert!((0.0..=1.0).contains(&row.gini));
            assert!(row.burn_rate >= params.economy.burn_min && row.burn_rate <= params.economy.burn_max);
        }
    }
}
