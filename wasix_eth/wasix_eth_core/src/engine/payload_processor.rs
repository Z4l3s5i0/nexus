use dashmap::{DashMap, DashSet};
use futures_util::{future::BoxFuture, FutureExt};
use crate::engine::canonicality_tracker::CanonicalState;
use crate::engine::sidechain_tracker::{BlockTree};
use crate::sync::registry::SyncRegistry;
use crate::Consensus;
use std::sync::Arc;
use tokio::sync::broadcast;
use wasix_eth_execution::execution_provider::ExecutionProvider;
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::{BlockProvider, ChainProvider, HeaderProvider};
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_storage::write_traits::{BlockWriter, HeaderWriter, TransactionWriter};
use wasix_eth_types::error::{RpcError, RpcResult};
use wasix_eth_types::{Block, BlockId, ChainConfig, PayloadStatus, PayloadStatusEnum, Transaction, B256, U256, proofs, Header, Hardfork, ChainManager, InvalidationReason};
use wasix_eth_utils::{debug, error, info, exp, metrics::BLOCKS_IMPORTED_TOTAL};
use crate::engine::engine::EngineEvent;

#[derive(Clone)]
pub struct PayloadProcessor {
    pub processing_payloads: Arc<DashSet<B256>>,
    pub processing_heights: Arc<DashMap<u64, B256>>,
    pub in_flight_results: Arc<DashMap<B256, broadcast::Sender<PayloadStatus>>>,
    pub block_tree: Arc<BlockTree>,
    pub canonical: Arc<CanonicalState>,
    pub consensus: Arc<dyn Consensus>,
    pub read_storage: DatabaseReadProvider,
    pub write_storage: DatabaseWriteProvider,
    pub execution: Arc<dyn ExecutionProvider>,
    pub chain: Arc<dyn ChainManager>,
    pub sync_registry: Arc<SyncRegistry>,
    pub event_tx: broadcast::Sender<EngineEvent>,
}

impl PayloadProcessor {
    pub async fn add_sync_target(&self, hash: B256, peer_id: Option<String>) {
        self.sync_registry.add_target(hash, peer_id, self.chain.clone()).await;
    }

