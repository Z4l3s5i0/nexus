use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::{BlockProvider, HeaderProvider};
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_types::{async_trait, Block, BlockId, SyncStatus, Transaction, B256, U256, ReorgContext, ChainManager, InvalidationReason};
use wasix_eth_utils::{warn, info, debug};
use crate::engine::canonicality_tracker::CanonicalState;
pub(crate) use crate::engine::sidechain_tracker::BlockTree;
use crate::engine::reorg_manager::ReorgHandler;
use std::sync::Arc;
use std::collections::HashSet;


pub struct ChainManagerImpl {
    read_storage: DatabaseReadProvider,
    _write_storage: DatabaseWriteProvider,
    sync_status: tokio::sync::RwLock<SyncStatus>,
    pub canonical: Arc<CanonicalState>,
    pub block_tree: Arc<BlockTree>,
    pub reorg_handler: Arc<ReorgHandler>,
}

impl ChainManagerImpl {
    pub fn new(
        read_storage: DatabaseReadProvider,
        write_storage: DatabaseWriteProvider,
        canonical: Arc<CanonicalState>,
        block_tree: Arc<BlockTree>,
        reorg_handler: Arc<ReorgHandler>,
    ) -> Self {
        Self {
            read_storage,
            _write_storage: write_storage,
            sync_status: tokio::sync::RwLock::new(SyncStatus::None),
            canonical,
            block_tree,
            reorg_handler,
        }
    }
}

#[async_trait]
impl ChainManager for ChainManagerImpl {
    async fn sync_status(&self) -> SyncStatus {
        let status = self.sync_status.read().await.clone();
        
        if let SyncStatus::Info(ref info) = status {
            if info.stages.is_none() {
                return status;
            }
        }
        
        let (_hash, number) = self.head_block().await;
        SyncStatus::Info(Box::new(alloy_rpc_types::SyncInfo {
            current_block: U256::from(number),
            highest_block: U256::from(number),
            starting_block: U256::ZERO,
            stages: Some(Vec::new()),
            warp_chunks_amount: None,
            warp_chunks_processed: None,
        }))
    }

    async fn set_sync_status(&self, status: SyncStatus) {
        let mut current = self.sync_status.write().await;
        *current = status;
    }

    async fn head_block(&self) -> (B256, u64) {
        self.canonical.get_head()
    }

    async fn set_head_block(&self, hash: B256, number: u64) {
        let _ = self.canonical.update_head(hash, number).await;
    }

    async fn has_block(&self, hash: B256) -> bool {
        // Check if header exists in storage
        match self.read_storage.header(BlockId::Hash(hash.into())).await {
            Ok(Some(_)) => return true,
            Ok(None) => {},
            Err(e) => {
                debug!("[ChainManager] Error reading header from storage for {:?}: {}", hash, e);
            }
        }
        
        // Check if block exists in block_tree (validated but not yet canonical/persistent)
        if self.block_tree.contains_block(hash).await {
            return true;
        }

        // Check payload map (persistent but not yet canonical)
        if self.read_storage.get_payload_by_block_hash(hash).await.is_some() {
            return true;
        }

        false
    }

    async fn is_invalid(&self, hash: B256) -> bool {
        self.block_tree.get_invalidation_reason(hash).await.is_some()
    }

    async fn get_invalidation_reason(&self, hash: B256) -> Option<InvalidationReason> {
        self.block_tree.get_invalidation_reason(hash).await
    }

    async fn add_invalid_block(&self, hash: B256, parent_hash: B256, reason: InvalidationReason) {
        self.block_tree.mark_invalid(hash, parent_hash, reason).await;
    }

    async fn remove_invalid_block(&self, hash: B256) {
        self.block_tree.remove_invalid(hash).await;
    }

