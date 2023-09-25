use crate::optimism::deposited_tx::UserDeposited;

#[derive(Debug)]
pub struct L1OptimismInfo {
    /// User deposits from that block
    pub user_deposits: Vec<UserDeposited>,
}
