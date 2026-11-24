use crate::ethereum::rpc_client::EthereumRpcClient;
use crate::ethereum::types::{parse_hex_f64, syncing_to_numeric};
use crate::model::{MetricSample, MetricSource, Node, RpcObservation};
use crate::storage::SharedStore;
use crate::telemetry::registry::TelemetryRegistry;
use crate::time::now_ms;
use anyhow::Result;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct RpcCollector {
    store: SharedStore,
    telemetry: TelemetryRegistry,
    timeout: Duration,
    experiment_id: Option<String>,
}

impl RpcCollector {
    pub fn new(
        store: SharedStore,
        telemetry: TelemetryRegistry,
        timeout: Duration,
        experiment_id: Option<String>,
    ) -> Self {
        Self {
            store,
            telemetry,
            timeout,
            experiment_id,
        }
    }

    pub async fn collect_node(&self, node: Node) -> Result<()> {
        let client = EthereumRpcClient::new(node.rpc_url.clone(), self.timeout)?;
        
        let methods = [
            "eth_chainId",
            "eth_blockNumber",
            "eth_syncing",
            "eth_gasPrice",
            "net_peerCount",
            "web3_clientVersion",
        ];

        let mut success_count = 0;

        for method in methods {
            let start = Instant::now();
            self.telemetry.rpc_requests.with_label_values(&[node.id.as_str(), method]).inc();
            
            let result = match method {
                "eth_chainId" => client.chain_id().await.map(|v| serde_json::to_value(v).unwrap()),
                "eth_blockNumber" => client.block_number().await.map(|v| serde_json::to_value(v).unwrap()),
                "eth_syncing" => client.syncing().await,
                "eth_gasPrice" => client.gas_price().await.map(|v| serde_json::to_value(v).unwrap()),
                "net_peerCount" => client.peer_count().await.map(|v| serde_json::to_value(v).unwrap()),
                "web3_clientVersion" => client.client_version().await.map(|v| serde_json::to_value(v).unwrap()),
                _ => unreachable!(),
            };

            let latency = start.elapsed().as_millis() as u64;
            let timestamp = now_ms();
            
            self.telemetry.rpc_latency.with_label_values(&[node.id.as_str(), method]).set(latency as f64);

            match result {
                Ok(value) => {
                    success_count += 1;
                    let observation = RpcObservation {
                        timestamp_ms: timestamp,
                        experiment_id: self.experiment_id.clone(),
                        node_id: node.id.clone(),
                        network: node.network.clone(),
                        client: node.client.clone(),
                        method: method.to_string(),
                        success: true,
                        latency_ms: latency,
                        value_json: Some(value.to_string()),
                        error: None,
                    };
                    self.store.insert_rpc_observation(&observation)?;

                    // Emit metrics
                    self.emit_metrics_from_rpc(&node, method, &value, timestamp)?;
                }
                Err(e) => {
                    self.telemetry.rpc_errors.with_label_values(&[node.id.as_str(), method]).inc();
                    let observation = RpcObservation {
                        timestamp_ms: timestamp,
                        experiment_id: self.experiment_id.clone(),
                        node_id: node.id.clone(),
                        network: node.network.clone(),
                        client: node.client.clone(),
                        method: method.to_string(),
                        success: false,
                        latency_ms: latency,
                        value_json: None,
                        error: Some(e.to_string()),
                    };
                    self.store.insert_rpc_observation(&observation)?;
                }
            }
        }

        let up = if success_count > 0 { 1.0 } else { 0.0 };
        self.telemetry.ethereum_rpc_up.with_label_values(&[node.id.as_str(), node.network.as_str(), node.client.as_str()]).set(up);
        self.store.insert_metric_sample(&rpc_status_sample(
            &node,
            self.experiment_id.clone(),
            now_ms(),
            up,
        ))?;

        Ok(())
    }

    fn emit_metrics_from_rpc(&self, node: &Node, method: &str, value: &serde_json::Value, timestamp: i64) -> Result<()> {
        let metric_name = match method {
            "eth_chainId" => "ethereum_chain_id",
            "eth_blockNumber" => "ethereum_latest_block",
            "eth_syncing" => "ethereum_syncing",
            "eth_gasPrice" => "ethereum_gas_price",
            "net_peerCount" => "ethereum_peer_count",
            _ => return Ok(()),
        };

        let val = if method == "eth_syncing" {
            syncing_to_numeric(value)?
        } else {
            parse_hex_f64(value)?
        };

        // Update telemetry
        match metric_name {
            "ethereum_chain_id" => self.telemetry.ethereum_chain_id.with_label_values(&[node.id.as_str(), node.network.as_str(), node.client.as_str()]).set(val),
            "ethereum_latest_block" => self.telemetry.ethereum_latest_block.with_label_values(&[node.id.as_str(), node.network.as_str(), node.client.as_str()]).set(val),
            "ethereum_syncing" => self.telemetry.ethereum_syncing.with_label_values(&[node.id.as_str(), node.network.as_str(), node.client.as_str()]).set(val),
            "ethereum_gas_price" => self.telemetry.ethereum_gas_price.with_label_values(&[node.id.as_str(), node.network.as_str(), node.client.as_str()]).set(val),
            "ethereum_peer_count" => self.telemetry.ethereum_peer_count.with_label_values(&[node.id.as_str(), node.network.as_str(), node.client.as_str()]).set(val),
            _ => {}
        }

        let sample = MetricSample {
            timestamp_ms: timestamp,
            experiment_id: self.experiment_id.clone(),
            node_id: node.id.clone(),
            network: node.network.clone(),
            client: node.client.clone(),
            source: MetricSource::JsonRpc,
            name: metric_name.to_string(),
            value: val,
            labels_json: None,
        };
        self.store.insert_metric_sample(&sample)?;

        Ok(())
    }
}

fn rpc_status_sample(
    node: &Node,
    experiment_id: Option<String>,
    timestamp_ms: i64,
    value: f64,
) -> MetricSample {
    MetricSample {
        timestamp_ms,
        experiment_id,
        node_id: node.id.clone(),
        network: node.network.clone(),
        client: node.client.clone(),
        source: MetricSource::Derived,
        name: "ethereum_rpc_up".to_string(),
        value,
        labels_json: Some(r#"{"semantics":"at_least_one_rpc_method_succeeded"}"#.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeStatus;

    #[test]
    fn rpc_status_sample_preserves_run_node_and_client_identity() {
        let node = Node {
            id: "node-a".into(),
            network: "12345".into(),
            chain_id: Some(12345),
            client: "wasix_eth".into(),
            rpc_url: "http://localhost:8545".into(),
            metrics_url: None,
            p2p_addr: None,
            discovery_addr: None,
            enode: None,
            status: NodeStatus::Active,
            last_seen_ms: None,
            last_successful_probe_ms: None,
            consecutive_failures: 0,
        };

        let sample = rpc_status_sample(&node, Some("run-1".into()), 123, 1.0);

        assert_eq!(sample.experiment_id.as_deref(), Some("run-1"));
        assert_eq!(sample.node_id, "node-a");
        assert_eq!(sample.client, "wasix_eth");
        assert_eq!(sample.source, MetricSource::Derived);
        assert_eq!(sample.name, "ethereum_rpc_up");
        assert_eq!(sample.value, 1.0);
    }
}
