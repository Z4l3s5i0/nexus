use alloy_rpc_types::engine::{
    ExecutionPayloadV1, 
    ExecutionPayloadV2 as AlloyExecutionPayloadV2,
    ExecutionPayloadV3 as AlloyExecutionPayloadV3,
    ExecutionPayloadV4 as AlloyExecutionPayloadV4,
};
use crate::eip4895::Withdrawal;
use alloy_primitives::U256;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPayloadV2 {
    #[serde(flatten)]
    pub payload_inner: ExecutionPayloadV1,
    pub withdrawals: Vec<Withdrawal>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPayloadV3 {
    #[serde(flatten)]
    pub payload_inner: ExecutionPayloadV2,
    pub blob_gas_used: u64,
    pub excess_blob_gas: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionPayloadV4 {
    #[serde(flatten)]
    pub payload_inner: ExecutionPayloadV3,
    pub block_access_list: alloy_primitives::Bytes,
    pub slot_number: u64,
}

impl From<ExecutionPayloadV2> for AlloyExecutionPayloadV2 {
    fn from(payload: ExecutionPayloadV2) -> Self {
        Self {
            payload_inner: payload.payload_inner,
            withdrawals: payload.withdrawals.into_iter().map(|w| w.into()).collect(),
        }
    }
}

impl From<ExecutionPayloadV3> for AlloyExecutionPayloadV3 {
    fn from(payload: ExecutionPayloadV3) -> Self {
        Self {
            payload_inner: payload.payload_inner.into(),
            blob_gas_used: payload.blob_gas_used,
            excess_blob_gas: payload.excess_blob_gas,
        }
    }
}

impl From<ExecutionPayloadV4> for AlloyExecutionPayloadV4 {
    fn from(payload: ExecutionPayloadV4) -> Self {
        Self {
            payload_inner: payload.payload_inner.into(),
            block_access_list: payload.block_access_list,
            slot_number: payload.slot_number,
        }
    }
}
