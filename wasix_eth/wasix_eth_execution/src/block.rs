use wasix_eth_types::*;
use wasix_eth_types::consensus_constants as constants;
use alloy_primitives::keccak256;
use wasix_eth_storage::write::BatchWriter;
use wasix_eth_storage::read_traits::AccountProvider;
use wasix_eth_storage::write_traits::{AccountWriter, BytecodeWriter, StorageWriter};
use wasix_eth_utils::{debug, info};
// use evm::uint::{H160, H256}; // Removed unused import
use anyhow::Result;
use sha2::{Sha256, Digest};

pub struct BlockProcessor<'a> {
    pub batch: &'a BatchWriter,
}

impl<'a> BlockProcessor<'a> {
    pub fn new(batch: &'a BatchWriter) -> Self {
        Self { batch }
    }

    pub fn apply_beacon_root(&self, fork: Hardfork, beacon_root: B256, timestamp: u64, state_root: Option<B256>) -> Result<u64> {
        if fork < Hardfork::Cancun {
            return Ok(0);
        }

        let beacon_root_contract = BEACON_ROOTS_ADDRESS;

        // Ensure the contract exists with correct code
        self.ensure_system_contract_code(beacon_root_contract, &BEACON_ROOTS_CODE, state_root)?;

        let index = timestamp % 8191;
        let timestamp_slot = B256::from(U256::from(index));
        let root_slot = B256::from(U256::from(index + 8191));
        
        self.batch.update_storage(beacon_root_contract, timestamp_slot, U256::from(timestamp))?;
        self.batch.update_storage(beacon_root_contract, root_slot, U256::from_be_bytes(beacon_root.0))?;
        self.batch.mark_account_touched(beacon_root_contract)?;

        Ok(0)
    }

    pub fn ensure_system_contract_code(&self, address: Address, code: &[u8], state_root: Option<B256>) -> Result<()> {
        let account = self.batch.account(address, state_root)?;
        let is_new = account.is_none();
        let mut contract_acc = account.unwrap_or(TrieAccount {
            nonce: 1,
            balance: U256::ZERO,
            storage_root: EMPTY_ROOT_HASH,
            code_hash: alloy_primitives::KECCAK256_EMPTY,
        });
        
        let code_hash = keccak256(code);
        
        if is_new || contract_acc.code_hash != code_hash {
            contract_acc.code_hash = code_hash;
            if is_new {
                contract_acc.nonce = 1;
            }
            self.batch.insert_bytecode(code_hash, code.to_vec().into())?;
            self.batch.update_account(address, contract_acc)?;
        }
        Ok(())
    }

    pub fn initialize_prague_system_contracts(&self, fork: Hardfork, state_root: Option<B256>) -> Result<u64> {
        if fork < Hardfork::Prague {
            return Ok(0);
        }

        self.ensure_system_contract_code(HISTORY_STORAGE_ADDRESS, &HISTORY_STORAGE_CODE, state_root)?;
        self.ensure_system_contract_code(WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, &WITHDRAWAL_REQUEST_PREDEPLOY_CODE, state_root)?;
        self.ensure_system_contract_code(CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS, &CONSOLIDATION_REQUEST_PREDEPLOY_CODE, state_root)?;
        info!("[Execution] Prague system contracts initialized");

        Ok(0)
    }

    pub fn process_withdrawals(&self, withdrawals: &[alloy_eips::eip4895::Withdrawal], state_root: Option<B256>) -> Result<()> {
        if withdrawals.is_empty() {
            return Ok(());
        }
        debug!("[Execution] Processing {} withdrawals", withdrawals.len());
        for withdrawal in withdrawals {
            let mut account = self.batch.account(withdrawal.address, state_root)?.unwrap_or(TrieAccount {
                nonce: 0,
                balance: U256::ZERO,
                storage_root: EMPTY_ROOT_HASH,
                code_hash: alloy_primitives::KECCAK256_EMPTY,
            });
            
            let amount_wei = U256::from(withdrawal.amount).saturating_mul(U256::from(1_000_000_000u64));
            account.balance = account.balance.saturating_add(amount_wei);
            
            account.storage_root = self.batch.calculate_storage_root(withdrawal.address, state_root)?;
            if account.code_hash == B256::default() {
                account.code_hash = alloy_primitives::KECCAK256_EMPTY;
            }

            self.batch.update_account(withdrawal.address, account)?;
        }
        Ok(())
    }

