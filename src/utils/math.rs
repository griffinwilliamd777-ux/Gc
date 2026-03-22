use ethers::types::U256;

/// Convert a human-readable amount to its wei representation.
/// e.g., "1.5" ETH with 18 decimals -> 1_500_000_000_000_000_000
pub fn to_wei(amount: f64, decimals: u8) -> U256 {
    let factor = 10u64.pow(decimals as u32);
    let wei = (amount * factor as f64) as u128;
    U256::from(wei)
}

/// Convert wei to a human-readable float.
pub fn from_wei(wei: U256, decimals: u8) -> f64 {
    let factor = 10u64.pow(decimals as u32) as f64;
    let val = wei.as_u128() as f64;
    val / factor
}

/// Calculate the price of token1 in terms of token0 from reserves.
/// price = reserve0 / reserve1 (adjusted for decimals)
pub fn price_from_reserves(
    reserve0: U256,
    reserve1: U256,
    decimals0: u8,
    decimals1: u8,
) -> f64 {
    let r0 = from_wei(reserve0, decimals0);
    let r1 = from_wei(reserve1, decimals1);
    if r1 == 0.0 {
        return 0.0;
    }
    r0 / r1
}

/// Calculate percentage difference between two prices.
pub fn price_diff_pct(price_a: f64, price_b: f64) -> f64 {
    if price_a == 0.0 {
        return 0.0;
    }
    ((price_b - price_a) / price_a) * 100.0
}

/// Find the optimal input amount for a two-pool arbitrage.
/// Based on the constant product formula.
/// Returns the optimal amount_in that maximizes profit.
pub fn optimal_input_v2(
    reserve_a_in: U256,
    reserve_a_out: U256,
    reserve_b_in: U256,
    reserve_b_out: U256,
    fee_bps: u64,
) -> Option<U256> {
    // For constant product AMMs:
    // Optimal input ≈ sqrt(r_a_in * r_a_out * r_b_in * r_b_out * f^2) - r_a_in * f
    // where f = (10000 - fee_bps) / 10000
    //
    // Simplified: we use a numerical approach to find the optimal input.
    let fee_factor = (10000.0 - fee_bps as f64) / 10000.0;

    let ra_in = reserve_a_in.as_u128() as f64;
    let ra_out = reserve_a_out.as_u128() as f64;
    let rb_out = reserve_b_in.as_u128() as f64; // token_in reserve on pool B
    let rb_in = reserve_b_out.as_u128() as f64; // token_out reserve on pool B

    if ra_in == 0.0 || ra_out == 0.0 || rb_in == 0.0 || rb_out == 0.0 {
        return None;
    }

    // Optimal input for cross-DEX arb:
    // amount_opt = sqrt(ra_in * rb_in * f^2 * ra_out * rb_out) / (ra_out * f + rb_in) - ra_in
    // This is a simplified approximation.
    let numerator = (ra_in * ra_out * rb_in * rb_out * fee_factor * fee_factor).sqrt();
    let denominator = ra_out * fee_factor + rb_in;

    if denominator == 0.0 {
        return None;
    }

    let optimal = numerator / denominator;
    if optimal <= 0.0 || optimal > u128::MAX as f64 {
        return None;
    }

    Some(U256::from(optimal as u128))
}
