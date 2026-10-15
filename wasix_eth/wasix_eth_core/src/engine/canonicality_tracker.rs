use alloy_primitives::B256;
use arc_swap::ArcSwap;
use std::sync::Arc;
use tokio::sync::Mutex;
use wasix_eth_storage::{BlockProvider, BlockWriter};
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_types::ForkchoiceState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CanonicalStateInner {
    state: ForkchoiceState,
    head_number: u64,
}

pub struct CanonicalState {
    write_storage: DatabaseWriteProvider,
    /// Current canonical state
    state: ArcSwap<CanonicalStateInner>,
    /// Mutex to serialize updates and ensure storage consistency
    update_lock: Mutex<()>,
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
            state: ArcSwap::new(Arc::new(CanonicalStateInner {
                state,
                head_number,
            })),
            update_lock: Mutex::new(()),
        }
    }

    pub fn get_head(&self) -> (B256, u64) {
        let inner = self.state.load();
        (inner.state.head_block_hash, inner.head_number)
    }

    pub fn get_safe(&self) -> B256 {
        self.state.load().state.safe_block_hash
    }

    pub fn get_finalized(&self) -> B256 {
        self.state.load().state.finalized_block_hash
    }

    pub fn get_state(&self) -> (ForkchoiceState, u64) {
        let inner = self.state.load();
        (inner.state, inner.head_number)
    }

    pub async fn update_head(&self, hash: B256, number: u64) -> anyhow::Result<()> {
        let _guard = self.update_lock.lock().await;
        let current = self.state.load();
        if current.state.head_block_hash == hash && current.head_number == number {
            return Ok(());
        }

        let new_state = ForkchoiceState {
            head_block_hash: hash,
            ..current.state
        };

        let storage = self.write_storage.clone();
        tokio::task::spawn_blocking(move || {
            storage.update_forkchoice(new_state.head_block_hash, Some(new_state.safe_block_hash), Some(new_state.finalized_block_hash))
        }).await.map_err(|e| anyhow::anyhow!("Storage task panicked: {}", e))??;

        self.state.store(Arc::new(CanonicalStateInner {
            state: new_state,
            head_number: number,
        }));

        Ok(())
    }

    pub async fn update_safe(&self, hash: B256) -> anyhow::Result<()> {
        let _guard = self.update_lock.lock().await;
        let current = self.state.load();
        if current.state.safe_block_hash == hash {
            return Ok(());
        }

        let new_state = ForkchoiceState {
            safe_block_hash: hash,
            ..current.state
        };

        let storage = self.write_storage.clone();
        tokio::task::spawn_blocking(move || {
            storage.update_forkchoice(new_state.head_block_hash, Some(new_state.safe_block_hash), Some(new_state.finalized_block_hash))
        }).await.map_err(|e| anyhow::anyhow!("Storage task panicked: {}", e))??;

        self.state.store(Arc::new(CanonicalStateInner {
            state: new_state,
            head_number: current.head_number,
        }));

        Ok(())
    }

    pub async fn update_finalized(&self, hash: B256) -> anyhow::Result<()> {
        let _guard = self.update_lock.lock().await;
        let current = self.state.load();
        if current.state.finalized_block_hash == hash {
            return Ok(());
        }

        let new_state = ForkchoiceState {
            finalized_block_hash: hash,
            ..current.state
        };

        let storage = self.write_storage.clone();
        tokio::task::spawn_blocking(move || {
            storage.update_forkchoice(new_state.head_block_hash, Some(new_state.safe_block_hash), Some(new_state.finalized_block_hash))
        }).await.map_err(|e| anyhow::anyhow!("Storage task panicked: {}", e))??;

        self.state.store(Arc::new(CanonicalStateInner {
            state: new_state,
            head_number: current.head_number,
        }));

        Ok(())
    }

    pub async fn update_state(&self, state: ForkchoiceState, head_number: u64) -> anyhow::Result<()> {
        let _guard = self.update_lock.lock().await;
        let current = self.state.load();
        if current.state == state && current.head_number == head_number {
            return Ok(());
        }

        let storage = self.write_storage.clone();
        tokio::task::spawn_blocking(move || {
            storage.update_forkchoice(state.head_block_hash, Some(state.safe_block_hash), Some(state.finalized_block_hash))
        }).await.map_err(|e| anyhow::anyhow!("Storage task panicked: {}", e))??;

        self.state.store(Arc::new(CanonicalStateInner {
            state,
            head_number,
        }));

        Ok(())
    }
}