use crate::{async_trait, Result, SyncStatus, TxEnvelope};
use alloy_consensus::Block;
use alloy_primitives::{B256, U256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InvalidationReason {
    /// Protocol violation, should never be retried.
    Hard,
    /// Transient error (e.g. missing blobs), can be retried.
    Soft,
}

#[derive(Debug, Clone)]
pub struct ReorgContext {
    pub common_ancestor_hash: B256,
    pub new_canonical_blocks: Vec<Block<TxEnvelope>>,
    pub is_reorg: bool,
}

#[async_trait]
pub trait ChainManager: Send + Sync {
    /// Returns the current synchronization status.
    async fn sync_status(&self) -> SyncStatus;

    /// Sets the current synchronization status.
    async fn set_sync_status(&self, status: SyncStatus);

    /// Returns the current head block (hash and number).
    async fn head_block(&self) -> (B256, u64);

    /// Sets the current head block.
    async fn set_head_block(&self, hash: B256, number: u64);

    /// Checks if the node has a block with the given hash.
    async fn has_block(&self, hash: B256) -> bool;

    /// Reverts the chain to a specific height.
    async fn revert_to_height(&self, height: u64) -> Result<()>;

    /// Checks if the given block hash is known to be invalid.
    async fn is_invalid(&self, hash: B256) -> bool;

    /// Gets the invalidation reason for a block.
    async fn get_invalidation_reason(&self, hash: B256) -> Option<InvalidationReason>;

    /// Marks a block as invalid and stores its parent hash.
    async fn add_invalid_block(&self, hash: B256, parent_hash: B256, reason: InvalidationReason);

    /// Removes a block from the invalid blocks list.
    async fn remove_invalid_block(&self, hash: B256);

    /// Returns the latest known valid ancestor of an invalid block.
    async fn get_latest_valid_ancestor(&self, hash: B256) -> Option<B256>;

    /// Retrieves the total difficulty of the head block.
    async fn total_difficulty(&self) -> U256;

    async fn determine_payload_status(&self, head_block_hash: B256) -> crate::PayloadStatus;
    async fn is_ancestor(&self, head: B256, target: B256) -> bool;

    /// Resolves a reorg by comparing the old and new heads.
    async fn resolve_reorg(&self, old_head: B256, new_head: B256) -> Result<ReorgContext>;

    /// Marks a branch as canonical in storage.
    async fn mark_branch_canonical(&self, blocks: &Vec<Block<TxEnvelope>>) -> Result<()>;
}