    pub async fn new_payload_internal(
        &self,
        block: Block<Transaction>,
        expected_block_hash: B256,
        expected_blob_versioned_hashes: Option<Vec<B256>>,
        parent_beacon_block_root: Option<B256>,
        is_local: bool,
    ) -> RpcResult<PayloadStatus> {
        tokio::task::yield_now().await;
        let actual_hash = block.header.hash_slow();
        info!("[PayloadProcessor] new_payload_internal: block={}, hash={}, parent={}", block.header.number, actual_hash, block.header.parent_hash);

        if actual_hash != expected_block_hash {
            debug!("[PayloadProcessor] block hash mismatch: actual={}, expected={}", actual_hash, expected_block_hash);
            
            // Check if it's already in storage before marking it invalid.
            if let Ok(Some(_)) = self.read_storage.block_body_by_hash(actual_hash) {
                 return Ok(PayloadStatus {
                    status: PayloadStatusEnum::Invalid { validation_error: "INVALID_BLOCK_HASH".to_string() },
                    latest_valid_hash: Some(actual_hash),
                });
            }

            self.chain.add_invalid_block(actual_hash, block.header.parent_hash, InvalidationReason::Soft).await;
            return Ok(PayloadStatus {
                status: PayloadStatusEnum::Invalid { validation_error: "INVALID_BLOCK_HASH".to_string() },
                latest_valid_hash: None,
            });
        }

        let chain_config = self.read_storage.chain_config().ok().flatten().unwrap_or_else(|| ChainConfig {
            chain_id: self.read_storage.chain_id().unwrap_or(31133),
            ..Default::default()
        });
        let active_fork = Hardfork::get_active_fork(&chain_config, block.header.number, block.header.timestamp);

        // 1. Idempotency check: See if this block is already being processed
        if let Some(sender) = self.in_flight_results.get(&actual_hash) {
            debug!("[PayloadProcessor] Block {:?} is already being processed, joining request", actual_hash);
            let mut receiver = sender.subscribe();
            drop(sender);
            match receiver.recv().await {
                Ok(status) => return Ok(status),
                Err(_) => {
                    // This happens if the original sender was dropped without sending a result.
                    // Fall through to start a new execution.
                }
            }
        }

        // 2. Mark as processing to avoid concurrent imports of the same block
        {
            // Optimization: Avoid concurrent processing of the same block HEIGHT
            // This prevents race conditions between different payloads for the same slot
            let block_number = block.header.number;
            
            if self.processing_payloads.contains(&actual_hash) {
                debug!("[PayloadProcessor] Block {:?} is already being processed, skipping", actual_hash);
                return Ok(PayloadStatus {
                    status: PayloadStatusEnum::Syncing,
                    latest_valid_hash: None,
                });
            }

            if let Some(existing_hash) = self.processing_heights.get(&block_number) {
                if *existing_hash != actual_hash {
                    debug!("[PayloadProcessor] Another block {:?} for height {} is already being processed, skipping", *existing_hash, block_number);
                    return Ok(PayloadStatus {
                        status: PayloadStatusEnum::Syncing,
                        latest_valid_hash: None,
                    });
                }
            }

            self.processing_payloads.insert(actual_hash);
            self.processing_heights.insert(block_number, actual_hash);
        }

        // Setup idempotency channel
        let (tx, _) = broadcast::channel(1);
        self.in_flight_results.insert(actual_hash, tx.clone());

        // Helper to broadcast result and cleanup
        let processing_heights = self.processing_heights.clone();
        let block_number = block.header.number;
        let cleanup = move |res: RpcResult<PayloadStatus>| {
            self.processing_payloads.remove(&actual_hash);
            processing_heights.remove(&block_number);
            self.in_flight_results.remove(&actual_hash);
            if let Ok(ref status) = res {
                let _ = tx.send(status.clone());
            }
            res
        };

        // Cancun validation (versioned hashes)
        if active_fork >= Hardfork::Cancun {
            if let Err(e) = self.consensus.validate_cancun(&block, expected_blob_versioned_hashes.as_deref()) {
                error!("[PayloadProcessor] Cancun validation (versioned hashes) failed for block {}: {}", actual_hash, e);

                // EIP-4844: "Client software SHOULD NOT permanently blacklist the block hash if it is rejected due to a mismatch between
                // expected_blob_versioned_hashes and the actual hashes in the block."
                // We mark it as Soft invalid so FCU returns SYNCING, but it's not permanently rejected.
                
                // If it's already in main storage, it's officially valid. Don't overwrite with SOFT invalidation.
                if self.read_storage.block_body_by_hash(actual_hash).ok().flatten().is_none() {
                    self.chain.add_invalid_block(actual_hash, block.header.parent_hash, InvalidationReason::Soft).await;
                }
                
                let latest_valid = self.chain.get_latest_valid_ancestor(block.header.parent_hash).await;
                return cleanup(Ok(PayloadStatus {
                    status: PayloadStatusEnum::Invalid { validation_error: e },
                    latest_valid_hash: latest_valid,
                }));
            }
            if let Err(e) = self.consensus.validate_parent_beacon_block_root(&block.header, parent_beacon_block_root) {
                error!("[PayloadProcessor] Cancun validation (parent beacon block root) failed for block {}: {}", actual_hash, e);

                // This is also a potentially retryable error if the CL sent the wrong root, 
                // but usually it's tied to the block itself. EIP-4844 doesn't explicitly mention this for root,
                // but it's safer to use Soft invalidation or none.
                
                // If it's already in main storage, it's officially valid. Don't overwrite with SOFT invalidation.
                if self.read_storage.block_body_by_hash(actual_hash).ok().flatten().is_none() {
                    self.chain.add_invalid_block(actual_hash, block.header.parent_hash, InvalidationReason::Soft).await;
                }

                let latest_valid = self.chain.get_latest_valid_ancestor(block.header.parent_hash).await;
                return cleanup(Ok(PayloadStatus {
                    status: PayloadStatusEnum::Invalid { validation_error: e },
                    latest_valid_hash: latest_valid,
                }));
            }
        }

        // Idempotency check: If block is in storage, it's VALID
        if let Ok(Some(_)) = self.read_storage.block_body_by_hash(actual_hash) {
            return cleanup(Ok(PayloadStatus {
                status: PayloadStatusEnum::Valid,
                latest_valid_hash: Some(actual_hash)
            }));
        }

        // Check if it's a known invalid block
        if let Some(reason) = self.chain.get_invalidation_reason(actual_hash).await {
            if reason == InvalidationReason::Hard {
                let latest_valid = self.chain.get_latest_valid_ancestor(block.header.parent_hash).await;
                return cleanup(Ok(PayloadStatus {
                    status: PayloadStatusEnum::Invalid { validation_error: "Block is known to be invalid".to_string() },
                    latest_valid_hash: latest_valid,
                }));
            }
        }

        // 0.1 Check if parent is explicitly known invalid, reject immediately
        let parent_hash = block.header.parent_hash;
        if let Some(reason) = self.chain.get_invalidation_reason(parent_hash).await {
            // Only reject if it's a HARD invalidation.
            if reason == InvalidationReason::Hard {
                let latest_valid = self.chain.get_latest_valid_ancestor(parent_hash).await;
                info!("[PayloadProcessor] rejecting payload {:?} because parent {:?} is known invalid (Hard). Latest valid: {:?}", actual_hash, parent_hash, latest_valid);
                
                // Also mark this block as invalid
                self.chain.add_invalid_block(actual_hash, parent_hash, InvalidationReason::Hard).await;
                
                return cleanup(Ok(PayloadStatus {
                    status: PayloadStatusEnum::Invalid { validation_error: "Parent block is known to be invalid".to_string() },
                    latest_valid_hash: latest_valid,
                }));
            } else if reason == InvalidationReason::Soft {
                 let latest_valid = self.chain.get_latest_valid_ancestor(parent_hash).await;
                 info!("[PayloadProcessor] rejecting payload {:?} because parent {:?} is known invalid (Soft). Latest valid: {:?}", actual_hash, parent_hash, latest_valid);

                self.chain.add_invalid_block(actual_hash, parent_hash, InvalidationReason::Soft).await;

                 return cleanup(Ok(PayloadStatus {
                    status: PayloadStatusEnum::Invalid { validation_error: "Parent block is known to be invalid (Soft)".to_string() },
                    latest_valid_hash: latest_valid,
                 }));
            }
        }

        let result = match self.new_payload_internal_inner(block.clone(), expected_block_hash, expected_blob_versioned_hashes, parent_beacon_block_root, is_local).await {
            Ok(res) => Ok(res),
            Err(e) => {
                error!("[PayloadProcessor] internal error during payload processing: {}", e);
                Err(e)
            }
        };

        let result = cleanup(result);

        if let Ok(ref status) = result {

            if status.status == PayloadStatusEnum::Valid {
                self.revalidate_dependent_payloads(actual_hash).await;
                
                info!("[PayloadProcessor] Successfully imported block {} (hash: {})", block.header.number, actual_hash);
                BLOCKS_IMPORTED_TOTAL.add(1.0);
            }
        }
        result
    }

