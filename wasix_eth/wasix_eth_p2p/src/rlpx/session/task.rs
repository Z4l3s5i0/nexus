use super::types::SessionRequest;
use crate::peer::peer_registry::DisconnectEvent;
use crate::rlpx::RlpxStream;
use alloy_primitives::Bytes;
use alloy_rlp::Decodable;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot, Mutex};
use wasix_eth_types::p2p::{BlockBodies, BlockHeaders, BlockRangeUpdate, Disconnect, EthMessageID, GetBlockBodies, GetBlockHeaders, GetNodeData, GetPooledTransactions, GetReceipts, GossipMessage, NewPooledTransactionHashes, NewPooledTransactionHashes66, NodeData, Ping, Pong, PooledTransactions, Receipts, RequestPair, Status, StatusEth69, StatusMessage, Transactions};
use wasix_eth_utils::{debug, error, info, exp};

pub struct SessionTask<S> {
    stream: RlpxStream<S>,
    request_rx: mpsc::Receiver<SessionRequest>,
    gossip_tx: Option<mpsc::Sender<GossipMessage>>,
    disconnect_tx: Option<mpsc::Sender<DisconnectEvent>>,
    status: Arc<Mutex<Option<StatusMessage>>>,
    best_height: Arc<Mutex<u64>>,
    last_activity: Arc<Mutex<Instant>>,
    pending_headers: HashMap<u64, oneshot::Sender<Result<RequestPair<BlockHeaders>>>>,
    pending_bodies: HashMap<u64, oneshot::Sender<Result<RequestPair<BlockBodies>>>>,
    pending_pooled_txs: HashMap<u64, oneshot::Sender<Result<RequestPair<PooledTransactions>>>>,
    pending_receipts: HashMap<u64, oneshot::Sender<Result<RequestPair<Receipts>>>>,
    pending_node_data: HashMap<u64, oneshot::Sender<Result<RequestPair<NodeData>>>>,
    peer_id: String,
    eth_offset: u8,
    session_id: u64,
}

