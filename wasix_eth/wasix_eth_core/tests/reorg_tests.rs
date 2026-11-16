use std::sync::Arc;
use wasix_eth_core::chain_manager::{ChainManager, ChainManagerImpl};
use wasix_eth_storage::EthDatabase;
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_storage::read_traits::BlockProvider;
use wasix_eth_storage::write_traits::{BlockWriter, HeaderWriter};
use wasix_eth_types::{Block, BlockBody, Header, Transaction, B256};
use tempfile::TempDir;

fn setup() -> (ChainManagerImpl, Arc<EthDatabase>, TempDir) {
    let temp_dir = TempDir::new_in("D:\\MA\\wasix_eth").unwrap();
    let db_path = temp_dir.path().join("db");
    let db = Arc::new(EthDatabase::open(&db_path).unwrap());
    db.init_tables().unwrap();
    
    let reader = DatabaseReadProvider::new(db.inner());
    let writer = DatabaseWriteProvider::new(db.inner());
    let chain_manager = ChainManagerImpl::new(reader.clone(), writer.clone());
    
    (chain_manager, db, temp_dir)
}

fn create_mock_header(number: u64, parent_hash: B256) -> Header {
    Header {
        number,
        parent_hash,
        ..Default::default()
    }
}

#[tokio::test]
async fn test_deep_reorg() {
    let (chain_manager, db, _tmp) = setup();
    let reader = DatabaseReadProvider::new(db.inner());
    let writer = DatabaseWriteProvider::new(db.inner());

    // 1. Build a chain of 5 blocks (1->2->3->4->5)
    let mut parent_hash = B256::ZERO;
    let mut main_chain_hashes = Vec::new();
    
    for i in 1..=5 {
        let header = create_mock_header(i, parent_hash);
        let hash = header.hash_slow();
        writer.insert_header(hash, header).unwrap();
        writer.insert_block_hash(hash, i).unwrap();
        writer.set_canonical(i, hash).unwrap();
        parent_hash = hash;
        main_chain_hashes.push(hash);
    }
    
    chain_manager.set_head_block(main_chain_hashes[4], 5).await;
    assert_eq!(reader.latest_block_number().unwrap(), Some(5));

    // 2. Build a competing chain starting from block 2 (1->2->3'->4'->5'->6')
    // Parent of 3' is 2
    let parent_2_hash = main_chain_hashes[1];
    let mut side_chain_hashes = Vec::new();
    let mut p_hash = parent_2_hash;
    
    for i in 3..=6 {
        let mut header = create_mock_header(i, p_hash);
        header.extra_data = wasix_eth_types::Bytes::from(format!("sidechain-{}", i).into_bytes());
        let hash = header.hash_slow();
        writer.insert_header(hash, header).unwrap();
        writer.insert_block_hash(hash, i).unwrap();
        // Do NOT set canonical yet
        p_hash = hash;
        side_chain_hashes.push(hash);
    }

    // 3. Trigger the ChainManager to recognize the second chain as canonical.
    // In this implementation, we manually trigger the reorg via revert_to_height and then setting new blocks
    chain_manager.revert_to_height(5, 2).await.unwrap();
    
    // After revert, latest should be 2
    assert_eq!(reader.latest_block_number().unwrap(), Some(2));
    
    // Now apply the side chain
    for (i, hash) in side_chain_hashes.iter().enumerate() {
        let height = (i + 3) as u64;
        writer.set_canonical(height, *hash).unwrap();
    }
    chain_manager.set_head_block(side_chain_hashes[3], 6).await;

    // 4. Verify latest_block_number() returns 6 and block_hash(3) returns the hash of 3'.
    assert_eq!(reader.latest_block_number().unwrap(), Some(6));
    assert_eq!(reader.block_hash(3).unwrap(), Some(side_chain_hashes[0]));
    assert_ne!(reader.block_hash(3).unwrap(), Some(main_chain_hashes[2]));
}

#[tokio::test]
async fn test_same_height_reorg() {
    let (chain_manager, db, _tmp) = setup();
    let reader = DatabaseReadProvider::new(db.inner());
    let writer = DatabaseWriteProvider::new(db.inner());

    // 1. Build a chain of 5 blocks (1->2->3->4->5)
    let mut parent_hash = B256::ZERO;
    let mut main_chain_hashes = Vec::new();
    
    for i in 1..=5 {
        let header = create_mock_header(i, parent_hash);
        let hash = header.hash_slow();
        writer.insert_header(hash, header.clone()).unwrap();
        writer.insert_block_hash(hash, i).unwrap();
        writer.insert_block_body(hash, i, BlockBody::default()).unwrap();
        writer.set_canonical(i, hash).unwrap();
        parent_hash = hash;
        main_chain_hashes.push(hash);
    }
    
    chain_manager.set_head_block(main_chain_hashes[4], 5).await;
    assert_eq!(reader.block_hash(5).unwrap(), Some(main_chain_hashes[4]));

    // 2. Build a competing block at same height 5 (parent 4 -> 5')
    let parent_4_hash = main_chain_hashes[3];
    let mut header_5_prime = create_mock_header(5, parent_4_hash);
    header_5_prime.extra_data = wasix_eth_types::Bytes::from("competing-5");
    let hash_5_prime = header_5_prime.hash_slow();
    writer.insert_header(hash_5_prime, header_5_prime.clone()).unwrap();
    writer.insert_block_hash(hash_5_prime, 5).unwrap();
    writer.insert_block_body(hash_5_prime, 5, BlockBody::default()).unwrap();

    // 3. Resolve reorg to 5'
    let context = chain_manager.resolve_reorg(main_chain_hashes[4], hash_5_prime).await.unwrap();
    assert!(context.is_reorg);
    assert_eq!(context.common_ancestor_hash, parent_4_hash);
    assert_eq!(context.new_canonical_blocks.len(), 1);
    assert_eq!(context.new_canonical_blocks[0].header.hash_slow(), hash_5_prime);

    // 4. Perform reorg
    chain_manager.revert_to_height(5, 4).await.unwrap();
    chain_manager.mark_branch_canonical(&context.new_canonical_blocks).await.unwrap();

    // 5. Verify
    assert_eq!(reader.block_hash(5).unwrap(), Some(hash_5_prime));
    assert_eq!(reader.latest_block_number().unwrap(), Some(5));
}
