use async_trait::async_trait;

use crate::{common::BlockInfo, engine::PayloadAttributes, derive::state::State, l1::L1BlockInfo};


#[async_trait]
pub trait SequencingSource {
    /// Returns the attributes for the next payload to be built, if any.
    /// If no new payload should be built yet, returns None.
    async fn get_next_attributes(
        &self,
        parent_l2_block: BlockInfo,
        parent_l2_block_epoch: L1BlockInfo,
        state: &State,
    ) -> Option<PayloadAttributes>;
}

pub struct NoOp;
#[async_trait]
impl SequencingSource for NoOp {
    async fn get_next_attributes(
        &self,
        _parent_l2_block: BlockInfo,
        _parent_l2_block_epoch: L1BlockInfo,
        _state: &State,
    ) -> Option<PayloadAttributes> { None }
}