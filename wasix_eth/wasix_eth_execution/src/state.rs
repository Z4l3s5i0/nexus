use wasix_eth_types::*;
use wasix_eth_storage::write::BatchWriter;
use wasix_eth_storage::read_traits::{AccountProvider, StorageProvider};
use wasix_eth_storage::write_traits::{AccountWriter, BytecodeWriter, StorageWriter};
use wasix_eth_utils::debug;
use evm::backend::OverlayedChangeSet;
use anyhow::Result;
use crate::backend::SputnikBackend;

pub struct StateApplier<'a> {
    pub batch: &'a BatchWriter,
}

impl<'a> StateApplier<'a> {
    pub fn new(batch: &'a BatchWriter) -> Self {
        Self { batch }
    }

    pub fn apply_changeset(
        &self,
        changeset: &OverlayedChangeSet,
        eip161: bool,
        state_root: Option<B256>,
    ) -> Result<()> {
        debug!("[Execution] Applying changeset: balances={}, nonces={}, codes={}, storages={}, deletes={}",
                changeset.balances.len(), changeset.nonces.len(), changeset.codes.len(), changeset.storages.len(), changeset.deletes.len());

        let mut affected_accounts = std::collections::BTreeSet::new();

        // 1. Balances
        for (address, balance) in &changeset.balances {
            let addr = Address::from_slice(address.as_bytes());
            affected_accounts.insert(*address);
            let mut trie_account = self.get_or_materialize_account(addr, state_root)?;

            let final_balance = {
                let mut bytes = [0u8; 32];
                balance.to_big_endian(&mut bytes);
                U256::from_be_bytes(bytes)
            };

            trie_account.balance = final_balance;
            self.batch.update_account(addr, trie_account)?;
        }

        // 2. Nonces
        for (address, nonce) in &changeset.nonces {
            let addr = Address::from_slice(address.as_bytes());
            affected_accounts.insert(*address);
            let mut trie_account = self.get_or_materialize_account(addr, state_root)?;
            debug!("[Execution] Nonce update: addr={:?}, old={}, new={}", addr, trie_account.nonce, nonce.as_u64());
            trie_account.nonce = nonce.as_u64();
            self.batch.update_account(addr, trie_account)?;
        }

        // 3. Codes
        for (address, code) in &changeset.codes {
            let addr = Address::from_slice(address.as_bytes());
            affected_accounts.insert(*address);
            let mut trie_account = self.get_or_materialize_account(addr, state_root)?;
            let code_hash = keccak256(code.as_slice());
            debug!("[Execution] Code update: addr={:?}, hash={:?}", addr, code_hash);
            trie_account.code_hash = code_hash;
            self.batch.insert_bytecode(code_hash, code.clone().into())?;
            self.batch.update_account(addr, trie_account)?;
        }

        // 4. Storage Resets
        for address in &changeset.storage_resets {
            let addr = Address::from_slice(address.as_bytes());
            affected_accounts.insert(*address);
            self.batch.reset_storage(addr)?;
        }

        // 5. Storages
        for ((address, slot), value) in &changeset.storages {
            let addr = Address::from_slice(address.as_bytes());
            affected_accounts.insert(*address);
            let slot_b256 = B256::from_slice(slot.as_bytes());
            let val_u256 = U256::from_be_bytes(value.0);
            
            self.batch.storage(addr, slot_b256, state_root).unwrap_or_default();

            self.batch.update_storage(addr, slot_b256, val_u256)?;
        }

        for address in &changeset.touched { affected_accounts.insert(*address); }
        for address in &changeset.deletes { affected_accounts.insert(*address); }

        for address in affected_accounts {
            let addr = Address::from_slice(address.as_bytes());
            if changeset.deletes.contains(&address) {
                self.batch.account(addr, state_root)?;
                self.batch.remove_account(addr)?;
                continue;
            }

            if let Some(mut trie_account) = self.batch.account(addr, state_root)? {
                let new_storage_root = self.batch.calculate_storage_root(addr, state_root)?;

                trie_account.storage_root = new_storage_root;
                if trie_account.code_hash == B256::default() {
                    trie_account.code_hash = alloy_primitives::KECCAK256_EMPTY;
                }
                

                if eip161 && SputnikBackend::is_account_empty(&trie_account) {
                    debug!("[Execution] Removing empty account: addr={:?}", addr);
                    self.batch.remove_account(addr)?;
                } else {
                    self.batch.update_account(addr, trie_account)?;
                }
            } else {
                 let trie_account = TrieAccount {
                    nonce: 0,
                    balance: U256::ZERO,
                    storage_root: EMPTY_ROOT_HASH,
                    code_hash: alloy_primitives::KECCAK256_EMPTY,
                 };
                 if !SputnikBackend::is_account_empty(&trie_account) {
                    self.batch.update_account(addr, trie_account)?;
                 }
            }
        }

        Ok(())
    }

    fn get_or_materialize_account(
        &self,
        addr: Address,
        state_root: Option<B256>,
    ) -> Result<TrieAccount> {
        Ok(self.batch.account(addr, state_root)?.unwrap_or_else(|| {
            TrieAccount {
                nonce: 0,
                balance: U256::ZERO,
                storage_root: EMPTY_ROOT_HASH,
                code_hash: alloy_primitives::KECCAK256_EMPTY,
            }
        }))
    }
}
