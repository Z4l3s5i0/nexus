use alloy_primitives::B256;
use std::collections::HashMap;
use tokio::sync::RwLock;
use wasix_eth_types::{Block, Transaction, InvalidationReason};

pub struct BlockTree {
    /// Memory pool for blocks not yet canonical or ancestors unknown
    blocks: RwLock<HashMap<B256, Block<Transaction>>>,
    /// Child index: parent_hash -> Vec<child_hash>
    children: RwLock<HashMap<B256, Vec<B256>>>,
    /// Known invalid blocks and their reason (Hard/Soft)
    invalid_blocks: RwLock<HashMap<B256, (B256, InvalidationReason)>>,
}

impl BlockTree {
    pub fn new() -> Self {
        Self {
            blocks: RwLock::new(HashMap::new()),
            children: RwLock::new(HashMap::new()),
            invalid_blocks: RwLock::new(HashMap::new()),
        }
    }

    pub async fn add_orphan(&self, parent_hash: B256, block: Block<Transaction>, _expected_blobs: Option<Vec<B256>>, _parent_beacon_root: Option<B256>) {
        let hash = block.header.hash_slow();
        self.blocks.write().await.insert(hash, block);
        // We could store expected_blobs and parent_beacon_root too if we had a struct for buffered block
        self.children.write().await.entry(parent_hash).or_default().push(hash);
    }

    pub async fn mark_invalid(&self, hash: B256, parent_hash: B256, reason: InvalidationReason) {
        if hash == parent_hash {
             wasix_eth_utils::error!("[BlockTree] Attempted to mark block {:?} as invalid with itself as parent!", hash);
             // We still store it but with ZERO parent to avoid cycles
             self.invalid_blocks.write().await.insert(hash, (B256::ZERO, reason));
        } else {
             self.invalid_blocks.write().await.insert(hash, (parent_hash, reason));
        }
        
        // Also ensure the block is removed from the orphan pool if it's there
        // but keep its parent link if we can't find it
        let mut blocks = self.blocks.write().await;
        if blocks.remove(&hash).is_none() {
             // Block was not in pool, but that's okay, we have the parent_hash from the argument
        }
    }

    pub async fn remove_invalid(&self, hash: B256) {
        self.invalid_blocks.write().await.remove(&hash);
    }

    pub async fn get_invalidation_reason(&self, hash: B256) -> Option<InvalidationReason> {
        self.invalid_blocks.read().await.get(&hash).map(|(_, reason)| *reason)
    }

    pub async fn get_invalid_parent(&self, hash: B256) -> Option<B256> {
        self.invalid_blocks.read().await.get(&hash).map(|(parent, _)| *parent)
    }

    pub async fn get_children(&self, parent_hash: B256) -> Vec<B256> {
        self.children.read().await.get(&parent_hash).cloned().unwrap_or_default()
    }

    pub async fn remove_children(&self, parent_hash: B256) -> Vec<B256> {
        self.children.write().await.remove(&parent_hash).unwrap_or_default()
    }

    pub async fn get_block(&self, hash: B256) -> Option<Block<Transaction>> {
        self.blocks.read().await.get(&hash).cloned()
    }

    pub async fn remove_block(&self, hash: B256) -> Option<Block<Transaction>> {
        self.blocks.write().await.remove(&hash)
    }

    pub async fn contains_block(&self, hash: B256) -> bool {
        self.blocks.read().await.contains_key(&hash)
    }

    pub async fn get_all_blocks(&self) -> Vec<Block<Transaction>> {
        self.blocks.read().await.values().cloned().collect()
    }
}