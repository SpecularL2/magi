use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ethers::{
    providers::{Http, HttpRateLimitRetryPolicy, Middleware, Provider, RetryClient},
    types::{Block, H256, U64},
};
use eyre::Result;
use reqwest::Url;

use crate::{
    common::{BlockInfo, RawTransaction, Epoch},
    driver::sequencing::SequencingSource,
    engine::PayloadAttributes,
    l1::L1BlockInfo,
};

pub mod config;
use config::Config;

pub struct AttributesBuilder {
    config: Config,
    provider: Provider<RetryClient<Http>>,
}

impl AttributesBuilder {
    pub fn new(config: Config) -> Self {
        let provider = generate_http_provider(&config.l1_rpc_url);
        Self { config, provider }
    }

    /// Returns true iff:
    /// 1. The parent l2 block is within the max safe lag.
    /// 2. The next timestamp isn't in the future.
    fn is_ready(&self, parent_l2_block: &BlockInfo, safe_l2_head: &BlockInfo) -> bool {
        safe_l2_head.number + self.config.max_safe_lag > parent_l2_block.number
            && self.next_timestamp(parent_l2_block.timestamp) <= unix_now()
    }

    /// Returns the next l2 block timestamp, given the `parent_block_timestamp``.
    fn next_timestamp(&self, parent_block_timestamp: u64) -> u64 {
        parent_block_timestamp + self.config.blocktime
    }

    /// Returns the drift bound on the next l2 block's timestamp.
    fn next_drift_bound(&self, curr_origin: &L1BlockInfo) -> u64 {
        curr_origin.timestamp + self.config.max_seq_drift
    }

    /// Finds the origin of the next L2 block: either the current origin or the next, if sufficient time has passed.
    async fn find_next_origin(
        &self,
        curr_l2_block: &BlockInfo,
        curr_origin: &L1BlockInfo,
    ) -> Result<L1BlockInfo> {
        let next_l2_ts = self.next_timestamp(curr_l2_block.timestamp);
        let next_l1_block = self.provider.get_block(curr_origin.number + 1).await;
        let next_drift_bound = self.next_drift_bound(curr_origin);
        let is_past_drift_bound = next_l2_ts > next_drift_bound;
        if is_past_drift_bound {
            tracing::info!("Next l2ts exceeds the drift bound {}", next_drift_bound);
        }
        match (next_l1_block, is_past_drift_bound) {
            // We found the next l1 block.
            (Ok(Some(next_l1_block)), _) => {
                if next_l2_ts >= next_l1_block.timestamp.as_u64() {
                    try_create_l1_block_info(&next_l1_block)
                } else {
                    Ok(curr_origin.clone())
                }
            }
            // We're not exceeding the drift bound, so we can just use the current origin.
            (result, false) => {
                if result.is_err() {
                    tracing::warn!("Failed to get next l1 block: {:?}", result.err());
                }
                tracing::info!("Falling back to current origin (couldn't find next).");
                Ok(curr_origin.clone())
            }
            // We exceeded the drift bound, so we can't use the current origin.
            // But we also can't use the next l1 block since we didn't find it.
            (_, _) => Err(eyre::eyre!("current origin drift bound exceeded.")),
        }
    }
}

#[async_trait]
impl SequencingSource for AttributesBuilder {
    async fn get_next_attributes(
        &self,
        safe_l2_head: &BlockInfo,
        parent_l2_block: &BlockInfo,
        parent_l2_block_origin: &L1BlockInfo,
    ) -> Result<Option<PayloadAttributes>> {
        if !self.is_ready(parent_l2_block, safe_l2_head) {
            return Ok(None);
        }
        let next_origin = self
            .find_next_origin(parent_l2_block, parent_l2_block_origin)
            .await?;
        let timestamp = self.next_timestamp(parent_l2_block.timestamp);
        let prev_randao = next_randao(&next_origin);
        let suggested_fee_recipient = self.config.suggested_fee_recipient;
        let txs = create_top_of_block_transactions(&next_origin);
        let no_tx_pool = timestamp > self.config.max_seq_drift;
        let gas_limit = self.config.system_config.gas_limit;
        Ok(Some(PayloadAttributes {
            timestamp: U64([timestamp]),
            prev_randao,
            suggested_fee_recipient,
            transactions: Some(txs),
            no_tx_pool,
            gas_limit: U64([gas_limit]),
            epoch: Some(create_epoch(next_origin)),
            l1_inclusion_block: None,
            seq_number: None,
        }))
    }
}

// TODO: implement. requires l1 info tx. requires signer...
// Creates the transaction(s) to include at the top of the next l2 block.
fn create_top_of_block_transactions(_origin: &L1BlockInfo) -> Vec<RawTransaction> {
    vec![]
}

/// Returns the next l2 block randao, reusing that of the `next_origin`.
fn next_randao(next_origin: &L1BlockInfo) -> H256 {
    next_origin.mix_hash
}

/// Tries to extract l1 block info from `block`.
fn try_create_l1_block_info(block: &Block<H256>) -> Result<L1BlockInfo> {
    Ok(L1BlockInfo {
        number: block
            .number
            .ok_or(eyre::eyre!("block number missing"))?
            .as_u64(),
        hash: block.hash.ok_or(eyre::eyre!("block hash missing"))?,
        timestamp: block.timestamp.as_u64(),
        base_fee: block
            .base_fee_per_gas
            .ok_or(eyre::eyre!("base fee missing"))?,
        mix_hash: block.mix_hash.ok_or(eyre::eyre!("mix_hash missing"))?,
    })
}

fn create_epoch(info: L1BlockInfo) -> Epoch {
    Epoch {
        number: info.number,
        hash: info.hash,
        timestamp: info.timestamp,
    }
}

fn generate_http_provider(url: &str) -> Provider<RetryClient<Http>> {
    let client = reqwest::ClientBuilder::new()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let http = Http::new_with_client(Url::parse(url).expect("ivnalid rpc url"), client);
    let policy = Box::new(HttpRateLimitRetryPolicy);
    let client = RetryClient::new(http, policy, 100, 50);
    Provider::new(client)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
