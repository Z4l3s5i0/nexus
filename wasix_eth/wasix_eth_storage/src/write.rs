use wasix_eth_types::{Address, Block, BlockBody, Bytes, Header, PayloadId, PeerEntry, Receipt, Result, Transaction, TrieAccount, B256, U256, BlockId, BlockNumberOrTag, BlobsBundleV1};
use wasix_eth_utils::debug;
use redb::{Database, WriteTransaction, ReadableTable, ReadableDatabase};
use crate::codecs::Table;
use crate::tables::*;
use std::collections::{HashSet, HashMap};
use std::sync::Mutex;
use std::sync::Arc;
use crate::write_traits::{AccountWriter, BlockWriter, BytecodeWriter, ChangeSetWriter, HeaderWriter, MetadataWriter, PeerDiscoveryWriter, StateWriter, StorageWriter, TransactionWriter};
use crate::read_traits::{AccountProvider, BytecodeProvider, StateProvider, StorageProvider, HeaderProvider};
use crate::read::DatabaseReadProvider;
use wasix_eth_types::ReceiptMeta;
use alloy_rlp::Decodable;

/// Implementation of database write providers using a `redb` database.
#[derive(Clone)]
pub struct DatabaseWriteProvider {
    db: Arc<Database>,
    touched_accounts: Arc<Mutex<HashSet<Address>>>,
    touched_storages: Arc<Mutex<HashMap<Address, HashSet<B256>>>>,
}

