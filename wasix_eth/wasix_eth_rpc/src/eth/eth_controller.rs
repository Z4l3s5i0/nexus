use async_trait::async_trait;
use wasix_eth_types::{
    rpc::{RpcBlock, RpcReceipt as RpcTransactionReceipt, RpcTransaction},
    Address, BlockId, Bytes, Filter, Log, SyncStatus, TransactionRequest, B256, U256,
};
use wasix_eth_types::error::{parse_loose_hash, RpcError, RpcResult};
use wasix_eth_types::eth::EthRpcServer;
use wasix_eth_utils::debug;
use wasix_eth_utils::metrics::RPC_REQUESTS_TOTAL;
use wasix_eth_utils::transaction_mapper::TransactionMapper;
use crate::EthService;

pub struct EthController {
    pub service: EthService,
}

#[async_trait]
impl EthRpcServer for EthController {
    async fn gas_price(&self) -> RpcResult<U256> {
        debug!("[RPC] eth_gasPrice");
        RPC_REQUESTS_TOTAL.inc();
        let price = self.service.gas_price().await?;
        debug!("[RPC] eth_gasPrice result: {}", price);
        Ok(U256::try_from(price).unwrap())
    }

    async fn accounts(&self) -> RpcResult<Vec<Address>> {
        debug!("[RPC] eth_accounts");
        RPC_REQUESTS_TOTAL.inc();
        let accounts = self.service.accounts().await?;
        debug!("[RPC] eth_accounts result count: {}", accounts.len());
        Ok(accounts)
    }

    async fn syncing(&self) -> RpcResult<SyncStatus> {
        debug!("[RPC] eth_syncing");
        RPC_REQUESTS_TOTAL.inc();
        let status = self.service.syncing().await?;
        debug!("[RPC] eth_syncing result: {:?}", status);
        Ok(status)
    }

    async fn mining(&self) -> RpcResult<bool> {
        debug!("[RPC] eth_mining");
        RPC_REQUESTS_TOTAL.inc();
        Ok(false)
    }

    async fn send_transaction(&self, request: TransactionRequest) -> RpcResult<B256> {
        debug!("[RPC] eth_sendTransaction: request={:?}", request);
        RPC_REQUESTS_TOTAL.inc();
        let hash = self.service.send_transaction(request).await?;
        debug!("[RPC] eth_sendTransaction result: {}", hash);
        Ok(hash)
    }

    async fn send_raw_transaction(&self, data: String) -> RpcResult<B256> {
        debug!("[RPC] eth_sendRawTransaction: data_len={}", data.len());
        RPC_REQUESTS_TOTAL.inc();
        let hash = self.service.send_raw_transaction(data).await?;
        debug!("[RPC] eth_sendRawTransaction result: {}", hash);
        Ok(hash)
    }

    async fn sign_transaction(&self, request: TransactionRequest) -> RpcResult<Bytes> {
        debug!("[RPC] eth_signTransaction: request={:?}", request);
        RPC_REQUESTS_TOTAL.inc();
        let signed_tx_rlp = self.service.sign_transaction(request).await?;
        debug!("[RPC] eth_signTransaction result size: {}", signed_tx_rlp.len());
        Ok(signed_tx_rlp)
    }

    async fn sign(&self, address: String, message: String) -> RpcResult<Bytes> {
        debug!("[RPC] eth_sign: address={}, message_len={}", address, message.len());
        RPC_REQUESTS_TOTAL.inc();
        let addr = TransactionMapper::parse_address(&address).map_err(|e| {
            e
        })?;
        let signature = self.service.sign(addr, message).await.map_err(|e| {
            e
        })?;
        let result = Bytes::from(signature.as_bytes().to_vec());
        debug!("[RPC] eth_sign result size: {}", result.len());
        Ok(result)
    }

    async fn call(&self, request: TransactionRequest, block_id: Option<serde_json::Value>) -> RpcResult<Bytes> {
        debug!("[RPC] eth_call: request={:?}, block_id={:?}", request, block_id);
        RPC_REQUESTS_TOTAL.inc();
        let block_id: Option<BlockId> = match block_id {
            Some(v) => Some(serde_json::from_value(v).map_err(|e| {
                RpcError::InvalidParamsCode(e.to_string())
            })?),
            None => None,
        };
        let result_bytes = self.service.call(request, block_id).await?;
        debug!("[RPC] eth_call result size: {}", result_bytes.len());
        Ok(result_bytes)
    }

