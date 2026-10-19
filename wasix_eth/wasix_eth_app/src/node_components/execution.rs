use std::sync::Arc;
use wasix_eth_core::account_manager::AccountManager;
use wasix_eth_core::engine::api::RPCEngine;
use wasix_eth_core::engine::forkchoice_validator::ForkchoiceValidator;
use wasix_eth_core::mempool::listener::MempoolListener;
use wasix_eth_core::mempool::mempool::Mempool;
use wasix_eth_core::engine::canonicality_tracker::CanonicalState;
use wasix_eth_core::engine::sidechain_tracker::BlockTree;
use wasix_eth_core::engine::reorg_manager::ReorgHandler;
use wasix_eth_core::{ChainManagerImpl, Engine, EthConsensus};
use wasix_eth_core::sync::registry::SyncRegistry;
use wasix_eth_execution::orchestrator::ExecutionOrchestrator;
use wasix_eth_storage::{read::DatabaseReadProvider, write::DatabaseWriteProvider};
use wasix_eth_types::{ChainManager, U256};

#[derive(Clone)]
pub struct ExecutionPayload {
    pub rpc_engine: Arc<RPCEngine>,
    pub engine: Arc<Engine>,
    pub mempool: Arc<Mempool>,
    pub chain_manager: Arc<dyn ChainManager>,
    pub sync_registry: Arc<SyncRegistry>,
}

impl ExecutionPayload {
    pub async fn new(
        read_provider: Arc<DatabaseReadProvider>,
        write_provider: Arc<DatabaseWriteProvider>,
    ) -> Self {
        let mempool = Arc::new(Mempool::new(U256::ZERO));
        let (engine_event_tx, _) = tokio::sync::broadcast::channel(2000);

        let execution_provider = Arc::new(ExecutionOrchestrator::new((*read_provider).clone(), (*write_provider).clone()));
        let account_manager = Arc::new(AccountManager::new_with_dev_keys());

        let canonical = Arc::new(CanonicalState::new((*read_provider).clone(), (*write_provider).clone()).await);
        let block_tree = Arc::new(BlockTree::new());
        let reorg_handler = Arc::new(ReorgHandler::new((*read_provider).clone(), (*write_provider).clone(), canonical.clone()));

        let chain_manager = Arc::new(ChainManagerImpl::new(
            (*read_provider).clone(),
            (*write_provider).clone(),
            canonical.clone(),
            block_tree.clone(),
            reorg_handler.clone(),
        ));
        let mempool_listener = Arc::new(MempoolListener::new(
            mempool.clone(),
            (*read_provider).clone(),
        ));
        let consensus = Arc::new(EthConsensus::new(read_provider.clone()));
        let sync_registry = Arc::new(SyncRegistry::new());
        let forkchoice_validator = ForkchoiceValidator::new(
            (*read_provider).clone(),
            chain_manager.clone(),
        );
        let rpc_engine = Arc::new(RPCEngine::new(
            (*read_provider).clone(),
            (*write_provider).clone(),
            account_manager.clone(),
            chain_manager.clone(),
            mempool.clone(),
            engine_event_tx.clone(),
            consensus.clone(),
            execution_provider.clone(),
            canonical.clone()
        ));
        let engine = Arc::new(Engine::new(
            (*read_provider).clone(),
            (*write_provider).clone(),
            execution_provider,
            account_manager,
            mempool.clone(),
            engine_event_tx,
            consensus,
            rpc_engine.clone(),
            chain_manager.clone(),
            canonical.clone(),
            block_tree.clone(),
            reorg_handler.clone(),
            forkchoice_validator.clone(),
            mempool_listener,
            sync_registry.clone(),
        ));

        Self {
            rpc_engine,
            engine: engine.clone(),
            mempool,
            chain_manager,
            sync_registry,
        }
    }
}
