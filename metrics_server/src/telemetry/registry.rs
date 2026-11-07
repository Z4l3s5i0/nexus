use prometheus::{Encoder, IntCounterVec, GaugeVec, Registry, TextEncoder};
use anyhow::Result;

#[derive(Clone)]
pub struct TelemetryRegistry {
    pub registry: Registry,
    pub collection_ticks: IntCounterVec,
    pub collection_errors: IntCounterVec,
    pub rpc_requests: IntCounterVec,
    pub rpc_errors: IntCounterVec,
    pub rpc_latency: GaugeVec,
    pub ethereum_rpc_up: GaugeVec,
    pub ethereum_latest_block: GaugeVec,
    pub ethereum_chain_id: GaugeVec,
    pub ethereum_peer_count: GaugeVec,
    pub ethereum_gas_price: GaugeVec,
    pub ethereum_syncing: GaugeVec,
    pub node_sync_status: GaugeVec,
    pub node_current_head_block: GaugeVec,
    pub node_connected_peers: GaugeVec,
    pub node_blocks_imported_total: IntCounterVec,
    pub node_transactions_committed_total: IntCounterVec,
    pub node_mempool_size: GaugeVec,
    pub node_mempool_rejected_transactions_total: IntCounterVec,
    pub node_evicted_transactions_total: IntCounterVec,
    pub node_gossip_messages_received_total: IntCounterVec,
    pub node_p2p_messages_sent_bytes_total: IntCounterVec,
    pub node_rpc_requests_total: IntCounterVec,
    pub node_sync_target_height: GaugeVec,
    pub bootstrap_nodes: GaugeVec,
    pub bootstrap_registration_requests: IntCounterVec,
    pub bootstrap_probe_total: IntCounterVec,
}

