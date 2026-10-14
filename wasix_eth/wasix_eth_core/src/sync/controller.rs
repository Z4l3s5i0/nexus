use crate::mempool::mempool_provider::MempoolProvider;
use crate::sync::downloader::Downloader;
use crate::sync::processor::BlockProcessor;
use crate::sync::registry::SyncRegistry;
use alloy_rpc_types::engine::ForkchoiceState;
use alloy_rpc_types::SyncInfo;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::{BlockProvider, ChainProvider, TransactionProvider};
use wasix_eth_storage::HeaderProvider;
use wasix_eth_types::sync::{PeerProvider, SyncProvider};
use wasix_eth_types::{async_trait, Block, ChainConfig, ChainManager, Hardfork, InvalidationReason, SyncStatus, Transaction, B256, U256};
use wasix_eth_utils::metrics::{CURRENT_HEAD_BLOCK, SYNC_STATUS, SYNC_TARGET_HEIGHT};
use wasix_eth_utils::{debug, error, info, warn};

pub struct SyncController {
    read_storage: DatabaseReadProvider,
    mempool: Arc<dyn MempoolProvider>,
    downloader: Downloader,
    processor: BlockProcessor,
    chain_manager: Arc<dyn ChainManager>,
    sync_registry: Arc<SyncRegistry>,
    sync_lock: tokio::sync::Mutex<()>,
    seen_blocks: tokio::sync::Mutex<HashSet<B256>>,
    seen_txs: tokio::sync::Mutex<HashSet<B256>>,
}

impl SyncController {
    pub fn new(
        read_storage: DatabaseReadProvider,
        peer_provider: Arc<dyn PeerProvider>,
        processor: BlockProcessor,
        chain_manager: Arc<dyn ChainManager>,
        sync_registry: Arc<SyncRegistry>,
        mempool: Arc<dyn MempoolProvider>,
    ) -> Self {
        Self {
            read_storage: read_storage.clone(),
            mempool,
            downloader: Downloader::new(peer_provider),
            processor,
            chain_manager,
            sync_registry,
            sync_lock: tokio::sync::Mutex::new(()),
            seen_blocks: tokio::sync::Mutex::new(HashSet::new()),
            seen_txs: tokio::sync::Mutex::new(HashSet::new()),
        }
    }

