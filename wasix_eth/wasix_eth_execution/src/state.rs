use wasix_eth_types::*;
use wasix_eth_storage::write::BatchWriter;
use wasix_eth_storage::read_traits::{AccountProvider, StorageProvider};
use wasix_eth_storage::write_traits::{AccountWriter, BytecodeWriter, StorageWriter, StateWriter};
use wasix_eth_utils::debug;
use evm::backend::OverlayedChangeSet;
use anyhow::Result;
use crate::backend::SputnikBackend;
use alloy_rlp::Encodable;

pub struct StateApplier<'a> {
    pub batch: &'a BatchWriter,
}

impl<'a> StateApplier<'a> {
    pub fn new(batch: &'a BatchWriter) -> Self {
        Self { batch }
    }

    pub async fn apply_changeset(
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
            let mut trie_account = self.get_or_materialize_account(addr, state_root).await?;

            let final_balance = {
                let mut bytes = [0u8; 32];
                balance.to_big_endian(&mut bytes);
                U256::from_be_bytes(bytes)
            };

            trie_account.balance = final_balance;
            self.batch.update_account(addr, trie_account).await?;
            
            // Sync PlainState and HashedState
            let mut acc_rlp = Vec::new();
            trie_account.encode(&mut acc_rlp);
            let acc_rlp_bytes = Bytes::from(acc_rlp);
            self.batch.update_plain_state(addr, acc_rlp_bytes.clone()).await?;
            self.batch.update_hashed_state(keccak256(addr), acc_rlp_bytes).await?;
        }

        // 2. Nonces
        for (address, nonce) in &changeset.nonces {
            let addr = Address::from_slice(address.as_bytes());
            affected_accounts.insert(*address);
            let mut trie_account = self.get_or_materialize_account(addr, state_root).await?;
            debug!("[Execution] Nonce update: addr={:?}, old={}, new={}", addr, trie_account.nonce, nonce.as_u64());
            trie_account.nonce = nonce.as_u64();
            self.batch.update_account(addr, trie_account).await?;
            
            // Sync PlainState and HashedState
            let mut acc_rlp = Vec::new();
            trie_account.encode(&mut acc_rlp);
            let acc_rlp_bytes = Bytes::from(acc_rlp);
            self.batch.update_plain_state(addr, acc_rlp_bytes.clone()).await?;
            self.batch.update_hashed_state(keccak256(addr), acc_rlp_bytes).await?;
        }

        // 3. Codes
        for (address, code) in &changeset.codes {
            let addr = Address::from_slice(address.as_bytes());
            affected_accounts.insert(*address);
            let mut trie_account = self.get_or_materialize_account(addr, state_root).await?;
            let code_hash = keccak256(code.as_slice());
            debug!("[Execution] Code update: addr={:?}, hash={:?}", addr, code_hash);
            trie_account.code_hash = code_hash;
            self.batch.insert_bytecode(code_hash, code.clone().into()).await?;
            self.batch.update_account(addr, trie_account).await?;
            
            // Sync PlainState and HashedState
            let mut acc_rlp = Vec::new();
            trie_account.encode(&mut acc_rlp);
            let acc_rlp_bytes = Bytes::from(acc_rlp);
            self.batch.update_plain_state(addr, acc_rlp_bytes.clone()).await?;
            self.batch.update_hashed_state(keccak256(addr), acc_rlp_bytes).await?;
        }

        // 4. Storage Resets
        for address in &changeset.storage_resets {
            let addr = Address::from_slice(address.as_bytes());
            affected_accounts.insert(*address);
            self.batch.reset_storage(addr).await?;
        }

        // 5. Storages
        for ((address, slot), value) in &changeset.storages {
            let addr = Address::from_slice(address.as_bytes());
            affected_accounts.insert(*address);
            let slot_b256 = B256::from_slice(slot.as_bytes());
            let val_u256 = U256::from_be_bytes(value.0);
            
            let _ = self.batch.storage(addr, slot_b256, state_root).await;

            self.batch.update_storage(addr, slot_b256, val_u256).await?;
            
            // Sync HashedState for storage
            let hashed_addr = keccak256(addr);
            let hashed_slot = keccak256(slot_b256);
            let combined_key = keccak256([hashed_addr.0, hashed_slot.0].concat());
            let val_bytes = val_u256.to_be_bytes::<32>();
            self.batch.update_hashed_state(combined_key, Bytes::from(val_bytes.to_vec())).await?;
        }

        for address in &changeset.touched { affected_accounts.insert(*address); }
        for address in &changeset.deletes { affected_accounts.insert(*address); }

        for address in affected_accounts {
            let addr = Address::from_slice(address.as_bytes());
            if changeset.deletes.contains(&address) {
                let _ = self.batch.account(addr, state_root).await;
                self.batch.remove_account(addr).await?;
                continue;
            }

            if let Some(mut trie_account) = self.batch.account(addr, state_root).await? {
                let new_storage_root = self.batch.calculate_storage_root(addr, state_root).await?;

                trie_account.storage_root = new_storage_root;
                if trie_account.code_hash == B256::default() {
                    trie_account.code_hash = alloy_primitives::KECCAK256_EMPTY;
                }
                

                if eip161 && SputnikBackend::is_account_empty(&trie_account) {
                    debug!("[Execution] Removing empty account: addr={:?}", addr);
                    self.batch.remove_account(addr).await?;
                    self.batch.remove_plain_state(addr).await?;
                } else {
                    self.batch.update_account(addr, trie_account).await?;
                    
                    // Sync PlainState and HashedState
                    let mut acc_rlp = Vec::new();
                    trie_account.encode(&mut acc_rlp);
                    let acc_rlp_bytes = Bytes::from(acc_rlp);
                    self.batch.update_plain_state(addr, acc_rlp_bytes.clone()).await?;
                    self.batch.update_hashed_state(keccak256(addr), acc_rlp_bytes).await?;
                }
            } else {
                 let trie_account = TrieAccount {
                    nonce: 0,
                    balance: U256::ZERO,
                    storage_root: EMPTY_ROOT_HASH,
                    code_hash: alloy_primitives::KECCAK256_EMPTY,
                 };
                 if !SputnikBackend::is_account_empty(&trie_account) {
                    self.batch.update_account(addr, trie_account).await?;
                    
                    // Sync PlainState and HashedState
                    let mut acc_rlp = Vec::new();
                    trie_account.encode(&mut acc_rlp);
                    let acc_rlp_bytes = Bytes::from(acc_rlp);
                    self.batch.update_plain_state(addr, acc_rlp_bytes.clone()).await?;
                    self.batch.update_hashed_state(keccak256(addr), acc_rlp_bytes).await?;
                 }
            }
        }

        Ok(())
    }

    async fn get_or_materialize_account(
        &self,
        addr: Address,
        state_root: Option<B256>,
    ) -> Result<TrieAccount> {
        Ok(self.batch.account(addr, state_root).await?.unwrap_or_else(|| {
            TrieAccount {
                nonce: 0,
                balance: U256::ZERO,
                storage_root: EMPTY_ROOT_HASH,
                code_hash: alloy_primitives::KECCAK256_EMPTY,
            }
        }))
    }
}
