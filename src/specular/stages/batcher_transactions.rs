use std::sync::mpsc;

use async_trait::async_trait;
use ethers::types::Bytes;
use eyre::Result;
use std::collections::VecDeque;

use crate::derive::async_iterator::AsyncIterator;
use crate::derive::stages::batcher_transactions::BatcherTransactionMessage;
use crate::derive::PurgeableAsyncIterator;
use crate::specular::common::{AppendTxBatchInput, APPEND_TX_BATCH_ABI, APPEND_TX_BATCH_SELECTOR};

/// The first stage in Specular's derivation pipeline.
/// This stage consumes [BatcherTransactionMessage]s and produces [SpecularBatcherTransaction]s.
pub struct SpecularBatcherTransactions {
    txs: VecDeque<SpecularBatcherTransaction>,
    transaction_rx: mpsc::Receiver<BatcherTransactionMessage>,
}

#[async_trait]
impl AsyncIterator for SpecularBatcherTransactions {
    type Item = SpecularBatcherTransaction;

    async fn next(&mut self) -> Option<Self::Item> {
        self.process_incoming();
        self.txs.pop_front()
    }
}

#[async_trait]
impl PurgeableAsyncIterator for SpecularBatcherTransactions {
    async fn purge(&mut self) {
        // drain the channel first
        while self.transaction_rx.try_recv().is_ok() {}
        self.txs.clear();
    }
}

impl SpecularBatcherTransactions {
    pub fn new(transaction_rx: mpsc::Receiver<BatcherTransactionMessage>) -> Self {
        Self {
            transaction_rx,
            txs: VecDeque::new(),
        }
    }

    pub fn process_incoming(&mut self) {
        while let Ok(BatcherTransactionMessage { txs, l1_origin }) = self.transaction_rx.try_recv()
        {
            for data in txs {
                let res = SpecularBatcherTransaction::new(l1_origin, &data).map(|tx| {
                    self.txs.push_back(tx);
                });

                if res.is_err() {
                    tracing::warn!("dropping invalid batcher transaction");
                }
            }
        }
    }
}

/// Specular batcher transaction representing a call to `appendTxBatch` on the `SequencerInbox` contract.
#[derive(Debug, Clone)]
pub struct SpecularBatcherTransaction {
    /// The block number of the L1 block that included this transaction.
    pub l1_inclusion_block: u64,
    pub version: u8,
    pub tx_batch: Bytes,
}

impl SpecularBatcherTransaction {
    /// Create a new batcher transaction from raw transaction data.
    /// Only `appendTxBatch` calls are considered valid.
    pub fn new(l1_inclusion_block: u64, data: &[u8]) -> Result<Self> {
        let tx_batch: AppendTxBatchInput =
            APPEND_TX_BATCH_ABI.decode_with_selector(*APPEND_TX_BATCH_SELECTOR, data)?;

        let version = tx_batch.0[0];
        let tx_batch = tx_batch.0.slice(1..).into();

        Ok(Self {
            l1_inclusion_block,
            version,
            tx_batch,
        })
    }
}
