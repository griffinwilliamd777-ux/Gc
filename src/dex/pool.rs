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

    /// Calculate output for V3 using simplified sqrt price math.
    /// This is a simplified model — production would use tick-level liquidity.
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

        // Simplified V3 output calculation.
        // In production, iterate through ticks for accurate output.
        // For a small swap within current tick range:
        // If token0 -> token1: delta_y = L * (sqrt_p_new - sqrt_p) simplified
        // If token1 -> token0: delta_x = L * (1/sqrt_p_new - 1/sqrt_p)

        // Apply fee
        let fee_factor = U256::from(1_000_000u64 - u64::from(self.fee_bps) * 100);
        let amount_in_after_fee = amount_in * fee_factor / U256::from(1_000_000u64);

        if is_zero_for_one {
            // token0 in -> token1 out
            // Approximate: use price = (sqrt_price / 2^96)^2
            // amount_out ≈ amount_in * price * fee_factor
            let price_sq = sqrt_price * sqrt_price;
            let q96_sq = U256::from(1u64) << 192;
            if q96_sq.is_zero() {
                return None;
            }
            let amount_out = amount_in_after_fee * price_sq / q96_sq;
            Some(amount_out)
        } else {
            // token1 in -> token0 out
            let q96_sq = U256::from(1u64) << 192;
            let price_sq = sqrt_price * sqrt_price;
            if price_sq.is_zero() {
                return None;
            }
            let amount_out = amount_in_after_fee * q96_sq / price_sq;
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
