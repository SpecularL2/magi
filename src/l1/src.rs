use ethers::types::{Address, Block, Transaction};
use enum_dispatch::enum_dispatch;
use serde::{Deserialize, Serialize};

use crate::l1;

#[enum_dispatch(BatcherTxDataSrc)]
pub trait BatcherTxExtractor {
    fn extract(
        &self,
        block: &Block<Transaction>,
        batch_sender: Address,
        batch_inbox: Address,
    ) -> Vec<l1::BatcherTransactionData>;
}

#[enum_dispatch]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BatcherTxDataSrc {
    EOA,
    Contract,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EOA;
impl BatcherTxExtractor for EOA {
    fn extract(
        &self,
        block: &Block<Transaction>,
        batch_sender: Address,
        batch_inbox: Address,
    ) -> Vec<l1::BatcherTransactionData> {
        l1::create_batcher_transactions(block, batch_sender, batch_inbox)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contract { pub method_id: [u8; 4], }
/// Creates a list of batcher transactions from a block, filtering by 
/// batch_inbox (assumed to be a contract addr) and method ID.
impl BatcherTxExtractor for Contract {
    fn extract(
        &self,
        block: &Block<Transaction>,
        batch_sender: Address,
        batch_inbox: Address,
    ) -> Vec<l1::BatcherTransactionData> {
        block
            .transactions
            .iter()
            .filter(|tx| tx.from == batch_sender && tx.to.map(|to| to == batch_inbox).unwrap_or(false))
            .filter(|tx| tx.input[..4] == self.method_id)
            .map(|tx| tx.input[4..].to_vec())
            .collect()
    }
}