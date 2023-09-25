use std::sync::{Arc, RwLock};

use ethers::types::{H256, U64};

use crate::common::{Epoch, RawTransaction};
use crate::config::{Config, SystemAccounts};
use crate::derive::state::State;
use crate::derive::PurgeableIterator;
use crate::engine::PayloadAttributes;
use crate::l1::L1Info;

use crate::optimism::deposited_tx::{OptimismTransactionDeriver, TransactionDeriver};

use super::batches::Batch;

pub struct Attributes {
    batch_iter: Box<dyn PurgeableIterator<Item = Batch>>,
    state: Arc<RwLock<State>>,
    sequence_number: u64,
    epoch_hash: H256,
    config: Arc<Config>,
}

impl Iterator for Attributes {
    type Item = PayloadAttributes;

    fn next(&mut self) -> Option<Self::Item> {
        self.batch_iter
            .next()
            .map(|batch| self.derive_attributes(batch))
    }
}

impl PurgeableIterator for Attributes {
    fn purge(&mut self) {
        self.batch_iter.purge();
        self.sequence_number = 0;
        self.epoch_hash = self.state.read().unwrap().safe_epoch.hash;
    }
}

impl Attributes {
    pub fn new(
        batch_iter: Box<dyn PurgeableIterator<Item = Batch>>,
        state: Arc<RwLock<State>>,
        config: Arc<Config>,
        seq: u64,
    ) -> Self {
        let epoch_hash = state.read().unwrap().safe_epoch.hash;

        Self {
            batch_iter,
            state,
            sequence_number: seq,
            epoch_hash,
            config,
        }
    }

    fn derive_attributes(&mut self, batch: Batch) -> PayloadAttributes {
        tracing::debug!("attributes derived from block {}", batch.epoch_num);
        tracing::debug!("batch epoch hash {:?}", batch.epoch_hash);

        self.update_sequence_number(batch.epoch_hash);

        let state = self.state.read().unwrap();
        let l1_info = state.l1_info_by_hash(batch.epoch_hash).unwrap();

        let epoch = Some(Epoch {
            number: batch.epoch_num,
            hash: batch.epoch_hash,
            timestamp: l1_info.block_info.timestamp,
        });

        let timestamp = U64([batch.timestamp]);
        let l1_inclusion_block = Some(batch.l1_inclusion_block);
        let seq_number = Some(self.sequence_number);
        let prev_randao = l1_info.block_info.mix_hash;
        let transactions = Some(self.derive_transactions(batch, l1_info));
        let suggested_fee_recipient = SystemAccounts::default().fee_vault;

        PayloadAttributes {
            timestamp,
            prev_randao,
            suggested_fee_recipient,
            transactions,
            no_tx_pool: true,
            gas_limit: U64([l1_info
                .system_config
                .as_optimism()
                .unwrap()
                .gas_limit
                .as_u64()]),
            epoch,
            l1_inclusion_block,
            seq_number,
        }
    }

    fn derive_transactions(&self, batch: Batch, l1_info: &L1Info) -> Vec<RawTransaction> {
        // TODO: inject from config/factory
        OptimismTransactionDeriver::default().derive_transactions(
            &self.config,
            &self.state,
            self.sequence_number,
            self.epoch_hash,
            batch,
            l1_info,
        )
    }

    fn update_sequence_number(&mut self, batch_epoch_hash: H256) {
        if self.epoch_hash != batch_epoch_hash {
            self.sequence_number = 0;
        } else {
            self.sequence_number += 1;
        }

        self.epoch_hash = batch_epoch_hash;
    }
}
