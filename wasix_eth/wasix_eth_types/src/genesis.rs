use std::collections::BTreeMap;
use alloy_consensus::private::serde::{Deserialize, Serialize};
pub use alloy_genesis::{ChainConfig, GenesisAccount};
use alloy_primitives::{Address, Bytes, B256, U256};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenesisConfiguration {
    pub config: ChainConfig,
    pub alloc: BTreeMap<Address, GenesisAccount>,
    pub coinbase: Option<Address>,
    pub difficulty: Option<U256>,
    pub extra_data: Option<Bytes>,
    pub gas_limit: Option<U256>,
    pub nonce: Option<U256>,
    #[serde(alias = "mixhash")]
    pub mix_hash: Option<B256>,
    pub parent_hash: Option<B256>,
    pub timestamp: Option<U256>,
    pub number: Option<U256>,
    pub gas_used: Option<U256>,
    pub base_fee_per_gas: Option<U256>,
    pub excess_blob_gas: Option<U256>,
    pub blob_gas_used: Option<U256>,
    pub parent_beacon_block_root: Option<B256>,
    pub requests_hash: Option<B256>,
    pub withdrawals_root: Option<B256>,
}
//maybe add #[serde(default, with = "serde_helper::num::option_u64_hex")] for gas_limit, nonce, timestamp, number and change u256 to u64 instead
