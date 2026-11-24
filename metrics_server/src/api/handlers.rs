use std::time::Duration;
use axum::{
    extract::{Query, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::IntoResponse,
    Json,
};
use tracing::info;
use crate::api::ApiState;
use crate::model::{MetricIngestionRequest, MetricRangeQuery, MetricRangeResponse, MetricPoint, MetricSample, MetricSource, RegisterNodeRequest, Node, NodeStatus, BootstrapNodesQuery, BootstrapNode, BootstrapNodesResponse};
use serde_json::json;
use crate::time::now_ms;

pub async fn health_handler() -> impl IntoResponse {
    info!("Health check requested");
    Json(json!({"status": "ok"}))
}

pub async fn register_node_handler(
    State(state): State<ApiState>,
    payload: Result<Json<RegisterNodeRequest>, JsonRejection>,
) -> impl IntoResponse {
    let Json(payload) = match payload {
        Ok(p) => p,
        Err(rejection) => {
            info!("Registration JSON rejection: {} - {}", rejection.status(), rejection.body_text());
            return (rejection.status(), rejection.body_text()).into_response();
        }
    };

    info!("Received registration request: {:?}", payload);

    if payload.id.is_empty() || payload.network.is_empty() || payload.rpc_url.is_empty() {
        info!("Registration request missing required fields: {:?}", payload);
        state.telemetry.bootstrap_registration_requests.with_label_values(&["bad_request"]).inc();
        return (StatusCode::BAD_REQUEST, "id, network, and rpc_url are required").into_response();
    }

    if let Some(bootstrap) = &state.config.bootstrap {
        // Enforce network if configured
        if let Some(expected_network) = &bootstrap.network {
            if &payload.network != expected_network {
                info!(node_id = payload.id, got = payload.network, expected = expected_network, "Network mismatch during registration");
                state.telemetry.bootstrap_registration_requests.with_label_values(&["bad_request"]).inc();
                return (StatusCode::BAD_REQUEST, format!("Invalid network. Expected: {}", expected_network)).into_response();
            }
        }

        // IP restrictions
        if !bootstrap.allow_loopback_ips || !bootstrap.allow_private_ips {
            let check_url = |url: &str| -> bool {
                if let Ok(u) = reqwest::Url::parse(url) {
                    if let Some(host) = u.host_str() {
                        if let Ok(addr) = host.parse::<std::net::IpAddr>() {
                            if !bootstrap.allow_loopback_ips && addr.is_loopback() {
                                return false;
                            }
                            // Simplified private IP check
                            if !bootstrap.allow_private_ips {
                                match addr {
                                    std::net::IpAddr::V4(v4) => if v4.is_private() { return false; },
                                    _ => {} // IPv6 private check omitted for brevity
                                }
                            }
                        } else if host == "localhost" && !bootstrap.allow_loopback_ips {
                            return false;
                        }
                    }
                }
                true
            };

            if !check_url(&payload.rpc_url) {
                info!(node_id = payload.id, url = payload.rpc_url, "Loopback or private IP rejected for rpc_url");
                return (StatusCode::BAD_REQUEST, "Loopback or private IPs not allowed for rpc_url").into_response();
            }
            if let Some(m_url) = &payload.metrics_url {
                if !check_url(m_url) {
                    tracing::warn!(node_id = payload.id, url = m_url, "Loopback or private IP rejected for metrics_url");
                    return (StatusCode::BAD_REQUEST, "Loopback or private IPs not allowed for metrics_url").into_response();
                }
            }
            
            // Check P2P and discovery addr
            let check_socket = |addr_str: &str| -> bool {
                if let Ok(addr) = addr_str.parse::<std::net::SocketAddr>() {
                    let ip = addr.ip();
                    if !bootstrap.allow_loopback_ips && ip.is_loopback() {
                        return false;
                    }
                    if !bootstrap.allow_private_ips {
                        match ip {
                            std::net::IpAddr::V4(v4) => if v4.is_private() { return false; },
                            _ => {}
                        }
                    }
                }
                true
            };

            if let Some(p2p) = &payload.p2p_addr {
                if !check_socket(p2p) {
                    tracing::warn!(node_id = payload.id, addr = p2p, "Loopback or private IP rejected for p2p_addr");
                    return (StatusCode::BAD_REQUEST, "Loopback or private IPs not allowed for p2p_addr").into_response();
                }
            }
            if let Some(disc) = &payload.discovery_addr {
                if !check_socket(disc) {
                    tracing::warn!(node_id = payload.id, addr = disc, "Loopback or private IP rejected for discovery_addr");
                    return (StatusCode::BAD_REQUEST, "Loopback or private IPs not allowed for discovery_addr").into_response();
                }
            }
        }
    }

    if !payload.rpc_url.starts_with("http://") && !payload.rpc_url.starts_with("https://") {
        info!(node_id = payload.id, rpc_url = payload.rpc_url, "rpc_url missing protocol");
        state.telemetry.bootstrap_registration_requests.with_label_values(&["bad_request"]).inc();
        return (StatusCode::BAD_REQUEST, "rpc_url must start with http:// or https://").into_response();
    }

    let node = Node {
        id: payload.id.clone(),
        network: payload.network,
        chain_id: payload.chain_id,
        client: payload.client,
        rpc_url: payload.rpc_url,
        metrics_url: payload.metrics_url,
        p2p_addr: payload.p2p_addr,
        discovery_addr: payload.discovery_addr,
        enode: payload.enode,
        status: NodeStatus::Pending,
        last_seen_ms: Some(now_ms()),
        last_successful_probe_ms: None,
        consecutive_failures: 0,
    };

    info!(node_id = node.id, network = node.network, "Registering node");

    match state.store.upsert_node(&node) {
        Ok(_) => {
            state.telemetry.bootstrap_registration_requests.with_label_values(&["accepted"]).inc();
            
            // Trigger immediate probe
            let store = state.store.clone();
            let telemetry = state.telemetry.clone();
            let config = state.config.clone();
            
            tokio::spawn(async move {
                if let Some(bootstrap_config) = &config.bootstrap {
                    let timeout = Duration::from_millis(config.collection.timeout_ms);
                    let result = crate::bootstrap::prober::BootstrapProber::probe_node_internal(&node, timeout).await;
                    let _ = crate::bootstrap::prober::BootstrapProber::handle_probe_result_internal(
                        store,
                        telemetry,
                        node,
                        result,
                        bootstrap_config
                    );
                }
            });

            Json(json!({"status": "accepted", "node_id": payload.id})).into_response()
        }
        Err(e) => {
            state.telemetry.bootstrap_registration_requests.with_label_values(&["error"]).inc();
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to register node: {}", e)).into_response()
        }
    }
}

pub async fn bootstrap_nodes_handler(
    State(state): State<ApiState>,
    Query(query): Query<BootstrapNodesQuery>,
) -> impl IntoResponse {
    info!(
        network = ?query.network,
        chain_id = ?query.chain_id,
        exclude_id = ?query.exclude_id,
        "Fetching bootstrap nodes"
    );
    match state.store.list_nodes() {
        Ok(nodes) => {
            let mut filtered: Vec<Node> = nodes
                .into_iter()
                .filter(|n| {
                    if n.status != NodeStatus::Active && n.status != NodeStatus::Pending {
                        return false;
                    }
                    if let Some(network) = &query.network {
                        if &n.network != network {
                            return false;
                        }
                    }
                    if let Some(chain_id) = query.chain_id {
                        if n.chain_id != Some(chain_id) {
                            return false;
                        }
                    }
                    if let Some(exclude_id) = &query.exclude_id {
                        if &n.id == exclude_id {
                            return false;
                        }
                    }
                    // At least one P2P-related field must exist
                    n.enode.is_some() || n.discovery_addr.is_some() || n.p2p_addr.is_some()
                })
                .collect();

            // Sort by:
            // 1. status (Active before Pending)
            // 2. last_successful_probe_ms DESC (most recently verified nodes first)
            // 3. has enode (more complete info)
            // 4. fewer consecutive failures
            filtered.sort_by(|a, b| {
                let a_active = a.status == NodeStatus::Active;
                let b_active = b.status == NodeStatus::Active;
                b_active.cmp(&a_active)
                    .then_with(|| b.last_successful_probe_ms.cmp(&a.last_successful_probe_ms))
                    .then_with(|| b.enode.is_some().cmp(&a.enode.is_some()))
                    .then_with(|| a.consecutive_failures.cmp(&b.consecutive_failures))
            });

            let max_nodes = state.config.bootstrap.as_ref()
                .map(|b| b.max_bootstrap_nodes)
                .unwrap_or(16);

            let limit = query.limit.unwrap_or(max_nodes).min(max_nodes);

            let result: Vec<BootstrapNode> = filtered
                .into_iter()
                .take(limit)
                .map(|n| BootstrapNode {
                    id: n.id,
                    network: n.network,
                    chain_id: n.chain_id,
                    client: n.client,
                    p2p_addr: n.p2p_addr,
                    discovery_addr: n.discovery_addr,
                    enode: n.enode,
                    last_seen_ms: n.last_seen_ms,
                })
                .collect();

            Json(BootstrapNodesResponse { nodes: result }).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to list nodes: {}", e)).into_response(),
    }
}

pub async fn metrics_handler(State(state): State<ApiState>) -> impl IntoResponse {
    info!("Internal metrics requested");
    match state.telemetry.gather_text() {
        Ok(text) => (
            StatusCode::OK,
            [("Content-Type", "text/plain; version=0.0.4")],
            text,
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to gather metrics: {}", e),
        ).into_response(),
    }
}

pub async fn nodes_handler(State(state): State<ApiState>) -> impl IntoResponse {
    info!("Listing all nodes requested");
    match state.store.list_nodes() {
        Ok(nodes) => Json(nodes).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list nodes: {}", e),
        ).into_response(),
    }
}

pub async fn bootstrap_status_handler(State(state): State<ApiState>) -> impl IntoResponse {
    info!("Bootstrap status requested");
    match state.store.list_nodes() {
        Ok(nodes) => {
            let mut counts = json!({
                "pending": 0,
                "active": 0,
                "stale": 0,
                "unhealthy": 0,
                "disabled": 0,
            });

            for node in nodes {
                let key = match node.status {
                    NodeStatus::Pending => "pending",
                    NodeStatus::Active => "active",
                    NodeStatus::Stale => "stale",
                    NodeStatus::Unhealthy => "unhealthy",
                    NodeStatus::Disabled => "disabled",
                };
                let current = counts[key].as_i64().unwrap_or(0);
                counts[key] = json!(current + 1);
            }

            let bootstrap = state.config.bootstrap.as_ref();
            Json(json!({
                "enabled": bootstrap.map(|b| b.enabled).unwrap_or(false),
                "network": bootstrap.and_then(|b| b.network.clone()),
                "chain_id": bootstrap.and_then(|b| b.chain_id),
                "nodes": counts,
            })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to list nodes: {}", e)).into_response(),
    }
}

pub async fn experiments_handler(State(state): State<ApiState>) -> impl IntoResponse {
    info!("Listing experiments requested");
    match state.store.list_experiments() {
        Ok(exps) => Json(exps).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to list experiments: {}", e)).into_response(),
    }
}

pub async fn latest_metrics_handler(State(state): State<ApiState>) -> impl IntoResponse {
    match state.store.latest_metrics() {
        Ok(metrics) => Json(metrics).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to get latest metrics: {}", e)).into_response(),
    }
}

pub async fn range_metrics_handler(
    State(state): State<ApiState>,
    Query(query): Query<MetricRangeQuery>,
) -> impl IntoResponse {
    if query.from_ms > query.to_ms {
        return (StatusCode::BAD_REQUEST, "from_ms must be <= to_ms").into_response();
    }

    match state.store.query_metric_range(&query) {
        Ok(samples) => {
            let points = samples
                .into_iter()
                .map(|s| MetricPoint {
                    timestamp_ms: s.timestamp_ms,
                    value: s.value,
                })
                .collect();
            
            Json(MetricRangeResponse {
                metric: query.metric,
                node_id: query.node_id,
                experiment_id: query.experiment_id,
                points,
            }).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to query metric range: {}", e)).into_response(),
    }
}

fn is_ingestion_authorized(headers: &HeaderMap, expected_token: &str) -> bool {
    let supplied = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    supplied == Some(expected_token)
}

fn sample_belongs_to_experiment(sample: &MetricSample, experiment_id: &str) -> bool {
    sample.experiment_id.as_deref() == Some(experiment_id)
}

fn validate_ingested_sample(sample: &MetricSample) -> Result<(), &'static str> {
    if !matches!(
        &sample.source,
        MetricSource::System | MetricSource::Log | MetricSource::Derived
    ) {
        return Err("only derived, system, and log metric samples may be ingested");
    }
    if sample.experiment_id.as_deref().is_none_or(str::is_empty) {
        return Err("experiment_id is required");
    }
    if sample.timestamp_ms <= 0 {
        return Err("timestamp_ms must be positive");
    }
    if sample.node_id.is_empty() || sample.node_id.len() > 256 {
        return Err("node_id must contain 1-256 characters");
    }
    if sample.network.is_empty() || sample.network.len() > 256 {
        return Err("network must contain 1-256 characters");
    }
    if sample.client.is_empty() || sample.client.len() > 128 {
        return Err("client must contain 1-128 characters");
    }
    if sample.name.is_empty()
        || sample.name.len() > 128
        || !sample.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
    {
        return Err("metric name must contain only ASCII letters, digits, underscores, or colons");
    }
    if !sample.value.is_finite() {
        return Err("metric value must be finite");
    }
    if let Some(labels) = &sample.labels_json {
        if labels.len() > 16 * 1024 {
            return Err("metric labels exceed the 16 KiB limit");
        }
        if !serde_json::from_str::<serde_json::Value>(labels)
            .is_ok_and(|value| value.is_object())
        {
            return Err("metric labels must be a JSON object");
        }
    }
    Ok(())
}

pub async fn ingest_metrics_handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    payload: Result<Json<MetricIngestionRequest>, JsonRejection>,
) -> impl IntoResponse {
    let expected_token = match std::env::var("METRICS_INGEST_TOKEN") {
        Ok(token) if !token.is_empty() => token,
        _ => return (StatusCode::SERVICE_UNAVAILABLE, "Metric ingestion is not configured").into_response(),
    };
    if !is_ingestion_authorized(&headers, &expected_token) {
        return (StatusCode::UNAUTHORIZED, "Invalid metric ingestion token").into_response();
    }

    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => return (rejection.status(), rejection.body_text()).into_response(),
    };
    if request.samples.is_empty() || request.samples.len() > 512 {
        return (StatusCode::BAD_REQUEST, "samples must contain 1-512 entries").into_response();
    }
    let active_experiment_id = match state.config.experiment.as_ref() {
        Some(experiment) => experiment.id.as_str(),
        None => return (StatusCode::SERVICE_UNAVAILABLE, "No active experiment is configured").into_response(),
    };
    for sample in &request.samples {
        if let Err(message) = validate_ingested_sample(sample) {
            return (StatusCode::BAD_REQUEST, message).into_response();
        }
        if !sample_belongs_to_experiment(sample, active_experiment_id) {
            return (StatusCode::BAD_REQUEST, "sample experiment_id does not match the active run").into_response();
        }
    }

    let accepted = request.samples.len();
    for sample in &request.samples {
        if let Err(error) = state.store.insert_metric_sample(sample) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to store metric sample: {error}"),
            ).into_response();
        }
    }
    Json(serde_json::json!({"accepted": accepted})).into_response()
}

