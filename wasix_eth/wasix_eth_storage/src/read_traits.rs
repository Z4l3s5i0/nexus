use wasix_eth_types::{Block, BlockBody, BlockId, Bytes, Filter, Header, Log, PayloadId, Receipt, ReceiptMeta, Transaction, TrieAccount, B256, U256, BlobsBundleV1};
use wasix_eth_types::{Address, PeerEntry};
use async_trait::async_trait;
use crate::trie::MyTrieNode;

/// Trait for providing information about active peers.
#[async_trait]
pub trait PeerDiscoveryProvider: Send + Sync {
    /// Retrieves a list of active peers.
    async fn get_active_peers(&self) -> anyhow::Result<Vec<PeerEntry>>;
}

#[async_trait]
pub trait HeaderProvider: Send + Sync {
    /// Retrieves a block header by its number.
    async fn header(&self, number: BlockId) -> anyhow::Result<Option<Header>>;
    /// Retrieves the total difficulty of a block by its hash.
    async fn header_td(&self, hash: B256) -> anyhow::Result<Option<U256>>;
    /// Retrieves the latest total difficulty.
    async fn latest_header_td(&self) -> anyhow::Result<Option<U256>>;
}

/// Trait for reading block-related information from the database.
#[async_trait]
pub trait BlockProvider: HeaderProvider + Send + Sync {
    /// Retrieves a full block by its number.
    async fn block(&self, id: BlockId) -> anyhow::Result<Option<Block<Transaction>>>;
    /// Retrieves the hash of a block by its number.
    async fn block_hash(&self, number: u64) -> anyhow::Result<Option<B256>>;
    /// Retrieves the number of a block by its hash.
    async fn block_number(&self, hash: B256) -> anyhow::Result<Option<u64>>;
    /// Retrieves a block body by block number.
    async fn block_body(&self, number: u64) -> anyhow::Result<Option<BlockBody<Transaction>>>;
    /// Retrieves a block body by its hash.
    async fn block_body_by_hash(&self, hash: B256) -> anyhow::Result<Option<BlockBody<Transaction>>>;
    /// Retrieves a block by its hash.
    async fn block_by_hash(&self, hash: B256) -> anyhow::Result<Option<Block<Transaction>>>;
    /// Retrieves a payload by its ID.
    async fn get_payload(&self, payload_id: &PayloadId) -> Option<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, BlobsBundleV1)>;
    /// Retrieves a payload by its block hash.
    async fn get_payload_by_block_hash(&self, hash: B256) -> Option<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, BlobsBundleV1)>;
    async fn all_payload_ids(&self) -> Vec<PayloadId>;
    /// Retrieves the latest block number.
    async fn latest_block_number(&self) -> anyhow::Result<Option<u64>>;
    /// Retrieves the highest block number from either canonical heads or forkchoice.
    async fn highest_block_number(&self) -> anyhow::Result<u64>;
    /// Retrieves a forkchoice value by key.
    async fn forkchoice(&self, key: &str) -> anyhow::Result<Option<B256>>;
    /// Checks if a block hash is canonical.
    async fn is_canonical(&self, hash: B256) -> anyhow::Result<bool>;
}

/// Trait for reading transaction-related information from the database.
#[async_trait]
pub trait TransactionProvider: Send + Sync {
    /// Retrieves a transaction by its hash.
    async fn transaction(&self, hash: B256) -> anyhow::Result<Option<Transaction>>;
    /// Retrieves a transaction receipt by its hash.
    async fn transaction_receipt(&self, hash: B256) -> anyhow::Result<Option<Receipt>>;
    /// Retrieves a transaction receipt by block hash and index.
    async fn receipt(&self, block_hash: B256, index: u64) -> anyhow::Result<Option<Receipt>>;
    /// Retrieves a transaction receipt metadata by block hash and index.
    async fn receipt_meta(&self, block_hash: B256, index: u64) -> anyhow::Result<Option<ReceiptMeta>>;
    /// Retrieves the block hash and transaction index containing the transaction with the given hash.
    async fn transaction_lookup(&self, hash: B256) -> anyhow::Result<Option<(B256, u64)>>;
    /// Retrieves the block number containing the transaction with the given hash.
    async fn transaction_block(&self, hash: B256) -> anyhow::Result<Option<u64>>;
    /// Retrieves the block reference (number, hash, index) for a transaction.
    async fn transaction_block_reference(&self, hash: B256) -> anyhow::Result<Option<(u64, B256, u64)>>;
}

#[async_trait]
pub trait LogProvider: Send + Sync {
    async fn logs(&self, filter: Filter) -> anyhow::Result<Vec<Log>>;
}

/// Trait for reading account information from the database.
#[async_trait]
pub trait AccountProvider: Send + Sync {
    /// Retrieves account information for a given address and optional state root.
    async fn account(&self, address: Address, state_root: Option<B256>) -> anyhow::Result<Option<TrieAccount>>;
    /// Retrieves account information for all addresses.
    async fn accounts(&self) -> anyhow::Result<Vec<(Address, TrieAccount)>>;
    async fn addresses(&self) -> anyhow::Result<Vec<Address>>;
    /// Retrieves the transaction count (nonce) for a given address.
    async fn transaction_count(&self, address: Address, block_id: BlockId, state_root: Option<B256>) -> anyhow::Result<u64>;
}

