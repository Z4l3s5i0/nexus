pub mod types;
pub mod task;

use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{timeout, Duration};
use anyhow::Result;
use wasix_eth_types::sync::P2pSession;
use wasix_eth_types::async_trait;
use wasix_eth_types::p2p::{
    NewPooledTransactionHashes, Transactions,
    GetBlockHeaders, BlockHeaders, GetBlockBodies, BlockBodies,
    GetPooledTransactions, PooledTransactions, GetReceipts, Receipts,
    GetNodeData, NodeData,
    RequestPair, Status, StatusMessage, GossipMessage, DisconnectReason
};
use crate::peer::peer_registry::DisconnectEvent;
use crate::rlpx::RlpxStream;
use self::types::SessionRequest;
use self::task::SessionTask;

pub struct PeerSession {
    request_tx: mpsc::Sender<SessionRequest>,
    pub status: Arc<Mutex<Option<StatusMessage>>>,
    pub best_height: Arc<Mutex<u64>>,
    pub last_activity: Arc<Mutex<Instant>>,
    pub(crate) is_initiator: bool,
    pub(crate) remote_addr: std::net::SocketAddr,
    pub session_id: u64,
}

impl PeerSession {
    pub fn new<S>(
        stream: RlpxStream<S>,
        remote_addr: std::net::SocketAddr,
        session_id: u64,
        gossip_tx: Option<mpsc::Sender<GossipMessage>>,
        disconnect_tx: Option<mpsc::Sender<DisconnectEvent>>,
        initial_status: Option<StatusMessage>,
    ) -> (Self, SessionTask<S>) 
    where S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + Sync + 'static
    {
        let (tx, rx) = mpsc::channel(32);
        let status = Arc::new(Mutex::new(initial_status.clone()));
        let mut initial_height = 0;
        if let Some(StatusMessage::Eth69(s)) = &initial_status {
            initial_height = s.latest;
        }
        let best_height = Arc::new(Mutex::new(initial_height));
        let last_activity = Arc::new(Mutex::new(Instant::now()));
        let is_initiator = stream.initiator;

        let task = SessionTask::new(
            stream,
            rx,
            session_id,
            gossip_tx,
            disconnect_tx,
            status.clone(),
            best_height.clone(),
            last_activity.clone(),
        );

        let session = Self { 
            request_tx: tx, 
            status, 
            best_height, 
            last_activity,
            is_initiator,
            remote_addr,
            session_id,
        };

        (session, task)
    }

    pub async fn send_status(&self, _status: Status) -> Result<()> {
        // Handled during handshake, but if needed we can add a request variant
        Ok(())
    }
}

