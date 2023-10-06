use std::sync::mpsc;

use ethers::{
    abi::parse_abi_str,
    contract::Lazy,
    prelude::BaseContract,
    types::{Bytes, Selector, U256},
};
use eyre::Result;
use std::collections::VecDeque;

use crate::derive::stages::batcher_transactions::BatcherTransactionMessage;
use crate::derive::PurgeableIterator;

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
        while let Ok(BatcherTransactionMessage { txs, l1_origin: _ }) = self.transaction_rx.try_recv()
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
    pub version: u64,
    pub tx_batch: Bytes,
}

impl SpecularBatcherTransaction {
    pub fn new(data: &[u8]) -> Result<Self> {
        if data.len() < 4 {
            eyre::bail!("invalid transaction data");
        }
        if data[..4] != *APPEND_TX_BATCH_SELECTOR {
            eyre::bail!("not appendTxBatch call");
        }

        Self::try_from(data)
    }
}

type AppendTxBatchInput = (U256, Bytes);
const APPEND_TX_BATCH_ABI_STR: &str = r#"[
    function appendTxBatch(uint256 txBatchVersion,bytes calldata txBatch) external
]"#;
static APPEND_TX_BATCH_ABI: Lazy<BaseContract> = Lazy::new(|| {
    BaseContract::from(parse_abi_str(APPEND_TX_BATCH_ABI_STR).expect("abi must be valid"))
});
static APPEND_TX_BATCH_SELECTOR: Lazy<Selector> = Lazy::new(|| {
    APPEND_TX_BATCH_ABI
        .abi()
        .function("appendTxBatch")
        .expect("function must be present")
        .short_signature()
});

impl TryFrom<&[u8]> for SpecularBatcherTransaction {
    type Error = eyre::Report;

    fn try_from(value: &[u8]) -> Result<Self> {
        let (tx_batch_version, tx_batch): AppendTxBatchInput =
            APPEND_TX_BATCH_ABI.decode("appendTxBatch", value)?;

        Ok(Self {
            version: tx_batch_version.as_u64(),
            tx_batch,
        })
    }
}
