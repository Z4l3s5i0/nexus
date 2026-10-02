pub mod rpc {
    pub use alloy_rpc_types::eth::{
        Block as RpcBlock, Header as RpcHeader, Transaction as RpcTransaction,
        TransactionReceipt as RpcReceipt,
    };
    pub use alloy_primitives::Signature as RpcSignature;
}

#[deprecated(note = "Use wasix_eth_types::rpc::RpcHeader instead")]
pub type RpcHeader = rpc::RpcHeader;
#[deprecated(note = "Use wasix_eth_types::rpc::RpcTransaction instead")]
pub type RpcTransaction = rpc::RpcTransaction;
#[deprecated(note = "Use wasix_eth_types::rpc::RpcBlock instead")]
pub type RpcBlock<T = RpcTransaction> = alloy_rpc_types::Block<T, rpc::RpcHeader>;

pub type RpcTransactionReceipt = rpc::RpcReceipt;

pub mod consensus;
pub use consensus::{BlockBody, Receipt as WrappedReceipt, ReceiptMeta, Hardfork};
pub use consensus::constants as consensus_constants;

pub mod api;
pub use api::*;
pub use api::admin::{PeerEntry, PeerInfoDetailed as PeerInfo, HelloResponse};

pub mod chain;
pub use chain::*;
pub mod sync;
pub mod genesis;
pub mod p2p;
pub mod error;
pub mod engine_types;
pub mod serde_utils {
    pub use crate::consensus::serde_utils::*;
}

pub use alloy_eips::eip2718::{Decodable2718, Encodable2718};
pub use alloy_consensus::{
    Block, Header as ConsensusHeader, Signed, TxEip4844Variant, TxEnvelope, Typed2718, Transaction as TransactionTrait,
    EMPTY_OMMER_ROOT_HASH, TxEip4844, Receipt as ConsensusReceipt, BlobTransactionSidecarVariant,
    Header,
};
pub use alloy_primitives::{TxKind, hex, B512, Address, Bloom, Bytes, B256, U256, FixedBytes, B64, U64, Log as PrimitiveLog};
pub use alloy_eips::eip4895;
pub mod proofs {
    pub use alloy_consensus::proofs::{calculate_transaction_root, calculate_withdrawals_root};
}
pub use alloy_eips::eip4844::calc_blob_gasprice;
pub use alloy_eips::eip4844;
pub use alloy_eips::eip4788::{BEACON_ROOTS_CODE};
pub use alloy_eips::eip2935::{HISTORY_STORAGE_CODE};
pub use alloy_eips::eip7002::{WITHDRAWAL_REQUEST_PREDEPLOY_CODE};
pub use alloy_eips::eip7251::{CONSOLIDATION_REQUEST_PREDEPLOY_CODE};
pub use alloy_eips::eip6110;
pub use alloy_eips::eip7685;
pub use alloy_genesis::ChainConfig;
pub use alloy_rpc_types::engine::*;
pub use alloy_trie::{EMPTY_ROOT_HASH, root};
pub use alloy_genesis::GenesisAccount;
pub use alloy_rpc_types::{
    Filter, SyncStatus, TransactionRequest,
};
pub use anyhow::Result;
pub use async_trait::async_trait;

#[async_trait]
pub trait GossipProvider: Send + Sync {
    async fn broadcast_raw(&self, data: Vec<u8>);
    async fn broadcast_transaction(&self, tx: &TxEnvelope);
    async fn broadcast_new_pooled_transaction_hashes(&self, txs: Vec<TxEnvelope>);
    async fn broadcast_block(&self, block: &Block<TxEnvelope>);
}

pub struct NoopGossip;

#[async_trait]
impl GossipProvider for NoopGossip {
    async fn broadcast_raw(&self, _data: Vec<u8>) {}
    async fn broadcast_transaction(&self, _tx: &TxEnvelope) {}
    async fn broadcast_new_pooled_transaction_hashes(&self, _txs: Vec<TxEnvelope>) {}
    async fn broadcast_block(&self, _block: &Block<TxEnvelope>) {}
}

pub type BlockTransactions<T = RpcTransaction> = alloy_rpc_types::BlockTransactions<T>;
