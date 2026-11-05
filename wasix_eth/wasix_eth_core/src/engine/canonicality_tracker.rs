use alloy_primitives::B256;
use tokio::sync::RwLock;
use wasix_eth_storage::{BlockProvider, BlockWriter};
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_types::ForkchoiceState;
use wasix_eth_utils::error;

pub struct CanonicalState {
    write_storage: DatabaseWriteProvider,
    /// Current canonical state
    state: RwLock<(ForkchoiceState, u64)>,
}

impl CanonicalState {
    pub fn new(read_storage: DatabaseReadProvider, write_storage: DatabaseWriteProvider) -> Self {
        let head_hash = read_storage.forkchoice("head").unwrap_or(None).unwrap_or_default();
        let head_number = if head_hash != B256::ZERO {
            read_storage.block_number(head_hash).unwrap_or(None).unwrap_or_default()
        } else {
            0
        };
        let safe_hash = read_storage.forkchoice("safe").unwrap_or(None).unwrap_or_default();
        let finalized_hash = read_storage.forkchoice("finalized").unwrap_or(None).unwrap_or_default();

        let state = ForkchoiceState {
            head_block_hash: head_hash,
            safe_block_hash: safe_hash,
            finalized_block_hash: finalized_hash,
        };

        Self {
            write_storage,
            state: RwLock::new((state, head_number)),
        }
    }

    pub async fn get_head(&self) -> (B256, u64) {
        let (state, number) = *self.state.read().await;
        (state.head_block_hash, number)
    }

    pub async fn get_safe(&self) -> B256 {
        self.state.read().await.0.safe_block_hash
    }

    pub async fn get_finalized(&self) -> B256 {
        self.state.read().await.0.finalized_block_hash
    }

    pub async fn get_state(&self) -> (ForkchoiceState, u64) {
        *self.state.read().await
    }

    pub async fn update_head(&self, hash: B256, number: u64) -> anyhow::Result<()> {
        let mut lock = self.state.write().await;
        if lock.0.head_block_hash == hash && lock.1 == number {
            return Ok(());
        }
        lock.0.head_block_hash = hash;
        lock.1 = number;
        let state = lock.0;
        drop(lock);

        let storage = self.write_storage.clone();
        tokio::spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                storage.update_forkchoice(state.head_block_hash, Some(state.safe_block_hash), Some(state.finalized_block_hash))
            }).await;
            
            if let Err(e) = res {
                error!("Storage task panicked during head update: {}", e);
            } else if let Ok(Err(e)) = res {
                error!("Storage error during head update: {}", e);
            }
        });
        Ok(())
    }

    pub async fn update_safe(&self, hash: B256) -> anyhow::Result<()> {
        let mut lock = self.state.write().await;
        if lock.0.safe_block_hash == hash {
            return Ok(());
        }
        lock.0.safe_block_hash = hash;
        let state = lock.0;
        drop(lock);

        let storage = self.write_storage.clone();
        tokio::spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                storage.update_forkchoice(state.head_block_hash, Some(state.safe_block_hash), Some(state.finalized_block_hash))
            }).await;

            if let Err(e) = res {
                error!("Storage task panicked during safe update: {}", e);
            } else if let Ok(Err(e)) = res {
                error!("Storage error during safe update: {}", e);
            }
        });
        Ok(())
    }

    pub async fn update_finalized(&self, hash: B256) -> anyhow::Result<()> {
        let mut lock = self.state.write().await;
        if lock.0.finalized_block_hash == hash {
            return Ok(());
        }
        lock.0.finalized_block_hash = hash;
        let state = lock.0;
        drop(lock);

        let storage = self.write_storage.clone();
        tokio::spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                storage.update_forkchoice(state.head_block_hash, Some(state.safe_block_hash), Some(state.finalized_block_hash))
            }).await;

            if let Err(e) = res {
                error!("Storage task panicked during finalized update: {}", e);
            } else if let Ok(Err(e)) = res {
                error!("Storage error during finalized update: {}", e);
            }
        });
        Ok(())
    }

    pub async fn update_state(&self, state: ForkchoiceState, head_number: u64) -> anyhow::Result<()> {
        let mut lock = self.state.write().await;
        if lock.0 == state && lock.1 == head_number {
            return Ok(());
        }
        *lock = (state, head_number);
        drop(lock);

        let storage = self.write_storage.clone();
        tokio::spawn(async move {
            let res = tokio::task::spawn_blocking(move || {
                storage.update_forkchoice(state.head_block_hash, Some(state.safe_block_hash), Some(state.finalized_block_hash))
            }).await;

            if let Err(e) = res {
                error!("Storage task panicked during state update: {}", e);
            } else if let Ok(Err(e)) = res {
                error!("Storage error during state update: {}", e);
            }
        });
        Ok(())
    }
}