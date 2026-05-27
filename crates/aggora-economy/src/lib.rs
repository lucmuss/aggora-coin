use aggora_crypto::{blake3_hex, hash_serializable, wallet_id_from_public_key};
use aggora_types::{
    IterationCommit, OperatorId, ResidualPolicy, SystemParameters, SystemState, Wallet, WalletId, MICRO_AGC_PER_AGC,
};
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeSet;

pub mod simulator;
pub use simulator::{run_simulation, SimConfig, SimMetrics};

pub fn micro_seed(parameters: &SystemParameters) -> u64 {
    parameters.economy.initial_seed_agc.saturating_mul(MICRO_AGC_PER_AGC)
}

pub fn compute_gini_from_balances(balances: &[u64]) -> f64 {
    if balances.is_empty() {
        return 0.0;
    }
    let mut sorted = balances.to_vec();
    sorted.sort_unstable();
    let total: u128 = sorted.iter().map(|v| *v as u128).sum();
    if total == 0 {
        return 0.0;
    }
    let n = sorted.len() as f64;
    let weighted_sum: u128 = sorted
        .iter()
        .enumerate()
        .map(|(idx, balance)| (idx as u128 + 1) * (*balance as u128))
        .sum();
    // Float rounding can push the result a hair outside [0, 1]; the Gini coefficient is defined on that range.
    (((2.0 * weighted_sum as f64) / (n * total as f64)) - ((n + 1.0) / n)).clamp(0.0, 1.0)
}

pub fn compute_gini(wallets: &[Wallet]) -> f64 {
    compute_gini_from_balances(&wallets.iter().map(|w| w.balance).collect::<Vec<_>>())
}

#[derive(Debug, Clone)]
pub struct IterationResult {
    pub commit: IterationCommit,
    pub wallets: Vec<Wallet>,
}

