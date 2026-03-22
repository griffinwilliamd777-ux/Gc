use ethers::abi::{self, Token};
use ethers::prelude::*;
use ethers::types::{transaction::eip2718::TypedTransaction, Address, Bytes, U256};
use eyre::Result;
use std::sync::Arc;
use tracing::{info, warn};

use crate::arbitrage::opportunity::ArbOpportunity;
use crate::flashbots::client::FlashbotsClient;

/// Builds and submits arbitrage transactions to the on-chain ArbExecutor.
pub struct TradeExecutor<M: Middleware> {
    client: Arc<M>,
    wallet: LocalWallet,
    flashbots: Option<FlashbotsClient>,
    executor_address: Address,
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
        executor_address: Address,
        slippage_bps: u64,
        gas_limit_multiplier: f64,
        simulate_before_send: bool,
        dry_run: bool,
    ) -> Self {
        Self {
            client,
            wallet,
            flashbots,
            executor_address,
            slippage_bps,
            gas_limit_multiplier,
            simulate_before_send,
            dry_run,
        }
    }

    /// Execute an arbitrage opportunity via the on-chain ArbExecutor contract.
    /// Only executes 2-hop CrossDex V2 opportunities.
    pub async fn execute(
        &self,
        opportunity: &ArbOpportunity,
        current_block: u64,
        gas_price: U256,
    ) -> Result<Option<TxHash>> {
        if opportunity.path.hop_count() != 2 {
            info!(
                hops = opportunity.path.hop_count(),
                "Skipping non-2-hop opportunity (V2-only executor)"
            );
            return Ok(None);
        }

        info!(
            arb_type = %opportunity.arb_type,
            net_profit = %opportunity.net_profit,
            "Executing arbitrage"
        );

        let tx = self.build_transaction(opportunity, gas_price).await?;

        if self.dry_run {
            info!("DRY RUN — transaction not sent");
            info!(
                to = ?tx.to(),
                data_len = tx.data().map(|d| d.len()).unwrap_or(0),
                "Transaction details"
            );
            return Ok(None);
        }

        // Simulate via eth_call before sending.
        if self.simulate_before_send {
            match self.client.call(&tx, None).await {
                Ok(_) => info!("Simulation succeeded"),
                Err(e) => {
                    warn!(error = %e, "Simulation reverted — skipping");
                    return Ok(None);
                }
            }
        }

        // Send via Flashbots or direct.
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

    /// Build tx calling ArbExecutor.executeArb(tokenIn, tokenMid, pairBuy, pairSell, amountIn, minProfit).
    async fn build_transaction(
        &self,
        opportunity: &ArbOpportunity,
        gas_price: U256,
    ) -> Result<TypedTransaction> {
        let hops = &opportunity.path.hops;
        let token_in = hops[0].token_in;
        let token_mid = hops[0].token_out;
        let pair_buy = hops[0].pool_address;
        let pair_sell = hops[1].pool_address;
        let amount_in = opportunity.amount_in;

        // min_profit = gross_profit * (1 - slippage)
        let slippage_factor = U256::from(10000u64.saturating_sub(self.slippage_bps));
        let min_profit = opportunity.gross_profit * slippage_factor / U256::from(10000u64);

        // ABI encode: executeArb(address,address,address,address,uint256,uint256)
        let selector = &ethers::utils::keccak256(
            "executeArb(address,address,address,address,uint256,uint256)",
        )[0..4];

        let params = abi::encode(&[
            Token::Address(token_in),
            Token::Address(token_mid),
            Token::Address(pair_buy),
            Token::Address(pair_sell),
            Token::Uint(amount_in),
            Token::Uint(min_profit),
        ]);

        let mut calldata = Vec::with_capacity(4 + params.len());
        calldata.extend_from_slice(selector);
        calldata.extend_from_slice(&params);

        let gas_estimate =
            U256::from(300_000u64) * U256::from(opportunity.path.hop_count() as u64);
        let gas_limit = {
            let multiplied = gas_estimate.as_u64() as f64 * self.gas_limit_multiplier;
            U256::from(multiplied as u64)
        };

        let nonce = self
            .client
            .get_transaction_count(self.wallet.address(), None)
            .await
            .map_err(|e| eyre::eyre!("Failed to get nonce: {}", e))?;

        let mut tx = TypedTransaction::Legacy(Default::default());
        tx.set_to(self.executor_address);
        tx.set_data(Bytes::from(calldata));
        tx.set_gas(gas_limit);
        tx.set_gas_price(gas_price);
        tx.set_nonce(nonce);
        tx.set_value(U256::zero());

        Ok(tx)
    }
}