    async fn estimate_gas(&self, request: TransactionRequest, block_id: Option<serde_json::Value>) -> RpcResult<U256> {
        debug!("[RPC] eth_estimateGas: request={:?}, block_id={:?}", request, block_id);
        RPC_REQUESTS_TOTAL.inc();
        let block_id: Option<BlockId> = match block_id {
            Some(v) => Some(serde_json::from_value(v).map_err(|e| {
                RpcError::InvalidParamsCode(e.to_string())
            })?),
            None => None,
        };
        let gas = self.service.estimate_gas(request, block_id).await?;
        debug!("[RPC] eth_estimateGas result: {}", gas);
        Ok(gas)
    }


    async fn get_block_by_number(&self, num: serde_json::Value, full: bool) -> RpcResult<Option<RpcBlock>> {
        debug!("[RPC] eth_getBlockByNumber: num={:?}, full={}", num, full);
        RPC_REQUESTS_TOTAL.inc();
        let num: BlockId = serde_json::from_value(num).map_err(|e| {
            RpcError::InvalidParamsCode(e.to_string())
        })?;
        let result = self.service.get_block_by_id(num, full).await.map_err(|e| {
            e
        })?;
        debug!("[RPC] eth_getBlockByNumber result: {}", if result.is_some() { "found" } else { "not found" });
        Ok(result)
    }

    async fn get_block_by_hash(&self, hash: serde_json::Value, full: bool) -> RpcResult<Option<RpcBlock>> {
        debug!("[RPC] eth_getBlockByHash: hash={:?}, full={}", hash, full);
        RPC_REQUESTS_TOTAL.inc();
        let hash: B256 = parse_loose_hash(hash).map_err(|e| {
            e
        })?;
        let result = self.service.get_block_by_id(BlockId::hash(hash), full).await.map_err(|e| {
            e.into()
        })?;
        debug!("[RPC] eth_getBlockByHash result: {}", if result.is_some() { "found" } else { "not found" });
        Ok(result)
    }

    async fn block_number(&self) -> RpcResult<U256> {
        debug!("[RPC] eth_blockNumber");
        RPC_REQUESTS_TOTAL.inc();
        let num = self.service.latest_block_number().await.map_err(|e| {
            e
        })?;
        debug!("[RPC] eth_blockNumber result: {}", num);
        Ok(U256::from(num))
    }

    async fn chain_id(&self) -> RpcResult<U256> {
        debug!("[RPC] eth_chainId");
        RPC_REQUESTS_TOTAL.inc();
        let id = self.service.chain_id().await.map_err(|e| {
            e
        })?;
        debug!("[RPC] eth_chainId result: {}", id);
        Ok(U256::from(id))
    }

    async fn get_block_transaction_count_by_number(&self, num: serde_json::Value) -> RpcResult<Option<U256>> {
        debug!("[RPC] eth_getBlockTransactionCountByNumber: num={:?}", num);
        RPC_REQUESTS_TOTAL.inc();
        let num: BlockId = serde_json::from_value(num).map_err(|e| {
            RpcError::InvalidParamsCode(e.to_string())
        })?;
        let count = self.service.get_block_transaction_count(num).await.map_err(|e| {
            e
        })?;
        debug!("[RPC] eth_getBlockTransactionCountByNumber result: {:?}", count);
        Ok(count.map(U256::from))
    }

    async fn get_block_transaction_count_by_hash(&self, hash: serde_json::Value) -> RpcResult<Option<U256>> {
        debug!("[RPC] eth_getBlockTransactionCountByHash: hash={:?}", hash);
        RPC_REQUESTS_TOTAL.inc();
        let hash: B256 = parse_loose_hash(hash).map_err(|e| {
            e
        })?;
        let count = self.service.get_block_transaction_count(BlockId::hash(hash)).await.map_err(|e| {
            e
        })?;
        debug!("[RPC] eth_getBlockTransactionCountByHash result: {:?}", count);
        Ok(count.map(U256::from))
    }

    async fn get_logs(&self, filter: Filter) -> RpcResult<Vec<Log>> {
        debug!("[RPC] eth_getLogs: filter={:?}", filter);
        RPC_REQUESTS_TOTAL.inc();
        let result = self.service.get_logs(filter).await.map_err(|e| {
            e
        })?;
        debug!("[RPC] eth_getLogs result count: {}", result.len());
        Ok(result)
    }

    async fn get_transaction_by_hash(&self, hash: serde_json::Value) -> RpcResult<Option<RpcTransaction>> {
        debug!("[RPC] eth_getTransactionByHash: hash={:?}", hash);
        RPC_REQUESTS_TOTAL.inc();
        let hash: B256 = parse_loose_hash(hash).map_err(|e| {
            e
        })?;
        let result = self.service.get_transaction_by_hash(hash).await.map_err(|e| {
            e
        })?;
        debug!("[RPC] eth_getTransactionByHash result: {}", if result.is_some() { "found" } else { "not found" });
        Ok(result)
    }

