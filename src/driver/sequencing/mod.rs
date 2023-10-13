use async_trait::async_trait;
use eyre::Result;

use crate::{common::BlockInfo, engine::PayloadAttributes, l1::L1BlockInfo};

pub mod utils;

#[async_trait]
pub trait SequencingSource {
    /// Returns true iff:
    /// 1. `parent_l2_block` is within the max safe lag (i.e. the unsafe head isn't too far ahead of the safe head).
    /// 2. The next timestamp isn't in the future.
    fn is_ready(&self, safe_l2_head: &BlockInfo, parent_l2_block: &BlockInfo) -> bool;
    /// Returns the attributes for the next payload to be built, if any.
    /// If no new payload should be built yet, returns None.
    async fn get_next_attributes(
        &self,
        parent_l2_block: &BlockInfo,
        parent_l1_epoch: &L1BlockInfo,
        next_l1_epoch: Option<&L1BlockInfo>,
    ) -> Result<PayloadAttributes>;
}

pub struct NoOp;
#[async_trait]
impl SequencingSource for NoOp {
    fn is_ready(&self, _: &BlockInfo, _: &BlockInfo) -> bool {
        false
    }

    async fn get_next_attributes(
        &self,
        _: &BlockInfo,
        _: &L1BlockInfo,
        _: Option<&L1BlockInfo>,
    ) -> Result<PayloadAttributes> {
        Ok(Default::default())
    }
}

/// Using this just enables avoiding explicit type qualification everywhere.
pub fn none() -> Option<NoOp> {
    None
}
