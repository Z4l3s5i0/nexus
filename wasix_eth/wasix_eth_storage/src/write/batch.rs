use crate::codecs::Table;
use crate::read::DatabaseReadProvider;
use crate::read_traits::{AccountProvider, BytecodeProvider, HeaderProvider, StateProvider, StorageProvider};
use crate::tables::*;
use crate::trie::EthTrie;
use crate::write_traits::{AccountWriter, BlockWriter, BytecodeWriter, ChangeSetWriter, HeaderWriter, MetadataWriter, PeerDiscoveryWriter, StateWriter, StorageWriter, TransactionWriter};
use async_trait::async_trait;
use dashmap::{DashMap, DashSet};
use arc_swap::ArcSwapOption;
use redb::{ReadTransaction, ReadableTable, WriteTransaction};
use std::sync::Arc;
use std::sync::Mutex;
use wasix_eth_types::ReceiptMeta;
use wasix_eth_types::{Address, BlobsBundleV1, Block, BlockBody, BlockId, BlockNumberOrTag, Bytes, Header, PayloadId, PeerEntry, Receipt, Result, Transaction, TrieAccount, B256, U256};
use wasix_eth_utils::debug;

use crate::{AccountProviderSync, BytecodeProviderSync, StateProviderSync, StorageProviderSync};

pub enum BatchTransaction {
    Write(WriteTransaction),
    Read(ReadTransaction),
}

impl BatchTransaction {
    pub fn commit(self) -> Result<()> {
        match self {
            BatchTransaction::Write(wtx) => {
                wtx.commit()?;
                Ok(())
            }
            BatchTransaction::Read(_) => Ok(()),
        }
    }
}

pub struct BatchWriter {
    pub(crate) tx: BatchTransaction,
    pub(crate) read_provider: DatabaseReadProvider,
    pub(crate) base_state_root: ArcSwapOption<B256>,
    pub(crate) touched_accounts: DashSet<Address>,
    pub(crate) touched_storages: DashMap<(Address, B256), U256>,
    pub(crate) resetted_storages: DashSet<Address>,
    pub(crate) original_accounts: DashMap<Address, Option<TrieAccount>>,
    pub(crate) original_storages: DashMap<(Address, B256), U256>,
}

impl BatchWriter {
    pub fn new(wtx: WriteTransaction, read_provider: DatabaseReadProvider) -> Self {
        Self {
            tx: BatchTransaction::Write(wtx),
            read_provider,
            base_state_root: ArcSwapOption::empty(),
            touched_accounts: DashSet::new(),
            touched_storages: DashMap::new(),
            resetted_storages: DashSet::new(),
            original_accounts: DashMap::new(),
            original_storages: DashMap::new(),
        }
    }

    pub fn new_read_only(rtx: ReadTransaction, read_provider: DatabaseReadProvider) -> Self {
        Self {
            tx: BatchTransaction::Read(rtx),
            read_provider,
            base_state_root: ArcSwapOption::empty(),
            touched_accounts: DashSet::new(),
            touched_storages: DashMap::new(),
            resetted_storages: DashSet::new(),
            original_accounts: DashMap::new(),
            original_storages: DashMap::new(),
        }
    }

    pub fn set_base_state_root(&self, root: B256) {
        self.base_state_root.store(Some(Arc::new(root)));
    }

    pub async fn collect_account_changes(&self) -> Vec<(Address, Option<Bytes>)> {
        let touched_cloned: Vec<Address> = self.touched_accounts.iter().map(|a| *a).collect();
        let mut results = Vec::new();
        for addr in touched_cloned {
            let acc = self.account(addr, None).await.unwrap();
            let bytes = acc.map(|a| {
                let mut buf = Vec::new();
                alloy_rlp::Encodable::encode(&a, &mut buf);
                Bytes::from(buf)
            });
            results.push((addr, bytes));
        }
        results
    }

