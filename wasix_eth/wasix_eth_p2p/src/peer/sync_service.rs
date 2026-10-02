use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{timeout, Duration};
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_types::{ConsensusTransaction, Typed2718};
use alloy_rlp::Encodable;
use wasix_eth_storage::read_traits::{BlockProvider, HeaderProvider, TransactionProvider, StateProvider};
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_types::sync::SyncProvider;
use wasix_eth_core::mempool::mempool_provider::MempoolProvider;
use wasix_eth_types::p2p::{
    GossipMessage, RequestPair, GetBlockHeaders, BlockHeaders, GetBlockBodies, BlockBodies,
    GetPooledTransactions, PooledTransactions, GetReceipts, Receipts, GetNodeData, NodeData
};
use wasix_eth_types::{async_trait, Block, BlockId, GossipProvider, Transaction, B256, TxEnvelope, TxPooledEnvelope, BlobTransactionSidecar, Signed, TxEip4844Variant, BlobTransactionSidecarVariant};
use wasix_eth_utils::metrics::GOSSIP_MESSAGES_RECEIVED;
use wasix_eth_utils::{debug, error, info};

use crate::peer::peer_manager::PeerManager;

pub const MAX_PEERS: usize = 50;

#[derive(Clone)]
pub struct SyncService {
    sync: Arc<RwLock<Arc<dyn SyncProvider>>>,
    read_provider: DatabaseReadProvider,
    mempool: Arc<dyn MempoolProvider>,
    peer_manager: Arc<PeerManager>,
    peer_gossip_tx: mpsc::Sender<GossipMessage>,
    peer_gossip_rx: Arc<tokio::sync::Mutex<Option<mpsc::Receiver<GossipMessage>>>>,
}

impl SyncService {
    async fn sync_provider(&self) -> Option<Arc<dyn SyncProvider>> {
        match timeout(Duration::from_secs(2), self.sync.read()).await {
            Ok(guard) => Some(guard.clone()),
            Err(_) => {
                error!("[Sync Service] Timed out waiting for sync provider lock");
                None
            }
        }
    }

    pub fn new(
        sync: Arc<dyn SyncProvider>,
        read_provider: DatabaseReadProvider,
        _write_provider: DatabaseWriteProvider,
        mempool: Arc<dyn MempoolProvider>,
        peer_manager: Arc<PeerManager>,
    ) -> Self {
        let (peer_gossip_tx, peer_gossip_rx) = mpsc::channel(10000);
        
        let service = Self {
            sync: Arc::new(RwLock::new(sync)),
            read_provider,
            mempool,
            peer_manager,
            peer_gossip_tx,
            peer_gossip_rx: Arc::new(tokio::sync::Mutex::new(Some(peer_gossip_rx))),
        };

        service
    }

    pub async fn set_provider(&self, sync: Arc<dyn SyncProvider>) {
        match timeout(Duration::from_secs(2), self.sync.write()).await {
            Ok(mut guard) => {
                *guard = sync;
            }
            Err(_) => {
                error!("[Sync Service] Timed out waiting to replace sync provider");
            }
        }
    }

    pub async fn start(&self) {
        let gossip_tx = self.peer_gossip_tx.clone();
        self.peer_manager.set_gossip_tx(gossip_tx).await;
    
        let service = self.clone();
        tokio::time::sleep(Duration::from_millis(100)).await;

        tokio::spawn(async move {
            let mut rx = service.peer_gossip_rx.lock().await.take().expect("Gossip already started");
            while let Some(msg) = rx.recv().await {
                let service_clone = service.clone();
                tokio::spawn(async move {
                    service_clone.handle_peer_gossip(msg).await;
                });
            }
        });
    }