impl DatabaseWriteProvider {
    /// Creates a new `DatabaseWriteProvider` from a `redb` database.
    pub fn new(db: Arc<Database>) -> Self {
        Self { 
            db,
            touched_accounts: Arc::new(Mutex::new(HashSet::new())),
            touched_storages: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Helper to execute a closure within a write transaction and commit it.
    fn with_write<F, R>(&self, f: F) -> Result<R>
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
        let wtx = self.db.begin_write()?;
        Ok(BatchWriter::new(wtx, DatabaseReadProvider::new(self.db.clone())))
    }

    pub fn clear_tracking(&self) {
        // This is a no-op for DatabaseWriteProvider as it doesn't track state changes itself
        // only BatchWriter does.
    }
}

pub struct BatchWriter {
    wtx: WriteTransaction,
    read_provider: DatabaseReadProvider,
    base_state_root: Arc<Mutex<Option<B256>>>,
    touched_accounts: Arc<Mutex<HashSet<Address>>>,
    touched_storages: Arc<Mutex<HashMap<(Address, B256), U256>>>,
    resetted_storages: Arc<Mutex<HashSet<Address>>>,
    original_accounts: Arc<Mutex<HashMap<Address, Option<TrieAccount>>>>,
    original_storages: Arc<Mutex<HashMap<(Address, B256), U256>>>,
}

impl BatchWriter {
    pub fn new(wtx: WriteTransaction, read_provider: DatabaseReadProvider) -> Self {
        Self {
            wtx,
            read_provider,
            base_state_root: Arc::new(Mutex::new(None)),
            touched_accounts: Arc::new(Mutex::new(HashSet::new())),
            touched_storages: Arc::new(Mutex::new(HashMap::new())),
            resetted_storages: Arc::new(Mutex::new(HashSet::new())),
            original_accounts: Arc::new(Mutex::new(HashMap::new())),
            original_storages: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn set_base_state_root(&self, root: B256) {
        *self.base_state_root.lock().unwrap() = Some(root);
    }
}

use crate::trie::EthTrie;

impl BatchWriter {
    pub fn collect_account_changes(&self) -> Vec<(Address, Option<Bytes>)> {
        let originals = self.original_accounts.lock().unwrap();
        originals.iter().map(|(addr, acc)| {
            let bytes = acc.map(|a| {
                let mut buf = Vec::new();
                alloy_rlp::Encodable::encode(&a, &mut buf);
                Bytes::from(buf)
            });
            (*addr, bytes)
        }).collect()
    }

    pub fn collect_storage_changes(&self) -> Vec<(Address, B256, U256)> {
        let originals = self.original_storages.lock().unwrap();
        originals.iter().map(|((addr, slot), val)| {
            (*addr, *slot, *val)
        }).collect()
    }

    fn calculate_state_root_internal(&self, is_eip161: bool, state_root: Option<B256>) -> Result<B256> {
        let state_root = state_root.or(*self.base_state_root.lock().unwrap());
        let initial_root = match state_root {
            Some(r) => r,
            None => {
                self.read_provider.header(BlockId::Number(BlockNumberOrTag::Latest)).ok().flatten()
                    .map(|h| h.state_root)
                    .unwrap_or(alloy_trie::EMPTY_ROOT_HASH)
            }
        };
        
        debug!("[Trie] calculate_state_root_internal: initial_root={:?}", initial_root);
        let mut trie = EthTrie::new(self, initial_root);

        // 1. Load addresses that were modified in this batch
        let batch_addresses = {
            let touched = self.touched_accounts.lock().unwrap();
            touched.iter().cloned().collect::<Vec<_>>()
        };

        for addr in batch_addresses {
            let hashed_addr = alloy_primitives::keccak256(addr);

            if let Some(acc) = self.account(addr, state_root)? {
                let storage_root = self.calculate_storage_root_internal(addr, state_root)?;
                
                // EIP-161 check
                if is_eip161 && acc.nonce == 0 && acc.balance == U256::ZERO && (acc.code_hash == B256::default() || acc.code_hash == alloy_primitives::KECCAK256_EMPTY) {
                    if storage_root == alloy_trie::EMPTY_ROOT_HASH {
                        debug!("[Trie] Deleting empty account from trie: addr={:?}", addr);
                        trie.delete(hashed_addr)?;
                        // Also remove from Accounts table
                        let mut table = self.wtx.open_table(Accounts::definition())?;
                        table.remove(addr)?;
                        
                        // Sync PlainState and HashedState removal
                        let mut plain_table = self.wtx.open_table(PlainState::definition())?;
                        plain_table.remove(addr)?;
                        let mut hashed_table = self.wtx.open_table(HashedState::definition())?;
                        hashed_table.remove(hashed_addr)?;
                        continue;
                    }
                }
                
                let trie_acc = TrieAccount {
                    nonce: acc.nonce,
                    balance: acc.balance,
                    storage_root,
                    code_hash: if acc.code_hash == B256::default() { alloy_primitives::KECCAK256_EMPTY } else { acc.code_hash },
                };

                debug!("[Trie] Updating account in trie: addr={:?}, balance={}, nonce={}, storage={:?}, code={:?}", addr, trie_acc.balance, trie_acc.nonce, trie_acc.storage_root, trie_acc.code_hash);

                // Update Accounts table with new storage root
                let mut table = self.wtx.open_table(Accounts::definition())?;
                table.insert(addr, trie_acc)?;

                // Sync PlainState and HashedState
                let mut acc_rlp = Vec::new();
                alloy_rlp::Encodable::encode(&trie_acc, &mut acc_rlp);
                let acc_rlp_bytes = Bytes::from(acc_rlp);
                
                let mut plain_table = self.wtx.open_table(PlainState::definition())?;
                plain_table.insert(addr, acc_rlp_bytes.clone())?;
                let mut hashed_table = self.wtx.open_table(HashedState::definition())?;
                hashed_table.insert(hashed_addr, acc_rlp_bytes.clone())?;

                trie.insert(hashed_addr, acc_rlp_bytes.0.to_vec())?;
            } else {
                debug!("[Trie] Account not found for trie update: addr={:?}", addr);
                trie.delete(hashed_addr)?;
                // Remove from PlainState and HashedState as well
                let mut plain_table = self.wtx.open_table(PlainState::definition())?;
                plain_table.remove(addr)?;
                let mut hashed_table = self.wtx.open_table(HashedState::definition())?;
                hashed_table.remove(hashed_addr)?;
            }
        }

        let root = trie.root_hash();
        debug!("[Trie] calculate_state_root_internal: final_root={:?}", root);
        trie.commit()?;
        
        // Ensure ALL dirty nodes are flushed before returning the root.
        // EthTrie::commit flushes nodes to self.state (which is self/BatchWriter).
        // BatchWriter::update_trie_node inserts them into the wtx table.
        
        Ok(root)
    }

    fn calculate_storage_root_internal(&self, address: Address, state_root: Option<B256>) -> Result<B256> {
        let state_root = state_root.or(*self.base_state_root.lock().unwrap());
        let initial_storage_root = if self.resetted_storages.lock().unwrap().contains(&address) {
            alloy_trie::EMPTY_ROOT_HASH
        } else {
            self.account(address, state_root)?.map(|acc| acc.storage_root).unwrap_or(alloy_trie::EMPTY_ROOT_HASH)
        };
        let mut trie = EthTrie::new(self, initial_storage_root);

        // 1. Load storage slots modified in this batch for this address
        let batch_slots = {
            let touched = self.touched_storages.lock().unwrap();
            touched.keys()
                .filter(|(a, _)| *a == address)
                .map(|(_, s)| *s)
                .collect::<Vec<_>>()
        };

        for slot in batch_slots {
            let hashed_slot = alloy_primitives::keccak256(slot);
            if let Ok(value) = self.storage(address, slot, state_root) {
                if value == U256::ZERO {
                    trie.delete(hashed_slot)?;
                    
                    // Sync HashedState
                    let hashed_addr = alloy_primitives::keccak256(address);
                    let combined_key = alloy_primitives::keccak256([hashed_addr.0, hashed_slot.0].concat());
                    let mut table = self.wtx.open_table(HashedState::definition())?;
                    table.remove(combined_key)?;
                } else {
                    let mut buf = Vec::new();
                    alloy_rlp::Encodable::encode(&value, &mut buf);
                    
                    // Sync HashedState
                    let val_bytes = value.to_be_bytes::<32>();
                    let hashed_addr = alloy_primitives::keccak256(address);
                    let combined_key = alloy_primitives::keccak256([hashed_addr.0, hashed_slot.0].concat());
                    let mut table = self.wtx.open_table(HashedState::definition())?;
                    table.insert(combined_key, Bytes::from(val_bytes.to_vec()))?;

                    trie.insert(hashed_slot, buf)?;
                }
            }
        }

        let root = trie.root_hash();
        trie.commit()?;
        
        Ok(root)
    }

    pub fn commit(self) -> Result<()> {
        self.wtx.commit()?;
        Ok(())
    }

    pub fn clear_tracking(&self) {
        self.touched_accounts.lock().unwrap().clear();
        self.touched_storages.lock().unwrap().clear();
        self.resetted_storages.lock().unwrap().clear();
        self.original_accounts.lock().unwrap().clear();
        self.original_storages.lock().unwrap().clear();
    }

    pub fn mark_account_touched(&self, address: Address) -> Result<()> {
        self.touched_accounts.lock().unwrap().insert(address);
        Ok(())
    }
}

impl AccountProvider for BatchWriter {
    fn account(&self, address: Address, state_root: Option<B256>) -> Result<Option<TrieAccount>> {
        let state_root = state_root.or(*self.base_state_root.lock().unwrap());
        // 1. Check if modified in CURRENT block's batch
        if self.touched_accounts.lock().unwrap().contains(&address) {
            let table = self.wtx.open_table(Accounts::definition())?;
            if let Some(value) = table.get(address)? {
                return Ok(Some(value.value()));
            }
            return Ok(None);
        }

        // 2. If not modified in this block yet, read from the PARENT state root
        // If a state root is provided, we must use trie traversal that sees our wtx
        if let Some(root) = state_root {
            if root == alloy_trie::EMPTY_ROOT_HASH {
                return Ok(None);
            }
            let hashed_address = alloy_primitives::keccak256(address);
            let mut trie = EthTrie::new(self, root);
            let acc_bytes = trie.get_nibbles(alloy_trie::Nibbles::unpack(hashed_address))?;
            if let Some(bytes) = acc_bytes {
                let mut slice = &bytes[..];
                let acc = TrieAccount::decode(&mut slice)?;
                return Ok(Some(acc));
            }
            return Ok(None);
        }

        self.read_provider.account(address, state_root)
    }

    fn accounts(&self) -> Result<Vec<(Address, TrieAccount)>> {
        let table = self.wtx.open_table(Accounts::definition())?;
        let mut accounts = Vec::new();
        for entry in table.iter()? {
            let (key, value) = entry?;
            accounts.push((key.value(), value.value()));
        }
        Ok(accounts)
    }

    fn addresses(&self) -> Result<Vec<Address>> {
        let mut addresses = HashSet::new();
        
        // Add addresses from persistent DB
        for addr in self.read_provider.addresses()? {
            addresses.insert(addr);
        }
        
        // Add/update addresses from current batch
        let table = self.wtx.open_table(Accounts::definition())?;
        for entry in table.iter()? {
            let (key, _) = entry?;
            addresses.insert(key.value());
        }
        
        Ok(addresses.into_iter().collect())
    }

    fn transaction_count(&self, address: Address, _block_id: BlockId, state_root: Option<B256>) -> Result<u64> {
        let account = self.account(address, state_root)?;
        Ok(account.as_ref().map(|a| a.nonce).unwrap_or(0))
    }
}

impl StorageProvider for BatchWriter {
    fn storage(&self, address: Address, slot: B256, state_root: Option<B256>) -> Result<U256> {
        let state_root = state_root.or(*self.base_state_root.lock().unwrap());
        // 1. First check current batch for modifications
        if let Some(value) = self.touched_storages.lock().unwrap().get(&(address, slot)) {
            return Ok(*value);
        }

        if self.resetted_storages.lock().unwrap().contains(&address) {
            return Ok(U256::ZERO);
        }

        // 2. If not in current batch, do lookup in read_provider (latest or historical)
        // If a state root is provided, we must use trie traversal that sees our wtx
        if let Some(root) = state_root {
            if root == alloy_trie::EMPTY_ROOT_HASH {
                return Ok(U256::ZERO);
            }
            // First find the account to get its storage root
            if let Some(acc) = self.account(address, Some(root))? {
                if acc.storage_root == alloy_trie::EMPTY_ROOT_HASH {
                    return Ok(U256::ZERO);
                }
                let hashed_slot = alloy_primitives::keccak256(slot);
                let mut trie = EthTrie::new(self, acc.storage_root);
                let val_bytes = trie.get_nibbles(alloy_trie::Nibbles::unpack(hashed_slot))?;
                if let Some(bytes) = val_bytes {
                    let mut slice = &bytes[..];
                    let val: U256 = alloy_rlp::Decodable::decode(&mut slice)?;
                    return Ok(val);
                }
            }
            return Ok(U256::ZERO);
        }

        self.read_provider.storage(address, slot, state_root)
    }

    fn account_storages(&self, address: Address, state_root: Option<B256>) -> Result<Vec<(B256, U256)>> {
        let mut storages = HashMap::new();

        // 1. Add/update from persistent DB (latest or historical)
        for (slot, value) in self.read_provider.account_storages(address, state_root)? {
            storages.insert(slot, value);
        }

        // 2. Overlay from current batch
        // We ALWAYS need the overlay because SputnikVM applies changes to the batch, 
        // and then we calculate root.
        let table = self.wtx.open_table(Storages::definition())?;
        let start = (address, B256::ZERO);
        let end = (address, B256::repeat_byte(0xff));
        
        for entry in table.range(start..=end)? {
            let (key, value) = entry?;
            let (_, slot) = key.value();
            storages.insert(slot, value.value());
        }
        
        Ok(storages.into_iter().collect())
    }
}

impl BytecodeProvider for BatchWriter {
    fn bytecode(&self, code_hash: B256) -> Result<Option<Bytes>> {
        let table = self.wtx.open_table(Bytecodes::definition())?;
        if let Some(value) = table.get(code_hash)? {
            return Ok(Some(value.value()));
        }
        self.read_provider.bytecode(code_hash)
    }
}

impl HeaderWriter for BatchWriter {
    fn insert_header(&self, hash: B256, header: Header) -> Result<()> {
        let number = header.number;
        let mut h_table = self.wtx.open_table(Headers::definition())?;
        h_table.insert(hash, header)?;
        
        let mut hn_table = self.wtx.open_table(HeaderNumbers::definition())?;
        hn_table.insert(hash, number)?;
        Ok(())
    }

    fn insert_block_hash(&self, hash: B256, number: u64) -> Result<()> {
        let mut table = self.wtx.open_table(HeaderNumbers::definition())?;
        table.insert(hash, number)?;
        Ok(())
    }

    fn insert_header_td(&self, hash: B256, td: U256) -> Result<()> {
        let mut table = self.wtx.open_table(HeaderTD::definition())?;
        table.insert(hash, td)?;
        Ok(())
    }

    fn insert_header_number(&self, hash: B256, number: u64) -> Result<()> {
        let mut table = self.wtx.open_table(HeaderNumbers::definition())?;
        table.insert(hash, number)?;
        Ok(())
    }
}

impl BlockWriter for BatchWriter {
    fn set_canonical(&self, number: u64, hash: B256) -> Result<()> {
        let mut table = self.wtx.open_table(CanonicalHeads::definition())?;
        table.insert(number, hash)?;
        Ok(())
    }

    fn remove_canonical(&self, number: u64) -> Result<()> {
        let mut table = self.wtx.open_table(CanonicalHeads::definition())?;
        table.remove(number)?;
        Ok(())
    }

    fn insert_block_body(&self, hash: B256, _number: u64, body: BlockBody<Transaction>) -> Result<()> {
        let mut b_table = self.wtx.open_table(BlockBodies::definition())?;
        b_table.insert(hash, body.clone())?;

        let mut tx_table = self.wtx.open_table(Transactions::definition())?;
        let mut l_table = self.wtx.open_table(TransactionLookup::definition())?;
        
        for (index, tx) in body.transactions.iter().enumerate() {
            let tx_hash = *tx.hash();
            tx_table.insert(tx_hash, tx.clone())?;
            l_table.insert(tx_hash, (hash, index as u64))?;
        }
        Ok(())
    }

    fn update_forkchoice(&self, head: B256, safe: Option<B256>, finalized: Option<B256>) -> Result<()> {
        let mut table = self.wtx.open_table(Forkchoice::definition())?;
        table.insert("head".to_string(), head)?;
        if let Some(s) = safe {
            table.insert("safe".to_string(), s)?;
        }
        if let Some(f) = finalized {
            table.insert("finalized".to_string(), f)?;
        }
        Ok(())
    }

    fn add_payload(&self, id: PayloadId, block: Block<Transaction>, receipts: Vec<Receipt>, metas: Vec<ReceiptMeta>, bundle: BlobsBundleV1) -> Result<()> {
        let mut table = self.wtx.open_table(Payloads::definition())?;
        table.insert(id, (block, receipts, metas, bundle))?;
        Ok(())
    }

    fn remove_payload_by_block_hash(&self, hash: B256) -> Result<()> {
        let mut table = self.wtx.open_table(Payloads::definition())?;
        let mut to_remove = Vec::new();
        for entry in table.iter()? {
            let (id, value) = entry?;
            let (block, _, _, _): (Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, BlobsBundleV1) = value.value();
            if block.header.hash_slow() == hash {
                to_remove.push(id.value());
            }
        }
        for id in to_remove {
            table.remove(id)?;
        }
        Ok(())
    }

    fn insert_block(&self, block: Block<Transaction>, receipts: Vec<Receipt>, metas: Vec<ReceiptMeta>) -> Result<()> {
        let hash = block.header.hash_slow();
        let number = block.header.number;
        
        self.insert_header(hash, block.header.clone())?;
        self.insert_block_body(hash, number, block.body.clone())?;
        
        // Calculate TD
        let parent_td = if number == 0 {
            U256::ZERO
        } else {
            self.read_provider.header_td(block.header.parent_hash)?.unwrap_or(U256::ZERO)
        };
        let td = parent_td + block.header.difficulty;
        self.insert_header_td(hash, td)?;

        for (i, receipt) in receipts.into_iter().enumerate() {
            self.insert_receipt(hash, i as u64, receipt)?;
        }

        for (i, meta) in metas.into_iter().enumerate() {
            self.insert_receipt_meta(hash, i as u64, meta)?;
        }

        Ok(())
    }
}

impl TransactionWriter for BatchWriter {
    fn insert_transaction(&self, hash: B256, tx: Transaction) -> Result<()> {
        let mut table = self.wtx.open_table(Transactions::definition())?;
        table.insert(hash, tx)?;
        Ok(())
    }

    fn insert_receipt(&self, block_hash: B256, index: u64, receipt: Receipt) -> Result<()> {
        let mut table = self.wtx.open_table(Receipts::definition())?;
        table.insert((block_hash, index), receipt)?;
        Ok(())
    }

    fn insert_receipt_meta(&self, block_hash: B256, index: u64, meta: ReceiptMeta) -> Result<()> {
        let mut table = self.wtx.open_table(ReceiptsMeta::definition())?;
        table.insert((block_hash, index), meta)?;
        Ok(())
    }

    fn remove_receipt_meta(&self, block_hash: B256, index: u64) -> Result<()> {
        let mut table = self.wtx.open_table(ReceiptsMeta::definition())?;
        table.remove((block_hash, index))?;
        Ok(())
    }

    fn insert_transaction_lookup(&self, hash: B256, block_hash: B256, index: u64) -> Result<()> {
        let mut table = self.wtx.open_table(TransactionLookup::definition())?;
        table.insert(hash, (block_hash, index))?;
        Ok(())
    }
}

impl AccountWriter for BatchWriter {
    fn update_account(&self, address: Address, account: TrieAccount) -> Result<()> {
        {
            let mut originals = self.original_accounts.lock().unwrap();
            if !originals.contains_key(&address) {
                let old = self.account(address, None)?;
                originals.insert(address, old);
            }
        }
        let mut table = self.wtx.open_table(Accounts::definition())?;
        table.insert(address, account)?;
        self.touched_accounts.lock().unwrap().insert(address);
        Ok(())
    }

    fn remove_account(&self, address: Address) -> Result<()> {
        {
            let mut originals = self.original_accounts.lock().unwrap();
            if !originals.contains_key(&address) {
                let old = self.account(address, None)?;
                originals.insert(address, old);
            }
        }
        let mut table = self.wtx.open_table(Accounts::definition())?;
        table.remove(address)?;
        self.touched_accounts.lock().unwrap().insert(address);
        Ok(())
    }

    fn calculate_state_root(&self, is_eip161: bool, state_root: Option<B256>) -> Result<B256> {
        self.calculate_state_root_internal(is_eip161, state_root)
    }

    fn calculate_storage_root(&self, address: Address, state_root: Option<B256>) -> Result<B256> {
        self.calculate_storage_root_internal(address, state_root)
    }

    fn clear_tracking(&self) {
        self.touched_accounts.lock().unwrap().clear();
        self.touched_storages.lock().unwrap().clear();
        self.resetted_storages.lock().unwrap().clear();
        self.original_accounts.lock().unwrap().clear();
        self.original_storages.lock().unwrap().clear();
    }
}

impl StorageWriter for BatchWriter {
    fn update_storage(&self, address: Address, slot: B256, value: U256) -> Result<()> {
        {
            let mut originals = self.original_storages.lock().unwrap();
            if !originals.contains_key(&(address, slot)) {
                let old = self.storage(address, slot, None)?;
                originals.insert((address, slot), old);
            }
        }
        let mut table = self.wtx.open_table(Storages::definition())?;
        if value == U256::ZERO {
            let _ = table.remove((address, slot))?;
        } else {
            table.insert((address, slot), value)?;
        }
        self.touched_storages.lock().unwrap().insert((address, slot), value);
        
        // Ensure account is marked as touched so its state root is updated
        if !self.touched_accounts.lock().unwrap().contains(&address) {
            let acc = self.account(address, None)?;
            let acc = acc.unwrap_or_else(|| TrieAccount {
                nonce: 0,
                balance: U256::ZERO,
                storage_root: alloy_trie::EMPTY_ROOT_HASH,
                code_hash: alloy_primitives::KECCAK256_EMPTY,
            });
            self.update_account(address, acc)?;
        }
        
        Ok(())
    }

    fn reset_storage(&self, address: Address) -> Result<()> {
        self.resetted_storages.lock().unwrap().insert(address);
        Ok(())
    }
}

impl BytecodeWriter for BatchWriter {
    fn insert_bytecode(&self, code_hash: B256, bytecode: Bytes) -> Result<()> {
        let mut table = self.wtx.open_table(Bytecodes::definition())?;
        table.insert(code_hash, bytecode)?;
        Ok(())
    }
}

impl StateProvider for BatchWriter {
    fn plain_state(&self, address: Address) -> anyhow::Result<Option<Bytes>> {
        let table = self.wtx.open_table(PlainState::definition())?;
        if let Some(value) = table.get(address)? {
            return Ok(Some(value.value()));
        }
        self.read_provider.plain_state(address)
    }

    fn hashed_state(&self, hash: B256) -> anyhow::Result<Option<Bytes>> {
        let table = self.wtx.open_table(HashedState::definition())?;
        if let Some(value) = table.get(hash)? {
            return Ok(Some(value.value()));
        }
        self.read_provider.hashed_state(hash)
    }

    fn trie_node(&self, hash: B256) -> anyhow::Result<Option<Bytes>> {
        let table = self.wtx.open_table(TrieNodes::definition())?;
        if let Some(value) = table.get(hash)? {
            return Ok(Some(value.value()));
        }
        self.read_provider.trie_node(hash)
    }
}

impl StateWriter for BatchWriter {
    fn update_plain_state(&self, address: Address, state: Bytes) -> Result<()> {
        let mut table = self.wtx.open_table(PlainState::definition())?;
        table.insert(address, state)?;
        self.touched_accounts.lock().unwrap().insert(address);
        Ok(())
    }

    fn remove_plain_state(&self, address: Address) -> Result<()> {
        let mut table = self.wtx.open_table(PlainState::definition())?;
        table.remove(address)?;
        self.touched_accounts.lock().unwrap().insert(address);
        Ok(())
    }

    fn update_hashed_state(&self, hash: B256, state: Bytes) -> Result<()> {
        let mut table = self.wtx.open_table(HashedState::definition())?;
        table.insert(hash, state)?;
        Ok(())
    }

    fn update_trie_node(&self, hash: B256, node: Bytes) -> Result<()> {
        let mut table = self.wtx.open_table(TrieNodes::definition())?;
        table.insert(hash, node.clone())?;
        Ok(())
    }
}

impl ChangeSetWriter for BatchWriter {
    fn insert_account_change_set(&self, number: u64, change_set: Vec<(Address, Option<Bytes>)>) -> Result<()> {
        let mut table = self.wtx.open_table(AccountChangeSets::definition())?;
        table.insert(number, change_set)?;
        Ok(())
    }

    fn insert_storage_change_set(&self, number: u64, change_set: Vec<(Address, B256, U256)>) -> Result<()> {
        let mut table = self.wtx.open_table(StorageChangeSets::definition())?;
        table.insert(number, change_set)?;
        Ok(())
    }

    fn remove_change_set(&self, number: u64) -> Result<()> {
        let mut acs_table = self.wtx.open_table(AccountChangeSets::definition())?;
        acs_table.remove(number)?;
        let mut scs_table = self.wtx.open_table(StorageChangeSets::definition())?;
        scs_table.remove(number)?;
        Ok(())
    }
}

impl MetadataWriter for BatchWriter {
    fn set_metadata(&self, key: String, value: Bytes) -> Result<()> {
        let mut table = self.wtx.open_table(Metadata::definition())?;
        table.insert(key, value)?;
        Ok(())
    }
}

impl HeaderWriter for DatabaseWriteProvider {
    fn insert_header(&self, hash: B256, header: Header) -> Result<()> {
        let number = header.number;
        self.with_write(|wtx| {
            let mut h_table = wtx.open_table(Headers::definition())?;
            h_table.insert(hash, header)?;
            
            let mut hn_table = wtx.open_table(HeaderNumbers::definition())?;
            hn_table.insert(hash, number)?;
            Ok(())
        })
    }

    fn insert_block_hash(&self, hash: B256, number: u64) -> Result<()> {
        // Redundant with insert_header now, but keeping for compatibility
        self.with_write(|wtx| {
            let mut table = wtx.open_table(HeaderNumbers::definition())?;
            table.insert(hash, number)?;
            Ok(())
        })
    }

    fn insert_header_td(&self, hash: B256, td: U256) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(HeaderTD::definition())?;
            table.insert(hash, td)?;
            Ok(())
        })
    }