    pub async fn get_header_from_anywhere(&self, hash: B256) -> Option<Header> {
        // 1. Check storage
        if let Ok(Some(header)) = self.read_storage.header(BlockId::Hash(hash.into())) {
            return Some(header);
        }
        
        // 2. Check payload map
        if let Some((payload, _, _, _)) = self.read_storage.get_payload_by_block_hash(hash) {
            return Some(payload.header.clone());
        }
        
        // 3. Check block tree
        if let Some(block) = self.block_tree.get_block(hash).await {
            return Some(block.header.clone());
        }
        
        None
    }


    pub async fn validate_parent_block(&self, parent_hash: B256) -> Option<PayloadStatus> {
        debug!("[PayloadProcessor] validate_parent_block: parent_hash={}", parent_hash);
        
        // 1. Check if parent or ANY ancestor is known to be invalid
        let mut current_invalid_check = parent_hash;
        for i in 0..64 {
            if let Some(reason) = self.chain.get_invalidation_reason(current_invalid_check).await {
                if reason == InvalidationReason::Hard || reason == InvalidationReason::Soft {
                    let latest_valid = self.chain.get_latest_valid_ancestor(current_invalid_check).await;
                    info!("[PayloadProcessor] Ancestor block {:?} (depth {}) is known to be invalid ({:?}). Latest valid ancestor: {:?}", current_invalid_check, i, reason, latest_valid);
                    return Some(PayloadStatus {
                        status: PayloadStatusEnum::Invalid { validation_error: format!("Ancestor block {} is known to be invalid", current_invalid_check) },
                        latest_valid_hash: latest_valid,
                    });
                }
            }
            
            // Try to find parent to continue walking back
            if let Some(header) = self.get_header_from_anywhere(current_invalid_check).await {
                if header.parent_hash == B256::ZERO { break; }
                current_invalid_check = header.parent_hash;
            } else {
                // Check if it's an invalid parent we know about
                if let Some(p) = self.block_tree.get_invalid_parent(current_invalid_check).await {
                    current_invalid_check = p;
                } else {
                    break;
                }
            }
        }

        let is_genesis = self.read_storage.is_canonical(parent_hash).unwrap_or(false) &&
            self.read_storage.block_number(parent_hash).ok().flatten() == Some(0);

        // If parent is not known (not in storage AND not in block_tree), it's SYNCING/ACCEPTED.
        // todo check if it is processing as well
        if !is_genesis && !self.chain.has_block(parent_hash).await {
            info!("[PayloadProcessor] Parent block {:?} not found. Returning ACCEPTED.", parent_hash);
            
            let registry = self.sync_registry.clone();
            let chain = self.chain.clone();
            tokio::spawn(async move {
                registry.add_target(parent_hash, None, chain).await;
            });

            return Some(PayloadStatus {
                status: PayloadStatusEnum::Accepted,
                latest_valid_hash: None,
            });
        }

        // 2. Check if ANY ancestor is invalid by walking back and validating headers
        let mut current_hash = parent_hash;
        let chain_config = self.read_storage.chain_config().ok().flatten().unwrap_or_else(|| ChainConfig {
            chain_id: self.read_storage.chain_id().unwrap_or(1),
            ..Default::default()
        });

        for i in 0..32 {
            if let Some(current_header) = self.get_header_from_anywhere(current_hash).await {
                let p_hash = current_header.parent_hash;

                // Check if parent is marked as invalid
                if let Some(reason) = self.chain.get_invalidation_reason(p_hash).await {
                    if reason == InvalidationReason::Hard || reason == InvalidationReason::Soft {
                        let latest_valid = self.chain.get_latest_valid_ancestor(p_hash).await;
                        info!("[PayloadProcessor] Ancestor block {:?} at depth {} is known to be invalid ({:?}). Latest valid ancestor: {:?}", p_hash, i, reason, latest_valid);
                        self.chain.add_invalid_block(current_hash, p_hash, InvalidationReason::Hard).await;
                        
                        // Trigger recursive invalidation of orphans
                        let self_clone = self.clone();
                        tokio::spawn(async move {
                            self_clone.invalidate_descendants(current_hash).await;
                        });

                        return Some(PayloadStatus {
                            status: PayloadStatusEnum::Invalid { validation_error: "Ancestor block is known to be invalid".to_string() },
                            latest_valid_hash: latest_valid,
                        });
                    }
                }

                // Check if this block is invalid relative to its parent header if we can find it
                if let Some(p_header) = self.get_header_from_anywhere(p_hash).await {
                    if let Err(e) = self.consensus.validate_header(&current_header, &p_header, &chain_config) {
                        // Only mark and return INVALID if both blocks are known,
                        // otherwise it might be a temporary side-chain inconsistency during sync.
                        let current_exists = self.chain.has_block(current_hash).await;
                        let parent_exists = self.chain.has_block(p_hash).await;

                        if current_exists && parent_exists {
                            info!("[PayloadProcessor] Ancestor block {:?} at depth {} failed header validation: {}. Latest valid ancestor: {:?}", current_hash, i, e, p_hash);
                            self.chain.add_invalid_block(current_hash, p_hash, InvalidationReason::Hard).await;
                            let latest_valid = self.chain.get_latest_valid_ancestor(p_hash).await;
                            return Some(PayloadStatus {
                                status: PayloadStatusEnum::Invalid { validation_error: format!("Ancestor header validation failed: {}", e) },
                                latest_valid_hash: latest_valid,
                            });
                        } else {
                            info!("[PayloadProcessor] Ancestor block {:?} at depth {} failed header validation, but one of them is missing. Returning ACCEPTED.", current_hash, i);
                            return Some(PayloadStatus {
                                status: PayloadStatusEnum::Accepted,
                                latest_valid_hash: None,
                            });
                        }
                    }
                }

                if p_hash == B256::ZERO || i == 15 { break; }
                current_hash = p_hash;
            } else {
                break;
            }
        }

        None
    }

