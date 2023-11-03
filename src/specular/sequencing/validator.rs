use async_trait::async_trait;
use ethers::{
    providers::{JsonRpcClient, Provider, ProviderError},
    types::{
        transaction::eip2718::TypedTransaction, BlockNumber, Bytes, Transaction, TransactionRequest,
    },
    utils::{
        rlp::{Decodable, Rlp},
        serialize,
    },
};
use eyre::Result;

use crate::{
    common::BlockInfo, driver::sequencing::SequencingPolicy, engine::PayloadAttributes,
    l1::L1BlockInfo,
};

pub struct AttributesValidator<T> {
    provider: Provider<T>,
    should_skip: bool,
}

impl<T: JsonRpcClient> AttributesValidator<T> {
    pub fn new(provider: Provider<T>) -> Self {
        Self {
            provider,
            should_skip: false,
        }
    }

    /// Returns true if the attributes starts a new epoch.
    /// [Attributes](crate::derive::stages::attributes::Attributes) stage will correctly set
    /// the sequence number according to epoch changes while taking care of reorgs.
    fn update_epoch(&mut self, attributes: &PayloadAttributes) -> Result<bool> {
        let epoch_changed = attributes
            .seq_number
            .ok_or(eyre::eyre!("attributes without seq number"))?
            == 0;
        if epoch_changed {
            // If the epoch changed, we should execute the l1 oracle update transaction.
            self.should_skip = false;
        }
        Ok(epoch_changed)
    }
}

#[async_trait]
impl<T: JsonRpcClient> SequencingPolicy for AttributesValidator<T> {
    fn is_ready(&self, _: &BlockInfo, _: &BlockInfo) -> bool {
        false
    }

    async fn get_attributes(
        &self,
        _: &BlockInfo,
        _: &L1BlockInfo,
        _: Option<&L1BlockInfo>,
    ) -> Result<PayloadAttributes> {
        Ok(Default::default())
    }

    async fn should_skip_attributes(&mut self, attributes: &PayloadAttributes) -> Result<bool> {
        let epoch_changed = self.update_epoch(attributes)?;
        if epoch_changed {
            // If the epoch changed, we need to check if the l1 oracle update transaction executes successfully.
            // If empty block, we can skip the cehck.
            if let Some(Some(raw_tx)) = attributes.transactions.as_ref().map(|txs| txs.first()) {
                // Construct l1 oracle update transaction call object from the raw transaction.
                let tx = Transaction::decode(&Rlp::new(&raw_tx.0))?;
                let tx: TypedTransaction = TransactionRequest::new()
                    .from(tx.from)
                    .to(tx.to.expect("to should be set"))
                    .gas(tx.gas)
                    .gas_price(tx.gas_price.expect("gas price should be set"))
                    .data(tx.input)
                    .into();
                // Use `eth_call` to check if the transaction executes successfully.
                // We use `BlockNumber::Pending` to make sure the transaction is executed in the pending block.
                // TODO: it is better to use `block override set` to better simulate the pending block once it is supported by ethers-rs.
                let tx = serialize(&tx);
                let block = serialize(&BlockNumber::Pending);
                let res: Result<Bytes, ProviderError> =
                    self.provider.request("eth_call", [tx, block]).await;
                // If the transaction fails, we should skip all batches in the same epoch.
                self.should_skip = res.is_err();
            }
        }
        Ok(self.should_skip)
    }
}
