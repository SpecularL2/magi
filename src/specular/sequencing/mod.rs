use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use ethers::{
    middleware::SignerMiddleware,
    providers::{Http, Middleware, Provider},
    signers::{LocalWallet, Signer},
    types::{TransactionRequest, H256, U256, U64},
};
use eyre::Result;

use crate::{
    common::{BlockInfo, Epoch, RawTransaction},
    driver::sequencing::SequencingPolicy,
    engine::PayloadAttributes,
    l1::L1BlockInfo,
};

use crate::specular::common::{
    SetL1OracleValuesInput, SET_L1_ORACLE_VALUES_ABI, SET_L1_ORACLE_VALUES_SELECTOR,
};

pub mod config;

pub struct AttributesBuilder {
    config: config::Config,
    client: Option<SignerMiddleware<Provider<Http>, LocalWallet>>,
}

impl AttributesBuilder {
    pub fn new(config: config::Config, l2_provider: Option<Provider<Http>>) -> Self {
        let wallet = LocalWallet::try_from(config.sequencer_private_key.clone())
            .expect("invalid sequencer private key");
        let client = l2_provider.map(|l2_provider| SignerMiddleware::new(l2_provider, wallet));
        Self { config, client }
    }

    /// Returns the next l2 block timestamp, given the `parent_block_timestamp`.
    fn next_timestamp(&self, parent_block_timestamp: u64) -> u64 {
        parent_block_timestamp + self.config.blocktime
    }

    /// Returns the drift bound on the next l2 block's timestamp.
    fn next_drift_bound(&self, curr_origin: &L1BlockInfo) -> u64 {
        curr_origin.timestamp + self.config.max_seq_drift
    }

    /// Finds the origin of the next L2 block: either the current origin or the next, if sufficient time has passed.
    async fn find_next_origin(
        &self,
        curr_l2_block: &BlockInfo,
        curr_l1_epoch: &L1BlockInfo,
        next_l1_epoch: Option<&L1BlockInfo>,
    ) -> Result<L1BlockInfo> {
        let next_l2_ts = self.next_timestamp(curr_l2_block.timestamp);
        let next_drift_bound = self.next_drift_bound(curr_l1_epoch);
        let is_drift_bound_exceeded = next_l2_ts > next_drift_bound;
        if is_drift_bound_exceeded {
            tracing::info!("Next l2 ts exceeds the drift bound {}", next_drift_bound);
        }
        match (next_l1_epoch, is_drift_bound_exceeded) {
            // We found the next l1 block.
            (Some(next_l1_epoch), _) => {
                if next_l2_ts >= next_l1_epoch.timestamp {
                    Ok(next_l1_epoch.clone())
                } else {
                    Ok(curr_l1_epoch.clone())
                }
            }
            // We exceeded the drift bound, so we can't use the current origin.
            // But we also can't use the next l1 block since we don't have it.
            (_, true) => Err(eyre::eyre!("current origin drift bound exceeded.")),
            // We're not exceeding the drift bound, so we can just use the current origin.
            (_, false) => {
                tracing::info!("Falling back to current origin (next is unknown).");
                Ok(curr_l1_epoch.clone())
            }
        }
    }
}

#[async_trait]
impl SequencingPolicy for AttributesBuilder {
    /// Returns true iff:
    /// 1. `parent_l2_block` is within the max safe lag (i.e. the unsafe head isn't too far ahead of the safe head).
    /// 2. The next timestamp isn't in the future.
    fn is_ready(&self, parent_l2_block: &BlockInfo, safe_l2_head: &BlockInfo) -> bool {
        safe_l2_head.number + self.config.max_safe_lag > parent_l2_block.number
            && self.next_timestamp(parent_l2_block.timestamp) <= unix_now()
    }

    async fn get_attributes(
        &self,
        parent_l2_block: &BlockInfo,
        parent_l1_epoch: &L1BlockInfo,
        next_l1_epoch: Option<&L1BlockInfo>,
    ) -> Result<PayloadAttributes> {
        let next_origin = self
            .find_next_origin(parent_l2_block, parent_l1_epoch, next_l1_epoch)
            .await?;
        let timestamp = self.next_timestamp(parent_l2_block.timestamp);
        let prev_randao = next_randao(&next_origin);
        let suggested_fee_recipient = self.config.system_config.batch_sender;
        let client = self
            .client
            .as_ref()
            .ok_or(eyre::eyre!("client not initialized"))?;
        let txs = create_l1_oracle_update_transaction(
            &self.config,
            client,
            parent_l2_block,
            parent_l1_epoch,
            &next_origin,
        )
        .await?;
        let no_tx_pool = timestamp > self.config.max_seq_drift;
        let gas_limit = self.config.system_config.gas_limit;
        Ok(PayloadAttributes {
            timestamp: U64::from(timestamp),
            prev_randao,
            suggested_fee_recipient,
            transactions: txs,
            no_tx_pool,
            gas_limit: U64::from(gas_limit),
            epoch: Some(create_epoch(next_origin)),
            l1_inclusion_block: None,
            seq_number: None,
        })
    }
}

