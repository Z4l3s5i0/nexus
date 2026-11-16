# Storage Layer Documentation

This document describes the persistent storage layer in NEXUS, implemented in the `wasix_eth_storage` crate using the `redb` embedded key-value store.

## Overview

The storage layer provides a durable, transactional database for all chain data, state, and metadata. It is built on `redb`, a Rust embedded key-value database inspired by SQLite, providing ACID guarantees and efficient B-tree based storage.

**Key components:**

- **`EthDatabase`** — Database wrapper that manages table initialization and genesis setup
- **Tables** — 20+ typed tables for chain data, state, indexes, engine data, and P2P
- **Codecs** — RLP-based serialization for all table keys and values
- **Read providers** — Trait-based read access to chain data, state, and metadata
- **Write providers** — Trait-based write access for block processing and state changes
- **Trie** — Custom Merkle Patricia Trie implementation for state root calculation

## Database Schema

### Chain Tables

| Table | Key | Value | Description |
|---|---|---|---|
| `Headers` | `u64` (block number) | `Header` | Block headers indexed by number |
| `HeaderTD` | `B256` (block hash) | `U256` | Total difficulty per block |
| `BlockBodies` | `u64` (block number) | `BlockBody<Transaction>` | Block bodies (transactions, ommers) |
| `Transactions` | `u64` (block number) | `Transaction` | Transactions indexed by block |
| `Receipts` | `(B256, u64)` (block hash, tx index) | `Receipt` | Transaction receipts |
| `ReceiptsMeta` | `(B256, u64)` (block hash, tx index) | `ReceiptMeta` | Receipt metadata (log indices) |

### Chain Index Tables

| Table | Key | Value | Description |
|---|---|---|---|
| `CanonicalHeads` | `u64` (block number) | `B256` (block hash) | Canonical chain head mapping |
| `HeaderNumbers` | `B256` (block hash) | `u64` (block number) | Hash-to-number reverse index |
| `TransactionLookup` | `B256` (tx hash) | `(B256, u64)` (block hash, index) | Transaction location index |

### State Tables

| Table | Key | Value | Description |
|---|---|---|---|
| `Accounts` | `Address` | `TrieAccount` | Account state (nonce, balance, code hash, storage root) |
| `Storages` | `(Address, B256)` (address, slot) | `U256` | Contract storage slots |
| `Bytecodes` | `B256` (code hash) | `Bytes` | Contract bytecode |

### State Change Tables (Reorg Support)

| Table | Key | Value | Description |
|---|---|---|---|
| `AccountChangeSets` | `(u64, Address)` (block number, address) | `Option<TrieAccount>` | Account state changes per block |
| `StorageChangeSets` | `(u64, Address, B256)` (block number, address, slot) | `U256` | Storage changes per block |

These tables enable state recovery to previous block heights for reorg handling. A `None` value in `AccountChangeSets` indicates an account was destroyed.

### State Index Tables

| Table | Key | Value | Description |
|---|---|---|---|
| `TrieNodes` | `B256` (node hash) | `MyTrieNode` | Cached trie nodes |
| `TrieNodeRefs` | `B256` (node hash) | `u64` (reference count) | Trie node reference counting |

### Metadata Table

| Table | Key | Value | Description |
|---|---|---|---|
| `Metadata` | `String` | `String` | Key-value metadata (chain ID, genesis hash, config, etc.) |

### Engine Tables

| Table | Key | Value | Description |
|---|---|---|---|
| `Payloads` | `PayloadId` | `(Block, Vec<Receipt>, Vec<ReceiptMeta>, BlobsBundleV1)` | Pending payloads for consensus client |
| `Forkchoice` | `String` (key: "head", "safe", "finalized") | `B256` | Forkchoice state tracking |
| `StateBranches` | `B256` (block hash) | `u64` (block number) | Non-canonical block tracking |

### P2P Discovery Table

| Table | Key | Value | Description |
|---|---|---|---|
| `ActivePeers` | `String` (peer ID) | `PeerEntry` | Active peer connections |

## Codecs

All table data is serialized using RLP (Recursive Length Prefix) encoding via the `RedbRlp` trait (`codecs.rs`).

### RedbRlp Trait

```rust
pub trait RedbRlp: Debug + 'static {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut);
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error>;
    fn rlp_length(&self) -> usize;
}
```

### Implemented Types

The `RedbRlp` trait is implemented for:

- Primitive types: `u64`, `String`
- Ethereum types: `Header`, `B256`, `Address`, `U256`, `Transaction`, `Receipt`, `Bytes`
- Composite types: `BlockBody<Transaction>`, `TrieAccount`, `PayloadId`
- Tuples: `(Address, B256)`, `Vec<(Address, Option<Bytes>)>` (for change sets)
- Receipt metadata: `ReceiptMeta`
- P2P types: `PeerEntry`
- Engine types: `BlobsBundleV1`

The `Table` trait defines each table's name and key/value types:

```rust
pub trait Table: Debug {
    const NAME: &'static str;
    type Key: RedbRlp + 'static;
    type Value: RedbRlp + 'static;
}
```

## Read Providers

### DatabaseReadProvider

The main read provider (`read.rs`) wraps the `redb` database and implements multiple trait-based interfaces:

### Provider Traits

