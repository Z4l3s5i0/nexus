use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use wasix_eth_core::{Engine};
use wasix_eth_core::engine::api::RPCEngine;
use wasix_eth_p2p::{PeerManager, SyncService};
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_core::mempool::mempool::Mempool;
use wasix_eth_storage::read_traits::ChainProvider;
use wasix_eth_types::ChainManager;
use wasix_eth_types::sync::SyncProvider;
use wasix_eth_types::genesis::GenesisConfiguration;
use wasix_eth_utils::info;
use crate::node_components::storage::{StoragePayload, StorageConfig};
use crate::node_components::network::{NetworkPayload, NetworkConfig};
use crate::node_components::execution::ExecutionPayload;
use crate::node_components::sync::SyncPayload;
use crate::import::import_blocks;


#[derive(Clone)]
pub struct Node {
    pub rpc_engine: Arc<RPCEngine>,
    pub engine: Arc<Engine>,
    pub peer_manager: Option<Arc<PeerManager>>,
    pub sync_service: Option<Arc<SyncService>>,
    pub sync_provider: Option<Arc<dyn SyncProvider>>,
    pub read_provider: Arc<DatabaseReadProvider>,
    pub write_provider: Arc<DatabaseWriteProvider>,
    pub mempool: Arc<Mempool>,
    pub chain_manager: Arc<dyn ChainManager>,
    pub storage_payload: StoragePayload,
    pub network_payload: Option<NetworkPayload>,
    pub execution_payload: ExecutionPayload,
    pub sync_payload: Option<SyncPayload>,
}

impl Node {
    pub async fn new_storage_only(
        args: &crate::cli::Args,
        genesis_config: GenesisConfiguration,
    ) -> Result<(), Box<dyn Error>> {
        let data_dir = args.common.data_dir.clone().unwrap_or_else(|| PathBuf::from("data"));
        let storage_name = args.common.peer_name.clone().unwrap_or_else(|| "default".to_string());

        let storage_config = StorageConfig {
            data_dir,
            storage_name,
        };
        
        // This initializes the database with genesis
        let _payload = StoragePayload::new(storage_config, genesis_config).await?;
        
        info!("[Node] Database initialized with genesis configuration");
        Ok(())
    }



    pub async fn new_no_init(
        args: &crate::cli::Args,
    ) -> Result<Self, Box<dyn Error>> {
        let data_dir = args.common.data_dir.clone().unwrap_or_else(|| PathBuf::from("data"));
        let storage_name = args.common.peer_name.clone().unwrap_or_else(|| "default".to_string());

        let storage_config = StorageConfig {
            data_dir,
            storage_name,
        };

        let storage_payload = StoragePayload::new_no_init(storage_config)?;
        
        // Verify initialization
        storage_payload.read_provider.chain_config().await?
            .ok_or("Chain config not found in database. Has the node been initialized?")?;

        // 2. Execution
        let execution_payload = ExecutionPayload::new(
            storage_payload.read_provider.clone(),
            storage_payload.write_provider.clone(),
        ).await;

        Ok(Self {
            rpc_engine: execution_payload.rpc_engine.clone(),
            engine: execution_payload.engine.clone(),
            peer_manager: None,
            sync_service: None,
            sync_provider: None,
            read_provider: storage_payload.read_provider.clone(),
            write_provider: storage_payload.write_provider.clone(),
            mempool: execution_payload.mempool.clone(),
            chain_manager: execution_payload.chain_manager.clone(),
            storage_payload,
            network_payload: None,
            execution_payload,
            sync_payload: None,
        })
    }
    
    pub async fn new(
        args: &crate::cli::Args,
        genesis_config: Option<GenesisConfiguration>,
    ) -> Result<Self, Box<dyn Error>> {
        let data_dir = args.common.data_dir.clone().unwrap_or_else(|| PathBuf::from("data"));
        let storage_name = args.common.peer_name.clone().unwrap_or_else(|| "default".to_string());

        // 1. Storage
        let storage_config = StorageConfig {
            data_dir: data_dir.clone(),
            storage_name: storage_name.clone(),
        };
        
        let storage_payload = if let Some(config) = genesis_config {
            StoragePayload::new(storage_config, config).await?
        } else {
            let payload = StoragePayload::new_no_init(storage_config)?;
            // Verify initialization
            payload.read_provider.chain_config().await?
                .ok_or("Chain config not found in database. Node must be initialized with --genesis-path at least once.")?;
            payload
        };

        let chain_config = storage_payload.read_provider.chain_config().await?
            .ok_or("Chain config not found in database")?;
        let chain_id = chain_config.chain_id;

        // 2. Execution
        let execution_payload = ExecutionPayload::new(
            storage_payload.read_provider.clone(),
            storage_payload.write_provider.clone(),
        ).await;

        // 3. Network
        let network_config = NetworkConfig {
            data_dir,
            storage_name,
            discovery_port: args.common.discovery_port,
            p2p_port: args.common.p2p_port,
            bootnodes: args.common.bootnodes.clone(),
            ext_ip: args.common.ext_ip,
        };
        let network_payload = NetworkPayload::new(
            network_config,
            storage_payload.read_provider.clone(),
            storage_payload.write_provider.clone(),
            execution_payload.mempool.clone(),
            execution_payload.chain_manager.clone(),
            chain_id,
            chain_config,
        ).await?;

        // 4. Sync
        let sync_payload = SyncPayload::new(
            storage_payload.read_provider.clone(),
            network_payload.peer_manager.clone(),
            execution_payload.engine.clone(),
            execution_payload.chain_manager.clone(),
            execution_payload.sync_registry.clone(),
            network_payload.sync_service.clone(),
        ).await;

        Ok(Self {
            rpc_engine: execution_payload.rpc_engine.clone(),
            engine: execution_payload.engine.clone(),
            peer_manager: Some(network_payload.peer_manager.clone()),
            sync_service: Some(network_payload.sync_service.clone()),
            sync_provider: Some(sync_payload.sync_controller.clone()),
            read_provider: storage_payload.read_provider.clone(),
            write_provider: storage_payload.write_provider.clone(),
            mempool: execution_payload.mempool.clone(),
            chain_manager: execution_payload.chain_manager.clone(),
            storage_payload,
            network_payload: Some(network_payload),
            execution_payload,
            sync_payload: Some(sync_payload),
        })
    }

    pub async fn start(&mut self, _args: &crate::cli::Args) {
        info!("[Node] Starting services...");

        if let Some(network) = &self.network_payload {
            network.start().await;
        }
        
        if let Some(sync) = &self.sync_payload {
            sync.start(
                self.engine.clone(),
                self.chain_manager.clone(),
                self.sync_service.as_ref().unwrap().clone(),
            );
        }
    }

    pub async fn import_blocks(&self, chain_rlp: Option<PathBuf>, blocks_dir: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
        import_blocks(self.execution_payload.clone(), chain_rlp, blocks_dir).await
    }
}

