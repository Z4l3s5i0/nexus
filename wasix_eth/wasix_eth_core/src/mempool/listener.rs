use crate::mempool::mempool_provider::MempoolProvider;
use std::sync::Arc;
use tokio::sync::mpsc;
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::HeaderProvider;
use wasix_eth_types::{Address, Block, BlockId, Transaction, B256, U256};
use wasix_eth_utils::{error, info};

enum MempoolEvent {
    CanonicalBlock {
        block: Block<Transaction>,
        affected_addresses: Option<Vec<Address>>,
    },
    Reorg(B256),
}

pub struct MempoolListener {
    event_tx: mpsc::UnboundedSender<MempoolEvent>,
    mempool: Arc<dyn MempoolProvider>,
}

impl MempoolListener {
    pub fn new(
        mempool: Arc<dyn MempoolProvider>,
        read_storage: DatabaseReadProvider,
    ) -> Self {
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();

        let listener = Self { event_tx: event_tx.clone(), mempool: mempool.clone() };

        // Spawn background task to process events
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                match event {
                    MempoolEvent::CanonicalBlock { block, affected_addresses } => {
                        Self::process_canonical_block(mempool.clone(), read_storage.clone(), block, affected_addresses).await;
                    }
                    MempoolEvent::Reorg(head_hash) => {
                        Self::process_reorg(mempool.clone(), read_storage.clone(), head_hash).await;
                    }
                }
            }
        });

        listener
    }

    pub async fn handle_canonical_block(&self, block: Block<Transaction>) {
        let _ = self.event_tx.send(MempoolEvent::CanonicalBlock { block, affected_addresses: None });
    }

    pub fn remove_included_now(&self, block: &Block<Transaction>) -> Vec<Address> {
        let tx_hashes = block.body.transactions.iter().map(|tx| *tx.hash()).collect::<Vec<_>>();
        let affected_addresses = self.mempool.remove_transactions_now(&tx_hashes);
        if !tx_hashes.is_empty() {
            info!("[MempoolListener] Removed {} included transactions synchronously from canonical block #{}", tx_hashes.len(), block.header.number);
        }
        let _ = self.event_tx.send(MempoolEvent::CanonicalBlock {
            block: block.clone(),
            affected_addresses: Some(affected_addresses.clone()),
        });
        affected_addresses
    }

    async fn process_canonical_block(
        mempool: Arc<dyn MempoolProvider>,
        read_storage: DatabaseReadProvider,
        block: Block<Transaction>,
        affected_addresses: Option<Vec<Address>>,
    ) {
        let base_fee = block.header.base_fee_per_gas.unwrap_or_default();
        let mut affected_addresses = if let Some(affected_addresses) = affected_addresses {
            affected_addresses
        } else {
            let tx_hashes = block.body.transactions.iter().map(|tx| *tx.hash()).collect::<Vec<_>>();
            if !tx_hashes.is_empty() {
                info!("[MempoolListener] Removing {} transactions included in canonical block #{}", tx_hashes.len(), block.header.number);
            }
            mempool.remove_transactions(&tx_hashes).await
        };

        // 2. Update base fee
        let old_base_fee = mempool.base_fee().await;
        let new_base_fee = U256::from(base_fee);
        mempool.update_base_fee(new_base_fee, &read_storage).await;

        // 3. Revalidate affected accounts if full revalidation wasn't already triggered
        if new_base_fee <= old_base_fee && !affected_addresses.is_empty() {
            affected_addresses.sort();
            affected_addresses.dedup();
            mempool.revalidate(&read_storage, Some(affected_addresses)).await;
        }
    }

    pub async fn handle_reorg(&self, head_hash: B256) {
        let _ = self.event_tx.send(MempoolEvent::Reorg(head_hash));
    }

    async fn process_reorg(mempool: Arc<dyn MempoolProvider>, read_storage: DatabaseReadProvider, head_hash: B256) {
        info!("[MempoolListener] Handling reorg to block {:?}", head_hash);
        if let Ok(Some(header)) = read_storage.header(BlockId::Hash(head_hash.into())) {
            let base_fee = header.base_fee_per_gas.unwrap_or_default();
            mempool.update_base_fee(U256::from(base_fee), &read_storage).await;
            mempool.revalidate(&read_storage, None).await;
        } else {
            error!("[MempoolListener] Failed to find header for new head {:?} after reorg", head_hash);
        }
    }
}
