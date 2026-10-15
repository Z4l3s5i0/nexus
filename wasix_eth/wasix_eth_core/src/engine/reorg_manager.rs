use alloy_primitives::B256;
use wasix_eth_storage::{AccountWriter, BlockProvider, BlockWriter, ChangeSetProvider, ChangeSetWriter, HeaderProvider, HeaderWriter, StateWriter, StorageWriter, TransactionProvider, TransactionWriter};
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
            let hash = block.header.hash_slow();
            let number = block.header.number;

            info!("[ChainManager] Marking block {} (hash {}) as canonical", number, hash);
            
            // Ensure both Header and Body are in main storage
            let body_exists = self.read_storage.block_body_by_hash(hash).ok().flatten().is_some();
            if !body_exists {
                debug!("[ChainManager] Block {} not found in main storage, checking Payloads table", hash);
                if let Some((full_block, receipts, metas, _bundle)) = self.read_storage.get_payload_by_block_hash(hash) {
                    // This call should internally insert Header, Body, Receipts, and Metas
                    self.write_storage.insert_block(full_block, receipts, metas)?;
                    self.write_storage.remove_payload_by_block_hash(hash)?;
                    debug!("[ChainManager] Promoted block {} from Payloads to main storage", hash);
                }
            }
            // 1. Mark as canonical (Number -> Hash)
            self.write_storage.set_canonical(number, hash)?;

            // 2. Ensure Hash -> Number mapping is correct
            self.write_storage.insert_header_number(hash, number)?;
            self.write_storage.insert_block_hash(hash, number)?;

            // 3. Ensure TD is correct
            if let Ok(parent_td) = self.read_storage.header_td(block.header.parent_hash) {
                if let Some(ptd) = parent_td {
                    let expected_td = ptd + block.header.difficulty;
                    let current_td = self.read_storage.header_td(hash).ok().flatten();
                    if current_td != Some(expected_td) {
                        self.write_storage.insert_header_td(hash, expected_td)?;
                    }
                }
            }

            // 4. Update transaction lookups
            for (i, tx) in block.body.transactions.iter().enumerate() {
                let tx_hash = *tx.hash();
                self.write_storage.insert_transaction_lookup(tx_hash, hash, i as u64)?;
            }
        }
        Ok(())
    }


    pub async fn revert_to_height(&self, height: u64) -> wasix_eth_types::Result<()> {
        let (_, head_number) = self.canonical.get_head();

        info!("Reverting chain from {} to {}", head_number, height);

        // Revert state changes from head_number down to height + 1
        for h in (height + 1..=head_number).rev() {
            // 1. Roll back account state changes (Balances, Nonces, etc.)
            if let Some(account_changes) = self.read_storage.account_change_set(h)? {
                for (address, old_state) in account_changes {
                    if let Some(state) = old_state {
                        // Restore the account to its prior state
                        let mut buf = &state[..];
                        let trie_acc = TrieAccount::decode(&mut buf)
                            .map_err(|e| anyhow::anyhow!("Failed to decode TrieAccount for {}: {}", address, e))?;

                        self.write_storage.update_account(address, trie_acc)?;
                        // Also update the plain state for consistency if your storage layer uses it
                        self.write_storage.update_plain_state(address, state.clone())?;
                    } else {
                        // If the account didn't exist before this block, delete it
                        self.write_storage.remove_account(address)?;
                        self.write_storage.remove_plain_state(address)?;
                    }
                }
            }

            // 2. Roll back storage changes
            if let Some(storage_changes) = self.read_storage.storage_change_set(h)? {
                for (address, slot, old_value) in storage_changes {
                    self.write_storage.update_storage(address, slot, old_value)?;
                }
            }

            // Clean up the change sets themselves
            self.write_storage.remove_change_set(h)?;

            // Remove from canonical heads
            if let Some(block_hash) = self.read_storage.block_hash(h)? {
                if let Some(body) = self.read_storage.block_body_by_hash(block_hash)? {
                    for i in 0..body.transactions.len() {
                        let _ = self.write_storage.remove_receipt_meta(block_hash, i as u64);
                    }
                }
            }
            self.write_storage.remove_canonical(h)?;
        }

        // Update canonical head
        let new_head_hash = self.read_storage.block_hash(height)?
            .ok_or_else(|| anyhow::anyhow!("Block hash not found for height {}", height))?;

        self.canonical.update_head(new_head_hash, height).await?;

        // Clear tracking maps to avoid polluting subsequent forward execution
        self.write_storage.clear_tracking();

        Ok(())
    }

}