use std::sync::Arc;

use ethers::providers::{Http, Middleware, Provider};
use ethers::types::Transaction;
use ethers::{
    types::{Block, H256},
    utils::keccak256,
};
use eyre::Result;
use tokio::sync::RwLock;
use tokio::time::{sleep, Duration};

use crate::{
    common::{BlockInfo, Epoch},
    config::Config,
    engine::{Engine, EngineApi, ExecutionPayload, ForkchoiceState, PayloadAttributes, Status},
};

pub struct EngineDriver<E: Engine> {
    /// The L2 execution engine
    engine: Arc<E>,
    /// Provider for the local L2 execution RPC
    provider: Provider<Http>,
    /// Blocktime of the L2 chain
    blocktime: u64,
    /// Most recent block found on the p2p network
    pub unsafe_head: BlockInfo,
    /// Batch epoch of the unsafe head (expected)
    pub unsafe_epoch: Epoch,
    /// Most recent block that can be derived from L1 data
    pub safe_head: BlockInfo,
    /// Batch epoch of the safe head
    pub safe_epoch: Epoch,
    /// Most recent block that can be derived from finalized L1 data
    pub finalized_head: BlockInfo,
    /// Batch epoch of the finalized head
    pub finalized_epoch: Epoch,
}

pub enum Action {
    /// Indicates that the attributes should be skipped.
    Skip(BlockInfo),
    /// Indicates that the attributes should be processed.
    /// If `bool` is true, reset unsafe head prior to processing.
    Process(bool),
}

pub enum ChainHead {
    Safe,
    Unsafe,
}

pub async fn handle_attributes<E: Engine>(
    attrs: &PayloadAttributes,
    target: ChainHead,
    engine_driver: Arc<RwLock<EngineDriver<E>>>,
) -> Result<()> {
    let action = {
        let engine_driver = engine_driver.read().await;
        engine_driver.determine_action(attrs.clone()).await?
    };
    execute_action(attrs, action, target, engine_driver).await
}

pub async fn execute_action<E: Engine>(
    attrs: &PayloadAttributes,
    action: Action,
    target: ChainHead,
    engine_driver: Arc<RwLock<EngineDriver<E>>>,
) -> Result<()> {
    match action {
        // Skip processing the attributes (fork-choice update-only).
        Action::Skip(info) => {
            let mut engine_driver = engine_driver.write().await;
            let epoch = *attrs.epoch.as_ref().unwrap();
            match target {
                ChainHead::Safe => engine_driver.update_safe_head(info, epoch, false),
                ChainHead::Unsafe => engine_driver.update_unsafe_head(info, epoch),
            }
        }
        // Process the attributes (build payload + fork-choice update).
        Action::Process(reorg) => {
            if reorg {
                let mut engine_driver = engine_driver.write().await;
                let safe_head = engine_driver.safe_head;
                let safe_epoch = engine_driver.safe_epoch;
                engine_driver.update_unsafe_head(safe_head, safe_epoch);
            }
            // Build new payload.
            let (new_head, new_epoch) = {
                engine_driver
                    .read()
                    .await
                    .build_new_payload(attrs.clone())
                    .await?
            };
            // Book-keeping: Update head.
            let mut engine_driver = engine_driver.write().await;
            match target {
                ChainHead::Safe => engine_driver.update_safe_head(new_head, new_epoch, true),
                ChainHead::Unsafe => engine_driver.update_unsafe_head(new_head, new_epoch),
            }
            // Final fork-choice update. TODO: downgrade lock to read.
            engine_driver.update_forkchoice().await?;
        }
    }
    Ok(())
}

impl<E: Engine> EngineDriver<E> {
    pub async fn determine_action(&self, attributes: PayloadAttributes) -> Result<Action> {
        match self.block_at(attributes.timestamp.as_u64()).await {
            Some(block) if should_skip(&block, &attributes)? => {
                Ok(Action::Skip(BlockInfo::try_from(block)?))
            }
            Some(_) => Ok(Action::Process(true)),
            _ => Ok(Action::Process(false)),
        }
    }

    /// TODO: No longer used.
    pub async fn handle_attributes(
        &mut self,
        attributes: PayloadAttributes,
        update_safe: bool,
    ) -> Result<()> {
        let block: Option<Block<Transaction>> = self.block_at(attributes.timestamp.as_u64()).await;

        if let Some(block) = block {
            tracing::info!("A local L2 block was found for the attrs timestamp");
            if should_skip(&block, &attributes)? {
                self.skip_attributes(attributes, BlockInfo::try_from(block)?)
                    .await
            } else {
                self.update_unsafe_head(self.safe_head, self.safe_epoch);
                self.process_attributes(attributes, update_safe).await
            }
        } else {
            tracing::info!("No local L2 block found for the attrs timestamp");
            self.process_attributes(attributes, update_safe).await
        }
    }