    fn insert_header_number(&self, hash: B256, number: u64) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(HeaderNumbers::definition())?;
            table.insert(hash, number)?;
            Ok(())
        })
    }
}

impl BlockWriter for DatabaseWriteProvider {
    fn set_canonical(&self, number: u64, hash: B256) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(CanonicalHeads::definition())?;
            table.insert(number, hash)?;
            Ok(())
        })
    }

    fn remove_canonical(&self, number: u64) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(CanonicalHeads::definition())?;
            table.remove(number)?;
            Ok(())
        })
    }

    fn insert_block_body(&self, hash: B256, _number: u64, body: BlockBody<Transaction>) -> Result<()> {
        self.with_write(|wtx| {
            let mut b_table = wtx.open_table(BlockBodies::definition())?;
            b_table.insert(hash, body.clone())?;

            let mut tx_table = wtx.open_table(Transactions::definition())?;
            let mut l_table = wtx.open_table(TransactionLookup::definition())?;
            
            for (index, tx) in body.transactions.iter().enumerate() {
                let tx_hash = *tx.hash();
                tx_table.insert(tx_hash, tx.clone())?;
                l_table.insert(tx_hash, (hash, index as u64))?;
            }
            Ok(())
        })
    }

    fn update_forkchoice(&self, head: B256, safe: Option<B256>, finalized: Option<B256>) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(Forkchoice::definition())?;
            table.insert("head".to_string(), head)?;
            if let Some(s) = safe {
                table.insert("safe".to_string(), s)?;
            }
            if let Some(f) = finalized {
                table.insert("finalized".to_string(), f)?;
            }
            Ok(())
        })
    }

    fn add_payload(&self, id: PayloadId, block: Block<Transaction>, receipts: Vec<Receipt>, metas: Vec<ReceiptMeta>, bundle: BlobsBundleV1) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(Payloads::definition())?;
            table.insert(id, (block, receipts, metas, bundle))?;
            Ok(())
        })
    }

    fn remove_payload_by_block_hash(&self, hash: B256) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(Payloads::definition())?;
            let mut to_remove = Vec::new();
            for entry in table.iter()? {
                let (id, value) = entry?;
                let (block, _, _, _): (Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, BlobsBundleV1) = value.value();
                if block.header.hash_slow() == hash {
                    to_remove.push(id.value());
                }
            }
            for id in to_remove {
                table.remove(id)?;
            }
            Ok(())
        })
    }

    fn insert_block(&self, block: Block<Transaction>, receipts: Vec<Receipt>, metas: Vec<ReceiptMeta>) -> Result<()> {
        let batch = self.begin_batch()?;
        batch.insert_block(block, receipts, metas)?;
        batch.commit()?;
        Ok(())
    }
}

