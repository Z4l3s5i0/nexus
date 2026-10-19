use std::collections::{HashMap, HashSet};
use wasix_eth_types::*;
use wasix_eth_storage::read_traits::{AccountProviderSync, BytecodeProviderSync, StorageProviderSync};
use wasix_eth_storage::write::BatchWriter;
use evm::backend::{InMemoryEnvironment, RuntimeBaseBackend, RuntimeEnvironment};
use evm::uint::{H160, H256, U256 as EvmU256};
use evm::interpreter::runtime::RuntimeBackend;

#[derive(Clone)]
pub struct SputnikBackend<'a> {
    pub read_provider: &'a dyn AccountProviderSync,
    pub storage_provider: &'a dyn StorageProviderSync,
    pub bytecode_provider: &'a dyn BytecodeProviderSync,
    pub environment: InMemoryEnvironment,
    pub state_root: Option<B256>,
    pub transient_storage: HashMap<(H160, H256), H256>,
    pub hot_accounts: HashSet<H160>,
    pub hot_storage: HashSet<(H160, H256)>,
    pub origin: H160,
}

impl<'a> SputnikBackend<'a>{
    pub fn is_account_empty(acc: &TrieAccount) -> bool {
        acc.nonce == 0 && acc.balance == U256::ZERO && acc.code_hash == alloy_primitives::KECCAK256_EMPTY
    }

    pub fn check_delegation(&self, address: H160) -> Option<H160> {
        let addr = Address::from_slice(address.as_bytes());
        if let Ok(Some(acc)) = self.read_provider.account_sync(addr, self.state_root) {
            if acc.code_hash != alloy_primitives::KECCAK256_EMPTY {
                if let Ok(Some(code)) = self.bytecode_provider.bytecode_sync(acc.code_hash) {
                    if code.starts_with(&[0xef, 0x01, 0x00]) {
                        if code.len() == 23 {
                            return Some(H160::from_slice(&code[3..23]));
                        }
                    }
                }
            }
        }
        None
    }
}

impl<'a> RuntimeBaseBackend for SputnikBackend<'a> {
    fn balance(&self, address: H160) -> EvmU256 {
        let addr = Address::from_slice(address.as_bytes());
        match self.read_provider.account_sync(addr, self.state_root) {
            Ok(Some(acc)) => alloy_u256_to_evm_u256(acc.balance),
            _ => EvmU256::zero(),
        }
    }

    fn code(&self, address: H160) -> Vec<u8> {
        if let Some(target) = self.check_delegation(address) {
            return self.code(target);
        }
        let addr = Address::from_slice(address.as_bytes());
        match self.read_provider.account_sync(addr, self.state_root) {
            Ok(Some(acc)) => {
                if acc.code_hash == B256::ZERO
                    || acc.code_hash == alloy_primitives::KECCAK256_EMPTY {
                    return Vec::new();
                }
                match self.bytecode_provider.bytecode_sync(acc.code_hash) {
                    Ok(Some(code)) => code.to_vec(),
                    _ => {
                        wasix_eth_utils::debug!("[Backend] Bytecode not found for hash: {:?}", acc.code_hash);
                        Vec::new()
                    },
                }
            }
            _ => Vec::new(),
        }
    }

    fn exists(&self, address: H160) -> bool {
        let addr = Address::from_slice(address.as_bytes());

        match self.read_provider.account_sync(addr, self.state_root) {
            Ok(Some(acc)) => {
                !Self::is_account_empty(&acc)
            }
            _ => {
                false
            }
        }
    }

    fn nonce(&self, address: H160) -> EvmU256 {
        self.check_delegation(address);
        let addr = Address::from_slice(address.as_bytes());
        match self.read_provider.account_sync(addr, self.state_root) {
            Ok(Some(acc)) => EvmU256::from(acc.nonce),
            _ => EvmU256::zero(),
        }
    }

    fn storage(&self, address: H160, index: H256) -> H256 {
        if let Some(target) = self.check_delegation(address) {
            return self.storage(target, index);
        }
        let addr = Address::from_slice(address.as_bytes());
        let slot = B256::from_slice(index.as_bytes());
        let val = self.storage_provider.storage_sync(addr, slot, self.state_root).unwrap_or_default();
        H256::from_slice(&val.to_be_bytes::<32>())
    }