    pub async fn handle_unsafe_payload(&mut self, payload: &ExecutionPayload) -> Result<()> {
        self.push_payload(payload.clone()).await?;
        self.unsafe_head = payload.into();
        // TODO: inspect payload so we can set unsafe_epoch.
        self.update_forkchoice().await?;

        tracing::info!(
            "head updated: {} {:?}",
            self.unsafe_head.number,
            self.unsafe_head.hash,
        );

        Ok(())
    }

    pub async fn build_new_payload(
        &self,
        attributes: PayloadAttributes,
    ) -> Result<(BlockInfo, Epoch)> {
        let new_epoch = *attributes.epoch.as_ref().unwrap();
        let payload = self.build_payload(attributes).await?;
        tracing::info!(
            "built payload: ts={} block#={} hash={}",
            payload.timestamp,
            payload.block_number,
            payload.block_hash
        );
        let new_head = BlockInfo {
            number: payload.block_number.as_u64(),
            hash: payload.block_hash,
            parent_hash: payload.parent_hash,
            timestamp: payload.timestamp.as_u64(),
        };
        self.push_payload(payload).await?;
        Ok((new_head, new_epoch))
    }

    pub fn update_unsafe_head(&mut self, head: BlockInfo, epoch: Epoch) {
        self.unsafe_head = head;
        self.unsafe_epoch = epoch;
    }

    pub fn update_safe_head(&mut self, head: BlockInfo, epoch: Epoch, reorg_unsafe: bool) {
        if self.safe_head != head {
            self.safe_head = head;
            self.safe_epoch = epoch;
        }
        if reorg_unsafe || self.safe_head.number > self.unsafe_head.number {
            tracing::info!(
                "updating unsafe {} to safe {}",
                self.unsafe_head.number,
                self.safe_head.number
            );
            self.update_unsafe_head(self.safe_head, self.safe_epoch);
        }
    }

    pub fn update_finalized(&mut self, head: BlockInfo, epoch: Epoch) {
        self.finalized_head = head;
        self.finalized_epoch = epoch;
    }

    pub fn reorg(&mut self) {
        self.unsafe_head = self.finalized_head;
        self.unsafe_epoch = self.finalized_epoch;
        self.safe_head = self.finalized_head;
        self.safe_epoch = self.finalized_epoch;
    }

    pub async fn engine_ready(&self) -> bool {
        let forkchoice = self.create_forkchoice_state();
        self.engine
            .forkchoice_updated(forkchoice, None)
            .await
            .map_err(|e| tracing::error!("engine not ready yet: {:?}", e))
            .is_ok()
    }

    async fn process_attributes(
        &mut self,
        attributes: PayloadAttributes,
        update_safe: bool,
    ) -> Result<()> {
        let new_epoch = *attributes.epoch.as_ref().unwrap();

        let payload = self.build_payload(attributes).await?;
        tracing::info!(
            "built payload: ts={} block#={} hash={}",
            payload.timestamp,
            payload.block_number,
            payload.block_hash
        );
        let new_head = BlockInfo {
            number: payload.block_number.as_u64(),
            hash: payload.block_hash,
            parent_hash: payload.parent_hash,
            timestamp: payload.timestamp.as_u64(),
        };

        self.push_payload(payload).await?;
        if update_safe {
            self.update_safe_head(new_head, new_epoch, true);
        } else {
            self.update_unsafe_head(new_head, new_epoch);
        }
        self.update_forkchoice().await?;

        Ok(())
    }

    async fn skip_attributes(
        &mut self,
        attributes: PayloadAttributes,
        new_head: BlockInfo,
    ) -> Result<()> {
        let new_epoch = *attributes.epoch.as_ref().unwrap();
        self.update_safe_head(new_head, new_epoch, false);
        self.update_forkchoice().await?;

        Ok(())
    }

    async fn build_payload(&self, attributes: PayloadAttributes) -> Result<ExecutionPayload> {
        let forkchoice = self.create_forkchoice_state();
        let no_tx_pool = attributes.no_tx_pool;

        let update = self
            .engine
            .forkchoice_updated(forkchoice, Some(attributes))
            .await?;

        if update.payload_status.status != Status::Valid {
            let err = update.payload_status.validation_error.unwrap_or_default();
            eyre::bail!(format!("invalid payload attributes: {}", err));
        }

        let id = update
            .payload_id
            .ok_or(eyre::eyre!("engine did not return payload id"))?;

        if !no_tx_pool {
            // Wait before fetching the payload to give the engine time to build it.
            sleep(Duration::from_secs(self.blocktime)).await
        }
        self.engine.get_payload(id).await
    }

