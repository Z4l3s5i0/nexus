use alloy_primitives::Address;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use wasix_eth_types::{async_trait, ConsensusTransaction, Transaction, B256, U256, Blob, Bytes48, TxPooledEnvelope, Bytes};
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::AccountProvider;
use crate::mempool::mempool::{Mempool, MempoolSnapshot};
use im::HashMap as ImmutableMap;
use wasix_eth_storage::{ChainProvider, HeaderProvider};
use wasix_eth_utils::{error, info};

impl Mempool {
    fn schedule_full_revalidation(&self, state: DatabaseReadProvider) {
        if !self.inner.request_full_revalidation() {
            return;
        }

        let mempool = self.clone();
        tokio::spawn(async move {
            loop {
                mempool.inner.full_revalidation_pending.store(false, Ordering::Release);
                mempool.revalidate_inner(&state, None).await;

                if mempool.inner.full_revalidation_pending.load(Ordering::Acquire) {
                    continue;
                }

                mempool.inner.full_revalidation_running.store(false, Ordering::Release);
                if !mempool.inner.full_revalidation_pending.swap(false, Ordering::AcqRel) {
                    break;
                }
                if mempool.inner.full_revalidation_running.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
                    break;
                }
            }
        });
    }

    async fn revalidate_inner(&self, state: &DatabaseReadProvider, addresses: Option<Vec<Address>>) {
        let base_fee = *self.inner.base_fee.read().await;
        let mempool_state = self.inner.state.load_full();
        let state = state.clone();

        let (to_remove, addresses_to_promote, current_nonces) = match tokio::task::spawn_blocking(move || {
            let chain_config = state.chain_config().ok().flatten();
            let latest_header = state.header(wasix_eth_types::BlockId::Number(wasix_eth_types::BlockNumberOrTag::Latest)).ok().flatten();
            let blob_base_fee = latest_header.as_ref().and_then(|header| header.excess_blob_gas).map(|excess| {
                let update_fraction = match (chain_config.as_ref(), latest_header.as_ref()) {
                    (Some(config), Some(header)) => {
                        let fork = wasix_eth_types::Hardfork::get_active_fork(config, header.number, header.timestamp);
                        fork.blob_params(config).map(|params| params.update_fraction)
                    }
                    _ => None,
                }.unwrap_or(wasix_eth_types::BLOB_GASPRICE_UPDATE_FRACTION as u128);
                wasix_eth_types::calc_blob_gasprice(excess, update_fraction)
            }).unwrap_or(0);

            let mut to_remove = Vec::new();
            let mut addresses_to_promote = Vec::new();
            let mut current_nonces = HashMap::new();

            let mut process_address = |address: Address| {
                let (current_nonce, current_balance) = if let Ok(Some(account)) = state.account(address, None) {
                    (account.nonce, account.balance)
                } else {
                    (0, U256::ZERO)
                };
                current_nonces.insert(address, current_nonce);

                if let Some(queue) = mempool_state.pending_transactions.get(&address) {
                    for tx in queue.iter() {
                        if should_remove_transaction(tx, current_nonce, current_balance, base_fee, blob_base_fee, mempool_state.as_ref()) {
                            to_remove.push(*tx.hash());
                        }
                    }
                }

                if let Some(queue) = mempool_state.queued_transactions.get(&address) {
                    for tx in queue.iter() {
                        if should_remove_transaction(tx, current_nonce, current_balance, base_fee, blob_base_fee, mempool_state.as_ref()) {
                            to_remove.push(*tx.hash());
                        }
                    }
                    addresses_to_promote.push(address);
                }
            };

            if let Some(addresses) = addresses {
                let mut seen = HashSet::new();
                for address in addresses {
                    if seen.insert(address) {
                        process_address(address);
                    }
                }
            } else {
                let mut seen = HashSet::new();
                for (address, _) in mempool_state.pending_transactions.iter() {
                    if seen.insert(*address) {
                        process_address(*address);
                    }
                }
                for (address, _) in mempool_state.queued_transactions.iter() {
                    if seen.insert(*address) {
                        process_address(*address);
                    }
                }
            }

            (to_remove, addresses_to_promote, current_nonces)
        }).await {
            Ok(result) => result,
            Err(join_error) => {
                error!("[Mempool] Revalidation database task failed: {}", join_error);
                return;
            }
        };

        if !to_remove.is_empty() {
            info!("[Mempool] Removing {} invalid transactions during revalidation", to_remove.len());
            self.inner.remove_transactions_exact(&to_remove);
        }

        for address in addresses_to_promote {
            let current_nonce = current_nonces.get(&address).copied().unwrap_or(0);
            let next_nonce = self.inner.state.load().pending_transactions.get(&address)
                .and_then(|queue| queue.back().map(|tx| tx.nonce() + 1))
                .unwrap_or(current_nonce);
            self.inner.promote_queued(address, next_nonce);
        }
    }
}