    pub fn collect_storage_changes(&self) -> Vec<(Address, B256, U256)> {
        self.touched_storages.iter().map(|r| {
            let (a, s) = r.key();
            (*a, *s, *r.value())
        }).collect()
    }

    async fn calculate_state_root_internal(&self, is_eip161: bool, state_root: Option<B256>) -> Result<B256> {
        let state_root = state_root.or_else(|| self.base_state_root.load().as_ref().map(|r| **r));
        let initial_root = match state_root {
            Some(r) => r,
            None => {
                self.read_provider.header(BlockId::Number(BlockNumberOrTag::Latest)).await.ok().flatten()
                    .map(|h| h.state_root)
                    .unwrap_or(alloy_trie::EMPTY_ROOT_HASH)
            }
        };

        debug!("[Trie] calculate_state_root_internal: initial_root={:?}", initial_root);
        let mut trie = EthTrie::new(self, initial_root);

        // 1. Load addresses that were modified in this batch
        let batch_addresses = self.touched_accounts.iter().map(|a| *a).collect::<Vec<_>>();

        for addr in batch_addresses {
            let hashed_addr = alloy_primitives::keccak256(addr);

            if let Some(acc) = self.account(addr, state_root).await? {
                let storage_root = self.calculate_storage_root_internal(addr, state_root).await?;

                // EIP-161 check
                if is_eip161 && acc.nonce == 0 && acc.balance == U256::ZERO && (acc.code_hash == B256::default() || acc.code_hash == alloy_primitives::KECCAK256_EMPTY) {
                    if storage_root == alloy_trie::EMPTY_ROOT_HASH {
                        debug!("[Trie] Deleting empty account from trie: addr={:?}", addr);
                        trie.delete(hashed_addr).await?;
                        // Also remove from Accounts table
                        match &self.tx {
                            BatchTransaction::Write(wtx) => {
                                let mut table = wtx.open_table(Accounts::definition())?;
                                table.remove(addr)?;
                                // Sync PlainState and HashedState removal
                                let mut plain_table = wtx.open_table(PlainState::definition())?;
                                plain_table.remove(addr)?;
                                let mut hashed_table = wtx.open_table(HashedState::definition())?;
                                hashed_table.remove(hashed_addr)?;
                            }
                            BatchTransaction::Read(_) => {}
                        }
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
                match &self.tx {
                    BatchTransaction::Write(wtx) => {
                        let mut table = wtx.open_table(Accounts::definition())?;
                        table.insert(addr, trie_acc.clone())?;
                        // Sync PlainState and HashedState
                        let mut acc_rlp = Vec::new();
                        alloy_rlp::Encodable::encode(&trie_acc, &mut acc_rlp);
                        let acc_bytes = Bytes::from(acc_rlp);

                        let mut plain_table = wtx.open_table(PlainState::definition())?;
                        plain_table.insert(addr, acc_bytes.clone())?;
                        let mut hashed_table = wtx.open_table(HashedState::definition())?;
                        hashed_table.insert(hashed_addr, acc_bytes)?;
                    }
                    BatchTransaction::Read(_) => {}
                }

                let mut trie_acc_rlp = Vec::new();
                alloy_rlp::Encodable::encode(&trie_acc, &mut trie_acc_rlp);
                trie.insert(hashed_addr, trie_acc_rlp).await?;
            } else {
                debug!("[Trie] Deleting account from trie: addr={:?}", addr);
                trie.delete(hashed_addr).await?;
            }
        }

        let root = trie.root_hash();
        debug!("[Trie] Final state root: {:?}", root);
        Ok(root)
    }

    async fn calculate_storage_root_internal(&self, address: Address, state_root: Option<B256>) -> Result<B256> {
        let state_root = state_root.or_else(|| self.base_state_root.load().as_ref().map(|r| **r));
        let initial_root = self.read_provider.account(address, state_root).await?
            .map(|a| a.storage_root)
            .unwrap_or(alloy_trie::EMPTY_ROOT_HASH);

        let mut trie = EthTrie::new(self, initial_root);

        let touched_slots = self.touched_storages.iter()
            .filter(|r| r.key().0 == address)
            .map(|r| (r.key().1, *r.value()))
            .collect::<Vec<_>>();

        if self.resetted_storages.contains(&address) {
            // If resetted, we should probably start from empty trie or handle it.
            // For now, let's just clear what we have in the trie.
            // This is complex because we don't know all slots.
            // But if we use HashedStorage table, we can iterate.
            let _hashed_address = alloy_primitives::keccak256(address);
            // redb doesn't support easy prefix delete, so we'd have to iterate.
            // Let's assume for now reset_storage is rare and not fully optimized here.
        }

        for (slot, value) in touched_slots {
            let hashed_slot = alloy_primitives::keccak256(slot);
            if value == U256::ZERO {
                trie.delete(hashed_slot).await?;
            } else {
                let mut buf = Vec::new();
                alloy_rlp::Encodable::encode(&value, &mut buf);
                trie.insert(hashed_slot, buf).await?;
            }
        }

        Ok(trie.root_hash())
    }

    pub fn commit(self) -> Result<()> {
        self.tx.commit()?;
        Ok(())
    }

    pub fn clear_tracking(&self) {
        self.touched_accounts.clear();
        self.touched_storages.clear();
        self.resetted_storages.clear();
        self.original_accounts.clear();
        self.original_storages.clear();
    }

    pub fn mark_account_touched(&self, address: Address) -> Result<()> {
        self.touched_accounts.insert(address);
        Ok(())
    }
}

impl AccountProviderSync for BatchWriter {
    fn account_sync(&self, address: Address, state_root: Option<B256>) -> Result<Option<TrieAccount>> {
        let state_root = state_root.or_else(|| self.base_state_root.load().as_ref().map(|r| **r));
        if self.touched_accounts.contains(&address) {
            match &self.tx {
                BatchTransaction::Write(wtx) => {
                    let table = wtx.open_table(Accounts::definition())?;
                    return Ok(table.get(address)?.map(|v| v.value()));
                }
                BatchTransaction::Read(rtx) => {
                    let table = rtx.open_table(Accounts::definition())?;
                    return Ok(table.get(address)?.map(|v| v.value()));
                }
            }
        }
        self.read_provider.account_sync(address, state_root)
    }
}

impl StorageProviderSync for BatchWriter {
    fn storage_sync(&self, address: Address, slot: B256, state_root: Option<B256>) -> Result<U256> {
        let state_root = state_root.or_else(|| self.base_state_root.load().as_ref().map(|r| **r));
        if let Some(val) = self.touched_storages.get(&(address, slot)) {
            return Ok(*val);
        }
        if self.resetted_storages.contains(&address) {
            return Ok(U256::ZERO);
        }
        self.read_provider.storage_sync(address, slot, state_root)
    }
}

impl BytecodeProviderSync for BatchWriter {
    fn bytecode_sync(&self, code_hash: B256) -> Result<Option<Bytes>> {
        match &self.tx {
            BatchTransaction::Write(wtx) => {
                let table = wtx.open_table(Bytecodes::definition())?;
                Ok(table.get(code_hash)?.map(|v| v.value()))
            }
            BatchTransaction::Read(rtx) => {
                let table = rtx.open_table(Bytecodes::definition())?;
                Ok(table.get(code_hash)?.map(|v| v.value()))
            }
        }
    }
}

impl StateProviderSync for BatchWriter {
    fn plain_state_sync(&self, address: Address) -> Result<Option<Bytes>> {
        self.read_provider.plain_state_sync(address)
    }
    fn hashed_state_sync(&self, hash: B256) -> Result<Option<Bytes>> {
        self.read_provider.hashed_state_sync(hash)
    }
    fn trie_node_sync(&self, hash: B256) -> Result<Option<Bytes>> {
        self.read_provider.trie_node_sync(hash)
    }
}


#[async_trait]
impl HeaderProvider for BatchWriter {
    async fn header(&self, number: BlockId) -> anyhow::Result<Option<Header>> {
        self.read_provider.header(number).await
    }
    async fn header_td(&self, hash: B256) -> anyhow::Result<Option<U256>> {
        self.read_provider.header_td(hash).await
    }
    async fn latest_header_td(&self) -> anyhow::Result<Option<U256>> {
        self.read_provider.latest_header_td().await
    }
}

#[async_trait]
impl AccountProvider for BatchWriter {
    async fn account(&self, address: Address, state_root: Option<B256>) -> Result<Option<TrieAccount>> {
        let state_root = state_root.or_else(|| self.base_state_root.load().as_ref().map(|r| **r));
        // First check our local cache/modified state
        if self.touched_accounts.contains(&address) {
            // If it was touched, it should be in the table or we deleted it
            match &self.tx {
                BatchTransaction::Write(wtx) => {
                    let table = wtx.open_table(Accounts::definition())?;
                    return Ok(table.get(address)?.map(|v| v.value()));
                }
                BatchTransaction::Read(rtx) => {
                    let table = rtx.open_table(Accounts::definition())?;
                    return Ok(table.get(address)?.map(|v| v.value()));
                }
            }
        }

        // Then check original state from DB
        self.read_provider.account(address, state_root).await
    }

    async fn accounts(&self) -> Result<Vec<(Address, TrieAccount)>> {
        self.read_provider.accounts().await
    }

    async fn addresses(&self) -> Result<Vec<Address>> {
        self.read_provider.addresses().await
    }

    async fn transaction_count(&self, address: Address, _block_id: BlockId, state_root: Option<B256>) -> Result<u64> {
        let state_root = state_root.or_else(|| self.base_state_root.load().as_ref().map(|r| **r));
        self.account(address, state_root).await.map(|acc| acc.map(|a| a.nonce).unwrap_or(0))
    }
}

#[async_trait]
impl StorageProvider for BatchWriter {
    async fn storage(&self, address: Address, slot: B256, state_root: Option<B256>) -> Result<U256> {
        let state_root = state_root.or_else(|| self.base_state_root.load().as_ref().map(|r| **r));
        // Check local modified storage
        if let Some(val) = self.touched_storages.get(&(address, slot)) {
            return Ok(*val);
        }

        if self.resetted_storages.contains(&address) {
            return Ok(U256::ZERO);
        }

        // Check DB
        self.read_provider.storage(address, slot, state_root).await
    }

    async fn account_storages(&self, address: Address, state_root: Option<B256>) -> Result<Vec<(B256, U256)>> {
        let state_root = state_root.or_else(|| self.base_state_root.load().as_ref().map(|r| **r));
        self.read_provider.account_storages(address, state_root).await
    }
}

#[async_trait]
impl BytecodeProvider for BatchWriter {
    async fn bytecode(&self, code_hash: B256) -> Result<Option<Bytes>> {
        match &self.tx {
            BatchTransaction::Write(wtx) => {
                let table = wtx.open_table(Bytecodes::definition())?;
                Ok(table.get(code_hash)?.map(|v| v.value()))
            }
            BatchTransaction::Read(rtx) => {
                let table = rtx.open_table(Bytecodes::definition())?;
                Ok(table.get(code_hash)?.map(|v| v.value()))
            }
        }
    }
}

#[async_trait]
impl HeaderWriter for BatchWriter {
    async fn insert_header(&self, hash: B256, header: Header) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(Headers::definition())?;
            table.insert(hash, header)?;
        }
        Ok(())
    }

