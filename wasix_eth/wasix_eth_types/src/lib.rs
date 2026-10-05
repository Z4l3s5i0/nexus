pub mod chain;
pub use chain::*;
pub mod sync;
pub mod genesis;
pub mod p2p;
pub mod eth;
pub mod error;
pub mod admin;
pub mod engine_types;
pub mod net;
pub mod web3;

pub use alloy_consensus::TxEip4844Variant;

pub use alloy_eips::eip4788::{BEACON_ROOTS_CODE};
pub use alloy_eips::eip2935::{HISTORY_STORAGE_CODE};
pub use alloy_eips::eip7002::{WITHDRAWAL_REQUEST_PREDEPLOY_CODE};
pub use alloy_eips::eip7251::{CONSOLIDATION_REQUEST_PREDEPLOY_CODE};
pub use alloy_eips::eip6110;
pub use alloy_eips::eip7685;
use std::net::SocketAddr;
pub use alloy_genesis::ChainConfig;
pub use alloy_rpc_types::engine::ForkchoiceUpdated;
pub use alloy_rpc_types::engine::ForkchoiceState;
pub use crate::engine_types::ExecutionPayloadV4;
pub use crate::engine_types::ExecutionPayloadV3;
pub use crate::engine_types::ExecutionPayloadV2;
pub use alloy_rpc_types::engine::ExecutionPayloadV4 as AlloyExecutionPayloadV4;
pub use alloy_rpc_types::engine::ExecutionPayloadV3 as AlloyExecutionPayloadV3;
pub use alloy_rpc_types::engine::ExecutionPayloadV2 as AlloyExecutionPayloadV2;
pub use alloy_rpc_types::engine::ExecutionPayloadV1;
pub use alloy_rpc_types::engine::PayloadAttributes;
pub use alloy_rpc_types::engine::PayloadId;
pub use alloy_rpc_types::engine::PayloadStatus;
pub use alloy_rpc_types::engine::TransitionConfiguration;
pub use alloy_rpc_types::engine::ExecutionPayloadBodyV1;
pub use alloy_rpc_types::engine::ExecutionPayloadEnvelopeV2;
pub use alloy_rpc_types::engine::ExecutionPayloadEnvelopeV3;
pub use alloy_rpc_types::engine::ExecutionPayloadEnvelopeV4;
pub use alloy_rpc_types::engine::ExecutionPayloadFieldV2;
pub use alloy_rpc_types::engine::BlobsBundleV1;
pub use alloy_rpc_types::engine::PayloadStatusEnum;
pub use alloy_primitives::address;
pub use alloy_primitives::Address;
pub use alloy_primitives::Bloom;
pub use alloy_primitives::Signature;
pub use alloy_primitives::B256;
pub use alloy_primitives::U256;
pub use alloy_primitives::Bytes;
pub use alloy_primitives::logs_bloom;
pub use alloy_primitives::BloomInput;
pub use alloy_primitives::Log as LogPrimitive;
pub use alloy_primitives::LogData;
pub use alloy_primitives::B128;
pub use alloy_primitives::B64;
pub use alloy_primitives::B512;
pub use alloy_primitives::hex;
pub use alloy_primitives::FixedBytes;
pub use alloy_primitives::keccak256;
pub use alloy_primitives::b256;
pub use alloy_consensus::TxType;
pub use alloy_consensus::RlpEncodableReceipt;
pub use alloy_consensus::Header;
pub use alloy_consensus::proofs;
pub use alloy_consensus::EMPTY_OMMER_ROOT_HASH;
pub use alloy_consensus::TrieAccount;
pub use alloy_consensus::Eip658Value;
pub use alloy_consensus::Receipt as ConsensusReceipt;
pub use alloy_consensus::Block;
pub use alloy_consensus::BlockBody;
pub use alloy_consensus::TransactionEnvelope;
pub use alloy_consensus::Transaction as ConsensusTransaction;
pub use alloy_consensus::TxEnvelope as Transaction;
pub use alloy_consensus::transaction::PooledTransaction as TxPooledEnvelope;
pub use alloy_consensus::BlobTransactionSidecar;
pub use alloy_consensus::TxLegacy;
pub use alloy_consensus::Signed;
pub use alloy_consensus::SignableTransaction;
pub use alloy_consensus::TxEip1559;
pub use alloy_consensus::TxEip4844;
pub use alloy_consensus::TxEip7702;
pub use alloy_consensus::transaction::Recovered;
pub use alloy_consensus::transaction::SignerRecoverable;
pub use alloy_consensus::Eip2718EncodableReceipt;
pub use alloy_primitives::TxKind;
pub use evm::backend::OverlayedChangeSet;
pub use alloy_trie::EMPTY_ROOT_HASH;
pub use alloy_trie::root;
pub use alloy_eips::eip4844::{self, BlobAndProofV1, BlobAndProofV2, Blob, Bytes48, fake_exponential};
pub use alloy_eips::eip7840::{self, BlobParams};
pub use alloy_eips::eip7594::BlobTransactionSidecarEip7594;
pub use alloy_eips::eip7594::BlobTransactionSidecarVariant;
pub use alloy_eips::eip4895;
pub use alloy_eips::BlockId;
pub use alloy_eips::BlockNumberOrTag;
pub use alloy_eips::eip2718::Decodable2718;
pub use alloy_eips::eip2718::Encodable2718;
pub use alloy_eips::eip2718::Typed2718;
pub use alloy_genesis::GenesisAccount;
pub use alloy_rpc_types::Filter;
pub use alloy_rpc_types::SyncStatus;
pub use alloy_rpc_types::Block as AlloyRpcBlock;
pub use alloy_rpc_types::Transaction as AlloyRpcTransaction;
pub use alloy_rpc_types::TransactionReceipt as RpcTransactionReceipt;
pub use alloy_rpc_types::eth::Log;
pub use anyhow::Result;
pub use async_trait::async_trait;
use evm::uint::{H160, U256 as EvmU256};
use serde::{Deserialize, Serialize};
pub use alloy_rpc_types::TransactionRequest;
pub use alloy_rpc_types::ReceiptWithBloom;
pub use alloy_rpc_types::BlockTransactions as AlloyBlockTransactions;
pub type RpcBlock<T = RpcTransaction> = alloy_rpc_types::Block<T, RpcHeader>;


