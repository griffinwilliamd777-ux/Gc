use ethers::prelude::*;
use ethers::types::{Address, U256};
use eyre::Result;
use std::sync::Arc;
use tracing::info;

use super::pool::{DexPool, DexType, PoolReserves, PoolState};

// Uniswap V3 Pool ABI — slot0() and liquidity()
abigen!(
    IUniswapV3Pool,
    r#"[
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint8 feeProtocol, bool unlocked)
        function liquidity() external view returns (uint128)
        function token0() external view returns (address)
        function token1() external view returns (address)
        function fee() external view returns (uint24)
    ]"#
);

/// Fetch current state for a Uniswap V3 pool.
pub async fn fetch_state_v3<M: Middleware + 'static>(
    client: Arc<M>,
    pool: &mut DexPool,
) -> Result<()> {
    let v3_pool = IUniswapV3Pool::new(pool.address, client);

    let (sqrt_price_x96, tick, _, _, _, _, _) = v3_pool.slot_0().call().await?;
    let liquidity = v3_pool.liquidity().call().await?;

    pool.state = PoolState::Active(PoolReserves {
        reserve0: U256::zero(),
        reserve1: U256::zero(),
        sqrt_price_x96: Some(U256::from(sqrt_price_x96)),
        tick: Some(tick),
        liquidity: Some(U256::from(liquidity)),
    });

    info!(
        pool = %pool.address,
        dex = %pool.dex_type,
        sqrt_price = %sqrt_price_x96,
        tick = tick,
        liquidity = %liquidity,
        "Updated V3 state"
    );

    Ok(())
}

/// Build a DexPool from config for a V3 DEX.
pub fn build_v3_pool(
    pool_address: Address,
    token0: Address,
    token1: Address,
    token0_decimals: u8,
    token1_decimals: u8,
    fee_bps: u32,
) -> DexPool {
    DexPool {
        dex_type: DexType::UniswapV3,
        address: pool_address,
        token0,
        token1,
        token0_decimals,
        token1_decimals,
        fee_bps,
        state: PoolState::Uninitialized,
    }
}
