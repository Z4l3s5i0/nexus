use crate::engine::canonicality_tracker::CanonicalState;
use crate::account_manager::AccountManager;
use crate::mempool::mempool_provider::MempoolProvider;
use crate::{Consensus, EngineEvent};
use alloy_rlp::{Decodable, Encodable};
use std::sync::Arc;
use tokio::sync::broadcast;
use wasix_eth_execution::execution_provider::{ExecutionProvider, TransactValueCallCreate};
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::AccountProvider;
use wasix_eth_storage::read_traits::BlockProvider;
use wasix_eth_storage::read_traits::BytecodeProvider;
use wasix_eth_storage::read_traits::ChainProvider;
use wasix_eth_storage::read_traits::HeaderProvider;
use wasix_eth_storage::read_traits::LogProvider;
use wasix_eth_storage::read_traits::StorageProvider;
use wasix_eth_storage::read_traits::TransactionProvider;
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_types::error::{RpcError, RpcResult};
use wasix_eth_types::{Address, ChainManager};
use wasix_eth_types::Block;
use wasix_eth_types::BlockId;
use wasix_eth_types::BlockNumberOrTag;
use wasix_eth_types::Bytes;
use wasix_eth_types::ChainConfig;
use wasix_eth_types::Filter;
use wasix_eth_types::FixedBytes;
use wasix_eth_types::Log;
use wasix_eth_types::Receipt;
use wasix_eth_types::Signature;
use wasix_eth_types::Signed;
use wasix_eth_types::Transaction;
use wasix_eth_types::TransactionRequest;
use wasix_eth_types::TxEip1559;
use wasix_eth_types::TxLegacy;
use wasix_eth_types::TxPooledEnvelope;
use wasix_eth_types::B256;
use wasix_eth_types::U256;
use wasix_eth_types::{hex, BlobTransactionSidecarVariant, MAX_INIT_CODE_SIZE, SignerRecoverable};
use wasix_eth_types::{ConsensusTransaction, Hardfork};
use wasix_eth_utils::warn;
use wasix_eth_utils::debug;
#[derive(Clone)]
pub struct RPCEngine {
    pub read_storage: DatabaseReadProvider,
    pub write_storage: DatabaseWriteProvider,
    pub account_manager: Arc<AccountManager>,
    pub chain: Arc<dyn ChainManager>,
    pub mempool: Arc<dyn MempoolProvider>,
    pub event_tx: broadcast::Sender<EngineEvent>,
    pub consensus: Arc<dyn Consensus>,
    pub execution: Arc<dyn ExecutionProvider>,
    pub canonical: Arc<CanonicalState>,
}

impl RPCEngine {

    pub fn new(
        read_storage: DatabaseReadProvider,
        write_storage: DatabaseWriteProvider,
        account_manager: Arc<AccountManager>,
        chain: Arc<dyn ChainManager>,
        mempool: Arc<dyn MempoolProvider>,
        event_tx: broadcast::Sender<EngineEvent>,
        consensus: Arc<dyn Consensus>,
        execution: Arc<dyn ExecutionProvider>,
        canonical: Arc<CanonicalState>,
    ) -> Self {
        Self {
            read_storage: read_storage.clone(),
            write_storage: write_storage.clone(),
            account_manager,
            chain: chain.clone(),
            mempool: mempool.clone(),
            event_tx: event_tx.clone(),
            consensus: consensus.clone(),
            execution: execution.clone(),
            canonical,
       }
    }

    pub async fn submit_transaction(&self, tx: Transaction) -> RpcResult<B256> {
        self.submit_transaction_with_source(tx, true).await
    }

