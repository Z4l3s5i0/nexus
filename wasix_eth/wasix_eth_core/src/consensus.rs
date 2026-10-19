use wasix_eth_types::{async_trait, Header, Block, Transaction, B256, U256, B64, EMPTY_OMMER_ROOT_HASH, Hardfork, ChainConfig, proofs, ConsensusTransaction, keccak256};
use std::sync::Arc;
use wasix_eth_storage::read::DatabaseReadProvider;
use anyhow::Result;
use wasix_eth_storage::read_traits::HeaderProvider;

#[async_trait]
pub trait Consensus: Send + Sync {
    async fn validate_header(&self, header: &Header, parent: &Header, chain_config: &ChainConfig) -> Result<(), String>;
    async fn validate_body(&self, block: &Block<Transaction>, chain_config: &ChainConfig) -> Result<(), String>;
    fn validate_block_post_execution(&self, block: &Block<Transaction>, cumulative_gas_used: u64, receipts_root: B256, logs_bloom: alloy_primitives::Bloom, calculated_state_root: B256) -> Result<(), String>;
    fn calculate_next_base_fee(&self, parent: &Header, chain_config: &ChainConfig) -> Option<u64>;
    async fn calculate_next_blob_base_fee(&self, parent: &Header, chain_config: &ChainConfig) -> Option<u128>;
    fn validate_cancun(&self, block: &Block<Transaction>, expected_blob_versioned_hashes: Option<&[B256]>) -> Result<(), String>;
    fn validate_parent_beacon_block_root(&self, header: &Header, parent_beacon_block_root: Option<B256>) -> Result<(), String>;
}

pub struct EthConsensus {
    read_storage: Arc<DatabaseReadProvider>,
}

impl EthConsensus {
    pub fn new(read_storage: Arc<DatabaseReadProvider>) -> Self {
        Self { read_storage }
    }
}

#[async_trait]
impl Consensus for EthConsensus {
    fn calculate_next_base_fee(&self, parent: &Header, _chain_config: &ChainConfig) -> Option<u64> {
        let parent_base_fee = parent.base_fee_per_gas?;
        let parent_gas_used = parent.gas_used;
        let parent_gas_target = parent.gas_limit / 2;

        if parent_gas_used == parent_gas_target {
            return Some(parent_base_fee);
        }

        if parent_gas_used > parent_gas_target {
            let gas_used_delta = parent_gas_used - parent_gas_target;
            let base_fee_delta = (parent_base_fee as u128 * gas_used_delta as u128) / parent_gas_target as u128 / 8;
            Some(parent_base_fee + (base_fee_delta as u64).max(1))
        } else {
            let gas_used_delta = parent_gas_target - parent_gas_used;
            let base_fee_delta = (parent_base_fee as u128 * gas_used_delta as u128) / parent_gas_target as u128 / 8;
            Some(parent_base_fee.saturating_sub(base_fee_delta as u64))
        }
    }

    async fn calculate_next_blob_base_fee(&self, parent: &Header, chain_config: &ChainConfig) -> Option<u128> {
        let parent_excess_blob_gas = parent.excess_blob_gas?;
        let parent_blob_gas_used = parent.blob_gas_used?;

        let td = self.read_storage.header_td(parent.hash_slow()).await.ok().flatten();
        let current_fork = Hardfork::get_active_fork_with_total_difficulty(chain_config, parent.number + 1, parent.timestamp + 1, td);
        
        let (target, update_fraction) = if let Some(params) = current_fork.blob_params(chain_config) {
             (params.target_blob_count * wasix_eth_types::DATA_GAS_PER_BLOB, params.update_fraction)
        } else {
             (wasix_eth_types::TARGET_BLOB_GAS_PER_BLOCK, wasix_eth_types::BLOB_GASPRICE_UPDATE_FRACTION as u128)
        };

        let new_excess_blob_gas = wasix_eth_types::calc_excess_blob_gas(Some(parent_excess_blob_gas), Some(parent_blob_gas_used), target);
        Some(wasix_eth_types::calc_blob_gasprice(new_excess_blob_gas, update_fraction))
    }