    async fn get_transaction_receipt(&self, hash: serde_json::Value) -> RpcResult<Option<RpcTransactionReceipt>> {
        debug!("[RPC] eth_getTransactionReceipt: hash={:?}", hash);
        RPC_REQUESTS_TOTAL.inc();
        let hash: B256 = parse_loose_hash(hash).map_err(|e| {
            e
        })?;
        let result = self.service.get_transaction_receipt(hash).await.map_err(|e| {
            e
        })?;
        debug!("[RPC] eth_getTransactionReceipt result: {}", if result.is_some() { "found" } else { "not found" });
        Ok(result)
    }

    async fn get_balance(&self, address: Address, block_id: Option<serde_json::Value>) -> RpcResult<U256> {
        debug!("[RPC] eth_getBalance: address={}, block_id={:?}", address, block_id);
        RPC_REQUESTS_TOTAL.inc();
        let block_id: BlockId = match block_id {
            Some(v) => serde_json::from_value(v).map_err(|e| {
                RpcError::InvalidParamsCode(e.to_string())
            })?,
            None => BlockId::latest(),
        };
        let balance = self.service.get_balance(address, block_id).await?;
        debug!("[RPC] eth_getBalance result: {}", balance);
        Ok(balance)
    }

    async fn get_transaction_count(&self, address: Address, block_id: Option<serde_json::Value>) -> RpcResult<U256> {
        debug!("[RPC] eth_getTransactionCount: address={}, block_id={:?}", address, block_id);
        RPC_REQUESTS_TOTAL.inc();
        let block_id: BlockId = match block_id {
            Some(v) => serde_json::from_value(v).map_err(|e| {
                RpcError::InvalidParamsCode(e.to_string())
            })?,
            None => BlockId::latest(),
        };
        let count = self.service.get_transaction_count(address, block_id).await?;
        debug!("[RPC] eth_getTransactionCount result: {}", count);
        Ok(U256::from(count))
    }

    async fn get_code(&self, address: Address, block_id: Option<serde_json::Value>) -> RpcResult<Bytes> {
        debug!("[RPC] eth_getCode: address={}, block_id={:?}", address, block_id);
        RPC_REQUESTS_TOTAL.inc();
        let block_id: BlockId = match block_id {
            Some(v) => serde_json::from_value(v).map_err(|e| {
                RpcError::InvalidParamsCode(e.to_string())
            })?,
            None => BlockId::latest(),
        };
        let result = self.service.get_code(address, block_id).await?;
        debug!("[RPC] eth_getCode result size: {}", result.len());
        Ok(result)
    }

    async fn get_storage_at(&self, address: Address, slot: serde_json::Value, block_id: Option<serde_json::Value>) -> RpcResult<B256> {
        debug!("[RPC] eth_getStorageAt: address={}, slot={:?}, block_id={:?}", address, slot, block_id);
        RPC_REQUESTS_TOTAL.inc();
        let slot = parse_loose_hash(slot)?;
        let block_id: BlockId = match block_id {
            Some(v) => serde_json::from_value(v).map_err(|e| {
                RpcError::InvalidParamsCode(e.to_string())
            })?,
            None => BlockId::latest(),
        };
        let value = self.service.get_storage_at(address, slot, block_id).await?;
        debug!("[RPC] eth_getStorageAt result: {}", value);
        Ok(value)
    }

    async fn get_block_receipts(&self, block_id: serde_json::Value) -> RpcResult<Option<Vec<RpcTransactionReceipt>>> {
        debug!("[RPC] eth_getBlockReceipts: block_id={:?}", block_id);
        RPC_REQUESTS_TOTAL.inc();
        let block_id: BlockId = serde_json::from_value(block_id).map_err(|e| {
            RpcError::InvalidParamsCode(e.to_string())
        })?;
        self.service.get_block_receipts(block_id).await
    }

    async fn blob_base_fee(&self) -> RpcResult<U256> {
        debug!("[RPC] eth_blobBaseFee");
        RPC_REQUESTS_TOTAL.inc();
        self.service.blob_base_fee().await
    }

    async fn max_priority_fee_per_gas(&self) -> RpcResult<U256> {
        debug!("[RPC] eth_maxPriorityFeePerGas");
        RPC_REQUESTS_TOTAL.inc();
        self.service.max_priority_fee_per_gas().await
    }
}
