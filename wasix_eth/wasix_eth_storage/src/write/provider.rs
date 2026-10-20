use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use std::collections::{HashSet, HashMap};
use redb::{Database, WriteTransaction, ReadableTable, ReadableDatabase};
use tokio::sync::{mpsc, oneshot};
use wasix_eth_types::{Address, Result, B256, U256, Block, BlockBody, Receipt, Transaction, TrieAccount, BlockId, BlockNumberOrTag, PayloadId, BlobsBundleV1, Bytes, Header, ReceiptMeta};
use wasix_eth_utils::error;

use crate::write::request::WriteRequest;
use crate::write::coordinator::WriteCoordinator;
use crate::write::batch::BatchWriter;
use crate::read::DatabaseReadProvider;
use crate::write_traits::{HeaderWriter, BlockWriter, TransactionWriter, AccountWriter, StorageWriter, BytecodeWriter, StateWriter, ChangeSetWriter, MetadataWriter, PeerDiscoveryWriter};
use crate::read_traits::{AccountProvider, StorageProvider, HeaderProvider, BytecodeProvider, StateProvider};
use crate::codecs::Table;
use crate::tables::*;
use wasix_eth_types::PeerEntry;

/// Implementation of database write providers using a `redb` database.
#[derive(Clone)]
pub struct DatabaseWriteProvider {
    pub(crate) db: Arc<Database>,
    pub(crate) touched_accounts: Arc<Mutex<HashSet<Address>>>,
    pub(crate) touched_storages: Arc<Mutex<HashMap<Address, HashSet<B256>>>>,
    pub(crate) tx: mpsc::UnboundedSender<WriteRequest>,
}

impl DatabaseWriteProvider {
    /// Creates a new `DatabaseWriteProvider` from a `redb` database.
    pub fn new(db: Arc<Database>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let provider = Self { 
            db: db.clone(),
            touched_accounts: Arc::new(Mutex::new(HashSet::new())),
            touched_storages: Arc::new(Mutex::new(HashMap::new())),
            tx,
        };
        
        let write_provider = provider.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let mut coordinator = WriteCoordinator::new(db, rx, write_provider);
                if let Err(e) = coordinator.run().await {
                    error!("WriteCoordinator failed: {}", e);
                }
            });
        });

        provider
    }

    pub fn request_tx(&self) -> mpsc::UnboundedSender<WriteRequest> {
        self.tx.clone()
    }

    pub async fn send_request(&self, req: WriteRequest, rx: oneshot::Receiver<Result<()>>) -> Result<()> {
        let tx = self.tx.clone();
        tx.send(req).map_err(|_| anyhow::anyhow!("WriteCoordinator channel closed"))?;
        rx.await.map_err(|_| anyhow::anyhow!("WriteCoordinator response channel closed"))?
    }

    /// Helper to execute a closure within a write transaction and commit it.
    #[allow(dead_code)]
    pub(crate) fn with_write<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&WriteTransaction) -> Result<R>,
    {
        let wtx = self.db.begin_write()?;
        let result = f(&wtx)?;
        wtx.commit()?;
        Ok(result)
    }

    /// Commits - no-op now as with_write handles it, but kept for compatibility if needed.
    pub fn commit(self) -> Result<()> {
        Ok(())
    }

    pub fn begin_batch(&self) -> Result<BatchWriter> {
        let rtx = self.db.begin_read()?;
        Ok(BatchWriter::new_read_only(rtx, DatabaseReadProvider::new(self.db.clone())))
    }

    pub fn begin_write_batch(&self) -> Result<BatchWriter> {
        let wtx = self.db.begin_write()?;
        Ok(BatchWriter::new(wtx, DatabaseReadProvider::new(self.db.clone())))
    }

    pub fn clear_tracking(&self) {
        // This is a no-op for DatabaseWriteProvider as it doesn't track state changes itself
        // only BatchWriter does.
        self.touched_accounts.lock().unwrap().clear();
        self.touched_storages.lock().unwrap().clear();
    }
}