    async fn insert_block_hash(&self, hash: B256, number: u64) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(HeaderNumbers::definition())?;
            table.insert(hash, number)?;
        }
        Ok(())
    }

    async fn insert_header_td(&self, hash: B256, td: U256) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(HeaderTD::definition())?;
            table.insert(hash, td)?;
        }
        Ok(())
    }

    async fn insert_header_number(&self, hash: B256, number: u64) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(HeaderNumbers::definition())?;
            table.insert(hash, number)?;
        }
        Ok(())
    }
}

#[async_trait]
impl BlockWriter for BatchWriter {
    async fn set_canonical(&self, number: u64, hash: B256) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(CanonicalHeads::definition())?;
            table.insert(number, hash)?;
        }
        Ok(())
    }

    async fn remove_canonical(&self, number: u64) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(CanonicalHeads::definition())?;
            table.remove(number)?;
        }
        Ok(())
    }

    async fn insert_block_body(&self, hash: B256, _number: u64, body: BlockBody<Transaction>) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(BlockBodies::definition())?;
            table.insert(hash, body.clone())?;
        }

        // Also insert transactions and transaction lookups
        for (idx, tx) in body.transactions.into_iter().enumerate() {
            let tx_hash = *tx.hash();
            self.insert_transaction(tx_hash, tx).await?;
            self.insert_transaction_lookup(tx_hash, hash, idx as u64).await?;
        }
        Ok(())
    }

    async fn update_forkchoice(&self, head: B256, safe: Option<B256>, finalized: Option<B256>) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(Forkchoice::definition())?;
            table.insert("head".to_string(), head)?;
            if let Some(s) = safe {
                table.insert("safe".to_string(), s)?;
            }
            if let Some(f) = finalized {
                table.insert("finalized".to_string(), f)?;
            }
        }
        Ok(())
    }

    async fn add_payload(&self, _id: PayloadId, _block: Block<Transaction>, _receipts: Vec<Receipt>, _metas: Vec<ReceiptMeta>, _bundle: BlobsBundleV1) -> Result<()> {
        // Not implemented in original BatchWriter
        Ok(())
    }

    async fn remove_payload_by_block_hash(&self, hash: B256) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            // Bodies
            let mut bodies = wtx.open_table(BlockBodies::definition())?;
            if let Some(body) = bodies.remove(hash)? {
                let body = body.value();
                // Transactions
                let mut txs = wtx.open_table(Transactions::definition())?;
                let mut lookups = wtx.open_table(TransactionLookup::definition())?;
                for tx in body.transactions {
                    let tx_hash = tx.hash();
                    txs.remove(*tx_hash)?;
                    lookups.remove(*tx_hash)?;
                }
            }
        }
        Ok(())
    }

    async fn insert_block(&self, block: Block<Transaction>, receipts: Vec<Receipt>, metas: Vec<ReceiptMeta>) -> Result<()> {
        let hash = block.header.hash_slow();
        let number = block.header.number;

        self.insert_header(hash, block.header.clone()).await?;
        self.insert_block_hash(hash, number).await?;
        self.insert_header_number(hash, number).await?;
        self.insert_block_body(hash, number, block.body).await?;

        for (idx, receipt) in receipts.into_iter().enumerate() {
            self.insert_receipt(hash, idx as u64, receipt).await?;
        }
        for (idx, meta) in metas.into_iter().enumerate() {
            self.insert_receipt_meta(hash, idx as u64, meta).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl TransactionWriter for BatchWriter {
    async fn insert_transaction(&self, hash: B256, tx: Transaction) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(Transactions::definition())?;
            table.insert(hash, tx)?;
        }
        Ok(())
    }

    async fn insert_receipt(&self, block_hash: B256, index: u64, receipt: Receipt) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(Receipts::definition())?;
            table.insert((block_hash, index), receipt)?;
        }
        Ok(())
    }

    async fn insert_receipt_meta(&self, block_hash: B256, index: u64, meta: ReceiptMeta) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(ReceiptsMeta::definition())?;
            table.insert((block_hash, index), meta)?;
        }
        Ok(())
    }

    async fn remove_receipt_meta(&self, block_hash: B256, index: u64) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(ReceiptsMeta::definition())?;
            table.remove((block_hash, index))?;
        }
        Ok(())
    }

    async fn insert_transaction_lookup(&self, hash: B256, block_hash: B256, index: u64) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(TransactionLookup::definition())?;
            table.insert(hash, (block_hash, index))?;
        }
        Ok(())
    }
}

