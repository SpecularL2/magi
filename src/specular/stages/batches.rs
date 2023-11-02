use std::collections::BTreeMap;

use core::fmt::Debug;
use std::cmp::Ordering;
use std::sync::{Arc, RwLock};

use ethers::types::H256;
use eyre::Result;

use crate::common::RawTransaction;
use crate::config::Config;
use crate::derive::stages::batches::Batch;
use crate::derive::state::State;
use crate::derive::PurgeableIterator;
use ethers::{
    types::Transaction,
    utils::rlp::{Decodable, Rlp},
};

use super::batcher_transactions::SpecularBatcherTransaction;
use crate::specular::common::{
    SetL1OracleValuesInput, SET_L1_ORACLE_VALUES_ABI, SET_L1_ORACLE_VALUES_SELECTOR,
};

/// The second stage of Specular's derive pipeline.
/// This stage consumes [SpecularBatcherTransaction]s and produces [SpecularBatchV0]s.
/// One [SpecularBatcherTransaction] may produce multiple [SpecularBatchV0]s.
/// [SpecularBatchV0]s are returned in order of their timestamps.
pub struct SpecularBatches<I> {
    /// Mapping of timestamps to batches
    batches: BTreeMap<u64, SpecularBatchV0>,
    batcher_transaction_iter: I,
    state: Arc<RwLock<State>>,
    config: Arc<Config>,
}

impl<I> Iterator for SpecularBatches<I>
where
    I: Iterator<Item = SpecularBatcherTransaction>,
{
    type Item = Batch;

    fn next(&mut self) -> Option<Self::Item> {
        self.try_next().unwrap_or_else(|_| {
            tracing::debug!("Failed to decode batch");
            None
        })
    }
}

impl<I> PurgeableIterator for SpecularBatches<I>
where
    I: PurgeableIterator<Item = SpecularBatcherTransaction>,
{
    fn purge(&mut self) {
        self.batcher_transaction_iter.purge();
        self.batches.clear();
    }
}

impl<I> SpecularBatches<I> {
    pub fn new(
        batcher_transaction_iter: I,
        state: Arc<RwLock<State>>,
        config: Arc<Config>,
    ) -> Self {
        Self {
            batches: BTreeMap::new(),
            batcher_transaction_iter,
            state,
            config,
        }
    }
}