    pub fn apply_block_rewards(&self, fork: Hardfork, beneficiary: Address, ommers: &[Header], state_root: Option<B256>, block_number: u64) -> Result<()> {
        if fork >= Hardfork::Paris {
            return Ok(());
        }

        let block_reward = match fork {
            Hardfork::Frontier | Hardfork::Homestead | Hardfork::TangerineWhistle | Hardfork::SpuriousDragon => U256::from(5) * U256::from(10u64.pow(18)),
            Hardfork::Byzantium => U256::from(3) * U256::from(10u64.pow(18)),
            Hardfork::Constantinople | Hardfork::Petersburg | Hardfork::Istanbul | Hardfork::MuirGlacier | Hardfork::Berlin | Hardfork::London | Hardfork::ArrowGlacier | Hardfork::GrayGlacier => U256::from(2) * U256::from(10u64.pow(18)),
            _ => U256::ZERO,
        };

        if block_reward > U256::ZERO {
            let mut main_reward = block_reward;
            let ommer_inclusion_reward = block_reward / U256::from(32);
            for _ in ommers {
                main_reward = main_reward.saturating_add(ommer_inclusion_reward);
            }

            let mut beneficiary_account = self.batch.account(beneficiary, state_root)?.unwrap_or_else(|| {
                TrieAccount {
                    nonce: 0,
                    balance: U256::ZERO,
                    storage_root: EMPTY_ROOT_HASH,
                    code_hash: alloy_primitives::KECCAK256_EMPTY,
                }
            });
            
            if beneficiary_account.code_hash == B256::default() {
                beneficiary_account.code_hash = alloy_primitives::KECCAK256_EMPTY;
            }

            beneficiary_account.balance = beneficiary_account.balance.saturating_add(main_reward);
            self.batch.update_account(beneficiary, beneficiary_account)?;

            // Ommer rewards
            for ommer in ommers {
                let ommer_reward = block_reward.saturating_mul(U256::from(8 + ommer.number - block_number)) / U256::from(8);
                let mut ommer_account = self.batch.account(ommer.beneficiary, state_root)?.unwrap_or_else(|| {
                    TrieAccount {
                        nonce: 0,
                        balance: U256::ZERO,
                        storage_root: EMPTY_ROOT_HASH,
                        code_hash: alloy_primitives::KECCAK256_EMPTY,
                    }
                });
                if ommer_account.code_hash == B256::default() {
                    ommer_account.code_hash = alloy_primitives::KECCAK256_EMPTY;
                }
                ommer_account.balance = ommer_account.balance.saturating_add(ommer_reward);
                self.batch.update_account(ommer.beneficiary, ommer_account)?;
            }
        }
        Ok(())
    }

