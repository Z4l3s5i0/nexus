pub mod types;
pub mod task;

use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::{timeout, Duration, MissedTickBehavior};
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

const CHANNEL_REPOLL_INTERVAL: Duration = Duration::from_millis(250);

async fn periodically_repoll<F: Future>(future: F) -> F::Output {
    tokio::pin!(future);
    let mut repoll_interval = tokio::time::interval(CHANNEL_REPOLL_INTERVAL);
    repoll_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            result = &mut future => return result,
            _ = repoll_interval.tick() => {}
        }
    }
}

async fn send_to_session(
    request_tx: &mpsc::Sender<SessionRequest>,
    request: SessionRequest,
) -> Result<()> {
    let permit = periodically_repoll(request_tx.reserve())
        .await
        .map_err(|_| anyhow::anyhow!("Session closed"))?;
    permit.send(request);
    Ok(())
}

async fn request_with_timeout<T, F>(
    request_tx: &mpsc::Sender<SessionRequest>,
    timeout_duration: Duration,
    timeout_message: &'static str,
    create_request: F,
) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(oneshot::Sender<Result<T>>) -> SessionRequest + Send,
{
    let (response_tx, response_rx) = oneshot::channel();
    let request = create_request(response_tx);

    timeout(timeout_duration, async {
        let permit = periodically_repoll(request_tx.reserve())
            .await
            .map_err(|_| anyhow::anyhow!("Session closed"))?;
        permit.send(request);

        periodically_repoll(response_rx)
            .await
            .map_err(|_| anyhow::anyhow!("Response channel closed"))?
    })
    .await
    .map_err(|_| anyhow::anyhow!(timeout_message))?
}

#[async_trait]
impl P2pSession for PeerSession {
    async fn get_block_headers(&self, request: RequestPair<GetBlockHeaders>) -> Result<RequestPair<BlockHeaders>> {
        request_with_timeout(
            &self.request_tx,
            Duration::from_secs(90),
            "GetBlockHeaders timed out",
            |response_tx| SessionRequest::GetHeaders { request, response_tx },
        )
        .await
    }

    async fn get_block_bodies(&self, request: RequestPair<GetBlockBodies>) -> Result<RequestPair<BlockBodies>> {
        request_with_timeout(
            &self.request_tx,
            Duration::from_secs(90),
            "GetBlockBodies timed out",
            |response_tx| SessionRequest::GetBodies { request, response_tx },
        )
        .await
    }

    async fn get_pooled_transactions(&self, request: RequestPair<GetPooledTransactions>) -> Result<RequestPair<PooledTransactions>> {
        request_with_timeout(
            &self.request_tx,
            Duration::from_secs(90),
            "GetPooledTransactions timed out",
            |response_tx| SessionRequest::GetPooledTransactions { request, response_tx },
        )
        .await
    }

    async fn get_receipts(&self, request: RequestPair<GetReceipts>) -> Result<RequestPair<Receipts>> {
        request_with_timeout(
            &self.request_tx,
            Duration::from_secs(90),
            "GetReceipts timed out",
            |response_tx| SessionRequest::GetReceipts { request, response_tx },
        )
        .await
    }

    async fn get_node_data(&self, request: RequestPair<GetNodeData>) -> Result<RequestPair<NodeData>> {
        request_with_timeout(
            &self.request_tx,
            Duration::from_secs(60),
            "GetNodeData timed out",
            |response_tx| SessionRequest::GetNodeData { request, response_tx },
        )
        .await
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
        send_to_session(&self.request_tx, SessionRequest::SendTransactions(txs)).await
    }

    async fn send_new_pooled_transaction_hashes(&self, hashes: NewPooledTransactionHashes) -> Result<()> {
        send_to_session(
            &self.request_tx,
            SessionRequest::SendNewPooledTransactionHashes(hashes),
        )
        .await
    }

    async fn ping(&self) -> Result<()> {
        send_to_session(&self.request_tx, SessionRequest::Ping).await
    }

    async fn disconnect(&self, reason: DisconnectReason) -> Result<()> {
        send_to_session(
            &self.request_tx,
            SessionRequest::Disconnect(wasix_eth_types::p2p::Disconnect { reason }),
        )
        .await
    }

    async fn send_block_headers(&self, headers: RequestPair<BlockHeaders>) -> Result<()> {
        send_to_session(&self.request_tx, SessionRequest::SendBlockHeaders(headers)).await
    }

    async fn send_block_bodies(&self, bodies: RequestPair<BlockBodies>) -> Result<()> {
        send_to_session(&self.request_tx, SessionRequest::SendBlockBodies(bodies)).await
    }

    async fn send_pooled_transactions(&self, txs: RequestPair<PooledTransactions>) -> Result<()> {
        send_to_session(&self.request_tx, SessionRequest::SendPooledTransactions(txs)).await
    }

    async fn send_receipts(&self, receipts: RequestPair<Receipts>) -> Result<()> {
        send_to_session(&self.request_tx, SessionRequest::SendReceipts(receipts)).await
    }

    async fn send_node_data(&self, data: RequestPair<NodeData>) -> Result<()> {
        send_to_session(&self.request_tx, SessionRequest::SendNodeData(data)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    struct FutureWithoutWake(Arc<AtomicBool>);

    impl Future for FutureWithoutWake {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            if self.0.load(Ordering::SeqCst) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }

    #[tokio::test]
    async fn periodically_repolls_future_without_a_waker_notification() {
        let ready = Arc::new(AtomicBool::new(false));
        let signal = ready.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            signal.store(true, Ordering::SeqCst);
        });

        timeout(
            Duration::from_secs(2),
            periodically_repoll(FutureWithoutWake(ready)),
        )
        .await
        .expect("periodic repoll should observe readiness without a wake");
    }

    #[tokio::test]
    async fn request_timeout_includes_queue_admission() {
        let (request_tx, mut request_rx) = mpsc::channel(1);
        request_tx.send(SessionRequest::Ping).await.unwrap();

        let error = request_with_timeout::<(), _>(
            &request_tx,
            Duration::from_millis(40),
            "request timed out",
            |_| SessionRequest::Ping,
        )
        .await
        .unwrap_err();

        assert_eq!(error.to_string(), "request timed out");
        assert!(matches!(request_rx.try_recv(), Ok(SessionRequest::Ping)));
        assert!(matches!(
            request_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn request_progresses_after_queue_backpressure_is_released() {
        let (request_tx, mut request_rx) = mpsc::channel(1);
        request_tx.send(SessionRequest::Ping).await.unwrap();

        let sender = request_tx.clone();
        let request_task = tokio::spawn(async move {
            request_with_timeout(
                &sender,
                Duration::from_secs(2),
                "request timed out",
                |response_tx| {
                    tokio::spawn(async move {
                        let _ = response_tx.send(Ok(()));
                    });
                    SessionRequest::Ping
                },
            )
            .await
        });

        assert!(matches!(request_rx.recv().await, Some(SessionRequest::Ping)));
        assert!(matches!(request_rx.recv().await, Some(SessionRequest::Ping)));
        request_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn request_reports_closed_response_channel() {
        let (request_tx, mut request_rx) = mpsc::channel(1);

        let error = request_with_timeout::<(), _>(
            &request_tx,
            Duration::from_secs(1),
            "request timed out",
            |response_tx| {
                drop(response_tx);
                SessionRequest::Ping
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.to_string(), "Response channel closed");
        assert!(matches!(request_rx.try_recv(), Ok(SessionRequest::Ping)));
    }
}