| Trait | Methods | Purpose |
|---|---|---|
| `HeaderProvider` | `header()`, `header_td()`, `latest_header_td()` | Read block headers |
| `BlockProvider` | `block()`, `block_hash()`, `block_number()`, `block_body()`, `get_payload()`, `is_canonical()` | Read blocks and payloads |
| `TransactionProvider` | `transaction()`, `transaction_receipt()`, `transaction_lookup()`, `transaction_block()` | Read transactions |
| `AccountProvider` | `account()`, `accounts()`, `addresses()`, `transaction_count()` | Read account state |
| `StorageProvider` | `storage()`, `account_storages()` | Read contract storage |
| `BytecodeProvider` | `bytecode()` | Read contract code |
| `StateProvider` | `trie_node()`, `state_root()` | Read trie and state roots |
| `ChainProvider` | `chain_id()`, `chain_config()` | Read chain metadata |
| `LogProvider` | `logs()` | Query logs by filter |
| `MetadataProvider` | `get_metadata()` | Read key-value metadata |
| `PeerDiscoveryProvider` | `get_active_peers()` | Read active peer list |

## Write Providers

### DatabaseWriteProvider

The write provider (`write.rs`) provides transactional write access:

### Writer Traits

| Trait | Methods | Purpose |
|---|---|---|
| `HeaderWriter` | `save_header()`, `save_headers()` | Save block headers |
| `BlockWriter` | `save_block()`, `save_block_body()` | Save blocks and bodies |
| `TransactionWriter` | `save_transaction()`, `save_receipt()` | Save transactions and receipts |
| `AccountWriter` | `save_account()`, `save_accounts()` | Save account state |
| `StorageWriter` | `save_storage()`, `save_storages()` | Save contract storage |
| `BytecodeWriter` | `save_bytecode()` | Save contract bytecode |
| `MetadataWriter` | `save_metadata()` | Save key-value metadata |
| `StateWriter` | `save_trie_node()`, `save_account_changeset()`, `save_storage_changeset()` | Save trie and change sets |
| `PeerDiscoveryWriter` | `save_active_peer()`, `remove_active_peer()` | Manage peer entries |

## Trie Implementation

The trie module (`trie.rs`) implements a custom Merkle Patricia Trie for state root calculation.

### Node Types

| Node | Description |
|---|---|
| `MyLeafNode` | Terminal node with key suffix and value |
| `MyExtensionNode` | Single-child node with key path compression |
| `MyBranchNode` | 16-way branching node with optional value |
| `MyTrieNode` | Enum wrapping the three node types |

### Path Encoding

Uses standard MPT hex encoding with flags:
- Bit 4 (0x10): odd-length flag
- Bit 5 (0x20): leaf flag

### State Root Calculation

The `calculate_state_root` function:

1. Iterates over all accounts sorted by address
2. For each account, computes the storage root from storage slots
3. Builds the account trie with RLP-encoded `TrieAccount` values
4. Returns the Keccak-256 hash of the root node

### Trie Operations

- **Insert** — Adds or updates a key-value pair, splitting nodes as needed
- **Delete** — Removes a key, merging nodes where possible
- **Get** — Traverses the trie to find a value by key
- **Root** — Computes the hash of the root node

## Genesis Initialization

The `init_genesis` method (`db.rs`) sets up the initial chain state:

1. **Check** — Verifies genesis hasn't already been initialized (via `genesis_hash` metadata)
2. **State** — Processes allocations: creates accounts, sets balances, deploys contract code
3. **State root** — Calculates the Merkle Patricia Trie root from all allocations
4. **Header** — Builds the genesis header with correct fork-specific fields (base fee, blob gas, withdrawals root, beacon block root)
5. **Metadata** — Saves chain ID, genesis hash, and chain configuration
6. **Commit** — Writes all data in a single transaction

### Fork-Aware Genesis

The genesis header includes fork-specific fields based on the active hardfork:

| Fork | Added Fields |
|---|---|
| London | `base_fee_per_gas` |
| Shanghai | `withdrawals_root` |
| Cancun | `blob_gas_used`, `excess_blob_gas`, `parent_beacon_block_root` |

## REORG_LIMIT and Payload Management

### Payload Storage

The `Payloads` table stores pending payloads created by the Engine API's `forkchoiceUpdated` with `PayloadAttributes`. Each payload is indexed by `PayloadId` and contains the full block, receipts, and blob bundle.

### State Branches

The `StateBranches` table tracks non-canonical blocks, enabling the node to serve payloads for forked chains without losing state data.

### Change Sets for Reorg Recovery

When a reorg occurs:

1. The node identifies the fork point (common ancestor)
2. Uses `AccountChangeSets` and `StorageChangeSets` to revert state changes block-by-block
3. Restores the state to the fork point
4. Applies the new chain's state changes

The `REORG_LIMIT` constant determines how many blocks of change sets are retained, limiting the reorg depth the node can handle.

## Database File

The database is stored as a single file (default: `<data_dir>/eth.redb`). The file name is derived from the chain ID and peer name configured via CLI flags.

## Export Tool

The `scripts/v2/db_exporter/` Rust tool provides a CLI for exporting database tables to JSON for analysis:

```bash
# List tables
db_exporter list --db-path eth.redb

# Export a table
db_exporter export --db-path eth.redb --table headers --db-type el
```

Supported database types: `el` (execution layer) and `metrics` (metrics server).
