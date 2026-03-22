use ethers::prelude::*;
use ethers::types::{Address, U256};
use eyre::Result;
use std::sync::Arc;
use tracing::info;

use super::pool::{DexPool, DexType, PoolReserves, PoolState};

// Uniswap V2 Pair ABI — getReserves()
abigen!(
    IUniswapV2Pair,
    r#"[
        function getReserves() external view returns (uint112 reserve0, uint112 reserve1, uint32 blockTimestampLast)
        function token0() external view returns (address)
        function token1() external view returns (address)
    ]"#
);

/// Fetch current reserves for a Uniswap V2 style pool.
pub async fn fetch_reserves_v2<M: Middleware + 'static>(
    client: Arc<M>,
    pool: &mut DexPool,
) -> Result<()> {
    let pair = IUniswapV2Pair::new(pool.address, client);
    let (reserve0, reserve1, _timestamp) = pair.get_reserves().call().await?;

    pool.state = PoolState::Active(PoolReserves {
        reserve0: U256::from(reserve0),
        reserve1: U256::from(reserve1),
        sqrt_price_x96: None,
        tick: None,
        liquidity: None,
    });

    info!(
        pool = %pool.address,
        dex = %pool.dex_type,
        reserve0 = %reserve0,
        reserve1 = %reserve1,
        "Updated V2 reserves"
    );

    Ok(())
}

/// Build a DexPool from config for a V2-style DEX.
pub fn build_v2_pool(
    dex_type: DexType,
    pool_address: Address,
    token0: Address,
    token1: Address,
    token0_decimals: u8,
    token1_decimals: u8,
    fee_bps: u32,
) -> DexPool {
    DexPool {
        dex_type,
        address: pool_address,
        token0,
        token1,
        token0_decimals,
        token1_decimals,
        fee_bps,
        state: PoolState::Uninitialized,
    }
}
