use crate::codecs::Table;
use crate::tables::*;
use redb::Database;
use redb::ReadableDatabase;
use redb::ReadableTable;
use wasix_eth_types::{Address, PeerEntry, BlobsBundleV1};
use wasix_eth_types::Block;
use wasix_eth_types::BlockBody;
use wasix_eth_types::BlockId;
use wasix_eth_types::BlockNumberOrTag;
use wasix_eth_types::Bytes;
use wasix_eth_types::Filter;
use wasix_eth_types::Header;
use wasix_eth_types::Log;
use wasix_eth_types::PayloadId;
use wasix_eth_types::Receipt;
use wasix_eth_types::ReceiptMeta;
use wasix_eth_types::Result;
use wasix_eth_types::Transaction;
use wasix_eth_types::TrieAccount;
use wasix_eth_types::B256;
use wasix_eth_types::U256;
use std::sync::Arc;
use alloy_rlp::Decodable;
use crate::trie::MyTrieNode;
use anyhow::Error;
use wasix_eth_types::error::RpcError;
use wasix_eth_utils::info;
use crate::read_traits::{AccountProvider, BlockProvider, BytecodeProvider, ChainProvider, ChangeSetProvider, HeaderProvider, LogProvider, MetadataProvider, PeerDiscoveryProvider, StateProvider, StorageProvider, TransactionProvider};
use crate::write_traits::StateWriter;
use crate::trie::EthTrie;

/// Implementation of database read providers using a `redb` database.
#[derive(Clone)]
pub struct DatabaseReadProvider {
    db: Arc<Database>,
}

impl DatabaseReadProvider {
    /// Creates a new `DatabaseReadProvider` from a `redb` database.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl HeaderProvider for DatabaseReadProvider {
    fn header(&self, id: BlockId) -> Result<Option<Header>> {
        let hash = match id {
            BlockId::Hash(hash) => Some(hash.block_hash),
            BlockId::Number(n) => match n {
                BlockNumberOrTag::Number(n) => self.block_hash(n)?,
                BlockNumberOrTag::Latest => {
                    let head_hash = self.forkchoice("head")?;
                    if let Some(h) = head_hash {
                        Some(h)
                    } else if let Some(n) = self.latest_block_number()? {
                        self.block_hash(n)?
                    } else {
                        None
                    }
                },
                BlockNumberOrTag::Safe => {
                    let hash = self.forkchoice("safe")?;
                    match hash {
                        Some(h) if h != B256::ZERO => Some(h),
                        _ => return Err(RpcError::BlockNotFound(BlockId::Number(n)).into()),
                    }
                },
                BlockNumberOrTag::Finalized => {
                    let hash = self.forkchoice("finalized")?;
                    match hash {
                        Some(h) if h != B256::ZERO => Some(h),
                        _ => return Err(RpcError::BlockNotFound(BlockId::Number(n)).into()),
                    }
                },
                BlockNumberOrTag::Pending => {
                    let head_hash = self.forkchoice("head")?;
                    if let Some(h) = head_hash {
                        Some(h)
                    } else if let Some(n) = self.latest_block_number()? {
                        self.block_hash(n)?
                    } else {
                        None
                    }
                },
                BlockNumberOrTag::Earliest => self.block_hash(0)?,
            },
        };

        let hash = match hash {
            Some(h) if h != B256::ZERO => h,
            _ => return Ok(None),
        };

        let tx = self.db.begin_read()?;
        let table = tx.open_table(Headers::definition())?;
        let value = table.get(hash)?;
        Ok(value.map(|v| v.value()))
    }

    fn header_td(&self, hash: B256) -> Result<Option<U256>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(HeaderTD::definition())?;
        let value = table.get(hash)?;
        Ok(value.map(|v| v.value()))
    }

    fn latest_header_td(&self) -> Result<Option<U256>> {
        if let Some(number) = self.latest_block_number()? {
            if let Some(hash) = self.block_hash(number)? {
                return self.header_td(hash);
            }
        }
        Ok(None)
    }
}

impl BlockProvider for DatabaseReadProvider {
    fn block(&self, id: BlockId) -> Result<Option<Block<Transaction>>> {
        let header = self.header(id)?;
        if let Some(header) = header {
            let hash = header.hash_slow();
            let body = self.block_body_by_hash(hash)?;
            if let Some(body) = body {
                return Ok(Some(Block {
                    header,
                    body,
                }));
            }
        }
        Ok(None)
    }