    async fn get_latest_valid_ancestor(&self, hash: B256) -> Option<B256> {
        if hash == B256::ZERO {
            return Some(B256::ZERO);
        }

        // Fast path: if it's canonical, it's valid
        if let Ok(true) = self.read_storage.is_canonical(hash).await {
            return Some(hash);
        }
        // If it's the current head from FCU, and not known invalid, it's a valid candidate
        let (head_hash, _) = self.head_block().await;
        if hash == head_hash && self.block_tree.get_invalidation_reason(hash).await.is_none() {
            return Some(hash);
        }

        let mut current = hash;
        let mut visited = HashSet::new();

        // If the starting hash is not in invalid_blocks, check if it's valid itself.
        if self.block_tree.get_invalidation_reason(current).await.is_none() {
            if self.has_block(current).await {
                return Some(current);
            }
            if let Ok(Some(genesis_hash)) = self.read_storage.block_hash(0).await {
                if current == genesis_hash {
                    return Some(current);
                }
            }
        }

        // Walk back through invalid blocks
        while let Some(reason) = self.block_tree.get_invalidation_reason(current).await {
            debug!("[ChainManager] get_latest_valid_ancestor: walking back from invalid block {:?} (reason: {:?})", current, reason);
            if !visited.insert(current) {
                warn!("Cycle detected in invalid_blocks at hash {:?}", current);
                return None;
            }

            // if we can find the height of the current invalid block,
            // we might be able to find a canonical ancestor faster.
            let current_number = if let Some(block) = self.block_tree.get_block(current).await {
                Some(block.header.number)
            } else if let Ok(Some(header)) = self.read_storage.header(BlockId::Hash(current.into())).await {
                Some(header.number)
            } else if let Some((payload, _, _, _)) = self.read_storage.get_payload_by_block_hash(current).await {
                Some(payload.header.number)
            } else {
                None
            };

            if let Some(num) = current_number {
                // Find the highest canonical block below this invalid block
                // and check if it's an ancestor.
                if let Ok(Some(canonical_hash)) = self.read_storage.block_hash(num.saturating_sub(1) as u64).await {
                    // Check if this canonical block is an ancestor of 'current'
                    if self.is_ancestor(current, canonical_hash).await {
                         debug!("[ChainManager] get_latest_valid_ancestor: found canonical ancestor {:?} via is_ancestor check at height {}", canonical_hash, num.saturating_sub(1));
                         return Some(canonical_hash);
                    }
                }
            }

            let mut parent_hash = self.block_tree.get_invalid_parent(current).await;

            // Fallback: If not explicitly in invalid_parents map, try to find the block's parent hash from tree or storage
            if parent_hash.is_none() {
                if let Some(block) = self.block_tree.get_block(current).await {
                    parent_hash = Some(block.header.parent_hash);
                } else if let Ok(Some(header)) = self.read_storage.header(BlockId::Hash(current.into())).await {
                    parent_hash = Some(header.parent_hash);
                } else if let Some((block, _, _, _)) = self.read_storage.get_payload_by_block_hash(current).await {
                    parent_hash = Some(block.header.parent_hash);
                }
            }

            if let Some(parent) = parent_hash {
                if parent == current {
                    warn!("Block {:?} is its own parent in invalid chain! Breaking.", current);
                    return None;
                }
                
                // if parent is canonical, it MUST be valid and not invalid.
                if let Ok(true) = self.read_storage.is_canonical(parent).await {
                    debug!("[ChainManager] get_latest_valid_ancestor: reached canonical block {:?} during walk", parent);
                    return Some(parent);
                }

                current = parent;
            } else {
                // We reached the end of the invalid chain but don't know the parent.
                debug!("[ChainManager] get_latest_valid_ancestor: reached end of invalid chain at {:?}, parent unknown", current);
                return None;
            }

            if visited.len() >= 1024 {
                warn!("Ancestry walk limit reached for hash {:?}", hash);
                return None;
            }
        }

        // Now current is the first non-invalid block we found.
        if self.has_block(current).await {
            debug!("[ChainManager] get_latest_valid_ancestor: found valid ancestor {:?}", current);
            return Some(current);
        }

        // Final fallbacks
        if let Ok(Some(genesis_hash)) = self.read_storage.block_hash(0).await {
            if current == genesis_hash {
                return Some(current);
            }
        }

        if current == B256::ZERO {
            return Some(B256::ZERO);
        }

        debug!("[ChainManager] get_latest_valid_ancestor: block {:?} is not invalid but not known (missing from storage)", current);
        None
    }

    async fn total_difficulty(&self) -> U256 {
        let (head_hash, _) = self.head_block().await;
        self.read_storage.header_td(head_hash).await.ok().flatten().unwrap_or(U256::ZERO)
    }

