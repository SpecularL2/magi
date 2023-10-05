use std::sync::mpsc;

use eyre::Result;
use std::collections::VecDeque;

use crate::derive::PurgeableIterator;
use crate::derive::stages::batcher_transactions::BatcherTransactionMessage;

pub struct SpecularBatcherTransactions {
    txs: VecDeque<SpecularBatcherTransaction>,
    transaction_rx: mpsc::Receiver<BatcherTransactionMessage>,
}

impl Iterator for SpecularBatcherTransactions {
    type Item = SpecularBatcherTransaction;

    fn next(&mut self) -> Option<Self::Item> {
        self.process_incoming();
        self.txs.pop_front()
    }
}

impl PurgeableIterator for SpecularBatcherTransactions {
    fn purge(&mut self) {
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
                let res = SpecularBatcherTransaction::new(&data).map(|tx| {
                    self.txs.push_back(tx);
                });

                if res.is_err() {
                    tracing::warn!("dropping invalid batcher transaction");
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpecularBatcherTransaction {
    pub data: Vec<u8>,
}

impl SpecularBatcherTransaction {
    pub fn new(data: &[u8]) -> Result<Self> {

        Ok(Self { data: Vec::from(data) })
    }
}