pub fn execute_iteration(
    mut wallets: Vec<Wallet>,
    parameters: &SystemParameters,
    state: &SystemState,
    operator_id: &OperatorId,
) -> Result<IterationResult> {
    let iteration_id = state.current_iteration + 1;
    let snapshot_supply: u64 = wallets.iter().map(|w| w.balance).sum();
    let snapshot_n_wallets = wallets.len() as u64;
    let snapshot_gini = compute_gini(&wallets);
    let inflation = if state.previous_iteration_supply > 0 {
        (snapshot_supply as f64 - state.previous_iteration_supply as f64) / state.previous_iteration_supply as f64
    } else {
        0.0
    };
    let beta = (parameters.economy.burn_base
        + parameters.economy.burn_sensitivity * (inflation - parameters.economy.target_inflation_per_iter))
        .clamp(parameters.economy.burn_min, parameters.economy.burn_max);

    let seed = micro_seed(parameters);
    let b_max = wallets.iter().map(|w| w.balance).max().unwrap_or(0);
    let denominator = if b_max > 0 && seed > 0 {
        (1.0 + (b_max as f64 / seed as f64)).ln()
    } else {
        0.0
    };

    // Penalties are built in the same order as `wallets`, so index i aligns with wallets[i]
    // throughout this function. This keeps every per-wallet pass O(N) instead of O(N^2).
    let mut penalties: Vec<(WalletId, u64)> = Vec::with_capacity(wallets.len());
    for wallet in &wallets {
        let base_penalty = if wallet.balance == 0 || denominator <= f64::EPSILON {
            0
        } else {
            let numerator = (1.0 + (wallet.balance as f64 / seed.max(1) as f64)).ln();
            (parameters.economy.penalty_rate * wallet.balance as f64 * numerator / denominator).floor() as u64
        };
        penalties.push((wallet.id.clone(), base_penalty.min(wallet.balance)));
    }

    let penalty_total: u64 = penalties.iter().map(|(_, p)| *p).sum();
    let target_penalty = (parameters.economy.target_penalty_share_of_supply * snapshot_supply as f64).floor() as u64;
    if penalty_total > 0 && target_penalty > 0 {
        let deviation = (penalty_total.abs_diff(target_penalty) as f64) / target_penalty as f64;
        if deviation > 0.20 {
            let scale = target_penalty as f64 / penalty_total as f64;
            for (idx, (_, penalty)) in penalties.iter_mut().enumerate() {
                *penalty = ((*penalty as f64 * scale).floor() as u64).min(wallets[idx].balance);
            }
        }
    }

    let penalty_total: u64 = penalties.iter().map(|(_, p)| *p).sum();
    let planned_new_wallets = ((parameters.growth.growth_factor_per_iteration * snapshot_n_wallets as f64).floor() as u64)
        .min(parameters.growth.max_new_wallets_per_iter);
    let faucet_needed = planned_new_wallets.saturating_mul(seed);

    // Allocate the penalty pool in priority order so the parts can never exceed the pool:
    //   1. faucet (capped by both the demand and the configured share φ·P)
    //   2. burn   (β·P, capped by what remains after faucet)
    //   3. redistribution (the remainder)
    // This guarantees faucet_from_penalty + burned + redistribution_pool == penalty_total,
    // which makes the supply invariant (D.11) hold exactly.
    let faucet_share_cap = (parameters.economy.faucet_share_of_penalty * penalty_total as f64).floor() as u64;
    let faucet_from_penalty = faucet_needed.min(faucet_share_cap).min(penalty_total);
    let pool_after_faucet = penalty_total - faucet_from_penalty;
    let burned_base = ((beta * penalty_total as f64).floor() as u64).min(pool_after_faucet);
    let redistribution_pool = pool_after_faucet - burned_base;
    let faucet_from_mint = faucet_needed.saturating_sub(faucet_from_penalty);

    for (idx, wallet) in wallets.iter_mut().enumerate() {
        let raw_activity = if wallet.iteration_tx_count >= parameters.economy.activity_min_tx_count
            && wallet.iteration_counterparties.len() as u32 >= parameters.economy.activity_min_counterparties
        {
            1.0
        } else {
            0.0
        };
        let lambda = parameters.economy.activity_ema_lambda.clamp(0.0, 1.0);
        wallet.activity_score = lambda * raw_activity + (1.0 - lambda) * wallet.activity_score;
        wallet.balance = wallet.balance.saturating_sub(penalties[idx].1);
    }

    let weights: Vec<f64> = wallets
        .iter()
        .map(|wallet| {
            let alpha = parameters.economy.redistribution_active_min
                + (parameters.economy.redistribution_active_max - parameters.economy.redistribution_active_min)
                    * wallet.activity_score.clamp(0.0, 1.0);
            let inverse = if parameters.economy.inverse_balance_weight > 0.0 {
                (1.0 + wallet.balance as f64).powf(-parameters.economy.inverse_balance_weight)
            } else {
                1.0
            };
            alpha.max(0.0) * inverse
        })
        .collect();
    let total_weight: f64 = weights.iter().sum();

    let mut rewards = Vec::with_capacity(wallets.len());
    let mut distributed = 0u64;
    for (idx, wallet) in wallets.iter_mut().enumerate() {
        let reward = if redistribution_pool > 0 && total_weight > 0.0 {
            (redistribution_pool as f64 * weights[idx] / total_weight).floor() as u64
        } else {
            0
        };
        wallet.balance = wallet.balance.saturating_add(reward);
        wallet.iteration_tx_count = 0;
        wallet.iteration_counterparties.clear();
        rewards.push((wallet.id.clone(), reward));
        distributed = distributed.saturating_add(reward);
    }

    let residual = redistribution_pool.saturating_sub(distributed);
    let mut burned = burned_base;
    match parameters.economy.residual_policy {
        ResidualPolicy::Burn => {
            burned = burned.saturating_add(residual);
        }
        ResidualPolicy::GenesisWallet => {
            if let Some(wallet) = wallets.first_mut() {
                wallet.balance = wallet.balance.saturating_add(residual);
                if let Some((_, reward)) = rewards.first_mut() {
                    *reward = reward.saturating_add(residual);
                }
            } else {
                burned = burned.saturating_add(residual);
            }
        }
    }

    let mut new_wallet_ids = Vec::new();
    for idx in 0..planned_new_wallets {
        let synthetic_pubkey = blake3_hex(format!("aggora-faucet:{iteration_id}:{idx}"));
        let public_key = synthetic_pubkey[..64].to_string();
        let wallet_id = wallet_id_from_public_key(&public_key)?;
        wallets.push(Wallet {
            id: wallet_id.clone(),
            pubkey: public_key,
            balance: seed,
            nonce: 0,
            created_at_tick: state.current_tick,
            created_at_iteration: iteration_id,
            created_by_operator: operator_id.clone(),
            iteration_tx_count: 0,
            iteration_counterparties: BTreeSet::new(),
            last_active_iteration: iteration_id,
            activity_score: 1.0,
        });
        new_wallet_ids.push(wallet_id);
    }

    let post_supply: u64 = wallets.iter().map(|w| w.balance).sum();
    let mut commit = IterationCommit {
        tx_id: String::new(),
        iteration_id,
        triggered_at_tick: state.current_tick,
        snapshot_supply,
        snapshot_n_wallets,
        snapshot_gini,
        inflation,
        burn_rate: beta,
        penalties,
        rewards,
        burned,
        faucet_from_penalty,
        faucet_from_mint,
        new_wallets: new_wallet_ids,
        post_supply,
        validator_sigs: vec![],
    };
    commit.tx_id = iteration_commit_hash(&commit)?;
    Ok(IterationResult { commit, wallets })
}

