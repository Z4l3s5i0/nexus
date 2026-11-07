use async_trait::async_trait;
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::tracing::info;
use wasix_eth_types::{BlockId, B256, Bytes, RpcTransaction};
use wasix_eth_types::error::{parse_loose_hash, RpcError, RpcResult};
use wasix_eth_utils::{transaction_mapper::TransactionMapper, metrics::RPC_REQUESTS_TOTAL};
use crate::DebugService;

#[rpc(server)]
pub trait DebugRpc {
    #[method(name = "debug_getMempool")]
    async fn get_mempool(&self) -> RpcResult<Vec<RpcTransaction>>;

    #[method(name = "debug_getRawBlock")]
    async fn get_raw_block(&self, id: serde_json::Value) -> RpcResult<Bytes>;

    #[method(name = "debug_getRawHeader")]
    async fn get_raw_header(&self, id: serde_json::Value) -> RpcResult<Bytes>;

    #[method(name = "debug_getRawReceipts")]
    async fn get_raw_receipts(&self, id: serde_json::Value) -> RpcResult<Vec<Bytes>>;

    #[method(name = "debug_getRawTransaction")]
    async fn get_raw_transaction(&self, hash: serde_json::Value) -> RpcResult<Bytes>;
}

pub struct DebugController {
    pub service: DebugService,
}

#[async_trait]
impl DebugRpcServer for DebugController {
    async fn get_mempool(&self) -> RpcResult<Vec<RpcTransaction>> {
        RPC_REQUESTS_TOTAL.inc();
        info!("[RPC] debug_getMempool");
        let mempool_txs = self.service.get_mempool().await?;
        let result: Vec<RpcTransaction> = mempool_txs.into_iter().map(|tx| TransactionMapper::to_rpc_transaction(tx, None, None)).collect();
        info!("[RPC] debug_getMempool result count: {}", result.len());
        Ok(result)
    }

    async fn get_raw_block(&self, id: serde_json::Value) -> RpcResult<Bytes> {
        RPC_REQUESTS_TOTAL.inc();
        info!("[RPC] debug_getRawBlock: {:?}", id);
        let id: BlockId = serde_json::from_value(id).map_err(|e| RpcError::InvalidParamsCode(e.to_string()))?;
        self.service.get_raw_block(id).await.map_err(|e| e.into())
    }

    async fn get_raw_header(&self, id: serde_json::Value) -> RpcResult<Bytes> {
        RPC_REQUESTS_TOTAL.inc();
        info!("[RPC] debug_getRawHeader: {:?}", id);
        let id: BlockId = serde_json::from_value(id).map_err(|e| RpcError::InvalidParamsCode(e.to_string()))?;
        self.service.get_raw_header(id).await.map_err(|e| e.into())
    }

    async fn get_raw_receipts(&self, id: serde_json::Value) -> RpcResult<Vec<Bytes>> {
        RPC_REQUESTS_TOTAL.inc();
        info!("[RPC] debug_getRawReceipts: {:?}", id);
        let id: BlockId = serde_json::from_value(id).map_err(|e| RpcError::InvalidParamsCode(e.to_string()))?;
        self.service.get_raw_receipts(id).await.map_err(|e| e.into())
    }

    async fn get_raw_transaction(&self, hash: serde_json::Value) -> RpcResult<Bytes> {
        RPC_REQUESTS_TOTAL.inc();
        info!("[RPC] debug_getRawTransaction: {:?}", hash);
        let hash: B256 = parse_loose_hash(hash)?;
        self.service.get_raw_transaction(hash).await.map_err(|e| e.into())
    }
}