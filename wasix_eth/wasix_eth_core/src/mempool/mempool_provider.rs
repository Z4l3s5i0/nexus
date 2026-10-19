use alloy_primitives::Address;
use std::collections::HashMap;
use std::sync::Arc;
use wasix_eth_types::{async_trait, ConsensusTransaction, Transaction, B256, U256, Blob, Bytes48, TxPooledEnvelope, Bytes};
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::AccountProvider;
use crate::mempool::mempool::{Mempool, MempoolSnapshot, MempoolInternalState};
use im::HashMap as ImmutableMap;
use wasix_eth_utils::info;

#[async_trait]
pub trait MempoolProvider: Send + Sync {
    async fn add_transaction(&self, tx: Transaction, current_nonce: u64) -> bool;
    /// Returns (state_nonce, next_pending_nonce).
    async fn nonce_lookup(&self, address: Address, state_nonce: u64) -> (u64, u64);
    async fn update_base_fee(&self, new_base_fee: U256, state: &DatabaseReadProvider);
    async fn revalidate(&self, state: &DatabaseReadProvider, addresses: Option<Vec<Address>>);
    async fn pop_transactions(&self, n: usize) -> Vec<Transaction>;
    async fn peek_transactions(&self, n: usize) -> Vec<Transaction>;
    async fn remove_transactions(&self, tx_hashes: &[B256]);
    async fn get_all_transactions(&self) -> Vec<Transaction>;
    async fn get_transactions_by_sender(&self, address: Address) -> Vec<Transaction>;
    async fn len(&self) -> usize;
    async fn is_empty(&self) -> bool;
    async fn clear(&self);
    async fn base_fee(&self) -> U256;
    async fn peek_best_transactions(&self, target_gas_limit: u64, base_fee: U256, blob_base_fee: Option<U256>, max_blobs_per_block: Option<u32>) -> Vec<Transaction>;
    fn snapshot(&self) -> MempoolSnapshot;
    async fn add_blob(&self, versioned_hash: B256, blob: Blob, commitment: Bytes48, proof: Bytes48);
    async fn get_blob(&self, versioned_hash: B256) -> Option<(Blob, Bytes48, Bytes48)>;
    async fn get_transaction(&self, hash: B256) -> Option<Transaction>;
    async fn add_pooled_envelope(&self, hash: B256, pooled: TxPooledEnvelope);
    async fn get_pooled_envelope(&self, hash: B256) -> Option<TxPooledEnvelope>;
    async fn add_pooled_bytes(&self, hash: B256, bytes: Bytes);
    async fn get_pooled_bytes(&self, hash: B256) -> Option<Bytes>;
    async fn next_expected_nonce(&self, address: Address, state_nonce: u64) -> u64;
}

#[async_trait]
impl MempoolProvider for Mempool {
    async fn add_transaction(&self, tx: Transaction, current_nonce: u64) -> bool {
        self.inner.add_transaction(tx, current_nonce).await
    }

    async fn update_base_fee(&self, new_base_fee: U256, state: &DatabaseReadProvider) {
        let (increased, old_fee) = {
            let mut base_fee_lock = self.inner.base_fee.write().await;
            let increased = new_base_fee > *base_fee_lock;
            let old_fee = *base_fee_lock;
            *base_fee_lock = new_base_fee;
            (increased, old_fee)
        };

        if increased {
            info!("[Mempool] Base fee increased from {} to {}. Revalidating mempool...", old_fee, new_base_fee);
            self.revalidate(state, None).await;
        }
    }