    pub async fn new_payload_internal_inner(
        &self,
        block: Block<Transaction>,
        _expected_block_hash: B256,
        expected_blob_versioned_hashes: Option<Vec<B256>>,
        parent_beacon_block_root: Option<B256>,
        is_local: bool,
    ) -> RpcResult<PayloadStatus> {
        tokio::task::yield_now().await;
        let actual_hash = block.header.hash_slow();
        let parent_hash = block.header.parent_hash;
        let block_number = block.header.number;
        debug!("[PayloadProcessor] new_payload_internal_inner: block={}, hash={}, parent={}", block_number, actual_hash, parent_hash);

        exp!("[EXP] BLOCK_EXEC_START number={} hash={:?} parent={:?} tx_count={}", block_number, actual_hash, parent_hash, block.body.transactions.len());
        if is_local {
            wasix_eth_utils::exp!("[EXP] BLOCK_RPC_RECV hash={:?}", actual_hash);
        }

        let chain_config = self.read_storage.chain_config().ok().flatten().unwrap_or_else(|| ChainConfig {
            chain_id: self.read_storage.chain_id().unwrap_or(31133),
            ..Default::default()
        });
        
        let active_fork = Hardfork::get_active_fork(&chain_config, block_number, block.header.timestamp);
        debug!("[PayloadProcessor] active_fork: {:?} for block {}", active_fork, block_number);

        // 1. Cancun validation
        if active_fork >= Hardfork::Cancun {
            if let Err(e) = self.consensus.validate_cancun(&block, expected_blob_versioned_hashes.as_deref()) {
                error!("[PayloadProcessor] Cancun validation failed for block {}: {}", actual_hash, e);

                // EIP-4844: "Client software SHOULD NOT permanently blacklist the block hash if it is rejected due to a mismatch between
                // expected_blob_versioned_hashes and the actual hashes in the block."
                // We mark it as Soft invalid so FCU returns SYNCING, but it's not permanently rejected.

                // If it's already in main storage, it's officially valid. Don't overwrite with SOFT invalidation.
                if self.read_storage.block_body_by_hash(actual_hash).ok().flatten().is_none() {
                    self.chain.add_invalid_block(actual_hash, parent_hash, InvalidationReason::Soft).await;
                }

                if let Ok(Some(existing_header)) = self.read_storage.header(BlockId::Hash(actual_hash.into())) {
                    if existing_header.hash_slow() == actual_hash {
                        let latest_valid = self.chain.get_latest_valid_ancestor(parent_hash).await;
                        return Ok(PayloadStatus {
                            status: PayloadStatusEnum::Invalid { validation_error: e.to_string() },
                            latest_valid_hash: latest_valid,
                        });
                    }
                }


                let latest_valid = self.chain.get_latest_valid_ancestor(parent_hash).await;
                return Ok(PayloadStatus {
                    status: PayloadStatusEnum::Invalid { validation_error: e.to_string() },
                    latest_valid_hash: latest_valid,
                });
            }
        }

        // 2. Structural validation (Body)
        if let Err(e) = self.consensus.validate_body(&block, &chain_config) {
            error!("[PayloadProcessor] Body validation failed for block {}: {}", actual_hash, e);

                // Structural failures are Hard invalid.
                // But we don't call add_invalid_block if it's already in storage.
                if let Ok(Some(existing_header)) = self.read_storage.header(BlockId::Hash(actual_hash.into())) {
                    if existing_header.hash_slow() == actual_hash {
                        let latest_valid = self.chain.get_latest_valid_ancestor(parent_hash).await;
                        return Ok(PayloadStatus {
                            status: PayloadStatusEnum::Invalid { validation_error: e.to_string() },
                            latest_valid_hash: latest_valid,
                        });
                    }
                }

                // We mark it as invalid to preserve the parent hash and allow walk-back.
                self.chain.add_invalid_block(actual_hash, parent_hash, InvalidationReason::Hard).await;

                let _ = self.write_storage.remove_payload_by_block_hash(actual_hash);
                let latest_valid = self.chain.get_latest_valid_ancestor(parent_hash).await;
                return Ok(PayloadStatus {
                    status: PayloadStatusEnum::Invalid { validation_error: e.to_string() },
                    latest_valid_hash: latest_valid,
                });
            }

        // 3. Parent beacon block root validation (Cancun)
        if active_fork >= Hardfork::Cancun {
            if let Err(e) = self.consensus.validate_parent_beacon_block_root(&block.header, parent_beacon_block_root) {
                error!("[PayloadProcessor] Parent beacon block root validation failed for block {}: {}", actual_hash, e);

                // If the block is already officially imported in main storage, it's officially valid.
                // Don't overwrite with SOFT invalidation.
                if self.read_storage.block_body_by_hash(actual_hash).ok().flatten().is_none() {
                    self.chain.add_invalid_block(actual_hash, parent_hash, InvalidationReason::Soft).await;
                }

                if let Ok(Some(existing_header)) = self.read_storage.header(BlockId::Hash(actual_hash.into())) {
                    if existing_header.hash_slow() == actual_hash {
                        let latest_valid = self.chain.get_latest_valid_ancestor(parent_hash).await;
                        return Ok(PayloadStatus {
                            status: PayloadStatusEnum::Invalid { validation_error: e.to_string() },
                            latest_valid_hash: latest_valid,
                        });
                    }
                }

                let latest_valid = self.chain.get_latest_valid_ancestor(parent_hash).await;
                return Ok(PayloadStatus {
                    status: PayloadStatusEnum::Invalid { validation_error: e.to_string() },
                    latest_valid_hash: latest_valid,
                });
            }
        }

        // 4. Validate parent block - After structural validations
        let parent_status = self.validate_parent_block(parent_hash).await;
        
        if let Some(status) = &parent_status {
            info!("[PayloadProcessor] parent_status: {:?}", status.status);
            
            // If parent is unknown (Syncing/Accepted), buffer it and return the parent status.
            if status.status == PayloadStatusEnum::Syncing || status.status == PayloadStatusEnum::Accepted {
                info!("[PayloadProcessor] parent is unknown, buffering block {} and returning {:?}", actual_hash, status.status);
                self.block_tree.add_orphan(parent_hash, block, expected_blob_versioned_hashes, parent_beacon_block_root).await;
                return Ok(status.clone());
            }

            if status.status != PayloadStatusEnum::Valid {
                info!("[PayloadProcessor] parent status is {:?}, marking block {} as invalid and walking descendants", status.status, actual_hash);
                self.chain.add_invalid_block(actual_hash, parent_hash, InvalidationReason::Hard).await;

                let self_clone = self.clone();
                let initial_invalid_hash = actual_hash;
                tokio::spawn(async move {
                    self_clone.invalidate_descendants(initial_invalid_hash).await;
                });

                let latest_valid = self.chain.get_latest_valid_ancestor(parent_hash).await;
                return Ok(PayloadStatus {
                    status: status.status.clone(),
                    latest_valid_hash: latest_valid,
                });
            }
        }

        // 5. Check if block is already known and validated
        // We already checked this at the very beginning of new_payload_internal.

        // Before executing, check if parent header is available in storage or Payloads table.
        let parent_header = self.read_storage.header(BlockId::Hash(parent_hash.into())).ok().flatten();
            // .or_else(|| {
            //     // Fallback: Check Payloads table if not in main Headers table
            //     self.read_storage.get_payload_by_block_hash(parent_hash).map(|(b, _, _)| b.header)
            // });
        
        // If header not found in storage/payloads, check if it's currently being processed or available elsewhere.
        let parent_header = if parent_header.is_none() {
            if self.processing_payloads.contains(&parent_hash) {
                // If it's being processed, we can't get its header yet but we know it's coming.
                  // We return SYNCING and the CL will retry.
                  info!("[PayloadProcessor] Parent block {:?} is currently being processed. Returning SYNCING.", parent_hash);
                  return Ok(PayloadStatus {
                      status: PayloadStatusEnum::Syncing,
                      latest_valid_hash: None,
                  });
             }

             // If parent is found in the orphan pool, we must buffer this child and return ACCEPTED.
             if let Some(anywhere_header) = self.get_header_from_anywhere(parent_hash).await {
                  info!("[PayloadProcessor] Parent block {:?} found in orphan pool. Buffering block {:?} and returning ACCEPTED.", parent_hash, actual_hash);
                  
                  // Even if we don't execute, we should validate the header if we have the parent header to return INVALID early if possible.
                  if let Err(e) = self.consensus.validate_header(&block.header, &anywhere_header, &chain_config) {
                      error!("[PayloadProcessor] Header validation failed for buffered block {}: {}", actual_hash, e);
                      self.chain.add_invalid_block(actual_hash, parent_hash, InvalidationReason::Hard).await;
                      let latest_valid = self.chain.get_latest_valid_ancestor(parent_hash).await;
                      return Ok(PayloadStatus {
                          status: PayloadStatusEnum::Invalid { validation_error: e.to_string() },
                          latest_valid_hash: latest_valid,
                      });
                  }

                  {
                      self.block_tree.add_orphan(parent_hash, block, expected_blob_versioned_hashes, parent_beacon_block_root).await;
                  }
                  return Ok(PayloadStatus {
                      status: PayloadStatusEnum::Accepted,
                      latest_valid_hash: None,
                  });
             }
             None
        } else {
             parent_header
        };

        if parent_header.is_none() {
            info!("[PayloadProcessor] Parent block {:?} header not found anywhere. Returning SYNCING to trigger discovery.", parent_hash);
            return Ok(PayloadStatus {
                status: PayloadStatusEnum::Syncing,
                latest_valid_hash: None,
            });
        }

        if let Some(parent) = &parent_header {
            if let Err(e) = self.consensus.validate_header(&block.header, parent, &chain_config) {
                error!("[PayloadProcessor] Header validation failed: {}", e);
                // Header validation is structural/independent of state.
                // We SHOULD blacklist if header validation fails, as it's inherent to the block.
                self.chain.add_invalid_block(actual_hash, parent_hash, InvalidationReason::Hard).await;
                
                let _ = self.write_storage.remove_payload_by_block_hash(actual_hash);
                let latest_valid = self.chain.get_latest_valid_ancestor(parent_hash).await;
                return Ok(PayloadStatus {
                    status: PayloadStatusEnum::Invalid { validation_error: e.to_string() },
                    latest_valid_hash: latest_valid,
                });
            }
        }

        // 5. Execute block
        let parent_state_root = parent_header.as_ref().map(|h| h.state_root);
        let execution = Arc::clone(&self.execution);
        let block_clone = block.clone();
        
        let start_time = std::time::Instant::now();
        let exec_result = tokio::task::spawn_blocking(move || {
            execution.execute_block_with_state_root(block_clone, true, parent_state_root)
        }).await.map_err(|e| RpcError::Internal(format!("Execution task panicked: {}", e)))?;
        let elapsed = start_time.elapsed();
        info!("[PayloadProcessor] Block {} executed in {:?}", block.header.number, elapsed);

        exp!("[EXP] BLOCK_EXEC_END number={} hash={:?} elapsed_ms={}", block.header.number, actual_hash, elapsed.as_millis());

        
        let (final_block, receipts, metas) = match exec_result {
            Ok(res) => res,
            Err(e) => {
                error!("[PayloadProcessor] Execution failed for block {}: {}", block.header.number, e);
                
                // For execution errors (state dependent), we should be careful about blacklisting
                // if we might be on a side-branch or syncing.
                // We mark it as invalid to preserve the parent hash and allow walk-back.
                info!("[PayloadProcessor] Marking block {} as INVALID (execution failure)", actual_hash);
                self.chain.add_invalid_block(actual_hash, parent_hash, InvalidationReason::Hard).await;
                
                let _ = self.write_storage.remove_payload_by_block_hash(actual_hash);
                let latest_valid = self.chain.get_latest_valid_ancestor(parent_hash).await;
                return Ok(PayloadStatus {
                    status: PayloadStatusEnum::Invalid { validation_error: e.to_string() },
                    latest_valid_hash: latest_valid,
                });
            }
        };

        // 6. Post-execution validation
        let cumulative_gas_used = receipts.last().map(|r| r.receipt.cumulative_gas_used).unwrap_or(0);
        let logs_bloom = {
            let mut bloom = wasix_eth_types::Bloom::ZERO;
            for receipt in &receipts {
                bloom.accrue_bloom(&receipt.logs_bloom);
            }
            bloom
        };
        if let Err(e) = self.consensus.validate_block_post_execution(
            &final_block,
            cumulative_gas_used,
            proofs::calculate_receipt_root(&receipts),
            logs_bloom,
            final_block.header.state_root
        ) {
            error!("[PayloadProcessor] Post-execution validation failed for block {}: {}", block_number, e);
            
            info!("[PayloadProcessor] Marking block {} as INVALID (post-execution failure)", actual_hash);
            self.chain.add_invalid_block(actual_hash, parent_hash, InvalidationReason::Hard).await;
            
            let _ = self.write_storage.remove_payload_by_block_hash(actual_hash);
            let latest_valid = self.chain.get_latest_valid_ancestor(parent_hash).await;
            return Ok(PayloadStatus {
                status: PayloadStatusEnum::Invalid { validation_error: e },
                latest_valid_hash: latest_valid,
            });
        }

        // 7. Persist block
        let block_hash = final_block.header.hash_slow();
        let block_number = final_block.header.number;
        info!("[PayloadProcessor] Committing block {} (hash: {}) with {} transactions", block_number, block_hash, final_block.body.transactions.len());

        let storage = self.write_storage.clone();
        let final_block_clone = final_block.clone();
        let receipts_clone = receipts.clone();
        let metas_clone = metas.clone();
        
        let parent_td = if block_number == 0 {
            U256::ZERO
        } else {
            match self.read_storage.header_td(final_block.header.parent_hash) {
                Ok(Some(td)) => td,
                Ok(None) => return Err(RpcError::Internal(format!("Parent TD not found for block {} (parent: {})", block_number, final_block.header.parent_hash))),
                Err(e) => return Err(RpcError::Internal(format!("Failed to retrieve parent TD for block {}: {}", block_number, e))),
            }
        };
        let td = parent_td + U256::from(final_block.header.difficulty);

        tokio::task::spawn_blocking(move || -> wasix_eth_types::error::RpcResult<()> {
            let batch = storage.begin_batch().map_err(|e| RpcError::Internal(e.to_string()))?;

            batch.insert_header(block_hash, final_block_clone.header.clone()).map_err(|e| RpcError::Internal(e.to_string()))?;
            batch.insert_header_td(block_hash, td).map_err(|e| RpcError::Internal(e.to_string()))?;
            batch.insert_block_body(block_hash, block_number, final_block_clone.body.clone()).map_err(|e| RpcError::Internal(e.to_string()))?;
            batch.insert_block_hash(block_hash, block_number).map_err(|e| RpcError::Internal(e.to_string()))?;

            // Persist receipts and transaction lookup
            for (i, tx) in final_block_clone.body.transactions.iter().enumerate() {
                let tx_hash = *tx.hash();
                batch.insert_transaction(tx_hash, tx.clone()).map_err(|e| RpcError::Internal(e.to_string()))?;
                if let Some(receipt) = receipts_clone.get(i) {
                    batch.insert_receipt(block_hash, i as u64, receipt.clone()).map_err(|e| RpcError::Internal(e.to_string()))?;
                }
                if let Some(meta) = metas_clone.get(i) {
                    batch.insert_receipt_meta(block_hash, i as u64, meta.clone()).map_err(|e| RpcError::Internal(e.to_string()))?;
                }
                batch.insert_transaction_lookup(tx_hash, block_hash, i as u64).map_err(|e| RpcError::Internal(e.to_string()))?;
            }

            // Also remove payload as it's now officially a block
            let _ = batch.remove_payload_by_block_hash(block_hash);

            batch.commit().map_err(|e| RpcError::Internal(e.to_string()))?;
            info!("[PayloadProcessor] Block {} (hash: {}) successfully committed to storage", block_number, block_hash);
            Ok(())
        }).await.map_err(|e| RpcError::Internal(format!("Storage task panicked: {}", e)))??;

        let _ = self.event_tx.send(EngineEvent::NewBlock {
            block: final_block.clone(),
            is_local,
        });

        // If this block was previously marked as invalid (e.g. due to bad parameters in a previous call),
        // we should remove it from the invalid blocks list now that we've successfully validated and imported it.
        self.chain.remove_invalid_block(actual_hash).await;

        Ok(PayloadStatus {
            status: PayloadStatusEnum::Valid,
            latest_valid_hash: Some(actual_hash),
        })
    }

