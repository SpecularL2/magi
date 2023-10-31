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
use ethers::utils::rlp::{DecoderError, Rlp};

use super::batcher_transactions::SpecularBatcherTransaction;

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
            let batches = decode_batches(&batcher_transaction, &self.state)?;
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

        // TODO[zhe]: fix correct epoch fetching
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
                        // TODO[zhe]: we might have to be stuck in the same epoch forever, so this is incorrect
                        next_epoch
                    };

                    Some(Batch {
                        epoch_num: epoch.number,
                        epoch_hash: epoch.hash,
                        parent_hash: Default::default(),
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
        // TODO[zhe]: do we need this?
        match batch.timestamp.cmp(&next_timestamp) {
            Ordering::Greater | Ordering::Less => return BatchStatus::Drop,
            Ordering::Equal => (),
        }

        // check that block builds on existing chain
        if batch.l2_block_number != head.number + 1 {
            tracing::warn!("invalid block number");
            return BatchStatus::Drop;
        }

        // TODO[zhe]: check inclusion delay, batch origin epoch, and sequencer drift

        if batch.has_invalid_transactions() {
            tracing::warn!("invalid transaction");
            return BatchStatus::Drop;
        }

        BatchStatus::Accept
    }
}

/// Decode [SpecularBatchV0] from a [SpecularBatcherTransaction].
fn decode_batches(
    batcher_ransaction: &SpecularBatcherTransaction,
    state: &RwLock<State>,
) -> Result<Vec<SpecularBatchV0>> {
    if batcher_ransaction.version != 0 {
        eyre::bail!("unsupported batcher transaction version");
    }

    let mut batches = Vec::new();

    let is_epoch_update = batcher_ransaction.tx_batch[0] == 0;
    let rlp = Rlp::new(&batcher_ransaction.tx_batch[1..]);

    let (rlp_offset, epoch_num, epoch_hash) = if is_epoch_update {
        let epoch_num: u64 = rlp.val_at(0)?;
        let epoch_hash: H256 = rlp.val_at(1)?;
        (2, epoch_num, epoch_hash)
    } else {
        // If not an epoch update, this batcher transaction extends the current epoch
        let state = state.read().unwrap();
        (0, state.safe_epoch.number, state.safe_epoch.hash)
    };

    let first_l2_block_number: u64 = rlp.val_at(rlp_offset)?;
    for (batch, idx) in rlp.at(rlp_offset + 1)?.iter().zip(0u64..) {
        let batch = SpecularBatchV0::decode(
            &batch,
            epoch_num,
            epoch_hash,
            first_l2_block_number + idx,
            batcher_ransaction.l1_inclusion_block,
        )?;
        batches.push(batch);
    }

    // Only the first batch in a batcher transaction will be marked as an epoch update
    if is_epoch_update {
        if let Some(batch) = batches.first_mut() {
            batch.is_epoch_update = true;
        }
    }

    Ok(batches)
}

#[derive(Debug, Clone, PartialEq)]
enum BatchStatus {
    Drop,
    Accept,
}

/// A batch of transactions with block contexts, which is essentially an L2 block.
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
    fn decode(
        rlp: &Rlp,
        epoch_num: u64,
        epoch_hash: H256,
        l2_block_number: u64,
        l1_inclusion_block: u64,
    ) -> Result<Self, DecoderError> {
        let timestamp = rlp.val_at(0)?;
        let transactions = rlp.list_at(1)?;

        Ok(Self {
            epoch_num,
            epoch_hash,
            timestamp,
            l2_block_number,
            transactions,
            l1_inclusion_block,
            is_epoch_update: false, // Will be set by the `decode_batches` function
        })
    }

    fn has_invalid_transactions(&self) -> bool {
        self.transactions.iter().any(|tx| tx.0.is_empty())
    }
}

impl From<SpecularBatchV0> for Batch {
    fn from(val: SpecularBatchV0) -> Self {
        // TODO[zhe]: this is incorrect, use the correct epoch when derivation pipeline is fixed
        Batch {
            epoch_num: val.epoch_num,
            epoch_hash: val.epoch_hash,
            parent_hash: Default::default(), // we don't care about parent hash
            timestamp: val.timestamp,
            transactions: val.transactions,
            l1_inclusion_block: val.l1_inclusion_block,
        }
    }
}