    pub async fn start(&self) {
        info!("[Sync] Starting synchronization controller...");
        let notify = self.sync_registry.subscribe();
        loop {
            // Process all pending targets in a loop to catch up quickly
            loop {
                match self.sync_step().await {
                    Ok(_) => {
                        // If we have more targets, continue processing immediately
                        if !self.sync_registry.has_targets().await {
                            break;
                        }
                    }
                    Err(e) => {
                        error!("[Sync] Sync step failed: {}", e);
                        break;
                    }
                }
            }

            // Wait for 1s OR for a new target notification
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(1)) => {},
                _ = notify.notified() => {
                    debug!("[Sync] Notified of new sync target, waking up...");
                }
            }
        }
    }

    async fn sync_step(&self) -> anyhow::Result<()> {
        let (_local_hash, local_height) = self.chain_manager.head_block().await;
        let local_td = self.chain_manager.total_difficulty().await;

        CURRENT_HEAD_BLOCK.set(local_height as f64);
        

        let _lock = match self.sync_lock.try_lock() {
            Ok(lock) => lock,
            Err(_) => {
                debug!("[Sync] Sync already in progress, skipping step");
                return Ok(());
            }
        };

        if let Some(target) = self.sync_registry.pop_target().await {
            let target_hash = target.hash;
            let affinity_peer = target.peer_id;
            let retries = target.retries;

            // B256::ZERO is used by trigger_sync to check for better peers.
            // If it's not ZERO, we are looking for a specific block.
            if target_hash != B256::ZERO {
                debug!("[Sync] Attempting to sync specific target {:?} (affinity: {:?}, retries: {})", target_hash, affinity_peer, retries);
                
                let mut sync_peer: Option<String> = None;
                if let Some(peer_id) = affinity_peer.as_ref() {
                    if let Some(_session) = self.downloader.peer_provider.get_session(peer_id).await {
                        sync_peer = Some(peer_id.clone());
                    }
                }
                
                if sync_peer.is_none() {
                    if let Some((best_peer_id, _, _)) = self.downloader.get_best_peer().await {
                        sync_peer = Some(best_peer_id);
                    }
                }
                // If still none, pick any active peer to try fetching the specific target
                if sync_peer.is_none() {
                    if let Some(any_peer_id) = self.downloader.get_any_peer().await {
                        sync_peer = Some(any_peer_id);
                    }
                }

                if let Some(peer_id) = sync_peer {
                    if let Err(e) = self.fetch_ancestors(&peer_id, target_hash, target_hash).await {
                        error!("[Sync] Failed to fetch target ancestor {:?}: {}", target_hash, e);
                        
                        if retries < 5 {
                            // Put it back if it's a transient error like a timeout
                            let err_str = e.to_string();
                            if err_str.contains("timed out") || err_str.contains("Session closed") || err_str.contains("channel closed") {
                                debug!("[Sync] Transient error, putting target {:?} back in registry (retry {})", target_hash, retries + 1);
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                self.sync_registry.add_target_with_retries(target_hash, None, retries + 1, self.chain_manager.clone()).await;
                            } else {
                                // Other error, maybe not found?
                                debug!("[Sync] Non-transient error fetching target {:?}, retrying anyway (retry {})", target_hash, retries + 1);
                                self.sync_registry.add_target_with_retries(target_hash, None, retries + 1, self.chain_manager.clone()).await;
                            }
                        } else {
                            warn!("[Sync] Maximum retries reached for target {:?}, dropping it", target_hash);
                        }
                    }
                } else {
                    debug!("[Sync] No peers found for target {:?}, triggering broadened discovery", target_hash);
                    // Put it back if we haven't reached max retries
                    if retries < 5 {
                        self.sync_registry.add_target_with_retries(target_hash, affinity_peer, retries + 1, self.chain_manager.clone()).await;
                    } else {
                        warn!("[Sync] Maximum retries reached for target {:?} without finding peers, dropping it", target_hash);
                    }
                }
            }
        }

        let best_peer = self.downloader.get_best_peer().await;
        if let Some((best_peer_id, best_td, best_height)) = best_peer {
            let is_better = if best_td > local_td {
                true
            } else if best_td == local_td && best_td > U256::ZERO {
                best_height > local_height
            } else if best_td == U256::ZERO {
                // For Eth69, assume better if height is higher
                best_height > local_height
            } else {
                false
            };

            if is_better {
                debug!("[Sync] Found better peer {} with TD {} height {} (local TD: {}, height: {})", 
                    best_peer_id, best_td, best_height, local_td, local_height);
                
                SYNC_TARGET_HEIGHT.set(best_height as f64);

                self.sync_range(&best_peer_id, local_height + 1).await?;
            } else {
                // Not actively syncing a range, set to None so ChainManagerImpl can return dynamic Info.
                self.chain_manager.set_sync_status(SyncStatus::None).await;
                SYNC_STATUS.set(1.0); // 1: synced
                SYNC_TARGET_HEIGHT.set(local_height as f64);
            }
        } else {
            debug!("[Sync] No peers found with better TD or height (local TD: {}, local height: {})", local_td, local_height);
        }

        Ok(())
    }

    async fn sync_range(&self, peer_id: &str, start: u64) -> anyhow::Result<()> {
        const BATCH_SIZE: u64 = 128;
        debug!("[Sync] Starting sync range from header {}", start);
        let local_height = self.read_storage.latest_block_number()?.unwrap_or(0);
        
        let chain_config = self.read_storage.chain_config().ok().flatten().unwrap_or_else(|| ChainConfig {
            chain_id: self.read_storage.chain_id().unwrap_or(1),
            ..Default::default()
        });

        // 1. Download headers in bulk
        let headers = match self.downloader.download_headers(peer_id, start, BATCH_SIZE).await {
            Ok(h) => h,
            Err(e) => {
                error!("[Sync] Failed to download headers from {}: {}", peer_id, e);
                self.chain_manager.set_sync_status(SyncStatus::None).await;
                return Err(e);
            }
        };

        if headers.is_empty() {
            debug!("[Sync] No new headers from peer {}", peer_id);
            return Ok(());
        }

        let end = headers.last().unwrap().number;

        self.chain_manager.set_sync_status(SyncStatus::Info(Box::new(SyncInfo {
            current_block: U256::from(start),
            highest_block: U256::from(end),
            starting_block: U256::from(local_height),
            warp_chunks_amount: None,
            warp_chunks_processed: None,
            stages: None,
        }))).await;
        SYNC_STATUS.set(0.0);

        // 2. Validate header chain (basic link check)
        for i in 1..headers.len() {
            if headers[i].parent_hash != headers[i-1].hash_slow() {
                self.chain_manager.set_sync_status(SyncStatus::None).await;
                return Err(anyhow::anyhow!("Downloaded header chain is not contiguous at height {}", headers[i].number));
            }
        }

        // 3. Download bodies in bulk
        let hashes: Vec<B256> = headers.iter().map(|h| h.hash_slow()).collect();
        let bodies = match self.downloader.download_bodies(peer_id, hashes.clone()).await {
            Ok(b) => b,
            Err(e) => {
                error!("[Sync] Failed to download bodies from {}: {}", peer_id, e);
                self.chain_manager.set_sync_status(SyncStatus::None).await;
                return Err(e);
            }
        };

        if bodies.len() != headers.len() {
            self.chain_manager.set_sync_status(SyncStatus::None).await;
            return Err(anyhow::anyhow!("Downloaded {} bodies for {} headers", bodies.len(), headers.len()));
        }

        let mut last_imported_block = None;
        let mut last_imported_timestamp = 0;
        // 4. Process blocks
        for (header, body) in headers.into_iter().zip(bodies.into_iter()) {
            let block_num = header.number;
            let block_timestamp = header.timestamp;
            let block = Block { header, body };
            let block_hash = block.header.hash_slow();

            // Skip if we already have this block
            if self.chain_manager.has_block(block_hash).await {
                debug!("[Sync] Skipping already known block {} ({})", block_num, block_hash);
                last_imported_block = Some((block_hash, block_num));
                last_imported_timestamp = block_timestamp;
                continue;
            }

            if let Err(e) = self.processor.process_block(block.clone()).await {
                error!("[Sync] Failed to process block {}: {}", block_num, e);
                self.chain_manager.set_sync_status(SyncStatus::None).await;
                return Err(e);
            }
            
            // Check if we should abort sync due to a reorg triggered by Engine API
            let (current_head_hash, _) = self.chain_manager.head_block().await;
            let active_fork = Hardfork::get_active_fork(&chain_config, block_num, block_timestamp);
            if active_fork >= Hardfork::Paris && current_head_hash != B256::ZERO {
                 // In post-merge, if the current canonical head is NOT an ancestor of what we are syncing,
                 // it might mean the CL shifted to a different branch.
                 // However, we are just "filling" the block tree/storage here, so we can continue
                 // unless the block we just processed was marked invalid.
            }

            last_imported_block = Some((block_hash, block_num));
            last_imported_timestamp = block_timestamp;
        }
        
        // 5. Update forkchoice to the latest imported block
        if let Some((hash, number)) = last_imported_block {
            // Post‑Merge rule: Do NOT alter canonical head from sync ranges.
            // In Paris/Shanghai and later, forkchoice is dictated by the CL via Engine API.
            let active_fork = Hardfork::get_active_fork(&chain_config, number, last_imported_timestamp);
            if active_fork >= Hardfork::Paris {
                debug!(
                    "[Sync] Synced post‑merge range up to block #{} ({}); head update deferred to Engine API",
                    number, hash
                );
                self.chain_manager.set_sync_status(SyncStatus::None).await;
                return Ok(());
            }

            let state = ForkchoiceState {
                head_block_hash: hash,
                safe_block_hash: hash,
                finalized_block_hash: B256::ZERO,
            };

            let version = if active_fork >= Hardfork::Cancun { 3 } else { 2 };

            match self
                .processor
                .engine
                .forkchoice_updated(state, None, version)
                .await
            {
                Ok(_) => info!(
                    "[Sync] Successfully updated forkchoice (V{}) to block {} ({})",
                    version, number, hash
                ),
                Err(e) => error!("[Sync] Failed to update forkchoice after sync range: {}", e),
            }
        }

        self.chain_manager.set_sync_status(SyncStatus::None).await;
        Ok(())
    }

    async fn fetch_ancestors(&self, peer_id: &str, hash: B256, requested_head: B256) -> anyhow::Result<()> {
        let mut current_hash = hash;
        let mut to_process: Vec<Block<Transaction>> = Vec::new();

        let chain_config = self.read_storage.chain_config().ok().flatten().unwrap_or_else(|| ChainConfig {
            chain_id: self.read_storage.chain_id().unwrap_or(1),
            ..Default::default()
        });

        while current_hash != B256::ZERO {
            // Check if we already have it
            if self.read_storage.block_by_hash(current_hash)?.is_some() {
                break;
            }

            // Check if it's known to be invalid
            if let Some(reason) = self.chain_manager.get_invalidation_reason(current_hash).await {
                if reason == InvalidationReason::Hard {
                    if requested_head != current_hash {
                        // Mark the requested head as invalid with its actual parent hash if we have it
                        // to allow walking back to find the latest valid ancestor.
                        // If we don't have it in the to_process vector yet, we'll try to find it.
                        let mut head_parent = B256::ZERO;
                        for b in &to_process {
                            if b.header.hash_slow() == requested_head {
                                head_parent = b.header.parent_hash;
                                break;
                            }
                        }
                        self.chain_manager.add_invalid_block(requested_head, head_parent, InvalidationReason::Hard).await;
                    }
                    
                    // Clear sync targets since we've reached a known invalid chain
                    self.sync_registry.clear_targets().await;

                    return Err(anyhow::anyhow!("Ancestor block {:?} is known to be invalid (Hard)", current_hash));
                } else {
                    debug!("[Sync] Ancestor block {:?} was marked as Soft invalid, attempting retry", current_hash);
                    self.chain_manager.remove_invalid_block(current_hash).await;
                }
            }

            debug!("[Sync] Fetching ancestor block {:?}", current_hash);
            // Try the affinity/best peer first; if it fails, try other active peers
            let mut fetched_block = self.downloader.download_block_by_hash(peer_id, current_hash).await;
            if fetched_block.is_err() {
                debug!("[Sync] Primary peer {} did not return block {:?}, trying other peers", peer_id, current_hash);
                if let Ok(peers) = self.downloader.peer_provider.get_active_peers().await {
                    for p in peers.iter().filter(|p| p.peer_id != *peer_id) {
                        if let Ok(block) = self.downloader.download_block_by_hash(&p.peer_id, current_hash).await {
                            fetched_block = Ok(block);
                            break;
                        }
                    }
                }
            }

            let block = fetched_block.map_err(|e| anyhow::anyhow!("Failed to fetch block {:?} from any peer: {}", current_hash, e))?;
            let parent_hash = block.header.parent_hash;
            to_process.push(block);
            current_hash = parent_hash;

            if to_process.len() > 100 {
                debug!("[Sync] Ancestor chain too long, processing current batch");
                break;
            }
        }

        // Process in reverse order (oldest to newest)
        let mut last_processed_hash = None;
        let mut last_processed_num = 0;
        let mut last_processed_timestamp = 0;

        for block in to_process.clone().into_iter().rev() {
            let block_num = block.header.number;
            let block_hash = block.header.hash_slow();
            let block_timestamp = block.header.timestamp;

            // Skip if we already have this block
            if self.chain_manager.has_block(block_hash).await {
                debug!("[Sync] Skipping already known ancestor block {} ({})", block_num, block_hash);
                last_processed_hash = Some(block_hash);
                last_processed_num = block_num;
                last_processed_timestamp = block_timestamp;
                continue;
            }

            if let Err(e) = self.processor.process_block(block.clone()).await {
                error!("[Sync] Failed to process ancestor block {}: {}", block_num, e);
                
                // If the block is invalid, mark the requested head as invalid as well.
                // Note: process_block already marks block_hash as invalid with its correct parent.
                if requested_head != block_hash {
                    // We mark the requested head as invalid with its ACTUAL parent hash
                    // so that get_latest_valid_ancestor can walk back correctly.
                    let mut head_parent = block.header.parent_hash; // default to current failed block's parent if we can't find better
                    for b in &to_process {
                        if b.header.hash_slow() == requested_head {
                            head_parent = b.header.parent_hash;
                            break;
                        }
                    }
                    self.chain_manager.add_invalid_block(requested_head, head_parent, InvalidationReason::Hard).await;
                }
                self.sync_registry.clear_targets().await;
                
                return Err(anyhow::anyhow!("Ancestor block processing failed: {}", e));
            }
            last_processed_hash = Some(block_hash);
            last_processed_num = block_num;
            last_processed_timestamp = block_timestamp;
        }

        // Update forkchoice if we processed any blocks
        if let Some(hash) = last_processed_hash {
            // Post‑Merge rule: Do NOT alter canonical head from ancestor fetching.
            // In Paris/Shanghai and later, forkchoice is dictated by the CL via Engine API.
            let active_fork =
                Hardfork::get_active_fork(&chain_config, last_processed_num, last_processed_timestamp);
            if active_fork >= Hardfork::Paris {
                debug!(
                    "[Sync] Processed post‑merge ancestor block #{} ({}); head update deferred to Engine API",
                    last_processed_num, hash
                );
                return Ok(());
            }

            let state = ForkchoiceState {
                head_block_hash: hash,
                safe_block_hash: hash,
                finalized_block_hash: B256::ZERO,
            };

            let version = if active_fork >= Hardfork::Cancun { 3 } else { 2 };

            if let Err(e) = self
                .processor
                .engine
                .forkchoice_updated(state, None, version)
                .await
            {
                error!(
                    "[Sync] Failed to update forkchoice (V{}) after fetching ancestors to block {}: {}",
                    version, last_processed_num, e
                );
            }
        }

        Ok(())
    }
}