    async fn determine_payload_status(&self, head_block_hash: B256) -> wasix_eth_types::PayloadStatus {
        if self.is_invalid(head_block_hash).await {
            let latest_valid = self.get_latest_valid_ancestor(head_block_hash).await;
            info!("[ChainManager] Block {:?} is known invalid. Latest valid ancestor: {:?}", head_block_hash, latest_valid);
            return wasix_eth_types::PayloadStatus {
                status: wasix_eth_types::PayloadStatusEnum::Invalid {
                    validation_error: "Block is known to be invalid".to_string(),
                },
                latest_valid_hash: latest_valid,
            };
        }

        if let Ok(Some(_)) = self.read_storage.header(BlockId::Hash(head_block_hash.into())).await {
            return wasix_eth_types::PayloadStatus {
                status: wasix_eth_types::PayloadStatusEnum::Valid,
                latest_valid_hash: Some(head_block_hash),
            };
        }

        // Check if it's in the payloads map (processed but not yet canonical)
        if let Some(_) = self.read_storage.get_payload_by_block_hash(head_block_hash).await {
            return wasix_eth_types::PayloadStatus {
                status: wasix_eth_types::PayloadStatusEnum::Valid,
                latest_valid_hash: Some(head_block_hash),
            };
        }

        wasix_eth_types::PayloadStatus {
            status: wasix_eth_types::PayloadStatusEnum::Syncing,
            latest_valid_hash: None,
        }
    }

    async fn is_ancestor(&self, head: B256, target: B256) -> bool {
        if head == target {
            return true;
        }
        if target == B256::ZERO {
            return true;
        }

        // Get block numbers to optimize search
        let head_number = if let Ok(Some(n)) = self.read_storage.block_number(head).await {
            Some(n)
        } else if let Some(block) = self.block_tree.get_block(head).await {
            Some(block.header.number)
        } else if let Some((payload, _, _, _)) = self.read_storage.get_payload_by_block_hash(head).await {
            Some(payload.header.number)
        } else {
            None
        };

        let target_number = if let Ok(Some(n)) = self.read_storage.block_number(target).await {
            Some(n)
        } else if let Some(block) = self.block_tree.get_block(target).await {
            Some(block.header.number)
        } else if let Some((payload, _, _, _)) = self.read_storage.get_payload_by_block_hash(target).await {
            Some(payload.header.number)
        } else {
            None
        };

        if let (Some(h_num), Some(t_num)) = (head_number, target_number) {
            if t_num > h_num {
                return false;
            }

            // Optimization: if target is canonical, we just check if head's ancestor at t_num is target
            if let Ok(true) = self.read_storage.is_canonical(target).await {
                // If head is also canonical, it's an ancestor if head_number >= target_number
                if let Ok(true) = self.read_storage.is_canonical(head).await {
                    return h_num >= t_num;
                }
                // If head is not canonical, we still need to check if its ancestor at t_num is target
            }
        }

        let mut current = head;
        let mut current_number = head_number;

        // Search up to 2048 blocks back
        for _ in 0..2048 {
            if current == target {
                return true;
            }
            if current == B256::ZERO {
                break;
            }

            // If we know both numbers and current is already below target, it's not an ancestor
            if let (Some(c_num), Some(t_num)) = (current_number, target_number) {
                if c_num < t_num {
                    return false;
                }
                
                // If current is canonical, we can do a fast check
                if let Ok(true) = self.read_storage.is_canonical(current).await {
                    if let Ok(Some(canonical_hash_at_t_num)) = self.read_storage.block_hash(t_num).await {
                        return canonical_hash_at_t_num == target;
                    }
                }
            }

            let parent_info = if let Ok(Some(header)) = self.read_storage.header(wasix_eth_types::BlockId::Hash(current.into())).await {
                Some((header.parent_hash, header.number.saturating_sub(1)))
            } else if let Some((payload, _, _, _)) = self.read_storage.get_payload_by_block_hash(current).await {
                Some((payload.header.parent_hash, payload.header.number.saturating_sub(1)))
            } else if let Some(block) = self.block_tree.get_block(current).await {
                Some((block.header.parent_hash, block.header.number.saturating_sub(1)))
            } else {
                let genesis_hash = self.read_storage.block_hash(0).await.unwrap_or(None);
                if Some(current) == genesis_hash {
                    Some((B256::ZERO, 0))
                } else {
                    None
                }
            };

            if let Some((parent, p_num)) = parent_info {
                current = parent;
                current_number = Some(p_num);
            } else {
                break;
            }
        }
        false
    }

    async fn revert_to_height(&self, height: u64) -> wasix_eth_types::Result<()> {
        self.reorg_handler.revert_to_height(height).await
    }

    async fn resolve_reorg(&self, old_head: B256, new_head: B256) -> wasix_eth_types::Result<ReorgContext> {
        self.reorg_handler.resolve_reorg(old_head, new_head).await
    }

    async fn mark_branch_canonical(&self, blocks: &Vec<Block<Transaction>>) -> wasix_eth_types::Result<()> {
        self.reorg_handler.mark_branch_canonical(blocks).await
    }
}
