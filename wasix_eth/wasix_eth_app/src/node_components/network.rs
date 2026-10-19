use wasix_eth_p2p::{PeerManager, SyncService, P2pServer};
use wasix_eth_p2p::discovery::v4_service::DiscoveryV4Service;
use wasix_eth_utils::identity::Identity;
use wasix_eth_storage::{read::DatabaseReadProvider, write::DatabaseWriteProvider, read_traits::BlockProvider};
use wasix_eth_types::sync::NoopSync;
use wasix_eth_core::mempool::mempool::Mempool;
use wasix_eth_types::{ChainConfig, ChainManager};
use std::sync::Arc;
use std::error::Error;
use std::path::PathBuf;
use tokio::task::JoinHandle;
use wasix_eth_utils::info;

pub struct NetworkConfig {
    pub data_dir: PathBuf,
    pub storage_name: String,
    pub discovery_port: u16,
    pub p2p_port: u16,
    pub bootnodes: Vec<String>,
    pub ext_ip: Option<std::net::IpAddr>,
}

#[derive(Clone)]
pub struct NetworkPayload {
    pub peer_manager: Arc<PeerManager>,
    pub sync_service: Arc<SyncService>,
    pub discovery_v4: Arc<DiscoveryV4Service>,
    pub mempool: Arc<Mempool>,
}

impl NetworkPayload {
    pub async fn new(
        config: NetworkConfig,
        read_provider: Arc<DatabaseReadProvider>,
        write_provider: Arc<DatabaseWriteProvider>,
        mempool: Arc<Mempool>,
        chain_manager: Arc<dyn ChainManager>,
        chain_id: u64,
        chain_config: ChainConfig,
    ) -> Result<Self, Box<dyn Error>> {
        let identity = Identity::new(Some(&config.data_dir), Some(&config.storage_name))
            .map_err(|e| format!("Failed to create identity: {}", e))?;

        let genesis_hash = read_provider.block_hash(0).await?.unwrap_or_default();

        let peer_manager = Arc::new(PeerManager::new(
            (*read_provider).clone(),
            (*write_provider).clone(),
            identity.clone(),
            config.discovery_port,
            config.p2p_port,
            config.ext_ip,
            config.bootnodes.clone(),
            chain_id,
            genesis_hash,
            chain_config,
            chain_manager,
        ));

        let bind_ip_discovery = std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0));
        let discovery_v4 = Arc::new(DiscoveryV4Service::new(
            identity.clone(),
            &format!("{}:{}", bind_ip_discovery, config.discovery_port),
            config.discovery_port,
            config.p2p_port,
            config.bootnodes.clone(),
            config.ext_ip,
        ).await?);

        let sync_service = Arc::new(SyncService::new(
            Arc::new(NoopSync),
            (*read_provider).clone(),
            (*write_provider).clone(),
            mempool.clone(),
            peer_manager.clone(),
        ));

        Ok(Self {
            peer_manager,
            sync_service,
            discovery_v4,
            mempool,
        })
    }

    pub async fn start(&self) -> Vec<JoinHandle<()>> {
        let mut tasks = Vec::new();

        let p2p_registry = self.peer_manager.registry.clone();
        let p2p_port = p2p_registry.p2p_port();
        let bind_ip = std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0));
    
        info!("[P2P Server] Starting server task on {}:{}", bind_ip, p2p_port);

        tasks.push(tokio::spawn(async move {
            info!("[P2P Server] Server task running");
            let addr = format!("{}:{}", bind_ip, p2p_port);
            if let Ok(server) = P2pServer::new(&addr, p2p_registry).await {
                server.run().await;
            } else {
                wasix_eth_utils::error!("[P2P Server] Failed to initialize server on {}", addr);
            }
        }));

        self.peer_manager.start(Some(self.discovery_v4.clone())).await;
        self.sync_service.start().await;

        tasks
    }
}
