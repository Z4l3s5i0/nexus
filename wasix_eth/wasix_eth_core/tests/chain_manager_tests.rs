#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use wasix_eth_storage::EthDatabase;
    use wasix_eth_storage::write_traits::*;
    use wasix_eth_types::{Header, B256, SyncStatus, U256, BlockBody};
    use std::sync::Arc;
    use tokio;

    async fn setup() -> (ChainManagerImpl, Arc<EthDatabase>, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.redb");
        let db = EthDatabase::open(&db_path).unwrap();
        db.init_tables().unwrap();
        let db_arc = Arc::new(db);
        let read = DatabaseReadProvider::new(db_arc.inner());
        let write = DatabaseWriteProvider::new(db_arc.inner());
        (ChainManagerImpl::new(read, write), db_arc, dir)
    }

    #[tokio::test]
    async fn test_sync_status_success() {
        let (cm, _, _dir) = setup().await;
        let status = SyncStatus::Info(Box::new(alloy_rpc_types::SyncInfo {
            starting_block: U256::ZERO,
            current_block: U256::from(10),
            highest_block: U256::from(100),
            warp_chunks_amount: None,
            warp_chunks_processed: None,
            stages: None,
        }));
        cm.set_sync_status(status.clone()).await;
        match (cm.sync_status().await, status) {
            (SyncStatus::Info(a), SyncStatus::Info(b)) => assert_eq!(a.current_block, b.current_block),
            _ => panic!("Expected SyncStatus::Info"),
        }
    }

    #[tokio::test]
    async fn test_sync_status_default() {
        let (cm, _, _dir) = setup().await;
        assert!(matches!(cm.sync_status().await, SyncStatus::None));
    }

    #[tokio::test]
    async fn test_sync_status_rapid_updates() {
        let (cm, _, _dir) = setup().await;
        for i in 0..100 {
            let status = if i % 2 == 0 { SyncStatus::None } else {
                SyncStatus::Info(Box::new(alloy_rpc_types::SyncInfo {
                    starting_block: U256::ZERO,
                    current_block: U256::from(i),
                    highest_block: U256::from(100),
                    warp_chunks_amount: None,
                    warp_chunks_processed: None,
                    stages: None,
                }))
            };
            cm.set_sync_status(status).await;
        }
        // Just verify it doesn't deadlock and has the last value
        if let SyncStatus::Info(info) = cm.sync_status().await {
            assert_eq!(info.current_block, U256::from(99));
        } else {
            panic!("Expected SyncStatus::Info");
        }
    }

    #[tokio::test]
    async fn test_head_block_success() {
        let (cm, _, _dir) = setup().await;
        let hash = B256::repeat_byte(0x11);
        cm.set_head_block(hash, 10).await;
        assert_eq!(cm.head_block().await, (hash, 10));
    }

    #[tokio::test]
    async fn test_head_block_initialization() {
        let (cm, _, _dir) = setup().await;
        assert_eq!(cm.head_block().await, (B256::ZERO, 0));
    }

    #[tokio::test]
    async fn test_head_block_not_persisted_automatically() {
        let (cm, db, _dir) = setup().await;
        let hash = B256::repeat_byte(0x22);
        cm.set_head_block(hash, 10).await;

        // Re-create ChainManager from same DB
        let read = DatabaseReadProvider::new(db.inner());
        let write = DatabaseWriteProvider::new(db.inner());
        let cm2 = ChainManagerImpl::new(read, write);

        // It should NOT have the new head because it wasn't written to Forkchoice table
        assert_eq!(cm2.head_block().await, (B256::ZERO, 0));
    }

    #[tokio::test]
    async fn test_has_block_success() {
        let (cm, db, _dir) = setup().await;
        let hash = B256::repeat_byte(0x33);
        let writer = DatabaseWriteProvider::new(db.inner());
        let mut header = Header::default();
        header.number = 1;
        writer.insert_header(1, header).unwrap();
        writer.insert_block_hash(hash, 1).unwrap();
        writer.insert_block_body(hash, 1, BlockBody::default()).unwrap();
        writer.commit().unwrap();

        assert!(cm.has_block(hash).await);
    }

    #[tokio::test]
    async fn test_has_block_failure() {
        let (cm, _, _dir) = setup().await;
        assert!(!cm.has_block(B256::repeat_byte(0x44)).await);
    }

    #[tokio::test]
    async fn test_has_block_after_revert() {
        let (cm, db, _dir) = setup().await;
        let hash = B256::repeat_byte(0x55);
        let writer = DatabaseWriteProvider::new(db.inner());
        let mut header = Header::default();
        header.number = 1;
        writer.insert_header(1, header).unwrap();
        writer.insert_block_hash(hash, 1).unwrap();
        writer.insert_block_body(hash, 1, BlockBody::default()).unwrap();
        writer.set_canonical(1, hash).unwrap();
        writer.commit().unwrap();

        cm.set_head_block(hash, 1).await;

        assert!(cm.has_block(hash).await);
    }

    #[tokio::test]
    async fn test_revert_to_height_success() {
        let (cm, db, _dir) = setup().await;
        let writer = DatabaseWriteProvider::new(db.inner());

        // Block 1
        let hash1 = B256::repeat_byte(1);
        let mut h1 = Header::default(); h1.number = 1;
        writer.insert_header(1, h1).unwrap();
        writer.insert_block_hash(hash1, 1).unwrap();
        writer.set_canonical(1, hash1).unwrap();

        // Block 2
        let hash2 = B256::repeat_byte(2);
        let mut h2 = Header::default(); h2.number = 2;
        writer.insert_header(2, h2).unwrap();
        writer.insert_block_hash(hash2, 2).unwrap();
        writer.set_canonical(2, hash2).unwrap();

        writer.commit().unwrap();
        cm.set_head_block(hash2, 2).await;

        cm.revert_to_height(2, 1).await.unwrap();
        assert_eq!(cm.head_block().await, (hash1, 1));
    }

    #[tokio::test]
    async fn test_revert_to_height_no_op() {
        let (cm, _, _dir) = setup().await;
        cm.set_head_block(B256::repeat_byte(1), 1).await;
        cm.revert_to_height(1, 1).await.unwrap();
        assert_eq!(cm.head_block().await, (B256::repeat_byte(1), 1));

        cm.revert_to_height(1, 1).await.unwrap();
        assert_eq!(cm.head_block().await, (B256::repeat_byte(1), 1));
    }

    #[tokio::test]
    async fn test_revert_to_height_zero() {
        let (cm, db, _dir) = setup().await;
        let writer = DatabaseWriteProvider::new(db.inner());

        // Genesis
        let hash0 = B256::repeat_byte(0);
        let mut h0 = Header::default(); h0.number = 0;
        writer.insert_header(0, h0).unwrap();
        writer.insert_block_hash(hash0, 0).unwrap();
        writer.set_canonical(0, hash0).unwrap();

        // Block 1
        let hash1 = B256::repeat_byte(1);
        let mut h1 = Header::default(); h1.number = 1;
        writer.insert_header(1, h1).unwrap();
        writer.insert_block_hash(hash1, 1).unwrap();
        writer.set_canonical(1, hash1).unwrap();

        writer.commit().unwrap();
        cm.set_head_block(hash1, 1).await;

        cm.revert_to_height(1, 0).await.unwrap();
        assert_eq!(cm.head_block().await, (hash0, 0));
    }

    #[tokio::test]
    async fn test_trigger_sync_success() {
        let (cm, _, _dir) = setup().await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        cm.set_sync_trigger(Box::new(move || {
            let tx = tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(true).await;
            });
        })).await;

        cm.trigger_sync().await.unwrap();
        assert!(rx.recv().await.unwrap());
    }

    #[tokio::test]
    async fn test_trigger_sync_no_trigger() {
        let (cm, _, _dir) = setup().await;
        cm.trigger_sync().await.unwrap(); // Should not panic
    }

    #[tokio::test]
    async fn test_trigger_sync_multiple_calls() {
        let (cm, _, _dir) = setup().await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(10);
        cm.set_sync_trigger(Box::new(move || {
            let tx = tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(true).await;
            });
        })).await;

        cm.trigger_sync().await.unwrap();
        cm.trigger_sync().await.unwrap();

        assert!(rx.recv().await.unwrap());
        assert!(rx.recv().await.unwrap());
    }
}