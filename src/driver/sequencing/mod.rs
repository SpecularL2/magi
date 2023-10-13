use async_trait::async_trait;
use eyre::Result;

use crate::{common::BlockInfo, engine::PayloadAttributes, l1::L1BlockInfo};

#[async_trait]
pub trait SequencingSource {
    /// Returns the attributes for the next payload to be built, if any.
    /// If no new payload should be built yet, returns None.
    async fn get_next_attributes(
        &self,
        safe_l2_head: BlockInfo,
        parent_l2_block: BlockInfo,
        parent_l2_block_origin: L1BlockInfo,
    ) -> Result<Option<PayloadAttributes>>;
}

pub struct NoOp;
#[async_trait]
impl SequencingSource for NoOp {
    async fn get_next_attributes(
        &self,
        _safe_l2_head: BlockInfo,
        _parent_l2_block: BlockInfo,
        _parent_l2_block_origin: L1BlockInfo,
    ) -> Result<Option<PayloadAttributes>> {
        Ok(None)
    }
}

/// Using this just enables avoiding explicit type qualification everywhere.
pub fn none() -> Option<NoOp> {
    None
}
