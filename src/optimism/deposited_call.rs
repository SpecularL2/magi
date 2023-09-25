use ethers::{
    abi::parse_abi_str,
    prelude::BaseContract,
    types::{Bytes, H256, U256},
};
use eyre::Result;

use crate::common::Epoch;

pub struct AttributesDepositedCall {
    pub number: u64,
    pub timestamp: u64,
    pub basefee: U256,
    pub hash: H256,
    pub sequence_number: u64,
    pub batcher_hash: H256,
    pub fee_overhead: U256,
    pub fee_scalar: U256,
}

type SetL1BlockValueInput = (u64, u64, U256, H256, u64, H256, U256, U256);
const L1_BLOCK_CONTRACT_ABI: &str = r#"[
    function setL1BlockValues(uint64 _number,uint64 _timestamp, uint256 _basefee, bytes32 _hash,uint64 _sequenceNumber,bytes32 _batcherHash,uint256 _l1FeeOverhead,uint256 _l1FeeScalar) external
]"#;

impl TryFrom<Bytes> for AttributesDepositedCall {
    type Error = eyre::Report;

    fn try_from(value: Bytes) -> Result<Self> {
        let abi = BaseContract::from(parse_abi_str(L1_BLOCK_CONTRACT_ABI)?);

        let (
            number,
            timestamp,
            basefee,
            hash,
            sequence_number,
            batcher_hash,
            fee_overhead,
            fee_scalar,
        ): SetL1BlockValueInput = abi.decode("setL1BlockValues", value)?;

        Ok(Self {
            number,
            timestamp,
            basefee,
            hash,
            sequence_number,
            batcher_hash,
            fee_overhead,
            fee_scalar,
        })
    }
}

impl From<&AttributesDepositedCall> for Epoch {
    fn from(call: &AttributesDepositedCall) -> Self {
        Self {
            number: call.number,
            timestamp: call.timestamp,
            hash: call.hash,
        }
    }
}

#[cfg(test)]
mod tests {
    mod attributed_deposited_call {
        use std::str::FromStr;

        use ethers::types::{Bytes, H256};

        use crate::optimism::deposited_call::AttributesDepositedCall;

        #[test]
        fn decode_from_bytes() -> eyre::Result<()> {
            // Arrange
            let calldata = "0x015d8eb900000000000000000000000000000000000000000000000000000000008768240000000000000000000000000000000000000000000000000000000064443450000000000000000000000000000000000000000000000000000000000000000e0444c991c5fe1d7291ff34b3f5c3b44ee861f021396d33ba3255b83df30e357d00000000000000000000000000000000000000000000000000000000000000050000000000000000000000007431310e026b69bfc676c0013e12a1a11411eec9000000000000000000000000000000000000000000000000000000000000083400000000000000000000000000000000000000000000000000000000000f4240";

            let expected_hash =
                H256::from_str("0444c991c5fe1d7291ff34b3f5c3b44ee861f021396d33ba3255b83df30e357d")?;
            let expected_block_number = 8874020;
            let expected_timestamp = 1682191440;

            // Act
            let call = AttributesDepositedCall::try_from(Bytes::from_str(calldata)?);

            // Assert
            assert!(call.is_ok());
            let call = call.unwrap();

            assert_eq!(call.hash, expected_hash);
            assert_eq!(call.number, expected_block_number);
            assert_eq!(call.timestamp, expected_timestamp);

            Ok(())
        }
    }
}