    pub async fn submit_transaction_with_source(&self, tx: Transaction, is_local: bool) -> RpcResult<B256> {
        let hash = *tx.hash();
        debug!("[Engine] Submitting transaction: {:?}", hash);
        if is_local {
            wasix_eth_utils::exp!("[EXP] TX_RPC_RECV hash={:?}", hash);
        }

        // Chain ID validation
        let network_chain_id = self.read_storage.chain_id().map_err(|e| RpcError::Internal(e.to_string()))?;
        if let Some(tx_chain_id) = tx.chain_id() {
            if tx_chain_id != network_chain_id {
                warn!("[Engine] Rejected transaction {:?} due to invalid chain ID: expected {}, got {}", hash, network_chain_id, tx_chain_id);
                return Err(RpcError::InvalidParams(format!("Invalid chain ID: expected {}, got {}", network_chain_id, tx_chain_id)));
            }
        }

        // EIP-7702 validation
        if let Transaction::Eip7702(_) = tx {
            let chain_config = self.read_storage.chain_config().ok().flatten().unwrap_or_else(|| ChainConfig {
                chain_id: network_chain_id,
                ..Default::default()
            });
            let latest_header = self.read_storage.header(BlockId::Number(BlockNumberOrTag::Latest)).ok().flatten();
            if let Some(header) = latest_header {
                let current_fork = Hardfork::get_active_fork(&chain_config, header.number, header.timestamp);
                if current_fork < Hardfork::Prague {
                    warn!("[Engine] Rejected EIP-7702 transaction {:?} because Prague is not yet active", hash);
                    return Err(RpcError::InvalidParams("EIP-7702 transactions are not supported in this fork".to_string()));
                }
            }
        }

        // EIP-3860 validation
        let chain_config = self.read_storage.chain_config().ok().flatten().unwrap_or_else(|| ChainConfig {
            chain_id: network_chain_id,
            ..Default::default()
        });
        let latest_header = self.read_storage.header(BlockId::Number(BlockNumberOrTag::Latest)).ok().flatten();
        if let Some(header) = latest_header {
            let current_fork = Hardfork::get_active_fork(&chain_config, header.number, header.timestamp);
            if current_fork >= Hardfork::Shanghai {
                if tx.to().is_none() && tx.input().len() > MAX_INIT_CODE_SIZE as usize {
                    warn!("[Engine] Rejected transaction {:?} because initcode size {} exceeds maximum limit {}", hash, tx.input().len(), MAX_INIT_CODE_SIZE);
                    return Err(RpcError::InvalidParams(format!("Initcode size exceeds maximum limit ({} > {})", tx.input().len(), MAX_INIT_CODE_SIZE)));
                }
            }
        }

        // 1. Add to mempool synchronously to ensure consistency for block building
        let from = tx.recover_signer().unwrap_or_default();
        let (head_hash, _head_number) = self.canonical.get_head().await;
        
        // Robust nonce lookup: use the state nonce at the current head and sync with mempool.
        let state_nonce = self.read_storage.transaction_count(from, BlockId::Hash(head_hash.into()), None).unwrap_or_default();
        let (current_state_nonce, next_pending_nonce) = self.mempool.nonce_lookup(from, state_nonce).await;
        let expected_nonce = std::cmp::max(current_state_nonce, next_pending_nonce);
        
        // Pass state_nonce to add_transaction to allow standard behavior AND replacements.
        if self.mempool.add_transaction(tx.clone(), state_nonce).await {
            debug!("[Engine] Added transaction {:?} (nonce: {}) to mempool (is_local: {})", hash, tx.nonce(), is_local);
            // Broadcast the new transaction
            let _ = self.event_tx.send(EngineEvent::NewTransaction { tx, is_local });
        } else {
            if is_local {
                // Fetch the latest state nonce again to check if the transaction was just included
                let latest_state_nonce = self.read_storage.transaction_count(from, BlockId::Hash(head_hash.into()), None).unwrap_or(state_nonce);
                if tx.nonce() < latest_state_nonce {
                     warn!("[Engine] Rejected local transaction {:?} because it was already included (nonce: {}, state_nonce: {})", hash, tx.nonce(), latest_state_nonce);
                     return Err(RpcError::InvalidParams(format!("Nonce too low: expected {}, got {}", latest_state_nonce, tx.nonce())));
                }

                warn!("[Engine] Rejected local transaction {:?} due to nonce collision or stale nonce (nonce: {}, state_nonce: {}, next_pending: {})", hash, tx.nonce(), state_nonce, next_pending_nonce);

                // If the mempool is stale, trigger a revalidation for this sender
                if next_pending_nonce < state_nonce {
                    let mempool = self.mempool.clone();
                    let read_storage = self.read_storage.clone();
                    tokio::spawn(async move {
                        mempool.revalidate(&read_storage, Some(vec![from])).await;
                    });
                }

                return Err(RpcError::InvalidParams(format!("Nonce too low: expected {}, got {}", expected_nonce, tx.nonce())));
            } else {
                debug!("[Engine] Ignored remote transaction {:?} due to nonce collision or stale nonce", hash);
            }
        }

        Ok(hash)
    }