impl BatchWriter {
    pub async fn insert_transaction_lookup_for_block(&self, block_hash: B256, _block_number: u64) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut lookups = wtx.open_table(TransactionLookup::definition())?;
            let body_table = wtx.open_table(BlockBodies::definition())?;

            if let Some(body) = body_table.get(block_hash)? {
                let body = body.value();
                for (idx, tx) in body.transactions.iter().enumerate() {
                    lookups.insert(*tx.hash(), (block_hash, idx as u64))?;
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl AccountWriter for BatchWriter {
    async fn update_account(&self, address: Address, account: TrieAccount) -> Result<()> {
        // Capture original for changeset - perform await before locking
        let original = self.account(address, None).await?;
        self.original_accounts.entry(address).or_insert(original);

        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(Accounts::definition())?;
            table.insert(address, account.clone())?;
        }

        self.touched_accounts.insert(address);
        Ok(())
    }

    async fn remove_account(&self, address: Address) -> Result<()> {
        // Capture original for changeset - perform await before locking
        let original = self.account(address, None).await?;
        self.original_accounts.entry(address).or_insert(original);

        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(Accounts::definition())?;
            table.remove(address)?;
        }

        self.touched_accounts.insert(address);
        Ok(())
    }

    async fn calculate_state_root(&self, is_eip161: bool, state_root: Option<B256>) -> Result<B256> {
        self.calculate_state_root_internal(is_eip161, state_root).await
    }

    async fn calculate_storage_root(&self, address: Address, state_root: Option<B256>) -> Result<B256> {
        self.calculate_storage_root_internal(address, state_root).await
    }

    fn clear_tracking(&self) {
        self.touched_accounts.clear();
        self.touched_storages.clear();
        self.resetted_storages.clear();
        self.original_accounts.clear();
        self.original_storages.clear();
    }
}

#[async_trait]
impl StorageWriter for BatchWriter {
    async fn update_storage(&self, address: Address, slot: B256, value: U256) -> Result<()> {
        // Capture original for changeset - perform await before locking
        let original = self.storage(address, slot, None).await?;
        self.original_storages.entry((address, slot)).or_insert(original);

        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(Storages::definition())?;
            table.insert((address, slot), value)?;

            // Sync HashedState
            let hashed_address = alloy_primitives::keccak256(address);
            let hashed_slot = alloy_primitives::keccak256(slot);
            let combined_key = alloy_primitives::keccak256([hashed_address.0, hashed_slot.0].concat());
            let mut hashed_table = wtx.open_table(HashedState::definition())?;
            hashed_table.insert(combined_key, Bytes::from(value.to_be_bytes::<32>().to_vec()))?;
        }