#[async_trait]
impl P2pSession for PeerSession {
    async fn get_block_headers(&self, request: RequestPair<GetBlockHeaders>) -> Result<RequestPair<BlockHeaders>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.request_tx.send(SessionRequest::GetHeaders { request, response_tx: tx }).await
            .map_err(|_| anyhow::anyhow!("Session closed"))?;
        timeout(Duration::from_secs(90), rx).await
            .map_err(|_| anyhow::anyhow!("GetBlockHeaders timed out"))?
            .map_err(|_| anyhow::anyhow!("Response channel closed"))?
    }

    async fn get_block_bodies(&self, request: RequestPair<GetBlockBodies>) -> Result<RequestPair<BlockBodies>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.request_tx.send(SessionRequest::GetBodies { request, response_tx: tx }).await
            .map_err(|_| anyhow::anyhow!("Session closed"))?;
        timeout(Duration::from_secs(90), rx).await
            .map_err(|_| anyhow::anyhow!("GetBlockBodies timed out"))?
            .map_err(|_| anyhow::anyhow!("Response channel closed"))?
    }

    async fn get_pooled_transactions(&self, request: RequestPair<GetPooledTransactions>) -> Result<RequestPair<PooledTransactions>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.request_tx.send(SessionRequest::GetPooledTransactions { request, response_tx: tx }).await
            .map_err(|_| anyhow::anyhow!("Session closed"))?;
        timeout(Duration::from_secs(90), rx).await
            .map_err(|_| anyhow::anyhow!("GetPooledTransactions timed out"))?
            .map_err(|_| anyhow::anyhow!("Response channel closed"))?
    }

    async fn get_receipts(&self, request: RequestPair<GetReceipts>) -> Result<RequestPair<Receipts>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.request_tx.send(SessionRequest::GetReceipts { request, response_tx: tx }).await
            .map_err(|_| anyhow::anyhow!("Session closed"))?;
        timeout(Duration::from_secs(90), rx).await
            .map_err(|_| anyhow::anyhow!("GetReceipts timed out"))?
            .map_err(|_| anyhow::anyhow!("Response channel closed"))?
    }

    async fn get_node_data(&self, request: RequestPair<GetNodeData>) -> Result<RequestPair<NodeData>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.request_tx.send(SessionRequest::GetNodeData { request, response_tx: tx }).await
            .map_err(|_| anyhow::anyhow!("Session closed"))?;
        timeout(Duration::from_secs(60), rx).await
            .map_err(|_| anyhow::anyhow!("GetNodeData timed out"))?
            .map_err(|_| anyhow::anyhow!("Response channel closed"))?
    }

    async fn eth_status(&self) -> Option<Status> {
        let guard = self.status.lock().await;
        guard.as_ref().map(|s| match s {
            StatusMessage::Legacy(l) => l.clone(),
            StatusMessage::Eth69(e) => Status {
                version: e.version,
                chain: e.chain,
                total_difficulty: alloy_primitives::U256::ZERO,
                blockhash: e.blockhash,
                genesis: e.genesis,
                forkid: e.forkid.clone(),
            }
        })
    }

    async fn best_height(&self) -> u64 {
        *self.best_height.lock().await
    }

    async fn send_transactions(&self, txs: Transactions) -> Result<()> {
        self.request_tx.send(SessionRequest::SendTransactions(txs)).await
            .map_err(|_| anyhow::anyhow!("Session closed"))
    }

    async fn send_new_pooled_transaction_hashes(&self, hashes: NewPooledTransactionHashes) -> Result<()> {
        self.request_tx.send(SessionRequest::SendNewPooledTransactionHashes(hashes)).await
            .map_err(|_| anyhow::anyhow!("Session closed"))
    }

    async fn ping(&self) -> Result<()> {
        self.request_tx.send(SessionRequest::Ping).await
            .map_err(|_| anyhow::anyhow!("Session closed"))
    }

    async fn disconnect(&self, reason: DisconnectReason) -> Result<()> {
        self.request_tx.send(SessionRequest::Disconnect(wasix_eth_types::p2p::Disconnect { reason })).await
            .map_err(|_| anyhow::anyhow!("Session closed"))
    }

    async fn send_block_headers(&self, headers: RequestPair<BlockHeaders>) -> Result<()> {
        self.request_tx.send(SessionRequest::SendBlockHeaders(headers)).await
            .map_err(|_| anyhow::anyhow!("Session closed"))
    }

    async fn send_block_bodies(&self, bodies: RequestPair<BlockBodies>) -> Result<()> {
        self.request_tx.send(SessionRequest::SendBlockBodies(bodies)).await
            .map_err(|_| anyhow::anyhow!("Session closed"))
    }

    async fn send_pooled_transactions(&self, txs: RequestPair<PooledTransactions>) -> Result<()> {
        self.request_tx.send(SessionRequest::SendPooledTransactions(txs)).await
            .map_err(|_| anyhow::anyhow!("Session closed"))
    }

    async fn send_receipts(&self, receipts: RequestPair<Receipts>) -> Result<()> {
        self.request_tx.send(SessionRequest::SendReceipts(receipts)).await
            .map_err(|_| anyhow::anyhow!("Session closed"))
    }

    async fn send_node_data(&self, data: RequestPair<NodeData>) -> Result<()> {
        self.request_tx.send(SessionRequest::SendNodeData(data)).await
            .map_err(|_| anyhow::anyhow!("Session closed"))
    }
}