// TODO: implement. requires l1 info tx. requires signer...
// Creates the transaction(s) to include at the top of the next l2 block.
async fn create_l1_oracle_update_transaction(
    config: &config::Config,
    client: &SignerMiddleware<Provider<Http>, LocalWallet>,
    parent_l2_block: &BlockInfo,
    parent_l1_epoch: &L1BlockInfo,
    origin: &L1BlockInfo,
) -> Result<Option<Vec<RawTransaction>>> {
    if parent_l1_epoch.number == origin.number {
        // Do not include the L1 oracle update tx if we are still in the same L1 epoch.
        return Ok(None);
    }
    // Construct L1 oracle update transaction data
    let set_l1_oracle_values_input: SetL1OracleValuesInput = (
        U256::from(origin.number),
        U256::from(origin.timestamp),
        origin.base_fee,
        origin.hash,
        origin.state_root,
    );
    let input = SET_L1_ORACLE_VALUES_ABI
        .encode_with_selector(*SET_L1_ORACLE_VALUES_SELECTOR, set_l1_oracle_values_input)
        .expect("failed to encode setL1OracleValues input");
    // Construct L1 oracle update transaction
    let mut tx = TransactionRequest::new()
        .to(config.l1_oracle)
        .gas(150_000_000) // TODO[zhe]: consider to lower this number
        .value(0)
        .data(input)
        .into();
    // TODO[zhe]: here we let the provider to fill in the gas price
    // TODO[zhe]: consider to make it constant?
    client
        .fill_transaction(&mut tx, Some((parent_l2_block.number + 1).into()))
        .await?;
    let signature = client.signer().sign_transaction(&tx).await?;
    let raw_tx = tx.rlp_signed(&signature);
    Ok(Some(vec![RawTransaction(raw_tx.0.into())]))
}

/// Returns the next l2 block randao, reusing that of the `next_origin`.
fn next_randao(next_origin: &L1BlockInfo) -> H256 {
    next_origin.mix_hash
}

/// Extracts the epoch information from `info`.
fn create_epoch(info: L1BlockInfo) -> Epoch {
    Epoch {
        number: info.number,
        hash: info.hash,
        timestamp: info.timestamp,
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use crate::{common::BlockInfo, driver::sequencing::SequencingPolicy};

    use super::{config, unix_now, AttributesBuilder};
    use ethers::abi::Address;
    use eyre::Result;
    #[test]
    fn test_is_ready() -> Result<()> {
        // Setup.
        let config = config::Config {
            blocktime: 2,
            max_seq_drift: 0, // anything
            max_safe_lag: 10,
            system_config: config::SystemConfig {
                batch_sender: Address::zero(),
                gas_limit: 1,
            }, // anything
            l1_oracle: Address::zero(),
            // random publicly known private key
            sequencer_private_key:
                "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318".to_string(),
        };
        let attrs_builder = AttributesBuilder::new(config.clone(), None);
        // Run test cases.
        let cases = vec![(true, true), (true, false), (false, true), (false, false)];
        for case in cases.iter() {
            let (input, expected) = generate_is_ready_case(case.0, case.1, config.clone());
            assert_eq!(
                attrs_builder.is_ready(&input.0, &input.1),
                expected,
                "case: {:?}",
                case
            );
        }
        Ok(())
    }

    /// Generates an (input, expected-output) test-case pair for `is_ready`.
    fn generate_is_ready_case(
        exceeds_lag: bool,
        exceeds_present: bool,
        config: config::Config,
    ) -> ((BlockInfo, BlockInfo), bool) {
        let now = unix_now();
        let parent_info = BlockInfo {
            number: if exceeds_lag {
                config.max_safe_lag
            } else {
                config.max_safe_lag - 1
            },
            hash: Default::default(),
            parent_hash: Default::default(),
            timestamp: if exceeds_present {
                now
            } else {
                now - config.blocktime
            },
        };
        let safe_head = BlockInfo {
            number: 0,
            hash: Default::default(),
            parent_hash: Default::default(),
            timestamp: 0,
        };
        ((parent_info, safe_head), !exceeds_lag && !exceeds_present)
    }
}
