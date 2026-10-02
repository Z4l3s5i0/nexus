use std::sync::Arc;
use wasix_eth_core::engine::api::RPCEngine;
use wasix_eth_types::{
    rpc::{RpcBlock, RpcReceipt as RpcTransactionReceipt, RpcTransaction},
    Address, BlockId, Bytes, Filter, Log, Signature, SyncStatus, TransactionRequest, B256, U256,
};
use wasix_eth_utils::block_mapper::BlockMapper;
use wasix_eth_utils::transaction_mapper::TransactionMapper;
use wasix_eth_utils::debug;
use wasix_eth_storage::read_traits::{ChainProvider, HeaderProvider, TransactionProvider};
use wasix_eth_types::error::{RpcError, RpcResult};

#[derive(Clone)]
pub struct EthService {
    pub engine: Arc<RPCEngine>,
}

impl EthService {
    pub async fn gas_price(&self) -> RpcResult<u128> {
        self.engine.get_gas_price().await.map_err(|e| RpcError::Internal(e.to_string()))
    }

    pub async fn accounts(&self) -> RpcResult<Vec<Address>> {
        self.engine.get_account_addresses().await.map_err(|e| RpcError::Internal(e.to_string()))
    }

    pub async fn syncing(&self) -> RpcResult<SyncStatus> {
        Ok(self.engine.chain.sync_status().await)
    }

    pub async fn send_transaction(&self, request: TransactionRequest) -> RpcResult<B256> {
        debug!("[EthService] eth_sendTransaction from: {:?}", request.from);
        self.engine.send_transaction(request).await
    }

    pub async fn call(&self, request: TransactionRequest, block_id: Option<BlockId>) -> RpcResult<Bytes> {
        debug!("[EthService] eth_call: to={:?}, data_len={}", request.to, request.input.data.as_ref().map(|d| d.len()).unwrap_or(0));
        self.engine.call(request, block_id).await
    }

    pub async fn estimate_gas(&self, request: TransactionRequest, block_id: Option<BlockId>) -> RpcResult<U256> {
        debug!("[EthService] eth_estimateGas: to={:?}, data_len={}", request.to, request.input.data.as_ref().map(|d| d.len()).unwrap_or(0));
        self.engine.estimate_gas(request, block_id).await
    }

    pub async fn send_raw_transaction(&self, data: String) -> RpcResult<B256> {
        self.engine.send_raw_transaction(data).await
    }

    pub async fn sign_transaction(&self, request: TransactionRequest) -> RpcResult<Bytes> {
        self.engine.sign_transaction(request).await
    }

    pub async fn sign(&self, address: Address, message: String) -> RpcResult<Signature> {
        self.engine.sign(address, message).await
    }

    pub async fn get_block_by_id(&self, id: BlockId, full: bool) -> RpcResult<Option<RpcBlock>> {
        let block = self.engine.get_block_by_id(id).await?;
        let (chain_config, td) = match &block {
            Some(b) => {
                let config = self.engine.read_storage.chain_config().ok().flatten().unwrap_or_default();
                let hash = b.header.hash_slow();
                let td = self.engine.read_storage.header_td(hash).ok().flatten();
                (config, td)
            }
            None => (wasix_eth_types::ChainConfig::default(), None),
        };
        Ok(block.map(|b| BlockMapper::to_rpc_block(b, full, &chain_config, td)))
    }

    pub async fn latest_block_number(&self) -> RpcResult<u64> {
        self.engine.latest_block_number().await
    }

    pub async fn chain_id(&self) -> RpcResult<u64> {
        self.engine.chain_id().await
    }

    pub async fn get_block_transaction_count(&self, id: BlockId) -> RpcResult<Option<u64>> {
        self.engine.get_block_transaction_count(id).await
    }
    
    pub async fn get_logs(&self, filter: Filter) -> RpcResult<Vec<Log>> {
        self.engine.get_logs(filter).await
    }

    pub async fn get_transaction_by_hash(&self, hash: B256) -> RpcResult<Option<RpcTransaction>> {
        let result = self.engine.get_transaction_by_hash(hash).await?;
        match result {
            Some((t, block_ref)) => {
                let header = block_ref
                    .and_then(|(_, hash, _)| self.engine.read_storage.header(BlockId::hash(hash)).ok().flatten());
                Ok(Some(TransactionMapper::to_rpc_transaction(t, block_ref, header)))
            }
            None => Ok(None),
        }
    }

