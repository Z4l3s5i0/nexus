use crate::mempool::mempool_provider::MempoolProvider;
use crate::engine::import_gate::ImportGate;
use crate::{Consensus, EngineEvent};
use std::sync::Arc;
use tokio::sync::{broadcast, Notify};
use wasix_eth_execution::cost_model::BuildCostModel;
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
use wasix_eth_utils::{debug, info, error, warn, exp};
use wasix_eth_types::Result;

use std::collections::HashMap;
use std::sync::RwLock;
use dashmap::{DashSet, DashMap};
pub use wasix_eth_types::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStatus {
    /// Initial empty block prepared, first non-empty build in progress.
    InitialBuildInProgress,
    /// First non-empty build (or best possible one) has been completed.
    InitialBuildCompleted,
}

#[derive(Clone, Debug)]
pub struct PayloadBuilderConfig {
    pub slot_duration_ms: u64,
    pub initial_wait_ms: u64,
    pub grace_period_ms: u64,
    /// Hard cap for how long `getPayload` waits for an in-flight build.
    pub get_payload_max_wait_ms: u64,
    /// Builds must be persisted this long before the slot starts.
    pub finalize_safety_margin_ms: u64,
    /// Minimum build window when FCU arrives late in the slot.
    pub min_build_window_ms: u64,
}

fn compute_budget_end(now: std::time::SystemTime, attr_ts: u64, cfg: &PayloadBuilderConfig) -> std::time::SystemTime {
    let slot_start = std::time::UNIX_EPOCH + std::time::Duration::from_secs(attr_ts);
    let margin_end = slot_start
        .checked_sub(std::time::Duration::from_millis(cfg.finalize_safety_margin_ms))
        .unwrap_or(std::time::UNIX_EPOCH);
    let min_end = now + std::time::Duration::from_millis(cfg.min_build_window_ms);
    margin_end.max(min_end)
}

/// Returns the maximum number of transactions for the next build, or `None` when the
/// remaining budget cannot even cover finalizing a single transaction.
/// The first build only uses 25% of the remaining budget so that a non-empty payload is
/// stored early; rebuilds reserve half of the budget for execution and half for finalizing.
fn plan_build(remaining: std::time::Duration, first_build: bool, model: &BuildCostModel) -> Option<usize> {
    if remaining < model.estimate_finalize(1) {
        return None;
    }
    let finalize_share = if first_build { remaining / 4 } else { remaining / 2 };
    Some(model.max_txs_for(finalize_share).max(1))
}

fn derive_effective_slot_ms(parent_ts: u64, attr_ts: u64, max_slot_ms: u64) -> u64 {
    attr_ts.saturating_sub(parent_ts)
        .saturating_mul(1000)
        .clamp(1000, max_slot_ms.max(1000))
}

impl Default for PayloadBuilderConfig {
    fn default() -> Self {
        Self {
            slot_duration_ms: 12000,
            initial_wait_ms: 2400, // 20% of 12s
            grace_period_ms: 200,
            get_payload_max_wait_ms: 1500,
            finalize_safety_margin_ms: 300,
            min_build_window_ms: 1000,
        }
    }
}

