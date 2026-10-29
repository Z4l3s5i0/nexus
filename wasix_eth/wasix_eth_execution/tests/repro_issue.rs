use wasix_eth_execution::execution_provider::{EthExecutionProvider, ExecutionProvider};
use wasix_eth_storage::EthDatabase;
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_storage::read_traits::{BlockProvider, HeaderProvider};
use wasix_eth_types::*;
use wasix_eth_types::genesis::GenesisConfiguration;
use tempfile::TempDir;
use alloy_signer_local::PrivateKeySigner;
use alloy_eips::eip2930::AccessList;
use alloy_network::TxSignerSync;

#[tokio::test]
async fn test_contract_deployment_and_call() {
    let temp_dir = TempDir::new().unwrap();
    let db = EthDatabase::open(temp_dir.path().join("repro.db").as_path()).unwrap();
    db.init_tables().unwrap();

    let mut genesis_config = GenesisConfiguration::default();
    genesis_config.config.london_block = Some(0);
    genesis_config.config.terminal_total_difficulty = Some(U256::ZERO);
    genesis_config.config.shanghai_time = Some(0);
    genesis_config.config.cancun_time = Some(0);
    genesis_config.config.chain_id = 1337;

    let signer: PrivateKeySigner = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".parse().unwrap();
    let sender = signer.address();

    // Fund the sender
    genesis_config.alloc.insert(sender, wasix_eth_types::genesis::GenesisAccount {
        balance: U256::from(10u128.pow(21)), // 1000 ETH
        ..Default::default()
    });

    db.init_genesis(genesis_config.clone()).unwrap();

    let read_provider = DatabaseReadProvider::new(db.inner());
    let write_provider = DatabaseWriteProvider::new(db.inner());
    let execution = EthExecutionProvider::new(read_provider.clone(), write_provider);

    // 1. Deploy contract
    // Simple contract that stores a value:
    // 6001600055 - SSTORE(0, 1)
    // 60005460005260206000f3 - mstore(0, sload(0)); return(0, 32)
    // constructor: 601080600d6000396000f3 (11 bytes)
    // bytecode: 600160005560005460005260206000f3 (16 bytes)
    // total: 27 bytes (0x1b)
    let bytecode = hex::decode("600160005560005460005260206000f3").unwrap();
    let mut init_code = hex::decode("601080600b6000396000f3").unwrap();
    init_code.extend_from_slice(&bytecode);

    let mut tx_deploy_inner = TxEip1559 {
        chain_id: 1337,
        nonce: 0,
        max_priority_fee_per_gas: 10,
        max_fee_per_gas: 100,
        gas_limit: 1000000,
        to: TxKind::Create,
        value: U256::ZERO,
        input: init_code.into(),
        access_list: AccessList::default(),
    };

    let signature = signer.sign_transaction_sync(&mut tx_deploy_inner).unwrap();
    let tx_deploy = Transaction::Eip1559(Signed::new_unchecked(
        tx_deploy_inner,
        signature,
        B256::ZERO,
    ));

    let mut block = Block::<Transaction>::default();
    block.header.number = 1;
    block.header.parent_hash = read_provider.block_hash(0).unwrap().unwrap();
    block.header.timestamp = 1000;
    block.header.gas_limit = 30000000;
    block.header.base_fee_per_gas = Some(10);
    block.body.transactions.push(tx_deploy.clone());

    // Lookup state root for block 0
    let state_root_0 = read_provider.header(BlockId::Number(BlockNumberOrTag::Number(0))).unwrap().unwrap().state_root;

    let (final_block, receipts, metas) = execution.execute_block_with_state_root_and_building(block, true, Some(state_root_0), false).expect("Deployment execution failed");
    assert!(match receipts[0].receipt.status { Eip658Value::Eip658(s) => s, _ => false }, "Deployment failed");
    let contract_address = metas[0].contract_address.expect("No contract address");

    // 1.5 Call contract in the SAME block
    let mut tx_call_same_inner = TxEip1559 {
        chain_id: 1337,
        nonce: 1,
        max_priority_fee_per_gas: 10,
        max_fee_per_gas: 100,
        gas_limit: 1000000,
        to: TxKind::Call(contract_address),
        value: U256::ZERO,
        input: Bytes::new(),
        access_list: AccessList::default(),
    };
    let signature = signer.sign_transaction_sync(&mut tx_call_same_inner).unwrap();
    let tx_call_same = Transaction::Eip1559(Signed::new_unchecked(
        tx_call_same_inner,
        signature,
        B256::ZERO,
    ));

    let mut block_same = Block::<Transaction>::default();
    block_same.header.number = 2; // Actually we want to test multi-tx block
    block_same.header.parent_hash = read_provider.block_hash(0).unwrap().unwrap();
    block_same.header.timestamp = 1000;
    block_same.header.gas_limit = 30000000;
    block_same.header.base_fee_per_gas = Some(10);
    block_same.body.transactions.push(tx_deploy.clone());
    block_same.body.transactions.push(tx_call_same);

    let (_, receipts_same, _) = execution.execute_block_with_state_root_and_building(block_same, false, Some(state_root_0), false).expect("Same block execution failed");
    assert!(match receipts_same[0].receipt.status { Eip658Value::Eip658(s) => s, _ => false }, "Deployment in same block failed");
    assert!(match receipts_same[1].receipt.status { Eip658Value::Eip658(s) => s, _ => false }, "Call in same block failed");


    // 2. Call contract in NEXT block
    let mut tx_call_inner = TxEip1559 {
        chain_id: 1337,
        nonce: 1,
        max_priority_fee_per_gas: 10,
        max_fee_per_gas: 100,
        gas_limit: 1000000,
        to: TxKind::Call(contract_address),
        value: U256::ZERO,
        input: Bytes::new(),
        access_list: AccessList::default(),
    };

    let signature = signer.sign_transaction_sync(&mut tx_call_inner).unwrap();
    let tx_call = Transaction::Eip1559(Signed::new_unchecked(
        tx_call_inner,
        signature,
        B256::ZERO,
    ));

    let mut block2 = Block::<Transaction>::default();
    block2.header.number = 3;
    block2.header.parent_hash = final_block.header.hash_slow();
    block2.header.timestamp = 1010;
    block2.header.gas_limit = 30000000;
    block2.header.base_fee_per_gas = Some(10);
    block2.body.transactions.push(tx_call);

    let (final_block2, receipts2, _metas2) = execution.execute_block_with_state_root_and_building(block2, true, Some(final_block.header.state_root), false).expect("Call execution failed");
    assert!(match receipts2[0].receipt.status { Eip658Value::Eip658(s) => s, _ => false }, "Contract call failed");

    // 3. Call contract in NEXT block again (testing persistence across many blocks)
    let mut tx_call2_inner = TxEip1559 {
        chain_id: 1337,
        nonce: 2,
        max_priority_fee_per_gas: 10,
        max_fee_per_gas: 100,
        gas_limit: 1000000,
        to: TxKind::Call(contract_address),
        value: U256::ZERO,
        input: Bytes::new(),
        access_list: AccessList::default(),
    };
    let signature = signer.sign_transaction_sync(&mut tx_call2_inner).unwrap();
    let tx_call2 = Transaction::Eip1559(Signed::new_unchecked(
        tx_call2_inner,
        signature,
        B256::ZERO,
    ));

    let mut block3 = Block::<Transaction>::default();
    block3.header.number = 4;
    block3.header.parent_hash = final_block2.header.hash_slow();
    block3.header.timestamp = 1020;
    block3.header.gas_limit = 30000000;
    block3.header.base_fee_per_gas = Some(10);
    block3.body.transactions.push(tx_call2);

    let state_root_3 = final_block2.header.state_root;
    let (_, receipts3, _) = execution.execute_block_with_state_root_and_building(block3, true, Some(state_root_3), false).expect("Call execution failed");
    assert!(match receipts3[0].receipt.status { Eip658Value::Eip658(s) => s, _ => false }, "Contract call in block 4 failed");
}
