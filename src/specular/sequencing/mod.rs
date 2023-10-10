use std::time::{SystemTime, UNIX_EPOCH, Duration};

use async_trait::async_trait;
use ethers::{types::{U64, H256, Block}, providers::{Provider, Http, RetryClient, HttpRateLimitRetryPolicy, Middleware}};
use reqwest::Url;

use crate::{config::Config, engine::PayloadAttributes, common::{BlockInfo, RawTransaction}, derive::state::State, driver::sequencing::SequencingSource, l1::L1BlockInfo};

pub struct AttributesBuilder {
    config: Config,
    provider: Provider<RetryClient<Http>>,
}

impl AttributesBuilder {
    pub fn new(config: Config) -> Self {
        let url = config.l1_rpc_url.clone();
        Self {
            config,
            provider: generate_http_provider(&url)
        }
    }

    /// Returns true iff:
    /// 1. The parent l2 block is within the max safe lag.
    /// 2. The next timestamp isn't in the future.
    fn is_ready(&self, parent_l2_block: BlockInfo, state: &State) -> bool { 
        state.safe_head.number + self.config.local_sequencer.max_safe_lag > parent_l2_block.number &&
        self.next_timestamp(parent_l2_block.timestamp) <= unix_now()
    }

    fn next_timestamp(&self, parent_block_timestamp: u64) -> u64 { parent_block_timestamp + self.config.chain.blocktime }

    fn next_randao(&self, next_block_epoch: L1BlockInfo) -> H256 { next_block_epoch.mix_hash }

    async fn find_next_origin(&self, _l2_block: BlockInfo, l2_block_origin: L1BlockInfo) -> L1BlockInfo {
        // let past_seq_drift = l2_block.timestamp + self.config.chain.blocktime > self.config.chain.max_seq_drift;
        let next = self.provider.get_block(l2_block_origin.number + 1).await;
        match next {
            Ok(Some(next)) => L1BlockInfo::from(next),
            _ => l2_block_origin
        }
    }
}

impl From<Block<H256>> for L1BlockInfo {
    fn from(block: Block<H256>) -> Self {
        Self {
            number: block.number.unwrap().as_u64(),
            hash: block.hash.unwrap(),
            timestamp: block.timestamp.as_u64(),
            base_fee: block.base_fee_per_gas.unwrap(),
            mix_hash: block.mix_hash.unwrap(),
        }
    }
}

#[async_trait]
impl SequencingSource for AttributesBuilder {

    async fn get_next_attributes(
        &self,
        parent_l2_block: BlockInfo,
        parent_l2_block_origin: L1BlockInfo,
        state: &State,
    ) -> Option<PayloadAttributes> {
        if !self.is_ready(parent_l2_block, state) {
            return None;
        }
        let next_origin = self.find_next_origin(parent_l2_block, parent_l2_block_origin).await;
        let timestamp = self.next_timestamp(parent_l2_block.timestamp);
        let prev_randao = self.next_randao(next_origin.clone());
        let suggested_fee_recipient = self.config.local_sequencer.suggested_fee_recipient; // expected to be SystemAccounts::default().fee_vault in optimism
        let txs = create_top_of_block_transactions(next_origin);
        let no_tx_pool = timestamp > self.config.chain.max_seq_drift;
        let gas_limit = self.config.chain.system_config.gas_limit;
        Some(
            PayloadAttributes{
            timestamp: U64([timestamp]),
            prev_randao,
            suggested_fee_recipient,
            transactions: Some(txs),
            no_tx_pool,
            gas_limit: U64([gas_limit.as_u64()]),
            epoch: None,
            l1_inclusion_block: None,
            seq_number: None,
            }
        )
    }
}


// TODO: implement. requires l1 info tx. requires signer...
// Creates the transaction(s) to include at the top of the next l2 block.
fn create_top_of_block_transactions(_origin: L1BlockInfo) -> Vec<RawTransaction> { vec![] }

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

fn unix_now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() }