impl<I> SpecularBatches<I>
where
    I: Iterator<Item = SpecularBatcherTransaction>,
{
    /// This function tries to decode batches from the next [SpecularBatcherTransaction] and
    /// returns the first valid batch if possible.
    fn try_next(&mut self) -> Result<Option<Batch>> {
        let batcher_transaction = self.batcher_transaction_iter.next();
        if let Some(batcher_transaction) = batcher_transaction {
            let batches = decode_batches(
                &batcher_transaction,
                &self.state,
                self.config.chain.blocktime,
            )?;
            batches.into_iter().for_each(|batch| {
                tracing::debug!(
                    "saw batch: t={}, bn={:?}, e={}",
                    batch.timestamp,
                    batch.l2_block_number,
                    batch.l1_inclusion_block,
                );
                self.batches.insert(batch.timestamp, batch);
            });
        }

        let derived_batch = loop {
            if let Some((_, batch)) = self.batches.first_key_value() {
                match self.batch_status(batch) {
                    BatchStatus::Accept => {
                        let batch = batch.clone();
                        self.batches.remove(&batch.timestamp);
                        break Some(batch);
                    }
                    BatchStatus::Drop => {
                        tracing::warn!("dropping invalid batch");
                        let timestamp = batch.timestamp;
                        self.batches.remove(&timestamp);
                    }
                }
            } else {
                break None;
            }
        };

        let batch = if derived_batch.is_none() {
            let state = self.state.read().unwrap();

            let current_l1_block = state.current_epoch_num;
            let safe_head = state.safe_head;
            let epoch = state.safe_epoch;
            let next_epoch = state.epoch_by_number(epoch.number + 1);
            let seq_window_size = self.config.chain.seq_window_size;

            if let Some(next_epoch) = next_epoch {
                if current_l1_block > epoch.number + seq_window_size {
                    let next_timestamp = safe_head.timestamp + self.config.chain.blocktime;
                    let epoch = if next_timestamp < next_epoch.timestamp {
                        epoch
                    } else {
                        next_epoch
                    };
                    tracing::trace!(
                        "inserting empty batch | ts={} epoch_num={}",
                        epoch.number,
                        next_timestamp
                    );
                    Some(Batch {
                        epoch_num: epoch.number,
                        epoch_hash: epoch.hash,
                        parent_hash: Default::default(), // We don't care about parent_hash
                        timestamp: next_timestamp,
                        transactions: Vec::new(),
                        l1_inclusion_block: current_l1_block,
                    })
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            derived_batch.map(|batch| batch.into())
        };

        Ok(batch)
    }

    /// Determine whether a batch is valid.
    fn batch_status(&self, batch: &SpecularBatchV0) -> BatchStatus {
        let state = self.state.read().unwrap();
        let head = state.safe_head;
        let next_timestamp = head.timestamp + self.config.chain.blocktime;

        // check timestamp range
        match batch.timestamp.cmp(&next_timestamp) {
            Ordering::Greater | Ordering::Less => return BatchStatus::Drop,
            Ordering::Equal => (),
        }

        // check that block builds on existing chain
        if batch.l2_block_number != head.number + 1 {
            tracing::warn!("invalid block number");
            return BatchStatus::Drop;
        }

        // check the inclusion delay
        if batch.epoch_num + self.config.chain.seq_window_size < batch.l1_inclusion_block {
            tracing::warn!("inclusion window elapsed");
            return BatchStatus::Drop;
        }

        // TODO[zhe]: check origin epoch and sequencer drift

        // check L1 oracle update transaction
        if batch.is_epoch_update {
            if let Err(err) = check_epoch_update_batch(batch, &self.config, &state) {
                tracing::warn!("invalid epoch update batch, err={:?}", err);
                return BatchStatus::Drop;
            }
        }

        if batch.has_invalid_transactions() {
            tracing::warn!("invalid transaction");
            return BatchStatus::Drop;
        }

        BatchStatus::Accept
    }
}

/// Decode Specular batches from a [SpecularBatcherTransaction] based on its version.
/// Currently only version 0 is supported.
// TODO: consider returning a generic/trait-type to support multiple versions.
fn decode_batches(
    batcher_tx: &SpecularBatcherTransaction,
    state: &RwLock<State>,
    blocktime: u64,
) -> Result<Vec<SpecularBatchV0>> {
    if batcher_tx.version != 0 {
        eyre::bail!("unsupported batcher transaction version");
    }
    decode_batches_v0(batcher_tx, state, blocktime)
}

/// Decodes [SpecularBatchV0]s from a [SpecularBatcherTransaction].
/// [SpecularBatcherTransaction] contains multiple lists of [SpecularBatchV0]s.
/// For each batch list in [SpecularBatcherTransaction], if the first byte of the list is 0,
/// the first [SpecularBatchV0] in the list is an epoch update; otherwise, it extends the current epoch.
fn decode_batches_v0(
    batcher_tx: &SpecularBatcherTransaction,
    state: &RwLock<State>,
    blocktime: u64,
) -> Result<Vec<SpecularBatchV0>> {
    let mut batches = Vec::new();
    let batch_lists = Rlp::new(&batcher_tx.tx_batch);
    for batch_list in batch_lists.iter() {
        // Decode the epoch-update indicator.
        let is_epoch_update = batch_list.val_at::<u8>(0)? == 0;
        // Get l2 safe head info.
        let state = state.read().unwrap();
        let safe_l2_num = state.safe_head.number;
        let safe_l2_ts = state.safe_head.timestamp;
        // Decode the first l2 block number.
        let first_l2_block_num: u64 = batch_list.val_at(1)?;
        let first_l2_block_timestamp = (first_l2_block_num - safe_l2_num) * blocktime + safe_l2_ts;
        // Decode the epoch number and hash (or extend the current epoch).
        let (epoch_num, epoch_hash) = if is_epoch_update {
            let epoch_num: u64 = batch_list.val_at(2)?;
            let epoch_hash: H256 = batch_list.val_at(3)?;
            (epoch_num, epoch_hash)
        } else {
            (state.safe_epoch.number, state.safe_epoch.hash)
        };
        // Decode the transaction batches.
        let batches_offset = if is_epoch_update { 4 } else { 2 };
        for (batch, idx) in batch_list.at(batches_offset)?.iter().zip(0u64..) {
            let batch = SpecularBatchV0 {
                epoch_num,
                epoch_hash,
                timestamp: first_l2_block_timestamp + idx * blocktime,
                transactions: batch.as_list()?,
                l2_block_number: first_l2_block_num + idx,
                l1_inclusion_block: batcher_tx.l1_inclusion_block,
                is_epoch_update: idx == 0 && is_epoch_update, // true only if first batch
            };
            batches.push(batch);
        }
    }

    Ok(batches)
}

#[derive(Debug, Clone, PartialEq)]
enum BatchStatus {
    Drop,
    Accept,
}

/// A batch of transactions, along with payload attributes.
#[derive(Debug, Clone)]
pub struct SpecularBatchV0 {
    pub epoch_num: u64,
    pub epoch_hash: H256,
    pub timestamp: u64,
    pub l2_block_number: u64,
    pub transactions: Vec<RawTransaction>,
    pub l1_inclusion_block: u64,
    pub is_epoch_update: bool,
}

impl SpecularBatchV0 {
    fn has_invalid_transactions(&self) -> bool {
        self.transactions.iter().any(|tx| tx.0.is_empty())
    }
}

impl From<SpecularBatchV0> for Batch {
    fn from(val: SpecularBatchV0) -> Self {
        Batch {
            epoch_num: val.epoch_num,
            epoch_hash: val.epoch_hash,
            parent_hash: Default::default(), // not used
            timestamp: val.timestamp,
            transactions: val.transactions,
            l1_inclusion_block: val.l1_inclusion_block,
        }
    }
}

fn check_epoch_update_batch(batch: &SpecularBatchV0, config: &Config, state: &State) -> Result<()> {
    if batch.transactions.is_empty() {
        eyre::bail!("no setL1OracleValues call");
    }

    let tx = Transaction::decode(&Rlp::new(&batch.transactions[0].0))?;
    match tx.to.map(|to| to == config.chain.meta.l1_oracle) {
        Some(true) => (),
        _ => eyre::bail!("setL1OracleValues call to wrong address"),
    }

    if tx.to.is_none() || tx.to.unwrap() != config.chain.meta.l1_oracle {
        eyre::bail!("setL1OracleValues call to wrong address");
    }
    let (epoch_num, timestamp, base_fee, epoch_hash, state_root): SetL1OracleValuesInput =
        SET_L1_ORACLE_VALUES_ABI
            .decode_with_selector(*SET_L1_ORACLE_VALUES_SELECTOR, &tx.input.0)?;
    if epoch_num.as_u64() != batch.epoch_num {
        eyre::bail!("epoch number mismatch with batcher transaction");
    }
    if epoch_hash != batch.epoch_hash {
        eyre::bail!("epoch hash mismatch with batcher transaction");
    }
    let target_epoch = state
        .l1_info_by_number(epoch_num.as_u64())
        .ok_or(eyre::eyre!("epoch {} does not exist", epoch_num.as_u64()))?;
    if epoch_hash != target_epoch.block_info.hash {
        eyre::bail!("epoch hash mismatch with L1");
    }
    if timestamp.as_u64() != target_epoch.block_info.timestamp {
        eyre::bail!("epoch timestamp mismatch with L1");
    }
    if base_fee != target_epoch.block_info.base_fee {
        eyre::bail!("epoch base fee mismatch with L1");
    }
    if state_root != target_epoch.block_info.state_root {
        eyre::bail!("epoch state root mismatch with L1");
    }

    Ok(())
}