    async fn push_payload(&self, payload: ExecutionPayload) -> Result<()> {
        let status = self.engine.new_payload(payload).await?;
        if status.status != Status::Valid && status.status != Status::Accepted {
            eyre::bail!("invalid execution payload");
        }

        Ok(())
    }

    pub async fn update_forkchoice(&self) -> Result<()> {
        let forkchoice = self.create_forkchoice_state();

        let update = self.engine.forkchoice_updated(forkchoice, None).await?;
        if update.payload_status.status != Status::Valid {
            eyre::bail!(
                "could not accept new forkchoice: {:?}",
                update.payload_status.validation_error
            );
        }

        Ok(())
    }

    fn create_forkchoice_state(&self) -> ForkchoiceState {
        ForkchoiceState {
            head_block_hash: self.unsafe_head.hash,
            safe_block_hash: self.safe_head.hash,
            finalized_block_hash: self.finalized_head.hash,
        }
    }

    async fn block_at(&self, timestamp: u64) -> Option<Block<Transaction>> {
        let time_diff = timestamp as i64 - self.finalized_head.timestamp as i64;
        let blocks = time_diff / self.blocktime as i64;
        let block_num = self.finalized_head.number as i64 + blocks;
        self.provider
            .get_block_with_txs(block_num as u64)
            .await
            .ok()?
    }
}

fn should_skip(block: &Block<Transaction>, attributes: &PayloadAttributes) -> Result<bool> {
    tracing::debug!(
        "comparing block at {} with attributes at {}",
        block.timestamp,
        attributes.timestamp
    );

    tracing::debug!("block: {:?}", block);
    tracing::debug!("attributes: {:?}", attributes);

    let attributes_hashes = attributes
        .transactions
        .as_ref()
        .unwrap()
        .iter()
        .map(|tx| H256(keccak256(&tx.0)))
        .collect::<Vec<_>>();

    let block_hashes = block
        .transactions
        .iter()
        .map(|tx| tx.hash())
        .collect::<Vec<_>>();

    tracing::debug!("attribute hashes: {:?}", attributes_hashes);

    let is_same = attributes_hashes == block_hashes
        && attributes.timestamp.as_u64() == block.timestamp.as_u64()
        && attributes.prev_randao == block.mix_hash.unwrap()
        && attributes.suggested_fee_recipient == block.author.unwrap()
        && attributes.gas_limit.as_u64() == block.gas_limit.as_u64();
    // if !is_same {
    //     tracing::info!(
    //         "NOSKIP(while): {:?} {:?} | {} {} | {} {} | {} {} | {} {}",
    //         attributes_hashes,
    //         block_hashes,
    //         attributes.prev_randao,
    //         block.mix_hash.unwrap(),
    //         attributes.suggested_fee_recipient,
    //         block.author.unwrap(),
    //         attributes.timestamp.as_u64(),
    //         block.timestamp.as_u64(),
    //         attributes.gas_limit.as_u64(),
    //         block.gas_limit.as_u64(),
    //     );
    //     let _ = attributes.transactions.as_ref().unwrap().iter().for_each(
    //         |tx| tracing::info!("{}", format!("0x{:?}", hex::encode(&tx.0)))
    //     );
    //     block.transactions.iter().for_each(
    //         |tx|
    //         tracing::info!(
    //             "nonce={} gas_price={} gas={} to={} val={} in={} from={}",
    //             tx.nonce,
    //             tx.gas_price.unwrap(),
    //             tx.gas,
    //             tx.to.unwrap(),
    //             tx.value,
    //             tx.input,
    //             tx.from,
    //         )
    //     );
    //     panic!("WHOOPS");
    // }

    Ok(is_same)
}

impl EngineDriver<EngineApi> {
    pub fn new(
        finalized_head: BlockInfo,
        finalized_epoch: Epoch,
        provider: Provider<Http>,
        config: &Arc<Config>,
    ) -> Result<Self> {
        let engine = Arc::new(EngineApi::new(&config.l2_engine_url, &config.jwt_secret));

        Ok(Self {
            engine,
            provider,
            blocktime: config.chain.blocktime,
            unsafe_head: finalized_head,
            unsafe_epoch: finalized_epoch,
            safe_head: finalized_head,
            safe_epoch: finalized_epoch,
            finalized_head,
            finalized_epoch,
        })
    }
}
