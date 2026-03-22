use ethers::types::{Address, U256};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Supported DEX types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DexType {
    UniswapV2,
    UniswapV3,
    SushiSwap,
}

impl fmt::Display for DexType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DexType::UniswapV2 => write!(f, "UniswapV2"),
            DexType::UniswapV3 => write!(f, "UniswapV3"),
            DexType::SushiSwap => write!(f, "SushiSwap"),
        }
    }
}

/// Live reserves / state for a pool.
#[derive(Debug, Clone)]
pub struct PoolReserves {
    pub reserve0: U256,
    pub reserve1: U256,
    /// For V3 pools: current sqrt price
    pub sqrt_price_x96: Option<U256>,
    /// For V3 pools: current tick
    pub tick: Option<i32>,
    /// For V3 pools: liquidity
    pub liquidity: Option<U256>,
}

/// Represents a liquidity pool on a DEX.
#[derive(Debug, Clone)]
pub struct DexPool {
    pub dex_type: DexType,
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub token0_decimals: u8,
    pub token1_decimals: u8,
    pub fee_bps: u32,
    pub state: PoolState,
}

/// Current state of the pool.
#[derive(Debug, Clone)]
pub enum PoolState {
    /// Pool state not yet fetched.
    Uninitialized,
    /// Pool is live with known reserves.
    Active(PoolReserves),
    /// Pool fetch failed.
    Error(String),
}

impl DexPool {
    /// Calculate the output amount for a given input using the constant product formula (V2).
    /// amount_out = (amount_in * fee_factor * reserve_out) / (reserve_in + amount_in * fee_factor)
    pub fn get_amount_out_v2(&self, amount_in: U256, token_in: Address) -> Option<U256> {
        let reserves = match &self.state {
            PoolState::Active(r) => r,
            _ => return None,
        };

        let (reserve_in, reserve_out) = if token_in == self.token0 {
            (reserves.reserve0, reserves.reserve1)
        } else {
            (reserves.reserve1, reserves.reserve0)
        };

        if reserve_in.is_zero() || reserve_out.is_zero() {
            return None;
        }

        // Fee: for Uniswap V2 style, fee is 30 bps (0.3%).
        // amount_in_with_fee = amount_in * (10000 - fee_bps)
        let fee_factor = U256::from(10000u64 - u64::from(self.fee_bps));
        let amount_in_with_fee = amount_in * fee_factor;
        let numerator = amount_in_with_fee * reserve_out;
        let denominator = reserve_in * U256::from(10000u64) + amount_in_with_fee;

        if denominator.is_zero() {
            return None;
        }

        Some(numerator / denominator)
    }

    /// Calculate output for V3 using single-tick concentrated liquidity math.
    /// Assumes the swap stays within the current tick range.
    /// For large swaps that cross ticks, this will overestimate output.
    pub fn get_amount_out_v3(&self, amount_in: U256, token_in: Address) -> Option<U256> {
        let reserves = match &self.state {
            PoolState::Active(r) => r,
            _ => return None,
        };

        let sqrt_price = reserves.sqrt_price_x96?;
        let liquidity = reserves.liquidity?;

        if sqrt_price.is_zero() || liquidity.is_zero() {
            return None;
        }

        let is_zero_for_one = token_in == self.token0;

        // Apply fee: fee_bps is in basis points (e.g., 30 = 0.3%).
        // V3 fees are in millionths, so fee_bps * 100 = fee in 1e-6 units.
        let fee_millionths = u64::from(self.fee_bps) * 100;
        if fee_millionths >= 1_000_000 {
            return None;
        }
        let fee_factor = U256::from(1_000_000u64 - fee_millionths);
        let amount_in_after_fee = amount_in * fee_factor / U256::from(1_000_000u64);

        let q96 = U256::from(1u64) << 96;

        if is_zero_for_one {
            // token0 in -> token1 out
            // new_sqrt_price = L * sqrt_price / (L + amount_in_after_fee * sqrt_price / 2^96)
            // amount_out = L * (sqrt_price - new_sqrt_price) / 2^96
            let l_x_sqrt = liquidity * sqrt_price;
            let denom = liquidity + amount_in_after_fee * sqrt_price / q96;
            if denom.is_zero() {
                return None;
            }
            let new_sqrt_price = l_x_sqrt / denom;
            if sqrt_price <= new_sqrt_price {
                return None; // No output (shouldn't happen for zero_for_one)
            }
            let delta_sqrt = sqrt_price - new_sqrt_price;
            let amount_out = liquidity * delta_sqrt / q96;
            Some(amount_out)
        } else {
            // token1 in -> token0 out
            // new_sqrt_price = sqrt_price + amount_in_after_fee * 2^96 / L
            // amount_out = L * 2^96 * (1/sqrt_price - 1/new_sqrt_price)
            //            = L * 2^96 * (new_sqrt_price - sqrt_price) / (sqrt_price * new_sqrt_price)
            let delta_sqrt = amount_in_after_fee * q96 / liquidity;
            let new_sqrt_price = sqrt_price + delta_sqrt;
            if new_sqrt_price.is_zero() || sqrt_price.is_zero() {
                return None;
            }
            // amount_out = L * (new_sqrt_price - sqrt_price) * 2^96 / (sqrt_price * new_sqrt_price)
            // Rewrite to avoid overflow: L * delta_sqrt / (sqrt_price * new_sqrt_price / 2^96)
            let price_product = sqrt_price * new_sqrt_price / q96;
            if price_product.is_zero() {
                return None;
            }
            let amount_out = liquidity * delta_sqrt / price_product;
            Some(amount_out)
        }
    }

    /// Get amount out based on pool type.
    pub fn get_amount_out(&self, amount_in: U256, token_in: Address) -> Option<U256> {
        match self.dex_type {
            DexType::UniswapV2 | DexType::SushiSwap => self.get_amount_out_v2(amount_in, token_in),
            DexType::UniswapV3 => self.get_amount_out_v3(amount_in, token_in),
        }
    }
}
