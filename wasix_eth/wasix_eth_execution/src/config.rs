use wasix_eth_types::*;
use evm::standard::Config;
use evm::backend::InMemoryEnvironment;
use evm::uint::{H160, H256, U256 as EvmU256};
use wasix_eth_utils::debug;

pub fn get_evm_config(chain_config: &ChainConfig, block_number: u64, timestamp: u64, total_difficulty: Option<U256>) -> Config {
    let fork = Hardfork::get_active_fork_with_total_difficulty(chain_config, block_number, timestamp, total_difficulty);
    debug!("[Execution] get_evm_config: block={}, fork={:?}", block_number, fork);
    let config = match fork {
        Hardfork::Prague => Config::prague(),
        Hardfork::Cancun => Config::cancun(),
        Hardfork::Shanghai => Config::shanghai(),
        Hardfork::Paris => Config::shanghai(), // Use Shanghai for Paris as it includes Merge changes
        Hardfork::GrayGlacier => Config::london(),
        Hardfork::ArrowGlacier => Config::london(),
        Hardfork::London => Config::london(),
        Hardfork::Berlin => Config::berlin(),
        Hardfork::MuirGlacier => Config::istanbul(),
        Hardfork::Istanbul => Config::istanbul(),
        Hardfork::Petersburg => Config::petersburg(),
        Hardfork::Constantinople => Config::petersburg(),
        Hardfork::Byzantium => Config::byzantium(),
        Hardfork::SpuriousDragon => Config::spurious_dragon(),
        Hardfork::TangerineWhistle => Config::tangerine_whistle(),
        Hardfork::Homestead => Config::homestead(),
        Hardfork::Frontier => Config::frontier(),
    };

    // Custom adjustments if any

    config
}

pub async fn prepare_execution_env(
    chain_id: u64,
    chain_config: &ChainConfig,
    header: &Header,
    total_difficulty: Option<U256>
) -> (Hardfork, Config, InMemoryEnvironment) {
    let fork = Hardfork::get_active_fork_with_total_difficulty(chain_config, header.number, header.timestamp, total_difficulty);
    let config = get_evm_config(chain_config, header.number, header.timestamp, total_difficulty);

    let mut block_difficulty = EvmU256::from_big_endian(&header.difficulty.to_be_bytes::<32>());
    let mut block_randomness = None;
    if fork >= Hardfork::Paris {
        block_difficulty = EvmU256::zero();
        block_randomness = Some(H256::from_slice(header.mix_hash.as_slice()));
    }

    let env = InMemoryEnvironment {
        block_hashes: std::collections::BTreeMap::new(),
        block_number: EvmU256::from(header.number),
        block_timestamp: EvmU256::from(header.timestamp),
        block_gas_limit: EvmU256::from(header.gas_limit),
        block_coinbase: H160::from_slice(header.beneficiary.as_slice()),
        block_difficulty,
        block_base_fee_per_gas: EvmU256::from(header.base_fee_per_gas.unwrap_or(0)),
        block_randomness,
        chain_id: EvmU256::from(chain_id),
        blob_base_fee_per_gas: EvmU256::from(header.excess_blob_gas.map(|e| {
            if let Some(params) = fork.blob_params(chain_config) {
                calc_blob_gasprice(e, params.update_fraction)
            } else {
                calc_blob_gasprice(e, BLOB_GASPRICE_UPDATE_FRACTION as u128)
            }
        }).unwrap_or(0)),
        blob_versioned_hashes: vec![],
    };

    (fork, config, env)
}

pub fn calculate_intrinsic_gas(tx: &Transaction, fork: Hardfork) -> u64 {
    let config = match fork {
        Hardfork::Prague => Config::prague(),
        Hardfork::Cancun => Config::cancun(),
        Hardfork::Shanghai => Config::shanghai(),
        _ => Config::london(), // Default to London for others if not explicitly handled here
    };
    calculate_intrinsic_gas_with_config(tx, fork, &config)
}

pub fn calculate_intrinsic_gas_with_config(tx: &Transaction, fork: Hardfork, _config: &Config) -> u64 {
    let mut gas = 21000;
    if tx.to().is_none() {
        gas += 32000;
        if fork >= Hardfork::Shanghai {
            let initcode_len = tx.input().len() as u64;
            let initcode_cost = (initcode_len + 31) / 32 * 2;
            gas += initcode_cost;
        }
    }

    let non_zero_data_gas = if fork >= Hardfork::Istanbul { 16 } else { 68 };
    let mut tokens_in_calldata = 0u64;

    for &b in tx.input().iter() {
        if b == 0 {
            gas += 4;
            tokens_in_calldata += 1;
        } else {
            gas += non_zero_data_gas;
            tokens_in_calldata += 4;
        }
    }

    // EIP-2930 Access list gas
    if let Some(al) = tx.access_list() {
        for item in &al.0 {
            gas += 2400; // address
            gas += item.storage_keys.len() as u64 * 1900; // storage keys
        }
    }

    if let Some(auth_list) = tx.as_eip7702().and_then(|s| s.tx().authorization_list()) {
        gas += auth_list.len() as u64 * 2500;
    }

    // EIP-7623: Floor gas cost
    if fork >= Hardfork::Prague {
        let floor_cost = if tx.to().is_none() {
            // EIP-7623: contract creation floor = 53000 + tokens * 10
            53000 + tokens_in_calldata * 10
        } else {
            // EIP-7623: standard call floor = 21000 + tokens * 10
            21000 + tokens_in_calldata * 10
        };
        if gas < floor_cost {
            gas = floor_cost;
        }
    }

    gas
}