impl TelemetryRegistry {
    pub fn new() -> Result<Self> {
        let registry = Registry::new();
        
        let collection_ticks = IntCounterVec::new(
            prometheus::opts!("metrics_server_collection_ticks_total", "Total number of collection ticks"),
            &[]
        )?;
        let collection_errors = IntCounterVec::new(
            prometheus::opts!("metrics_server_collection_errors_total", "Total number of collection errors"),
            &["node_id", "source"]
        )?;
        let rpc_requests = IntCounterVec::new(
            prometheus::opts!("metrics_server_rpc_requests_total", "Total number of RPC requests"),
            &["node_id", "method"]
        )?;
        let rpc_errors = IntCounterVec::new(
            prometheus::opts!("metrics_server_rpc_errors_total", "Total number of RPC errors"),
            &["node_id", "method"]
        )?;
        let rpc_latency = GaugeVec::new(
            prometheus::opts!("metrics_server_rpc_latency_ms", "RPC latency in milliseconds"),
            &["node_id", "method"]
        )?;
        let ethereum_rpc_up = GaugeVec::new(
            prometheus::opts!("ethereum_rpc_up", "Ethereum RPC up status"),
            &["node_id", "network", "client"]
        )?;
        let ethereum_latest_block = GaugeVec::new(
            prometheus::opts!("ethereum_latest_block", "Latest block number"),
            &["node_id", "network", "client"]
        )?;
        let ethereum_chain_id = GaugeVec::new(
            prometheus::opts!("ethereum_chain_id", "Ethereum chain ID"),
            &["node_id", "network", "client"]
        )?;
        let ethereum_peer_count = GaugeVec::new(
            prometheus::opts!("ethereum_peer_count", "Ethereum peer count"),
            &["node_id", "network", "client"]
        )?;
        let ethereum_gas_price = GaugeVec::new(
            prometheus::opts!("ethereum_gas_price", "Ethereum gas price"),
            &["node_id", "network", "client"]
        )?;
        let ethereum_syncing = GaugeVec::new(
            prometheus::opts!("ethereum_syncing", "Ethereum syncing status"),
            &["node_id", "network", "client"]
        )?;
        let node_sync_status = GaugeVec::new(
            prometheus::opts!("sync_status", "Sync status (1: synced, 0: syncing, -1: stalled)"),
            &["node_id", "network", "client"]
        )?;
        let node_current_head_block = GaugeVec::new(
            prometheus::opts!("current_head_block", "Current head block height"),
            &["node_id", "network", "client"]
        )?;
        let node_connected_peers = GaugeVec::new(
            prometheus::opts!("connected_peers", "Number of currently connected peers"),
            &["node_id", "network", "client"]
        )?;
        let node_blocks_imported_total = IntCounterVec::new(
            prometheus::opts!("blocks_imported_total", "Total number of blocks successfully imported"),
            &["node_id", "network", "client"]
        )?;
        let node_transactions_committed_total = IntCounterVec::new(
            prometheus::opts!("transactions_committed_total", "Total number of successfully committed transactions"),
            &["node_id", "network", "client"]
        )?;
        let node_mempool_size = GaugeVec::new(
            prometheus::opts!("mempool_size", "Current number of transactions in the mempool"),
            &["node_id", "network", "client"]
        )?;
        let node_mempool_rejected_transactions_total = IntCounterVec::new(
            prometheus::opts!("mempool_rejected_transactions_total", "Total number of transactions rejected by the mempool"),
            &["node_id", "network", "client"]
        )?;
        let node_evicted_transactions_total = IntCounterVec::new(
            prometheus::opts!("evicted_transactions_total", "Total number of transactions evicted from the mempool"),
            &["node_id", "network", "client"]
        )?;
        let node_gossip_messages_received_total = IntCounterVec::new(
            prometheus::opts!("gossip_messages_received_total", "Total gossip messages received"),
            &["node_id", "network", "client"]
        )?;
        let node_p2p_messages_sent_bytes_total = IntCounterVec::new(
            prometheus::opts!("p2p_messages_sent_bytes_total", "Total bytes sent over the p2p network"),
            &["node_id", "network", "client"]
        )?;
        let node_rpc_requests_total = IntCounterVec::new(
            prometheus::opts!("rpc_requests_total", "Total number of RPC requests received"),
            &["node_id", "network", "client"]
        )?;
        let node_sync_target_height = GaugeVec::new(
            prometheus::opts!("sync_target_height", "The target block height the node is syncing towards"),
            &["node_id", "network", "client"]
        )?;
        let bootstrap_nodes = GaugeVec::new(
            prometheus::opts!("metrics_server_bootstrap_nodes", "Number of nodes in bootstrap registry"),
            &["status", "network"]
        )?;
        let bootstrap_registration_requests = IntCounterVec::new(
            prometheus::opts!("metrics_server_bootstrap_registration_requests_total", "Total number of registration requests"),
            &["result"]
        )?;
        let bootstrap_probe_total = IntCounterVec::new(
            prometheus::opts!("metrics_server_bootstrap_probe_total", "Total number of bootstrap probes"),
            &["result"]
        )?;

        registry.register(Box::new(collection_ticks.clone()))?;
        registry.register(Box::new(collection_errors.clone()))?;
        registry.register(Box::new(rpc_requests.clone()))?;
        registry.register(Box::new(rpc_errors.clone()))?;
        registry.register(Box::new(rpc_latency.clone()))?;
        registry.register(Box::new(ethereum_rpc_up.clone()))?;
        registry.register(Box::new(ethereum_latest_block.clone()))?;
        registry.register(Box::new(ethereum_chain_id.clone()))?;
        registry.register(Box::new(ethereum_peer_count.clone()))?;
        registry.register(Box::new(ethereum_gas_price.clone()))?;
        registry.register(Box::new(ethereum_syncing.clone()))?;
        registry.register(Box::new(node_sync_status.clone()))?;
        registry.register(Box::new(node_current_head_block.clone()))?;
        registry.register(Box::new(node_connected_peers.clone()))?;
        registry.register(Box::new(node_blocks_imported_total.clone()))?;
        registry.register(Box::new(node_transactions_committed_total.clone()))?;
        registry.register(Box::new(node_mempool_size.clone()))?;
        registry.register(Box::new(node_mempool_rejected_transactions_total.clone()))?;
        registry.register(Box::new(node_evicted_transactions_total.clone()))?;
        registry.register(Box::new(node_gossip_messages_received_total.clone()))?;
        registry.register(Box::new(node_p2p_messages_sent_bytes_total.clone()))?;
        registry.register(Box::new(node_rpc_requests_total.clone()))?;
        registry.register(Box::new(node_sync_target_height.clone()))?;
        registry.register(Box::new(bootstrap_nodes.clone()))?;
        registry.register(Box::new(bootstrap_registration_requests.clone()))?;
        registry.register(Box::new(bootstrap_probe_total.clone()))?;

        Ok(Self {
            registry,
            collection_ticks,
            collection_errors,
            rpc_requests,
            rpc_errors,
            rpc_latency,
            ethereum_rpc_up,
            ethereum_latest_block,
            ethereum_chain_id,
            ethereum_peer_count,
            ethereum_gas_price,
            ethereum_syncing,
            node_sync_status,
            node_current_head_block,
            node_connected_peers,
            node_blocks_imported_total,
            node_transactions_committed_total,
            node_mempool_size,
            node_mempool_rejected_transactions_total,
            node_evicted_transactions_total,
            node_gossip_messages_received_total,
            node_p2p_messages_sent_bytes_total,
            node_rpc_requests_total,
            node_sync_target_height,
            bootstrap_nodes,
            bootstrap_registration_requests,
            bootstrap_probe_total,
        })
    }

    pub fn gather_text(&self) -> Result<String> {
        let mut buffer = Vec::new();
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        encoder.encode(&metric_families, &mut buffer)?;
        Ok(String::from_utf8(buffer)?)
    }
}