    async fn handle_peer_gossip(&self, msg: GossipMessage) {
        GOSSIP_MESSAGES_RECEIVED.inc();
        match msg {
            GossipMessage::NewBlock(peer_id, session_id, m) => {
                if !self.peer_manager.registry.is_current_session(&peer_id, session_id).await {
                    return;
                }
                info!("[Sync Service] Received NewBlock {} (hash: {:?}) from {}", m.block.header.number, m.block.header.hash_slow(), peer_id);
                let Some(sync) = self.sync_provider().await else {
                    return;
                };
                let _ = sync.process_gossip_block(m.block, m.total_difficulty).await;
            }
            GossipMessage::Transactions(peer_id, session_id, m) => {
                if !self.peer_manager.registry.is_current_session(&peer_id, session_id).await {
                    return;
                }
                info!("[Sync Service] Received {} Transactions from {}", m.0.len(), peer_id);
                let Some(sync) = self.sync_provider().await else {
                    return;
                };
                let _ = sync.process_gossip_transactions(m.0).await;
            }
            GossipMessage::NewPooledTransactionHashes(peer_id, session_id, m) => {
                if !self.peer_manager.registry.is_current_session(&peer_id, session_id).await {
                    return;
                }
                info!("[Sync Service] Received {} NewPooledTransactionHashes from {}", m.hashes.len(), peer_id);
                let Some(sync) = self.sync_provider().await else {
                    return;
                };
                if let Err(e) = sync.handle_announced_pooled_transactions(peer_id, m.hashes).await {
                    error!("[Sync Service] Failed to handle announced pooled transactions: {}", e);
                }
            }
            GossipMessage::GetBlockHeaders(peer_id, session_id, req) => {
                if !self.peer_manager.registry.is_current_session(&peer_id, session_id).await {
                    return;
                }
                let service = self.clone();
                tokio::spawn(async move {
                    service.handle_get_block_headers(peer_id, req).await;
                });
            }
            GossipMessage::GetBlockBodies(peer_id, session_id, req) => {
                if !self.peer_manager.registry.is_current_session(&peer_id, session_id).await {
                    return;
                }
                let service = self.clone();
                tokio::spawn(async move {
                    service.handle_get_block_bodies(peer_id, req).await;
                });
            }
            GossipMessage::GetPooledTransactions(peer_id, session_id, req) => {
                if !self.peer_manager.registry.is_current_session(&peer_id, session_id).await {
                    return;
                }
                let service = self.clone();
                tokio::spawn(async move {
                    service.handle_get_pooled_transactions(peer_id, req).await;
                });
            }
            GossipMessage::GetReceipts(peer_id, session_id, req) => {
                if !self.peer_manager.registry.is_current_session(&peer_id, session_id).await {
                    return;
                }
                let service = self.clone();
                tokio::spawn(async move {
                    service.handle_get_receipts(peer_id, req).await;
                });
            }
            GossipMessage::GetNodeData(peer_id, session_id, req) => {
                if !self.peer_manager.registry.is_current_session(&peer_id, session_id).await {
                    return;
                }
                let service = self.clone();
                tokio::spawn(async move {
                    service.handle_get_node_data(peer_id, req).await;
                });
            }
            GossipMessage::NewBlockHashes(peer_id, session_id, m) => {
                if !self.peer_manager.registry.is_current_session(&peer_id, session_id).await {
                    return;
                }
                debug!("[Sync Service] Received {} NewBlockHashes from {}", m.0.len(), peer_id);
                let Some(sync) = self.sync_provider().await else {
                    return;
                };
                if let Err(e) = sync.handle_announced_block_hashes(peer_id, m.0).await {
                    error!("[Sync Service] Failed to handle announced block hashes: {}", e);
                }
            }
        }
    }

