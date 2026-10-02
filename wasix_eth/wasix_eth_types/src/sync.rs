pub use crate::{async_trait, SyncStatus, Result, B256, PeerEntry, TxEnvelope};
use std::sync::Arc;
use alloy_consensus::Block;

use crate::p2p::{
    GetBlockHeaders, BlockHeaders, GetBlockBodies, BlockBodies, 
    GetPooledTransactions, PooledTransactions, GetReceipts, Receipts,
    GetNodeData, NodeData,
    RequestPair
};

#[async_trait]
pub trait SyncProvider: Send + Sync {
    async fn status(&self) -> SyncStatus;
    async fn trigger_sync(&self) -> Result<()>;
    async fn has_block(&self, hash: B256) -> bool;
    async fn get_block_by_hash(&self, hash: B256) -> Result<Option<Block<TxEnvelope>>>;
    async fn process_gossip_block(&self, block: Block<TxEnvelope>, td: alloy_primitives::U256) -> Result<()>;
    async fn process_gossip_transactions(&self, txs: Vec<TxEnvelope>) -> Result<()>;
    async fn process_pooled_transactions(&self, txs: Vec<TxEnvelope>) -> Result<()>;
    async fn handle_announced_pooled_transactions(&self, peer_id: String, hashes: Vec<B256>) -> Result<()>;
    async fn handle_announced_block_hashes(&self, peer_id: String, hashes: Vec<crate::p2p::BlockHashAndNumber>) -> Result<()>;
}

#[async_trait]
pub trait PeerProvider: Send + Sync {
    async fn get_active_peers(&self) -> Result<Vec<PeerEntry>>;
    async fn get_session(&self, peer_id: &str) -> Option<Arc<dyn P2pSession>>;
    fn as_any(&self) -> &dyn std::any::Any;
    fn disconnect_peer(&self, peer_id: &str) -> Result<()>;
}


#[async_trait]
pub trait P2pSession: Send + Sync {
    async fn get_block_headers(&self, request: RequestPair<GetBlockHeaders>) -> Result<RequestPair<BlockHeaders>>;
    async fn get_block_bodies(&self, request: RequestPair<GetBlockBodies>) -> Result<RequestPair<BlockBodies>>;
    async fn get_pooled_transactions(&self, request: RequestPair<GetPooledTransactions>) -> Result<RequestPair<PooledTransactions>>;
    async fn get_receipts(&self, request: RequestPair<GetReceipts>) -> Result<RequestPair<Receipts>>;
    async fn get_node_data(&self, request: RequestPair<GetNodeData>) -> Result<RequestPair<NodeData>>;
    async fn eth_status(&self) -> Option<crate::p2p::Status>;
    async fn best_height(&self) -> u64;

    async fn send_new_block_hashes(&self, hashes: crate::p2p::NewBlockHashes) -> Result<()>;
    async fn send_transactions(&self, txs: crate::p2p::Transactions) -> Result<()>;
    async fn send_new_block(&self, block: crate::p2p::NewBlock) -> Result<()>;
    async fn send_new_pooled_transaction_hashes(&self, hashes: crate::p2p::NewPooledTransactionHashes) -> Result<()>;
    async fn ping(&self) -> Result<()>;
    async fn disconnect(&self, reason: crate::p2p::DisconnectReason) -> Result<()>;

    // New response methods
    async fn send_block_headers(&self, headers: RequestPair<BlockHeaders>) -> Result<()>;
    async fn send_block_bodies(&self, bodies: RequestPair<BlockBodies>) -> Result<()>;
    async fn send_pooled_transactions(&self, txs: RequestPair<PooledTransactions>) -> Result<()>;
    async fn send_receipts(&self, receipts: RequestPair<Receipts>) -> Result<()>;
    async fn send_node_data(&self, data: RequestPair<NodeData>) -> Result<()>;
}

pub struct NoopSync;

#[async_trait]
impl SyncProvider for NoopSync {
    async fn status(&self) -> SyncStatus {
        SyncStatus::None
    }
    async fn trigger_sync(&self) -> Result<()> {
        Ok(())
    }
    async fn has_block(&self, _hash: B256) -> bool {
        false
    }
    async fn get_block_by_hash(&self, _hash: B256) -> Result<Option<Block<TxEnvelope>>> {
        Ok(None)
    }
    async fn process_gossip_block(&self, _block: Block<TxEnvelope>, _td: alloy_primitives::U256) -> Result<()> {
        Ok(())
    }
    async fn process_gossip_transactions(&self, _txs: Vec<TxEnvelope>) -> Result<()> {
        Ok(())
    }
    async fn process_pooled_transactions(&self, _txs: Vec<TxEnvelope>) -> Result<()> {
        Ok(())
    }
    async fn handle_announced_pooled_transactions(&self, _peer_id: String, _hashes: Vec<B256>) -> Result<()> {
        Ok(())
    }
    async fn handle_announced_block_hashes(&self, _peer_id: String, _hashes: Vec<crate::p2p::BlockHashAndNumber>) -> Result<()> {
        Ok(())
    }
}