#[derive(Clone)]
pub struct PayloadBuilder {
    pub consensus: Arc<dyn Consensus>,
    pub read_storage: DatabaseReadProvider,
    pub write_storage: DatabaseWriteProvider,
    pub execution: Arc<dyn ExecutionProvider>,
    pub mempool: Arc<dyn MempoolProvider>,
    pub event_tx: broadcast::Sender<EngineEvent>,
    /// Per-PayloadID locks to avoid serializing initialization of different payloads.
    pub build_locks: Arc<DashMap<PayloadId, Arc<tokio::sync::Mutex<()>>>>,
    pub cancel_tokens: Arc<RwLock<HashMap<PayloadId, CancellationToken>>>,
    pub rebuild_interruption_tokens: Arc<RwLock<HashMap<PayloadId, CancellationToken>>>,
    pub in_flight_interruptions: Arc<DashSet<PayloadId>>,
    /// Tracks the build status of payloads.
    pub build_statuses: Arc<RwLock<HashMap<PayloadId, BuildStatus>>>,
    /// Effective consensus slot length for each payload.
    pub payload_slots: Arc<DashMap<PayloadId, u64>>,
    /// Wall-clock time by which each payload's builds must be persisted.
    pub payload_budget_end: Arc<DashMap<PayloadId, std::time::SystemTime>>,
    /// Number of transactions offered to the build currently running for each payload.
    pub in_flight_tx_counts: Arc<DashMap<PayloadId, usize>>,
    /// Mempool generation used by the last build attempt for each payload.
    pub last_built_generation: Arc<DashMap<PayloadId, u64>>,
    pub import_gate: Arc<ImportGate>,
    /// Notifies waiters when a payload's build status changes.
    pub build_notify: Arc<RwLock<HashMap<PayloadId, Arc<Notify>>>>,
    pub config: PayloadBuilderConfig,
}

impl PayloadBuilder {
    async fn wait_for_import_or_cancellation(&self, token: &CancellationToken) -> bool {
        tokio::select! {
            _ = self.import_gate.wait_idle() => !token.is_cancelled(),
            _ = token.cancelled() => false,
        }
    }