    async fn handle_get_block_headers(&self, peer_id: String, req: RequestPair<GetBlockHeaders>) {
        debug!("[Sync Service] Received GetBlockHeaders (id: {}, amount: {}) from {}", req.request_id, req.message.amount, peer_id);
        let session = match self.peer_manager.registry.get_session(&peer_id).await {
            Some(s) => s,
            None => return,
        };

        let mut headers = Vec::new();
        let mut current_id = req.message.block;
        let skip = req.message.skip;

        for _ in 0..req.message.amount {
            let header = match current_id {
                wasix_eth_types::p2p::BlockHashOrNumber::Number(n) => self.read_provider.header(wasix_eth_types::BlockId::Number(wasix_eth_types::BlockNumberOrTag::Number(n))),
                wasix_eth_types::p2p::BlockHashOrNumber::Hash(h) => {
                    let h_res = self.read_provider.header(wasix_eth_types::BlockId::Hash(h.into()));
                    if let Ok(Some(header)) = h_res {
                        Ok(Some(header))
                    } else {
                        // Try sync provider (which checks block tree)
                        debug!("[Sync Service] Header for {:?} not found in storage, trying SyncProvider", h);
                        match self.get_block_by_hash(h).await {
                            Ok(Some(block)) => {
                                debug!("[Sync Service] Header for {:?} found in SyncProvider (BlockTree)", h);
                                Ok(Some(block.header))
                            },
                            Ok(None) => {
                                debug!("[Sync Service] Header for {:?} not found in SyncProvider", h);
                                h_res
                            }
                            Err(e) => {
                                error!("[Sync Service] Error getting block for {:?} from SyncProvider: {}", h, e);
                                h_res
                            }
                        }
                    }
                }
            };

            if let Ok(Some(h)) = header {
                headers.push(h.clone());
                if req.message.reverse {
                    if current_id.as_number().unwrap_or(0) < skip + 1 { break; }
                    current_id = wasix_eth_types::p2p::BlockHashOrNumber::Number(current_id.as_number().unwrap_or(0) - skip - 1);
                } else {
                    current_id = wasix_eth_types::p2p::BlockHashOrNumber::Number(h.number + skip + 1);
                }
            } else {
                break;
            }
        }

        let header_count = headers.len();
        let response = RequestPair {
            request_id: req.request_id,
            message: BlockHeaders(headers),
        };
        let _ = session.send_block_headers(response).await;
        debug!("[Sync Service] Sent {} BlockHeaders (id: {}) to {}", header_count, req.request_id, peer_id);
    }

    async fn handle_get_block_bodies(&self, peer_id: String, req: RequestPair<GetBlockBodies>) {
        debug!("[Sync Service] Received GetBlockBodies (id: {}, hashes: {}) from {}", req.request_id, req.message.0.len(), peer_id);
        let session = match self.peer_manager.registry.get_session(&peer_id).await {
            Some(s) => s,
            None => return,
        };

        let mut bodies = Vec::new();
        for hash in req.message.0 {
            if let Ok(Some(body)) = self.read_provider.block_body_by_hash(hash) {
                bodies.push(body);
            } else {
                debug!("[Sync Service] Body for {:?} not found in storage, trying SyncProvider", hash);
                match self.get_block_by_hash(hash).await {
                    Ok(Some(block)) => {
                        debug!("[Sync Service] Body for {:?} found in SyncProvider (BlockTree)", hash);
                        bodies.push(block.body);
                    },
                    Ok(None) => {
                        debug!("[Sync Service] Body for {:?} not found in SyncProvider", hash);
                    }
                    Err(e) => {
                        error!("[Sync Service] Error getting block for {:?} from SyncProvider: {}", hash, e);
                    }
                }
            }
        }

        let body_count = bodies.len();
        let response = RequestPair {
            request_id: req.request_id,
            message: BlockBodies(bodies),
        };
        let _ = session.send_block_bodies(response).await;
        debug!("[Sync Service] Sent {} BlockBodies (id: {}) to {}", body_count, req.request_id, peer_id);
    }