    pub fn revalidate_dependent_payloads(&self, initial_parent_hash: B256) -> BoxFuture<'_, ()> {
        async move {
            let mut parents_to_process = vec![initial_parent_hash];
            
            while let Some(parent_hash) = parents_to_process.pop() {
                let children_hashes = self.block_tree.remove_children(parent_hash).await;
                
                if !children_hashes.is_empty() {
                    info!("[PayloadProcessor] Found {} dependent payloads for parent {:?}", children_hashes.len(), parent_hash);
                    for child_hash in children_hashes {
                        let block = self.block_tree.remove_block(child_hash).await;
                        
                        if let Some(block) = block {
                            // Mark as processing to avoid concurrent imports of the same block
                            if self.processing_payloads.contains(&child_hash) {
                                debug!("[PayloadProcessor] Child block {:?} is already being processed, skipping revalidation", child_hash);
                                continue;
                            }
                            self.processing_payloads.insert(child_hash);

                            let result = self.new_payload_internal_inner(block.clone(), child_hash, None, None, false).await;

                            self.processing_payloads.remove(&child_hash);

                            match result {
                                Ok(status) => {
                                    if status.status == PayloadStatusEnum::Valid {
                                        info!("[PayloadProcessor] Successfully revalidated child block {} (hash: {})", block.header.number, child_hash);
                                        parents_to_process.push(child_hash);
                                    }
                                }

                                Err(e) => {
                                    error!("[PayloadProcessor] Failed to revalidate dependent payload {:?}: {}", child_hash, e);
                                }
                            }
                        }
                    }
                }
            }
        }.boxed()
    }

    pub fn invalidate_descendants(&self, initial_invalid_hash: B256) -> BoxFuture<'_, ()> {
        async move {
            let mut parents_to_invalidate = vec![initial_invalid_hash];
            let mut blocks_to_remove = Vec::new(); // Track blocks to delete safely
            
            while let Some(parent_hash) = parents_to_invalidate.pop() {
                let children_hashes = self.block_tree.remove_children(parent_hash).await;
                
                if !children_hashes.is_empty() {
                    debug!("[PayloadProcessor] Invalidating {} dependent payloads for invalid parent {:?}", children_hashes.len(), parent_hash);
                    for child_hash in children_hashes {
                        // Mark as invalid in chain manager immediately
                        self.chain.add_invalid_block(child_hash, parent_hash, InvalidationReason::Hard).await;
                        
                        // Push for recursion and tracking
                        parents_to_invalidate.push(child_hash);
                        blocks_to_remove.push(child_hash);
                    }
                }
            }

            // Remove blocks from the tree after the traversal is safely complete
            for hash in blocks_to_remove {
                self.block_tree.remove_block(hash).await;
            }
        }.boxed()
    }
}