use alloy_primitives::B256;
use wasix_eth_storage::{AccountWriter, BlockProvider, BlockWriter, ChangeSetProvider, ChangeSetWriter, HeaderProvider, HeaderWriter, StorageWriter, TransactionWriter};
use wasix_eth_storage::codecs::RedbRlp;
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_types::{Block, BlockId, ReorgContext, Transaction, TrieAccount};
use wasix_eth_utils::{debug, info};

use crate::engine::canonicality_tracker::CanonicalState;
use std::sync::Arc;

pub struct ReorgHandler{
    read_storage: DatabaseReadProvider,
    write_storage: DatabaseWriteProvider,
    canonical: Arc<CanonicalState>,
}

impl ReorgHandler {
    pub fn new(read_provider: DatabaseReadProvider, write_provider: DatabaseWriteProvider, canonical: Arc<CanonicalState>) -> Self {
        Self {
            read_storage: read_provider,
            write_storage: write_provider,
            canonical,
        }
    }

    pub async fn resolve_reorg(&self, old_head: B256, new_head: B256) -> wasix_eth_types::Result<ReorgContext> {
        let mut new_canonical_blocks = Vec::new();
        let mut current_hash = new_head;
        let mut common_ancestor_hash = B256::ZERO;
        let mut common_ancestor_found = false;

        debug!("[ChainManager] Resolving reorg from {:?} to {:?}", old_head, new_head);

        while current_hash != B256::ZERO {
            if current_hash == old_head {
                debug!("[ChainManager] Found old head {:?} as common ancestor", current_hash);
                common_ancestor_hash = old_head;
                common_ancestor_found = true;
                break;
            }

            // Check if this block is already marked canonical in O(1) if possible
            if let Ok(Some(number)) = self.read_storage.block_number(current_hash) {
                if let Ok(Some(canonical_hash)) = self.read_storage.block_hash(number) {
                    if canonical_hash == current_hash {
                        debug!("[ChainManager] Found canonical block #{} hash {:?} as common ancestor", number, current_hash);
                        common_ancestor_hash = current_hash;
                        common_ancestor_found = true;
                        break;
                    }
                }
            }

            // If not canonical, we need to walk back
            if let Ok(Some(header)) = self.read_storage.header(BlockId::Hash(current_hash.into())) {
                if let Ok(Some(block)) = self.read_storage.block_by_hash(current_hash) {
                    new_canonical_blocks.push(block);
                } else if let Some((block, _, _, _)) = self.read_storage.get_payload_by_block_hash(current_hash) {
                    new_canonical_blocks.push(block.clone());
                } else {
                    return Err(anyhow::anyhow!("Block data missing for hash {:?} during reorg walk-back", current_hash).into());
                }

                current_hash = header.parent_hash;
            } else if let Some((block, _, _, _)) = self.read_storage.get_payload_by_block_hash(current_hash) {
                new_canonical_blocks.push(block.clone());
                current_hash = block.header.parent_hash;
            } else {
                debug!("[ChainManager] Walk-back reached unknown block {:?}", current_hash);
                break;
            }
        }

        // If the new head itself is the common ancestor, it means we are attempting to move backwards.
        if common_ancestor_found && common_ancestor_hash == new_head {
            debug!(
                "[ChainManager] Proposed new head is an ancestor (hash {:?}). Skipping reorg.",
                new_head
            );
            return Ok(ReorgContext {
                common_ancestor_hash,
                new_canonical_blocks: Vec::new(),
                is_reorg: false,
            });
        }

        // No reorg if the old head equals the common ancestor (already on canonical tip)
        if common_ancestor_found && common_ancestor_hash == old_head {
            return Ok(ReorgContext {
                common_ancestor_hash,
                new_canonical_blocks,
                is_reorg: false,
            });
        }

        let is_reorg = common_ancestor_found && old_head != common_ancestor_hash;
        if is_reorg {
            info!(
                "[ChainManager] Reorg detected! Common ancestor: {:?} at height {}",
                common_ancestor_hash,
                self.read_storage
                    .header(BlockId::Hash(common_ancestor_hash.into()))
                    .ok()
                    .flatten()
                    .map(|h| h.number)
                    .unwrap_or(0)
            );
        }

        Ok(ReorgContext {
            common_ancestor_hash,
            new_canonical_blocks,
            is_reorg,
        })
    }