        self.touched_storages.insert((address, slot), value);
        self.touched_accounts.insert(address);
        Ok(())
    }

    async fn reset_storage(&self, address: Address) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(Storages::definition())?;
            let mut hashed_table = wtx.open_table(HashedState::definition())?;
            let hashed_address = alloy_primitives::keccak256(address);

            // redb doesn't support easy prefix delete
            let mut to_remove = Vec::new();
            for entry in table.iter()? {
                let (key, _) = entry?;
                let (addr, slot) = key.value();
                if addr == address {
                    to_remove.push((addr, slot));
                }
            }
            for key in to_remove {
                table.remove(key)?;

                // Sync HashedState
                let hashed_slot = alloy_primitives::keccak256(key.1);
                let combined_key = alloy_primitives::keccak256([hashed_address.0, hashed_slot.0].concat());
                hashed_table.remove(combined_key)?;
            }
        }

        self.resetted_storages.insert(address);
        self.touched_accounts.insert(address);
        Ok(())
    }
}

#[async_trait]
impl BytecodeWriter for BatchWriter {
    async fn insert_bytecode(&self, code_hash: B256, bytecode: Bytes) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(Bytecodes::definition())?;
            table.insert(code_hash, bytecode)?;
        }
        Ok(())
    }
}

#[async_trait]
impl StateProvider for BatchWriter {
    async fn plain_state(&self, address: Address) -> Result<Option<Bytes>> {
        match &self.tx {
            BatchTransaction::Write(wtx) => {
                let table = wtx.open_table(PlainState::definition())?;
                Ok(table.get(address)?.map(|v| v.value()))
            }
            BatchTransaction::Read(rtx) => {
                let table = rtx.open_table(PlainState::definition())?;
                Ok(table.get(address)?.map(|v| v.value()))
            }
        }
    }