impl TransactionWriter for DatabaseWriteProvider {
    fn insert_transaction(&self, hash: B256, tx: Transaction) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(Transactions::definition())?;
            table.insert(hash, tx)?;
            Ok(())
        })
    }

    fn insert_receipt(&self, block_hash: B256, index: u64, receipt: Receipt) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(Receipts::definition())?;
            table.insert((block_hash, index), receipt)?;
            Ok(())
        })
    }

    fn insert_receipt_meta(&self, block_hash: B256, index: u64, meta: ReceiptMeta) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(ReceiptsMeta::definition())?;
            table.insert((block_hash, index), meta)?;
            Ok(())
        })
    }

    fn remove_receipt_meta(&self, block_hash: B256, index: u64) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(ReceiptsMeta::definition())?;
            table.remove((block_hash, index))?;
            Ok(())
        })
    }

    fn insert_transaction_lookup(&self, hash: B256, block_hash: B256, index: u64) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(TransactionLookup::definition())?;
            table.insert(hash, (block_hash, index))?;
            Ok(())
        })
    }
}

impl AccountProvider for DatabaseWriteProvider {
    fn account(&self, address: Address, _state_root: Option<B256>) -> Result<Option<TrieAccount>> {
        let read_provider = DatabaseReadProvider::new(self.db.clone());
        read_provider.account(address, None)
    }