    fn block_hash(&self, number: u64) -> Result<Option<B256>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(CanonicalHeads::definition())?;
        let value = table.get(number)?;
        Ok(value.map(|v| v.value()))
    }

    fn block_number(&self, hash: B256) -> Result<Option<u64>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(HeaderNumbers::definition())?;
        let value = table.get(hash)?;
        Ok(value.map(|v| v.value()))
    }

    fn block_body(&self, number: u64) -> Result<Option<BlockBody<Transaction>>> {
        if let Some(hash) = self.block_hash(number)? {
            return self.block_body_by_hash(hash);
        }
        Ok(None)
    }

    fn block_body_by_hash(&self, hash: B256) -> Result<Option<BlockBody<Transaction>>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(BlockBodies::definition())?;
        let value = table.get(hash)?;
        Ok(value.map(|v| v.value()))
    }

    fn block_by_hash(&self, hash: B256) -> Result<Option<Block<Transaction>>> {
        self.block(BlockId::Hash(hash.into()))
    }

    fn get_payload(&self, payload_id: &PayloadId) -> Option<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, BlobsBundleV1)> {
        let tx = self.db.begin_read().ok()?;
        let table = tx.open_table(Payloads::definition()).ok()?;
        let value = table.get(*payload_id).ok()??;
        Some(value.value())
    }

    fn get_payload_by_block_hash(&self, hash: B256) -> Option<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, BlobsBundleV1)> {
        let tx = self.db.begin_read().ok()?;
        let table = tx.open_table(Payloads::definition()).ok()?;
        for entry in table.iter().ok()? {
            if let Ok((_, value)) = entry {
                let (block, receipts, metas, bundle): (Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, BlobsBundleV1) = value.value();
                if block.header.hash_slow() == hash {
                    return Some((block, receipts, metas, bundle));
                }
            }
        }
        None
    }

    fn all_payload_ids(&self) -> Vec<PayloadId> {
        let mut ids = Vec::new();
        if let Ok(tx) = self.db.begin_read() {
            if let Ok(table) = tx.open_table(Payloads::definition()) {
                if let Ok(iter) = table.iter() {
                    for entry in iter {
                        if let Ok((id, _)) = entry {
                            ids.push(id.value());
                        }
                    }
                }
            }
        }
        ids
    }

    fn latest_block_number(&self) -> Result<Option<u64>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(CanonicalHeads::definition())?;
        let last = table.iter()?.rev().next();
        if let Some(last) = last {
            let (number, _) = last?;
            return Ok(Some(number.value()));
        }
        Ok(None)
    }

    fn highest_block_number(&self) -> Result<u64> {
        let latest = self.latest_block_number()?.unwrap_or(0);
        let head_hash = self.forkchoice("head")?;
        if let Some(hash) = head_hash {
            if let Some(number) = self.block_number(hash)? {
                return Ok(std::cmp::max(latest, number));
            }
        }
        Ok(latest)
    }

    fn forkchoice(&self, key: &str) -> Result<Option<B256>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(Forkchoice::definition())?;
        let value = table.get(key.to_string())?;
        Ok(value.map(|v| v.value()))
    }

    fn is_canonical(&self, hash: B256) -> Result<bool> {
        if let Some(number) = self.block_number(hash)? {
            if let Some(canonical_hash) = self.block_hash(number)? {
                return Ok(canonical_hash == hash);
            }
        }
        Ok(false)
    }
}