fn should_remove_transaction(
    tx: &wasix_eth_types::Transaction,
    current_nonce: u64,
    current_balance: U256,
    base_fee: U256,
    blob_base_fee: u128,
    mempool_state: &crate::mempool::mempool::MempoolInternalState,
) -> bool {
    let tx_nonce = tx.nonce();
    let max_fee = tx.max_fee_per_gas();
    if max_fee < base_fee.to::<u128>() {
        info!("[Mempool] Evicting transaction {:?} (nonce: {}) because max fee {} is below base fee {}", tx.hash(), tx_nonce, max_fee, base_fee);
        return true;
    }

    let gas_limit = tx.gas_limit() as u128;
    let value = tx.value().to::<u128>();
    let mut required_balance = U256::from(gas_limit * max_fee + value);
    if tx.is_eip4844() {
        let blob_gas_used = tx.blob_versioned_hashes().map(|hashes| hashes.len()).unwrap_or(0) as u64
            * wasix_eth_types::DATA_GAS_PER_BLOB;
        required_balance = required_balance.saturating_add(U256::from(blob_gas_used as u128 * blob_base_fee));
    }

    if tx_nonce < current_nonce || current_balance < required_balance {
        info!("[Mempool] Evicting transaction {:?} (nonce: {}) due to {} (balance: {}, required: {})",
            tx.hash(), tx_nonce, if tx_nonce < current_nonce { "stale nonce" } else { "insufficient balance" }, current_balance, required_balance);
        return true;
    }

    if tx.is_eip4844() {
        if let Some(hashes) = tx.blob_versioned_hashes() {
            for hash in hashes {
                if !mempool_state.blobs.contains_key(hash) {
                    info!("[Mempool] Evicting transaction {:?} because blob {:?} is missing", tx.hash(), hash);
                    return true;
                }
            }
        }
    }

    false
}

