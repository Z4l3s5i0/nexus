use std::sync::Arc;
use wasix_eth_storage::{ChainProvider, BlockProvider};
use wasix_eth_core::Engine;
use wasix_eth_types::eip4895::Withdrawal;
use wasix_eth_types::{ExecutionPayloadBodyV1, ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4, ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, ExecutionPayloadV4, ForkchoiceState, ForkchoiceUpdated, PayloadAttributes, PayloadId, PayloadStatus, TransitionConfiguration, B256, B128, Bytes, U256};
use wasix_eth_types::{BlobAndProofV1, BlobAndProofV2};
use wasix_eth_types::error::RpcResult;
use wasix_eth_utils::info;
use serde_json::Value;

pub struct EngineService {
    pub engine: Arc<Engine>,
}

impl EngineService {
    pub fn new(engine: Arc<Engine>) -> Self {
        info!("[EngineService] Initializing engine service");
        Self { engine }
    }

    pub async fn exchange_capabilities(&self, _capabilities: Vec<String>) -> RpcResult<Vec<String>> {
        Ok(vec![
            "engine_exchangeCapabilities".to_string(),
            "engine_forkchoiceUpdatedV1".to_string(),
            "engine_forkchoiceUpdatedV2".to_string(),
            "engine_forkchoiceUpdatedV3".to_string(),
            "engine_forkchoiceUpdatedV4".to_string(),
            "engine_newPayloadV1".to_string(),
            "engine_newPayloadV2".to_string(),
            "engine_newPayloadV3".to_string(),
            "engine_newPayloadV4".to_string(),
            "engine_getPayloadV1".to_string(),
            "engine_getPayloadV2".to_string(),
            "engine_getPayloadV3".to_string(),
            "engine_getPayloadV4".to_string(),
            "engine_exchangeTransitionConfigurationV1".to_string(),
            "engine_getPayloadBodiesByHashV1".to_string(),
            "engine_getPayloadBodiesByRangeV1".to_string(),
            "engine_getBlobsV1".to_string(),
            "engine_getBlobsV2".to_string(),
            "engine_getBlobsV3".to_string(),
            "engine_getBlobsV4".to_string(),
        ])
    }

    pub async fn forkchoice_updated_v3(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        self.forkchoice_updated(forkchoice_state, payload_attributes, 3).await
    }

    pub async fn forkchoice_updated_v4(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        self.forkchoice_updated(forkchoice_state, payload_attributes, 4).await
    }

