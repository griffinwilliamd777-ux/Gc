use ethers::types::{Address, U256};
use tracing::{debug, info, warn};

use crate::dex::pool::{DexPool, PoolState};

use super::opportunity::{ArbHop, ArbOpportunity, ArbPath, ArbType};

/// The arbitrage detection engine.
/// Scans pools for cross-DEX and triangular arbitrage opportunities.
pub struct ArbitrageDetector {
    /// Minimum net profit in wei to consider an opportunity viable.
    min_profit_wei: U256,
    /// Estimated gas per swap hop (used for rough profitability check).
    estimated_gas_per_hop: U256,
    /// Current gas price in wei.
    gas_price: U256,
    /// Whether triangular scanning is enabled.
    enable_triangular: bool,
    /// Whether cross-DEX scanning is enabled.
    enable_cross_dex: bool,
    /// Maximum hops for triangular arb.
    max_hops: usize,
}

impl ArbitrageDetector {
    pub fn new(
        min_profit_wei: U256,
        gas_price: U256,
        enable_triangular: bool,
        enable_cross_dex: bool,
        max_hops: usize,
    ) -> Self {
        Self {
            min_profit_wei,
            estimated_gas_per_hop: U256::from(150_000u64), // ~150k gas per swap
            gas_price,
            enable_triangular,
            enable_cross_dex,
            max_hops,
        }
    }

    /// Update the current gas price for profitability calculations.
    pub fn update_gas_price(&mut self, gas_price: U256) {
        self.gas_price = gas_price;
    }

    /// Scan all pools for arbitrage opportunities.
    pub fn scan(
        &self,
        pools: &[DexPool],
        input_amounts: &[U256],
        block_number: u64,
    ) -> Vec<ArbOpportunity> {
        let mut opportunities = Vec::new();

        if self.enable_cross_dex {
            let cross_dex = self.find_cross_dex_opportunities(pools, input_amounts, block_number);
            opportunities.extend(cross_dex);
        }

        if self.enable_triangular {
            let triangular =
                self.find_triangular_opportunities(pools, input_amounts, block_number);
            opportunities.extend(triangular);
        }

        // Sort by net profit descending.
        opportunities.sort_by(|a, b| b.net_profit.cmp(&a.net_profit));

        if !opportunities.is_empty() {
            info!(
                count = opportunities.len(),
                "Found arbitrage opportunities"
            );
        }

        opportunities
    }

    /// Find cross-DEX arbitrage: same pair on different DEXes with price discrepancy.
    fn find_cross_dex_opportunities(
        &self,
        pools: &[DexPool],
        input_amounts: &[U256],
        block_number: u64,
    ) -> Vec<ArbOpportunity> {
        let mut opportunities = Vec::new();

        // Group pools by token pair (sorted addresses).
        let mut pair_pools: std::collections::HashMap<(Address, Address), Vec<&DexPool>> =
            std::collections::HashMap::new();

        for pool in pools {
            if !matches!(pool.state, PoolState::Active(_)) {
                continue;
            }
            let key = if pool.token0 < pool.token1 {
                (pool.token0, pool.token1)
            } else {
                (pool.token1, pool.token0)
            };
            pair_pools.entry(key).or_default().push(pool);
        }

        // For each pair with multiple pools, check for price discrepancy.
        for ((_t0, _t1), pools_for_pair) in &pair_pools {
            if pools_for_pair.len() < 2 {
                continue;
            }

            for amount_in in input_amounts {
                // Try buying on pool_a, selling on pool_b, and vice versa.
                for (i, pool_a) in pools_for_pair.iter().enumerate() {
                    for pool_b in pools_for_pair.iter().skip(i + 1) {
                        // Direction 1: token0 -> token1 on pool_a, token1 -> token0 on pool_b
                        if let Some(opp) = self.check_cross_dex_pair(
                            pool_a,
                            pool_b,
                            pool_a.token0,
                            pool_a.token1,
                            *amount_in,
                            block_number,
                        ) {
                            opportunities.push(opp);
                        }

                        // Direction 2: token0 -> token1 on pool_b, token1 -> token0 on pool_a
                        if let Some(opp) = self.check_cross_dex_pair(
                            pool_b,
                            pool_a,
                            pool_a.token0,
                            pool_a.token1,
                            *amount_in,
                            block_number,
                        ) {
                            opportunities.push(opp);
                        }
                    }
                }
            }
        }

        opportunities
    }