    fn accounts(&self) -> Result<Vec<(Address, TrieAccount)>> {
        let read_provider = DatabaseReadProvider::new(self.db.clone());
        read_provider.accounts()
    }

    fn addresses(&self) -> Result<Vec<Address>> {
        let read_provider = DatabaseReadProvider::new(self.db.clone());
        read_provider.addresses()
    }

    fn transaction_count(&self, address: Address, block_id: BlockId, state_root: Option<B256>) -> Result<u64> {
        let read_provider = DatabaseReadProvider::new(self.db.clone());
        read_provider.transaction_count(address, block_id, state_root)
    }
}

impl StorageProvider for DatabaseWriteProvider {
    fn storage(&self, address: Address, slot: B256, state_root: Option<B256>) -> Result<U256> {
        let read_provider = DatabaseReadProvider::new(self.db.clone());
        read_provider.storage(address, slot, state_root)
    }

    fn account_storages(&self, address: Address, state_root: Option<B256>) -> Result<Vec<(B256, U256)>> {
        let read_provider = DatabaseReadProvider::new(self.db.clone());
        read_provider.account_storages(address, state_root)
    }
}

impl AccountWriter for DatabaseWriteProvider {
    fn update_account(&self, address: Address, account: TrieAccount) -> Result<()> {
        self.with_write(|wtx| {
            let mut a_table = wtx.open_table(Accounts::definition())?;
            a_table.insert(address, account.clone())?;
            self.touched_accounts.lock().unwrap().insert(address);
            Ok(())
        })
    }