impl TransactionProvider for DatabaseReadProvider {
    fn transaction(&self, hash: B256) -> Result<Option<Transaction>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(Transactions::definition())?;
        let value = table.get(hash)?;
        Ok(value.map(|v| v.value()))
    }
    

    fn transaction_receipt(&self, hash: B256) -> Result<Option<Receipt>> {
        if let Some((_number, block_hash, index)) = self.transaction_block_reference(hash)? {
            return self.receipt(block_hash, index);
        }
        Ok(None)
    }

    fn receipt(&self, block_hash: B256, index: u64) -> Result<Option<Receipt>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(Receipts::definition())?;
        let value = table.get((block_hash, index))?;
        Ok(value.map(|v| v.value()))
    }

    fn receipt_meta(&self, block_hash: B256, index: u64) -> Result<Option<ReceiptMeta>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(ReceiptsMeta::definition())?;
        let value = table.get((block_hash, index))?;
        Ok(value.map(|v| v.value()))
    }

    fn transaction_lookup(&self, hash: B256) -> Result<Option<(B256, u64)>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(TransactionLookup::definition())?;
        let value = table.get(hash)?;
        Ok(value.map(|v| v.value()))
    }

    fn transaction_block(&self, hash: B256) -> Result<Option<u64>> {
        if let Some((block_hash, _)) = self.transaction_lookup(hash)? {
            if let Some(number) = self.block_number(block_hash)? {
                // Verify if it's canonical
                if let Some(canonical_hash) = self.block_hash(number)? {
                    if canonical_hash == block_hash {
                        return Ok(Some(number));
                    }
                }
            }
        }
        Ok(None)
    }

    fn transaction_block_reference(&self, hash: B256) -> Result<Option<(u64, B256, u64)>> {
        if let Some((block_hash, index)) = self.transaction_lookup(hash)? {
            if let Some(number) = self.block_number(block_hash)? {
                // Verify if it's canonical
                if let Some(canonical_hash) = self.block_hash(number)? {
                    if canonical_hash == block_hash {
                        return Ok(Some((number, block_hash, index)));
                    }
                }
            }
        }
        Ok(None)
    }
}

impl LogProvider for DatabaseReadProvider {
    fn logs(&self, filter: Filter) -> Result<Vec<Log>> {
        let mut all_logs = Vec::new();

        let latest_block = self.latest_block_number()?.unwrap_or(0);
        let from_block = match filter.block_option.get_from_block() {
            Some(BlockNumberOrTag::Number(n)) => *n,
            Some(BlockNumberOrTag::Latest) => latest_block,
            Some(BlockNumberOrTag::Earliest) => 0,
            _ => 0,
        };

        let to_block = match filter.block_option.get_to_block() {
            Some(BlockNumberOrTag::Number(n)) => *n,
            Some(BlockNumberOrTag::Latest) => latest_block,
            _ => std::cmp::max(latest_block, from_block),
        };

        for number in from_block..=to_block {
            let block_hash = match self.block_hash(number)? {
                Some(h) => h,
                None => continue,
            };

            let body = match self.block_body(number)? {
                Some(b) => b,
                None => continue,
            };

            for (tx_index, tx) in body.transactions.iter().enumerate() {
                let tx_hash = tx.hash();
                let receipt = match self.receipt(block_hash, tx_index as u64)? {
                    Some(r) => r,
                    None => continue,
                };

                for (log_index, log) in receipt.receipt.logs.iter().enumerate() {
                    // Filter by address
                    if !filter.address.is_empty() {
                        if !filter.address.contains(&log.address) {
                            continue;
                        }
                    }

                    // Filter by topics
                    let mut topics_match = true;
                    for (i, filter_topic) in filter.topics.iter().enumerate() {
                        if !filter_topic.is_empty() {
                            if i >= log.data.topics().len() {
                                topics_match = false;
                                break;
                            }
                            if !filter_topic.contains(&log.data.topics()[i]) {
                                topics_match = false;
                                break;
                            }
                        }
                    }

                    if topics_match {
                        all_logs.push(Log {
                            inner: log.clone(),
                            block_hash: Some(block_hash),
                            block_number: Some(number),
                            block_timestamp: None, // We don't have it easily here, can be added if needed
                            transaction_hash: Some(*tx_hash),
                            transaction_index: Some(tx_index as u64),
                            log_index: Some(log_index as u64),
                            removed: false,
                        });
                    }
                }
            }
        }

        Ok(all_logs)
    }
}

impl AccountProvider for DatabaseReadProvider {
    fn account(&self, address: Address, state_root: Option<B256>) -> Result<Option<TrieAccount>> {
        if let Some(root) = state_root {
            if root == alloy_trie::EMPTY_ROOT_HASH {
                return Ok(None);
            }

            let hashed_address = alloy_primitives::keccak256(address);
            
            // Proper trie traversal for historical queries
            return self.get_account_from_trie(root, hashed_address);
        }
        
        let tx = self.db.begin_read()?;
        let table = tx.open_table(Accounts::definition())?;
        let value = table.get(address)?;
        Ok(value.map(|v| v.value()))
    }
    fn accounts(&self) -> Result<Vec<(Address, TrieAccount)>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(Accounts::definition())?;
        let mut accounts = Vec::new();