fn iteration_commit_hash(commit: &IterationCommit) -> Result<String> {
    #[derive(Serialize)]
    struct CommitPayload<'a> {
        iteration_id: u64,
        triggered_at_tick: u64,
        snapshot_supply: u64,
        snapshot_n_wallets: u64,
        snapshot_gini: f64,
        inflation: f64,
        burn_rate: f64,
        penalties: &'a [(String, u64)],
        rewards: &'a [(String, u64)],
        burned: u64,
        faucet_from_penalty: u64,
        faucet_from_mint: u64,
        new_wallets: &'a [String],
        post_supply: u64,
    }
    hash_serializable(&CommitPayload {
        iteration_id: commit.iteration_id,
        triggered_at_tick: commit.triggered_at_tick,
        snapshot_supply: commit.snapshot_supply,
        snapshot_n_wallets: commit.snapshot_n_wallets,
        snapshot_gini: commit.snapshot_gini,
        inflation: commit.inflation,
        burn_rate: commit.burn_rate,
        penalties: &commit.penalties,
        rewards: &commit.rewards,
        burned: commit.burned,
        faucet_from_penalty: commit.faucet_from_penalty,
        faucet_from_mint: commit.faucet_from_mint,
        new_wallets: &commit.new_wallets,
        post_supply: commit.post_supply,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aggora_types::SystemState;

    #[test]
    fn gini_bounds_hold() {
        assert_eq!(compute_gini_from_balances(&[]), 0.0);
        assert_eq!(compute_gini_from_balances(&[10, 10, 10]), 0.0);
        let gini = compute_gini_from_balances(&[0, 0, 100]);
        assert!((0.0..=1.0).contains(&gini));
    }

    fn sample_wallet(id: &str, balance: u64) -> Wallet {
        Wallet {
            id: id.into(),
            pubkey: "00".repeat(32),
            balance,
            nonce: 0,
            created_at_tick: 0,
            created_at_iteration: 0,
            created_by_operator: "op".into(),
            iteration_tx_count: 2,
            iteration_counterparties: ["other".to_string()].into_iter().collect(),
            last_active_iteration: 0,
            activity_score: 1.0,
        }
    }

    #[test]
    fn iteration_preserves_accounting() {
        let params = SystemParameters::default();
        let op = "op".to_string();
        let wallets = vec![sample_wallet("w1", 100_000_000)];
        let result = execute_iteration(wallets, &params, &SystemState::default(), &op).unwrap();
        let sum: u64 = result.wallets.iter().map(|wallet| wallet.balance).sum();
        assert_eq!(sum, result.commit.post_supply);
    }

    #[test]
    fn supply_invariant_holds_under_high_burn_and_growth() {
        // β_max + φ > 1 used to over-report `burned`; the staged allocation must keep
        //   post_supply == snapshot_supply - burned + faucet_from_mint   (spec D.11)
        // exactly, with the penalty pool never over-allocated.
        let mut params = SystemParameters::default();
        params.economy.burn_max = 0.9;
        params.economy.burn_base = 0.9;
        params.economy.faucet_share_of_penalty = 0.9;
        params.growth.growth_factor_per_iteration = 0.5;
        let op = "op".to_string();
        let wallets: Vec<Wallet> = (0..50)
            .map(|i| sample_wallet(&format!("w{i}"), 10_000_000 + i as u64 * 5_000_000))
            .collect();
        let snapshot_supply: u64 = wallets.iter().map(|w| w.balance).sum();
        let result = execute_iteration(wallets, &params, &SystemState::default(), &op).unwrap();
        let commit = &result.commit;
        assert_eq!(commit.snapshot_supply, snapshot_supply);
        // burned can never exceed what was actually taken from balances.
        let penalty_total: u64 = commit.penalties.iter().map(|(_, p)| *p).sum();
        let reward_total: u64 = commit.rewards.iter().map(|(_, r)| *r).sum();
        assert!(commit.burned + commit.faucet_from_penalty + reward_total <= penalty_total + 1);
        // The headline supply equation must balance to the last micro-AGC.
        assert_eq!(
            commit.post_supply,
            snapshot_supply - commit.burned + commit.faucet_from_mint
        );
        // And the recomputed balances must match the reported post_supply.
        let sum: u64 = result.wallets.iter().map(|w| w.balance).sum();
        assert_eq!(sum, commit.post_supply);
    }
}