#[async_trait]
impl SyncProvider for SyncController {
    async fn status(&self) -> SyncStatus {
        self.chain_manager.sync_status().await
    }

    async fn trigger_sync(&self) -> anyhow::Result<()> {
        debug!("[Sync] Manual sync trigger received");
        let result = self.sync_step().await;
        if result.is_ok() {
            debug!("[Sync] Manual sync step completed successfully");
        } else {
            error!("[Sync] Manual sync step failed: {:?}", result.as_ref().err());
        }
        result
    }

    async fn has_block(&self, hash: B256) -> bool {
        self.chain_manager.has_block(hash).await
    }

    async fn get_block_by_hash(&self, hash: B256) -> anyhow::Result<Option<Block<Transaction>>> {
        if let Ok(Some(block)) = self.read_storage.block_by_hash(hash) {
            return Ok(Some(block));
        }
        let block = self.processor.engine.payload_processor.block_tree.get_block(hash).await;
        if block.is_some() {
            debug!("[SyncController] Found block {:?} in block tree", hash);
        }
        Ok(block)
    }

    async fn process_gossip_transactions(&self, txs: Vec<Transaction>) -> anyhow::Result<()> {
        for tx in txs {
            let _ = self.processor.process_transaction(tx).await;
        }
        Ok(())
    }