        for entry in table.iter()? {
            let (key, value) = entry?;
            accounts.push((key.value(), value.value()));
        }

        Ok(accounts)
    }

    fn addresses(&self) -> Result<Vec<Address>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(Accounts::definition())?;
        let mut addresses = Vec::new();

        for entry in table.iter()? {
            let (key, _) = entry?;
            addresses.push(key.value());
        }

        Ok(addresses)
    }

    fn transaction_count(&self, address: Address, _block_id: BlockId, state_root: Option<B256>) -> Result<u64> {
        let account = self.account(address, state_root)?;
        Ok(account.as_ref().map(|a| a.nonce).unwrap_or(0))
    }
}

impl DatabaseReadProvider {
    fn get_account_from_trie(&self, root: B256, hashed_address: B256) -> Result<Option<TrieAccount>> {
        let mut current_hash = root;
        let nibbles = alloy_trie::Nibbles::unpack(hashed_address);
        let mut cursor = 0;

        loop {
            let node_bytes = match self.trie_node(current_hash)? {
                Some(bytes) => bytes,
                None => {
                    return Ok(None)
                },
            };

            let mut data = &node_bytes.0[..];
            let node = match MyTrieNode::decode(&mut data) {
                Ok(n) => n,
                Err(_) => {
                    // If it's not a trie node, it might be the account RLP itself
                    let mut buf = &node_bytes.0[..];
                    if let Ok(acc) = TrieAccount::decode(&mut buf) {
                        return Ok(Some(acc));
                    }
                    return Ok(None);
                }
            };

            match node {
                MyTrieNode::Branch(branch) => {
                    if cursor == nibbles.len() {
                        return Ok(branch.value.and_then(|v| {
                            let mut slice = &v[..];
                            TrieAccount::decode(&mut slice).ok()
                        }));
                    }
                    let nibble = nibbles.get(cursor).unwrap() as usize;
                    let child = &branch.stack[nibble];
                    if let Some(child_hash) = child.as_hash() {
                        current_hash = child_hash;
                        cursor += 1;
                    } else if !child.is_empty() {
                        // Embedded node
                        let mut child_data = child.as_slice();
                        let mut current_node = MyTrieNode::decode(&mut child_data)?;
                        cursor += 1;
                        loop {
                            match current_node {
                                MyTrieNode::Leaf(leaf) => {
                                    if nibbles.slice(cursor..) == leaf.key {
                                        let mut v = &leaf.value[..];
                                        return Ok(Some(TrieAccount::decode(&mut v).map_err(|e| Error::msg(format!("Failed to decode account: {:?}", e)))?));
                                    }
                                    return Ok(None);
                                }
                                MyTrieNode::Extension(ext) => {
                                    if nibbles.slice(cursor..).starts_with(&ext.key) {
                                        cursor += ext.key.len();
                                        if let Some(h) = ext.child.as_hash() {
                                            current_hash = h;
                                            break; // Back to outer loop to fetch from DB
                                        } else {
                                            let mut next_data = ext.child.as_slice();
                                            current_node = MyTrieNode::decode(&mut next_data)?;
                                        }
                                    } else {
                                        return Ok(None);
                                    }
                                }
                                MyTrieNode::Branch(b) => {
                                    if cursor == nibbles.len() {
                                        return Ok(b.value.and_then(|v| {
                                            let mut slice = &v[..];
                                            TrieAccount::decode(&mut slice).ok()
                                        }));
                                    }
                                    let n = nibbles.get(cursor).unwrap() as usize;
                                    let c = &b.stack[n];
                                    if let Some(h) = c.as_hash() {
                                        current_hash = h;
                                        cursor += 1;
                                        break; // Back to outer loop
                                    } else if !c.is_empty() {
                                        let mut next_data = c.as_slice();
                                        current_node = MyTrieNode::decode(&mut next_data)?;
                                        cursor += 1;
                                    } else {
                                        return Ok(None);
                                    }
                                }
                                _ => return Ok(None),
                            }
                        }
                    } else {
                        return Ok(None);
                    }
                }
                MyTrieNode::Leaf(leaf) => {
                    let leaf_nibbles = leaf.key.clone();
                    if nibbles.slice(cursor..) == leaf_nibbles {
                        let mut v = &leaf.value[..];
                        return Ok(Some(TrieAccount::decode(&mut v).map_err(|e| Error::msg(format!("Failed to decode account: {:?}", e)))?))
                    } else {
                        return Ok(None);
                    }
                }
                MyTrieNode::Extension(ext) => {
                    let ext_nibbles = ext.key.clone();
                    if nibbles.slice(cursor..).starts_with(&ext_nibbles) {
                        cursor += ext_nibbles.len();
                        if let Some(child_hash) = ext.child.as_hash() {
                            current_hash = child_hash;
                        } else {
                            // Embedded node in extension
                            let mut child_data = ext.child.as_slice();
                            let mut current_node = MyTrieNode::decode(&mut child_data)?;
                            loop {
                                match current_node {
                                    MyTrieNode::Leaf(leaf) => {
                                        if nibbles.slice(cursor..) == leaf.key {
                                            let mut v = &leaf.value[..];
                                            return Ok(Some(TrieAccount::decode(&mut v).map_err(|e| Error::msg(format!("Failed to decode account: {:?}", e)))?));
                                        }
                                        return Ok(None);
                                    }
                                    MyTrieNode::Extension(ext_inner) => {
                                        if nibbles.slice(cursor..).starts_with(&ext_inner.key) {
                                            cursor += ext_inner.key.len();
                                            if let Some(h) = ext_inner.child.as_hash() {
                                                current_hash = h;
                                                break;
                                            } else {
                                                let mut next_data = ext_inner.child.as_slice();
                                                current_node = MyTrieNode::decode(&mut next_data)?;
                                            }
                                        } else {
                                            return Ok(None);
                                        }
                                    }
                                    MyTrieNode::Branch(b) => {
                                        if cursor == nibbles.len() {
                                            return Ok(b.value.and_then(|v| {
                                                let mut slice = &v[..];
                                                TrieAccount::decode(&mut slice).ok()
                                            }));
                                        }
                                        let n = nibbles.get(cursor).unwrap() as usize;
                                        let c = &b.stack[n];
                                        if let Some(h) = c.as_hash() {
                                            current_hash = h;
                                            cursor += 1;
                                            break;
                                        } else if !c.is_empty() {
                                            let mut next_data = c.as_slice();
                                            current_node = MyTrieNode::decode(&mut next_data)?;
                                            cursor += 1;
                                        } else {
                                            return Ok(None);
                                        }
                                    }
                                    _ => return Ok(None),
                                }
                            }
                        }
                    } else {
                        return Ok(None);
                    }
                }
                _ => return Ok(None),
            }
        }
    }

