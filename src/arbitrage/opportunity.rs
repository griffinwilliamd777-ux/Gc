use ethers::types::{Address, U256};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Type of arbitrage opportunity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArbType {
    /// Buy on DEX A, sell on DEX B for the same pair.
    CrossDex,
    /// A -> B -> C -> A within one or more DEXes.
    Triangular,
}

impl fmt::Display for ArbType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArbType::CrossDex => write!(f, "CrossDEX"),
            ArbType::Triangular => write!(f, "Triangular"),
        }
    }
}

/// A single hop in an arbitrage path.
#[derive(Debug, Clone)]
pub struct ArbHop {
    /// Pool address to trade through.
    pub pool_address: Address,
    /// DEX name for logging.
    pub dex_name: String,
    /// Token going in.
    pub token_in: Address,
    /// Token coming out.
    pub token_out: Address,
    /// Expected output amount for this hop.
    pub expected_out: U256,
}

/// The full path of an arbitrage opportunity.
#[derive(Debug, Clone)]
pub struct ArbPath {
    /// Ordered list of hops.
    pub hops: Vec<ArbHop>,
}

impl ArbPath {
    pub fn hop_count(&self) -> usize {
        self.hops.len()
    }
}

/// A detected arbitrage opportunity ready for execution.
#[derive(Debug, Clone)]
pub struct ArbOpportunity {
    /// Type of arb.
    pub arb_type: ArbType,
    /// The trading path.
    pub path: ArbPath,
    /// Input token.
    pub token_in: Address,
    /// Input amount.
    pub amount_in: U256,
    /// Expected output amount (same token as input).
    pub expected_out: U256,
    /// Expected gross profit in token_in terms.
    pub gross_profit: U256,
    /// Estimated gas cost in wei.
    pub estimated_gas_cost: U256,
    /// Net profit after gas.
    pub net_profit: U256,
    /// Block number when this opportunity was detected.
    pub block_number: u64,
    /// Timestamp of detection.
    pub detected_at: chrono::DateTime<chrono::Utc>,
}

impl ArbOpportunity {
    /// Whether this opportunity is profitable after gas.
    pub fn is_profitable(&self) -> bool {
        self.expected_out > self.amount_in && !self.net_profit.is_zero()
    }
}

impl fmt::Display for ArbOpportunity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} hops | in={} | out={} | profit={} | gas={} | net={}",
            self.arb_type,
            self.path.hop_count(),
            self.amount_in,
            self.expected_out,
            self.gross_profit,
            self.estimated_gas_cost,
            self.net_profit,
        )
    }
}