    pub async fn get_transaction_receipt(&self, hash: B256) -> RpcResult<Option<RpcTransactionReceipt>> {
        let result = self.engine.get_transaction_receipt(hash).await?;
        match result {
            Some((r, block_ref)) => {
                let tx = self.engine.get_transaction_by_hash(hash).await?.map(|(t, _)| t);
                let receipt = self.map_receipt(r, block_ref, tx.as_ref(), Some(hash))?;
                Ok(Some(receipt))
            }
            None => Ok(None),
        }
    }

    fn map_receipt(&self, r: wasix_eth_types::Receipt, block_ref: Option<(u64, B256, u64)>, tx: Option<&wasix_eth_types::Transaction>, tx_hash_override: Option<B256>) -> RpcResult<RpcTransactionReceipt> {
        let (meta, gas_used, base_fee, blob_gas_price) = if let Some((_, block_hash, index)) = block_ref {
            let meta = self.engine.read_storage.receipt_meta(block_hash, index)
                .map_err(|e| RpcError::Internal(e.to_string()))?;

            let header = self.engine.read_storage.header(BlockId::hash(block_hash)).ok().flatten();
            let base_fee = header.as_ref().and_then(|h| h.base_fee_per_gas);
            let excess_blob_gas = header.as_ref().and_then(|h| h.excess_blob_gas);
            let blob_gas_price = excess_blob_gas.map(wasix_eth_types::calc_blob_gasprice);

            let gas_used = if index == 0 {
                r.receipt.cumulative_gas_used as u64
            } else {
                let prev_receipt = self.engine.read_storage.receipt(block_hash, index - 1)
                    .map_err(|e| RpcError::Internal(e.to_string()))?;
                match prev_receipt {
                    Some(prev) => (r.receipt.cumulative_gas_used - prev.receipt.cumulative_gas_used) as u64,
                    None => r.receipt.cumulative_gas_used as u64, // Should not happen
                }
            };
            (meta, gas_used, base_fee, blob_gas_price)
        } else {
            (None, r.receipt.cumulative_gas_used as u64, None, None)
        };

        let mut receipt = TransactionMapper::to_rpc_receipt(r, meta, block_ref, tx, tx_hash_override, gas_used, base_fee, blob_gas_price);
        
        // Ensure transaction hash is set
        if let Some(hash) = tx_hash_override {
            receipt.transaction_hash = hash;
        } else if let Some(t) = tx {
            receipt.transaction_hash = *t.hash();
        }

        Ok(receipt)
    }

    pub async fn get_block_receipts(&self, id: BlockId) -> RpcResult<Option<Vec<RpcTransactionReceipt>>> {
        let block = self.engine.get_block_by_id(id).await?;
        if let Some(block) = block {
            let block_hash = block.header.hash_slow();
            let block_number = block.header.number;
            let receipts = self.engine.get_receipts_by_block_id(id).await?;
            let mut rpc_receipts = Vec::new();
            for (i, r) in receipts.into_iter().enumerate() {
                let tx = block.body.transactions.get(i);
                let block_ref = Some((block_number, block_hash, i as u64));
                rpc_receipts.push(self.map_receipt(r, block_ref, tx, None)?);
            }
            Ok(Some(rpc_receipts))
        } else {
            Ok(None)
        }
    }

    pub async fn get_balance(&self, address: Address, block_id: BlockId) -> RpcResult<U256> {
        self.engine.get_balance(address, block_id).await
    }

    pub async fn get_transaction_count(&self, address: Address, block_id: BlockId) -> RpcResult<u64> {
        self.engine.get_transaction_count(address, block_id).await
    }

    pub async fn get_code(&self, address: Address, block_id: BlockId) -> RpcResult<Bytes> {
        self.engine.get_code(address, block_id).await
    }

    pub async fn get_storage_at(&self, address: Address, slot: B256, block_id: BlockId) -> RpcResult<B256> {
        self.engine.get_storage_at(address, slot, block_id).await
    }

    pub async fn blob_base_fee(&self) -> RpcResult<U256> {
        // Return 0 as default for now if not supported
        Ok(U256::ZERO)
    }

    pub async fn max_priority_fee_per_gas(&self) -> RpcResult<U256> {
        // Return 0 or some default value
        Ok(U256::ZERO)
    }
}


