use jsonrpsee::proc_macros::rpc;
use alloy_primitives::{Address, Bytes, B256, U256};
use crate::rpc::{RpcBlock, RpcReceipt as RpcTransactionReceipt, RpcTransaction};
pub use alloy_rpc_types::eth::Header as RpcHeader;
use crate::{Filter, SyncStatus, TransactionRequest};
use alloy_primitives::Log;
use crate::error::RpcResult;

#[rpc(server)]
pub trait EthRpc {
    #[method(name = "eth_gasPrice")]
    async fn gas_price(&self) -> RpcResult<U256>;
    #[method(name = "eth_accounts")]
    async fn accounts(&self) -> RpcResult<Vec<Address>>;
    #[method(name = "eth_syncing")]
    async fn syncing(&self) -> RpcResult<SyncStatus>;
    #[method(name = "eth_mining")]
    async fn mining(&self) -> RpcResult<bool>;
    #[method(name = "eth_sendTransaction")]
    async fn send_transaction(&self, request: TransactionRequest) -> RpcResult<B256>;
    #[method(name = "eth_sendRawTransaction")]
    async fn send_raw_transaction(&self, data: String) -> RpcResult<B256>;
    #[method(name = "eth_signTransaction")]
    async fn sign_transaction(&self, request: TransactionRequest) -> RpcResult<Bytes>;
    #[method(name = "eth_sign")]
    async fn sign(&self, address: String, message: String) -> RpcResult<Bytes>;
    #[method(name = "eth_blockNumber")]
    async fn block_number(&self) -> RpcResult<U256>;
    #[method(name = "eth_call")]
    async fn call(&self, request: TransactionRequest, block_id: Option<serde_json::Value>) -> RpcResult<Bytes>;
    #[method(name = "eth_estimateGas")]
    async fn estimate_gas(&self, request: TransactionRequest, block_id: Option<serde_json::Value>) -> RpcResult<U256>;
    #[method(name = "eth_getBlockByNumber")]
    async fn get_block_by_number(&self, num: serde_json::Value, full: bool) -> RpcResult<Option<RpcBlock>>;
    #[method(name = "eth_getBlockByHash")]
    async fn get_block_by_hash(&self, hash: serde_json::Value, full: bool) -> RpcResult<Option<RpcBlock>>;
    #[method(name = "eth_chainId")]
    async fn chain_id(&self) -> RpcResult<U256>;
    #[method(name = "eth_getBlockTransactionCountByNumber")]
    async fn get_block_transaction_count_by_number(&self, num: serde_json::Value) -> RpcResult<Option<U256>>;
    #[method(name = "eth_getBlockTransactionCountByHash")]
    async fn get_block_transaction_count_by_hash(&self, hash: serde_json::Value) -> RpcResult<Option<U256>>;
    #[method(name = "eth_getLogs")]
    async fn get_logs(&self, filter: Filter) -> RpcResult<Vec<Log>>;
    #[method(name = "eth_getTransactionByHash")]
    async fn get_transaction_by_hash(&self, hash: serde_json::Value) -> RpcResult<Option<RpcTransaction>>;
    #[method(name = "eth_getTransactionReceipt")]
    async fn get_transaction_receipt(&self, hash: serde_json::Value) -> RpcResult<Option<RpcTransactionReceipt>>;
    #[method(name = "eth_getBalance")]
    async fn get_balance(&self, address: Address, block_id: Option<serde_json::Value>) -> RpcResult<U256>;
    #[method(name = "eth_getTransactionCount")]
    async fn get_transaction_count(&self, address: Address, block_id: Option<serde_json::Value>) -> RpcResult<U256>;
    #[method(name = "eth_getCode")]
    async fn get_code(&self, address: Address, block_id: Option<serde_json::Value>) -> RpcResult<Bytes>;
    #[method(name = "eth_getStorageAt")]
    async fn get_storage_at(&self, address: Address, slot: serde_json::Value, block_id: Option<serde_json::Value>) -> RpcResult<B256>;

    #[method(name = "eth_getBlockReceipts")]
    async fn get_block_receipts(&self, block_id: serde_json::Value) -> RpcResult<Option<Vec<RpcTransactionReceipt>>>;

    #[method(name = "eth_blobBaseFee")]
    async fn blob_base_fee(&self) -> RpcResult<U256>;

    #[method(name = "eth_maxPriorityFeePerGas")]
    async fn max_priority_fee_per_gas(&self) -> RpcResult<U256>;
}
