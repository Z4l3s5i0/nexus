use crate::mempool::mempool_provider::MempoolProvider;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::HeaderProvider;
use wasix_eth_types::{Block, BlockId, SignerRecoverable, Transaction, B256, U256};
use wasix_eth_utils::{error, info, debug};
use futures_util::stream::{self, StreamExt};

#[derive(Debug)]
enum MempoolEvent {
    CanonicalBlock(Block<Transaction>),
    Reorg(B256),
}

pub struct MempoolListener {
    event_tx: mpsc::UnboundedSender<MempoolEvent>,
    reorg_tx: mpsc::UnboundedSender<B256>,
}

impl MempoolListener {
    pub fn new(
        mempool: Arc<dyn MempoolProvider>,
        read_storage: DatabaseReadProvider,
    ) -> Self {
        let (event_tx, mut event_rx) = mpsc::unbounded_channel::<MempoolEvent>();
        let (reorg_tx, mut reorg_rx) = mpsc::unbounded_channel::<B256>();

        let listener = Self { event_tx, reorg_tx };

        // Spawn background task to process events
        tokio::spawn(async move {
            let pending_blocks = Arc::new(Mutex::new(Vec::new()));

            loop {
                tokio::select! {
                    // 1. Prioritize Reorgs
                    Some(head_hash) = reorg_rx.recv() => {
                        info!("[MempoolListener] Prioritizing reorg to block {:?}", head_hash);
                        // Clear pending blocks as they are obsolete
                        pending_blocks.lock().await.clear();
                        Self::process_reorg(mempool.clone(), read_storage.clone(), head_hash).await;
                    }
                    // 2. Process CanonicalBlocks
                    Some(event) = event_rx.recv() => {
                        match event {
                            MempoolEvent::CanonicalBlock(block) => {
                                let mut blocks = pending_blocks.lock().await;
                                blocks.push(block);
                                
                                // Load shedding: if too many blocks, trigger full revalidation
                                if blocks.len() > 50 {
                                    info!("[MempoolListener] Too many pending blocks ({}), triggering load shedding revalidation", blocks.len());
                                    // Process all blocks' tx removals first
                                    for b in blocks.drain(..) {
                                        Self::remove_included_txs(mempool.clone(), &b).await;
                                    }
                                    // Then full revalidation
                                    mempool.revalidate(&read_storage, None).await;
                                } else {
                                    // Drop lock to process
                                    drop(blocks);
                                    let mut blocks = pending_blocks.lock().await;
                                    if let Some(block) = blocks.pop() {
                                        drop(blocks);
                                        Self::process_canonical_block(mempool.clone(), read_storage.clone(), block).await;
                                    }
                                }
                            }
                            MempoolEvent::Reorg(head_hash) => {
                                // This case should be handled by reorg_rx now, but for safety:
                                pending_blocks.lock().await.clear();
                                Self::process_reorg(mempool.clone(), read_storage.clone(), head_hash).await;
                            }
                        }
                    }
                    else => break,
                }
            }
        });

        listener
    }

    pub async fn handle_canonical_block(&self, block: Block<Transaction>) {
        let _ = self.event_tx.send(MempoolEvent::CanonicalBlock(block));
    }

    async fn remove_included_txs(mempool: Arc<dyn MempoolProvider>, block: &Block<Transaction>) {
        let mut tx_hashes = Vec::new();
        for tx in &block.body.transactions {
            tx_hashes.push(*tx.hash());
        }
        if !tx_hashes.is_empty() {
            debug!("[MempoolListener] Removing {} transactions from block #{}", tx_hashes.len(), block.header.number);
            mempool.remove_transactions(&tx_hashes).await;
        }
    }

    async fn process_canonical_block(mempool: Arc<dyn MempoolProvider>, read_storage: DatabaseReadProvider, block: Block<Transaction>) {
        let base_fee = block.header.base_fee_per_gas.unwrap_or_default();
        let mut affected_addresses = Vec::new();

        for tx in &block.body.transactions {
            if let Ok(sender) = tx.recover_signer() {
                affected_addresses.push(sender);
            }
        }

        // 1. Remove included transactions FIRST (Decoupled)
        Self::remove_included_txs(mempool.clone(), &block).await;

        // 2. Update base fee
        let old_base_fee = mempool.base_fee().await;
        let new_base_fee = U256::from(base_fee);
        mempool.update_base_fee(new_base_fee, &read_storage).await;

        // 3. Revalidate affected accounts in parallel
        if new_base_fee <= old_base_fee && !affected_addresses.is_empty() {
            affected_addresses.sort();
            affected_addresses.dedup();
            
            info!("[MempoolListener] Revalidating {} affected accounts for block #{}", affected_addresses.len(), block.header.number);
            
            // Partition addresses for parallel processing
            stream::iter(affected_addresses)
                .for_each_concurrent(4, |address| {
                    let mempool = mempool.clone();
                    let read_storage = read_storage.clone();
                    async move {
                        mempool.revalidate(&read_storage, Some(vec![address])).await;
                    }
                })
                .await;
        }
    }

    pub async fn handle_reorg(&self, head_hash: B256) {
        let _ = self.reorg_tx.send(head_hash);
    }

    async fn process_reorg(mempool: Arc<dyn MempoolProvider>, read_storage: DatabaseReadProvider, head_hash: B256) {
        info!("[MempoolListener] Handling reorg to block {:?}", head_hash);
        if let Ok(Some(header)) = read_storage.header(BlockId::Hash(head_hash.into())).await {
            let base_fee = header.base_fee_per_gas.unwrap_or_default();
            mempool.update_base_fee(U256::from(base_fee), &read_storage).await;
            mempool.revalidate(&read_storage, None).await;
        } else {
            error!("[MempoolListener] Failed to find header for new head {:?} after reorg", head_hash);
        }
    }
}