    pub fn calculate_state_root(&self, _is_eip161: bool, state_root: Option<B256>) -> Result<B256> {
        let mut trie_accounts = std::collections::HashMap::new();

        if let Some(root) = state_root {
            info!("[Storage] Loading historical state from root (read-only): {:?}", root);
            let leaves = self.iterate_trie(root)?;
            for (hashed_addr, encoded_acc) in leaves {
                trie_accounts.insert(hashed_addr, encoded_acc.to_vec());
            }
        } else {
            let addresses = self.addresses()?;
            for addr in addresses {
                if let Some(acc) = self.account(addr, None)? {
                    let storage_root = self.calculate_storage_root(addr, None)?;
                    let trie_acc = TrieAccount {
                        nonce: acc.nonce,
                        balance: acc.balance,
                        storage_root,
                        code_hash: if acc.code_hash == B256::default() { alloy_primitives::KECCAK256_EMPTY } else { acc.code_hash },
                    };
                    let mut buf = Vec::new();
                    alloy_rlp::Encodable::encode(&trie_acc, &mut buf);
                    trie_accounts.insert(alloy_primitives::keccak256(addr), buf);
                }
            }
        }

        let mut sorted_accounts: Vec<_> = trie_accounts.into_iter().collect();
        sorted_accounts.sort_by_key(|(h, _)| *h);

        let root = crate::trie::calculate_trie_root(sorted_accounts)?;
        info!("[Storage] Calculated state root (read-only): {:?}", root);
        Ok(root)
    }