    fn remove_account(&self, address: Address) -> Result<()> {
        self.with_write(|wtx| {
            let mut a_table = wtx.open_table(Accounts::definition())?;
            a_table.remove(address)?;
            self.touched_accounts.lock().unwrap().insert(address);
            Ok(())
        })
    }

    fn calculate_state_root(&self, is_eip161: bool, state_root: Option<B256>) -> Result<B256> {
        let read_provider = DatabaseReadProvider::new(self.db.clone());
        let initial_root = match state_root {
            Some(r) => r,
            None => {
                read_provider.header(BlockId::Number(BlockNumberOrTag::Latest)).ok().flatten()
                    .map(|h| h.state_root)
                    .unwrap_or(alloy_trie::EMPTY_ROOT_HASH)
            }
        };

        let mut trie = EthTrie::new(self, initial_root);

        // 1. Load addresses that were modified in this provider's lifetime
        let batch_addresses = {
            let touched = self.touched_accounts.lock().unwrap();
            touched.iter().cloned().collect::<Vec<_>>()
        };

        for addr in batch_addresses {
            let hashed_addr = alloy_primitives::keccak256(addr);
            
            if let Some(acc) = self.account(addr, state_root)? {
                let storage_root = self.calculate_storage_root(addr, state_root)?;
                
                // EIP-161 check
                if is_eip161 && acc.nonce == 0 && acc.balance == U256::ZERO && (acc.code_hash == B256::default() || acc.code_hash == alloy_primitives::KECCAK256_EMPTY) {
                    if storage_root == alloy_trie::EMPTY_ROOT_HASH {
                        trie.delete(hashed_addr)?;
                        // Also remove from Accounts table
                        self.with_write(|wtx| {
                            let mut a_table = wtx.open_table(Accounts::definition())?;
                            a_table.remove(addr)?;
                            Ok(())
                        })?;
                        continue;
                    }
                }
                
                let trie_acc = TrieAccount {
                    nonce: acc.nonce,
                    balance: acc.balance,
                    storage_root,
                    code_hash: if acc.code_hash == B256::default() { alloy_primitives::KECCAK256_EMPTY } else { acc.code_hash },
                };

                // Update Accounts table with new storage root
                self.with_write(|wtx| {
                    let mut a_table = wtx.open_table(Accounts::definition())?;
                    a_table.insert(addr, trie_acc.clone())?;
                    Ok(())
                })?;

                let mut buf = Vec::new();
                alloy_rlp::Encodable::encode(&trie_acc, &mut buf);
                trie.insert(hashed_addr, buf)?;
            } else {
                trie.delete(hashed_addr)?;
            }
        }

        let root = trie.root_hash();
        trie.commit()?;
        
        Ok(root)
    }

