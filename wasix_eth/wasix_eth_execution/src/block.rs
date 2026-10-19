use alloy_eips::eip4895::Withdrawal;
use wasix_eth_types::*;
use wasix_eth_types::constants::*;
use wasix_eth_storage::write::BatchWriter;
use wasix_eth_storage::read_traits::AccountProvider;
use wasix_eth_storage::write_traits::{AccountWriter, BytecodeWriter, StorageWriter};
use wasix_eth_utils::debug;
use anyhow::Result;

const BEACON_ROOTS_ADDRESS: Address = Address::new([
    0x00, 0x00, 0x00, 0x00, 0x21, 0x9a, 0xb5, 0x40, 0x35, 0x6c, 0xBB, 0x83, 0x9C, 0xbe, 0x05, 0x30,
    0x3d, 0x77, 0x05, 0xFa,
]);
const BEACON_ROOTS_CODE: [u8; 0] = []; // Placeholder, adjust if known
const HISTORY_STORAGE_PRUNING_WINDOW: u64 = 8192;

pub struct BlockProcessor<'a> {
    pub batch: &'a BatchWriter,
}

impl<'a> BlockProcessor<'a> {
    pub fn new(batch: &'a BatchWriter) -> Self {
        Self { batch }
    }

    /// EIP-4788: Beacon block root in the Ethereum state
    /// The parent beacon block root is stored in a contract at a specific address.
    pub async fn apply_beacon_root(&self, fork: Hardfork, beacon_root: B256, timestamp: u64, state_root: Option<B256>) -> Result<u64> {
        if fork < Hardfork::Cancun {
            return Ok(0);
        }

        let beacon_root_contract = BEACON_ROOTS_ADDRESS;
        
        // Ensure contract code is present
        self.ensure_system_contract_code(beacon_root_contract, &BEACON_ROOTS_CODE, state_root).await?;

        let timestamp_slot = B256::from(U256::from(timestamp % HISTORY_STORAGE_PRUNING_WINDOW));
        let root_slot = B256::from(U256::from(timestamp % HISTORY_STORAGE_PRUNING_WINDOW) + U256::from(HISTORY_STORAGE_PRUNING_WINDOW));

        self.batch.update_storage(beacon_root_contract, timestamp_slot, U256::from(timestamp)).await?;
        self.batch.update_storage(beacon_root_contract, root_slot, U256::from_be_bytes(beacon_root.0)).await?;
        
        Ok(0)
    }

    pub async fn ensure_system_contract_code(&self, address: Address, code: &[u8], state_root: Option<B256>) -> Result<()> {
        let account = self.batch.account(address, state_root).await?;
        let code_hash = keccak256(code);
        
        let needs_init = match account {
            Some(acc) => acc.code_hash == B256::ZERO || acc.code_hash == alloy_primitives::KECCAK256_EMPTY,
            None => true,
        };

        if needs_init {
            let mut contract_acc = account.unwrap_or(TrieAccount {
                nonce: 1,
                balance: U256::ZERO,
                storage_root: EMPTY_ROOT_HASH,
                code_hash,
            });
            contract_acc.code_hash = code_hash;
            
            self.batch.insert_bytecode(code_hash, code.to_vec().into()).await?;
            self.batch.update_account(address, contract_acc).await?;
        }
        Ok(())
    }

    pub async fn process_withdrawals(&self, withdrawals: &[Withdrawal], state_root: Option<B256>) -> Result<()> {
        for withdrawal in withdrawals {
            let amount_wei = U256::from(withdrawal.amount) * U256::from(10u128.pow(9));
            let mut account = self.batch.account(withdrawal.address, state_root).await?.unwrap_or(TrieAccount {
                nonce: 0,
                balance: U256::ZERO,
                storage_root: EMPTY_ROOT_HASH,
                code_hash: alloy_primitives::KECCAK256_EMPTY,
            });
            account.balance += amount_wei;
            self.batch.update_account(withdrawal.address, account).await?;
        }
        Ok(())
    }

    pub async fn apply_block_rewards(&self, fork: Hardfork, beneficiary: Address, ommers: &[Header], state_root: Option<B256>, block_number: u64) -> Result<()> {
        if fork >= Hardfork::Paris {
            return Ok(());
        }

        let base_reward = self.get_base_reward(fork);
        let mut beneficiary_account = self.batch.account(beneficiary, state_root).await?.unwrap_or_default();
        
        beneficiary_account.balance += base_reward;
        
        for ommer in ommers {
            let ommer_reward = base_reward * U256::from(8 + ommer.number - block_number) / U256::from(8);
            let mut ommer_beneficiary = self.batch.account(ommer.beneficiary, state_root).await?.unwrap_or_default();
            ommer_beneficiary.balance += ommer_reward;
            self.batch.update_account(ommer.beneficiary, ommer_beneficiary).await?;
            
            beneficiary_account.balance += base_reward / U256::from(32);
        }
        
        self.batch.update_account(beneficiary, beneficiary_account).await?;
        Ok(())
    }

    fn get_base_reward(&self, fork: Hardfork) -> U256 {
        if fork >= Hardfork::Constantinople {
            U256::from(2) * U256::from(10u128.pow(18))
        } else if fork >= Hardfork::Byzantium {
            U256::from(3) * U256::from(10u128.pow(18))
        } else {
            U256::from(5) * U256::from(10u128.pow(18))
        }
    }
}