    pub fn calculate_storage_root(&self, address: Address, state_root: Option<B256>) -> Result<B256> {
        let mut trie_storages = std::collections::HashMap::new();

        if let Some(root) = state_root {
            if let Some(acc) = self.account(address, Some(root))? {
                let leaves = self.iterate_trie(acc.storage_root)?;
                for (hashed_key, encoded_val) in leaves {
                    trie_storages.insert(hashed_key, encoded_val.to_vec());
                }
            }
        } else {
            let storages = self.account_storages(address, None)?;
            for (slot, value) in storages {
                if value != U256::ZERO {
                    let mut buf = Vec::new();
                    alloy_rlp::Encodable::encode(&value, &mut buf);
                    trie_storages.insert(alloy_primitives::keccak256(slot), buf);
                }
            }
        }

        if trie_storages.is_empty() {
            return Ok(alloy_trie::EMPTY_ROOT_HASH);
        }

        let mut sorted_storages: Vec<_> = trie_storages.into_iter().collect();
        sorted_storages.sort_by_key(|(h, _)| *h);

        let root = crate::trie::calculate_trie_root(sorted_storages)?;
        Ok(root)
    }
}

impl ChainProvider for DatabaseReadProvider {
    fn chain_id(&self) -> Result<u64> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(Metadata::definition())?;
        if let Some(value) = table.get("chain_id".to_string())? {
            let chain_id_bytes = value.value();
            
            // Try decoding as 8-byte big-endian first
            if chain_id_bytes.len() == 8 {
                let mut b = [0u8; 8];
                b.copy_from_slice(&chain_id_bytes);
                return Ok(u64::from_be_bytes(b));
            }

            let chain_id_str = String::from_utf8_lossy(&chain_id_bytes);
            
            // Try parsing as decimal
            if let Ok(id) = chain_id_str.parse::<u64>() {
                return Ok(id);
            }
            
            // Try parsing as hex if it starts with 0x
            let s = chain_id_str.trim();
            if let Some(hex_str) = s.strip_prefix("0x") {
                if let Ok(id) = u64::from_str_radix(hex_str, 16) {
                    return Ok(id);
                }
            } else if let Ok(id) = u64::from_str_radix(s, 16) {
                // Try hex without prefix as well
                return Ok(id);
            }
            return Err(Error::from(RpcError::ParseError(format!("Invalid chain_id format in database: {}", chain_id_str))));
        }
        // Default to 1 (Ethereum Mainnet) if not specified
        Ok(31133)
    }

    fn chain_config(&self) -> Result<Option<wasix_eth_types::ChainConfig>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(Metadata::definition())?;
        if let Some(value) = table.get("chain_config".to_string())? {
            let data = value.value();
            let config: wasix_eth_types::ChainConfig = serde_json::from_slice(&data)?;
            return Ok(Some(config));
        }
        Ok(None)
    }
}

impl StateWriter for DatabaseReadProvider {
    fn update_plain_state(&self, _address: Address, _state: Bytes) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("DatabaseReadProvider is read-only"))
    }
    fn remove_plain_state(&self, _address: Address) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("DatabaseReadProvider is read-only"))
    }
    fn update_hashed_state(&self, _hash: B256, _state: Bytes) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("DatabaseReadProvider is read-only"))
    }
    fn update_trie_node(&self, _hash: B256, _node: Bytes) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("DatabaseReadProvider is read-only"))
    }
}

impl StorageProvider for DatabaseReadProvider {
    fn storage(
        &self,
        address: Address,
        slot: B256,
        state_root: Option<B256>,
    ) -> Result<U256> {
        if let Some(root) = state_root {
            if root == alloy_trie::EMPTY_ROOT_HASH {
                return Ok(U256::ZERO);
            }

            // 1. Get account from trie
            let account = self.account(address, Some(root))?;
            if let Some(acc) = account {
                if acc.storage_root == alloy_trie::EMPTY_ROOT_HASH {
                    return Ok(U256::ZERO);
                }
                
                // 2. Get storage value from storage trie
                let hashed_slot = alloy_primitives::keccak256(slot);
                let mut trie = EthTrie::new(self, acc.storage_root);
                let val_bytes = trie.get(hashed_slot).map_err(|e| anyhow::anyhow!("Storage trie lookup failed: {}", e))?;
                if let Some(bytes) = val_bytes {
                    let mut slice = &bytes[..];
                    let val: U256 = alloy_rlp::Decodable::decode(&mut slice).map_err(|e| anyhow::anyhow!("RLP decode failed: {}", e))?;
                    return Ok(val);
                }
            }
            return Ok(U256::ZERO);
        }

        let tx = self.db.begin_read()?;
        let table = tx.open_table(Storages::definition())?;
        let value = table.get((address, slot))?;
        Ok(value.map(|v| v.value()).unwrap_or_default())
    }

