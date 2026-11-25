use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::broadcast;
use wasix_eth_core::account_manager::AccountManager;
use wasix_eth_core::chain_manager::ChainManagerImpl;
use wasix_eth_core::consensus::{Consensus, EthConsensus};
use wasix_eth_core::engine::api::RPCEngine;
use wasix_eth_core::engine::canonicality_tracker::CanonicalState;
use wasix_eth_core::engine::engine::EngineEvent;
use wasix_eth_core::engine::reorg_manager::ReorgHandler;
use wasix_eth_core::engine::sidechain_tracker::BlockTree;
use wasix_eth_core::mempool::mempool::Mempool;
use wasix_eth_core::mempool::mempool_provider::MempoolProvider;
use wasix_eth_execution::execution_provider::{EthExecutionProvider, ExecutionProvider};
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::AccountProvider;
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_storage::write_traits::{AccountWriter, BlockWriter, BytecodeWriter, MetadataWriter};
use wasix_eth_storage::EthDatabase;
use wasix_eth_types::{
    Address, Block, BlockId, BlockNumberOrTag, Bytes, Transaction, TransactionRequest, TrieAccount,
    B256, U256, EMPTY_ROOT_HASH,
};

struct TestDatabase(PathBuf);

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn setup_engine() -> (RPCEngine, TestDatabase) {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("wasix-rpc-simulation-{unique}"));
    std::fs::create_dir_all(&path).unwrap();
    let db = EthDatabase::open(path.join("simulation.redb").as_path()).unwrap();
    db.init_tables().unwrap();

    let read_storage = DatabaseReadProvider::new(db.inner());
    let write_storage = DatabaseWriteProvider::new(db.inner());
    write_storage.set_metadata("chain_id".to_string(), 1u64.to_be_bytes().to_vec().into()).unwrap();
    write_storage.clone().commit().unwrap();

    let canonical = Arc::new(CanonicalState::new(read_storage.clone(), write_storage.clone()));
    let block_tree = Arc::new(BlockTree::new());
    let reorg_handler = Arc::new(ReorgHandler::new(
        read_storage.clone(),
        write_storage.clone(),
        canonical.clone(),
    ));
    let chain = Arc::new(ChainManagerImpl::new(
        read_storage.clone(),
        write_storage.clone(),
        canonical.clone(),
        block_tree,
        reorg_handler,
    ));
    let execution: Arc<dyn ExecutionProvider> = Arc::new(EthExecutionProvider::new(
        read_storage.clone(),
        write_storage.clone(),
    ));
    let account_manager = Arc::new(AccountManager::new());
    let mempool: Arc<dyn MempoolProvider> = Arc::new(Mempool::new(U256::ZERO));
    let (event_tx, _) = broadcast::channel::<EngineEvent>(1);
    let consensus: Arc<dyn Consensus> = Arc::new(EthConsensus::new(Arc::new(read_storage.clone())));

    let engine = RPCEngine::new(
        read_storage,
        write_storage,
        account_manager,
        chain,
        mempool,
        event_tx,
        consensus,
        execution,
        canonical,
    );

    (engine, TestDatabase(path))
}

#[tokio::test]
async fn rpc_simulations_use_request_caller_and_nonce_without_persisting_changes() {
    let (engine, _database) = setup_engine();
    let caller = Address::repeat_byte(0xa7);
    let contract = Address::repeat_byte(0xcc);
    let code = Bytes::from(vec![0x33, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3]);
    let code_hash = wasix_eth_types::keccak256(&code);

    engine.write_storage.insert_bytecode(code_hash, code).unwrap();
    engine.write_storage.update_account(contract, TrieAccount {
        nonce: 1,
        balance: U256::ZERO,
        storage_root: EMPTY_ROOT_HASH,
        code_hash,
    }).unwrap();
    engine.write_storage.update_account(caller, TrieAccount {
        nonce: 4,
        balance: U256::ZERO,
        storage_root: EMPTY_ROOT_HASH,
        code_hash: B256::ZERO,
    }).unwrap();
    engine.write_storage.clone().commit().unwrap();

    let state_root = engine.write_storage.calculate_state_root(true, None).unwrap();
    let mut block = Block::<Transaction>::default();
    block.header.number = 1;
    block.header.gas_limit = 30_000_000;
    block.header.state_root = state_root;
    engine.write_storage.insert_block(block, vec![], vec![]).unwrap();
    let block_id = BlockId::Number(BlockNumberOrTag::Number(1));

    let mut request = TransactionRequest::default();
    request.from = Some(caller);
    request.to = Some(contract.into());
    request.nonce = Some(4);
    request.gas = Some(100_000);
    request.gas_price = Some(1);

    let output = engine.call(request.clone(), Some(block_id)).await.unwrap();
    assert_eq!(output.len(), 32);
    assert_eq!(&output[12..], caller.as_slice());

    let estimated_gas = engine.estimate_gas(request.clone(), Some(block_id)).await.unwrap();
    assert!(estimated_gas > U256::ZERO);

    let caller_after = engine.read_storage.account(caller, Some(state_root)).unwrap().unwrap();
    assert_eq!(caller_after.nonce, 4);
    assert_eq!(caller_after.balance, U256::ZERO);

    let mut future_nonce_request = request.clone();
    future_nonce_request.nonce = Some(5);
    let output = engine.call(future_nonce_request.clone(), Some(block_id)).await.unwrap();
    assert_eq!(&output[12..], caller.as_slice());
    assert!(engine.estimate_gas(future_nonce_request, Some(block_id)).await.unwrap() > U256::ZERO);

    let mut omitted_nonce_request = request.clone();
    omitted_nonce_request.nonce = None;
    let output = engine.call(omitted_nonce_request, Some(block_id)).await.unwrap();
    assert_eq!(&output[12..], caller.as_slice());

    let mut default_caller_request = request;
    default_caller_request.from = None;
    default_caller_request.nonce = None;
    let output = engine.call(default_caller_request, Some(block_id)).await.unwrap();
    assert_eq!(output.len(), 32);
}