    /// Revalidate the mempool against the latest state.
    /// Removes transactions that are no longer valid (e.g. nonce too low, insufficient balance).
    /// If `addresses` is provided, only those accounts are revalidated.
    async fn revalidate(&self, state: &DatabaseReadProvider, addresses: Option<Vec<Address>>) {
        let mut to_remove = Vec::new();
        let mut addresses_to_promote = Vec::new();

        let base_fee = *self.inner.base_fee.read().await;
        let mempool_state = self.inner.state.load();

        let revalidate_addr = |address: Address, to_remove: &mut Vec<B256>, addresses_to_promote: &mut Vec<Address>, current_nonce: u64, current_balance: U256| {
            // Check pending transactions
            if let Some(queue) = mempool_state.pending_transactions.get(&address) {
                for tx in queue.iter() {
                    let tx_nonce = tx.nonce();
                    let max_fee = tx.max_fee_per_gas();

                    if max_fee < base_fee.to::<u128>() {
                        info!("[Mempool] Evicting transaction {:?} (nonce: {}) because max fee {} is below base fee {}", 
                            tx.hash(), tx_nonce, max_fee, base_fee);
                        to_remove.push(*tx.hash());
                        continue;
                    }

                    let gas_limit = tx.gas_limit() as u128;
                    let max_fee_per_gas = tx.max_fee_per_gas();
                    let value = tx.value().to::<u128>();
                    let required_balance = U256::from(gas_limit * max_fee_per_gas + value);

                    if tx_nonce < current_nonce || current_balance < required_balance {
                        to_remove.push(*tx.hash());
                        continue;
                    }

                    if tx.is_eip4844() {
                        if let Some(hashes) = tx.blob_versioned_hashes() {
                            for hash in hashes {
                                if !mempool_state.blobs.contains_key(hash) {
                                    info!("[Mempool] Evicting transaction {:?} because blob {:?} is missing", tx.hash(), hash);
                                    to_remove.push(*tx.hash());
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            // Check queued transactions
            if let Some(queue) = mempool_state.queued_transactions.get(&address) {
                for tx in queue.iter() {
                    let tx_nonce = tx.nonce();
                    let max_fee = tx.max_fee_per_gas();

                    if max_fee < base_fee.to::<u128>() {
                        info!("[Mempool] Evicting transaction {:?} (nonce: {}) because max fee {} is below base fee {}", 
                            tx.hash(), tx_nonce, max_fee, base_fee);
                        to_remove.push(*tx.hash());
                        continue;
                    }

                    let gas_limit = tx.gas_limit() as u128;
                    let max_fee_per_gas = tx.max_fee_per_gas();
                    let value = tx.value().to::<u128>();
                    let required_balance = U256::from(gas_limit * max_fee_per_gas + value);

                    if tx_nonce < current_nonce || current_balance < required_balance {
                        to_remove.push(*tx.hash());
                        continue;
                    }

                    if tx.is_eip4844() {
                        if let Some(hashes) = tx.blob_versioned_hashes() {
                            for hash in hashes {
                                if !mempool_state.blobs.contains_key(hash) {
                                    info!("[Mempool] Evicting transaction {:?} because blob {:?} is missing", tx.hash(), hash);
                                    to_remove.push(*tx.hash());
                                    break;
                                }
                            }
                        }
                    }
                }
                addresses_to_promote.push(address);
            }
        };

        if let Some(addrs) = addresses {
            for addr in addrs {
                let (current_nonce, current_balance) = if let Ok(Some(acc)) = state.account(addr, None).await {
                    (acc.nonce, acc.balance)
                } else {
                    (0, U256::ZERO)
                };
                revalidate_addr(addr, &mut to_remove, &mut addresses_to_promote, current_nonce, current_balance);
            }
        } else {
            // Full revalidation
            for (addr, _) in mempool_state.pending_transactions.iter() {
                let (current_nonce, current_balance) = if let Ok(Some(acc)) = state.account(*addr, None).await {
                    (acc.nonce, acc.balance)
                } else {
                    (0, U256::ZERO)
                };
                revalidate_addr(*addr, &mut to_remove, &mut addresses_to_promote, current_nonce, current_balance);
            }
            for (addr, _) in mempool_state.queued_transactions.iter() {
                if !mempool_state.pending_transactions.contains_key(addr) {
                    let (current_nonce, current_balance) = if let Ok(Some(acc)) = state.account(*addr, None).await {
                        (acc.nonce, acc.balance)
                    } else {
                        (0, U256::ZERO)
                    };
                    revalidate_addr(*addr, &mut to_remove, &mut addresses_to_promote, current_nonce, current_balance);
                }
            }
        }

        if !to_remove.is_empty() {
            info!("[Mempool] Removing {} invalid transactions during revalidation", to_remove.len());
            self.remove_transactions(&to_remove).await;
        }

        // Try to promote queued transactions in case some were removed or new ones can be moved
        for address in addresses_to_promote {
            let current_nonce = if let Ok(Some(acc)) = state.account(address, None).await {
                acc.nonce
            } else {
                0
            };
            let next_nonce = self.inner.state.load().pending_transactions.get(&address)
                .and_then(|q| q.back().map(|t| t.nonce() + 1))
                .unwrap_or(current_nonce);
            self.inner.promote_queued(address, next_nonce);
        }
    }

    async fn pop_transactions(&self, n: usize) -> Vec<Transaction> {
        self.inner.pop_transactions(n).await
    }

    async fn peek_transactions(&self, n: usize) -> Vec<Transaction> {
        self.inner.peek_transactions(n)
    }

    async fn remove_transactions(&self, tx_hashes: &[B256]) {
        self.inner.remove_transactions(tx_hashes);
    }

    async fn get_all_transactions(&self) -> Vec<Transaction> {
        self.inner.get_all_transactions()
    }

    async fn get_transactions_by_sender(&self, address: Address) -> Vec<Transaction> {
        let mut txs = Vec::new();
        let state = self.inner.state.load();
        if let Some(pending) = state.pending_transactions.get(&address) {
            txs.extend(pending.iter().cloned());
        }
        if let Some(queued) = state.queued_transactions.get(&address) {
            txs.extend(queued.iter().cloned());
        }
        txs
    }

    async fn len(&self) -> usize {
        self.inner.len()
    }

    async fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    async fn clear(&self) {
        self.inner.clear();
    }

    async fn base_fee(&self) -> U256 {
        *self.inner.base_fee.read().await
    }

    async fn peek_best_transactions(&self, target_gas_limit: u64, base_fee: U256, blob_base_fee: Option<U256>, max_blobs_per_block: Option<u32>) -> Vec<Transaction> {
        self.peek_best_transactions(target_gas_limit, base_fee, blob_base_fee, max_blobs_per_block).await
    }

    fn snapshot(&self) -> MempoolSnapshot {
        self.snapshot()
    }

    async fn add_blob(&self, versioned_hash: B256, blob: Blob, commitment: Bytes48, proof: Bytes48) {
        self.inner.update_state(move |state| {
            state.blobs.insert(versioned_hash, Arc::new((blob.clone(), commitment, proof)));
        });
    }

    async fn get_blob(&self, versioned_hash: B256) -> Option<(Blob, Bytes48, Bytes48)> {
        self.inner.state.load().blobs.get(&versioned_hash).map(|r| (**r).clone())
    }

    async fn get_transaction(&self, hash: B256) -> Option<Transaction> {
        let state = self.inner.state.load();
        for (_, queue) in state.pending_transactions.iter() {
            if let Some(tx) = queue.iter().find(|tx| *tx.hash() == hash) {
                return Some(tx.clone());
            }
        }
        for (_, queue) in state.queued_transactions.iter() {
            if let Some(tx) = queue.iter().find(|tx| *tx.hash() == hash) {
                return Some(tx.clone());
            }
        }
        None
    }

    async fn add_pooled_envelope(&self, hash: B256, pooled: TxPooledEnvelope) {
        self.inner.update_state(move |state| {
            state.pooled_envelopes.insert(hash, pooled.clone());
        });
    }

    async fn get_pooled_envelope(&self, hash: B256) -> Option<TxPooledEnvelope> {
        self.inner.state.load().pooled_envelopes.get(&hash).cloned()
    }

    async fn add_pooled_bytes(&self, hash: B256, bytes: Bytes) {
        self.inner.update_state(move |state| {
            state.pooled_bytes.insert(hash, bytes.clone());
        });
    }

    async fn get_pooled_bytes(&self, hash: B256) -> Option<Bytes> {
        self.inner.state.load().pooled_bytes.get(&hash).cloned()
    }

    async fn next_expected_nonce(&self, address: Address, state_nonce: u64) -> u64 {
        self.next_expected_nonce(address, state_nonce).await
    }

    async fn nonce_lookup(&self, address: Address, state_nonce: u64) -> (u64, u64) {
        let state = self.inner.state.load();
        let next_pending = if let Some(q) = state.pending_transactions.get(&address) {
            let mut expected = state_nonce;
            for tx in q.iter() {
                if tx.nonce() == expected {
                    expected += 1;
                } else if tx.nonce() > expected {
                    break;
                }
            }
            expected
        } else {
            state_nonce
        };
        (state_nonce, next_pending)
    }
}

pub struct NoopMempoolProvider;

impl NoopMempoolProvider {
    fn empty_blobs() -> Arc<ImmutableMap<B256, Arc<(Blob, Bytes48, Bytes48)>>> {
        Arc::new(ImmutableMap::new())
    }
}

#[async_trait]
impl MempoolProvider for NoopMempoolProvider {
    async fn add_transaction(&self, _tx: Transaction, _current_nonce: u64) -> bool { true }
    async fn update_base_fee(&self, _new_base_fee: U256, _state: &DatabaseReadProvider) {}
    async fn revalidate(&self, _state: &DatabaseReadProvider, _addresses: Option<Vec<Address>>) {}
    async fn pop_transactions(&self, _n: usize) -> Vec<Transaction> { Vec::new() }
    async fn peek_transactions(&self, _n: usize) -> Vec<Transaction> { Vec::new() }
    async fn remove_transactions(&self, _tx_hashes: &[B256]) {}
    async fn get_all_transactions(&self) -> Vec<Transaction> { Vec::new() }
    async fn get_transactions_by_sender(&self, _address: Address) -> Vec<Transaction> { Vec::new() }
    async fn len(&self) -> usize { 0 }
    async fn is_empty(&self) -> bool { true }
    async fn clear(&self) {}
    async fn base_fee(&self) -> U256 { U256::ZERO }
    async fn peek_best_transactions(&self, _target_gas_limit: u64, _base_fee: U256, _blob_base_fee: Option<U256>, _max_blobs_per_block: Option<u32>) -> Vec<Transaction> { Vec::new() }
    fn snapshot(&self) -> MempoolSnapshot { MempoolSnapshot { pending: ImmutableMap::new(), blobs: Self::empty_blobs() } }
    async fn add_blob(&self, _versioned_hash: B256, _blob: Blob, _commitment: Bytes48, _proof: Bytes48) {}
    async fn get_blob(&self, _versioned_hash: B256) -> Option<(Blob, Bytes48, Bytes48)> { None }
    async fn get_transaction(&self, _hash: B256) -> Option<Transaction> { None }
    async fn add_pooled_envelope(&self, _hash: B256, _pooled: TxPooledEnvelope) {}
    async fn get_pooled_envelope(&self, _hash: B256) -> Option<TxPooledEnvelope> { None }
    async fn add_pooled_bytes(&self, _hash: B256, _bytes: Bytes) {}
    async fn get_pooled_bytes(&self, _hash: B256) -> Option<Bytes> { None }
    async fn next_expected_nonce(&self, _address: Address, state_nonce: u64) -> u64 { state_nonce }
    async fn nonce_lookup(&self, _address: Address, state_nonce: u64) -> (u64, u64) { (state_nonce, state_nonce) }
}