    pub async fn get_account_addresses(&self) -> RpcResult<Vec<Address>> {
        self.read_storage.addresses().map_err(|e| RpcError::Internal(e.to_string()))
    }

    pub async fn get_gas_price(&self) -> RpcResult<u128> {
        if let Ok(Some(header)) = self.read_storage.header(BlockId::Number(BlockNumberOrTag::Latest)) {
            if let Some(base_fee) = header.base_fee_per_gas {
                // Return base_fee + 1.5 Gwei priority fee as a reasonable default
                let priority_fee = 1_500_000_000u64;
                return Ok(u128::try_from(U256::from(base_fee + priority_fee)).map_err(|e| RpcError::Internal(e.to_string()))?);
            }
        }
        // Fallback to 1 Gwei if no block data found
        Ok(1_000_000_000u128)
    }

    pub async fn estimate_gas(&self, request: TransactionRequest, block_id: Option<BlockId>) -> RpcResult<U256> {
        let block_id = block_id.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));
        let header = self.read_storage.header(block_id)
            .map_err(|e| RpcError::Internal(e.to_string()))?
            .ok_or(RpcError::BlockNotFound(block_id))?;
        let block = self.read_storage.block(block_id)
            .map_err(|e| RpcError::Internal(e.to_string()))?
            .ok_or(RpcError::BlockNotFound(block_id))?;

        let signature = Signature::test_signature();
        let hash = B256::ZERO;

        let tx_envelope = self.build_transaction_envelope(request, signature, hash)
            .map_err(|e| RpcError::Internal(format!("Failed to build transaction envelope: {:?}", e)))?;

        let result = self.execution.run_simulation_with_state_root(vec![tx_envelope], block, Some(header.state_root)).map(|v| v.0.into_iter().next().unwrap())
            .map_err(|e| RpcError::Internal(e.to_string()))?;
        Ok(U256::from(result.gas_used))
    }


    pub async fn call(&self, request: TransactionRequest, block_id: Option<BlockId>) -> RpcResult<Bytes> {
        let block_id = block_id.unwrap_or(BlockId::Number(BlockNumberOrTag::Latest));
        let header = self.read_storage.header(block_id)
            .map_err(|e| RpcError::Internal(e.to_string()))?
            .ok_or(RpcError::BlockNotFound(block_id))?;
        let block = self.read_storage.block(block_id)
            .map_err(|e| RpcError::Internal(e.to_string()))?
            .ok_or(RpcError::BlockNotFound(block_id))?;

        // Use a test signature for simulation since it won't be verified for `call`
        let signature = Signature::test_signature();
        let hash = B256::ZERO;

        let tx_envelope = self.build_transaction_envelope(request, signature, hash)
            .map_err(|e| RpcError::Internal(format!("Failed to build transaction envelope: {:?}", e)))?;

        let result = self.execution.run_simulation_with_state_root(vec![tx_envelope], block, Some(header.state_root))
            .map(|v| v.0.into_iter().next().unwrap()).map_err(|e| RpcError::Internal(e.to_string()))?;

        match result.call_create {
            TransactValueCallCreate::Call { retval, .. } => Ok(retval.into()),
            TransactValueCallCreate::Create { .. } => Ok(Bytes::new()),
        }
    }
    pub async fn send_transaction(&self, request: TransactionRequest) -> RpcResult<B256> {
        let from = request.from.ok_or_else(|| RpcError::InvalidParams("from address is required".to_string()))?;
        if !self.account_manager.is_managed(&from) {
            return Err(RpcError::AccountNotFound(from));
        }

        let chain_id = self.read_storage.chain_id().map_err(|e| RpcError::Internal(e.to_string()))?;
        let nonce = if let Some(n) = request.nonce {
            n
        } else {
            self.get_transaction_count(from, BlockId::Number(BlockNumberOrTag::Pending)).await?
        };

        // For eth_sendTransaction, we should handle gas and gas_price properly
        let gas_limit = if let Some(g) = request.gas {
            g
        } else {
            // For simple transfers use 21000, else estimate
            if request.to.is_some() && request.input.data.is_none() {
                21000
            } else {
                self.estimate_gas(request.clone(), None).await?.to::<u64>()
            }
        };

        let tx = if let (Some(max_fee), Some(max_priority)) = (request.max_fee_per_gas, request.max_priority_fee_per_gas) {
            // EIP-1559
            let tx_1559 = TxEip1559 {
                chain_id,
                nonce,
                gas_limit,
                max_fee_per_gas: max_fee,
                max_priority_fee_per_gas: max_priority,
                to: request.to.unwrap_or_default().into(),
                value: request.value.unwrap_or_default(),
                input: request.input.data.clone().unwrap_or_default(),
                access_list: request.access_list.clone().unwrap_or_default(),
            };
            let signed_tx: Transaction = self.account_manager.sign_transaction_1559(&from, tx_1559).await
                .map_err(|e| RpcError::Internal(format!("Signing failed: {:?}", e)))?;
            signed_tx
        } else {
            // Legacy
            let gas_price = if let Some(p) = request.gas_price {
                p
            } else {
                self.get_gas_price().await?
            };
            let tx_legacy = TxLegacy {
                chain_id: Some(chain_id),
                nonce,
                gas_price,
                gas_limit,
                to: request.to.unwrap_or_default().into(),
                value: request.value.unwrap_or_default(),
                input: request.input.data.clone().unwrap_or_default(),
            };
            let signed_tx: Transaction = self.account_manager.sign_transaction(&from, tx_legacy).await
                .map_err(|e| RpcError::Internal(format!("Signing failed: {:?}", e)))?;
            signed_tx
        };

        let hash = tx.hash().clone();
        self.submit_transaction(tx).await?;

        Ok(hash)
    }

    pub async fn send_raw_transaction(&self, data: String) -> RpcResult<B256> {
        let data = hex::decode(data.trim_start_matches("0x"))
            .map_err(|e| RpcError::InvalidParams(format!("Invalid hex: {}", e)))?;

        // Try decoding as TxPooledEnvelope to handle 4844 transactions with sidecars (network format)
        let pooled_tx: TxPooledEnvelope = Decodable::decode(&mut &data[..])
            .map_err(|e| RpcError::InvalidParams(format!("Invalid RLP: {}", e)))?;

        self.import_pooled_transaction(pooled_tx, data).await
    }

    pub async fn import_pooled_transaction(&self, pooled_tx: TxPooledEnvelope, data: Vec<u8>) -> RpcResult<B256> {
        self.import_pooled_transaction_with_source(pooled_tx, data, true).await
    }

    pub async fn import_pooled_transaction_with_source(&self, pooled_tx: TxPooledEnvelope, data: Vec<u8>, is_local: bool) -> RpcResult<B256> {
        let hash = *pooled_tx.hash();
        debug!("[Engine] Importing pooled transaction: {:?}", hash);

        // Store pooled envelope for re-serving over P2P (structured path)
        self.mempool.add_pooled_envelope(hash, pooled_tx.clone()).await;

        self.mempool.add_pooled_bytes(hash, Bytes::copy_from_slice(&data)).await;

        // Store blobs if present (EIP-4844)
        if let TxPooledEnvelope::Eip4844(signed_tx_sidecar) = &pooled_tx {
            let inner_tx = signed_tx_sidecar.tx();
            let sidecar = &inner_tx.sidecar;

            for (i, versioned_hash) in inner_tx.blob_versioned_hashes().unwrap_or_default().iter().enumerate() {
                if let (Some(blob), Some(commitment)) = (
                    sidecar.blobs.get(i),
                    sidecar.commitments.get(i)
                ) {
                    debug!("[Engine] Adding blob for versioned hash: {:?}, blob len: {}", versioned_hash, blob.len());
                    // use cell_proofs as proofs if compatible or handle accordingly
                    let proof = sidecar.cell_proofs.get(i).cloned().unwrap_or_default();
                    self.mempool.add_blob(*versioned_hash, (*blob).clone(), *commitment, proof).await;
                }
            }
        }

        let mut signed_tx: Transaction = match pooled_tx.clone() {
            TxPooledEnvelope::Legacy(tx) => Transaction::Legacy(tx),
            TxPooledEnvelope::Eip2930(tx) => Transaction::Eip2930(tx),
            TxPooledEnvelope::Eip1559(tx) => Transaction::Eip1559(tx),
            TxPooledEnvelope::Eip4844(tx) => {
                let inner_tx = tx.tx();
                wasix_eth_types::Transaction::Eip4844(Signed::new_unchecked(
                    wasix_eth_types::TxEip4844Variant::TxEip4844(inner_tx.tx.clone()),
                    *tx.signature(),
                    *tx.hash(),
                ))
            }
            TxPooledEnvelope::Eip7702(tx) => Transaction::Eip7702(tx),
        };
        self.submit_transaction_with_source(signed_tx, is_local).await
    }

    pub async fn sign_transaction(&self, request: TransactionRequest) -> RpcResult<Bytes> {
        let from = request.from.ok_or_else(|| RpcError::InvalidParams("from address is required".to_string()))?;

        if !self.account_manager.is_managed(&from) {
            return Err(RpcError::AccountNotFound(from));
        }

        let mut request = request;
        if request.nonce.is_none() {
            request.nonce = Some(self.get_transaction_count(from, BlockId::Number(BlockNumberOrTag::Pending)).await?);
        }

        let signed_tx: Transaction = self.build_transaction_envelope(request, Signature::test_signature(), B256::ZERO)
            .map_err(|e| RpcError::Internal(format!("Failed to build transaction envelope: {:?}", e)))?;

        let mut rlp_data = Vec::new();
        signed_tx.encode(&mut rlp_data);

        Ok(Bytes::from(rlp_data))
    }

    pub async fn sign(&self, address: Address, message: String) -> RpcResult<Signature> {
        if !self.account_manager.is_managed(&address) {
            return Err(RpcError::AccountNotFound(address));
        }

        let message = hex::decode(message.trim_start_matches("0x"))
            .map_err(|e| RpcError::InvalidParams(format!("Invalid hex: {}", e)))?;

        self.account_manager.sign(&address, &message).await
    }

    pub async fn get_block_by_id(&self, id: BlockId) -> RpcResult<Option<Block<Transaction>>> {
        let block = self.read_storage.block(id).map_err(|e| {
            e.downcast_ref::<RpcError>().cloned().unwrap_or_else(|| RpcError::Internal(e.to_string()))
        })?;

        if block.is_none() {
            if let BlockId::Hash(rpc_hash) = id {
                let hash = rpc_hash.block_hash;
                if self.chain.is_invalid(hash).await {
                    return Ok(None);
                }
                if let Some((payload_block, _, _, _)) = self.read_storage.get_payload_by_block_hash(hash) {
                    return Ok(Some(payload_block));
                }
            }
        }

        Ok(block)
    }

    pub async fn latest_block_number(&self) -> RpcResult<u64> {
        let (_, number) = self.chain.head_block().await;
        Ok(number)
    }

    pub async fn chain_id(&self) -> RpcResult<u64> {
        self.read_storage.chain_id().map_err(|e| RpcError::Internal(e.to_string()))
    }

    pub async fn get_block_transaction_count(&self, id: BlockId) -> RpcResult<Option<u64>> {
        let block = self.read_storage.block(id).map_err(|e| {
            e.downcast_ref::<RpcError>().cloned().unwrap_or_else(|| RpcError::Internal(e.to_string()))
        })?;
        Ok(block.map(|b| b.body.transactions.len() as u64))
    }

    pub async fn get_logs(&self, filter: Filter) -> RpcResult<Vec<Log>> {
        self.read_storage.logs(filter).map_err(|e| RpcError::Internal(e.to_string()))
    }

    pub async fn get_transaction_by_hash(&self, hash: B256) -> RpcResult<Option<(Transaction, Option<(u64, B256, u64)>)>> {
        let tx = self.read_storage.transaction(hash).map_err(|e| RpcError::Internal(e.to_string()))?;
        if let Some(t) = tx {
            let block_ref = self.read_storage.transaction_block_reference(hash).map_err(|e| RpcError::Internal(e.to_string()))?;
            return Ok(Some((t, block_ref)));
        }

        // Check mempool
        if let Some(tx) = self.mempool.get_transaction(hash).await {
            return Ok(Some((tx, None)));
        }

        Ok(None)
    }

    pub async fn get_transaction_receipt(&self, hash: B256) -> RpcResult<Option<(Receipt, Option<(u64, B256, u64)>)>> {
        let block_ref = self.read_storage.transaction_block_reference(hash).map_err(|e| RpcError::Internal(e.to_string()))?;
        if let Some(br) = block_ref {
            let receipt = self.read_storage.receipt(br.1, br.2).map_err(|e| RpcError::Internal(e.to_string()))?;
            return Ok(receipt.map(|r| (r, Some(br))));
        }
        Ok(None)
    }

    pub async fn get_receipts_by_block_id(&self, id: BlockId) -> RpcResult<Vec<Receipt>> {
        let block = self.get_block_by_id(id).await?
            .ok_or_else(|| RpcError::BlockNotFound(id))?;

        if block.header.number == 0 {
            return Ok(Vec::new());
        }

        let mut receipts = Vec::new();
        for tx in block.body.transactions {
            let receipt = self.read_storage.transaction_receipt(*tx.hash())
                .map_err(|e| RpcError::Internal(e.to_string()))?
                .ok_or_else(|| RpcError::Internal(format!("Receipt not found for transaction {}", tx.hash())))?;
            receipts.push(receipt);
        }

        Ok(receipts)
    }

    pub async fn get_balance(&self, address: Address, block_id: BlockId) -> RpcResult<U256> {
        let is_pending = matches!(block_id, BlockId::Number(BlockNumberOrTag::Pending));

        let header = self.read_storage.header(block_id)
            .map_err(|e| {
                e.downcast_ref::<RpcError>().cloned().unwrap_or_else(|| RpcError::Internal(e.to_string()))
            })?;
        let state_root = header.map(|h| h.state_root);

        let account = self.read_storage.account(address, state_root)
            .map_err(|e| RpcError::Internal(e.to_string()))?;

        let mut balance = account.map(|a| a.balance).unwrap_or_default();

        if is_pending {
            let mempool_txs = self.mempool.get_transactions_by_sender(address).await;
            for tx in mempool_txs {
                let cost = tx.gas_limit() as u128 * tx.max_fee_per_gas() + tx.value().to::<u128>();
                let cost_u256 = U256::from(cost);
                if balance > cost_u256 {
                    balance -= cost_u256;
                } else {
                    balance = U256::ZERO;
                }
            }
        }

        Ok(balance)
    }

    pub async fn get_transaction_count(&self, address: Address, block_id: BlockId) -> RpcResult<u64> {
        let is_pending = matches!(block_id, BlockId::Number(BlockNumberOrTag::Pending));
        
        let header = self.read_storage.header(block_id)
            .map_err(|e| {
                e.downcast_ref::<RpcError>().cloned().unwrap_or_else(|| RpcError::Internal(e.to_string()))
            })?;
        let state_root = header.map(|h| h.state_root);

        let count = self.read_storage.transaction_count(address, block_id, state_root).map_err(|e| RpcError::Internal(e.to_string()))?;

        if is_pending {
            Ok(self.mempool.next_expected_nonce(address, count).await)
        } else {
            Ok(count)
        }
    }

    pub async fn get_code(&self, address: Address, block_id: BlockId) -> RpcResult<Bytes> {
        let header = self.read_storage.header(block_id)
            .map_err(|e| {
                e.downcast_ref::<RpcError>().cloned().unwrap_or_else(|| RpcError::Internal(e.to_string()))
            })?;
        let state_root = header.map(|h| h.state_root);

        let account = self.read_storage.account(address, state_root).map_err(|e| RpcError::Internal(e.to_string()))?;
        if let Some(account) = account {
            let code = self.read_storage.bytecode(account.code_hash).map_err(|e| RpcError::Internal(e.to_string()))?;
            return Ok(code.unwrap_or_default());
        }
        Ok(Bytes::default())
    }

    pub async fn get_storage_at(&self, address: Address, slot: B256, block_id: BlockId) -> RpcResult<B256> {
        let header = self.read_storage.header(block_id)
            .map_err(|e| {
                e.downcast_ref::<RpcError>().cloned().unwrap_or_else(|| RpcError::Internal(e.to_string()))
            })?;
        let state_root = header.map(|h| h.state_root);

        let value = self.read_storage.storage(address, slot, state_root).map_err(|e| RpcError::Internal(e.to_string()))?;
        Ok(B256::from(value.to_be_bytes()))
    }
    fn build_transaction_envelope(&self, request: TransactionRequest, signature: Signature, hash: FixedBytes<32>) -> Result<Transaction, RpcError> {
        let tx_envelope = if let (Some(max_fee), Some(max_priority)) = (request.max_fee_per_gas, request.max_priority_fee_per_gas) {
            let tx = TxEip1559 {
                chain_id: self.read_storage.chain_id().map_err(|e| RpcError::Internal(e.to_string()))?,
                nonce: request.nonce.unwrap_or_default(),
                gas_limit: request.gas.unwrap_or(30_000_000), // Use block gas limit or a high enough value
                max_fee_per_gas: max_fee,
                max_priority_fee_per_gas: max_priority,
                to: request.to.unwrap_or_default().into(),
                value: request.value.unwrap_or_default(),
                input: request.input.data.clone().unwrap_or_default(),
                access_list: request.access_list.clone().unwrap_or_default(),
            };
            Transaction::Eip1559(Signed::new_unchecked(tx, signature, hash))
        } else {
            let tx = TxLegacy {
                chain_id: Some(self.read_storage.chain_id().map_err(|e| RpcError::Internal(e.to_string()))?),
                nonce: request.nonce.unwrap_or_default(),
                gas_price: request.gas_price.unwrap_or(1_000_000_000u128),
                gas_limit: request.gas.unwrap_or(30_000_000),
                to: request.to.unwrap_or_default().into(),
                value: request.value.unwrap_or_default(),
                input: request.input.data.clone().unwrap_or_default(),
            };
            Transaction::Legacy(Signed::new_unchecked(tx, signature, hash))
        };
        Ok(tx_envelope)
    }
}