#[async_trait]
impl HeaderWriter for DatabaseWriteProvider {
    async fn insert_header(&self, hash: B256, header: Header) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::InsertHeader { hash, header, resp: tx }, rx).await
    }

    async fn insert_block_hash(&self, hash: B256, number: u64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::InsertHeaderNumber { hash, number, resp: tx }, rx).await
    }

    async fn insert_header_td(&self, hash: B256, td: U256) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::InsertHeaderTD { hash, td, resp: tx }, rx).await
    }

    async fn insert_header_number(&self, hash: B256, number: u64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::InsertHeaderNumber { hash, number, resp: tx }, rx).await
    }
}

#[async_trait]
impl BlockWriter for DatabaseWriteProvider {
    async fn set_canonical(&self, number: u64, hash: B256) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::SetCanonical { number, hash, resp: tx }, rx).await
    }

    async fn remove_canonical(&self, number: u64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::RemoveCanonical { number, resp: tx }, rx).await
    }

    async fn insert_block_body(&self, hash: B256, number: u64, body: BlockBody<Transaction>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::InsertBlockBody { hash, number, body, resp: tx }, rx).await
    }

    async fn update_forkchoice(&self, head: B256, safe: Option<B256>, finalized: Option<B256>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::UpdateForkchoice { head, safe, finalized, resp: tx }, rx).await
    }

    async fn add_payload(&self, id: PayloadId, block: Block<Transaction>, receipts: Vec<Receipt>, metas: Vec<ReceiptMeta>, bundle: BlobsBundleV1) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::AddPayload { id, block, receipts, metas, bundle, resp: tx }, rx).await
    }

    async fn remove_payload_by_block_hash(&self, hash: B256) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::RemovePayloadByBlockHash { hash, resp: tx }, rx).await
    }

    async fn insert_block(&self, block: Block<Transaction>, receipts: Vec<Receipt>, metas: Vec<ReceiptMeta>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::InsertBlock { block, receipts, metas, resp: tx }, rx).await
    }
}

#[async_trait]
impl TransactionWriter for DatabaseWriteProvider {
    async fn insert_transaction(&self, hash: B256, tx: Transaction) -> Result<()> {
        let (otx, rx) = oneshot::channel();
        self.send_request(WriteRequest::InsertTransaction { hash, tx, resp: otx }, rx).await
    }

    async fn insert_receipt(&self, block_hash: B256, index: u64, receipt: Receipt) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::InsertReceipt { block_hash, index, receipt, resp: tx }, rx).await
    }

    async fn insert_receipt_meta(&self, block_hash: B256, index: u64, meta: ReceiptMeta) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::InsertReceiptMeta { block_hash, index, meta, resp: tx }, rx).await
    }

    async fn remove_receipt_meta(&self, block_hash: B256, index: u64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::RemoveReceiptMeta { block_hash, index, resp: tx }, rx).await
    }

    async fn insert_transaction_lookup(&self, hash: B256, block_hash: B256, index: u64) -> Result<()> {
        let (otx, rx) = oneshot::channel();
        self.send_request(WriteRequest::InsertTransactionLookup { hash, block_hash, index, resp: otx }, rx).await
    }
}

#[async_trait]
impl AccountProvider for DatabaseWriteProvider {
    async fn account(&self, address: Address, _state_root: Option<B256>) -> Result<Option<TrieAccount>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.account(address, None).await
    }

    async fn accounts(&self) -> Result<Vec<(Address, TrieAccount)>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.accounts().await
    }

    async fn addresses(&self) -> Result<Vec<Address>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.addresses().await
    }

    async fn transaction_count(&self, address: Address, block_id: BlockId, state_root: Option<B256>) -> Result<u64> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.transaction_count(address, block_id, state_root).await
    }
}

#[async_trait]
impl StorageProvider for DatabaseWriteProvider {
    async fn storage(&self, address: Address, slot: B256, state_root: Option<B256>) -> Result<U256> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.storage(address, slot, state_root).await
    }

    async fn account_storages(&self, address: Address, state_root: Option<B256>) -> Result<Vec<(B256, U256)>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.account_storages(address, state_root).await
    }
}

