use ethers::prelude::*;
use ethers::types::U256;
use eyre::Result;
use std::sync::Arc;
use tracing::{debug, info};

/// Gas optimization utilities for MEV transactions.
/// Implements "gas golfing" techniques to minimize transaction costs.
pub struct GasOptimizer<M: Middleware> {
    client: Arc<M>,
    /// Maximum gas price in wei the bot is willing to pay.
    max_gas_price: U256,
    /// History of recent gas prices for trend analysis.
    gas_history: Vec<U256>,
    /// Maximum history entries to keep.
    max_history: usize,
}

impl<M: Middleware + 'static> GasOptimizer<M> {
    pub fn new(client: Arc<M>, max_gas_gwei: u64) -> Self {
        Self {
            client,
            max_gas_price: U256::from(max_gas_gwei) * U256::from(1_000_000_000u64),
            gas_history: Vec::new(),
            max_history: 50,
        }
    }

    /// Get the current optimal gas price considering network conditions.
    pub async fn get_optimal_gas_price(&mut self) -> Result<U256> {
        let current_gas = self
            .client
            .get_gas_price()
            .await
            .map_err(|e| eyre::eyre!("Failed to get gas price: {}", e))?;

        // Track history.
        self.gas_history.push(current_gas);
        if self.gas_history.len() > self.max_history {
            self.gas_history.remove(0);
        }

        // Use a competitive gas price: current + small premium.
        // This helps get included faster without overpaying.
        let premium = current_gas / U256::from(20u64); // 5% premium
        let optimal = current_gas + premium;

        // Cap at max gas price.
        let final_price = if optimal > self.max_gas_price {
            debug!(
                current = %current_gas,
                optimal = %optimal,
                max = %self.max_gas_price,
                "Gas price exceeds maximum, capping"
            );
            self.max_gas_price
        } else {
            optimal
        };

        info!(
            current_gwei = %self.wei_to_gwei(current_gas),
            optimal_gwei = %self.wei_to_gwei(final_price),
            "Gas price calculated"
        );

        Ok(final_price)
    }

    /// Estimate if a trade is worth executing at current gas prices.
    /// Returns the estimated gas cost in wei.
    pub async fn estimate_gas_cost(&self, estimated_gas_units: U256) -> Result<U256> {
        let gas_price = self
            .client
            .get_gas_price()
            .await
            .map_err(|e| eyre::eyre!("Failed to get gas price: {}", e))?;
        Ok(estimated_gas_units * gas_price)
    }

    /// Check if gas price is within acceptable range.
    pub async fn is_gas_acceptable(&self) -> Result<bool> {
        let gas_price = self
            .client
            .get_gas_price()
            .await
            .map_err(|e| eyre::eyre!("Failed to get gas price: {}", e))?;
        Ok(gas_price <= self.max_gas_price)
    }

    /// Gas golfing: compute the most gas-efficient way to encode calldata.
    /// Zero bytes in calldata cost 4 gas; non-zero bytes cost 16 gas.
    /// This function reports the calldata gas cost.
    pub fn calldata_gas_cost(data: &[u8]) -> u64 {
        let mut cost: u64 = 0;
        for byte in data {
            if *byte == 0 {
                cost += 4; // Zero byte cost
            } else {
                cost += 16; // Non-zero byte cost
            }
        }
        cost
    }

    /// Estimate savings from using access lists (EIP-2930).
    /// Warm storage slots cost 100 gas vs 2600 for cold.
    pub fn estimate_access_list_savings(num_storage_slots: u64) -> u64 {
        let cold_cost = num_storage_slots * 2600;
        let warm_cost = num_storage_slots * 100;
        let access_list_overhead = num_storage_slots * 1900; // Declaring slots in access list
        let savings = cold_cost.saturating_sub(warm_cost + access_list_overhead);
        debug!(
            slots = num_storage_slots,
            savings = savings,
            "Access list savings estimate"
        );
        savings
    }

    /// Get gas price trend (rising, falling, stable).
    pub fn gas_trend(&self) -> GasTrend {
        if self.gas_history.len() < 5 {
            return GasTrend::Unknown;
        }

        let recent: Vec<&U256> = self.gas_history.iter().rev().take(5).collect();
        let older: Vec<&U256> = self.gas_history.iter().rev().skip(5).take(5).collect();

        if older.is_empty() {
            return GasTrend::Unknown;
        }

        let recent_avg: U256 = {
            let mut acc = U256::zero();
            for val in &recent {
                acc += **val;
            }
            acc / U256::from(recent.len())
        };
        let older_avg: U256 = {
            let mut acc = U256::zero();
            for val in &older {
                acc += **val;
            }
            acc / U256::from(older.len())
        };

        let threshold = older_avg / U256::from(10u64); // 10% change threshold

        if recent_avg > older_avg + threshold {
            GasTrend::Rising
        } else if recent_avg + threshold < older_avg {
            GasTrend::Falling
        } else {
            GasTrend::Stable
        }
    }

    fn wei_to_gwei(&self, wei: U256) -> U256 {
        wei / U256::from(1_000_000_000u64)
    }
}

/// Gas price trend indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GasTrend {
    Rising,
    Falling,
    Stable,
    Unknown,
}