    // pub fn finalize_block_header(
    //     &self,
    //     block: &mut Block<Transaction>,
    //     receipts: &[Receipt],
    //     cumulative_gas_used: u64,
    //     calculated_root: B256,
    //     fork: Hardfork,
    //     additional_requests: &[Vec<u8>],
    // ) -> Result<()> {
    //     let transactions_root = proofs::calculate_transaction_root(&block.body.transactions);
    //     let receipts_root = calculate_receipt_root(receipts);
    //
    //     let mut logs_bloom = Bloom::default();
    //     for receipt in receipts {
    //         logs_bloom.accrue_bloom(&receipt.logs_bloom);
    //     }
    //
    //     // Determine if we should update or validate based on the presence of values.
    //     let is_building = block.header.state_root == B256::ZERO
    //         || (block.header.gas_used == 0 && cumulative_gas_used > 0);
    //
    //     if is_building {
    //         block.header.gas_used = cumulative_gas_used;
    //         block.header.transactions_root = transactions_root;
    //         block.header.receipts_root = receipts_root;
    //         block.header.logs_bloom = logs_bloom;
    //         block.header.state_root = calculated_root;
    //
    //         if fork >= Hardfork::Shanghai && block.header.withdrawals_root.is_none() {
    //             block.header.withdrawals_root = Some(EMPTY_ROOT_HASH);
    //         }
    //
    //         if fork >= Hardfork::Cancun {
    //             // Calculate actual blob gas used
    //             let mut blob_gas_used = 0u64;
    //             for tx in &block.body.transactions {
    //                 if let Transaction::Eip4844(s) = tx {
    //                     blob_gas_used += s.tx().blob_versioned_hashes().unwrap_or_default().len() as u64 * DATA_GAS_PER_BLOB;
    //                 }
    //             }
    //
    //             if block.header.blob_gas_used.is_none() {
    //                 block.header.blob_gas_used = Some(blob_gas_used);
    //             }
    //             if block.header.excess_blob_gas.is_none() {
    //                 block.header.excess_blob_gas = Some(0);
    //             }
    //             if block.header.parent_beacon_block_root.is_none() {
    //                 block.header.parent_beacon_block_root = Some(B256::ZERO);
    //             }
    //         }
    //
    //         if fork >= Hardfork::Prague {
    //             if block.header.requests_hash.is_none() {
    //                 let mut all_requests = Vec::new();
    //                 let deposits = self.collect_deposits(receipts);
    //                 for deposit in deposits {
    //                     let mut out = Vec::new();
    //                     eip6110_utils::encode_deposit_request(&deposit, &mut out);
    //                     let mut request = Vec::with_capacity(1 + out.len());
    //                     request.push(0x00u8);
    //                     request.extend_from_slice(&out);
    //                     all_requests.push(request);
    //                 }
    //                 all_requests.extend(additional_requests.iter().cloned());
    //                 block.header.requests_hash = Some(self.calculate_requests_hash(fork, &all_requests));
    //             }
    //         }
    //     } else {
    //         if block.header.gas_used != cumulative_gas_used {
    //             return Err(anyhow::anyhow!("Gas used mismatch: expected {}, calculated {}", block.header.gas_used, cumulative_gas_used));
    //         }
    //         if block.header.transactions_root != transactions_root {
    //             return Err(anyhow::anyhow!("Transactions root mismatch"));
    //         }
    //         if block.header.receipts_root != receipts_root {
    //             return Err(anyhow::anyhow!("Receipts root mismatch"));
    //         }
    //         if block.header.state_root != calculated_root {
    //             return Err(anyhow::anyhow!("State root mismatch: expected {:?}, calculated {:?}", block.header.state_root, calculated_root));
    //         }
    //     }
    //
    //     Ok(())
    // }
    //
    // fn collect_deposits(&self, receipts: &[Receipt]) -> Vec<DepositRequest> {
    //     let mut deposits = Vec::new();
    //     for receipt in receipts {
    //         for log in &receipt.receipt.logs {
    //             if log.address == DEPOSIT_CONTRACT_ADDRESS {
    //                 if let Ok(deposit) = eip6110_utils::decode_deposit_log(&log.data.data.0) {
    //                     deposits.push(deposit);
    //                 }
    //             }
    //         }
    //     }
    //     deposits
    // }

    pub fn calculate_requests_hash(&self, fork: Hardfork, requests: &[Vec<u8>]) -> B256 {
        let mut filtered_requests: Vec<Vec<u8>> = requests.iter()
            .cloned()
            .collect();

        if filtered_requests.is_empty() {
            return constants::EMPTY_REQUESTS_HASH;
        }

        // Sort by type (first byte) only from Prague
        if fork >= Hardfork::Prague {
            filtered_requests.sort_by_key(|r| r[0]);
        }

        let mut hasher = Sha256::new();
        for request in &filtered_requests {
            hasher.update(Sha256::digest(request));
        }
        B256::from_slice(&hasher.finalize())
    }
}
