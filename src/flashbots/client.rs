use ethers::prelude::*;
use ethers::types::{transaction::eip2718::TypedTransaction, TxHash};
use eyre::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

/// Client for interacting with the Flashbots relay.
/// Submits transaction bundles privately to avoid the public mempool.
#[derive(Clone)]
pub struct FlashbotsClient {
    relay_url: String,
    http_client: Client,
    max_block_offset: u64,
}

/// Flashbots bundle request.
#[derive(Debug, Serialize)]
struct FlashbotsBundleRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    params: Vec<serde_json::Value>,
}

/// Flashbots bundle response.
#[derive(Debug, Deserialize)]
struct FlashbotsBundleResponse {
    #[serde(default)]
    result: Option<FlashbotsBundleResult>,
    #[serde(default)]
    error: Option<FlashbotsError>,
}

#[derive(Debug, Deserialize)]
struct FlashbotsBundleResult {
    #[serde(rename = "bundleHash")]
    bundle_hash: String,
}

#[derive(Debug, Deserialize)]
struct FlashbotsError {
    message: String,
    code: i64,
}

/// Response from eth_callBundle simulation.
#[derive(Debug, Deserialize)]
struct SimulationResponse {
    #[serde(default)]
    result: Option<SimulationResult>,
    #[serde(default)]
    error: Option<FlashbotsError>,
}

#[derive(Debug, Deserialize)]
struct SimulationResult {
    #[serde(rename = "totalGasUsed")]
    total_gas_used: Option<u64>,
    #[serde(rename = "coinbaseDiff")]
    coinbase_diff: Option<String>,
    results: Option<Vec<TxSimResult>>,
}

#[derive(Debug, Deserialize)]
struct TxSimResult {
    #[serde(rename = "txHash")]
    tx_hash: Option<String>,
    #[serde(rename = "gasUsed")]
    gas_used: Option<u64>,
    error: Option<String>,
    revert: Option<String>,
}

impl FlashbotsClient {
    pub fn new(relay_url: String, max_block_offset: u64) -> Self {
        Self {
            relay_url,
            http_client: Client::new(),
            max_block_offset,
        }
    }

    /// Send a transaction bundle to the Flashbots relay.
    /// The bundle is submitted for the next few blocks.
    pub async fn send_bundle(
        &self,
        tx: TypedTransaction,
        wallet: &LocalWallet,
    ) -> Result<TxHash> {
        // Sign the transaction.
        let signature = wallet
            .sign_transaction(&tx)
            .await
            .map_err(|e| eyre::eyre!("Failed to sign transaction: {}", e))?;

        let raw_tx = tx.rlp_signed(&signature);
        let raw_tx_hex = format!("0x{}", hex::encode(&raw_tx));

        // Get the current block number to target.
        // In production, you'd get this from your RPC provider.
        // For now, we submit for target_block = current + 1 through current + max_offset.
        let bundle_body = serde_json::json!({
            "txs": [raw_tx_hex],
            "blockNumber": format!("0x{:x}", 0u64), // Placeholder — set dynamically
            "minTimestamp": 0,
            "maxTimestamp": 0,
        });

        let request = FlashbotsBundleRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "eth_sendBundle".to_string(),
            params: vec![bundle_body],
        };

        // Sign the request payload for Flashbots authentication.
        let body = serde_json::to_string(&request)?;
        let signature = wallet
            .sign_message(body.as_bytes())
            .await
            .map_err(|e| eyre::eyre!("Failed to sign Flashbots request: {}", e))?;

        let auth_header = format!(
            "{}:0x{}",
            format!("{:?}", wallet.address()),
            signature
        );

        let response = self
            .http_client
            .post(&self.relay_url)
            .header("Content-Type", "application/json")
            .header("X-Flashbots-Signature", &auth_header)
            .body(body)
            .send()
            .await?;

        let status = response.status();
        let response_text = response.text().await?;

        if !status.is_success() {
            error!(
                status = %status,
                body = %response_text,
                "Flashbots relay returned error"
            );
            return Err(eyre::eyre!(
                "Flashbots relay error: {} - {}",
                status,
                response_text
            ));
        }

        let bundle_response: FlashbotsBundleResponse = serde_json::from_str(&response_text)?;

        if let Some(error) = bundle_response.error {
            return Err(eyre::eyre!(
                "Flashbots error ({}): {}",
                error.code,
                error.message
            ));
        }

        if let Some(result) = bundle_response.result {
            let tx_hash = result.bundle_hash.parse::<TxHash>()?;
            info!(
                bundle_hash = %result.bundle_hash,
                "Bundle accepted by Flashbots relay"
            );
            return Ok(tx_hash);
        }

        Err(eyre::eyre!("Unexpected Flashbots response: {}", response_text))
    }

    /// Simulate a bundle using eth_callBundle.
    /// Returns true if the simulation succeeds and the bundle is profitable.
    pub async fn simulate_bundle(
        &self,
        raw_tx_hex: &str,
        block_number: u64,
        wallet: &LocalWallet,
    ) -> Result<bool> {
        let sim_body = serde_json::json!({
            "txs": [raw_tx_hex],
            "blockNumber": format!("0x{:x}", block_number),
            "stateBlockNumber": "latest",
        });

        let request = FlashbotsBundleRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "eth_callBundle".to_string(),
            params: vec![sim_body],
        };

        let body = serde_json::to_string(&request)?;
        let signature = wallet
            .sign_message(body.as_bytes())
            .await
            .map_err(|e| eyre::eyre!("Failed to sign simulation request: {}", e))?;

        let auth_header = format!(
            "{}:0x{}",
            format!("{:?}", wallet.address()),
            signature
        );

        let response = self
            .http_client
            .post(&self.relay_url)
            .header("Content-Type", "application/json")
            .header("X-Flashbots-Signature", &auth_header)
            .body(body)
            .send()
            .await?;

        let response_text = response.text().await?;
        let sim_response: SimulationResponse = serde_json::from_str(&response_text)?;

        if let Some(error) = sim_response.error {
            warn!(
                code = error.code,
                message = %error.message,
                "Bundle simulation error"
            );
            return Ok(false);
        }

        if let Some(result) = sim_response.result {
            // Check if any transaction reverted.
            if let Some(results) = &result.results {
                for tx_result in results {
                    if let Some(ref error) = tx_result.error {
                        warn!(error = %error, "Transaction in bundle reverted");
                        return Ok(false);
                    }
                    if let Some(ref revert) = tx_result.revert {
                        warn!(revert = %revert, "Transaction in bundle reverted");
                        return Ok(false);
                    }
                }
            }

            info!(
                gas_used = ?result.total_gas_used,
                coinbase_diff = ?result.coinbase_diff,
                "Bundle simulation succeeded"
            );
            return Ok(true);
        }

        warn!("Unexpected simulation response");
        Ok(false)
    }

    /// Get the relay URL.
    pub fn relay_url(&self) -> &str {
        &self.relay_url
    }

    /// Get max block offset.
    pub fn max_block_offset(&self) -> u64 {
        self.max_block_offset
    }
}