    pub async fn mark_branch_canonical(&self, blocks: &Vec<Block<Transaction>>) -> wasix_eth_types::Result<()> {
        for block in blocks.iter().rev() {
            let batch = self.write_storage.begin_batch()?;
            let hash = block.header.hash_slow();
            let number = block.header.number;

            info!("[ChainManager] Marking block {} (hash {}) as canonical", number, hash);
            
            // Ensure both Header and Body are in main storage
            let body_exists = self.read_storage.block_body_by_hash(hash).ok().flatten().is_some();
            if !body_exists {
                debug!("[ChainManager] Block {} not found in main storage, checking Payloads table", hash);
                if let Some((full_block, receipts, metas, _bundle)) = self.read_storage.get_payload_by_block_hash(hash) {
                    // This call should internally insert Header, Body, Receipts, and Metas
                    batch.insert_block(full_block, receipts, metas)?;
                    batch.remove_payload_by_block_hash(hash)?;
                    debug!("[ChainManager] Promoted block {} from Payloads to main storage", hash);
                }
            }
            // 1. Mark as canonical (Number -> Hash)
            batch.set_canonical(number, hash)?;

            // 2. Ensure Hash -> Number mapping is correct
            batch.insert_header_number(hash, number)?;
            batch.insert_block_hash(hash, number)?;

            // 3. Ensure TD is correct
            if let Ok(parent_td) = self.read_storage.header_td(block.header.parent_hash) {
                if let Some(ptd) = parent_td {
                    let expected_td = ptd + block.header.difficulty;
                    let current_td = self.read_storage.header_td(hash).ok().flatten();
                    if current_td != Some(expected_td) {
                        batch.insert_header_td(hash, expected_td)?;
                    }
                }
            }

            // 4. Update transaction lookups
            for (i, tx) in block.body.transactions.iter().enumerate() {
                let tx_hash = *tx.hash();
                batch.insert_transaction_lookup(tx_hash, hash, i as u64)?;
            }
            batch.commit()?;
            // Yield between blocks to allow other DB operations
            tokio::task::yield_now().await;
        }
        Ok(())
    }


    pub async fn revert_to_height(&self, height: u64) -> wasix_eth_types::Result<()> {
        let (_, head_number) = self.canonical.get_head().await;

        info!("Reverting chain from {} to {}", head_number, height);

        // Revert state changes from head_number down to height + 1 in chunks
        for h in (height + 1..=head_number).rev() {
            let batch = self.write_storage.begin_batch()?;

            // 1. Roll back account state changes (Balances, Nonces, etc.)
            if let Some(account_changes) = self.read_storage.account_change_set(h)? {
                for (address, old_state) in account_changes {
                    if let Some(state) = old_state {
                        // Restore the account to its prior state
                        let mut buf = &state[..];
                        let trie_acc = TrieAccount::decode(&mut buf)
                            .map_err(|e| anyhow::anyhow!("Failed to decode TrieAccount for {}: {}", address, e))?;

                        batch.update_account(address, trie_acc)?;
                    } else {
                        // If the account didn't exist before this block, delete it
                        batch.remove_account(address)?;
                    }
                }
            }

            // 2. Roll back storage changes
            if let Some(storage_changes) = self.read_storage.storage_change_set(h)? {
                for (address, slot, old_value) in storage_changes {
                    batch.update_storage(address, slot, old_value)?;
                }
            }

            // Clean up the change sets themselves
            batch.remove_change_set(h)?;

            // Remove from canonical heads
            if let Some(block_hash) = self.read_storage.block_hash(h)? {
                if let Some(body) = self.read_storage.block_body_by_hash(block_hash)? {
                    for i in 0..body.transactions.len() {
                        let _ = batch.remove_receipt_meta(block_hash, i as u64);
                    }
                }
            }
            batch.remove_canonical(h)?;

            batch.commit()?;
            // Yield between blocks to allow other DB operations
            tokio::task::yield_now().await;
        }

        // Update canonical head
        let new_head_hash = self.read_storage.block_hash(height)?
            .ok_or_else(|| anyhow::anyhow!("Block hash not found for height {}", height))?;

        self.canonical.update_head(new_head_hash, height).await?;

        // Finalize reorg with head update
        let batch = self.write_storage.begin_batch()?;
        // Clear tracking maps to avoid polluting subsequent forward execution
        batch.clear_tracking();
        batch.commit()?;

        Ok(())
    }

