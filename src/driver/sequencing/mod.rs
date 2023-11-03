use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use ethers::providers::{Http, JsonRpcClient, Provider};
use eyre::Result;
use futures::future::Either;
use futures::join;

use crate::{
    common::BlockInfo,
    derive::state::State,
    engine::{Engine, PayloadAttributes},
    l1::{utils::get_l1_block_info, L1BlockInfo},
};

use super::engine_driver::EngineDriver;

/// TODO: Support system config updates.
#[async_trait(?Send)]
pub trait SequencingSource<E: Engine> {
    /// Returns the next payload attributes to be built (if any) on top of
    /// the current unsafe head, as determined by inspecting the `engine_driver`.
    /// If no attributes are ready to be built, returns `None`.
    async fn get_next_attributes(
        &self,
        state: &Arc<RwLock<State>>,
        engine_driver: &EngineDriver<E>,
    ) -> Result<Option<PayloadAttributes>>;

    /// Returns true if the attributes should be skipped in the derivation pipeline.
    /// Provides flexibility to post-process attributes before they are used in an async way.
    async fn should_skip_attributes(&mut self, attributes: &PayloadAttributes) -> Result<bool>;
}

pub struct Source<T: SequencingPolicy, U: JsonRpcClient> {
    /// The sequencing policy to use to build attributes.
    policy: T,
    /// L1 provider for ad-hoc queries
    provider: Provider<U>,
}

impl<T: SequencingPolicy, U: JsonRpcClient> Source<T, U> {
    pub fn new(policy: T, provider: Provider<U>) -> Self {
        Self { policy, provider }
    }
}

#[async_trait(?Send)]
impl<E: Engine, T: SequencingPolicy, U: JsonRpcClient> SequencingSource<E> for Source<T, U> {
    async fn get_next_attributes(
        &self,
        state: &Arc<RwLock<State>>,
        engine_driver: &EngineDriver<E>,
    ) -> Result<Option<PayloadAttributes>> {
        let parent_l2_block = &engine_driver.unsafe_head;
        let safe_l2_head = {
            let state = state.read().unwrap();
            state.safe_head
        };
        // Check if we're ready to try building a new payload.
        if !self.policy.is_ready(parent_l2_block, &safe_l2_head) {
            return Ok(None);
        }
        // Get full l1 epoch info.
        let parent_epoch = engine_driver.unsafe_epoch;
        let (parent_l1_epoch, next_l1_epoch) = {
            // Acquire read lock on state to get epoch info (if it exists).
            let state = state.read().unwrap();
            (
                state
                    .l1_info_by_hash(parent_epoch.hash)
                    .map(|i| i.block_info.clone()),
                state
                    .l1_info_by_number(parent_epoch.number + 1)
                    .map(|i| i.block_info.clone()),
            )
        };
        // Get l1 epoch info from provider if it doesn't exist in state.
        // TODO: consider using caching e.g. with the cached crate.
        let (parent_l1_epoch, next_l1_epoch) = join!(
            match parent_l1_epoch {
                Some(info) => Either::Left(async { Ok(info) }),
                None => Either::Right(get_l1_block_info(parent_epoch.hash, &self.provider)),
            },
            match next_l1_epoch {
                Some(info) => Either::Left(async { Ok(info) }),
                None => Either::Right(get_l1_block_info(parent_epoch.number + 1, &self.provider)),
            },
        );
        // TODO: handle recoverable errors, if any.
        // Get next payload attributes and build the payload.
        Ok(Some(
            self.policy
                .get_attributes(
                    parent_l2_block,
                    &parent_l1_epoch?,
                    next_l1_epoch.ok().as_ref(),
                )
                .await?,
        ))
    }

    async fn should_skip_attributes(&mut self, attributes: &PayloadAttributes) -> Result<bool> {
        self.policy.should_skip_attributes(attributes).await
    }
}

#[async_trait]
pub trait SequencingPolicy {
    /// Returns true iff the policy is ready to build a payload on top of `parent_l2_block`.
    fn is_ready(&self, parent_l2_block: &BlockInfo, safe_l2_head: &BlockInfo) -> bool;
    /// Returns the attributes for a payload to be built on top of `parent_l2_block`.
    /// If `next_l1_epoch` is `None`, `parent_l1_epoch` is attempted to be used as the epoch.
    /// However, if it's too late to use `parent_l1_epoch` as the epoch, an error is returned.
    async fn get_attributes(
        &self,
        parent_l2_block: &BlockInfo,
        parent_l1_epoch: &L1BlockInfo,
        next_l1_epoch: Option<&L1BlockInfo>,
    ) -> Result<PayloadAttributes>;

    /// Returns true if the attributes should be skipped in the derivation pipeline.
    /// Provides flexibility to post-process attributes before they are used in an async way.
    async fn should_skip_attributes(&mut self, attributes: &PayloadAttributes) -> Result<bool>;
}

pub struct NoOp;
#[async_trait]
impl SequencingPolicy for NoOp {
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

    async fn should_skip_attributes(&mut self, _: &PayloadAttributes) -> Result<bool> {
        Ok(false)
    }
}

/// Using this just enables avoiding explicit type qualification everywhere.
pub fn none() -> Option<Source<NoOp, Http>> {
    None
}