    pub async fn new_payload_v3(
        &self,
        payload: ExecutionPayloadV3,
        expected_blob_versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> RpcResult<PayloadStatus> {
        self.engine.new_payload_v3(payload, expected_blob_versioned_hashes, parent_beacon_block_root).await
    }

    pub async fn new_payload_v4(
        &self,
        payload: ExecutionPayloadV4,
        expected_blob_versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
        execution_requests: Vec<Bytes>,
    ) -> RpcResult<PayloadStatus> {
        self.engine.new_payload_v4(payload, expected_blob_versioned_hashes, parent_beacon_block_root, execution_requests).await
    }

    pub async fn exchange_transition_configuration_v1(
        &self,
        config: TransitionConfiguration,
    ) -> RpcResult<TransitionConfiguration> {
        let chain_config = self.engine.read_storage.chain_config().await.ok().flatten().unwrap_or_default();
        let genesis_hash = self.engine.read_storage.block_hash(0).await.ok().flatten().unwrap_or(B256::ZERO);

        let ttd = chain_config.terminal_total_difficulty.unwrap_or(U256::ZERO);
        
        info!("[EngineService] exchange_transition_configuration_v1: client_ttd={:?}, client_hash={:?}, our_ttd={:?}, our_hash={:?}", 
            config.terminal_total_difficulty, config.terminal_block_hash, ttd, genesis_hash);

        Ok(TransitionConfiguration {
            terminal_total_difficulty: ttd,
            terminal_block_hash: genesis_hash,
            terminal_block_number: 0,
        })
    }

    pub async fn get_payload_bodies_by_hash_v1(
        &self,
        hashes: Vec<B256>,
    ) -> RpcResult<Vec<Option<ExecutionPayloadBodyV1>>> {
        self.engine.get_payload_bodies_by_hash(hashes).await
    }


    pub async fn get_payload_bodies_by_range_v1(
        &self,
        start: u64,
        count: u64,
    ) -> RpcResult<Vec<Option<ExecutionPayloadBodyV1>>> {
        self.engine.get_payload_bodies_by_range(start, count).await
    }

    pub async fn get_payload_v1(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadV1> {
        self.engine.get_payload_v1(payload_id).await
    }
    pub async fn get_payload_v2(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadEnvelopeV2> {
        self.engine.get_payload_v2(payload_id).await
    }
    pub async fn get_payload_v3(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadEnvelopeV3> {
        self.engine.get_payload_v3(payload_id).await
    }
    pub async fn get_payload_v4(&self, payload_id: PayloadId) -> RpcResult<ExecutionPayloadEnvelopeV4> {
        self.engine.get_payload_v4(payload_id).await
    }

    pub async fn get_blobs_v1(&self, versioned_hashes: Vec<B256>) -> RpcResult<Vec<Option<BlobAndProofV1>>> {
        self.engine.get_blobs_v1(versioned_hashes).await
    }

    pub async fn get_blobs_v2(&self, versioned_hashes: Vec<B256>) -> RpcResult<Option<Vec<BlobAndProofV2>>> {
        self.engine.get_blobs_v2(versioned_hashes).await
    }

    pub async fn get_blobs_v3(&self, versioned_hashes: Vec<B256>) -> RpcResult<Option<Vec<Option<BlobAndProofV2>>>> {
        self.engine.get_blobs_v3(versioned_hashes).await
    }

    pub async fn get_blobs_v4(&self, versioned_hashes: Vec<B256>, indices_bitarray: B128) -> RpcResult<Option<Value>> {
        self.engine.get_blobs_v4(versioned_hashes, indices_bitarray).await
    }
    
    pub async fn forkchoice_updated_v1(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        self.forkchoice_updated(forkchoice_state, payload_attributes, 1).await
    }

    pub async fn forkchoice_updated_v2(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
    ) -> RpcResult<ForkchoiceUpdated> {
        self.forkchoice_updated(forkchoice_state, payload_attributes, 2).await
    }

    async fn forkchoice_updated(
        &self,
        forkchoice_state: ForkchoiceState,
        payload_attributes: Option<PayloadAttributes>,
        version: u8,
    ) -> RpcResult<ForkchoiceUpdated> {
        info!("[EngineService] forkchoiceUpdated: head={:?}, payload_attributes={:?}", forkchoice_state.head_block_hash, payload_attributes);
        self.engine.forkchoice_updated(forkchoice_state, payload_attributes, version).await
    }
    
    pub async fn new_payload_v1(&self, payload: ExecutionPayloadV1) -> RpcResult<PayloadStatus> {
        self.new_payload(payload.into(), None).await
    }

    pub async fn new_payload_v2(&self, payload: ExecutionPayloadV2) -> RpcResult<PayloadStatus> {
        self.new_payload(payload.payload_inner.into(), Some(payload.withdrawals)).await
    }

    async fn new_payload(
        &self,
        payload_v1: ExecutionPayloadV1,
        withdrawals: Option<Vec<Withdrawal>>,
    ) -> RpcResult<PayloadStatus> {
        info!("[EngineService] newPayload: block_number={}, block_hash={:?}, parent_hash={:?}", payload_v1.block_number, payload_v1.block_hash, payload_v1.parent_hash);
        
        self.engine.new_payload(payload_v1, withdrawals).await
    }

}