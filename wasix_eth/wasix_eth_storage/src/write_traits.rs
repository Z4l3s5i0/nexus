use wasix_eth_types::{Address, Block, BlockBody, Bytes, Header, PayloadId, PeerEntry, Transaction, TrieAccount, B256, U256, Receipt, ReceiptMeta, BlobsBundleV1};
use async_trait::async_trait;

/// Trait for managing the active peers table.
#[async_trait]
pub trait PeerDiscoveryWriter: Send + Sync {
    /// Adds or updates a peer in the table.
    async fn register_peer(&self, peer: PeerEntry) -> anyhow::Result<()>;
    /// Removes a peer from the table by its ID.
    async fn remove_peer(&self, peer_id: String) -> anyhow::Result<()>;
}

/// Trait for writing block headers to the database.
#[async_trait]
pub trait HeaderWriter: Send + Sync {
    /// Inserts a block header into the database.
    async fn insert_header(&self, hash: B256, header: Header) -> anyhow::Result<()>;
    /// Inserts a block hash to block number mapping.
    async fn insert_block_hash(&self, hash: B256, number: u64) -> anyhow::Result<()>;
    /// Inserts a block hash to block number mapping in the HeaderNumbers table.
    async fn insert_header_number(&self, hash: B256, number: u64) -> anyhow::Result<()>;
    /// Inserts the total difficulty of a block.
    async fn insert_header_td(&self, hash: B256, td: U256) -> anyhow::Result<()>;
}

/// Trait for writing block-related information to the database.
#[async_trait]
pub trait BlockWriter: Send + Sync {
    /// Sets a block hash as canonical for a given block number.
    async fn set_canonical(&self, number: u64, hash: B256) -> anyhow::Result<()>;
    /// Removes a canonical block hash for a given block number.
    async fn remove_canonical(&self, number: u64) -> anyhow::Result<()>;
    /// Inserts a block body for a given block number and hash.
    async fn insert_block_body(&self, hash: B256, number: u64, body: BlockBody<Transaction>) -> anyhow::Result<()>;
    /// Updates the forkchoice state.
    async fn update_forkchoice(&self, head: B256, safe: Option<B256>, finalized: Option<B256>) -> anyhow::Result<()>;
    /// Adds a new payload to the database.
    async fn add_payload(&self, id: PayloadId, block: Block<Transaction>, receipts: Vec<Receipt>, metas: Vec<ReceiptMeta>, bundle: BlobsBundleV1) -> anyhow::Result<()>;
    /// Removes a payload from the database by its block hash.
    async fn remove_payload_by_block_hash(&self, hash: B256) -> anyhow::Result<()>;
    /// Inserts a full block into the database and marks it as canonical.
    async fn insert_block(&self, block: Block<Transaction>, receipts: Vec<Receipt>, metas: Vec<ReceiptMeta>) -> anyhow::Result<()>;
}

/// Trait for writing transaction-related information to the database.
#[async_trait]
pub trait TransactionWriter: Send + Sync {
    /// Inserts a transaction into the database.
    async fn insert_transaction(&self, hash: B256, tx: Transaction) -> anyhow::Result<()>;
    /// Inserts a transaction receipt into the database.
    async fn insert_receipt(&self, block_hash: B256, index: u64, receipt: Receipt) -> anyhow::Result<()>;
    /// Inserts a transaction receipt metadata into the database.
    async fn insert_receipt_meta(&self, block_hash: B256, index: u64, meta: ReceiptMeta) -> anyhow::Result<()>;
    /// Removes a transaction receipt metadata from the database.
    async fn remove_receipt_meta(&self, block_hash: B256, index: u64) -> anyhow::Result<()>;
    /// Inserts a transaction hash to block hash and index mapping for fast lookups.
    async fn insert_transaction_lookup(&self, hash: B256, block_hash: B256, index: u64) -> anyhow::Result<()>;
}

/// Trait for writing account information to the database.
#[async_trait]
pub trait AccountWriter: Send + Sync {
    /// Updates account information for a given address.
    async fn update_account(&self, address: Address, account: TrieAccount) -> anyhow::Result<()>;
    /// Removes account information for a given address.
    async fn remove_account(&self, address: Address) -> anyhow::Result<()>;
    /// Calculates the state root.
    /// In many Ethereum implementations, calculate_state_root() only becomes canonical after trie updates are flushed/committed consistently.
    async fn calculate_state_root(&self, is_eip161: bool, state_root: Option<B256>) -> anyhow::Result<B256>;
    /// Calculates the storage root for a given address.
    async fn calculate_storage_root(&self, address: Address, state_root: Option<B256>) -> anyhow::Result<B256>;
    /// Clears any tracked "original" values used for ChangeSet generation.
    fn clear_tracking(&self);
    /// Marks an account as touched (modified) in the current batch.
    async fn mark_account_touched(&self, address: Address) -> anyhow::Result<()>;
}

/// Trait for writing storage information to the database.
#[async_trait]
pub trait StorageWriter: Send + Sync {
    /// Updates the value of a storage slot for a given address and slot key.
    async fn update_storage(
        &self,
        address: Address,
        slot: B256,
        value: U256,
    ) -> anyhow::Result<()>;
    /// Resets all storage slots for a given address.
    async fn reset_storage(&self, address: Address) -> anyhow::Result<()>;
}

/// Trait for writing contract bytecode to the database.
#[async_trait]
pub trait BytecodeWriter: Send + Sync {
    /// Inserts contract bytecode associated with a given code hash.
    async fn insert_bytecode(&self, code_hash: B256, bytecode: Bytes) -> anyhow::Result<()>;
}

/// Trait for writing state-related information to the database.
#[async_trait]
pub trait StateWriter: Send + Sync {
    /// Updates the plain state value for a given address.
    async fn update_plain_state(&self, address: Address, state: Bytes) -> anyhow::Result<()>;
    /// Removes the plain state value for a given address.
    async fn remove_plain_state(&self, address: Address) -> anyhow::Result<()>;
    /// Updates the hashed state value for a given hash.
    async fn update_hashed_state(&self, hash: B256, state: Bytes) -> anyhow::Result<()>;
    /// Updates a trie node at the given path.
    async fn update_trie_node(&self, hash: B256, node: Bytes) -> anyhow::Result<()>;
}

/// Trait for writing state change sets to the database.
#[async_trait]
pub trait ChangeSetWriter: Send + Sync {
    /// Inserts an account change set for a given block number.
    async fn insert_account_change_set(&self, number: u64, change_set: Vec<(Address, Option<Bytes>)>) -> anyhow::Result<()>;
    /// Inserts a storage change set for a given block number.
    async fn insert_storage_change_set(&self, number: u64, change_set: Vec<(Address, B256, U256)>) -> anyhow::Result<()>;
    /// Removes a change set for a given block number.
    async fn remove_change_set(&self, number: u64) -> anyhow::Result<()>;
}

#[async_trait]
pub trait MetadataWriter: Send + Sync {
    async fn set_metadata(&self, key: String, value: Bytes) -> anyhow::Result<()>;
}