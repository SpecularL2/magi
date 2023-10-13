use std::time::Duration;

use ethers::{
    providers::{Http, HttpRateLimitRetryPolicy, JsonRpcClient, Middleware, Provider, RetryClient},
    types::{Block, BlockId, H256},
};
use eyre::{Result, WrapErr};
use reqwest::Url;

use crate::l1::L1BlockInfo;

pub async fn get_l1_block_info<T: JsonRpcClient, U: Into<BlockId> + Send + Sync>(
    provider: &Provider<T>,
    block_id: U,
) -> Result<L1BlockInfo> {
    let block = provider.get_block(block_id).await;
    block
        .wrap_err_with(|| "failed to get l1 block")
        .and_then(|b| b.ok_or(eyre::eyre!("no l1 block found")))
        .and_then(|b| try_create_l1_block_info(&b))
}

pub fn generate_http_provider(url: &str) -> Provider<RetryClient<Http>> {
    let client = reqwest::ClientBuilder::new()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let http = Http::new_with_client(Url::parse(url).expect("ivnalid rpc url"), client);
    let policy = Box::new(HttpRateLimitRetryPolicy);
    let client = RetryClient::new(http, policy, 100, 50);
    Provider::new(client)
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