    /// Check a specific cross-DEX pair for profitability.
    fn check_cross_dex_pair(
        &self,
        buy_pool: &DexPool,
        sell_pool: &DexPool,
        token_in: Address,
        token_mid: Address,
        amount_in: U256,
        block_number: u64,
    ) -> Option<ArbOpportunity> {
        // Buy: token_in -> token_mid on buy_pool
        let mid_amount = buy_pool.get_amount_out(amount_in, token_in)?;
        if mid_amount.is_zero() {
            return None;
        }

        // Sell: token_mid -> token_in on sell_pool
        let final_amount = sell_pool.get_amount_out(mid_amount, token_mid)?;
        if final_amount.is_zero() {
            return None;
        }

        // Check profitability.
        if final_amount <= amount_in {
            return None;
        }

        let gross_profit = final_amount - amount_in;
        let gas_cost = self.estimated_gas_per_hop * U256::from(2u64) * self.gas_price;

        if gross_profit <= gas_cost {
            debug!(
                buy = %buy_pool.address,
                sell = %sell_pool.address,
                gross = %gross_profit,
                gas = %gas_cost,
                "Cross-DEX opportunity not profitable after gas"
            );
            return None;
        }

        let net_profit = gross_profit - gas_cost;
        if net_profit < self.min_profit_wei {
            return None;
        }

        let path = ArbPath {
            hops: vec![
                ArbHop {
                    pool_address: buy_pool.address,
                    dex_name: buy_pool.dex_type.to_string(),
                    token_in,
                    token_out: token_mid,
                    expected_out: mid_amount,
                },
                ArbHop {
                    pool_address: sell_pool.address,
                    dex_name: sell_pool.dex_type.to_string(),
                    token_in: token_mid,
                    token_out: token_in,
                    expected_out: final_amount,
                },
            ],
        };

        info!(
            buy_dex = %buy_pool.dex_type,
            sell_dex = %sell_pool.dex_type,
            net_profit = %net_profit,
            "Cross-DEX arbitrage opportunity found"
        );

        Some(ArbOpportunity {
            arb_type: ArbType::CrossDex,
            path,
            token_in,
            amount_in,
            expected_out: final_amount,
            gross_profit,
            estimated_gas_cost: gas_cost,
            net_profit,
            block_number,
            detected_at: chrono::Utc::now(),
        })
    }

    /// Find triangular arbitrage: A -> B -> C -> A through multiple pools.
    fn find_triangular_opportunities(
        &self,
        pools: &[DexPool],
        input_amounts: &[U256],
        block_number: u64,
    ) -> Vec<ArbOpportunity> {
        let mut opportunities = Vec::new();

        if self.max_hops < 3 {
            warn!("max_hops < 3, triangular arb disabled");
            return opportunities;
        }

        let active_pools: Vec<&DexPool> = pools
            .iter()
            .filter(|p| matches!(p.state, PoolState::Active(_)))
            .collect();

        // Build adjacency: token -> list of (pool_index, other_token)
        let mut adjacency: std::collections::HashMap<Address, Vec<(usize, Address)>> =
            std::collections::HashMap::new();

        for (idx, pool) in active_pools.iter().enumerate() {
            adjacency
                .entry(pool.token0)
                .or_default()
                .push((idx, pool.token1));
            adjacency
                .entry(pool.token1)
                .or_default()
                .push((idx, pool.token0));
        }

        // For each starting token and input amount, DFS for 3-hop cycles.
        for amount_in in input_amounts {
            for start_token in adjacency.keys() {
                let paths = self.dfs_triangular(
                    &active_pools,
                    &adjacency,
                    *start_token,
                    *start_token,
                    *amount_in,
                    &mut Vec::new(),
                    0,
                );

                for (path_hops, final_out) in paths {
                    if final_out <= *amount_in {
                        continue;
                    }

                    let gross_profit = final_out - *amount_in;
                    let gas_cost = self.estimated_gas_per_hop
                        * U256::from(path_hops.len() as u64)
                        * self.gas_price;

                    if gross_profit <= gas_cost {
                        continue;
                    }

                    let net_profit = gross_profit - gas_cost;
                    if net_profit < self.min_profit_wei {
                        continue;
                    }

                    let path = ArbPath { hops: path_hops };

                    info!(
                        hops = path.hop_count(),
                        net_profit = %net_profit,
                        "Triangular arbitrage opportunity found"
                    );

                    opportunities.push(ArbOpportunity {
                        arb_type: ArbType::Triangular,
                        path,
                        token_in: *start_token,
                        amount_in: *amount_in,
                        expected_out: final_out,
                        gross_profit,
                        estimated_gas_cost: gas_cost,
                        net_profit,
                        block_number,
                        detected_at: chrono::Utc::now(),
                    });
                }
            }
        }

        opportunities
    }

    /// DFS to find triangular paths returning to start_token.
    fn dfs_triangular(
        &self,
        pools: &[&DexPool],
        adjacency: &std::collections::HashMap<Address, Vec<(usize, Address)>>,
        start_token: Address,
        current_token: Address,
        current_amount: U256,
        used_pools: &mut Vec<usize>,
        depth: usize,
    ) -> Vec<(Vec<ArbHop>, U256)> {
        let mut results = Vec::new();

        if depth >= self.max_hops {
            return results;
        }

        let neighbors = match adjacency.get(&current_token) {
            Some(n) => n,
            None => return results,
        };

        for (pool_idx, next_token) in neighbors {
            if used_pools.contains(pool_idx) {
                continue;
            }

            let pool = pools[*pool_idx];
            let amount_out = match pool.get_amount_out(current_amount, current_token) {
                Some(a) if !a.is_zero() => a,
                _ => continue,
            };

            let hop = ArbHop {
                pool_address: pool.address,
                dex_name: pool.dex_type.to_string(),
                token_in: current_token,
                token_out: *next_token,
                expected_out: amount_out,
            };

            // If we've completed a cycle back to start (at least 3 hops).
            if *next_token == start_token && depth >= 2 {
                results.push((vec![hop], amount_out));
                continue;
            }

            // Continue DFS.
            if depth < self.max_hops - 1 {
                used_pools.push(*pool_idx);
                let sub_results = self.dfs_triangular(
                    pools,
                    adjacency,
                    start_token,
                    *next_token,
                    amount_out,
                    used_pools,
                    depth + 1,
                );
                used_pools.pop();

                for (mut sub_hops, final_out) in sub_results {
                    let mut full_hops = vec![hop.clone()];
                    full_hops.append(&mut sub_hops);
                    results.push((full_hops, final_out));
                }
            }
        }

        results
    }
}
