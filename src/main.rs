use std::path::PathBuf;
use std::sync::Arc;
use std::str::FromStr;

use clap::Parser;
use ethers::prelude::*;
use ethers::types::{Address, U256};
use eyre::Result;
use tokio::time::{self, Duration};
use tracing::{error, info, warn};

use arb_bot::arbitrage::ArbitrageDetector;
use arb_bot::config::{BotConfig, PoolConfig};
use arb_bot::dex::pool::{DexPool, DexType};
use arb_bot::dex::uniswap_v2;
use arb_bot::dex::uniswap_v3;
use arb_bot::execution::{GasOptimizer, TradeExecutor};
use arb_bot::flashbots::FlashbotsClient;
use arb_bot::utils::init_logging;

/// High-performance cross-DEX arbitrage bot with Flashbots integration.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Path to the configuration file.
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env if present.
    dotenv::dotenv().ok();

    // Initialize logging.
    init_logging();

    info!("Starting arb-bot");

    // Parse CLI args.
    let cli = Cli::parse();

    // Load configuration.
    let config = BotConfig::from_file(&cli.config)?;
    info!(chain_id = config.rpc.chain_id, "Configuration loaded");

    // Connect to Ethereum node via HTTP.
    let provider = Provider::<Http>::try_from(&config.rpc.http_url)?;
    let client = Arc::new(provider);

    // Load wallet from environment variable.
    let private_key = std::env::var(&config.execution.private_key_env)
        .map_err(|_| eyre::eyre!("Private key env var '{}' not set", config.execution.private_key_env))?;
    let wallet: LocalWallet = private_key
        .parse::<LocalWallet>()?
        .with_chain_id(config.rpc.chain_id);
    info!(address = %wallet.address(), "Wallet loaded");

    // Build token address lookup.
    let token_map: std::collections::HashMap<String, (Address, u8)> = config
        .tokens
        .iter()
        .map(|t| {
            let addr = Address::from_str(&t.address).expect("Invalid token address");
            (t.symbol.clone(), (addr, t.decimals))
        })
        .collect();

    // Build pool objects.
    let mut pools: Vec<DexPool> = Vec::new();
    for pool_cfg in &config.pools {
        let pool = build_pool_from_config(pool_cfg, &token_map)?;
        pools.push(pool);
    }
    info!(count = pools.len(), "Pools configured");

    // Initialize Flashbots client if enabled.
    let flashbots = if config.flashbots.enabled {
        info!(relay = %config.flashbots.relay_url, "Flashbots enabled");
        Some(FlashbotsClient::new(
            config.flashbots.relay_url.clone(),
            config.flashbots.max_block_offset,
        ))
    } else {
        info!("Flashbots disabled — transactions will be sent directly");
        None
    };

    // Initialize gas optimizer.
    let mut gas_optimizer = GasOptimizer::new(client.clone(), config.arbitrage.max_gas_gwei);

    // Initialize arbitrage detector.
    let min_profit = U256::from_dec_str(&config.arbitrage.min_profit_wei)
        .map_err(|_| eyre::eyre!("Invalid min_profit_wei"))?;
    let initial_gas_price = gas_optimizer.get_optimal_gas_price().await?;
    let mut detector = ArbitrageDetector::new(
        min_profit,
        initial_gas_price,
        config.arbitrage.enable_triangular,
        config.arbitrage.enable_cross_dex,
        config.arbitrage.max_hops,
    );

    // Parse executor contract address.
    let executor_address = Address::from_str(&config.execution.executor_address)
        .map_err(|_| eyre::eyre!("Invalid executor_address: {}", config.execution.executor_address))?;
    info!(executor = %executor_address, "Executor contract configured");

    // Initialize trade executor.
    let executor = TradeExecutor::new(
        client.clone(),
        wallet,
        flashbots,
        executor_address,
        config.execution.slippage_bps,
        config.execution.gas_limit_multiplier,
        config.execution.simulate_before_send,
        config.execution.dry_run,
    );

    // Build token-aware input amounts: test multiple sizes per token.
    // For 18-decimal tokens (ETH/DAI): 1, 5, 10 units.
    // For 6-decimal tokens (USDC/USDT): 1000, 5000, 10000 units.
    let mut input_amounts: Vec<U256> = Vec::new();
    for token in &config.tokens {
        let base = U256::from(10u64).pow(U256::from(token.decimals));
        if token.decimals >= 12 {
            // 18-decimal tokens: 1, 5, 10
            input_amounts.push(base);
            input_amounts.push(base * U256::from(5u64));
            input_amounts.push(base * U256::from(10u64));
        } else {
            // 6-decimal tokens: 1000, 5000, 10000
            input_amounts.push(base * U256::from(1000u64));
            input_amounts.push(base * U256::from(5000u64));
            input_amounts.push(base * U256::from(10000u64));
        }
    }
    input_amounts.sort();
    input_amounts.dedup();

    info!("Entering main loop");

    // Main bot loop.
    let poll_interval = Duration::from_millis(config.rpc.poll_interval_ms);
    let mut interval = time::interval(poll_interval);

    loop {
        interval.tick().await;

        // Update gas price.
        let gas_price = match gas_optimizer.get_optimal_gas_price().await {
            Ok(gas_price) => {
                detector.update_gas_price(gas_price);
                gas_price
            }
            Err(e) => {
                warn!(error = %e, "Failed to update gas price");
                continue;
            }
        };

        // Check if gas is acceptable (without refetching from RPC).
        if gas_price > gas_optimizer.max_gas_price() {
            info!(gas_price = %gas_price, "Gas price too high — skipping this cycle");
            continue;
        }

        // Refresh pool states.
        for pool in &mut pools {
            let result = match pool.dex_type {
                DexType::UniswapV2 | DexType::SushiSwap => {
                    uniswap_v2::fetch_reserves_v2(client.clone(), pool).await
                }
                DexType::UniswapV3 => {
                    uniswap_v3::fetch_state_v3(client.clone(), pool).await
                }
            };

            if let Err(e) = result {
                warn!(pool = %pool.address, error = %e, "Failed to update pool state");
            }
        }

        // Get current block number.
        let block_number = match client.get_block_number().await {
            Ok(bn) => bn.as_u64(),
            Err(e) => {
                warn!(error = %e, "Failed to get block number");
                continue;
            }
        };

        // Scan for arbitrage opportunities.
        let opportunities = detector.scan(&pools, &input_amounts, block_number);

        // Execute the best opportunity.
        if let Some(best) = opportunities.first() {
            info!(opportunity = %best, "Executing best opportunity");

            match executor.execute(best, block_number, gas_price).await {
                Ok(Some(tx_hash)) => {
                    info!(tx_hash = %tx_hash, "Trade executed successfully");
                }
                Ok(None) => {
                    info!("Trade skipped (dry run or simulation failure)");
                }
                Err(e) => {
                    error!(error = %e, "Trade execution failed");
                }
            }
        }
    }
}

