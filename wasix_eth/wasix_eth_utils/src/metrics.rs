use prometheus::{
    register_counter, register_gauge, Counter, Gauge,
};
use lazy_static::lazy_static;

lazy_static! {
    pub static ref SYNC_STATUS: Gauge = register_gauge!("sync_status", "Sync status (1: synced, 0: syncing, -1: stalled)").unwrap();
    pub static ref CURRENT_HEAD_BLOCK: Gauge = register_gauge!("current_head_block", "Current head block height").unwrap();
    pub static ref CONNECTED_PEERS: Gauge = register_gauge!("connected_peers", "Number of currently connected peers").unwrap();
    pub static ref BLOCKS_IMPORTED_TOTAL: Gauge = register_gauge!("blocks_imported_total", "Total number of blocks successfully imported").unwrap();
    pub static ref TRANSACTIONS_COMMITTED_TOTAL: Gauge = register_gauge!("transactions_committed_total", "Total number of successfully committed transactions").unwrap();

    // Mempool Metrics
    pub static ref MEMPOOL_SIZE: Gauge = register_gauge!("mempool_size", "Current number of transactions in the mempool").unwrap();
    pub static ref MEMPOOL_REJECTED_TRANSACTIONS: Counter = register_counter!("mempool_rejected_transactions_total", "Total number of transactions rejected by the mempool").unwrap();
    pub static ref EVICTED_TRANSACTIONS: Counter = register_counter!("evicted_transactions_total", "Total number of transactions evicted from the mempool").unwrap();

    // Network Metrics
    pub static ref GOSSIP_MESSAGES_RECEIVED: Counter = register_counter!("gossip_messages_received_total", "Total gossip messages received").unwrap();
    pub static ref P2P_MESSAGES_SENT_BYTES: Counter = register_counter!("p2p_messages_sent_bytes_total", "Total bytes sent over the p2p network").unwrap();

    // RPC Metrics
    pub static ref RPC_REQUESTS_TOTAL: Counter = register_counter!("rpc_requests_total", "Total number of RPC requests received").unwrap();

    // Sync Enhancements
    pub static ref SYNC_TARGET_HEIGHT: Gauge = register_gauge!("sync_target_height", "The target block height the node is syncing towards").unwrap();
}

pub fn init_metrics() {
    let _ = *SYNC_STATUS;
    let _ = *CURRENT_HEAD_BLOCK;
    let _ = *CONNECTED_PEERS;
    let _ = *BLOCKS_IMPORTED_TOTAL;
    let _ = *TRANSACTIONS_COMMITTED_TOTAL;

    let _ = *MEMPOOL_SIZE;
    let _ = *MEMPOOL_REJECTED_TRANSACTIONS;
    let _ = *EVICTED_TRANSACTIONS;
    let _ = *GOSSIP_MESSAGES_RECEIVED;
    let _ = *P2P_MESSAGES_SENT_BYTES;

    let _ = *RPC_REQUESTS_TOTAL;

    let _ = *SYNC_TARGET_HEIGHT;
}
