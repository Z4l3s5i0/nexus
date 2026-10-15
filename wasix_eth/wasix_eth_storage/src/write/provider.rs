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

impl HeaderWriter for DatabaseWriteProvider {
    fn insert_header(&self, hash: B256, header: Header) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::InsertHeader { hash, header, resp: tx }, rx))
    }

    fn insert_block_hash(&self, hash: B256, number: u64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::InsertHeaderNumber { hash, number, resp: tx }, rx))
    }

    fn insert_header_td(&self, hash: B256, td: U256) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::InsertHeaderTD { hash, td, resp: tx }, rx))
    }

    fn insert_header_number(&self, hash: B256, number: u64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::InsertHeaderNumber { hash, number, resp: tx }, rx))
    }
}

impl BlockWriter for DatabaseWriteProvider {
    fn set_canonical(&self, number: u64, hash: B256) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::SetCanonical { number, hash, resp: tx }, rx))
    }

    fn remove_canonical(&self, number: u64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::RemoveCanonical { number, resp: tx }, rx))
    }

    fn insert_block_body(&self, hash: B256, number: u64, body: BlockBody<Transaction>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::InsertBlockBody { hash, number, body, resp: tx }, rx))
    }

    fn update_forkchoice(&self, head: B256, safe: Option<B256>, finalized: Option<B256>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::UpdateForkchoice { head, safe, finalized, resp: tx }, rx))
    }

    fn add_payload(&self, id: PayloadId, block: Block<Transaction>, receipts: Vec<Receipt>, metas: Vec<ReceiptMeta>, bundle: BlobsBundleV1) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::AddPayload { id, block, receipts, metas, bundle, resp: tx }, rx))
    }

    fn remove_payload_by_block_hash(&self, hash: B256) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::RemovePayloadByBlockHash { hash, resp: tx }, rx))
    }

    fn insert_block(&self, block: Block<Transaction>, receipts: Vec<Receipt>, metas: Vec<ReceiptMeta>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::InsertBlock { block, receipts, metas, resp: tx }, rx))
    }
}

impl TransactionWriter for DatabaseWriteProvider {
    fn insert_transaction(&self, hash: B256, tx: Transaction) -> Result<()> {
        let (otx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::InsertTransaction { hash, tx, resp: otx }, rx))
    }

    fn insert_receipt(&self, block_hash: B256, index: u64, receipt: Receipt) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::InsertReceipt { block_hash, index, receipt, resp: tx }, rx))
    }

    fn insert_receipt_meta(&self, block_hash: B256, index: u64, meta: ReceiptMeta) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::InsertReceiptMeta { block_hash, index, meta, resp: tx }, rx))
    }

    fn remove_receipt_meta(&self, block_hash: B256, index: u64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::RemoveReceiptMeta { block_hash, index, resp: tx }, rx))
    }

    fn insert_transaction_lookup(&self, hash: B256, block_hash: B256, index: u64) -> Result<()> {
        let (otx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::InsertTransactionLookup { hash, block_hash, index, resp: otx }, rx))
    }
}

impl AccountProvider for DatabaseWriteProvider {
    fn account(&self, address: Address, _state_root: Option<B256>) -> Result<Option<TrieAccount>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.account(address, None)
    }

    fn accounts(&self) -> Result<Vec<(Address, TrieAccount)>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.accounts()
    }

    fn addresses(&self) -> Result<Vec<Address>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.addresses()
    }

    fn transaction_count(&self, address: Address, block_id: BlockId, state_root: Option<B256>) -> Result<u64> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.transaction_count(address, block_id, state_root)
    }
}

impl StorageProvider for DatabaseWriteProvider {
    fn storage(&self, address: Address, slot: B256, state_root: Option<B256>) -> Result<U256> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.storage(address, slot, state_root)
    }

    fn account_storages(&self, address: Address, state_root: Option<B256>) -> Result<Vec<(B256, U256)>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.account_storages(address, state_root)
    }
}

impl AccountWriter for DatabaseWriteProvider {
    fn update_account(&self, address: Address, account: TrieAccount) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::UpdateAccount { address, account, resp: tx }, rx))
    }

    fn remove_account(&self, address: Address) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::RemoveAccount { address, resp: tx }, rx))
    }

    fn calculate_state_root(&self, is_eip161: bool, state_root: Option<B256>) -> Result<B256> {
        // Calculate root requires a full batch commit
        let batch = self.begin_batch()?;
        let root = batch.calculate_state_root(is_eip161, state_root)?;
        batch.commit()?;
        Ok(root)
    }

    fn calculate_storage_root(&self, address: Address, state_root: Option<B256>) -> Result<B256> {
        let batch = self.begin_batch()?;
        let root = batch.calculate_storage_root(address, state_root)?;
        batch.commit()?;
        Ok(root)
    }

    fn clear_tracking(&self) {
        self.touched_accounts.lock().unwrap().clear();
        self.touched_storages.lock().unwrap().clear();
    }
}

impl StorageWriter for DatabaseWriteProvider {
    fn update_storage(&self, address: Address, slot: B256, value: U256) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::UpdateStorage { address, slot, value, resp: tx }, rx))
    }

    fn reset_storage(&self, address: Address) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::ResetStorage { address, resp: tx }, rx))
    }
}

impl BytecodeWriter for DatabaseWriteProvider {
    fn insert_bytecode(&self, code_hash: B256, bytecode: Bytes) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::InsertBytecode { code_hash, bytecode, resp: tx }, rx))
    }
}

impl StateProvider for DatabaseWriteProvider {
    fn plain_state(&self, address: Address) -> Result<Option<Bytes>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.plain_state(address)
    }

    fn hashed_state(&self, hash: B256) -> Result<Option<Bytes>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.hashed_state(hash)
    }

    fn trie_node(&self, hash: B256) -> Result<Option<Bytes>> {
        let read = DatabaseReadProvider::new(self.db.clone());
        read.trie_node(hash)
    }
}

impl StateWriter for DatabaseWriteProvider {
    fn update_plain_state(&self, address: Address, state: Bytes) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::UpdatePlainState { address, state, resp: tx }, rx))
    }

    fn remove_plain_state(&self, address: Address) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::RemovePlainState { address, resp: tx }, rx))
    }

    fn update_hashed_state(&self, hash: B256, state: Bytes) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::UpdateHashedState { hash, state, resp: tx }, rx))
    }

    fn update_trie_node(&self, hash: B256, node: Bytes) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::UpdateTrieNode { hash, node, resp: tx }, rx))
    }
}

impl MetadataWriter for DatabaseWriteProvider {
    fn set_metadata(&self, key: String, value: Bytes) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::SetMetadata { key, value, resp: tx }, rx))
    }
}

impl PeerDiscoveryWriter for DatabaseWriteProvider {
    fn register_peer(&self, peer: PeerEntry) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::RegisterPeer { peer, resp: tx }, rx))
    }

    fn remove_peer(&self, peer_id: String) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::RemovePeer { peer_id, resp: tx }, rx))
    }
}

impl ChangeSetWriter for DatabaseWriteProvider {
    fn insert_account_change_set(&self, number: u64, change_set: Vec<(Address, Option<Bytes>)>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::InsertAccountChangeSet { number, change_set, resp: tx }, rx))
    }

    fn insert_storage_change_set(&self, number: u64, change_set: Vec<(Address, B256, U256)>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::InsertStorageChangeSet { number, change_set, resp: tx }, rx))
    }

    fn remove_change_set(&self, number: u64) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(self.send_request(WriteRequest::RemoveChangeSet { number, resp: tx }, rx))
    }
}
