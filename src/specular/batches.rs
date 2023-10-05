use std::collections::BTreeMap;

use std::sync::{Arc, RwLock};

use eyre::Result;

use crate::config::Config;
use crate::derive::stages::batcher_transactions::BatcherTransaction;
use crate::derive::stages::batches::Batch;
use crate::derive::state::State;
use crate::derive::PurgeableIterator;

pub struct SpecularBatches<I> {
    /// Mapping of timestamps to batches
    batches: BTreeMap<u64, Batch>,
    batcher_transaction_iter: I,
    state: Arc<RwLock<State>>,
    config: Arc<Config>,
}

impl<I> Iterator for SpecularBatches<I>
where
    I: Iterator<Item = BatcherTransaction>,
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
    I: PurgeableIterator<Item = BatcherTransaction>,
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
    I: Iterator<Item = BatcherTransaction>,
{
    fn try_next(&mut self) -> Result<Option<Batch>> {
        let _batcher_ransaction = self.batcher_transaction_iter.next();
        Ok(None)
    }
}

fn decode_batches(_batcher_ransaction: &BatcherTransaction) -> Result<Vec<Batch>> {
    let batches = Vec::new();

    Ok(batches)
}
