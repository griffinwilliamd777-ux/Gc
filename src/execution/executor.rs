use ethers::prelude::*;
use ethers::types::{transaction::eip2718::TypedTransaction, Bytes, U256};
use eyre::Result;
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::arbitrage::opportunity::ArbOpportunity;
use crate::flashbots::client::FlashbotsClient;

/// Builds and submits arbitrage transactions.
pub struct TradeExecutor<M: Middleware> {
    client: Arc<M>,
    wallet: LocalWallet,
    flashbots: Option<FlashbotsClient>,
    slippage_bps: u64,
    gas_limit_multiplier: f64,
    simulate_before_send: bool,
    dry_run: bool,
}

impl<M: Middleware + 'static> TradeExecutor<M> {
    pub fn new(
        client: Arc<M>,
        wallet: LocalWallet,
        flashbots: Option<FlashbotsClient>,
        slippage_bps: u64,
        gas_limit_multiplier: f64,
        simulate_before_send: bool,
        dry_run: bool,
    ) -> Self {
        Self {
            client,
            wallet,
            flashbots,
            slippage_bps,
            gas_limit_multiplier,
            simulate_before_send,
            dry_run,
        }
    }

    /// Execute an arbitrage opportunity.
    pub async fn execute(
        &self,
        opportunity: &ArbOpportunity,
        current_block: u64,
        gas_price: U256,
    ) -> Result<Option<TxHash>> {
        info!(
            arb_type = %opportunity.arb_type,
            net_profit = %opportunity.net_profit,
            hops = opportunity.path.hop_count(),
            "Executing arbitrage"
        );

        // Build the multicall / swap transaction.
        let tx = self.build_transaction(opportunity, gas_price).await?;

        if self.dry_run {
            info!("DRY RUN — transaction not sent");
            info!(to = ?tx.to(), data_len = tx.data().map(|d| d.len()).unwrap_or(0), "Transaction details");
            return Ok(None);
        }

        // Simulate if configured.
        if self.simulate_before_send {
            match self.simulate_transaction(&tx).await {
                Ok(success) => {
                    if !success {
                        warn!("Simulation failed — skipping execution");
                        return Ok(None);
                    }
                    info!("Simulation succeeded");
                }
                Err(e) => {
                    error!(error = %e, "Simulation error — skipping execution");
                    return Ok(None);
                }
            }
        }

        // Send via Flashbots if available, otherwise direct.
        if let Some(ref flashbots) = self.flashbots {
            let tx_hash = flashbots
                .send_bundle(tx, &self.wallet, current_block)
                .await?;
            info!(tx_hash = %tx_hash, "Bundle submitted via Flashbots");
            Ok(Some(tx_hash))
        } else {
            let pending = self
                .client
                .send_transaction(tx, None)
                .await
                .map_err(|e| eyre::eyre!("Failed to send transaction: {}", e))?;
            let tx_hash = pending.tx_hash();
            info!(tx_hash = %tx_hash, "Transaction sent directly");
            Ok(Some(tx_hash))
        }
    }

    /// Build the swap transaction for the arbitrage path.
    async fn build_transaction(
        &self,
        opportunity: &ArbOpportunity,
        gas_price: U256,
    ) -> Result<TypedTransaction> {
        // Calculate minimum output with slippage protection.
        let min_output = self.apply_slippage(opportunity.expected_out);

        // For a production bot, you'd encode a multicall through a custom
        // router contract that executes all hops atomically.
        // Here we build a representative transaction structure.
        let calldata = self.encode_swap_calldata(opportunity, min_output)?;

        // Use the first pool as target (in production, use custom router).
        let target = opportunity.path.hops[0].pool_address;

        let gas_estimate = U256::from(150_000u64) * U256::from(opportunity.path.hop_count() as u64);
        let gas_limit = self.apply_gas_multiplier(gas_estimate);

        let gas_price = self
            .client
            .get_gas_price()
            .await
            .map_err(|e| eyre::eyre!("Failed to get gas price: {}", e))?;

        let nonce = self
            .client
            .get_transaction_count(self.wallet.address(), None)
            .await
            .map_err(|e| eyre::eyre!("Failed to get nonce: {}", e))?;

        let mut tx = TypedTransaction::Legacy(Default::default());
        tx.set_to(target);
        tx.set_data(calldata);
        tx.set_gas(gas_limit);
        tx.set_gas_price(gas_price);
        tx.set_nonce(nonce);
        tx.set_value(U256::zero());

        Ok(tx)
    }

    /// Encode the swap calldata for the arbitrage path.
    /// In production, this would call a custom router contract.
    fn encode_swap_calldata(
        &self,
        opportunity: &ArbOpportunity,
        min_output: U256,
    ) -> Result<Bytes> {
        // Build a simplified encoding of the swap path.
        // A real implementation would use a custom smart contract ABI.
        //
        // Example encoding for a multicall router:
        // function executeArbitrage(
        //   address[] memory pools,
        //   address[] memory tokens,
        //   uint256 amountIn,
        //   uint256 minAmountOut
        // )
        let mut data = Vec::new();

        // Function selector (keccak256("executeArbitrage(address[],address[],uint256,uint256)"))
        data.extend_from_slice(&[0x12, 0x34, 0x56, 0x78]); // Placeholder selector

        // Encode pools.
        for hop in &opportunity.path.hops {
            let pool_bytes: [u8; 20] = hop.pool_address.into();
            data.extend_from_slice(&[0u8; 12]); // Left-pad to 32 bytes
            data.extend_from_slice(&pool_bytes);
        }

        // Encode amount_in (32 bytes).
        let mut amount_bytes = [0u8; 32];
        opportunity.amount_in.to_big_endian(&mut amount_bytes);
        data.extend_from_slice(&amount_bytes);

        // Encode min_output (32 bytes).
        let mut min_bytes = [0u8; 32];
        min_output.to_big_endian(&mut min_bytes);
        data.extend_from_slice(&min_bytes);

        Ok(Bytes::from(data))
    }

    /// Apply slippage tolerance to an amount.
    fn apply_slippage(&self, amount: U256) -> U256 {
        // min_out = amount * (10000 - slippage_bps) / 10000
        let factor = U256::from(10000u64 - self.slippage_bps);
        amount * factor / U256::from(10000u64)
    }

    /// Apply gas limit multiplier.
    fn apply_gas_multiplier(&self, gas: U256) -> U256 {
        let multiplied = gas.as_u64() as f64 * self.gas_limit_multiplier;
        U256::from(multiplied as u64)
    }

    /// Simulate a transaction using eth_call.
    async fn simulate_transaction(&self, tx: &TypedTransaction) -> Result<bool> {
        match self.client.call(tx, None).await {
            Ok(_output) => Ok(true),
            Err(e) => {
                warn!(error = %e, "Transaction simulation reverted");
                Ok(false)
            }
        }
    }
}
