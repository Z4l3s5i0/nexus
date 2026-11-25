use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use wasix_eth_execution::execution_provider::{EthExecutionProvider, ExecutionProvider};
use wasix_eth_execution::transaction::SimulationContext;
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::{AccountProvider, BlockProvider};
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_storage::write_traits::{AccountWriter, BytecodeWriter};
use wasix_eth_storage::EthDatabase;
use wasix_eth_types::genesis::GenesisConfiguration;
use wasix_eth_types::{
    Address, Block, Bytes, Signature, Signed, SignerRecoverable, Transaction, TrieAccount,
    TxLegacy, B256, U256, EMPTY_ROOT_HASH,
};

struct TestDatabase(PathBuf);

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn simulation_uses_requested_caller_without_changing_block_sender_validation() {
    let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("wasix-execution-simulation-{unique}"));
    std::fs::create_dir_all(&path).unwrap();
    let _database = TestDatabase(path.clone());
    let db = EthDatabase::open(path.join("simulation.redb").as_path()).unwrap();
    db.init_tables().unwrap();
    db.init_genesis(GenesisConfiguration::default()).unwrap();

    let read_provider = DatabaseReadProvider::new(db.inner());
    let write_provider = DatabaseWriteProvider::new(db.inner());
    let caller = Address::repeat_byte(0xaa);
    let contract = Address::repeat_byte(0xcc);
    let code = Bytes::from(vec![0x33, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xf3]);
    let code_hash = wasix_eth_types::keccak256(&code);
    write_provider.insert_bytecode(code_hash, code).unwrap();
    write_provider.update_account(contract, TrieAccount {
        nonce: 1,
        balance: U256::ZERO,
        storage_root: EMPTY_ROOT_HASH,
        code_hash,
    }).unwrap();
    write_provider.update_account(caller, TrieAccount {
        nonce: 2,
        balance: U256::ZERO,
        storage_root: EMPTY_ROOT_HASH,
        code_hash: B256::ZERO,
    }).unwrap();

    let tx = Transaction::Legacy(Signed::new_unchecked(
        TxLegacy {
            nonce: 3,
            gas_price: 1,
            gas_limit: 100_000,
            to: contract.into(),
            value: U256::ZERO,
            input: Bytes::default(),
            chain_id: Some(1),
        },
        Signature::test_signature(),
        B256::ZERO,
    ));
    let recovered_sender = tx.recover_signer().unwrap();
    assert_ne!(caller, recovered_sender);
    write_provider.update_account(recovered_sender, TrieAccount {
        nonce: 0,
        balance: U256::from(1_000_000),
        storage_root: EMPTY_ROOT_HASH,
        code_hash: B256::ZERO,
    }).unwrap();
    write_provider.clone().commit().unwrap();

    let execution = EthExecutionProvider::new(read_provider.clone(), write_provider);
    let (results, _) = execution.run_simulation_with_state_root_and_context(
        vec![tx.clone()],
        Block {
            header: wasix_eth_types::Header {
                gas_limit: 30_000_000,
                ..Default::default()
            },
            ..Default::default()
        },
        None,
        SimulationContext { caller },
    ).expect("simulation should validate against the requested caller");
    assert_eq!(results[0].output.len(), 32);
    assert_eq!(&results[0].output[12..], caller.as_slice());

    let caller_after = read_provider.account(caller, None).unwrap().unwrap();
    assert_eq!(caller_after.nonce, 2);
    assert_eq!(caller_after.balance, U256::ZERO);

    let mut block = Block::<Transaction>::default();
    block.header.number = 1;
    block.header.gas_limit = 30_000_000;
    block.header.parent_hash = read_provider.block_hash(0).unwrap().unwrap();
    block.body.transactions.push(tx);
    let error = execution.execute_block(block).unwrap_err();
    assert!(error.to_string().contains("expected 0, got 3"));
}