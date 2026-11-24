use crate::config;
use crate::storage::{memory::MemoryStore, redb_store::RedbStore, SharedStore};
use crate::telemetry::registry::TelemetryRegistry;
use crate::api::{ApiState, routes};
use crate::collector::scheduler::CollectorScheduler;
use crate::model::{Experiment, ExperimentConfig, Node};
use crate::time::now_ms;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{info, error};

pub async fn run(config_path: PathBuf) -> anyhow::Result<()> {
    if let Err(e) = tracing_subscriber::fmt::try_init() {
        eprintln!("Failed to initialize tracing: {}", e);
    }

    let mut config = config::load_config(&config_path)?;
    if let Ok(experiment_id) = std::env::var("EXPERIMENT_RUN_ID") {
        if !experiment_id.trim().is_empty() {
            if let Some(experiment) = config.experiment.as_mut() {
                experiment.id = experiment_id;
            } else {
                config.experiment = Some(ExperimentConfig {
                    id: experiment_id.clone(),
                    name: format!("Experiment run {experiment_id}"),
                    description: None,
                    tags: vec!["experiment-suite".to_string()],
                });
            }
        }
    }
    config::validate_config(&config)?;

    info!("Starting metrics_server with config: {:?}", config_path);
    info!("Storage configuration: {:?}", config.storage);
    info!("Collection configuration: interval={}s, timeout={}ms, max_concurrent={}", 
        config.collection.interval_seconds, 
        config.collection.timeout_ms, 
        config.collection.max_concurrent_nodes
    );

    let store: SharedStore = match config.storage.kind.as_str() {
        "memory" => {
            info!("Using memory storage");
            Arc::new(MemoryStore::new())
        },
        "redb" => {
            info!("Using redb storage at {}", config.storage.path);
            Arc::new(RedbStore::new(&config.storage.path)?)
        },
        _ => unreachable!(),
    };

    if let Some(exp_config) = &config.experiment {
        info!("Experiment configured: {} ({})", exp_config.name, exp_config.id);
        let experiment = Experiment {
            id: exp_config.id.clone(),
            name: exp_config.name.clone(),
            description: exp_config.description.clone(),
            tags: exp_config.tags.clone(),
            started_at_ms: now_ms(),
            ended_at_ms: None,
        };
        store.upsert_experiment(&experiment)?;
    }

    for node_config in &config.nodes {
        info!("Adding static node: {} (network={}, chain_id={:?}, rpc={})", node_config.id, node_config.network, node_config.chain_id, node_config.rpc_url);
        let node = Node {
            id: node_config.id.clone(),
            network: node_config.network.clone(),
            chain_id: node_config.chain_id,
            client: node_config.client.clone(),
            rpc_url: node_config.rpc_url.clone(),
            metrics_url: node_config.metrics_url.clone(),
            p2p_addr: node_config.p2p_addr.clone(),
            discovery_addr: node_config.discovery_addr.clone(),
            enode: node_config.enode.clone(),
            status: crate::model::NodeStatus::Active,
            last_seen_ms: Some(now_ms()),
            last_successful_probe_ms: None,
            consecutive_failures: 0,
        };
        store.upsert_node(&node)?;
    }

    let telemetry = TelemetryRegistry::new()?;

    let state = ApiState {
        config: config.clone(),
        store: store.clone(),
        telemetry: telemetry.clone(),
    };

    let app = routes::router(state);
    let addr: std::net::SocketAddr = config.server.bind_addr.parse()?;
    
    info!("HTTP API listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let prober = crate::bootstrap::prober::BootstrapProber::new(config.clone(), store.clone(), telemetry.clone());
    let prober_rx = shutdown_rx.clone();
    let prober_handle = tokio::spawn(async move {
        if let Err(e) = prober.run(prober_rx).await {
            error!("Bootstrap prober failed: {}", e);
        }
    });

    let scheduler = CollectorScheduler::new(config, store, telemetry);
    let scheduler_handle = tokio::spawn(async move {
        if let Err(e) = scheduler.run(shutdown_rx).await {
            error!("Collector scheduler failed: {}", e);
        }
    });

    let server_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app.into_make_service()).await {
            error!("HTTP server failed: {}", e);
        }
    });

    tokio::select! {
        res = tokio::signal::ctrl_c() => {
            if let Err(e) = res {
                error!("Failed to listen for ctrl_c: {}", e);
            } else {
                info!("Received Ctrl+C, shutting down");
            }
            let _ = shutdown_tx.send(true);
        }
        _ = server_handle => {
            error!("HTTP server task exited unexpectedly");
        }
    }

    // Wait for tasks to finish
    let _ = scheduler_handle.await;
    let _ = prober_handle.await;

    Ok(())
}
