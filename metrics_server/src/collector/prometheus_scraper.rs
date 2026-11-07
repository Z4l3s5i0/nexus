use crate::model::{MetricSample, MetricSource, Node, CollectionError};
use crate::storage::SharedStore;
use crate::telemetry::registry::TelemetryRegistry;
use crate::time::now_ms;
use anyhow::Result;
use dashmap::DashMap;
use std::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct PrometheusScraper {
    store: SharedStore,
    telemetry: TelemetryRegistry,
    timeout: Duration,
    experiment_id: Option<String>,
    last_counter_values: Arc<DashMap<(String, String), u64>>,
}

impl PrometheusScraper {
    pub fn new(store: SharedStore, telemetry: TelemetryRegistry, timeout: Duration, experiment_id: Option<String>) -> Self {
        Self { 
            store, 
            telemetry, 
            timeout, 
            experiment_id,
            last_counter_values: Arc::new(DashMap::new()),
        }
    }

    pub async fn scrape_node(&self, node: Node) -> Result<()> {
        let url = match &node.metrics_url {
            Some(url) => format!("{}/metrics", url),
            None => return Ok(()),
        };

        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .build()?;

        let response = match client.get(url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                let error = CollectionError {
                    timestamp_ms: now_ms(),
                    experiment_id: self.experiment_id.clone(),
                    node_id: node.id.clone(),
                    source: "prometheus_scraper".to_string(),
                    message: format!("Failed to fetch metrics: {}", e),
                    recoverable: true,
                };
                self.store.insert_collection_error(&error)?;
                return Ok(());
            }
        };

        let body = response.text().await?;
        let timestamp = now_ms();

        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if let Some(sample) = self.parse_line(line, &node, timestamp) {
                self.store.insert_metric_sample(&sample)?;
                self.emit_telemetry(&node, &sample);
            }
        }

        Ok(())
    }

    fn parse_line(&self, line: &str, node: &Node, timestamp: i64) -> Option<MetricSample> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            return None;
        }

        let metric_part = parts[0];
        let value_part = parts[1];

        let value: f64 = value_part.parse().ok()?;

        let (name, labels_json) = if let Some(bracket_start) = metric_part.find('{') {
            let name = &metric_part[..bracket_start];
            let bracket_end = metric_part.find('}')?;
            let labels_str = &metric_part[bracket_start + 1..bracket_end];
            let labels = self.parse_labels(labels_str);
            let labels_json = serde_json::to_string(&labels).ok();
            (name.to_string(), labels_json)
        } else {
            (metric_part.to_string(), None)
        };

        Some(MetricSample {
            timestamp_ms: timestamp,
            experiment_id: self.experiment_id.clone(),
            node_id: node.id.clone(),
            network: node.network.clone(),
            client: node.client.clone(),
            source: MetricSource::PrometheusScrape,
            name,
            value,
            labels_json,
        })
    }

    fn parse_labels(&self, labels_str: &str) -> HashMap<String, String> {
        let mut labels = HashMap::new();
        for pair in labels_str.split(',') {
            let kv: Vec<&str> = pair.split('=').collect();
            if kv.len() == 2 {
                let key = kv[0].trim().to_string();
                let val = kv[1].trim().trim_matches('"').to_string();
                labels.insert(key, val);
            }
        }
        labels
    }

    fn emit_telemetry(&self, node: &Node, sample: &MetricSample) {
        let labels = [node.id.as_str(), node.network.as_str(), node.client.as_str()];
        
        let get_delta = |metric_name: &str, current_val: f64| -> u64 {
            let current_u64 = current_val as u64;
            let key = (node.id.clone(), metric_name.to_string());
            let mut entry = self.last_counter_values.entry(key).or_insert(0);
            let prev_val = *entry;
            
            if current_u64 >= prev_val {
                let delta = current_u64 - prev_val;
                *entry = current_u64;
                delta
            } else {
                // Counter reset
                *entry = current_u64;
                current_u64
            }
        };

        match sample.name.as_str() {
            "sync_status" => self.telemetry.node_sync_status.with_label_values(&labels).set(sample.value),
            "current_head_block" => self.telemetry.node_current_head_block.with_label_values(&labels).set(sample.value),
            "connected_peers" => self.telemetry.node_connected_peers.with_label_values(&labels).set(sample.value),
            "blocks_imported_total" => {
                let delta = get_delta("blocks_imported_total", sample.value);
                self.telemetry.node_blocks_imported_total.with_label_values(&labels).inc_by(delta);
            },
            "transactions_committed_total" => {
                let delta = get_delta("transactions_committed_total", sample.value);
                self.telemetry.node_transactions_committed_total.with_label_values(&labels).inc_by(delta);
            },
            "mempool_size" => self.telemetry.node_mempool_size.with_label_values(&labels).set(sample.value),
            "mempool_rejected_transactions_total" => {
                let delta = get_delta("mempool_rejected_transactions_total", sample.value);
                self.telemetry.node_mempool_rejected_transactions_total.with_label_values(&labels).inc_by(delta);
            },
            "evicted_transactions_total" => {
                let delta = get_delta("evicted_transactions_total", sample.value);
                self.telemetry.node_evicted_transactions_total.with_label_values(&labels).inc_by(delta);
            },
            "gossip_messages_received_total" => {
                let delta = get_delta("gossip_messages_received_total", sample.value);
                self.telemetry.node_gossip_messages_received_total.with_label_values(&labels).inc_by(delta);
            },
            "p2p_messages_sent_bytes_total" => {
                let delta = get_delta("p2p_messages_sent_bytes_total", sample.value);
                self.telemetry.node_p2p_messages_sent_bytes_total.with_label_values(&labels).inc_by(delta);
            },
            "rpc_requests_total" => {
                let delta = get_delta("rpc_requests_total", sample.value);
                self.telemetry.node_rpc_requests_total.with_label_values(&labels).inc_by(delta);
            },
            "sync_target_height" => self.telemetry.node_sync_target_height.with_label_values(&labels).set(sample.value),
            _ => {}
        }
    }
}
