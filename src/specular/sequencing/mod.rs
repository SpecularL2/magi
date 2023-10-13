use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ethers::types::{H256, U64};
use eyre::Result;

use crate::{
    common::{BlockInfo, Epoch, RawTransaction},
    driver::sequencing::SequencingSource,
    engine::PayloadAttributes,
    l1::L1BlockInfo,
};

pub mod config;
use config::Config;

pub struct AttributesBuilder {
    config: Config,
}

impl AttributesBuilder {
    pub fn new(config: Config) -> Self {
        Self { config }
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
        curr_l1_epoch: &L1BlockInfo,
        next_l1_epoch: Option<&L1BlockInfo>,
    ) -> Result<L1BlockInfo> {
        let next_l2_ts = self.next_timestamp(curr_l2_block.timestamp);
        let next_drift_bound = self.next_drift_bound(curr_l1_epoch);
        let is_drift_bound_exceeded = next_l2_ts > next_drift_bound;
        if is_drift_bound_exceeded {
            tracing::info!("Next l2 ts exceeds the drift bound {}", next_drift_bound);
        }
        match (next_l1_epoch, is_drift_bound_exceeded) {
            // We found the next l1 block.
            (Some(next_l1_block), _) => {
                if next_l2_ts >= next_l1_block.timestamp {
                    Ok(next_l1_block.clone())
                } else {
                    Ok(curr_l1_epoch.clone())
                }
            }
            // We're not exceeding the drift bound, so we can just use the current origin.
            (_, false) => {
                tracing::info!("Falling back to current origin (couldn't find next).");
                Ok(curr_l1_epoch.clone())
            }
            // We exceeded the drift bound, so we can't use the current origin.
            // But we also can't use the next l1 block since we didn't find it.
            (_, _) => Err(eyre::eyre!("current origin drift bound exceeded.")),
        }
    }
}

#[async_trait]
impl SequencingSource for AttributesBuilder {
    fn is_ready(&self, safe_l2_head: &BlockInfo, parent_l2_block: &BlockInfo) -> bool {
        safe_l2_head.number + self.config.max_safe_lag > parent_l2_block.number
            && self.next_timestamp(parent_l2_block.timestamp) <= unix_now()
    }

    async fn get_next_attributes(
        &self,
        parent_l2_block: &BlockInfo,
        parent_l1_epoch: &L1BlockInfo,
        next_l1_epoch: Option<&L1BlockInfo>,
    ) -> Result<PayloadAttributes> {
        let next_origin = self
            .find_next_origin(parent_l2_block, parent_l1_epoch, next_l1_epoch)
            .await?;
        let timestamp = self.next_timestamp(parent_l2_block.timestamp);
        let prev_randao = next_randao(&next_origin);
        let suggested_fee_recipient = self.config.suggested_fee_recipient;
        let txs = create_top_of_block_transactions(&next_origin);
        let no_tx_pool = timestamp > self.config.max_seq_drift;
        let gas_limit = self.config.system_config.gas_limit;
        Ok(PayloadAttributes {
            timestamp: U64([timestamp]),
            prev_randao,
            suggested_fee_recipient,
            transactions: Some(txs),
            no_tx_pool,
            gas_limit: U64([gas_limit]),
            epoch: Some(create_epoch(next_origin)),
            l1_inclusion_block: None,
            seq_number: None,
        })
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

/// Extracts the epoch information from `info`.
fn create_epoch(info: L1BlockInfo) -> Epoch {
    Epoch {
        number: info.number,
        hash: info.hash,
        timestamp: info.timestamp,
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