    async fn process_pooled_transactions(&self, txs: Vec<wasix_eth_types::TxPooledEnvelope>) -> wasix_eth_types::Result<()> {
        for tx in txs {
            let _ = self.processor.process_pooled_transaction(tx).await;
        }
        Ok(())
    }

    async fn handle_announced_pooled_transactions(&self, peer_id: String, hashes: Vec<B256>) -> wasix_eth_types::Result<()> {
        if self.downloader.peer_provider.get_session(&peer_id).await.is_none() {
            debug!("[Sync] Skipping pooled transaction announcement from disconnected peer {}", peer_id);
            return Ok(());
        }

        let mut hashes_to_download = Vec::with_capacity(hashes.len());
        {
            let mut seen = self.seen_txs.lock().await;
            for hash in hashes {
                // Skip if already in mempool
                if self.mempool.get_transaction(hash).await.is_some() {
                    continue;
                }
                // Skip if already on-chain
                if self.read_storage.transaction(hash).unwrap_or_default().is_some() {
                    continue;
                }
                // Skip if we've seen it recently
                if !seen.insert(hash) {
                    continue;
                }
                hashes_to_download.push(hash);
            }
        }

        if hashes_to_download.is_empty() {
            return Ok(());
        }

        let txs = self.downloader.download_pooled_transactions(&peer_id, hashes_to_download.clone()).await
            .map_err(|e| wasix_eth_types::error::RpcError::Internal(format!("Failed to download pooled transactions: {}", e)))?;
        
        info!("[Sync] Downloaded {} pooled transactions from {} (requested {})", txs.len(), peer_id, hashes_to_download.len());
        self.process_pooled_transactions(txs).await
    }
}