    fn transient_storage(&self, address: H160, index: H256) -> H256 {
        self.transient_storage.get(&(address, index)).cloned().unwrap_or_default()
    }
}

impl<'a> RuntimeBackend for SputnikBackend<'a> {
    fn original_storage(&self, address: H160, index: H256) -> H256 {
        self.storage(address, index)
    }

    fn deleted(&self, _address: H160) -> bool {
        false
    }

    fn created(&self, _address: H160) -> bool {
        false
    }

    fn is_cold(&self, address: H160, index: Option<H256>) -> bool {
        match index {
            Some(index) => !self.hot_storage.contains(&(address, index)),
            None => !self.hot_accounts.contains(&address),
        }
    }

    fn mark_hot(&mut self, address: H160, _kind: evm::interpreter::runtime::TouchKind) {
        self.hot_accounts.insert(address);
    }

    fn mark_storage_hot(&mut self, address: H160, index: H256) {
        if !self.hot_storage.contains(&(address, index)) {
            println!("[HOT_DEBUG] Marking storage {:?}:{:?} as hot", address, index);
        }
        self.hot_storage.insert((address, index));
    }

    fn set_storage(&mut self, _address: H160, _index: H256, _value: H256) -> Result<(), evm::interpreter::ExitError> {
        Ok(())
    }

    fn set_transient_storage(&mut self, address: H160, index: H256, value: H256) -> Result<(), evm::interpreter::ExitError> {
        self.transient_storage.insert((address, index), value);
        Ok(())
    }

    fn log(&mut self, _log: evm::interpreter::runtime::Log) -> Result<(), evm::interpreter::ExitError> {
        Ok(())
    }

    fn mark_delete_reset(&mut self, _address: H160) {
    }

    fn mark_create(&mut self, _address: H160) {
    }

    fn reset_storage(&mut self, _address: H160) {
    }

    fn set_code(&mut self, _address: H160, _code: Vec<u8>, _origin: evm::interpreter::runtime::SetCodeOrigin) -> Result<(), evm::interpreter::ExitError> {
        Ok(())
    }

    fn deposit(&mut self, _address: H160, _amount: EvmU256) {
    }

    fn withdrawal(&mut self, _address: H160, _amount: EvmU256) -> Result<(), evm::interpreter::ExitError> {
        Ok(())
    }

    fn inc_nonce(&mut self, _address: H160) -> Result<(), evm::interpreter::ExitError> {
        Ok(())
    }

}

impl<'a> RuntimeEnvironment for SputnikBackend<'a> {
    fn block_hash(&self, number: EvmU256) -> H256 {
        if let Some(hash) = self.environment.block_hashes.get(&number) {
            return *hash;
        }

        // EIP-2935: Serve historical block hashes from state
        let block_number = self.environment.block_number;
        let target_number = number;

        let target_u64 = target_number.low_u64();
        let current_u64 = block_number.low_u64();

        // 8192 is the window size
        if current_u64 > target_u64 && current_u64 <= target_u64 + 8192 {
            let history_storage_contract = H160::from_slice(HISTORY_STORAGE_ADDRESS.as_slice());
            let index = target_u64 % 8192;
            let slot = H256::from_slice(&B256::from(U256::from(index)).0);

            // Read from storage provider
            return self.storage(history_storage_contract, slot);
        }

        H256::zero()
    }

    fn block_number(&self) -> EvmU256 {
        self.environment.block_number
    }

    fn block_coinbase(&self) -> H160 {
        self.environment.block_coinbase
    }

    fn block_timestamp(&self) -> EvmU256 {
        self.environment.block_timestamp
    }

    fn block_difficulty(&self) -> EvmU256 {
        self.environment.block_difficulty
    }

    fn block_gas_limit(&self) -> EvmU256 {
        self.environment.block_gas_limit
    }

    fn block_base_fee_per_gas(&self) -> EvmU256 {
        self.environment.block_base_fee_per_gas
    }

    fn block_randomness(&self) -> Option<H256> {
        self.environment.block_randomness
    }

    fn blob_versioned_hash(&self, index: EvmU256) -> H256 {
        self.environment.blob_versioned_hashes.get(index.as_usize()).cloned().unwrap_or_default()
    }

    fn blob_base_fee_per_gas(&self) -> EvmU256 {
        self.environment.blob_base_fee_per_gas
    }

    fn chain_id(&self) -> EvmU256 {
        self.environment.chain_id
    }
}
