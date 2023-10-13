use ethers::types::H160;

use crate::config;

pub struct Config {
    pub l1_rpc_url: String,
    pub max_safe_lag: u64,
    pub max_seq_drift: u64,
    pub blocktime: u64,
    pub suggested_fee_recipient: H160,
    pub system_config: SystemConfig,
}

pub struct SystemConfig {
    pub gas_limit: u64,
}

impl Config {
    pub fn new(config: &config::Config) -> Self {
        Self {
            l1_rpc_url: config.l1_rpc_url.clone(),
            max_safe_lag: config.local_sequencer.max_safe_lag,
            max_seq_drift: config.chain.max_seq_drift,
            blocktime: config.chain.blocktime,
            suggested_fee_recipient: config.local_sequencer.suggested_fee_recipient,
            system_config: SystemConfig::new(&config.chain.system_config),
        }
    }
}

impl SystemConfig {
    pub fn new(config: &config::SystemConfig) -> Self {
        Self {
            gas_limit: config.gas_limit.as_u64(),
        }
    }
}