    async fn hashed_state(&self, hash: B256) -> Result<Option<Bytes>> {
        match &self.tx {
            BatchTransaction::Write(wtx) => {
                let table = wtx.open_table(HashedState::definition())?;
                Ok(table.get(hash)?.map(|v| v.value()))
            }
            BatchTransaction::Read(rtx) => {
                let table = rtx.open_table(HashedState::definition())?;
                Ok(table.get(hash)?.map(|v| v.value()))
            }
        }
    }

    async fn trie_node(&self, hash: B256) -> Result<Option<Bytes>> {
        match &self.tx {
            BatchTransaction::Write(wtx) => {
                let table = wtx.open_table(TrieNodes::definition())?;
                Ok(table.get(hash)?.map(|v| v.value()))
            }
            BatchTransaction::Read(rtx) => {
                let table = rtx.open_table(TrieNodes::definition())?;
                Ok(table.get(hash)?.map(|v| v.value()))
            }
        }
    }
}

#[async_trait]
impl StateWriter for BatchWriter {
    async fn update_plain_state(&self, address: Address, state: Bytes) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(PlainState::definition())?;
            table.insert(address, state)?;
        }
        Ok(())
    }

    async fn remove_plain_state(&self, address: Address) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(PlainState::definition())?;
            table.remove(address)?;
        }
        Ok(())
    }

    async fn update_hashed_state(&self, hash: B256, state: Bytes) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(HashedState::definition())?;
            table.insert(hash, state)?;
        }
        Ok(())
    }

    async fn update_trie_node(&self, hash: B256, node: Bytes) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(TrieNodes::definition())?;
            table.insert(hash, node)?;
        }
        Ok(())
    }
}

