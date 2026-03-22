use eyre::Result;
use serde::Deserialize;
use std::path::Path;

/// Top-level bot configuration loaded from TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct BotConfig {
    pub rpc: RpcConfig,
    pub flashbots: FlashbotsConfig,
    pub arbitrage: ArbitrageConfig,
    pub execution: ExecutionConfig,
    pub tokens: Vec<TokenConfig>,
    pub pools: Vec<PoolConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcConfig {
    /// Primary HTTP RPC endpoint
    pub http_url: String,
    /// WebSocket RPC endpoint for real-time events
    pub ws_url: String,
    /// Chain ID (1 = Ethereum mainnet, 8453 = Base, etc.)
    pub chain_id: u64,
    /// Polling interval in milliseconds for pool state
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FlashbotsConfig {
    /// Flashbots relay URL
    pub relay_url: String,
    /// Whether to use Flashbots for bundle submission
    pub enabled: bool,
    /// Max blocks into the future to target
    pub max_block_offset: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ArbitrageConfig {
    /// Minimum profit threshold in wei to execute a trade
    pub min_profit_wei: String,
    /// Maximum gas price in gwei we're willing to pay
    pub max_gas_gwei: u64,
    /// Whether to enable triangular arbitrage scanning
    pub enable_triangular: bool,
    /// Whether to enable cross-DEX arbitrage scanning
    pub enable_cross_dex: bool,
    /// Maximum number of hops for triangular arb
    pub max_hops: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExecutionConfig {
    /// Private key for signing transactions (loaded from env)
    pub private_key_env: String,
    /// Slippage tolerance in basis points (e.g., 50 = 0.5%)
    pub slippage_bps: u64,
    /// Gas limit multiplier (1.2 = 20% buffer)
    pub gas_limit_multiplier: f64,
    /// Whether to simulate transactions before sending
    pub simulate_before_send: bool,
    /// Dry run mode — log but don't send transactions
    pub dry_run: bool,
    /// Deployed ArbExecutor contract address
    pub executor_address: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenConfig {
    /// Human-readable symbol (e.g., "WETH")
    pub symbol: String,
    /// Contract address
    pub address: String,
    /// Token decimals
    pub decimals: u8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PoolConfig {
    /// DEX identifier (e.g., "uniswap_v2", "uniswap_v3", "sushiswap")
    pub dex: String,
    /// Pool / pair contract address
    pub address: String,
    /// Token0 symbol (must match a token in `tokens`)
    pub token0: String,
    /// Token1 symbol (must match a token in `tokens`)
    pub token1: String,
    /// Fee tier in basis points (for V3 pools)
    pub fee_bps: Option<u32>,
}

impl BotConfig {
    /// Load configuration from a TOML file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: BotConfig = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    /// Validate configuration values at startup.
    fn validate(&self) -> Result<()> {
        if self.execution.slippage_bps >= 10000 {
            return Err(eyre::eyre!(
                "slippage_bps must be < 10000, got {}",
                self.execution.slippage_bps
            ));
        }
        if self.execution.gas_limit_multiplier < 1.0 {
            return Err(eyre::eyre!(
                "gas_limit_multiplier must be >= 1.0, got {}",
                self.execution.gas_limit_multiplier
            ));
        }
        for pool in &self.pools {
            let fee = pool.fee_bps.unwrap_or(30);
            if fee >= 10000 {
                return Err(eyre::eyre!(
                    "Pool {} fee_bps must be < 10000, got {}",
                    pool.address,
                    fee
                ));
            }
        }
        if self.pools.is_empty() {
            return Err(eyre::eyre!("No pools configured"));
        }
        if self.tokens.is_empty() {
            return Err(eyre::eyre!("No tokens configured"));
        }
        Ok(())
    }
}
