use async_trait::async_trait;
use jsonrpsee::proc_macros::rpc;
use wasix_eth_types::{
    ExecutionPayloadBodyV1, ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3,
    ExecutionPayloadEnvelopeV4, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3,
    ExecutionPayloadV4, ForkchoiceState, ForkchoiceUpdated, PayloadAttributes, PayloadId,
    PayloadStatus, TransitionConfiguration, B256, BlobAndProofV1, BlobAndProofV2, B128, Bytes, U256,
};
use wasix_eth_types::error::{RpcError, RpcResult};
use wasix_eth_utils::{debug, info, metrics::RPC_REQUESTS_TOTAL};
use crate::EngineService;
use serde_json::Value;

#[rpc(server)]
pub trait EngineRpc {
    #[method(name = "engine_exchangeCapabilities")]
    async fn exchange_capabilities(&self, capabilities: Vec<String>) -> RpcResult<Vec<String>>;

    #[method(name = "engine_exchangeTransitionConfigurationV1")]
    async fn exchange_transition_configuration_v1(
        &self,
        config: TransitionConfiguration,
    ) -> RpcResult<TransitionConfiguration>;

    #[method(name = "engine_forkchoiceUpdatedV1")]
    async fn forkchoice_updated_v1(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated>;

    #[method(name = "engine_forkchoiceUpdatedV2")]
    async fn forkchoice_updated_v2(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated>;

    #[method(name = "engine_forkchoiceUpdatedV3")]
    async fn forkchoice_updated_v3(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated>;

    #[method(name = "engine_forkchoiceUpdatedV4")]
    async fn forkchoice_updated_v4(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated>;
    #[method(name = "engine_getPayloadBodiesByHashV1")]
    async fn get_payload_bodies_by_hash_v1(
        &self,
        hashes: Value,
    ) -> RpcResult<Vec<Option<ExecutionPayloadBodyV1>>>;

    #[method(name = "engine_getPayloadBodiesByRangeV1")]
    async fn get_payload_bodies_by_range_v1(
        &self,
        start: Value,
        count: Value,
    ) -> RpcResult<Vec<Option<ExecutionPayloadBodyV1>>>;


    #[method(name = "engine_getPayloadV1")]
    async fn get_payload_v1(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadV1>;

    #[method(name = "engine_getPayloadV2")]
    async fn get_payload_v2(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadEnvelopeV2>;

    #[method(name = "engine_getPayloadV3")]
    async fn get_payload_v3(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadEnvelopeV3>;

    #[method(name = "engine_getPayloadV4")]
    async fn get_payload_v4(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadEnvelopeV4>;

    #[method(name = "engine_newPayloadV1")]
    async fn new_payload_v1(&self, payload: ExecutionPayloadV1) -> RpcResult<PayloadStatus>;

    #[method(name = "engine_newPayloadV2")]
    async fn new_payload_v2(&self, payload: ExecutionPayloadV2) -> RpcResult<PayloadStatus>;

    #[method(name = "engine_newPayloadV3")]
    async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        expected_blob_versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus>;

    #[method(name = "engine_newPayloadV4")]
    async fn new_payload_v4(
        &self,
        payload: ExecutionPayloadV4,
        expected_blob_versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        execution_requests: Vec<Bytes>,
    ) -> RpcResult<PayloadStatus>;

    #[method(name = "engine_getBlobsV1")]
    async fn get_blobs_v1(&self, versioned_hashes: Vec<B256>) -> RpcResult<Vec<Option<BlobAndProofV1>>>;

    #[method(name = "engine_getBlobsV2")]
    async fn get_blobs_v2(&self, versioned_hashes: Vec<B256>) -> RpcResult<Option<Vec<BlobAndProofV2>>>;

    #[method(name = "engine_getBlobsV3")]
    async fn get_blobs_v3(&self, versioned_hashes: Vec<B256>) -> RpcResult<Option<Vec<Option<BlobAndProofV2>>>>;

    #[method(name = "engine_getBlobsV4")]
    async fn get_blobs_v4(&self, versioned_hashes: Vec<B256>, indices_bitarray: B128) -> RpcResult<Option<Value>>;

}

pub struct EngineController {
    pub service: EngineService,
}

#[async_trait]
impl EngineRpcServer for EngineController {
    async fn exchange_capabilities(&self, capabilities: Vec<String>) -> RpcResult<Vec<String>> {
        RPC_REQUESTS_TOTAL.inc();
        debug!("[RPC] engine_exchangeCapabilities: capabilities={:?}", capabilities);
        let result = self.service.exchange_capabilities(capabilities).await?;
        debug!("[RPC] engine_exchangeCapabilities result: {:?}", result);
        Ok(result)
    }

    async fn forkchoice_updated_v1(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        RPC_REQUESTS_TOTAL.inc();
        debug!("[RPC] engine_forkchoiceUpdatedV1: head={:?}, attributes={:?}", forkchoice_state.head_block_hash, payload_attributes);
        let result = self.service.forkchoice_updated_v1(forkchoice_state, payload_attributes).await?;
        debug!("[RPC] engine_forkchoiceUpdatedV1 result status={:?}, payload_id={:?}", result.payload_status.status, result.payload_id);
        Ok(result)
    }

    async fn forkchoice_updated_v2(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        RPC_REQUESTS_TOTAL.inc();
        debug!("[RPC] engine_forkchoiceUpdatedV2: head={:?}, attributes={:?}", forkchoice_state.head_block_hash, payload_attributes);
        let result = self.service.forkchoice_updated_v2(forkchoice_state, payload_attributes).await?;
        debug!("[RPC] engine_forkchoiceUpdatedV2 result status={:?}, payload_id={:?}", result.payload_status.status, result.payload_id);
        Ok(result)
    }

    async fn forkchoice_updated_v3(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        debug!("[RPC] engine_forkchoiceUpdatedV3: head={:?}, attributes={:?}", forkchoice_state.head_block_hash, payload_attributes);
        let result = self.service.forkchoice_updated_v3(forkchoice_state, payload_attributes).await?;
        debug!("[RPC] engine_forkchoiceUpdatedV3 result status={:?}, payload_id={:?}", result.payload_status.status, result.payload_id);
        Ok(result)
    }

    async fn forkchoice_updated_v4(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        debug!("[RPC] engine_forkchoiceUpdatedV4: head={:?}, attributes={:?}", forkchoice_state.head_block_hash, payload_attributes);
        let result = self.service.forkchoice_updated_v4(forkchoice_state, payload_attributes).await?;
        debug!("[RPC] engine_forkchoiceUpdatedV4 result status={:?}, payload_id={:?}", result.payload_status.status, result.payload_id);
        Ok(result)
    }

    async fn exchange_transition_configuration_v1(
        &self,
        config: TransitionConfiguration,
    ) -> RpcResult<TransitionConfiguration> {
        debug!("[RPC] engine_exchangeTransitionConfigurationV1: config={:?}", config);
        let result = self.service.exchange_transition_configuration_v1(config).await?;
        debug!("[RPC] engine_exchangeTransitionConfigurationV1 result: {:?}", result);
        Ok(result)
    }

    async fn get_payload_bodies_by_hash_v1(
        &self,
        hashes: Value,
    ) -> RpcResult<Vec<Option<ExecutionPayloadBodyV1>>> {
        let hashes: Vec<B256> = serde_json::from_value(hashes).map_err(|e: serde_json::Error| RpcError::InvalidParamsCode(e.to_string()))?;
        debug!("[RPC] engine_getPayloadBodiesByHashV1: count={}", hashes.len());
        let result = self.service.get_payload_bodies_by_hash_v1(hashes).await?;
        debug!("[RPC] engine_getPayloadBodiesByHashV1 result count={}", result.len());
        Ok(result)
    }


    async fn get_payload_bodies_by_range_v1(
        &self,
        start: Value,
        count: Value,
    ) -> RpcResult<Vec<Option<ExecutionPayloadBodyV1>>> {
        let start: u64 = serde_json::from_value::<U256>(start).map_err(|e: serde_json::Error| RpcError::InvalidParamsCode(e.to_string()))?.to::<u64>();
        let count: u64 = serde_json::from_value::<U256>(count).map_err(|e: serde_json::Error| RpcError::InvalidParamsCode(e.to_string()))?.to::<u64>();
        debug!("[RPC] engine_getPayloadBodiesByRangeV1: start={}, count={}", start, count);
        let result = self.service.get_payload_bodies_by_range_v1(start, count).await?;
        debug!("[RPC] engine_getPayloadBodiesByRangeV1 result count={}", result.len());
        Ok(result)
    }


    async fn get_payload_v1(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadV1> {
        debug!("[RPC] engine_getPayloadV1: payload_id={:?}", payload_id);
        let result = self.service.get_payload_v1(payload_id).await?;
        debug!("[RPC] engine_getPayloadV1 result: hash={:?}", result.block_hash);
        Ok(result)
    }

    async fn get_payload_v2(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadEnvelopeV2> {
        debug!("[RPC] engine_getPayloadV2: payload_id={:?}", payload_id);
        let result = self.service.get_payload_v2(payload_id).await?;
        debug!("[RPC] engine_getPayloadV2 blockValue: value={:?}", result.block_value);
        Ok(result)
    }

    async fn get_payload_v3(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadEnvelopeV3> {
        debug!("[RPC] engine_getPayloadV3: payload_id={:?}", payload_id);
        let result = self.service.get_payload_v3(payload_id).await?;
        debug!("[RPC] engine_getPayloadV3 result");
        Ok(result)
    }

    async fn get_payload_v4(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadEnvelopeV4> {
        debug!("[RPC] engine_getPayloadV4: payload_id={:?}", payload_id);
        let result = self.service.get_payload_v4(payload_id).await?;
        debug!("[RPC] engine_getPayloadV4 result");
        Ok(result)
    }

    async fn new_payload_v1(&self, payload: ExecutionPayloadV1) -> RpcResult<PayloadStatus> {
        RPC_REQUESTS_TOTAL.inc();
        debug!("[RPC] engine_newPayloadV1: block_number={}, block_hash={:?}", payload.block_number, payload.block_hash);
        let result = self.service.new_payload_v1(payload).await?;
        debug!("[RPC] engine_newPayloadV1 result status={:?}, latest_valid_hash={:?}", result.status, result.latest_valid_hash);
        Ok(result)
    }

    async fn new_payload_v2(&self, payload: ExecutionPayloadV2) -> RpcResult<PayloadStatus> {
        info!("[RPC] engine_newPayloadV2: block_number={}, block_hash={:?}", payload.payload_inner.block_number, payload.payload_inner.block_hash);
        let result = self.service.new_payload_v2(payload).await?;
        info!("[RPC] engine_newPayloadV2 result status={:?}, latest_valid_hash={:?}", result.status, result.latest_valid_hash);
        if matches!(result.status, wasix_eth_types::PayloadStatusEnum::Invalid { .. }) {
            wasix_eth_utils::error!("[RPC] engine_newPayloadV2 returned INVALID status: {:?}", result);
        }
        Ok(result)
    }

    async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        expected_blob_versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus> {
        RPC_REQUESTS_TOTAL.inc();
        debug!("[RPC] engine_newPayloadV3: block_number={}, block_hash={:?}", payload.payload_inner.block_number, payload.payload_inner.block_hash);
        let result = self.service.new_payload_v3(payload, expected_blob_versioned_hashes, parent_beacon_block_root).await?;
        debug!("[RPC] engine_newPayloadV3 result status={:?}, latest_valid_hash={:?}", result.status, result.latest_valid_hash);
        Ok(result)
    }

    async fn new_payload_v4(
        &self,
        payload: ExecutionPayloadV4,
        expected_blob_versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        execution_requests: Vec<Bytes>,
    ) -> RpcResult<PayloadStatus> {
        RPC_REQUESTS_TOTAL.inc();
        debug!("[RPC] engine_newPayloadV4: block_number={}, block_hash={:?}", payload.payload_inner.block_number, payload.payload_inner.block_hash);
        let result = self.service.new_payload_v4(payload, expected_blob_versioned_hashes, parent_beacon_block_root, execution_requests).await?;
        debug!("[RPC] engine_newPayloadV4 result status={:?}, latest_valid_hash={:?}", result.status, result.latest_valid_hash);
        Ok(result)
    }

    async fn get_blobs_v1(&self, versioned_hashes: Vec<B256>) -> RpcResult<Vec<Option<BlobAndProofV1>>> {
        debug!("[RPC] engine_getBlobsV1: versioned_hashes={:?}", versioned_hashes);
        let result = self.service.get_blobs_v1(versioned_hashes).await?;
        debug!("[RPC] engine_getBlobsV1 result");
        Ok(result)
    }

    async fn get_blobs_v2(&self, versioned_hashes: Vec<B256>) -> RpcResult<Option<Vec<BlobAndProofV2>>> {
        debug!("[RPC] engine_getBlobsV2: versioned_hashes={:?}", versioned_hashes);
        let result = self.service.get_blobs_v2(versioned_hashes).await?;
        debug!("[RPC] engine_getBlobsV2 result");
        Ok(result)
    }

    async fn get_blobs_v3(&self, versioned_hashes: Vec<B256>) -> RpcResult<Option<Vec<Option<BlobAndProofV2>>>> {
        debug!("[RPC] engine_getBlobsV3: versioned_hashes={:?}", versioned_hashes);
        let result = self.service.get_blobs_v3(versioned_hashes).await?;
        debug!("[RPC] engine_getBlobsV3 result");
        Ok(result)
    }

    async fn get_blobs_v4(&self, versioned_hashes: Vec<B256>, indices_bitarray: B128) -> RpcResult<Option<Value>> {
        debug!("[RPC] engine_getBlobsV4: versioned_hashes={:?}, indices_bitarray={:?}", versioned_hashes, indices_bitarray);
        let result = self.service.get_blobs_v4(versioned_hashes, indices_bitarray).await?;
        debug!("[RPC] engine_getBlobsV4 result");
        Ok(result)
    }
}
