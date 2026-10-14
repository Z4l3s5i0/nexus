use alloy_primitives::B256;
use dashmap::DashMap;
use wasix_eth_types::{Block, InvalidationReason, Transaction};

pub struct BlockTree {
    /// Memory pool for blocks not yet canonical or ancestors unknown
    blocks: DashMap<B256, Block<Transaction>>,
    /// Child index: parent_hash -> Vec<child_hash>
    children: DashMap<B256, Vec<B256>>,
    /// Known invalid blocks and their reason (Hard/Soft)
    invalid_blocks: DashMap<B256, (B256, InvalidationReason)>,
}

impl BlockTree {
    pub fn new() -> Self {
        Self {
            blocks: DashMap::new(),
            children: DashMap::new(),
            invalid_blocks: DashMap::new(),
        }
    }

    pub async fn add_orphan(&self, parent_hash: B256, block: Block<Transaction>, _expected_blobs: Option<Vec<B256>>, _parent_beacon_root: Option<B256>) {
        let hash = block.header.hash_slow();
        self.blocks.insert(hash, block);
        self.children.entry(parent_hash).or_default().push(hash);
    }
    pub async fn mark_invalid(&self, hash: B256, parent_hash: B256, reason: InvalidationReason) {
        if hash == parent_hash {
            wasix_eth_utils::error!("[BlockTree] Attempted to mark block {:?} as invalid with itself as parent!", hash);
            // We still store it but with ZERO parent to avoid cycles
            self.invalid_blocks.insert(hash, (B256::ZERO, reason));
        } else {
            self.invalid_blocks.insert(hash, (parent_hash, reason));
        }

        // Also ensure the block is removed from the orphan pool if it's there
        self.blocks.remove(&hash);
    }

    pub async fn remove_invalid(&self, hash: B256) {
        self.invalid_blocks.remove(&hash);
    }

    pub async fn get_invalidation_reason(&self, hash: B256) -> Option<InvalidationReason> {
        self.invalid_blocks.get(&hash).map(|r| r.value().1)
    }

    pub async fn get_invalid_parent(&self, hash: B256) -> Option<B256> {
        self.invalid_blocks.get(&hash).map(|r| r.value().0)
    }

    pub async fn get_children(&self, parent_hash: B256) -> Vec<B256> {
        self.children.get(&parent_hash).map(|r| r.value().clone()).unwrap_or_default()
    }

    pub async fn remove_children(&self, parent_hash: B256) -> Vec<B256> {
        self.children.remove(&parent_hash).map(|(_, v)| v).unwrap_or_default()
    }

    pub async fn get_block(&self, hash: B256) -> Option<Block<Transaction>> {
        self.blocks.get(&hash).map(|r| r.value().clone())
    }

    pub async fn remove_block(&self, hash: B256) -> Option<Block<Transaction>> {
        self.blocks.remove(&hash).map(|(_, v)| v)
    }

    pub async fn contains_block(&self, hash: B256) -> bool {
        self.blocks.contains_key(&hash)
    }

    pub async fn get_all_blocks(&self) -> Vec<Block<Transaction>> {
        self.blocks.iter().map(|r| r.value().clone()).collect()
    }
}