    async fn handle_get_pooled_transactions(&self, peer_id: String, req: RequestPair<GetPooledTransactions>) {
        let session = match self.peer_manager.registry.get_session(&peer_id).await {
            Some(s) => s,
            None => return,
        };

        let mut txs = Vec::new();
        for hash in req.message.0 {
            // Prefer original pooled envelope if available (guarantees wire-format parity for EIP-4844)
            if let Some(pooled) = self.mempool.get_pooled_envelope(hash).await {
                {
                    use alloy_rlp::Encodable;
                    let mut buf = Vec::new();
                    pooled.encode(&mut buf);
                    debug!("[Sync] Serving stored pooled transaction {} length: {}", hash, buf.len());
                    if let TxPooledEnvelope::Eip4844(s) = &pooled {
                         let inner_tx = s.tx();
                         let sidecar = &inner_tx.sidecar;
                         debug!("[Sync] Blobs: {}, first blob len: {}", sidecar.blobs().len(), sidecar.blobs().get(0).map(|b| b.len()).unwrap_or(0));
                    }
                }
                txs.push(pooled);
                continue;
            }

            let tx_opt = if let Ok(Some(tx)) = self.read_provider.transaction(hash) {
                Some(tx)
            } else {
                self.mempool.get_transaction(hash).await
            };

            if let Some(tx) = tx_opt {
                let pooled = match tx {
                    Transaction::Legacy(t) => TxPooledEnvelope::Legacy(t),
                    Transaction::Eip2930(t) => TxPooledEnvelope::Eip2930(t),
                    Transaction::Eip1559(t) => TxPooledEnvelope::Eip1559(t),
                    Transaction::Eip4844(t) => {
                        let mut blobs = Vec::new();
                        let mut commitments = Vec::new();
                        let mut proofs = Vec::new();
                        
                        let versioned_hashes = t.blob_versioned_hashes().unwrap_or_default();
                        for v_hash in versioned_hashes {
                            if let Some((blob, comm, proof)) = self.mempool.get_blob(*v_hash).await {
                                blobs.push(blob.clone());
                                commitments.push(comm.clone());
                                proofs.push(proof.clone());
                            }
                        }

                        if !blobs.is_empty() {
                            let (tx_variant, signature, hash) = t.clone().into_parts();
                            let sidecar = BlobTransactionSidecar {
                                blobs,
                                commitments,
                                proofs,
                            };
                            let pooled = match tx_variant {
                                TxEip4844Variant::TxEip4844(inner_tx) => {
                                    TxPooledEnvelope::Eip4844(Signed::new_unchecked(inner_tx.with_sidecar(BlobTransactionSidecarVariant::Eip4844(sidecar)), signature, hash))
                                }
                                TxEip4844Variant::TxEip4844WithSidecar(inner_tx) => {
                                    TxPooledEnvelope::Eip4844(Signed::new_unchecked(inner_tx.tx.with_sidecar(BlobTransactionSidecarVariant::Eip4844(sidecar)), signature, hash))
                                }
                            };
                            
                            {
                                use alloy_rlp::Encodable;
                                let mut buf = Vec::new();
                                pooled.encode(&mut buf);
                                debug!("[Sync] Reconstructed pooled transaction {} length: {}", hash, buf.len());
                                if let TxPooledEnvelope::Eip4844(s) = &pooled {
                                     let inner_tx = s.tx();
                                     let sidecar = &inner_tx.sidecar;
                                     debug!("[Sync] Blobs: {}, first blob len: {}", sidecar.blobs().len(), sidecar.blobs().get(0).map(|b| b.len()).unwrap_or(0));
                                }
                            }
                            
                            pooled
                        } else {
                            // If it's a blob tx but we don't have the sidecar, we can't serve it in PooledTransactions
                            // We still need to send the response, so we just skip this transaction.
                            // If ALL transactions are skipped, we send an empty response.
                            continue;
                        }
                    }
                    Transaction::Eip7702(t) => TxPooledEnvelope::Eip7702(t),
                };
                txs.push(pooled);
            }
        }

        let response = RequestPair {
            request_id: req.request_id,
            message: PooledTransactions(txs),
        };
        let _ = session.send_pooled_transactions(response).await;
    }

