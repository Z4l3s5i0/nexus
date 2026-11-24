use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub collection: CollectionConfig,
    pub experiment: Option<ExperimentConfig>,
    pub bootstrap: Option<BootstrapConfig>,
    #[serde(default)]
    pub nodes: Vec<NodeConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapConfig {
    pub enabled: bool,
    pub network: Option<String>,
    pub chain_id: Option<u64>,
    pub registration_token: Option<String>,
    pub probe_interval_seconds: u64,
    pub stale_after_seconds: u64,
    pub max_bootstrap_nodes: usize,
    pub allow_private_ips: bool,
    pub allow_loopback_ips: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bind_addr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub kind: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionConfig {
    pub interval_seconds: u64,
    pub timeout_ms: u64,
    pub max_concurrent_nodes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentConfig {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeConfig {
    pub id: String,
    pub network: String,
    pub chain_id: Option<u64>,
    pub client: String,
    pub rpc_url: String,
    pub metrics_url: Option<String>,
    pub p2p_addr: Option<String>,
    pub discovery_addr: Option<String>,
    pub enode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Experiment {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub network: String,
    pub chain_id: Option<u64>,
    pub client: String,
    pub rpc_url: String,
    pub metrics_url: Option<String>,

    pub p2p_addr: Option<String>,
    pub discovery_addr: Option<String>,
    pub enode: Option<String>,

    pub status: NodeStatus,
    pub last_seen_ms: Option<i64>,
    pub last_successful_probe_ms: Option<i64>,
    pub consecutive_failures: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum NodeStatus {
    #[default]
    Pending,
    Active,
    Stale,
    Unhealthy,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterNodeRequest {
    pub id: String,
    pub network: String,
    pub chain_id: Option<u64>,
    pub client: String,
    pub rpc_url: String,
    pub metrics_url: Option<String>,
    pub p2p_addr: Option<String>,
    pub discovery_addr: Option<String>,
    pub enode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapNodesQuery {
    pub network: Option<String>,
    pub chain_id: Option<u64>,
    pub limit: Option<usize>,
    pub exclude_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapNode {
    pub id: String,
    pub network: String,
    pub chain_id: Option<u64>,
    pub client: String,
    pub p2p_addr: Option<String>,
    pub discovery_addr: Option<String>,
    pub enode: Option<String>,
    pub last_seen_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapNodesResponse {
    pub nodes: Vec<BootstrapNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcObservation {
    pub timestamp_ms: i64,
    pub experiment_id: Option<String>,
    pub node_id: String,
    pub network: String,
    pub client: String,
    pub method: String,
    pub success: bool,
    pub latency_ms: u64,
    pub value_json: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSample {
    pub timestamp_ms: i64,
    pub experiment_id: Option<String>,
    pub node_id: String,
    pub network: String,
    pub client: String,
    pub source: MetricSource,
    pub name: String,
    pub value: f64,
    pub labels_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricIngestionRequest {
    pub samples: Vec<MetricSample>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MetricSource {
    JsonRpc,
    PrometheusScrape,
    System,
    Log,
    Derived,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionError {
    pub timestamp_ms: i64,
    pub experiment_id: Option<String>,
    pub node_id: String,
    pub source: String,
    pub message: String,
    pub recoverable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricRangeQuery {
    pub metric: String,
    pub node_id: Option<String>,
    pub experiment_id: Option<String>,
    pub from_ms: i64,
    pub to_ms: i64,
    pub labels: Option<std::collections::HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPoint {
    pub timestamp_ms: i64,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricRangeResponse {
    pub metric: String,
    pub node_id: Option<String>,
    pub experiment_id: Option<String>,
    pub points: Vec<MetricPoint>,
}