#[cfg(test)]
mod ingestion_tests {
    use super::*;

    fn sample(source: MetricSource) -> MetricSample {
        MetricSample {
            timestamp_ms: 1,
            experiment_id: Some("run-1".into()),
            node_id: "node-1".into(),
            network: "12345".into(),
            client: "wasix_eth".into(),
            source,
            name: "system_cpu_percent".into(),
            value: 12.5,
            labels_json: Some(r#"{"scope":"container"}"#.into()),
        }
    }

    #[test]
    fn accepts_derived_system_and_log_samples_with_provenance() {
        assert!(validate_ingested_sample(&sample(MetricSource::System)).is_ok());
        assert!(validate_ingested_sample(&sample(MetricSource::Log)).is_ok());
        assert!(validate_ingested_sample(&sample(MetricSource::Derived)).is_ok());
    }

    #[test]
    fn rejects_other_sources_and_non_finite_values() {
        assert!(validate_ingested_sample(&sample(MetricSource::JsonRpc)).is_err());
        let mut invalid = sample(MetricSource::System);
        invalid.value = f64::NAN;
        assert!(validate_ingested_sample(&invalid).is_err());
    }

    #[test]
    fn requires_labels_to_be_a_json_object() {
        let mut invalid = sample(MetricSource::Log);
        invalid.labels_json = Some("[]".into());
        assert!(validate_ingested_sample(&invalid).is_err());
    }

    #[test]
    fn ingestion_requires_the_bearer_token() {
        let mut headers = HeaderMap::new();
        assert!(!is_ingestion_authorized(&headers, "secret"));
        headers.insert(AUTHORIZATION, "Bearer secret".parse().unwrap());
        assert!(is_ingestion_authorized(&headers, "secret"));
        assert!(!is_ingestion_authorized(&headers, "different"));
    }

    #[test]
    fn ingestion_rejects_samples_from_another_run() {
        let current = sample(MetricSource::Log);
        let mut stale = sample(MetricSource::Log);
        stale.experiment_id = Some("old-run".into());

        assert!(sample_belongs_to_experiment(&current, "run-1"));
        assert!(!sample_belongs_to_experiment(&stale, "run-1"));
    }
}