    async fn validate_header(&self, header: &Header, parent: &Header, chain_config: &ChainConfig) -> Result<(), String> {
        // 1. Validate block number
        if header.number != parent.number + 1 {
            return Err(format!("Invalid block number: expected {}, got {}", parent.number + 1, header.number));
        }

        // 2. Validate timestamp
        if header.timestamp <= parent.timestamp {
            return Err(format!("Invalid timestamp: must be greater than parent ({} <= {})", header.timestamp, parent.timestamp));
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if header.timestamp > now + 15 {
            return Err(format!("Invalid timestamp: block is too far in the future ({} > {} + 15)", header.timestamp, now));
        }
        
        // 3. Validate gas limit
        let parent_gas_limit = parent.gas_limit;
        let gas_limit = header.gas_limit;

        if gas_limit < 5000 {
            return Err(format!("Gas limit too low: {} < 5000", gas_limit));
        }

        let td = self.read_storage.header_td(parent.hash_slow()).await.ok().flatten();
        let parent_fork = Hardfork::get_active_fork_with_total_difficulty(chain_config, parent.number, parent.timestamp, td);
        let current_fork = Hardfork::get_active_fork_with_total_difficulty(chain_config, header.number, header.timestamp, td);

        let is_london_transition = parent_fork < Hardfork::London && current_fork >= Hardfork::London;

        let (target_gas_limit, limit_delta) = if is_london_transition {
            // EIP-1559: gas limit doubles at the London transition
            let target = parent_gas_limit * 2;
            (target, target / 1024)
        } else {
            (parent_gas_limit, parent_gas_limit / 1024)
        };

        let diff = if gas_limit > target_gas_limit {
            gas_limit - target_gas_limit
        } else {
            target_gas_limit - gas_limit
        };

        if diff >= limit_delta {
            return Err(format!("Invalid gas limit change: parent={}, current={}, diff={}, max_diff={}, is_london_transition={}", 
                parent_gas_limit, gas_limit, diff, limit_delta.saturating_sub(1), is_london_transition));
        }

        // 4. Validate gas used
        if header.gas_used > header.gas_limit {
            return Err(format!("Gas used exceeds gas limit: used={}, limit={}", header.gas_used, header.gas_limit));
        }

        // 5. Validate extra data
        if header.extra_data.len() > 32 {
            return Err(format!("Extra data too long: {} > 32", header.extra_data.len()));
        }

        // 6. Validate base fee
        if current_fork >= Hardfork::London {
            let actual_base_fee = header.base_fee_per_gas.ok_or_else(|| "Base fee missing in post-London block".to_string())?;

            if let Some(expected_base_fee) = self.calculate_next_base_fee(parent, chain_config) {
                // Special case for transition block: if parent has NO base fee, expected is 1 Gwei
                let expected_base_fee = if parent.base_fee_per_gas.is_none() {
                    1_000_000_000 // 1 Gwei INITIAL_BASE_FEE
                } else {
                    expected_base_fee
                };

                if actual_base_fee != expected_base_fee {
                    return Err(format!("Invalid base fee: expected {}, got {}", expected_base_fee, actual_base_fee));
                }
            }
        }

        // 7. Validate Merge/Post-Merge constraints
        if current_fork >= Hardfork::Paris {
            if header.difficulty != U256::ZERO {
                return Err(format!("Post-merge block must have difficulty 0, got {}", header.difficulty));
            }
            if header.nonce != B64::ZERO {
                return Err(format!("Post-merge block must have nonce 0, got {:?}", header.nonce));
            }
            if header.ommers_hash != EMPTY_OMMER_ROOT_HASH {
                return Err(format!("Post-merge block must have empty ommers hash, got {:?}", header.ommers_hash));
            }
        } 

        // 8. Validate Shanghai/Cancun/Prague fields
        if current_fork >= Hardfork::Shanghai {
            if header.withdrawals_root.is_none() {
                return Err("Shanghai active: withdrawals_root must be Some".to_string());
            }
        } else if header.withdrawals_root.is_some() {
            return Err("Shanghai not active: withdrawals_root must be None".to_string());
        }

        if current_fork >= Hardfork::Cancun {
            if header.blob_gas_used.is_none() {
                return Err("Cancun active: blob_gas_used must be Some".to_string());
            }
            if header.excess_blob_gas.is_none() {
                return Err("Cancun active: excess_blob_gas must be Some".to_string());
            }
            if header.parent_beacon_block_root.is_none() {
                return Err("Cancun active: parent_beacon_block_root must be Some".to_string());
            }

            let params = current_fork.blob_params(chain_config);
            let target = params.map(|p| p.target_blob_count * wasix_eth_types::DATA_GAS_PER_BLOB).unwrap_or(wasix_eth_types::TARGET_BLOB_GAS_PER_BLOCK);
            let max = params.map(|p| p.max_blob_count * wasix_eth_types::DATA_GAS_PER_BLOB).unwrap_or(wasix_eth_types::MAX_BLOB_GAS_PER_BLOCK);

            if parent_fork.is_cancun_active() {
                let expected_excess = wasix_eth_types::calc_excess_blob_gas(
                    parent.excess_blob_gas,
                    parent.blob_gas_used,
                    target,
                );
                if header.excess_blob_gas.unwrap() != expected_excess {
                    return Err(format!("Invalid excess blob gas: expected {}, got {}", expected_excess, header.excess_blob_gas.unwrap()));
                }
            } else if header.excess_blob_gas.unwrap() != 0 {
                return Err(format!("Invalid excess blob gas for Cancun transition: expected 0, got {}", header.excess_blob_gas.unwrap()));
            }

            if header.blob_gas_used.unwrap() > max {
                return Err(format!("Blob gas used exceeds maximum: used={}, max={}", header.blob_gas_used.unwrap(), max));
            }
        } else {
            if header.blob_gas_used.is_some() {
                return Err("Cancun not active: blob_gas_used must be None".to_string());
            }
            if header.excess_blob_gas.is_some() {
                return Err("Cancun not active: excess_blob_gas must be None".to_string());
            }
            if header.parent_beacon_block_root.is_some() {
                return Err("Cancun not active: parent_beacon_block_root must be None".to_string());
            }
        }

        if current_fork >= Hardfork::Prague {
            if header.requests_hash.is_none() {
                return Err("Prague active: requests_hash must be Some".to_string());
            }
        } else if header.requests_hash.is_some() {
            return Err("Prague not active: requests_hash must be None".to_string());
        }

        Ok(())
    }

    async fn validate_body(&self, block: &Block<Transaction>, chain_config: &ChainConfig) -> Result<(), String> {
        // 0. Resolve fork for Prague validation
        // Note: For post-merge blocks (which Engine API handles), TD is usually not needed for fork resolution
        // as forks are timestamp-based. If we don't have the parent TD, we assume merge has happened
        // if the block number/timestamp suggests a post-merge fork.
        let td = self.read_storage.header_td(block.header.parent_hash).await.ok().flatten();
        let current_fork = Hardfork::get_active_fork_with_total_difficulty(chain_config, block.header.number, block.header.timestamp, td);
        let is_prague = current_fork >= Hardfork::Prague;

        // 1. Validate transactions root
        let tx_root = proofs::calculate_transaction_root(&block.body.transactions);
        if tx_root != block.header.transactions_root {
            return Err(format!("Invalid transactions root: expected {}, got {}", block.header.transactions_root, tx_root));
        }

        // 2. Validate ommers hash
        let ommers_hash = keccak256(alloy_rlp::encode(&block.body.ommers));
        if ommers_hash != block.header.ommers_hash {
            return Err(format!("Invalid ommers hash: expected {}, got {}", block.header.ommers_hash, ommers_hash));
        }

        // 3. Validate withdrawals root
        if let Some(withdrawals) = &block.body.withdrawals {
            let withdrawals_root = proofs::calculate_withdrawals_root(withdrawals);
            if Some(withdrawals_root) != block.header.withdrawals_root {
                return Err(format!("Invalid withdrawals root: expected {:?}, got {:?}", block.header.withdrawals_root, Some(withdrawals_root)));
            }
        } else if block.header.withdrawals_root.is_some() {
            return Err("Withdrawals missing in body but present in header".to_string());
        }

        // 4. Validate blob gas used (Cancun)
        let mut calculated_blob_gas_used = 0u64;
        for tx in &block.body.transactions {
            if let Transaction::Eip4844(signed_tx) = tx {
                // EIP-4844: blob_gas_used is DATA_GAS_PER_BLOB * num_blobs
                let num_blobs = signed_tx.tx().blob_versioned_hashes().map(|h| h.len()).unwrap_or(0);
                calculated_blob_gas_used += num_blobs as u64 * 131072;
            }
        }

        if let Some(header_blob_gas_used) = block.header.blob_gas_used {
            if calculated_blob_gas_used != header_blob_gas_used {
                return Err(format!("Invalid blob gas used: expected {}, got {}", header_blob_gas_used, calculated_blob_gas_used));
            }
        } else if calculated_blob_gas_used > 0 {
            return Err("Blob transactions present but blob_gas_used is missing in header".to_string());
        }

        // 5. Validate EIP-7702 transactions (Prague)
        for tx in &block.body.transactions {
            if let Transaction::Eip7702(_) = tx {
                if !is_prague {
                    return Err("EIP-7702 transaction present before Prague activation".to_string());
                }
            }
        }

        Ok(())
    }

    fn validate_block_post_execution(&self, block: &Block<Transaction>, cumulative_gas_used: u64, receipts_root: B256, logs_bloom: alloy_primitives::Bloom, calculated_state_root: B256) -> Result<(), String> {
        if block.header.gas_used != cumulative_gas_used {
            return Err(format!("Gas used mismatch: expected {}, got {}", block.header.gas_used, cumulative_gas_used));
        }

        if block.header.receipts_root != receipts_root {
            return Err(format!("Receipts root mismatch: expected {:?}, got {:?}", block.header.receipts_root, receipts_root));
        }

        if block.header.logs_bloom != logs_bloom {
            return Err(format!("Logs bloom mismatch: expected {:?}, got {:?}", block.header.logs_bloom, logs_bloom));
        }

        if block.header.state_root != calculated_state_root {
            return Err(format!("State root mismatch: expected {:?}, got {:?}", block.header.state_root, calculated_state_root));
        }

        Ok(())
    }

    fn validate_cancun(&self, block: &Block<Transaction>, expected_blob_versioned_hashes: Option<&[B256]>) -> Result<(), String> {
        if let Some(expected_hashes) = expected_blob_versioned_hashes {
            let mut actual_hashes = Vec::new();
            for tx in &block.body.transactions {
                if let Transaction::Eip4844(tx_4844) = tx {
                    if let Some(h) = tx_4844.tx().blob_versioned_hashes() {
                        actual_hashes.extend(h.iter().cloned());
                    }
                }
            }
            if &actual_hashes != expected_hashes {
                return Err(format!("Blob versioned hashes mismatch: expected {:?}, actual {:?}", expected_hashes, actual_hashes));
            }
        }
        Ok(())
    }

    fn validate_parent_beacon_block_root(&self, header: &Header, parent_beacon_block_root: Option<B256>) -> Result<(), String> {
        if let Some(root) = parent_beacon_block_root {
            if header.parent_beacon_block_root != Some(root) {
                return Err(format!("Parent beacon block root mismatch: expected {:?}, actual {:?}", root, header.parent_beacon_block_root));
            }
        }
        Ok(())
    }
}
