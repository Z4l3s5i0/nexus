use axum::{routing::{get, post}, Router, extract::DefaultBodyLimit};
use tower_http::trace::TraceLayer;
use crate::api::{ApiState, handlers};

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/health", get(handlers::health_handler))
        .route("/metrics", get(handlers::metrics_handler))
        .route("/api/nodes", get(handlers::nodes_handler))
        .route("/api/nodes/register", post(handlers::register_node_handler))
        .route("/api/bootstrap/nodes", get(handlers::bootstrap_nodes_handler))
        .route("/api/bootstrap/status", get(handlers::bootstrap_status_handler))
        .route("/api/experiments", get(handlers::experiments_handler))
        .route("/api/metrics/latest", get(handlers::latest_metrics_handler))
        .route("/api/metrics/range", get(handlers::range_metrics_handler))
        .route("/api/metrics/ingest", post(handlers::ingest_metrics_handler))
        .layer(DefaultBodyLimit::max(1024 * 512))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
