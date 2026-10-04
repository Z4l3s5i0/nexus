use wasix_eth_types::{
    Address, Block, BlockBody, BlobsBundleV1, Header, Receipt, ReceiptMeta, Transaction,
    TrieAccount, B256, U256, PeerEntry, Bytes,
};
use crate::codecs::Table;

// --- Chain
#[derive(Debug)]
pub struct Headers;
impl Table for Headers { const NAME: &'static str = "headers"; type Key = B256; type Value = Header; }

#[derive(Debug)]
pub struct HeaderTD;
impl Table for HeaderTD { const NAME: &'static str = "header_td"; type Key = B256; type Value = U256; }

#[derive(Debug)]
pub struct BlockBodies;
impl Table for BlockBodies { const NAME: &'static str = "block_bodies"; type Key = B256; type Value = BlockBody<Transaction>; }

#[derive(Debug)]
pub struct Transactions;
impl Table for Transactions { const NAME: &'static str = "transactions"; type Key = B256; type Value = Transaction; }

#[derive(Debug)]
pub struct Receipts;
impl Table for Receipts { const NAME: &'static str = "receipts"; type Key = (B256, u64); type Value = Receipt; }

#[derive(Debug)]
pub struct ReceiptsMeta;
impl Table for ReceiptsMeta { const NAME: &'static str = "receipts_meta"; type Key = (B256, u64); type Value = ReceiptMeta; }

// --- Chain indexes
#[derive(Debug)]
pub struct CanonicalHeads;
impl Table for CanonicalHeads { const NAME: &'static str = "canonical_heads"; type Key = u64; type Value = B256; }

#[derive(Debug)]
pub struct HeaderNumbers;
impl Table for HeaderNumbers { const NAME: &'static str = "header_numbers"; type Key = B256; type Value = u64; }

#[derive(Debug)]
pub struct TransactionLookup;
impl Table for TransactionLookup { const NAME: &'static str = "transaction_lookup"; type Key = B256; type Value = (B256, u64); }

// --- State
#[derive(Debug)]
pub struct Accounts;
impl Table for Accounts { const NAME: &'static str = "accounts"; type Key = Address; type Value = TrieAccount; }

#[derive(Debug)]
pub struct Storages;
impl Table for Storages { const NAME: &'static str = "storages"; type Key = (Address, B256); type Value = U256; }

#[derive(Debug)]
pub struct Bytecodes;
impl Table for Bytecodes { const NAME: &'static str = "bytecodes"; type Key = B256; type Value = Bytes; }

// --- State changes (reorgs)
#[derive(Debug)]
pub struct AccountChangeSets;
impl Table for AccountChangeSets { const NAME: &'static str = "account_changesets"; type Key = u64; type Value = Vec<(Address, Option<TrieAccount>)>; }

#[derive(Debug)]
pub struct StorageChangeSets;
impl Table for StorageChangeSets { const NAME: &'static str = "storage_changesets"; type Key = u64; type Value = Vec<((Address, B256), Option<U256>)>; }

// --- State indexes
#[derive(Debug)]
pub struct PlainState; 
impl Table for PlainState { const NAME: &'static str = "plain_state"; type Key = Address; type Value = TrieAccount; }

#[derive(Debug)]
pub struct HashedState;
impl Table for HashedState { const NAME: &'static str = "hashed_state"; type Key = B256; type Value = TrieAccount; }

#[derive(Debug)]
pub struct HashedStorages;
impl Table for HashedStorages { const NAME: &'static str = "hashed_storages"; type Key = B256; type Value = U256; }

#[derive(Debug)]
pub struct TrieNodes;
impl Table for TrieNodes { const NAME: &'static str = "trie_nodes"; type Key = B256; type Value = Bytes; }

// --- Metadata
#[derive(Debug)]
pub struct Metadata;
impl Table for Metadata { const NAME: &'static str = "metadata"; type Key = String; type Value = Bytes; }

// --- Engine
#[derive(Debug)]
pub struct Payloads;
impl Table for Payloads { const NAME: &'static str = "payloads"; type Key = wasix_eth_types::PayloadId; type Value = (Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, BlobsBundleV1); }

#[derive(Debug)]
pub struct Forkchoice;
impl Table for Forkchoice { const NAME: &'static str = "forkchoice"; type Key = String; type Value = B256; }

// --- P2P Discovery
#[derive(Debug)]
pub struct ActivePeers;
impl Table for ActivePeers { const NAME: &'static str = "active_peers"; type Key = String; type Value = PeerEntry; }