    async fn handle_get_receipts(&self, peer_id: String, req: RequestPair<GetReceipts>) {
        let session = match self.peer_manager.registry.get_session(&peer_id).await {
            Some(s) => s,
            None => return,
        };

        let mut receipts = Vec::new();
        for hash in req.message.0 {
            if let Ok(Some(block)) = self.read_provider.block_by_hash(hash) {
                let mut block_receipts = Vec::new();
                for (i, _) in block.body.transactions.iter().enumerate() {
                    if let Ok(Some(receipt)) = self.read_provider.receipt(hash, i as u64) {
                        block_receipts.push(receipt);
                    }
                }
                receipts.push(block_receipts);
            }
        }

        let response = RequestPair {
            request_id: req.request_id,
            message: Receipts(receipts),
        };
        let _ = session.send_receipts(response).await;
    }

    async fn handle_get_node_data(&self, peer_id: String, req: RequestPair<GetNodeData>) {
        let session = match self.peer_manager.registry.get_session(&peer_id).await {
            Some(s) => s,
            None => return,
        };

        let mut data = Vec::new();
        for hash in req.message.0 {
            if let Ok(Some(node)) = self.read_provider.trie_node(hash) {
                data.push(node);
            }
        }

        let response = RequestPair {
            request_id: req.request_id,
            message: NodeData(data),
        };
        let _ = session.send_node_data(response).await;
    }

    pub fn gossip_rx(&self) -> mpsc::Receiver<Vec<u8>> {
        // Not used anymore
        let (_, rx) = mpsc::channel(1);
        rx
    }
    pub fn dial_peer(&self, addr: SocketAddr) {
        self.peer_manager.dialer.dial_peer(addr);
    }
    pub async fn trigger_sync(&self) -> Result<()> {
        let Some(sync) = self.sync_provider().await else {
            return Err(anyhow::anyhow!("Timed out waiting for sync provider"));
        };
        sync.trigger_sync().await
    }

    pub async fn get_block(&self, id: BlockId) -> Result<Option<Block<TxEnvelope>>> {
        self.read_provider.block(id)
    }

    pub async fn get_block_hash(&self, number: u64) -> Result<Option<B256>> {
        self.read_provider.block_hash(number)
    }

    pub async fn get_block_by_hash(&self, hash: B256) -> Result<Option<Block<TxEnvelope>>> {
        if let Ok(Some(block)) = self.read_provider.block_by_hash(hash) {
            return Ok(Some(block));
        }
        if let Some(sync) = self.sync_provider().await {
            return sync.get_block_by_hash(hash).await;
        }
        Ok(None)
    }

    pub async fn latest_block_number(&self) -> Result<Option<u64>> {
        self.read_provider.latest_block_number()
    }
    pub async fn broadcast_gossip(&self, data: Vec<u8>) {
        self.broadcast_raw(data).await;
    }
}

#[async_trait]
impl GossipProvider for SyncService {
    async fn broadcast_raw(&self, _data: Vec<u8>) {
        // Not used anymore as we have structured gossip
    }

    async fn broadcast_transaction(&self, tx: &TxEnvelope) {
        self.broadcast_new_pooled_transaction_hashes(vec![tx.clone()]).await;
    }