    pub async fn fast_switch(&self, new_head_hash: B256, state_root: B256) -> wasix_eth_types::Result<()> {
        info!("[ChainManager] Performing Fast Switch to state root {:?}", state_root);

        let batch = self.write_storage.begin_batch()?;

        batch.update_forkchoice(new_head_hash, None, None)?;

        // Update the in-memory head
        let number = self.read_storage.block_number(new_head_hash)?
            .ok_or_else(|| anyhow::anyhow!("Block number not found for hash {:?}", new_head_hash))?;
        
        self.canonical.update_head(new_head_hash, number).await?;

        batch.commit()?;

        // Trigger background reconciliation for the flat state tables (Accounts, Storages)
        // to eventually match the MPT root.
        let _read_provider = self.read_storage.clone();
        let _write_provider = self.write_storage.clone();
        // tokio::spawn(async move {
        //     if let Err(e) = heal_flat_state(read_provider, write_provider, state_root).await {
        //         wasix_eth_utils::error!("[ChainManager] Flat-state reconciliation failed: {:?}", e);
        //     } else {
        //         wasix_eth_utils::info!("[ChainManager] Flat-state reconciliation completed for root {:?}", state_root);
        //     }
        // });

        Ok(())
    }

// async fn heal_flat_state(
//     read_provider: DatabaseReadProvider,
//     write_provider: DatabaseWriteProvider,
//     state_root: B256,
// ) -> anyhow::Result<()> {
//     if state_root == alloy_trie::EMPTY_ROOT_HASH {
//         // Clear all flat state if trie is empty
//         write_provider.with_write(|wtx| {
//             wtx.open_table(crate::tables::Accounts::definition())?.retain(|_, _| false)?;
//             wtx.open_table(crate::tables::Storages::definition())?.retain(|_, _| false)?;
//             Ok(())
//         })?;
//         return Ok(());
//     }
// 
//     // 1. Collect all known addresses from the flat state and change sets
//     // to build a HashedAddress -> Address mapping.
//     let mut address_map = std::collections::HashMap::new();
//     
//     // From flat state
//     if let Ok(addresses) = read_provider.addresses() {
//         for addr in addresses {
//             address_map.insert(alloy_primitives::keccak256(addr), addr);
//         }
//     }
// 
//     // 2. Traverse MPT and gather updates
//     let account_entries = {
//         let mut trie = crate::trie::EthTrie::new(&read_provider, state_root);
//         let entries = trie.all_entries()?;
//         
//         let mut account_entries = Vec::new();
//         for (hashed_addr, rlp_acc) in entries {
//             let mut slice = &rlp_acc[..];
//             if let Ok(acc) = wasix_eth_types::TrieAccount::decode(&mut slice) {
//                 account_entries.push((hashed_addr, acc));
//             }
//         }
//         account_entries
//     };
// 
//     // 3. Apply updates in chunks to avoid long write locks
//     const CHUNK_SIZE: usize = 1000;
//     for chunk in account_entries.chunks(CHUNK_SIZE) {
//         write_provider.with_write(|wtx| {
//             let mut acc_table = wtx.open_table(crate::tables::Accounts::definition())?;
//             let mut storage_table = wtx.open_table(crate::tables::Storages::definition())?;
// 
//             for (hashed_addr, acc) in chunk {
//                 if let Some(addr) = address_map.get(hashed_addr) {
//                     // Update account
//                     acc_table.insert(*addr, acc.clone())?;
// 
//                     // Heal storage for this account
//                     if acc.storage_root != alloy_trie::EMPTY_ROOT_HASH {
//                         let mut storage_trie = crate::trie::EthTrie::new(&read_provider, acc.storage_root);
//                         if let Ok(storage_entries) = storage_trie.all_entries() {
//                             for (hashed_slot, rlp_val) in storage_entries {
//                                 // We don't have a slot mapping, but many implementations use hashed slots 
//                                 // directly in flat state or we skip if unknown.
//                                 // For now, we can only heal if we have a way to know the plain slot.
//                                 // Some implementations use the B256 hashed slot as the key in Storages.
//                                 // Let's check codecs.rs again: type Key = (Address, B256);
//                             }
//                         }
//                     }
//                 }
//             }
//             Ok(())
//         })?;
//         tokio::task::yield_now().await;
//     }
// 
//     Ok(())
}