#[async_trait]
impl ChangeSetWriter for BatchWriter {
    async fn insert_account_change_set(&self, number: u64, change_set: Vec<(Address, Option<Bytes>)>) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(AccountChangeSets::definition())?;
            table.insert(number, change_set)?;
        }
        Ok(())
    }

    async fn insert_storage_change_set(&self, number: u64, change_set: Vec<(Address, B256, U256)>) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(StorageChangeSets::definition())?;
            table.insert(number, change_set)?;
        }
        Ok(())
    }

    async fn remove_change_set(&self, number: u64) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut ac = wtx.open_table(AccountChangeSets::definition())?;
            ac.remove(number)?;
            let mut sc = wtx.open_table(StorageChangeSets::definition())?;
            sc.remove(number)?;
        }
        Ok(())
    }
}

#[async_trait]
impl MetadataWriter for BatchWriter {
    async fn set_metadata(&self, key: String, value: Bytes) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(Metadata::definition())?;
            table.insert(key, value)?;
        }
        Ok(())
    }
}

#[async_trait]
impl PeerDiscoveryWriter for BatchWriter {
    async fn register_peer(&self, peer: PeerEntry) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(ActivePeers::definition())?;
            table.insert(peer.peer_id.clone(), peer)?;
        }
        Ok(())
    }

    async fn remove_peer(&self, peer_id: String) -> Result<()> {
        if let BatchTransaction::Write(wtx) = &self.tx {
            let mut table = wtx.open_table(ActivePeers::definition())?;
            table.remove(peer_id)?;
        }
        Ok(())
    }
}