#[async_trait]
pub trait MempoolProvider: Send + Sync {
    async fn add_transaction(&self, tx: Transaction, current_nonce: u64) -> bool;
    /// Returns (state_nonce, next_pending_nonce).
    async fn nonce_lookup(&self, address: Address, state_nonce: u64) -> (u64, u64);
    async fn update_base_fee(&self, new_base_fee: U256, state: &DatabaseReadProvider);
    async fn revalidate(&self, state: &DatabaseReadProvider, addresses: Option<Vec<Address>>);
    async fn pop_transactions(&self, n: usize) -> Vec<Transaction>;
    async fn peek_transactions(&self, n: usize) -> Vec<Transaction>;
    fn remove_transactions_now(&self, tx_hashes: &[B256]) -> Vec<Address>;
    async fn remove_transactions(&self, tx_hashes: &[B256]) -> Vec<Address> {
        self.remove_transactions_now(tx_hashes)
    }
    async fn get_all_transactions(&self) -> Vec<Transaction>;
    async fn get_transactions_by_sender(&self, address: Address) -> Vec<Transaction>;
    async fn len(&self) -> usize;
    async fn is_empty(&self) -> bool;
    async fn clear(&self);
    async fn base_fee(&self) -> U256;
    async fn peek_best_transactions(&self, target_gas_limit: u64, base_fee: U256, blob_base_fee: Option<U256>, max_blobs_per_block: Option<u32>) -> (Vec<Transaction>, Vec<Transaction>);
    fn snapshot(&self) -> MempoolSnapshot;
    async fn add_blob(&self, versioned_hash: B256, blob: Blob, commitment: Bytes48, proof: Bytes48);
    async fn get_blob(&self, versioned_hash: B256) -> Option<(Blob, Bytes48, Bytes48)>;
    async fn get_transaction(&self, hash: B256) -> Option<Transaction>;
    async fn add_pooled_envelope(&self, hash: B256, pooled: TxPooledEnvelope);
    async fn get_pooled_envelope(&self, hash: B256) -> Option<TxPooledEnvelope>;
    async fn add_pooled_bytes(&self, hash: B256, bytes: Bytes);
    async fn get_pooled_bytes(&self, hash: B256) -> Option<Bytes>;
    async fn next_expected_nonce(&self, address: Address, state_nonce: u64) -> u64;
    /// Identify transactions in the mempool that are EIP-4844 but are missing their required blobs.
    async fn get_transactions_with_missing_blobs(&self) -> Vec<B256>;
    async fn mark_blob_sync_attempt(&self, tx_hashes: &[B256]);
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
            info!("[Mempool] Base fee increased from {} to {}. Revalidating mempool asynchronously...", old_fee, new_base_fee);
            self.schedule_full_revalidation(state.clone());
        }
    }

    /// Revalidate the mempool against the latest state.
    /// Removes transactions that are no longer valid (e.g. nonce too low, insufficient balance).
    /// If `addresses` is provided, only those accounts are revalidated.
    async fn revalidate(&self, state: &DatabaseReadProvider, addresses: Option<Vec<Address>>) {
        if let Some(addresses) = addresses {
            self.revalidate_inner(state, Some(addresses)).await;
        } else {
            self.schedule_full_revalidation(state.clone());
        }
    }

    async fn pop_transactions(&self, n: usize) -> Vec<Transaction> {
        self.inner.pop_transactions(n).await
    }

    async fn peek_transactions(&self, n: usize) -> Vec<Transaction> {
        self.inner.peek_transactions(n)
    }

    fn remove_transactions_now(&self, tx_hashes: &[B256]) -> Vec<Address> {
        self.inner.remove_transactions(tx_hashes)
    }

    async fn get_all_transactions(&self) -> Vec<Transaction> {
        self.inner.get_all_transactions()
    }

    async fn get_transactions_by_sender(&self, address: Address) -> Vec<Transaction> {
        let mut txs = Vec::new();
        let state = self.inner.state.load();
        if let Some(pending) = state.pending_transactions.get(&address) {
            txs.extend(pending.iter().map(|pooled| pooled.tx.clone()));
        }
        if let Some(queued) = state.queued_transactions.get(&address) {
            txs.extend(queued.iter().map(|pooled| pooled.tx.clone()));
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

    async fn peek_best_transactions(&self, target_gas_limit: u64, base_fee: U256, blob_base_fee: Option<U256>, max_blobs_per_block: Option<u32>) -> (Vec<Transaction>, Vec<Transaction>) {
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
                return Some(tx.tx.clone());
            }
        }
        for (_, queue) in state.queued_transactions.iter() {
            if let Some(tx) = queue.iter().find(|tx| *tx.hash() == hash) {
                return Some(tx.tx.clone());
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

    async fn get_transactions_with_missing_blobs(&self) -> Vec<B256> {
        let mut missing = Vec::new();
        let state = self.inner.state.load();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        
        for (_, queue) in state.pending_transactions.iter() {
            for tx in queue.iter() {
                if tx.is_eip4844() {
                    let hash = *tx.hash();
                    if let Some(last_attempt) = state.missing_blobs.get(&hash) {
                        if now - last_attempt < 60 {
                            continue;
                        }
                    }

                    if let Some(hashes) = tx.blob_versioned_hashes() {
                        for h in hashes {
                            if !state.blobs.contains_key(h) {
                                missing.push(hash);
                                break;
                            }
                        }
                    }
                }
                if missing.len() >= 32 { break; }
            }
            if missing.len() >= 32 { break; }
        }

        if missing.len() < 32 {
            for (_, queue) in state.queued_transactions.iter() {
                for tx in queue.iter() {
                    if tx.is_eip4844() {
                        let hash = *tx.hash();
                        if let Some(last_attempt) = state.missing_blobs.get(&hash) {
                            if now - last_attempt < 60 {
                                continue;
                            }
                        }

                        if let Some(hashes) = tx.blob_versioned_hashes() {
                            for h in hashes {
                                if !state.blobs.contains_key(h) {
                                    missing.push(hash);
                                    break;
                                }
                            }
                        }
                    }
                    if missing.len() >= 32 { break; }
                }
                if missing.len() >= 32 { break; }
            }
        }
        missing
    }

    async fn mark_blob_sync_attempt(&self, tx_hashes: &[B256]) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
        let hashes = tx_hashes.to_vec();
        self.inner.update_state(move |state| {
            for hash in &hashes {
                state.missing_blobs.insert(*hash, now);
            }
        });
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
    fn remove_transactions_now(&self, _tx_hashes: &[B256]) -> Vec<Address> { Vec::new() }
    async fn get_all_transactions(&self) -> Vec<Transaction> { Vec::new() }
    async fn get_transactions_by_sender(&self, _address: Address) -> Vec<Transaction> { Vec::new() }
    async fn len(&self) -> usize { 0 }
    async fn is_empty(&self) -> bool { true }
    async fn clear(&self) {}
    async fn base_fee(&self) -> U256 { U256::ZERO }
    async fn peek_best_transactions(&self, _target_gas_limit: u64, _base_fee: U256, _blob_base_fee: Option<U256>, _max_blobs_per_block: Option<u32>) -> (Vec<Transaction>, Vec<Transaction>) {
        (Vec::new(), Vec::new())
    }
    fn snapshot(&self) -> MempoolSnapshot { MempoolSnapshot { pending: ImmutableMap::new(), blobs: Self::empty_blobs(), generation: 0 } }
    async fn add_blob(&self, _versioned_hash: B256, _blob: Blob, _commitment: Bytes48, _proof: Bytes48) {}
    async fn get_blob(&self, _versioned_hash: B256) -> Option<(Blob, Bytes48, Bytes48)> { None }
    async fn get_transaction(&self, _hash: B256) -> Option<Transaction> { None }
    async fn add_pooled_envelope(&self, _hash: B256, _pooled: TxPooledEnvelope) {}
    async fn get_pooled_envelope(&self, _hash: B256) -> Option<TxPooledEnvelope> { None }
    async fn add_pooled_bytes(&self, _hash: B256, _bytes: Bytes) {}
    async fn get_pooled_bytes(&self, _hash: B256) -> Option<Bytes> { None }
    async fn next_expected_nonce(&self, _address: Address, state_nonce: u64) -> u64 { state_nonce }
    async fn get_transactions_with_missing_blobs(&self) -> Vec<B256> { Vec::new() }
    async fn mark_blob_sync_attempt(&self, _tx_hashes: &[B256]) {}
    async fn nonce_lookup(&self, _address: Address, state_nonce: u64) -> (u64, u64) { (state_nonce, state_nonce) }
}
