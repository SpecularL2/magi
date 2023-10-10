use std::{time::{SystemTime, UNIX_EPOCH}, sync::{RwLock, Arc}};

use ethers::types::{U64, H256};

use crate::{config::Config, engine::PayloadAttributes, common::{BlockInfo, RawTransaction, Epoch}, derive::state::State, driver::sequencing::SequencingSource};

pub struct AttributesBuilder {
    config: Config,
}

impl AttributesBuilder {
    pub fn new(config: Config) -> Self { Self { config } }

    fn is_ready(&self, parent_l2_block: BlockInfo, state: &State) -> bool { 
        if state.safe_head.number + self.config.local_sequencer.max_safe_lag <= parent_l2_block.number {
            return false;
        }
        // Don't create the attributes if the timestamp is in the future.
        if self.next_timestamp(parent_l2_block.timestamp) > unix_now() {
            return false;
        }
        true
    }

    fn next_timestamp(&self, parent_block_timestamp: u64) -> u64 { parent_block_timestamp + self.config.chain.blocktime }

    // TODO: implement.
    fn next_randao(&self, _parent_l2_block: BlockInfo) -> H256 {
        H256::random() 
    }
}

impl SequencingSource for AttributesBuilder {

    fn get_next_attributes(&self, parent_l2_block: BlockInfo, state: &Arc<RwLock<State>>) -> Option<PayloadAttributes> {
        let state = state.read().unwrap();
        if !self.is_ready(parent_l2_block, &state) {
            return None;
        }
        let timestamp = self.next_timestamp(parent_l2_block.timestamp);
        let prev_randao = self.next_randao(parent_l2_block);
        let suggested_fee_recipient = self.config.local_sequencer.suggested_fee_recipient; // expected to be SystemAccounts::default().fee_vault in optimism
        let txs = {
            let parent_l2_block_origin = state.epoch_by_hash(parent_l2_block.hash).unwrap();
            create_top_of_block_transactions(parent_l2_block, parent_l2_block_origin)
        };
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

// TODO: implement.
// Creates the transactions to include at the top of the next l2 block (following `parent_l2_block`).
fn create_top_of_block_transactions(_parent_l2_block: BlockInfo, _parent_l2_block_origin: Epoch) -> Vec<RawTransaction> { vec![] }

fn unix_now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() }