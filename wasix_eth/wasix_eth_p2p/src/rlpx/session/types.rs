use anyhow::Result;
use tokio::sync::oneshot;
use wasix_eth_types::p2p::{
    NewPooledTransactionHashes, Transactions,
    GetBlockHeaders, BlockHeaders, GetBlockBodies, BlockBodies,
    GetPooledTransactions, PooledTransactions, GetReceipts, Receipts,
    GetNodeData, NodeData,
    RequestPair, Disconnect
};

pub enum SessionRequest {
    GetHeaders {
        request: RequestPair<GetBlockHeaders>,
        response_tx: oneshot::Sender<Result<RequestPair<BlockHeaders>>>,
    },
    GetBodies {
        request: RequestPair<GetBlockBodies>,
        response_tx: oneshot::Sender<Result<RequestPair<BlockBodies>>>,
    },
    GetPooledTransactions {
        request: RequestPair<GetPooledTransactions>,
        response_tx: oneshot::Sender<Result<RequestPair<PooledTransactions>>>,
    },
    GetReceipts {
        request: RequestPair<GetReceipts>,
        response_tx: oneshot::Sender<Result<RequestPair<Receipts>>>,
    },
    GetNodeData {
        request: RequestPair<GetNodeData>,
        response_tx: oneshot::Sender<Result<RequestPair<NodeData>>>,
    },
    SendTransactions(Transactions),
    SendNewPooledTransactionHashes(NewPooledTransactionHashes),
    Ping,
    Disconnect(Disconnect),
    SendBlockHeaders(RequestPair<BlockHeaders>),
    SendBlockBodies(RequestPair<BlockBodies>),
    SendPooledTransactions(RequestPair<PooledTransactions>),
    SendReceipts(RequestPair<Receipts>),
    SendNodeData(RequestPair<NodeData>),
}