pub mod serde_utils {
    use serde::{Deserialize, Deserializer, Serializer};
    use alloy_primitives::{U64, U256, B64};

    pub fn u64_hex<S>(val: &u64, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(&format!("0x{:x}", val))
    }

    pub fn u64_hex_opt<S>(val: &Option<u64>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match val {
            Some(v) => s.serialize_str(&format!("0x{:x}", v)),
            None => s.serialize_none(),
        }
    }

    pub fn u256_hex<S>(val: &U256, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(&format!("0x{:x}", val))
    }

    pub fn u256_hex_opt<S>(val: &Option<U256>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match val {
            Some(v) => s.serialize_str(&format!("0x{:x}", v)),
            None => s.serialize_none(),
        }
    }

    pub fn b64_hex<S>(val: &B64, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(&format!("0x{:x}", val))
    }

    pub fn deserialize_u64_hex_opt<'de, D>(d: D) -> Result<Option<u64>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let val: Option<U64> = Option::deserialize(d)?;
        Ok(val.map(|v| v.to::<u64>()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcHeader {
    pub hash: Option<B256>,
    pub parent_hash: B256,
    #[serde(rename = "sha3Uncles")]
    pub ommers_hash: B256,
    #[serde(rename = "miner")]
    pub beneficiary: Address,
    pub state_root: B256,
    pub transactions_root: B256,
    pub receipts_root: B256,
    pub logs_bloom: Bloom,
    #[serde(serialize_with = "serde_utils::u256_hex")]
    pub difficulty: U256,
    #[serde(serialize_with = "serde_utils::u64_hex")]
    pub number: u64,
    #[serde(serialize_with = "serde_utils::u64_hex")]
    pub gas_limit: u64,
    #[serde(serialize_with = "serde_utils::u64_hex")]
    pub gas_used: u64,
    #[serde(serialize_with = "serde_utils::u64_hex")]
    pub timestamp: u64,
    pub extra_data: Bytes,
    pub mix_hash: B256,
    #[serde(serialize_with = "serde_utils::b64_hex")]
    pub nonce: B64,
    #[serde(
        default,
        serialize_with = "serde_utils::u256_hex_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub total_difficulty: Option<U256>,
    #[serde(
        default,
        serialize_with = "serde_utils::u256_hex_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub size: Option<U256>,
    #[serde(
        default,
        serialize_with = "serde_utils::u64_hex_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub base_fee_per_gas: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub withdrawals_root: Option<B256>,
    #[serde(
        default,
        serialize_with = "serde_utils::u64_hex_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub blob_gas_used: Option<u64>,
    #[serde(
        default,
        serialize_with = "serde_utils::u64_hex_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub excess_blob_gas: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_beacon_block_root: Option<B256>,
    pub requests_hash: Option<B256>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcTransaction<T = Transaction> {
    #[serde(flatten)]
    pub inner: T,
    pub from: Address,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_hash: Option<B256>,
    #[serde(
        default,
        serialize_with = "serde_utils::u64_hex_opt",
        deserialize_with = "serde_utils::deserialize_u64_hex_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub block_number: Option<u64>,
    #[serde(
        default,
        serialize_with = "serde_utils::u64_hex_opt",
        deserialize_with = "serde_utils::deserialize_u64_hex_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub transaction_index: Option<u64>,
    #[serde(
        default,
        serialize_with = "serde_utils::u64_hex_opt",
        deserialize_with = "serde_utils::deserialize_u64_hex_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub effective_gas_price: Option<u64>,
    #[serde(
        default,
        serialize_with = "serde_utils::u64_hex_opt",
        deserialize_with = "serde_utils::deserialize_u64_hex_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub block_timestamp: Option<u64>,
}
pub const HISTORY_STORAGE_ADDRESS: Address = address!("0000F90827F1C53A10CB7A02335B175320002935");
pub const BEACON_ROOTS_ADDRESS: Address = address!("0x000F3df6D732807Ef1319fB7B8bB8522d0Beac02");
pub const WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS: Address = address!("0x00000961ef480eb55e80d19ad83579a64c007002");
pub const CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS: Address = address!("0x0000bbddc7ce488642fb579f8b00f3a590007251");
pub const DEPOSIT_CONTRACT_ADDRESS: Address = eip6110::MAINNET_DEPOSIT_CONTRACT_ADDRESS;
pub const SYSTEM_ADDRESS: Address = address!("0xfffffffffffffffffffffffffffffffffffffffe");
pub const TARGET_BLOB_GAS_PER_BLOCK: u64 = 393216;
pub const MAX_BLOB_GAS_PER_BLOCK: u64 = 786432;
pub const DATA_GAS_PER_BLOB: u64 = 131072;
pub const BLOB_GASPRICE_UPDATE_FRACTION: u64 = 3338477;

pub const MAX_INIT_CODE_SIZE: u64 = 49152;

pub const MAX_MEMPOOL_SIZE: usize = 50000;

pub type BlockTransactions<T = RpcTransaction> = AlloyBlockTransactions<T>;

pub mod constants {
    pub use alloy_eips::eip7685::EMPTY_REQUESTS_HASH;
    pub const WITHDRAWAL_REQUEST_TYPE: u8 = 0x01;
    pub const CONSOLIDATION_REQUEST_TYPE: u8 = 0x02;

    // EIP-7002 constants
    pub const EXCESS_WITHDRAWAL_REQUESTS_STORAGE_SLOT: alloy_primitives::U256 = alloy_primitives::uint!(0_U256);
    pub const WITHDRAWAL_REQUEST_COUNT_STORAGE_SLOT: alloy_primitives::U256 = alloy_primitives::uint!(1_U256);
    pub const WITHDRAWAL_REQUEST_QUEUE_HEAD_STORAGE_SLOT: alloy_primitives::U256 = alloy_primitives::uint!(2_U256);
    pub const WITHDRAWAL_REQUEST_QUEUE_TAIL_STORAGE_SLOT: alloy_primitives::U256 = alloy_primitives::uint!(3_U256);
    pub const WITHDRAWAL_REQUEST_QUEUE_STORAGE_OFFSET: alloy_primitives::U256 = alloy_primitives::uint!(4_U256);
    pub const MAX_WITHDRAWAL_REQUESTS_PER_BLOCK: u64 = 16;
    pub const TARGET_WITHDRAWAL_REQUESTS_PER_BLOCK: u64 = 2;
    pub const MIN_WITHDRAWAL_REQUEST_FEE: u128 = 1;
    pub const WITHDRAWAL_REQUEST_FEE_UPDATE_FRACTION: u128 = 17;
    pub const EXCESS_INHIBITOR: alloy_primitives::U256 = alloy_primitives::U256::MAX;
}

pub mod eip6110_utils {
    use crate::*;
    use alloy_rlp::Encodable;

    pub const DEPOSIT_EVENT_SIGNATURE: B256 = alloy_primitives::b256!("649bbe30d5d036237248064a38d58c142c3098528994539a82046465494191c0");

    pub fn encode_deposit_request(deposit: &eip6110::DepositRequest, out: &mut Vec<u8>) {
        // EIP-6110: The request data is the RLP encoding of the DepositRequest fields
        // [pubkey, withdrawal_credentials, amount, signature, index]
        deposit.pubkey.encode(out);
        deposit.withdrawal_credentials.encode(out);
        deposit.amount.encode(out);
        deposit.signature.encode(out);
        deposit.index.encode(out);
    }

    pub fn decode_deposit_log(data: &[u8]) -> Result<eip6110::DepositRequest> {
        // The deposit contract log data is ABI-encoded (bytes, bytes, bytes, bytes, bytes)
        // Each 'bytes' field has an offset (32 bytes), then length (32 bytes), then data (padded).
        // Since there are 5 fields, there are 5 offsets at the beginning (5 * 32 = 160 bytes).
        if data.len() < 160 {
            return Err(anyhow::anyhow!("Invalid deposit log data length"));
        }

        let mut pubkey = FixedBytes::<48>::ZERO;
        let mut withdrawal_credentials = FixedBytes::<32>::ZERO;
        let mut amount = 0u64;
        let mut signature = FixedBytes::<96>::ZERO;
        let mut index = 0u64;

        for i in 0..5 {
            let offset = U256::from_be_slice(&data[i*32..(i+1)*32]).to::<usize>();
            if offset + 32 > data.len() {
                return Err(anyhow::anyhow!("Invalid offset in deposit log"));
            }
            let len = U256::from_be_slice(&data[offset..offset+32]).to::<usize>();
            if offset + 32 + len > data.len() {
                return Err(anyhow::anyhow!("Invalid length in deposit log"));
            }
            let field_data = &data[offset+32..offset+32+len];

            match i {
                0 => { // pubkey (48 bytes)
                    if field_data.len() != 48 { return Err(anyhow::anyhow!("Invalid pubkey length")); }
                    pubkey.copy_from_slice(field_data);
                }
                1 => { // withdrawal_credentials (32 bytes)
                    if field_data.len() != 32 { return Err(anyhow::anyhow!("Invalid withdrawal_credentials length")); }
                    withdrawal_credentials.copy_from_slice(field_data);
                }
                2 => { // amount (8 bytes, but logged as bytes)
                    if field_data.len() != 8 { return Err(anyhow::anyhow!("Invalid amount length")); }
                    amount = u64::from_le_bytes(field_data.try_into()?);
                }
                3 => { // signature (96 bytes)
                    if field_data.len() != 96 { return Err(anyhow::anyhow!("Invalid signature length")); }
                    signature.copy_from_slice(field_data);
                }
                4 => { // index (8 bytes)
                    if field_data.len() != 8 { return Err(anyhow::anyhow!("Invalid index length")); }
                    index = u64::from_le_bytes(field_data.try_into()?);
                }
                _ => unreachable!(),
            }
        }

        Ok(eip6110::DepositRequest {
            pubkey,
            withdrawal_credentials,
            amount,
            signature,
            index,
        })
    }
}

pub fn calc_blob_gasprice(excess_blob_gas: u64, update_fraction: u128) -> u128 {
    fake_exponential(1, excess_blob_gas as u128, update_fraction)
}

pub fn calc_excess_blob_gas(parent_excess_blob_gas: Option<u64>, parent_blob_gas_used: Option<u64>, target_blob_gas_per_block: u64) -> u64 {
    (parent_excess_blob_gas.unwrap_or(0) + parent_blob_gas_used.unwrap_or(0)).saturating_sub(target_blob_gas_per_block)
}
#[async_trait]
pub trait GossipProvider: Send + Sync {
    async fn broadcast_raw(&self, data: Vec<u8>);
    async fn broadcast_transaction(&self, tx: &Transaction);
    async fn broadcast_new_pooled_transaction_hashes(&self, txs: Vec<Transaction>);
    async fn broadcast_block(&self, block: &Block<Transaction>);
}

pub struct NoopGossip;

#[async_trait]
impl GossipProvider for NoopGossip {
    async fn broadcast_raw(&self, _data: Vec<u8>) {}
    async fn broadcast_transaction(&self, _tx: &Transaction) {}
    async fn broadcast_new_pooled_transaction_hashes(&self, _txs: Vec<Transaction>) {}
    async fn broadcast_block(&self, _block: &Block<Transaction>) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Hardfork {
    Frontier,
    Homestead,
    TangerineWhistle,
    SpuriousDragon,
    Byzantium,
    Constantinople,
    Petersburg,
    Istanbul,
    MuirGlacier,
    Berlin,
    London,
    ArrowGlacier,
    GrayGlacier,
    Paris,
    Shanghai,
    Cancun,
    Prague,
}

impl Hardfork {
    pub fn get_active_fork(config: &ChainConfig, block_number: u64, timestamp: u64) -> Self {
        Self::get_active_fork_with_total_difficulty(config, block_number, timestamp, None)
    }

    pub fn is_shanghai_active(&self) -> bool {
        *self >= Hardfork::Shanghai
    }

    pub fn is_cancun_active(&self) -> bool {
        *self >= Hardfork::Cancun
    }

    pub fn get_active_fork_with_total_difficulty(config: &ChainConfig, block_number: u64, timestamp: u64, total_difficulty: Option<U256>) -> Self {
        let mut forks = Vec::new();
        if let Some(b) = config.homestead_block { forks.push((b, false, Hardfork::Homestead)); }
        if let Some(b) = config.eip150_block { forks.push((b, false, Hardfork::TangerineWhistle)); }
        if let Some(b) = config.eip155_block { forks.push((b, false, Hardfork::SpuriousDragon)); }
        if let Some(b) = config.eip158_block { forks.push((b, false, Hardfork::SpuriousDragon)); }
        if let Some(b) = config.byzantium_block { forks.push((b, false, Hardfork::Byzantium)); }
        if let Some(b) = config.constantinople_block { forks.push((b, false, Hardfork::Constantinople)); }
        if let Some(b) = config.petersburg_block { forks.push((b, false, Hardfork::Petersburg)); }
        if let Some(b) = config.istanbul_block { forks.push((b, false, Hardfork::Istanbul)); }
        if let Some(b) = config.muir_glacier_block { forks.push((b, false, Hardfork::MuirGlacier)); }
        if let Some(b) = config.berlin_block { forks.push((b, false, Hardfork::Berlin)); }
        if let Some(b) = config.london_block { forks.push((b, false, Hardfork::London)); }
        if let Some(b) = config.arrow_glacier_block { forks.push((b, false, Hardfork::ArrowGlacier)); }
        if let Some(b) = config.gray_glacier_block { forks.push((b, false, Hardfork::GrayGlacier)); }
        if let Some(b) = config.merge_netsplit_block { forks.push((b, false, Hardfork::Paris)); }
        if let Some(t) = config.shanghai_time { forks.push((t, true, Hardfork::Shanghai)); }
        if let Some(t) = config.cancun_time { forks.push((t, true, Hardfork::Cancun)); }
        if let Some(t) = config.prague_time { forks.push((t, true, Hardfork::Prague)); }

        // Sort by type then value (EIP-6122)
        forks.sort_by(|a, b| (a.1, a.0).cmp(&(b.1, b.0)));

        let mut active_fork = Hardfork::Frontier;
        for (val, is_timestamp, fork) in forks {
            let active = if is_timestamp { val <= timestamp } else { val <= block_number };
            if active {
                active_fork = fork;
            } else {
                break;
            }
        }

        // Special check for TTD/Paris if not already covered by block number
        if active_fork < Hardfork::Paris {
            if let (Some(ttd), Some(td)) = (config.terminal_total_difficulty, total_difficulty) {
                if td >= ttd {
                    return Hardfork::Paris;
                }
            }
        }

        active_fork
    }

    pub fn blob_params(&self, config: &ChainConfig) -> Option<BlobParams> {
        let name = match self {
            Hardfork::Cancun => "cancun",
            Hardfork::Prague => "prague",
            _ => return None,
        };
        config.blob_schedule.get(name).copied()
    }
}

pub fn address_to_h160(address: Address) -> H160 {
    H160(address.0 .0)
}
pub fn alloy_u256_to_evm_u256(u256: U256) -> EvmU256 {
    EvmU256(u256.into_limbs())
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReceiptMeta {
    pub contract_address: Option<Address>,
}

impl alloy_rlp::Encodable for ReceiptMeta {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let payload_length = match self.contract_address {
            Some(_) => 21, // 1 byte header (0x94) + 20 bytes address
            None => 0,    // empty list
        };
        alloy_rlp::Header { list: true, payload_length }.encode(out);
        if let Some(addr) = self.contract_address {
            alloy_rlp::Encodable::encode(&addr, out);
        }
    }
    fn length(&self) -> usize {
        let l = match self.contract_address {
            Some(_) => 21,
            None => 0,
        };
        alloy_rlp::length_of_length(l) + l
    }
}

impl alloy_rlp::Decodable for ReceiptMeta {
    fn decode(buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        let h = alloy_rlp::Header::decode(buf)?;
        if !h.list {
            return Err(alloy_rlp::Error::Custom("Expected list for ReceiptMeta"));
        }
        
        let mut body = &buf[..h.payload_length];
        *buf = &buf[h.payload_length..];

        let contract_address = if body.is_empty() {
             None
        } else {
             Some(<Address as alloy_rlp::Decodable>::decode(&mut body)?)
        };
        
        if !body.is_empty() {
             return Err(alloy_rlp::Error::UnexpectedLength);
        }

        Ok(ReceiptMeta { contract_address })
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct PeerEntry {
    pub peer_id: String,
    pub discovery_addr: SocketAddr,
    pub p2p_addr: SocketAddr,
}
#[derive(Clone)]
pub struct PeerInfo {
    pub discovery_addr: SocketAddr,
    pub p2p_addr: SocketAddr,
    pub discovery_url: String,
    pub p2p_url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloResponse {
    pub peer_id: String,
    pub discovery_addr: String,
    pub p2p_addr: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Receipt {
    pub tx_type: u8,
    pub receipt: ConsensusReceipt,
    pub logs_bloom: Bloom,
}

impl alloy_rlp::Encodable for Receipt {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        if self.tx_type == 0 {
            self.receipt.rlp_encode_with_bloom(&self.logs_bloom, out);
        } else {
            out.put_u8(self.tx_type);
            self.receipt.rlp_encode_with_bloom(&self.logs_bloom, out);
        }
    }

    fn length(&self) -> usize {
        let len = self.receipt.rlp_encoded_length_with_bloom(&self.logs_bloom);
        if self.tx_type == 0 {
            len
        } else {
            len + 1
        }
    }
}

impl alloy_rlp::Decodable for Receipt {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        if buf.is_empty() {
            return Err(alloy_rlp::Error::InputTooShort);
        }
        let tx_type = if buf[0] > 0x7f {
            0
        } else {
            let t = buf[0];
            *buf = &buf[1..];
            t
        };
        let rb = alloy_consensus::ReceiptWithBloom::<ConsensusReceipt>::decode(buf)?;
        Ok(Receipt {
            tx_type,
            receipt: rb.receipt,
            logs_bloom: rb.logs_bloom,
        })
    }
}

impl alloy_eips::eip2718::Typed2718 for Receipt {
    fn ty(&self) -> u8 {
        self.tx_type
    }
}

impl alloy_consensus::RlpEncodableReceipt for Receipt {
    fn rlp_encoded_length_with_bloom(&self, bloom: &Bloom) -> usize {
        self.receipt.rlp_encoded_length_with_bloom(bloom)
    }

    fn rlp_encode_with_bloom(&self, bloom: &Bloom, out: &mut dyn alloy_rlp::BufMut) {
        self.receipt.rlp_encode_with_bloom(bloom, out);
    }
}

impl Eip2718EncodableReceipt for Receipt {
    fn eip2718_encoded_length_with_bloom(&self, bloom: &Bloom) -> usize {
        let len = self.receipt.rlp_encoded_length_with_bloom(bloom);
        if self.tx_type == 0 {
            len
        } else {
            len + 1
        }
    }

    fn eip2718_encode_with_bloom(&self, bloom: &Bloom, out: &mut dyn alloy_rlp::BufMut) {
        if self.tx_type == 0 {
            self.receipt.rlp_encode_with_bloom(bloom, out);
        } else {
            out.put_u8(self.tx_type);
            self.receipt.rlp_encode_with_bloom(bloom, out);
        }
    }
}

impl Encodable2718 for Receipt {
    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.eip2718_encode_with_bloom(&self.logs_bloom, out);
    }

    fn encode_2718_len(&self) -> usize {
        self.eip2718_encoded_length_with_bloom(&self.logs_bloom)
    }
}
impl Default for Receipt {
    fn default() -> Self {
        Self {
            tx_type: 0,
            receipt: ConsensusReceipt {
                status: Eip658Value::Eip658(true),
                cumulative_gas_used: 0,
                logs: Vec::new(),
            },
            logs_bloom: Bloom::ZERO,
        }
    }
}