    fn calculate_storage_root(&self, address: Address, state_root: Option<B256>) -> Result<B256> {
        let initial_storage_root = self.account(address, state_root)?.map(|acc| acc.storage_root).unwrap_or(alloy_trie::EMPTY_ROOT_HASH);
        let mut trie = EthTrie::new(self, initial_storage_root);

        // 1. Load storage slots modified in this provider's lifetime for this address
        let batch_slots = {
            let touched = self.touched_storages.lock().unwrap();
            touched.get(&address).map(|slots| slots.iter().cloned().collect::<Vec<_>>()).unwrap_or_default()
        };

        for slot in batch_slots {
            let hashed_slot = alloy_primitives::keccak256(slot);
            if let Ok(value) = self.storage(address, slot, state_root) {
                if value == U256::ZERO {
                    trie.delete(hashed_slot)?;
                } else {
                    let mut buf = Vec::new();
                    alloy_rlp::Encodable::encode(&value, &mut buf);
                    trie.insert(hashed_slot, buf)?;
                }
            }
        }

        let root = trie.root_hash();
        trie.commit()?;
        
        Ok(root)
    }

    fn clear_tracking(&self) {
        self.touched_accounts.lock().unwrap().clear();
        self.touched_storages.lock().unwrap().clear();
    }
}