#[async_trait]
impl AccountWriter for DatabaseWriteProvider {
    async fn update_account(&self, address: Address, account: TrieAccount) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::UpdateAccount { address, account, resp: tx }, rx).await
    }

    async fn remove_account(&self, address: Address) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::RemoveAccount { address, resp: tx }, rx).await
    }

    async fn calculate_state_root(&self, is_eip161: bool, state_root: Option<B256>) -> Result<B256> {
        // Calculate root requires a full batch commit
        let batch = self.begin_batch()?;
        let root = batch.calculate_state_root(is_eip161, state_root).await?;
        batch.commit()?;
        Ok(root)
    }

    async fn calculate_storage_root(&self, address: Address, state_root: Option<B256>) -> Result<B256> {
        let batch = self.begin_batch()?;
        let root = batch.calculate_storage_root(address, state_root).await?;
        batch.commit()?;
        Ok(root)
    }

    fn clear_tracking(&self) {
        self.touched_accounts.lock().unwrap().clear();
        self.touched_storages.lock().unwrap().clear();
    }

    async fn mark_account_touched(&self, address: Address) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::MarkAccountTouched { address, resp: tx }, rx).await
    }
}

#[async_trait]
impl StorageWriter for DatabaseWriteProvider {
    async fn update_storage(&self, address: Address, slot: B256, value: U256) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::UpdateStorage { address, slot, value, resp: tx }, rx).await
    }

    async fn reset_storage(&self, address: Address) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::ResetStorage { address, resp: tx }, rx).await
    }
}

#[async_trait]
impl BytecodeProvider for DatabaseWriteProvider {
    async fn bytecode(&self, code_hash: B256) -> Result<Option<Bytes>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.bytecode(code_hash).await
    }
}

#[async_trait]
impl HeaderProvider for DatabaseWriteProvider {
    async fn header(&self, number: BlockId) -> anyhow::Result<Option<Header>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.header(number).await
    }
    async fn header_td(&self, hash: B256) -> anyhow::Result<Option<U256>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.header_td(hash).await
    }
    async fn latest_header_td(&self) -> anyhow::Result<Option<U256>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.latest_header_td().await
    }
}

#[async_trait]
impl StateProvider for DatabaseWriteProvider {
    async fn plain_state(&self, address: Address) -> Result<Option<Bytes>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.plain_state(address).await
    }

    async fn hashed_state(&self, hash: B256) -> Result<Option<Bytes>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.hashed_state(hash).await
    }

    async fn trie_node(&self, hash: B256) -> Result<Option<Bytes>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.trie_node(hash).await
    }
}

#[async_trait]
impl StateWriter for DatabaseWriteProvider {
    async fn update_plain_state(&self, address: Address, state: Bytes) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::UpdatePlainState { address, state, resp: tx }, rx).await
    }

    async fn remove_plain_state(&self, address: Address) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::RemovePlainState { address, resp: tx }, rx).await
    }

    async fn update_hashed_state(&self, hash: B256, state: Bytes) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::UpdateHashedState { hash, state, resp: tx }, rx).await
    }

    async fn update_trie_node(&self, hash: B256, node: Bytes) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::UpdateTrieNode { hash, node, resp: tx }, rx).await
    }
}

#[async_trait]
impl MetadataWriter for DatabaseWriteProvider {
    async fn set_metadata(&self, key: String, value: Bytes) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::SetMetadata { key, value, resp: tx }, rx).await
    }
}

#[async_trait]
impl PeerDiscoveryWriter for DatabaseWriteProvider {
    async fn register_peer(&self, peer: PeerEntry) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::RegisterPeer { peer, resp: tx }, rx).await
    }

    async fn remove_peer(&self, peer_id: String) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::RemovePeer { peer_id, resp: tx }, rx).await
    }
}

#[async_trait]
impl ChangeSetWriter for DatabaseWriteProvider {
    async fn insert_account_change_set(&self, number: u64, change_set: Vec<(Address, Option<Bytes>)>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::InsertAccountChangeSet { number, change_set, resp: tx }, rx).await
    }

    async fn insert_storage_change_set(&self, number: u64, change_set: Vec<(Address, B256, U256)>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::InsertStorageChangeSet { number, change_set, resp: tx }, rx).await
    }

    async fn remove_change_set(&self, number: u64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.send_request(WriteRequest::RemoveChangeSet { number, resp: tx }, rx).await
    }
}
