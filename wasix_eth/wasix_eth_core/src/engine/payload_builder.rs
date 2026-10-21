use crate::mempool::mempool_provider::MempoolProvider;
use crate::{Consensus, EngineEvent};
use std::sync::Arc;
use tokio::sync::{broadcast, Notify};
use wasix_eth_execution::execution_provider::ExecutionProvider;
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::{BlockProvider, ChainProvider};
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_storage::write_traits::BlockWriter;
use wasix_eth_types::error::{RpcError, RpcResult};
use wasix_eth_types::Block;
use wasix_eth_types::ChainConfig;
use wasix_eth_types::PayloadAttributes;
use wasix_eth_types::PayloadId;
use wasix_eth_types::PayloadStatus;
use wasix_eth_types::PayloadStatusEnum;
use wasix_eth_types::Receipt;
use wasix_eth_types::Transaction;
use wasix_eth_types::B256;
use wasix_eth_types::U256;
use wasix_eth_types::BlobsBundleV1;
use wasix_eth_types::ConsensusTransaction;
use wasix_eth_utils::{debug, info, error, warn};
use wasix_eth_types::Result;

use std::collections::HashMap;
use std::sync::RwLock;

// Simple CancellationToken since we don't have tokio-util
#[derive(Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub async fn cancelled(&self) {
        while !self.is_cancelled() {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStatus {
    /// Initial empty block prepared, first non-empty build in progress.
    InitialBuildInProgress,
    /// First non-empty build (or best possible one) has been completed.
    InitialBuildCompleted,
}

#[derive(Clone)]
pub struct PayloadBuilder {
    pub consensus: Arc<dyn Consensus>,
    pub read_storage: DatabaseReadProvider,
    pub write_storage: DatabaseWriteProvider,
    pub execution: Arc<dyn ExecutionProvider>,
    pub mempool: Arc<dyn MempoolProvider>,
    pub event_tx: broadcast::Sender<EngineEvent>,
    pub build_lock: Arc<tokio::sync::Mutex<()>>,
    pub cancel_tokens: Arc<RwLock<HashMap<PayloadId, CancellationToken>>>,
    /// Tracks the build status of payloads.
    pub build_statuses: Arc<RwLock<HashMap<PayloadId, BuildStatus>>>,
    /// Notifies waiters when a payload's build status changes.
    pub build_notify: Arc<RwLock<HashMap<PayloadId, Arc<Notify>>>>,
}

impl PayloadBuilder {
    pub async fn generate_payload_id(&self, head_block_hash: &B256, attr: &PayloadAttributes) -> PayloadId {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        head_block_hash.hash(&mut hasher);
        attr.timestamp.hash(&mut hasher);
        attr.prev_randao.hash(&mut hasher);
        attr.suggested_fee_recipient.hash(&mut hasher);
        if let Some(withdrawals) = &attr.withdrawals {
            withdrawals.hash(&mut hasher);
        }
        if let Some(parent_beacon_block_root) = &attr.parent_beacon_block_root {
            parent_beacon_block_root.hash(&mut hasher);
        }
        let id_bytes = hasher.finish().to_be_bytes();
        PayloadId::new(id_bytes)
    }

    pub async fn build_new_payload(
        &self,
        head_block_hash: B256,
        attr: PayloadAttributes,
        status: &PayloadStatus,
    ) -> RpcResult<Option<PayloadId>> {
        if status.status == PayloadStatusEnum::Syncing {
            info!("[PayloadBuilder] Cannot build payload: head block missing (status is Syncing)");
            return Ok(None);
        }

        let id = self.generate_payload_id(&head_block_hash, &attr).await;

        // 1. Initial check without lock
        if self.read_storage.get_payload(&id).is_some() {
            return Ok(Some(id));
        }

        // 2. Synchronize building initialization
        let _guard = self.build_lock.lock().await;

        // 3. Re-check after acquiring lock
        if self.read_storage.get_payload(&id).is_some() {
            return Ok(Some(id));
        }

        // Validate attributes
        let parent_block = self.read_storage.block_by_hash(head_block_hash).map_err(|e| RpcError::Internal(e.to_string()))?
            .or_else(|| {
                // Fallback to payloads map if head is not in storage yet
                self.read_storage.get_payload_by_block_hash(head_block_hash)
                    .map(|(b, _, _, _)| b)
            })
            .ok_or_else(|| RpcError::InvalidForkchoiceState(format!("Head block not found for payload building: {:?}", head_block_hash)))?;

        if attr.timestamp <= parent_block.header.timestamp {
            return Err(RpcError::InvalidPayloadAttributes("Invalid timestamp".to_string()));
        }

        // 4. Build initial empty payload
        self.build_empty_payload(head_block_hash, attr.clone(), id.clone()).await?;

        // Initialize build status and notify
        self.build_statuses.write().map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?.insert(id.clone(), BuildStatus::InitialBuildInProgress);
        self.build_notify.write().map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?.insert(id.clone(), Arc::new(Notify::new()));

        // 5. Perform immediate synchronous rebuild
        let _ = self.maybe_rebuild_payload(id.clone()).await;

        // 6. Start continuous building background task
        let builder = self.clone();
        let token = CancellationToken::new();
        {
            let mut tokens = self.cancel_tokens.write().map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?;
            if let Some(old_token) = tokens.insert(id.clone(), token.clone()) {
                old_token.cancel();
            }
        }

        let mut event_rx = self.event_tx.subscribe();
        tokio::spawn(async move {
            let start_time = std::time::Instant::now();
            let mut last_rebuild_time = std::time::Instant::now();
            let slot_duration = std::time::Duration::from_millis(10500); // SLOT_DURATION_MS
            
            while !token.is_cancelled() {
                tokio::task::yield_now().await;
                if start_time.elapsed() >= slot_duration {
                    info!("[PayloadBuilder] Stopping continuous building for {:?}: time limit reached", id);
                    break;
                }

                // Wait for a bit or until mempool change
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {},
                    Ok(EngineEvent::NewTransaction { .. }) = event_rx.recv() => {
                        // Debounce: wait a bit for more transactions to arrive
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        // Drain extra events
                        let mut count = 1;
                        while let Ok(_) = event_rx.try_recv() {
                            count += 1;
                        }
                        debug!("[PayloadBuilder] Triggering reactive rebuild for {:?} due to {} new transaction(s)", id, count);
                    },
                    _ = token.cancelled() => break,
                }

                // Minimum 500ms between rebuilds to avoid CPU saturation under high TPS
                let elapsed_since_last = last_rebuild_time.elapsed();
                if elapsed_since_last < std::time::Duration::from_millis(500) {
                    tokio::time::sleep(std::time::Duration::from_millis(500) - elapsed_since_last).await;
                }

                last_rebuild_time = std::time::Instant::now();
                if let Err(e) = builder.maybe_rebuild_payload(id.clone()).await {
                    error!("[PayloadBuilder] Error during continuous building for {:?}: {:?}", id, e);
                }
            }
            
            let mut tokens = builder.cancel_tokens.write().unwrap();
            if let Some(t) = tokens.get(&id) {
                if t.is_cancelled() == token.is_cancelled() {
                    tokens.remove(&id);
                }
            }
        });

        Ok(Some(id))
    }

    async fn build_empty_payload(
        &self,
        head_block_hash: B256,
        attr: PayloadAttributes,
        id: PayloadId,
    ) -> RpcResult<()> {
        let parent_block = self.read_storage.block_by_hash(head_block_hash).map_err(|e| RpcError::Internal(e.to_string()))?
            .or_else(|| {
                self.read_storage.get_payload_by_block_hash(head_block_hash)
                    .map(|(b, _, _, _)| b)
            })
            .ok_or_else(|| RpcError::InvalidForkchoiceState(format!("Head block not found for empty payload building: {:?}", head_block_hash)))?;

        let chain_config = self.read_storage.chain_config().ok().flatten().unwrap_or_else(|| ChainConfig {
            chain_id: self.read_storage.chain_id().unwrap_or(1),
            ..Default::default()
        });
        let base_fee_per_gas = self.consensus.calculate_next_base_fee(&parent_block.header, &chain_config);

        let execution = Arc::clone(&self.execution);
        let parent_header_clone = parent_block.header.clone();
        let attr_clone = attr.clone();
        
        let (finalized_block, receipts, metas) = match tokio::task::spawn_blocking(move || {
            execution.execute_block_for_payload(
                vec![], // Empty transaction set
                &parent_header_clone,
                &attr_clone,
                base_fee_per_gas,
            )
        }).await {
            Ok(Ok(res)) => res,
            Ok(Err(e)) => {
                warn!("[PayloadBuilder] Execution failed for empty payload building: {}", e);
                return Err(RpcError::Internal(e.to_string()));
            }
            Err(e) => {
                error!("[PayloadBuilder] Empty payload building task panicked: {}", e);
                return Err(RpcError::Internal(e.to_string()));
            }
        };

        let bundle = BlobsBundleV1::default();
        let write_storage = self.write_storage.clone();
        let finalized_block_clone = finalized_block.clone();
        let receipts_clone = receipts.clone();
        let metas_clone = metas.clone();
        let id_clone = id.clone();

        match tokio::task::spawn_blocking(move || {
            write_storage.add_payload(id_clone, finalized_block_clone, receipts_clone, metas_clone, bundle)
        }).await {
            Ok(Ok(_)) => {},
            Ok(Err(e)) => {
                error!("[PayloadBuilder] Failed to persist empty payload: {}", e);
                return Err(RpcError::Internal(e.to_string()));
            }
            Err(e) => {
                error!("[PayloadBuilder] Empty payload persistence task panicked: {}", e);
                return Err(RpcError::Internal(e.to_string()));
            }
        }

        info!("[PayloadBuilder] Created initial empty payload_id={:?} for block_number={}", id, parent_block.header.number + 1);
        Ok(())
    }

    pub async fn maybe_rebuild_payload(
        &self,
        payload_id: PayloadId,
    ) -> Result<()> {
        let (payload_block, payload_receipts, _payload_metas, _payload_bundle) = match self.read_storage.get_payload(&payload_id) {
            Some(p) => p,
            None => return Ok(()),
        };

        let head_hash = payload_block.header.parent_hash;
        let old_value = self.calculate_block_value(&payload_block, &payload_receipts);

        let attr = PayloadAttributes {
                timestamp: payload_block.header.timestamp,
                prev_randao: payload_block.header.mix_hash,
                suggested_fee_recipient: payload_block.header.beneficiary,
                withdrawals: payload_block.body.withdrawals.as_ref().map(|w| w.0.clone()),
                parent_beacon_block_root: payload_block.header.parent_beacon_block_root,
                slot_number: None,
                target_gas_limit: None,
        };

        // 1. Validate attributes (check if parent block still exists)
        let parent_block = self.read_storage.block_by_hash(head_hash).map_err(|e| RpcError::Internal(e.to_string()))?
            .or_else(|| {
                self.read_storage.get_payload_by_block_hash(head_hash)
                    .map(|(b, _, _, _)| b)
            });
        
        let parent_block = match parent_block {
            Some(b) => b,
            None => {
                info!("[PayloadBuilder] Cannot rebuild payload {:?}: head block not found", payload_id);
                return Ok(());
            }
        };

        // 2. Compute base fees and max blobs
        let chain_config = self.read_storage.chain_config().ok().flatten().unwrap_or_else(|| ChainConfig {
            chain_id: self.read_storage.chain_id().unwrap_or(1),
            ..Default::default()
        });
        let base_fee_per_gas = self.consensus.calculate_next_base_fee(&parent_block.header, &chain_config);
        let blob_base_fee = self.consensus.calculate_next_blob_base_fee(&parent_block.header, &chain_config);

        let active_fork = wasix_eth_types::Hardfork::get_active_fork(&chain_config, parent_block.header.number + 1, attr.timestamp);
        let max_blobs_per_block = active_fork.blob_params(&chain_config).map(|p| p.max_blob_count as u32);

        // 3. Take a mempool snapshot and peek best transactions
        let snapshot = self.mempool.snapshot();
        let transactions = snapshot.peek_best_transactions(
            parent_block.header.gas_limit, 
            U256::from(base_fee_per_gas.unwrap_or_default()),
            blob_base_fee.map(U256::from),
            max_blobs_per_block
        );

        // Optimization: if transaction set is identical, skip rebuilding
        if transactions.len() == payload_block.body.transactions.len() {
            let mut identical = true;
            for (new_tx, old_tx) in transactions.iter().zip(payload_block.body.transactions.iter()) {
                if new_tx.hash() != old_tx.hash() {
                    identical = false;
                    break;
                }
            }
            if identical {
                return Ok(());
            }
        }

        if transactions.is_empty() && !payload_block.body.transactions.is_empty() {
             // Should not happen if we already have a payload with transactions, but good to be safe.
             return Ok(());
        }

        tokio::task::yield_now().await;

        let write_storage = self.write_storage.clone();
        // 4. Re-run execution
        let execution = Arc::clone(&self.execution);
        let parent_header_clone = parent_block.header.clone();
        let attr_clone = attr.clone();
        let (finalized_block, receipts, metas) = tokio::task::spawn_blocking(move || {
            execution.execute_block_for_payload(
                transactions,
                &parent_header_clone,
                &attr_clone,
                base_fee_per_gas,
            )
        }).await.map_err(|e| RpcError::Internal(format!("Payload rebuilding task panicked: {}", e)))?
        .map_err(|e| RpcError::Internal(e.to_string()))?;

        // 5. Check if new block is better
        let new_value = self.calculate_block_value(&finalized_block, &receipts);
        
        // If the new one has higher value, replace it.
        // If values are equal, but new one has more transactions, prefer it to maximize throughput.
        let should_replace = if new_value > old_value {
            true
        } else if new_value == old_value {
            finalized_block.body.transactions.len() >= payload_block.body.transactions.len()
        } else {
            false
        };

        if !should_replace && !payload_block.body.transactions.is_empty() {
            debug!("[PayloadBuilder] NOT rebuilding payload {:?}: new value {} is not better than old value {} (tx count: {} vs {})", 
                payload_id, new_value, old_value, finalized_block.body.transactions.len(), payload_block.body.transactions.len());
            return Ok(());
        }

        info!("[PayloadBuilder] Rebuilding payload {:?}: value {} -> {} (tx count: {} -> {})", 
            payload_id, old_value, new_value, payload_block.body.transactions.len(), finalized_block.body.transactions.len());

        // 6. Collect blobs
        let mut bundle = BlobsBundleV1::default();
        for tx in &finalized_block.body.transactions {
            if let Transaction::Eip4844(signed_tx) = tx {
                if let Some(hashes) = signed_tx.tx().blob_versioned_hashes() {
                    for hash in hashes {
                        if let Some((blob, commitment, proof)) = self.mempool.get_blob(*hash).await {
                            bundle.blobs.push(blob);
                            bundle.commitments.push(commitment);
                            bundle.proofs.push(proof);
                        } else {
                            return Err(RpcError::Internal(format!("Missing blob for transaction included in rebuilt block: {:?}", signed_tx.hash())).into());
                        }
                    }
                }
            }
        }

        let finalized_block_clone = finalized_block.clone();
        let receipts_clone = receipts.clone();
        let metas_clone = metas.clone();
        tokio::task::spawn_blocking(move || {
            write_storage.add_payload(payload_id, finalized_block_clone, receipts_clone, metas_clone, bundle)
        }).await.map_err(|e| RpcError::Internal(format!("Payload rebuilding task panicked: {}", e)))?
        .map_err(|e| RpcError::Internal(e.to_string()))?;

        // Update build status and notify waiters
        self.build_statuses.write().map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?.insert(payload_id, BuildStatus::InitialBuildCompleted);
        if let Some(notify) = self.build_notify.read().map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?.get(&payload_id) {
            notify.notify_waiters();
        }

        Ok(())
    }

    pub async fn get_payload(&self, payload_id: &PayloadId) -> RpcResult<(Block<Transaction>, Vec<Receipt>, Vec<wasix_eth_types::ReceiptMeta>, BlobsBundleV1)> {
        // Wait for initial build to complete if it's still in progress
        let notify = {
            let statuses = self.build_statuses.read().map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?;
            if let Some(status) = statuses.get(payload_id) {
                if *status == BuildStatus::InitialBuildInProgress {
                    self.build_notify.read().map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?.get(payload_id).cloned()
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(notify) = notify {
            debug!("[PayloadBuilder] get_payload called while initial build in progress for {:?}, waiting...", payload_id);
            let timeout = std::time::Duration::from_secs(4);
            let _ = tokio::time::timeout(timeout, notify.notified()).await;
        }

        let payload = self.read_storage.get_payload(payload_id)
            .ok_or_else(|| RpcError::BlockNotFound(wasix_eth_types::BlockId::Hash(B256::from_slice(&payload_id.0[..]).into())))?;
        
        info!("[PayloadBuilder] Returning payload {:?} with {} transactions", payload_id, payload.0.body.transactions.len());
        Ok(payload)
    }

    pub fn calculate_block_value(&self, block: &Block<Transaction>, receipts: &[Receipt]) -> U256 {
        let mut value = U256::ZERO;
        let base_fee = block.header.base_fee_per_gas.unwrap_or(0);
        
        // 1. Transaction fees (only the priority fee part/tips)
        let mut prev_cumulative_gas = 0;
        for (tx, receipt) in block.body.transactions.iter().zip(receipts.iter()) {
            let cumulative_gas = receipt.receipt.cumulative_gas_used;
            let gas_used = cumulative_gas.saturating_sub(prev_cumulative_gas);
            prev_cumulative_gas = cumulative_gas;

            let effective_gas_price = match tx {
                Transaction::Legacy(t) => t.tx().gas_price,
                Transaction::Eip2930(t) => t.tx().gas_price,
                Transaction::Eip1559(t) => {
                    let priority_fee = t.tx().max_priority_fee_per_gas;
                    let max_fee = t.tx().max_fee_per_gas;
                    base_fee as u128 + priority_fee.min(max_fee.saturating_sub(base_fee as u128))
                },
                Transaction::Eip4844(t) => {
                    let priority_fee = t.tx().max_priority_fee_per_gas().unwrap_or(0);
                    let max_fee = t.tx().max_fee_per_gas();
                    base_fee as u128 + priority_fee.min(max_fee.saturating_sub(base_fee as u128))
                },
                Transaction::Eip7702(t) => {
                    let priority_fee = t.tx().max_priority_fee_per_gas;
                    let max_fee = t.tx().max_fee_per_gas;
                    base_fee as u128 + priority_fee.min(max_fee.saturating_sub(base_fee as u128))
                }
            };
            
            let priority_fee_per_gas = effective_gas_price.saturating_sub(base_fee as u128);
            let tx_profit = U256::from(gas_used) * U256::from(priority_fee_per_gas);

            value += tx_profit;
        }
        
        value
    }
}