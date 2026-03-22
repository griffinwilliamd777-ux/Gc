use ethers::prelude::*;
use ethers::types::{transaction::eip2718::TypedTransaction, U256};
use eyre::Result;
use std::sync::Arc;
use tracing::{info, warn};

/// Local transaction simulator.
/// Runs eth_call to verify transactions will succeed before submission.
pub struct TransactionSimulator<M: Middleware> {
    client: Arc<M>,
}

impl<M: Middleware + 'static> TransactionSimulator<M> {
    pub fn new(client: Arc<M>) -> Self {
        Self { client }
    }

    /// Simulate a transaction using eth_call.
    /// Returns Ok(true) if the call succeeds, Ok(false) if it reverts.
    pub async fn simulate(&self, tx: &TypedTransaction) -> Result<bool> {
        match self.client.call(tx, None).await {
            Ok(output) => {
                info!(
                    output_len = output.len(),
                    "Transaction simulation succeeded"
                );
                Ok(true)
            }
            Err(e) => {
                warn!(error = %e, "Transaction simulation failed");
                Ok(false)
            }
        }
    }

    /// Estimate gas for a transaction.
    pub async fn estimate_gas(&self, tx: &TypedTransaction) -> Result<U256> {
        let gas = self
            .client
            .estimate_gas(tx, None)
            .await
            .map_err(|e| eyre::eyre!("Gas estimation failed: {}", e))?;
        info!(gas = %gas, "Gas estimated");
        Ok(gas)
    }

    /// Simulate and return detailed results.
    pub async fn simulate_with_details(
        &self,
        tx: &TypedTransaction,
    ) -> Result<SimulationResult> {
        let call_result = self.client.call(tx, None).await;
        let gas_estimate = self.client.estimate_gas(tx, None).await;

        match (call_result, gas_estimate) {
            (Ok(output), Ok(gas)) => Ok(SimulationResult {
                success: true,
                output: Some(output),
                gas_used: Some(gas),
                error: None,
            }),
            (Err(e), _) => Ok(SimulationResult {
                success: false,
                output: None,
                gas_used: None,
                error: Some(format!("{}", e)),
            }),
            (Ok(output), Err(e)) => Ok(SimulationResult {
                success: true,
                output: Some(output),
                gas_used: None,
                error: Some(format!("Gas estimation failed: {}", e)),
            }),
        }
    }
}

/// Result of a transaction simulation.
#[derive(Debug)]
pub struct SimulationResult {
    /// Whether the simulation succeeded.
    pub success: bool,
    /// Raw output bytes from the call.
    pub output: Option<ethers::types::Bytes>,
    /// Estimated gas usage.
    pub gas_used: Option<U256>,
    /// Error message if simulation failed.
    pub error: Option<String>,
}