/// Build a DexPool from a PoolConfig.
fn build_pool_from_config(
    pool_cfg: &PoolConfig,
    token_map: &std::collections::HashMap<String, (Address, u8)>,
) -> Result<DexPool> {
    let (token0_addr, token0_dec) = token_map
        .get(&pool_cfg.token0)
        .ok_or_else(|| eyre::eyre!("Token '{}' not found in config", pool_cfg.token0))?;
    let (token1_addr, token1_dec) = token_map
        .get(&pool_cfg.token1)
        .ok_or_else(|| eyre::eyre!("Token '{}' not found in config", pool_cfg.token1))?;

    let pool_address = Address::from_str(&pool_cfg.address)
        .map_err(|_| eyre::eyre!("Invalid pool address: {}", pool_cfg.address))?;

    let fee_bps = pool_cfg.fee_bps.unwrap_or(30); // Default 0.3% for V2

    match pool_cfg.dex.as_str() {
        "uniswap_v2" => Ok(uniswap_v2::build_v2_pool(
            DexType::UniswapV2,
            pool_address,
            *token0_addr,
            *token1_addr,
            *token0_dec,
            *token1_dec,
            fee_bps,
        )),
        "sushiswap" => Ok(uniswap_v2::build_v2_pool(
            DexType::SushiSwap,
            pool_address,
            *token0_addr,
            *token1_addr,
            *token0_dec,
            *token1_dec,
            fee_bps,
        )),
        "uniswap_v3" => Ok(uniswap_v3::build_v3_pool(
            pool_address,
            *token0_addr,
            *token1_addr,
            *token0_dec,
            *token1_dec,
            fee_bps,
        )),
        other => Err(eyre::eyre!("Unknown DEX type: {}", other)),
    }
}