    async fn wait_for_rebuild_to_clear(&self, payload_id: &PayloadId, deadline: std::time::Instant) {
        let notify = self.build_notify.read().ok()
            .and_then(|notifies| notifies.get(payload_id).cloned());

        let wait_until_complete = async {
            loop {
                let has_active_build = self.rebuild_interruption_tokens.read().unwrap().contains_key(payload_id);
                if !has_active_build {
                    break;
                }

                if let Some(notify) = &notify {
                    tokio::select! {
                        _ = notify.notified() => {},
                        _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {},
                    }
                } else {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
        };

        let timeout = deadline.saturating_duration_since(std::time::Instant::now());
        let _ = tokio::time::timeout(timeout, wait_until_complete).await;
    }

    fn finalize_margin(&self, payload_id: &PayloadId, max_wait: std::time::Duration) -> std::time::Duration {
        let in_flight_txs = self.in_flight_tx_counts.get(payload_id).map(|count| *count).unwrap_or(0);
        self.execution.cost_model().estimate_finalize(in_flight_txs).min(max_wait / 2)
    }

    fn effective_slot_ms(&self, parent_ts: u64, attr_ts: u64) -> u64 {
        derive_effective_slot_ms(parent_ts, attr_ts, self.config.slot_duration_ms)
    }
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

        // 2. Synchronize building initialization for this specific payload ID
        let lock = self.build_locks.entry(id.clone()).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))).value().clone();
        let _guard = lock.lock().await;

        // 3. Re-check after acquiring lock
        if self.read_storage.get_payload(&id).is_some() {
            // Clean up the lock if we are done with it (optional, but good for memory if many IDs)
            // Actually, we should probably keep it until the payload is fully built/expired
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
        let effective_slot_ms = self.effective_slot_ms(parent_block.header.timestamp, attr.timestamp);
        self.payload_slots.insert(id.clone(), effective_slot_ms);
        let budget_end = compute_budget_end(std::time::SystemTime::now(), attr.timestamp, &self.config);
        self.payload_budget_end.insert(id.clone(), budget_end);

        {
            let tokens = self.cancel_tokens.read().unwrap();
            for (other_id, old_token) in tokens.iter() {
                if other_id != &id {
                    old_token.cancel();
                }
            }
        }

        let token = CancellationToken::new();
        {
            let mut tokens = self.cancel_tokens.write().unwrap();
            if let Some(old_token) = tokens.insert(id.clone(), token.clone()) {
                old_token.cancel();
            }
        }

        // 4. Build initial empty payload
        self.build_empty_payload(head_block_hash, attr.clone(), id.clone(), Some(token.clone())).await?;


        // Initialize build status and notify
        self.build_statuses.write().map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?.insert(id.clone(), BuildStatus::InitialBuildInProgress);
        self.build_notify.write().map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?.insert(id.clone(), Arc::new(Notify::new()));

        // 5. Start continuous building background task
        let builder = self.clone();
        let id_for_task = id.clone();
        let mut event_rx = self.event_tx.subscribe();
        tokio::spawn(async move {
            let start_time = std::time::Instant::now();
            let mut last_rebuild_time = std::time::Instant::now();
            let loop_lifetime = std::time::Duration::from_millis(effective_slot_ms.saturating_mul(80) / 100);

            // Perform initial build in background
            if builder.wait_for_import_or_cancellation(&token).await
                && start_time.elapsed() < loop_lifetime
            {
                if let Err(e) = builder.maybe_rebuild_payload(id_for_task.clone()).await {
                    error!("[PayloadBuilder] Error during initial background build for {:?}: {:?}", id_for_task, e);
                }
            }

            while !token.is_cancelled() {
                tokio::task::yield_now().await;
                if start_time.elapsed() >= loop_lifetime {
                    info!("[PayloadBuilder] Stopping continuous building for {:?}: time limit reached", id_for_task);
                    break;
                }

                let debounce_ms = (effective_slot_ms.saturating_mul(2) / 100).max(1);
                let min_rebuild_ms = (effective_slot_ms.saturating_mul(10) / 100).max(100);

                // Wait for a bit or until mempool change
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(min_rebuild_ms)) => {},
                    event = event_rx.recv() => {
                        match event {
                            Ok(EngineEvent::NewTransaction { .. }) | Err(broadcast::error::RecvError::Lagged(_)) => {
                                tokio::select! {
                                    _ = tokio::time::sleep(std::time::Duration::from_millis(debounce_ms)) => {},
                                    _ = token.cancelled() => break,
                                }
                                let mut count = 1;
                                loop {
                                    match event_rx.try_recv() {
                                        Ok(EngineEvent::NewTransaction { .. }) => count += 1,
                                        Ok(EngineEvent::NewBlock { .. }) => {},
                                        Err(broadcast::error::TryRecvError::Lagged(_)) => break,
                                        Err(broadcast::error::TryRecvError::Empty)
                                        | Err(broadcast::error::TryRecvError::Closed) => break,
                                    }
                                }
                                debug!("[PayloadBuilder] Triggering reactive rebuild for {:?} due to {} new transaction(s)", id_for_task, count);
                            }
                            Ok(EngineEvent::NewBlock { .. }) => continue,
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    },
                    _ = token.cancelled() => break,
                }

                let elapsed_since_last = last_rebuild_time.elapsed();
                if elapsed_since_last < std::time::Duration::from_millis(min_rebuild_ms) {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_millis(min_rebuild_ms) - elapsed_since_last) => {},
                        _ = token.cancelled() => break,
                    }
                }

                if !builder.wait_for_import_or_cancellation(&token).await
                    || start_time.elapsed() >= loop_lifetime
                {
                    break;
                }
                last_rebuild_time = std::time::Instant::now();
                if let Err(e) = builder.maybe_rebuild_payload(id_for_task.clone()).await {
                    error!("[PayloadBuilder] Error during continuous building for {:?}: {:?}", id_for_task, e);
                }
            }
            
            let mut tokens = builder.cancel_tokens.write().unwrap();
            if let Some(t) = tokens.get(&id_for_task) {
                if t.is_cancelled() == token.is_cancelled() {
                    tokens.remove(&id_for_task);
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
        interruption_token: Option<CancellationToken>,
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
        
        let effective_slot_ms = self.payload_slots.get(&id).map(|slot| *slot).unwrap_or(self.config.slot_duration_ms);
        let deadline_ms = effective_slot_ms.saturating_mul(35) / 100;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);

        let (finalized_block, receipts, metas, measurements) = match tokio::task::spawn_blocking(move || {
            execution.execute_block_for_payload_timed(
                vec![], // Empty transaction set
                vec![], // No optimistic transactions
                &parent_header_clone,
                &attr_clone,
                base_fee_per_gas,
                interruption_token,
                Some(deadline)
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
        let persistence_start = std::time::Instant::now();

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

        let persistence = persistence_start.elapsed();
        self.execution.cost_model().record(0, measurements.state_root + persistence);
        exp!("[EXP] BUILD_END id={:?} txs=0 exec_ms={} root_ms={} persist_ms={}",
            id,
            measurements.execution.as_millis(),
            measurements.state_root.as_millis(),
            persistence.as_millis());

        info!("[PayloadBuilder] Created initial empty payload_id={:?} for block_number={}", id, parent_block.header.number + 1);
        Ok(())
    }

    pub async fn maybe_rebuild_payload(
        &self,
        payload_id: PayloadId,
    ) -> Result<()> {
        let result = self.maybe_rebuild_payload_inner(payload_id).await;
        
        // Ensure build status is updated and waiters are notified on success (even if no rebuild happened)
        if result.is_ok() {
            self.build_statuses.write().map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?.insert(payload_id, BuildStatus::InitialBuildCompleted);
            if let Some(notify) = self.build_notify.read().map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?.get(&payload_id) {
                notify.notify_waiters();
            }
        }

        result
    }

    async fn maybe_rebuild_payload_inner(
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
        let (mut transactions, mut optimistic_transactions) = snapshot.peek_best_transactions(
            parent_block.header.gas_limit, 
            U256::from(base_fee_per_gas.unwrap_or_default()),
            blob_base_fee.map(U256::from),
            max_blobs_per_block
        );

        let last_built_generation = self.last_built_generation.get(&payload_id).map(|generation| *generation);
        if last_built_generation == Some(snapshot.generation) {
            return Ok(());
        }

        // Budget the build so execution, state root and persistence finish before the slot starts.
        let now = std::time::SystemTime::now();
        let budget_end = self.payload_budget_end.get(&payload_id).map(|end| *end)
            .unwrap_or_else(|| compute_budget_end(now, payload_block.header.timestamp, &self.config));
        let remaining = budget_end.duration_since(now).unwrap_or_default();
        let first_build = self.build_statuses.read().ok()
            .and_then(|statuses| statuses.get(&payload_id).copied())
            == Some(BuildStatus::InitialBuildInProgress);
        let cost_model = self.execution.cost_model();
        let max_txs = match plan_build(remaining, first_build, &cost_model) {
            Some(max_txs) => max_txs,
            None => {
                debug!("[PayloadBuilder] Skipping build for {:?}: budget exhausted ({}ms left)", payload_id, remaining.as_millis());
                return Ok(());
            }
        };
        transactions.truncate(max_txs);
        optimistic_transactions.truncate(max_txs.saturating_sub(transactions.len()));

        let transactions_unchanged = transactions.len() == payload_block.body.transactions.len()
            && transactions.iter().zip(payload_block.body.transactions.iter())
                .all(|(new_tx, old_tx)| new_tx.hash() == old_tx.hash());
        if transactions_unchanged {
            self.last_built_generation.insert(payload_id, snapshot.generation);
            return Ok(());
        }

        if transactions.is_empty() && optimistic_transactions.is_empty() && !payload_block.body.transactions.is_empty() {
             // Should not happen if we already have a payload with transactions, but good to be safe.
             self.last_built_generation.insert(payload_id, snapshot.generation);
             return Ok(());
        }

        tokio::task::yield_now().await;
        
        // --- Interruption management ---
        let token = CancellationToken::new();
        {
            let mut tokens = self.rebuild_interruption_tokens.write().unwrap();
            tokens.insert(payload_id, token.clone());
        }
        // Cleanup token when this scope ends
        let rebuild_tokens = Arc::clone(&self.rebuild_interruption_tokens);
        let in_flight_interruptions = Arc::clone(&self.in_flight_interruptions);
        let in_flight_tx_counts = Arc::clone(&self.in_flight_tx_counts);
        in_flight_tx_counts.insert(payload_id, transactions.len() + optimistic_transactions.len());
        let payload_id_copy = payload_id;
        let _cleanup = scopeguard::guard((), move |_| {
            let mut tokens = rebuild_tokens.write().unwrap();
            tokens.remove(&payload_id_copy);
            in_flight_interruptions.remove(&payload_id_copy);
            in_flight_tx_counts.remove(&payload_id_copy);
        });
        // --------------------------------

        let write_storage = self.write_storage.clone();
        // 4. Re-run execution
        let execution = Arc::clone(&self.execution);
        let parent_header_clone = parent_block.header.clone();
        let attr_clone = attr.clone();
        let token_clone = token.clone();
        
        let effective_slot_ms = self.payload_slots.get(&payload_id).map(|slot| *slot).unwrap_or(self.config.slot_duration_ms);
        let deadline_ms = effective_slot_ms.saturating_mul(35) / 100;
        let loop_start = std::time::Instant::now();
        let slot_cap = loop_start + std::time::Duration::from_millis(deadline_ms);
        let budget_cap = loop_start + remaining.saturating_sub(cost_model.estimate_finalize(max_txs));
        let deadline = slot_cap.min(budget_cap);
        let result = tokio::task::spawn_blocking(move || {
            execution.execute_block_for_payload_timed(
                transactions,
                optimistic_transactions,
                &parent_header_clone,
                &attr_clone,
                base_fee_per_gas,
                Some(token_clone),
                Some(deadline)
            )
        }).await.map_err(|e| RpcError::Internal(format!("Payload rebuilding task panicked: {}", e)))?
        .map_err(|e| RpcError::Internal(e.to_string()))?;

        let (finalized_block, receipts, metas, measurements) = result;
        let tx_count = finalized_block.body.transactions.len();

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
            self.last_built_generation.insert(payload_id, snapshot.generation);
            self.execution.cost_model().record(tx_count, measurements.state_root);
            exp!("[EXP] BUILD_END id={:?} txs={} exec_ms={} root_ms={} persist_ms=0",
                payload_id,
                tx_count,
                measurements.execution.as_millis(),
                measurements.state_root.as_millis());
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
        let persistence_start = std::time::Instant::now();
        tokio::task::spawn_blocking(move || {
            write_storage.add_payload(payload_id, finalized_block_clone, receipts_clone, metas_clone, bundle)
        }).await.map_err(|e| RpcError::Internal(format!("Payload rebuilding task panicked: {}", e)))?
        .map_err(|e| RpcError::Internal(e.to_string()))?;
        let persistence = persistence_start.elapsed();
        self.execution.cost_model().record(tx_count, measurements.state_root + persistence);
        exp!("[EXP] BUILD_END id={:?} txs={} exec_ms={} root_ms={} persist_ms={}",
            payload_id,
            tx_count,
            measurements.execution.as_millis(),
            measurements.state_root.as_millis(),
            persistence.as_millis());
        self.last_built_generation.insert(payload_id, snapshot.generation);

        Ok(())
    }

    pub async fn get_payload(&self, payload_id: &PayloadId) -> RpcResult<(Block<Transaction>, Vec<Receipt>, Vec<wasix_eth_types::ReceiptMeta>, BlobsBundleV1)> {
        let wait_start = std::time::Instant::now();
        let max_wait = std::time::Duration::from_millis(self.config.get_payload_max_wait_ms);
        let hard_deadline = wait_start + max_wait;

        // 1. While the first build is pending (possibly still waiting for an import), wait for it
        // without cancelling anything, leaving enough time for an interrupted build to finalize.
        loop {
            let status = self.build_statuses.read()
                .map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?
                .get(payload_id)
                .copied();
            let pending = status == Some(BuildStatus::InitialBuildInProgress) || self.import_gate.is_busy();
            if !pending {
                break;
            }

            let soft_deadline = hard_deadline
                .checked_sub(self.finalize_margin(payload_id, max_wait))
                .unwrap_or(wait_start);
            let now = std::time::Instant::now();
            if now >= soft_deadline {
                break;
            }

            let notify = self.build_notify.read()
                .map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?
                .get(payload_id)
                .cloned();
            let poll = (soft_deadline - now).min(std::time::Duration::from_millis(20));
            match notify {
                Some(notify) => {
                    let _ = tokio::time::timeout(poll, notify.notified()).await;
                }
                None => tokio::time::sleep(poll).await,
            }
        }

        // 2. Stop starting new builds, then stop only the tx loop of a running build so its
        // state root and persistence still complete, and wait for it up to the hard deadline.
        if let Some(loop_token) = self.cancel_tokens.read().unwrap().get(payload_id).cloned() {
            loop_token.cancel();
        }
        let rebuild_token = self.rebuild_interruption_tokens.read().unwrap().get(payload_id).cloned();
        if let Some(rebuild_token) = rebuild_token {
            if !self.in_flight_interruptions.contains(payload_id) {
                self.in_flight_interruptions.insert(*payload_id);
                debug!("[PayloadBuilder] Stopping tx loop of in-flight build for payload {:?}", payload_id);
                rebuild_token.cancel();
            }
            self.wait_for_rebuild_to_clear(payload_id, hard_deadline).await;
        }

        // 5. Return whatever we have in storage. If the builder was interrupted, 
        // it should have persisted the last successful state.
        let payload = self.read_storage.get_payload(payload_id)
            .ok_or_else(|| RpcError::BlockNotFound(wasix_eth_types::BlockId::Hash(B256::from_slice(&payload_id.0[..]).into())))?;

        info!("[PayloadBuilder] Returning payload {:?} with {} transactions after {:?}ms", 
            payload_id, payload.0.body.transactions.len(), wait_start.elapsed().as_millis());
        let build_status = self.build_statuses.read()
            .map_err(|_| RpcError::Internal("Lock poisoned".to_string()))?
            .get(payload_id)
            .map(|status| format!("{:?}", status))
            .unwrap_or_else(|| "Unknown".to_string());
        exp!("[EXP] GET_PAYLOAD id={:?} waited_ms={} status={} txs={}",
            payload_id,
            wait_start.elapsed().as_millis(),
            build_status,
            payload.0.body.transactions.len());
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

#[cfg(test)]
mod tests {
    use super::{compute_budget_end, derive_effective_slot_ms, plan_build, PayloadBuilderConfig};
    use std::time::{Duration, UNIX_EPOCH};
    use wasix_eth_execution::cost_model::BuildCostModel;

    #[test]
    fn budget_ends_before_future_slot_start() {
        let cfg = PayloadBuilderConfig::default();
        let now = UNIX_EPOCH + Duration::from_secs(100);
        let end = compute_budget_end(now, 106, &cfg);
        assert_eq!(end, UNIX_EPOCH + Duration::from_secs(106) - Duration::from_millis(cfg.finalize_safety_margin_ms));
    }

    #[test]
    fn late_fcu_gets_minimum_build_window() {
        let cfg = PayloadBuilderConfig::default();
        let now = UNIX_EPOCH + Duration::from_secs(110);
        let end = compute_budget_end(now, 106, &cfg);
        assert_eq!(end, now + Duration::from_millis(cfg.min_build_window_ms));
    }

    #[test]
    fn exhausted_budget_skips_build() {
        let model = BuildCostModel::default();
        assert_eq!(plan_build(Duration::ZERO, false, &model), None);
        assert_eq!(plan_build(model.estimate_finalize(1) / 2, true, &model), None);
    }

    #[test]
    fn first_build_is_smaller_than_rebuild() {
        let model = BuildCostModel::default();
        let budget = Duration::from_secs(4);
        let first = plan_build(budget, true, &model).unwrap();
        let rebuild = plan_build(budget, false, &model).unwrap();
        assert!(first >= 1);
        assert!(first <= rebuild);
    }

    #[test]
    fn effective_slot_is_derived_and_clamped() {
        assert_eq!(derive_effective_slot_ms(10, 14, 12_000), 4_000);
        assert_eq!(derive_effective_slot_ms(10, 10, 12_000), 1_000);
        assert_eq!(derive_effective_slot_ms(11, 10, 12_000), 1_000);
        assert_eq!(derive_effective_slot_ms(10, 40, 12_000), 12_000);
    }
}