    async fn broadcast_new_pooled_transaction_hashes(&self, txs: Vec<TxEnvelope>) {
        let sessions = self.peer_manager.registry.get_all_sessions().await;
        if sessions.is_empty() { return; }

        let mut types = Vec::with_capacity(txs.len());
        let mut sizes = Vec::with_capacity(txs.len());
        let mut hashes = Vec::with_capacity(txs.len());

        for tx in txs {
            let hash = *tx.hash();
            
            // Prefer original pooled envelope if available for accurate size advertisement
            let pooled = if let Some(pooled) = self.mempool.get_pooled_envelope(hash).await {
                pooled
            } else {
                match &tx {
                    Transaction::Legacy(t) => TxPooledEnvelope::Legacy(t.clone()),
                    Transaction::Eip2930(t) => TxPooledEnvelope::Eip2930(t.clone()),
                    Transaction::Eip1559(t) => TxPooledEnvelope::Eip1559(t.clone()),
                    Transaction::Eip4844(t) => {
                        let mut blobs = Vec::new();
                        let mut commitments = Vec::new();
                        let mut proofs = Vec::new();

                        let versioned_hashes = t.blob_versioned_hashes().unwrap_or_default();
                        for v_hash in versioned_hashes {
                            if let Some((blob, comm, proof)) = self.mempool.get_blob(*v_hash).await {
                                blobs.push(blob.clone());
                                commitments.push(comm.clone());
                                proofs.push(proof.clone());
                            }
                        }

                        if !blobs.is_empty() {
                            let (tx_variant, signature, hash) = t.clone().into_parts();
                            let sidecar = BlobTransactionSidecar {
                                blobs,
                                commitments,
                                proofs,
                            };
                            match tx_variant {
                                TxEip4844Variant::TxEip4844(inner_tx) => {
                                    TxPooledEnvelope::Eip4844(Signed::new_unchecked(inner_tx.with_sidecar(BlobTransactionSidecarVariant::Eip4844(sidecar)), signature, hash))
                                }
                                TxEip4844Variant::TxEip4844WithSidecar(inner_tx) => {
                                    TxPooledEnvelope::Eip4844(Signed::new_unchecked(inner_tx.tx.with_sidecar(BlobTransactionSidecarVariant::Eip4844(sidecar)), signature, hash))
                                }
                            }
                        } else {
                            // If it's a blob tx but we don't have the sidecar, we can't accurately advertise its size
                            continue;
                        }
                    }
                    Transaction::Eip7702(t) => TxPooledEnvelope::Eip7702(t.clone()),
                }
            };
            types.push(tx.ty());
            // Prefer exact raw bytes length if available to guarantee parity with wire format
            if let Some(raw) = self.mempool.get_pooled_bytes(hash).await {
                debug!("[Sync] Using stored raw pooled bytes for size: {} -> {} bytes", hash, raw.len());
                sizes.push(raw.len() as u64);
            } else {
                let advertised_size = pooled.length();
                sizes.push(advertised_size as u64);
            }
            hashes.push(hash);
        }

        if hashes.is_empty() { return; }

        let msg = wasix_eth_types::p2p::NewPooledTransactionHashes {
            types: types.into(),
            sizes,
            hashes: hashes.clone(),
        };

        info!("[Sync] Broadcasting {} pooled transaction hashes to {} peers", hashes.len(), sessions.len());
        for session in sessions {
            use wasix_eth_types::sync::P2pSession;
            let _ = session.send_new_pooled_transaction_hashes(msg.clone()).await;
        }
    }

    async fn broadcast_block(&self, block: &Block<TxEnvelope>) {
        let sessions = self.peer_manager.registry.get_all_sessions().await;
        if sessions.is_empty() { return; }

        let block_hash = block.header.hash_slow();
        let head_td = self.read_provider.header_td(block_hash).ok().flatten()
            .unwrap_or(alloy_primitives::U256::ZERO);

        let hash_msg = wasix_eth_types::p2p::NewBlockHashes(vec![wasix_eth_types::p2p::BlockHashAndNumber {
            hash: block_hash,
            number: block.header.number,
        }]);

        info!("[Sync] Broadcasting NewBlockHashes (block: {}, hash: {:?}) to {} peers", block.header.number, block_hash, sessions.len());
        for session in sessions {
            use wasix_eth_types::sync::P2pSession;
            let _ = session.send_new_block_hashes(hash_msg.clone()).await;
        }
    }
}