impl<S> SessionTask<S> 
where S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static
{
    pub fn new(
        stream: RlpxStream<S>,
        request_rx: mpsc::Receiver<SessionRequest>,
        session_id: u64,
        gossip_tx: Option<mpsc::Sender<GossipMessage>>,
        disconnect_tx: Option<mpsc::Sender<DisconnectEvent>>,
        status: Arc<Mutex<Option<StatusMessage>>>,
        best_height: Arc<Mutex<u64>>,
        last_activity: Arc<Mutex<Instant>>,
    ) -> Self {
        let peer_id = format!("{:?}", stream.remote_id.unwrap_or_default());
        let eth_offset = stream.shared_capabilities.iter()
            .find(|c| c.name == "eth")
            .map(|c| c.offset)
            .unwrap_or(0x10);

        Self {
            stream,
            request_rx,
            gossip_tx,
            disconnect_tx,
            status,
            best_height,
            last_activity,
            pending_headers: HashMap::new(),
            pending_bodies: HashMap::new(),
            pending_pooled_txs: HashMap::new(),
            pending_receipts: HashMap::new(),
            pending_node_data: HashMap::new(),
            peer_id,
            eth_offset,
            session_id,
        }
    }

    pub async fn run(mut self) {
        info!("[P2P Session] Started session for peer {}", self.peer_id);
        let mut ping_interval = tokio::time::interval(Duration::from_secs(10)); // Reduced from 20s
        
        loop {
            tokio::select! {
                biased;
                
                Some(req) = self.request_rx.recv() => {
                    if let Err(_) = self.handle_request(req).await {
                        // Request handling failed in a way that should terminate session
                        break;
                    }
                    // Yield to ensure the message is actually sent and the runtime gets a chance to poll other tasks
                    tokio::task::yield_now().await;
                }
                _ = ping_interval.tick() => {
                    let _ = self.stream.send_p2p(&Ping {}, 0x02).await;
                }
                res = self.stream.read_message() => {
                    {
                        let mut la = self.last_activity.lock().await;
                        *la = Instant::now();
                    }
                    
                    match res {
                        Ok((id, payload)) => {
                            if !self.handle_message(id, payload).await {
                                info!("[P2P Session] Protocol requested disconnect for {}", self.peer_id);
                                break;
                            }
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            if err_str.contains("early eof") || err_str.contains("Broken pipe") || err_str.contains("Connection reset") {
                                debug!("[P2P Session] Connection closed by remote peer {}: {}", self.peer_id, err_str);
                            } else {
                                error!("[P2P Session] Read error from {}: {}", self.peer_id, e);
                            }
                            break;
                        }
                    }
                }
            }
        }
        self.notify_disconnect().await;
    }

    async fn handle_request(&mut self, req: SessionRequest) -> Result<()> {
        let has_request_id = self.stream.eth_version().as_ref().map(|v| v.has_request_id()).unwrap_or(false);
        match req {
            SessionRequest::GetHeaders { request, response_tx } => {
                let id = request.request_id;
                let res = if has_request_id {
                    self.stream.send_eth(&request, EthMessageID::GetBlockHeaders.to_u8()).await
                } else {
                    self.stream.send_eth(&request.message, EthMessageID::GetBlockHeaders.to_u8()).await
                };
                if let Err(e) = res {
                    let _ = response_tx.send(Err(e));
                } else {
                    self.pending_headers.insert(id, response_tx);
                }
            }
            SessionRequest::GetBodies { request, response_tx } => {
                let id = request.request_id;
                let res = if has_request_id {
                    self.stream.send_eth(&request, EthMessageID::GetBlockBodies.to_u8()).await
                } else {
                    self.stream.send_eth(&request.message, EthMessageID::GetBlockBodies.to_u8()).await
                };
                if let Err(e) = res {
                    let _ = response_tx.send(Err(e));
                } else {
                    self.pending_bodies.insert(id, response_tx);
                }
            }
            SessionRequest::GetPooledTransactions { request, response_tx } => {
                let id = request.request_id;
                let res = if has_request_id {
                    self.stream.send_eth(&request, EthMessageID::GetPooledTransactions.to_u8()).await
                } else {
                    self.stream.send_eth(&request.message, EthMessageID::GetPooledTransactions.to_u8()).await
                };
                if let Err(e) = res {
                    let _ = response_tx.send(Err(e));
                } else {
                    self.pending_pooled_txs.insert(id, response_tx);
                }
            }
            SessionRequest::GetReceipts { request, response_tx } => {
                let id = request.request_id;
                let res = if has_request_id {
                    self.stream.send_eth(&request, EthMessageID::GetReceipts.to_u8()).await
                } else {
                    self.stream.send_eth(&request.message, EthMessageID::GetReceipts.to_u8()).await
                };
                if let Err(e) = res {
                    let _ = response_tx.send(Err(e));
                } else {
                    self.pending_receipts.insert(id, response_tx);
                }
            }
            SessionRequest::GetNodeData { request, response_tx } => {
                let id = request.request_id;
                let res = if has_request_id {
                    self.stream.send_eth(&request, EthMessageID::GetNodeData.to_u8()).await
                } else {
                    self.stream.send_eth(&request.message, EthMessageID::GetNodeData.to_u8()).await
                };
                if let Err(e) = res {
                    let _ = response_tx.send(Err(e));
                } else {
                    self.pending_node_data.insert(id, response_tx);
                }
            }
            SessionRequest::SendTransactions(m) => { 
                for tx_item in &m.0 {
                    exp!("[EXP] P2P_SEND_TX peer={} hash={:?}", self.peer_id, tx_item.hash());
                }
                let _ = self.stream.send_eth(&m, EthMessageID::Transactions.to_u8()).await; 
            }
            SessionRequest::SendNewPooledTransactionHashes(m) => {
                let version = self.stream.eth_version();
                if version.as_ref().map(|v| v.is_eth68_or_newer()).unwrap_or(false) {
                    let _ = self.stream.send_eth(&m, EthMessageID::NewPooledTransactionHashes.to_u8()).await;
                } else {
                    let legacy = NewPooledTransactionHashes66 { hashes: m.hashes };
                    let _ = self.stream.send_eth(&legacy, EthMessageID::NewPooledTransactionHashes.to_u8()).await;
                }
            }
            SessionRequest::Ping => { let _ = self.stream.send_p2p(&Ping {}, 0x02).await; }
            SessionRequest::Disconnect(m) => { 
                let _ = self.stream.send_p2p(&m, 0x01).await; 
                return Err(anyhow::anyhow!("Disconnect requested"));
            }
            SessionRequest::SendBlockHeaders(headers) => {
                let version = self.stream.eth_version();
                if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
                    let _ = self.stream.send_eth(&headers, EthMessageID::BlockHeaders.to_u8()).await;
                } else {
                    let _ = self.stream.send_eth(&headers.message, EthMessageID::BlockHeaders.to_u8()).await;
                }
            }
            SessionRequest::SendBlockBodies(bodies) => {
                let version = self.stream.eth_version();
                if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
                    let _ = self.stream.send_eth(&bodies, EthMessageID::BlockBodies.to_u8()).await;
                } else {
                    let _ = self.stream.send_eth(&bodies.message, EthMessageID::BlockBodies.to_u8()).await;
                }
            }
            SessionRequest::SendPooledTransactions(txs) => {
                let version = self.stream.eth_version();
                if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
                    let _ = self.stream.send_eth(&txs, EthMessageID::PooledTransactions.to_u8()).await;
                } else {
                    let _ = self.stream.send_eth(&txs.message, EthMessageID::PooledTransactions.to_u8()).await;
                }
            }
            SessionRequest::SendReceipts(receipts) => {
                let version = self.stream.eth_version();
                if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
                    let _ = self.stream.send_eth(&receipts, EthMessageID::Receipts.to_u8()).await;
                } else {
                    let _ = self.stream.send_eth(&receipts.message, EthMessageID::Receipts.to_u8()).await;
                }
            }
            SessionRequest::SendNodeData(data) => {
                let version = self.stream.eth_version();
                if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
                    let _ = self.stream.send_eth(&data, EthMessageID::NodeData.to_u8()).await;
                } else {
                    let _ = self.stream.send_eth(&data.message, EthMessageID::NodeData.to_u8()).await;
                }
            }
        }
        Ok(())
    }

    async fn handle_message(&mut self, id: u8, payload: Vec<u8>) -> bool {
        if id < self.eth_offset {
            self.handle_p2p_message(id, payload).await
        } else {
            let eth_id = id - self.eth_offset;
            self.handle_eth_message(eth_id, payload).await
        }
    }

    async fn handle_p2p_message(&mut self, id: u8, payload: Vec<u8>) -> bool {
        match id {
            0x01 => { // Disconnect
                if let Ok(m) = Disconnect::decode(&mut &payload[..]) {
                    info!("[P2P Session] Received Disconnect from {}: {}", self.peer_id, m.reason);
                } else {
                    info!("[P2P Session] Received Disconnect from {} (failed to decode reason)", self.peer_id);
                }
                self.notify_disconnect().await;
                false
            }
            0x02 => { // Ping
                let _ = self.stream.send_p2p(&Pong {}, 0x03).await;
                true
            }
            0x03 => { // Pong
                true
            }
            _ => {
                info!("[P2P Session] Received unhandled P2P message ID: {}", id);
                true
            }
        }
    }

    async fn handle_eth_message(&mut self, eth_id: u8, payload: Vec<u8>) -> bool {
        debug!("[P2P Session] Received ETH message ID: {} from {}", eth_id, self.peer_id);
        match EthMessageID::decode(&mut &vec![eth_id][..]) {
            Ok(EthMessageID::Status) => self.handle_status(payload).await,
            Ok(EthMessageID::NewBlockHashes) => {
                debug!("[P2P Session] Received NewBlockHashes from {}, but block gossip is disabled. Ignoring.", self.peer_id);
                true
            }
            Ok(EthMessageID::Transactions) => self.handle_transactions(payload).await,
            Ok(EthMessageID::GetBlockHeaders) => {
                debug!("[P2P Session] Handling GetBlockHeaders from {}", self.peer_id);
                self.handle_get_block_headers(payload).await
            }
            Ok(EthMessageID::BlockHeaders) => {
                debug!("[P2P Session] Handling BlockHeaders from {}", self.peer_id);
                self.handle_block_headers(payload).await
            }
            Ok(EthMessageID::GetBlockBodies) => {
                debug!("[P2P Session] Handling GetBlockBodies from {}", self.peer_id);
                self.handle_get_block_bodies(payload).await
            }
            Ok(EthMessageID::BlockBodies) => {
                debug!("[P2P Session] Handling BlockBodies from {}", self.peer_id);
                self.handle_block_bodies(payload).await
            }
            Ok(EthMessageID::GetPooledTransactions) => self.handle_get_pooled_transactions(payload).await,
            Ok(EthMessageID::PooledTransactions) => self.handle_pooled_transactions(payload).await,
            Ok(EthMessageID::GetReceipts) => self.handle_get_receipts(payload).await,
            Ok(EthMessageID::Receipts) => self.handle_receipts(payload).await,
            Ok(EthMessageID::GetNodeData) => {
                if self.stream.eth_version().as_ref().map(|v| v.supports_get_node_data()).unwrap_or(true) {
                    self.handle_get_node_data(payload).await
                } else {
                    debug!("[P2P Session] Received GetNodeData from {}, but it's not supported in {:?}", self.peer_id, self.stream.eth_version());
                    true
                }
            }
            Ok(EthMessageID::NodeData) => {
                if self.stream.eth_version().as_ref().map(|v| v.supports_get_node_data()).unwrap_or(true) {
                    self.handle_node_data(payload).await
                } else {
                    debug!("[P2P Session] Received NodeData from {}, but it's not supported in {:?}", self.peer_id, self.stream.eth_version());
                    true
                }
            }
            Ok(EthMessageID::NewBlock) => {
                debug!("[P2P Session] Received NewBlock from {}, but block gossip is disabled. Ignoring.", self.peer_id);
                true
            }
            Ok(EthMessageID::NewPooledTransactionHashes) => self.handle_new_pooled_transaction_hashes(payload).await,
            Ok(EthMessageID::BlockRangeUpdate) => self.handle_block_range_update(payload).await,
            _ => {
                info!("[P2P Session] Received unhandled ETH message ID: {}", eth_id);
                true
            }
        }
    }

    async fn handle_get_block_headers(&mut self, payload: Vec<u8>) -> bool {
        let version = self.stream.eth_version();
        let request = if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
            if let Ok(r) = RequestPair::<GetBlockHeaders>::decode(&mut &payload[..]) {
                r
            } else {
                return true;
            }
        } else {
            if let Ok(m) = GetBlockHeaders::decode(&mut &payload[..]) {
                RequestPair {
                    request_id: 0,
                    message: m,
                }
            } else {
                return true;
            }
        };

        if let Some(tx) = &self.gossip_tx {
            let _ = tx.send(GossipMessage::GetBlockHeaders(self.peer_id.clone(), self.session_id, request)).await;
        }
        true
    }

    async fn handle_get_block_bodies(&mut self, payload: Vec<u8>) -> bool {
        let version = self.stream.eth_version();
        let request = if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
            if let Ok(r) = RequestPair::<GetBlockBodies>::decode(&mut &payload[..]) {
                r
            } else {
                return true;
            }
        } else {
            if let Ok(m) = GetBlockBodies::decode(&mut &payload[..]) {
                RequestPair {
                    request_id: 0,
                    message: m,
                }
            } else {
                return true;
            }
        };

        if let Some(tx) = &self.gossip_tx {
            let _ = tx.send(GossipMessage::GetBlockBodies(self.peer_id.clone(), self.session_id, request)).await;
        }
        true
    }

    async fn handle_get_pooled_transactions(&mut self, payload: Vec<u8>) -> bool {
        let version = self.stream.eth_version();
        let request = if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
            if let Ok(r) = RequestPair::<GetPooledTransactions>::decode(&mut &payload[..]) {
                r
            } else {
                return true;
            }
        } else {
            if let Ok(m) = GetPooledTransactions::decode(&mut &payload[..]) {
                RequestPair {
                    request_id: 0,
                    message: m,
                }
            } else {
                return true;
            }
        };

        if let Some(tx) = &self.gossip_tx {
            let _ = tx.send(GossipMessage::GetPooledTransactions(self.peer_id.clone(), self.session_id, request)).await;
        }
        true
    }

    async fn handle_get_receipts(&mut self, payload: Vec<u8>) -> bool {
        let version = self.stream.eth_version();
        if let Some(v) = version {
            if !v.supports_receipts() {
                debug!("[P2P Session] Received GetReceipts (0x0f) on {}, which does not support it. Ignoring.", v as u8);
                return true;
            }
        }
        
        let version = self.stream.eth_version();
        let request = if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
            if let Ok(r) = RequestPair::<GetReceipts>::decode(&mut &payload[..]) {
                r
            } else {
                return true;
            }
        } else {
            if let Ok(m) = GetReceipts::decode(&mut &payload[..]) {
                RequestPair {
                    request_id: 0,
                    message: m,
                }
            } else {
                return true;
            }
        };

        if let Some(tx) = &self.gossip_tx {
            let _ = tx.send(GossipMessage::GetReceipts(self.peer_id.clone(), self.session_id, request)).await;
        }
        true
    }

    async fn handle_get_node_data(&mut self, payload: Vec<u8>) -> bool {
        let version = self.stream.eth_version();
        let request = if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
            if let Ok(r) = RequestPair::<GetNodeData>::decode(&mut &payload[..]) {
                r
            } else {
                return true;
            }
        } else {
            if let Ok(m) = GetNodeData::decode(&mut &payload[..]) {
                RequestPair {
                    request_id: 0,
                    message: m,
                }
            } else {
                return true;
            }
        };

        if let Some(tx) = &self.gossip_tx {
            let _ = tx.send(GossipMessage::GetNodeData(self.peer_id.clone(), self.session_id, request)).await;
        }
        true
    }

    async fn handle_status(&mut self, payload: Vec<u8>) -> bool {
        let s = if let Ok(s) = Status::decode(&mut &payload[..]) {
            StatusMessage::Legacy(s)
        } else if let Ok(s) = StatusEth69::decode(&mut &payload[..]) {
            StatusMessage::Eth69(s)
        } else {
            error!("[P2P Session] Failed to decode Status from {}", self.peer_id);
            return true;
        };
        
        let (hash, td, height) = match &s {
            StatusMessage::Legacy(l) => (l.blockhash, l.total_difficulty, None),
            StatusMessage::Eth69(e) => (e.blockhash, alloy_primitives::U256::ZERO, Some(e.latest)),
        };
        
        debug!("[P2P Session] Received Status from {}: head={:?}, TD={}, height={:?}", self.peer_id, hash, td, height);
        if let Some(h) = height {
            let mut h_guard = self.best_height.lock().await;
            if h > *h_guard {
                *h_guard = h;
            }
        }
        let mut guard = self.status.lock().await;
        *guard = Some(s);
        true
    }

    async fn handle_transactions(&mut self, payload: Vec<u8>) -> bool {
        if let (Some(tx), Ok(m)) = (&self.gossip_tx, Transactions::decode(&mut &payload[..])) {
            debug!("[P2P Session] Received {} Transactions from {}", m.0.len(), self.peer_id);
            for tx_item in &m.0 {
                exp!("[EXP] P2P_RECV_TX peer={} hash={:?}", self.peer_id, tx_item.hash());
            }
            let _ = tx.send(GossipMessage::Transactions(self.peer_id.clone(), self.session_id, m)).await;
        }
        true
    }

    async fn handle_block_headers(&mut self, payload: Vec<u8>) -> bool {
        let version = self.stream.eth_version();
        let headers = if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
            if let Ok(headers) = RequestPair::<BlockHeaders>::decode(&mut &payload[..]) {
                headers
            } else {
                return true;
            }
        } else {
            if let Ok(m) = BlockHeaders::decode(&mut &payload[..]) {
                RequestPair { request_id: 0, message: m }
            } else {
                return true;
            }
        };

        debug!("[P2P Session] Received {} BlockHeaders from {}", headers.message.0.len(), self.peer_id);
        {
            let mut h_guard = self.best_height.lock().await;
            for h in &headers.message.0 {
                if h.number > *h_guard {
                    *h_guard = h.number;
                }
            }
        }
        if let Some(tx) = self.pending_headers.remove(&headers.request_id) {
            let _ = tx.send(Ok(headers));
        }
        true
    }

    async fn handle_block_bodies(&mut self, payload: Vec<u8>) -> bool {
        let version = self.stream.eth_version();
        let bodies = if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
            if let Ok(bodies) = RequestPair::<BlockBodies>::decode(&mut &payload[..]) {
                bodies
            } else {
                return true;
            }
        } else {
            if let Ok(m) = BlockBodies::decode(&mut &payload[..]) {
                RequestPair { request_id: 0, message: m }
            } else {
                return true;
            }
        };

        debug!("[P2P Session] Received {} BlockBodies from {}", bodies.message.0.len(), self.peer_id);
        if let Some(tx) = self.pending_bodies.remove(&bodies.request_id) {
            let _ = tx.send(Ok(bodies));
        }
        true
    }

    async fn handle_pooled_transactions(&mut self, payload: Vec<u8>) -> bool {
        let version = self.stream.eth_version();
        let res = if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
            if let Ok(m) = RequestPair::<PooledTransactions>::decode(&mut &payload[..]) {
                m
            } else {
                return true;
            }
        } else {
            if let Ok(m) = PooledTransactions::decode(&mut &payload[..]) {
                RequestPair { request_id: 0, message: m }
            } else {
                return true;
            }
        };

        debug!("[P2P Session] Received {} PooledTransactions from {}", res.message.0.len(), self.peer_id);
        if let Some(tx) = self.pending_pooled_txs.remove(&res.request_id) {
            let _ = tx.send(Ok(res));
        }
        true
    }

    async fn handle_receipts(&mut self, payload: Vec<u8>) -> bool {
        let version = self.stream.eth_version();
        if let Some(v) = version {
            if !v.supports_receipts() {
                debug!("[P2P Session] Received Receipts (0x10) on {}, which does not support it. Ignoring.", v as u8);
                return true;
            }
        }

        let version = self.stream.eth_version();
        let res = if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
            if let Ok(m) = RequestPair::<Receipts>::decode(&mut &payload[..]) {
                m
            } else {
                return true;
            }
        } else {
            if let Ok(m) = Receipts::decode(&mut &payload[..]) {
                RequestPair { request_id: 0, message: m }
            } else {
                return true;
            }
        };

        debug!("[P2P Session] Received {} Receipts from {}", res.message.0.len(), self.peer_id);
        if let Some(tx) = self.pending_receipts.remove(&res.request_id) {
            let _ = tx.send(Ok(res));
        }
        true
    }

    async fn handle_node_data(&mut self, payload: Vec<u8>) -> bool {
        let version = self.stream.eth_version();
        let res = if version.as_ref().map(|v| v.has_request_id()).unwrap_or(false) {
            if let Ok(m) = RequestPair::<NodeData>::decode(&mut &payload[..]) {
                m
            } else {
                return true;
            }
        } else {
            if let Ok(m) = NodeData::decode(&mut &payload[..]) {
                RequestPair { request_id: 0, message: m }
            } else {
                return true;
            }
        };

        debug!("[P2P Session] Received {} NodeData from {}", res.message.0.len(), self.peer_id);
        if let Some(tx) = self.pending_node_data.remove(&res.request_id) {
            let _ = tx.send(Ok(res));
        }
        true
    }

    async fn handle_new_pooled_transaction_hashes(&mut self, payload: Vec<u8>) -> bool {
        let version = self.stream.eth_version();
        let m = if version.as_ref().map(|v| v.is_eth68_or_newer()).unwrap_or(false) {
            if let Ok(m) = NewPooledTransactionHashes::decode(&mut &payload[..]) {
                m
            } else {
                return true;
            }
        } else {
            if let Ok(m) = NewPooledTransactionHashes66::decode(&mut &payload[..]) {
                NewPooledTransactionHashes {
                    types: Bytes::new(),
                    sizes: Vec::new(),
                    hashes: m.hashes,
                }
            } else {
                return true;
            }
        };

        if let Some(tx) = &self.gossip_tx {
            debug!("[P2P Session] Received {} NewPooledTransactionHashes from {}", m.hashes.len(), self.peer_id);
            let _ = tx.send(GossipMessage::NewPooledTransactionHashes(self.peer_id.clone(), self.session_id, m)).await;
        }
        true
    }

    async fn handle_block_range_update(&mut self, payload: Vec<u8>) -> bool {
        if let Ok(m) = BlockRangeUpdate::decode(&mut &payload[..]) {
            debug!("[P2P Session] Received BlockRangeUpdate {:?} from {}", m, self.peer_id);
            // In a real client we would update our peer's history range.
            // For now we just log it.
        }
        true
    }

    async fn notify_disconnect(&self) {
        if let Some(tx) = &self.disconnect_tx {
            let _ = tx.send(DisconnectEvent {
                peer_id: self.peer_id.clone(),
                session_id: self.session_id,
            }).await;
        }
    }
}
