use crate::model::{MetricSample, MetricSource, Node, CollectionError};
use crate::storage::SharedStore;
use crate::telemetry::registry::TelemetryRegistry;
use crate::time::now_ms;
use anyhow::Result;
use dashmap::DashMap;
use std::time::Duration;
use std::collections::BTreeMap;
use std::iter::Peekable;
use std::str::Chars;
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
            Some(url) => match reqwest::Url::parse(url) {
                Ok(mut url) => {
                    if url.path().is_empty() || url.path() == "/" {
                        url.set_path("/metrics");
                    }
                    url
                }
                Err(e) => {
                    self.record_error(&node, format!("Failed to fetch metrics: {}", e))?;
                    return Ok(());
                }
            },
            None => return Ok(()),
        };

        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        let response = match client.get(url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                self.record_error(&node, format!("Failed to fetch metrics: {}", e))?;
                return Ok(());
            }
        };

        if !response.status().is_success() {
            self.record_error(&node, format!("Failed to fetch metrics: HTTP {}", response.status()))?;
            return Ok(());
        }

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

    fn record_error(&self, node: &Node, message: String) -> Result<()> {
        self.store.insert_collection_error(&CollectionError {
            timestamp_ms: now_ms(),
            experiment_id: self.experiment_id.clone(),
            node_id: node.id.clone(),
            source: "prometheus_scraper".to_string(),
            message,
            recoverable: true,
        })
    }

    fn parse_line(&self, line: &str, node: &Node, timestamp: i64) -> Option<MetricSample> {
        let mut chars = line.chars().peekable();
        let first = chars.next()?;
        if !first.is_ascii_alphabetic() && first != '_' && first != ':' {
            return None;
        }
        let mut name = first.to_string();
        while chars.peek().is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':') {
            name.push(chars.next()?);
        }

        let labels_json = if chars.peek() == Some(&'{') {
            chars.next();
            let labels = self.parse_labels(&mut chars)?;
            if labels.is_empty() { None } else { Some(serde_json::to_string(&labels).ok()?) }
        } else {
            None
        };

        if !chars.peek().is_some_and(|c| c.is_whitespace()) {
            return None;
        }
        let rest: String = chars.collect();
        let mut parts = rest.split_whitespace();
        let value: f64 = parts.next()?.parse().ok()?;
        if !value.is_finite() {
            return None;
        }
        if let Some(sample_timestamp) = parts.next() {
            sample_timestamp.parse::<i64>().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }

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

    fn parse_labels(&self, chars: &mut Peekable<Chars<'_>>) -> Option<BTreeMap<String, String>> {
        let mut labels = BTreeMap::new();
        loop {
            while chars.peek().is_some_and(|c| c.is_whitespace()) { chars.next(); }
            if chars.peek() == Some(&'}') {
                chars.next();
                return Some(labels);
            }
            let first = chars.next()?;
            if !first.is_ascii_alphabetic() && first != '_' { return None; }
            let mut key = first.to_string();
            while chars.peek().is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_') {
                key.push(chars.next()?);
            }
            while chars.peek().is_some_and(|c| c.is_whitespace()) { chars.next(); }
            if chars.next()? != '=' { return None; }
            while chars.peek().is_some_and(|c| c.is_whitespace()) { chars.next(); }
            if chars.next()? != '"' { return None; }
            let mut value = String::new();
            loop {
                match chars.next()? {
                    '"' => break,
                    '\\' => value.push(match chars.next()? {
                        '\\' => '\\',
                        '"' => '"',
                        'n' => '\n',
                        _ => return None,
                    }),
                    c => value.push(c),
                }
            }
            if labels.insert(key, value).is_some() { return None; }
            while chars.peek().is_some_and(|c| c.is_whitespace()) { chars.next(); }
            match chars.next()? {
                '}' => return Some(labels),
                ',' => continue,
                _ => return None,
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Experiment, MetricRangeQuery, NodeStatus, RpcObservation};
    use crate::storage::{memory::MemoryStore, MetricsStore};
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    struct TestStore {
        memory: MemoryStore,
        errors: Mutex<Vec<CollectionError>>,
    }

    impl Default for TestStore {
        fn default() -> Self {
            Self { memory: MemoryStore::new(), errors: Mutex::new(Vec::new()) }
        }
    }

    impl MetricsStore for TestStore {
        fn upsert_experiment(&self, experiment: &Experiment) -> Result<()> { self.memory.upsert_experiment(experiment) }
        fn upsert_node(&self, node: &Node) -> Result<()> { self.memory.upsert_node(node) }
        fn get_node(&self, id: &str) -> Result<Option<Node>> { self.memory.get_node(id) }
        fn delete_node(&self, id: &str) -> Result<()> { self.memory.delete_node(id) }
        fn update_node_status(&self, id: &str, status: NodeStatus, seen: Option<i64>, success: Option<i64>, failures: u32) -> Result<()> {
            self.memory.update_node_status(id, status, seen, success, failures)
        }
        fn insert_rpc_observation(&self, observation: &RpcObservation) -> Result<()> { self.memory.insert_rpc_observation(observation) }
        fn insert_metric_sample(&self, sample: &MetricSample) -> Result<()> { self.memory.insert_metric_sample(sample) }
        fn insert_collection_error(&self, error: &CollectionError) -> Result<()> {
            self.memory.insert_collection_error(error)?;
            self.errors.lock().unwrap().push(error.clone());
            Ok(())
        }
        fn list_nodes(&self) -> Result<Vec<Node>> { self.memory.list_nodes() }
        fn list_experiments(&self) -> Result<Vec<Experiment>> { self.memory.list_experiments() }
        fn latest_metrics(&self) -> Result<Vec<MetricSample>> { self.memory.latest_metrics() }
        fn query_metric_range(&self, query: &MetricRangeQuery) -> Result<Vec<MetricSample>> { self.memory.query_metric_range(query) }
    }

    fn node(url: Option<String>) -> Node {
        Node {
            id: "geth-1".into(), network: "testnet".into(), chain_id: Some(123),
            client: "geth".into(), rpc_url: "http://localhost:8545".into(), metrics_url: url,
            p2p_addr: None, discovery_addr: None, enode: None, status: NodeStatus::Active,
            last_seen_ms: None, last_successful_probe_ms: None, consecutive_failures: 0,
        }
    }

    fn scraper() -> (PrometheusScraper, Arc<TestStore>) {
        let store = Arc::new(TestStore::default());
        let scraper = PrometheusScraper::new(store.clone(), TelemetryRegistry::new().unwrap(), Duration::from_secs(3), Some("experiment-1".into()));
        (scraper, store)
    }

    async fn server(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!("HTTP/1.1 {}\r\nLocation: /redirected\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", status, body.len(), body);
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 1024];
            loop {
                let count = stream.read(&mut buf).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buf[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") { break; }
            }
            stream.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request).unwrap().lines().next().unwrap().to_string()
        });
        (format!("http://{}", address), task)
    }

    #[tokio::test]
    async fn scrapes_root_and_complete_paths_with_queries() {
        for (suffix, expected) in [("", "/metrics"), ("/", "/metrics"), ("?x=1", "/metrics?x=1"), ("/?x=1", "/metrics?x=1"), ("/metrics", "/metrics"), ("/debug/metrics/prometheus?x=1", "/debug/metrics/prometheus?x=1")] {
            let (base, request) = server("200 OK", "geth_chain_head_block 42\n").await;
            let (scraper, store) = scraper();
            scraper.scrape_node(node(Some(format!("{}{}", base, suffix)))).await.unwrap();
            assert_eq!(request.await.unwrap(), format!("GET {} HTTP/1.1", expected));
            let samples = store.latest_metrics().unwrap();
            assert_eq!(samples.len(), 1);
            assert_eq!(samples[0].name, "geth_chain_head_block");
            assert_eq!(samples[0].value, 42.0);
            assert!(store.errors.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn records_http_failure_without_parsing_body() {
        for status in ["404 Not Found", "500 Internal Server Error", "302 Found"] {
            let (base, request) = server(status, "should_not_be_stored 1\n").await;
            let (scraper, store) = scraper();
            scraper.scrape_node(node(Some(base))).await.unwrap();
            request.await.unwrap();
            assert!(store.latest_metrics().unwrap().is_empty());
            let errors = store.errors.lock().unwrap();
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].node_id, "geth-1");
            assert_eq!(errors[0].experiment_id.as_deref(), Some("experiment-1"));
            assert_eq!(errors[0].source, "prometheus_scraper");
            assert!(errors[0].recoverable);
            assert!(errors[0].message.contains(&status[..3]));
        }
    }

    #[tokio::test]
    async fn missing_url_and_empty_data_produce_no_samples() {
        let (scraper, store) = scraper();
        scraper.scrape_node(node(None)).await.unwrap();
        let (base, request) = server("200 OK", "# HELP absent No data\n\n").await;
        scraper.scrape_node(node(Some(base))).await.unwrap();
        request.await.unwrap();
        assert!(store.latest_metrics().unwrap().is_empty());
        assert!(store.errors.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn records_invalid_url_and_connection_failure() {
        let (scraper, store) = scraper();
        scraper.scrape_node(node(Some("not a url".into()))).await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        scraper.scrape_node(node(Some(format!("http://{}", address)))).await.unwrap();
        assert!(store.latest_metrics().unwrap().is_empty());
        let errors = store.errors.lock().unwrap();
        assert_eq!(errors.len(), 2);
        assert!(errors.iter().all(|error| error.message.contains("Failed to fetch metrics") && error.node_id == "geth-1"));
    }

    #[tokio::test]
    async fn preserves_real_samples_and_series_identity() {
        let body = concat!(
            "# TYPE geth_rpc_requests_total counter\n",
            r#"geth_rpc_requests_total{method="eth_getBlock, by=hash",path="C:\\geth\"node\nnext",brace="}"} 1.25e+2 1700000000000"#, "\n",
            r#"geth_rpc_requests_total{brace="}", path="C:\\geth\"node\nnext", method="eth_getBlock, by=hash"} 125"#, "\n",
            "geth_chain_head_block -3.5\n",
        );
        let (base, request) = server("200 OK", body).await;
        let (scraper, store) = scraper();
        scraper.scrape_node(node(Some(base))).await.unwrap();
        request.await.unwrap();
        let samples = store.latest_metrics().unwrap();
        assert_eq!(samples.len(), 2);
        let sample = samples.iter().find(|s| s.name == "geth_rpc_requests_total").unwrap();
        assert_eq!(sample.value, 125.0);
        assert_eq!(sample.experiment_id.as_deref(), Some("experiment-1"));
        assert_eq!(sample.node_id, "geth-1");
        assert_eq!(sample.network, "testnet");
        assert_eq!(sample.client, "geth");
        assert_eq!(sample.source, MetricSource::PrometheusScrape);
        let labels: serde_json::Value = serde_json::from_str(sample.labels_json.as_ref().unwrap()).unwrap();
        assert_eq!(labels["method"], "eth_getBlock, by=hash");
        assert_eq!(labels["path"], "C:\\geth\"node\nnext");
        assert_eq!(labels["brace"], "}");
        assert_eq!(samples.iter().find(|s| s.name == "geth_chain_head_block").unwrap().value, -3.5);
    }

    #[test]
    fn rejects_malformed_and_non_finite_samples() {
        let (scraper, _) = scraper();
        let node = node(None);
        for line in ["metric", "metric NaN", "metric +Inf", "metric -Inf", "metric 1e999", "metric{label=unquoted} 1", r#"metric{label="bad\tescape"} 1"#, r#"metric{label="unfinished} 1"#, r#"metric{label="a",label="b"} 1"#, "metric{}garbage 1", "metric 1 not-a-timestamp", "metric 1 123 extra"] {
            assert!(scraper.parse_line(line, &node, 123).is_none(), "accepted {line}");
        }
        for line in ["metric 1", "metric +2.5", "metric .5", "metric 1e-3", "metric{} 0 123", r#"namespace:metric{label="μ,=}",} 1"#] {
            assert!(scraper.parse_line(line, &node, 123).is_some(), "rejected {line}");
        }
    }
}
