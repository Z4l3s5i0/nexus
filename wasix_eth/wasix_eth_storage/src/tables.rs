// --- Chain
#[derive(Debug)]
pub struct Headers;
#[derive(Debug)]
pub struct HeaderTD;
#[derive(Debug)]
pub struct BlockBodies;
#[derive(Debug)]
pub struct Transactions;
#[derive(Debug)]
pub struct Receipts;
#[derive(Debug)]
pub struct ReceiptsMeta;

// --- Chain indexes
#[derive(Debug)]
pub struct CanonicalHeads;
#[derive(Debug)]
pub struct HeaderNumbers;
#[derive(Debug)]
pub struct TransactionLookup;

// --- State
#[derive(Debug)]
pub struct Accounts;
#[derive(Debug)]
pub struct Storages;
#[derive(Debug)]
pub struct Bytecodes;

// --- State changes (reorgs)
#[derive(Debug)]
pub struct AccountChangeSets;
#[derive(Debug)]
pub struct StorageChangeSets;

// --- State indexes
#[derive(Debug)]
pub struct PlainState; 
#[derive(Debug)]
pub struct HashedState;
#[derive(Debug)]
pub struct TrieNodes;

// --- Metadata
#[derive(Debug)]
pub struct Metadata;

// --- Engine
#[derive(Debug)]
pub struct PayloadsByHash;
#[derive(Debug)]
pub struct PayloadIdToBlockHash;
#[derive(Debug)]
pub struct Forkchoice;

// --- P2P Discovery
#[derive(Debug)]
pub struct ActivePeers;