impl StorageWriter for DatabaseWriteProvider {
    fn update_storage(
        &self,
        address: Address,
        slot: B256,
        value: U256,
    ) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(Storages::definition())?;
            if value == U256::ZERO {
                let _ = table.remove((address, slot))?;
            } else {
                table.insert((address, slot), value)?;
            }
            let mut touched = self.touched_storages.lock().unwrap();
            touched.entry(address).or_default().insert(slot);
            self.touched_accounts.lock().unwrap().insert(address);
            Ok(())
        })
    }

    fn reset_storage(&self, address: Address) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(Storages::definition())?;
            let start = (address, B256::ZERO);
            let end = (address, B256::repeat_byte(0xff));
            let keys: Vec<_> = table.range(start..=end)?
                .filter_map(|res| res.ok().map(|(k, _)| k.value()))
                .collect();
            for key in keys {
                table.remove(key)?;
                let mut touched = self.touched_storages.lock().unwrap();
                touched.entry(address).or_default().insert(key.1);
            }
            self.touched_accounts.lock().unwrap().insert(address);
            Ok(())
        })
    }
}

impl BytecodeWriter for DatabaseWriteProvider {
    fn insert_bytecode(&self, code_hash: B256, bytecode: Bytes) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(Bytecodes::definition())?;
            table.insert(code_hash, bytecode)?;
            Ok(())
        })
    }
}

impl StateProvider for DatabaseWriteProvider {
    fn plain_state(&self, address: Address) -> anyhow::Result<Option<Bytes>> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(PlainState::definition())?;
        let value = table.get(address)?;
        Ok(value.map(|v| v.value()))
    }

    fn hashed_state(&self, hash: B256) -> anyhow::Result<Option<Bytes>> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(HashedState::definition())?;
        let value = table.get(hash)?;
        Ok(value.map(|v| v.value()))
    }

    fn trie_node(&self, hash: B256) -> anyhow::Result<Option<Bytes>> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(TrieNodes::definition())?;
        let value = table.get(hash)?;
        Ok(value.map(|v| v.value()))
    }
}

impl StateWriter for DatabaseWriteProvider {
    fn update_plain_state(&self, address: Address, state: Bytes) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(PlainState::definition())?;
            table.insert(address, state)?;
            Ok(())
        })
    }

    fn remove_plain_state(&self, address: Address) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(PlainState::definition())?;
            table.remove(address)?;
            Ok(())
        })
    }

    fn update_hashed_state(&self, hash: B256, state: Bytes) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(HashedState::definition())?;
            table.insert(hash, state)?;
            Ok(())
        })
    }

    fn update_trie_node(&self, hash: B256, node: Bytes) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(TrieNodes::definition())?;
            table.insert(hash, node)?;
            Ok(())
        })
    }
}

impl MetadataWriter for DatabaseWriteProvider {
    fn set_metadata(&self, key: String, value: Bytes) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(Metadata::definition())?;
            table.insert(key, value)?;
            Ok(())
        })
    }
}

impl PeerDiscoveryWriter for DatabaseWriteProvider {
    fn register_peer(&self, peer: PeerEntry) -> Result<()> {
        self.with_write(|tx| {
            let mut table = tx.open_table(ActivePeers::definition())?;
            table.insert(peer.peer_id.clone(), peer)?;
            Ok(())
        })
    }

    fn remove_peer(&self, peer_id: String) -> Result<()> {
        self.with_write(|tx| {
            let mut table = tx.open_table(ActivePeers::definition())?;
            table.remove(peer_id)?;
            Ok(())
        })
    }
}

impl ChangeSetWriter for DatabaseWriteProvider {
    fn insert_account_change_set(&self, number: u64, change_set: Vec<(Address, Option<Bytes>)>) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(AccountChangeSets::definition())?;
            table.insert(number, change_set)?;
            Ok(())
        })
    }

    fn insert_storage_change_set(&self, number: u64, change_set: Vec<(Address, B256, U256)>) -> Result<()> {
        self.with_write(|wtx| {
            let mut table = wtx.open_table(StorageChangeSets::definition())?;
            table.insert(number, change_set)?;
            Ok(())
        })
    }

    fn remove_change_set(&self, number: u64) -> Result<()> {
        self.with_write(|wtx| {
            let mut a_table = wtx.open_table(AccountChangeSets::definition())?;
            a_table.remove(number)?;
            let mut s_table = wtx.open_table(StorageChangeSets::definition())?;
            s_table.remove(number)?;
            Ok(())
        })
    }
}