/// Trait for reading chain-related information.
#[async_trait]
pub trait ChainProvider: Send + Sync {
    /// Retrieves the chain ID.
    async fn chain_id(&self) -> anyhow::Result<u64>;
    /// Retrieves the chain configuration.
    async fn chain_config(&self) -> anyhow::Result<Option<wasix_eth_types::ChainConfig>>;
}

/// Trait for reading storage information from the database.
#[async_trait]
pub trait StorageProvider: Send + Sync {
    /// Retrieves the value of a storage slot for a given address and slot key.
    async fn storage(
        &self,
        address: Address,
        slot: B256,
        state_root: Option<B256>,
    ) -> anyhow::Result<U256>;
    async fn account_storages(&self, address: Address, state_root: Option<B256>) -> anyhow::Result<Vec<(B256, U256)>>;
}

/// Trait for reading contract bytecode from the database.
#[async_trait]
pub trait BytecodeProvider: Send + Sync {
    /// Retrieves the bytecode associated with a given code hash.
    async fn bytecode(&self, code_hash: B256) -> anyhow::Result<Option<Bytes>>;
}

/// Trait for reading state-related information from the database.
#[async_trait]
pub trait StateProvider: Send + Sync {
    /// Retrieves the plain state value for a given address.
    async fn plain_state(&self, address: Address) -> anyhow::Result<Option<Bytes>>;
    /// Retrieves the hashed state value for a given hash.
    async fn hashed_state(&self, hash: B256) -> anyhow::Result<Option<Bytes>>;
    /// Retrieves a trie node by its hash.
    async fn trie_node(&self, hash: B256) -> anyhow::Result<Option<Bytes>>;

    /// Iterates over all leaves of a trie starting from the given root.
    /// Returns a vector of (hashed_key, encoded_value) pairs.
    async fn iterate_trie(&self, root: B256) -> anyhow::Result<Vec<(B256, Bytes)>> {
        if root == alloy_trie::EMPTY_ROOT_HASH {
            return Ok(Vec::new());
        }

        let mut leaves = Vec::new();
        enum NodeSource {
            Hash(B256),
            Raw(Vec<u8>),
        }
        let mut stack = vec![(NodeSource::Hash(root), alloy_trie::Nibbles::default())];

        while let Some((source, prefix)) = stack.pop() {
            let data = match source {
                NodeSource::Hash(hash) => match self.trie_node(hash).await? {
                    Some(bytes) => bytes.to_vec(),
                    None => continue,
                },
                NodeSource::Raw(raw) => raw,
            };

            let mut data_slice = &data[..];
            let node = match MyTrieNode::decode(&mut data_slice) {
                Ok(n) => n,
                Err(_) => continue,
            };

            match node {
                MyTrieNode::Branch(branch) => {
                    for i in (0..16).rev() {
                        let child = &branch.stack[i as usize];
                        if let Some(hash) = child.as_hash() {
                            let mut next_prefix = prefix.clone();
                            next_prefix.push_unchecked(i as u8);
                            stack.push((NodeSource::Hash(hash), next_prefix));
                        } else if !child.is_empty() {
                            let mut next_prefix = prefix.clone();
                            next_prefix.push_unchecked(i as u8);
                            stack.push((NodeSource::Raw(child.as_slice().to_vec()), next_prefix));
                        }
                    }
                    if let Some(value) = branch.value {
                        if prefix.len() == 64 {
                            leaves.push((B256::from_slice(&prefix.pack()[..]), Bytes::from(value)));
                        }
                    }
                }
                MyTrieNode::Leaf(leaf) => {
                    let mut full_key = prefix.clone();
                    full_key.extend(&leaf.key);
                    if full_key.len() == 64 {
                        leaves.push((B256::from_slice(&full_key.pack()[..]), Bytes::from(leaf.value)));
                    } else {
                        wasix_eth_utils::warn!("[Storage] Trie leaf has unexpected key length: {} (expected 64)", full_key.len());
                    }
                }
                MyTrieNode::Extension(ext) => {
                    let mut next_prefix = prefix.clone();
                    next_prefix.extend(&ext.key);
                    if let Some(hash) = ext.child.as_hash() {
                        stack.push((NodeSource::Hash(hash), next_prefix));
                    } else if !ext.child.is_empty() {
                        stack.push((NodeSource::Raw(ext.child.as_slice().to_vec()), next_prefix));
                    }
                }
                _ => {}
            }
        }

        Ok(leaves)
    }
}

/// Trait for reading state change sets from the database.
#[async_trait]
pub trait ChangeSetProvider: Send + Sync {
    /// Retrieves the account change set for a given block number.
    async fn account_change_set(&self, number: u64) -> anyhow::Result<Option<Vec<(Address, Option<Bytes>)>>>;
    /// Retrieves the storage change set for a given block number.
    async fn storage_change_set(&self, number: u64) -> anyhow::Result<Option<Vec<(Address, B256, U256)>>>;
}

#[async_trait]
pub trait MetadataProvider: Send + Sync {
    async fn get_metadata(&self, key: String) -> anyhow::Result<Option<Bytes>>;
}