    fn account_storages(&self, address: Address, state_root: Option<B256>) -> Result<Vec<(B256, U256)>> {
        if let Some(root) = state_root {
            // 1. Get account from trie
            let account = self.account(address, Some(root))?;
            if let Some(acc) = account {
                if acc.storage_root == alloy_trie::EMPTY_ROOT_HASH {
                    return Ok(Vec::new());
                }
                
                // 2. Walk the storage trie
                let mut trie = EthTrie::new(self, acc.storage_root);
                let entries = trie.all_entries().map_err(|e| anyhow::anyhow!("Storage trie walk failed: {}", e))?;
                let mut storages = Vec::with_capacity(entries.len());
                for (hashed_slot, val_bytes) in entries {
                    let mut slice = &val_bytes[..];
                    let val: U256 = alloy_rlp::Decodable::decode(&mut slice).map_err(|e| anyhow::anyhow!("RLP decode failed: {}", e))?;
                    storages.push((hashed_slot, val));
                }
                return Ok(storages);
            }
            return Ok(Vec::new());
        }

        let tx = self.db.begin_read()?;
        let table = tx.open_table(Storages::definition())?;
        let mut storages = Vec::new();
        // Since the key is (Address, B256), we can use a range scan
        let start = (address, B256::ZERO);
        let end = (address, B256::repeat_byte(0xff));
        
        for entry in table.range(start..=end)? {
            let (key, value) = entry?;
            let (_, slot) = key.value();
            storages.push((slot, value.value()));
        }
        Ok(storages)
    }
}

impl BytecodeProvider for DatabaseReadProvider {
    fn bytecode(&self, code_hash: B256) -> Result<Option<Bytes>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(Bytecodes::definition())?;
        let value = table.get(code_hash)?;
        Ok(value.map(|v| v.value()))
    }
}

impl StateProvider for DatabaseReadProvider {
    fn plain_state(&self, address: Address) -> Result<Option<Bytes>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(PlainState::definition())?;
        let value = table.get(address)?;
        Ok(value.map(|v| v.value()))
    }

    fn hashed_state(&self, hash: B256) -> Result<Option<Bytes>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(HashedState::definition())?;
        let value = table.get(hash)?;
        Ok(value.map(|v| v.value()))
    }

    fn trie_node(&self, hash: B256) -> Result<Option<Bytes>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(TrieNodes::definition())?;
        
        // Standard lookup
        if let Some(value) = table.get(hash)? {
            return Ok(Some(value.value()));
        }
        
        // If hash represents a combined key H(root || hash) used in BatchWriter,
        // it would have been found above. If we are looking for a node by its hash
        // but it was stored by combined key only, we'd need the root.
        // However, standard EthTrie uses hashes of RLP.
        
        Ok(None)
    }
}

impl MetadataProvider for DatabaseReadProvider {
    fn get_metadata(&self, key: String) -> Result<Option<Bytes>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(Metadata::definition())?;
        let value = table.get(key)?;
        Ok(value.map(|v| v.value()))
    }
}

impl PeerDiscoveryProvider for DatabaseReadProvider {
    fn get_active_peers(&self) -> Result<Vec<PeerEntry>> {
        let rtx = self.db.begin_read()?;
        let table = rtx.open_table(ActivePeers::definition())?;
        let mut peers = Vec::new();
        for item in table.iter()? {
            let (_, value) = item?;
            peers.push(value.value());
        }
        Ok(peers)
    }
}

impl ChangeSetProvider for DatabaseReadProvider {
    fn account_change_set(&self, number: u64) -> Result<Option<Vec<(Address, Option<Bytes>)>>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(AccountChangeSets::definition())?;
        let value = table.get(number)?;
        Ok(value.map(|v| v.value()))
    }

    fn storage_change_set(&self, number: u64) -> Result<Option<Vec<(Address, B256, U256)>>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(StorageChangeSets::definition())?;
        let value = table.get(number)?;
        Ok(value.map(|v| v.value()))
    }
}