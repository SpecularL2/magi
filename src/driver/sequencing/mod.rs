use std::sync::{RwLock, Arc};

use crate::{common::BlockInfo, engine::PayloadAttributes, derive::state::State};


pub trait SequencingSource {
    fn get_next_attributes(&self, parent_l2_block: BlockInfo, state: &Arc<RwLock<State>>) -> Option<PayloadAttributes>;
}

pub struct NoOp;
impl SequencingSource for NoOp {
    fn get_next_attributes(&self, _parent_l2_block: BlockInfo, _state: &Arc<RwLock<State>>) -> Option<PayloadAttributes> { None }
}