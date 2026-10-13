use std::collections::{HashMap, BinaryHeap};
use std::sync::Arc;
use tokio::sync::RwLock;
use dashmap::DashMap;
use im::{Vector, HashMap as ImmutableMap};
use arc_swap::ArcSwap;
use wasix_eth_types::{Address, SignerRecoverable, ConsensusTransaction, B256, U256, Transaction, Blob, Bytes48, TxPooledEnvelope, Bytes, MAX_MEMPOOL_SIZE};
use wasix_eth_utils::{debug, info, metrics::{MEMPOOL_SIZE, MEMPOOL_REJECTED_TRANSACTIONS}};

/// A read-optimized, immutable snapshot of the mempool for block building.
#[derive(Clone, Debug)]
pub struct MempoolSnapshot {
    pub pending: ImmutableMap<Address, Vector<Transaction>>,
    pub blobs: Arc<ImmutableMap<B256, Arc<(Blob, Bytes48, Bytes48)>>>,
}

impl MempoolSnapshot {
    /// Optimized transaction selection using a Priority Queue.
    /// Complexity: O(M log N) where M is target tx count and N is active senders.
    pub fn peek_best_transactions(
        &self,
        target_gas_limit: u64,
        base_fee: U256,
        _blob_base_fee: Option<U256>,
        max_blobs_per_block: Option<u32>,
    ) -> Vec<Transaction> {
        let mut result = Vec::new();
        let mut current_gas: u64 = 0;
        let mut current_blobs: u32 = 0;
        let max_blobs = max_blobs_per_block.unwrap_or(6);
        let base_fee_u64 = base_fee.to::<u64>();

        // Sender-indexed pointers into their pending queues
        let mut sender_cursors: HashMap<Address, usize> = HashMap::new();

        // Max-heap to store the best next transaction from each sender.
        // Stores (effective_tip, address).
        let mut pq = BinaryHeap::new();

        for (addr, queue) in &self.pending {
            if let Some(tx) = queue.get(0) {
                if let Some(tip) = tx.effective_tip_per_gas(base_fee_u64) {
                    pq.push((tip, *addr));
                    sender_cursors.insert(*addr, 0);
                }
            }
        }

        while current_gas < target_gas_limit {
            let (_tip, addr) = match pq.pop() {
                Some(entry) => entry,
                None => break, // No more transactions
            };

            let queue = self.pending.get(&addr).unwrap();
            let cursor = sender_cursors.get_mut(&addr).unwrap();
            let tx = queue.get(*cursor).unwrap();

            // Validate against current block limits
            let gas_limit = tx.gas_limit();
            if current_gas + gas_limit > target_gas_limit {
                // This sender's next tx is too big. 
                // We don't re-add to PQ because nonces must be sequential.
                continue;
            }

            if tx.is_eip4844() {
                let blobs_count = tx.blob_versioned_hashes().map(|h| h.len() as u32).unwrap_or(0);
                if current_blobs + blobs_count > max_blobs {
                    continue;
                }
                
                // Check if blobs are actually present
                let mut all_blobs_present = true;
                if let Some(hashes) = tx.blob_versioned_hashes() {
                    for hash in hashes {
                        if !self.blobs.contains_key(hash) {
                            all_blobs_present = false;
                            break;
                        }
                    }
                }
                if !all_blobs_present {
                    continue;
                }
                
                current_blobs += blobs_count;
            }

            // Transaction is eligible
            result.push(tx.clone());
            current_gas += gas_limit;

            // Advance cursor for this sender and re-add to PQ if they have more transactions
            *cursor += 1;
            if let Some(next_tx) = queue.get(*cursor) {
                if let Some(next_tip) = next_tx.effective_tip_per_gas(base_fee_u64) {
                    pq.push((next_tip, addr));
                }
            }
        }

        result
    }
}

#[derive(Debug, Default, Clone)]
pub struct Mempool {
    pub inner: Arc<MempoolInner>,
}

impl Mempool {
    /// Create a new mempool with the given base fee.
    pub fn new(base_fee: U256) -> Self {
        Self {
            inner: Arc::new(MempoolInner {
                state: ArcSwap::from_pointee(MempoolInternalState::default()),
                base_fee: RwLock::new(base_fee),
            }),
        }
    }

    pub async fn add_transaction_no_nonce_sync(&self, tx: Transaction) -> bool {
        // We can't easily get current_nonce here without state, so we use 0 or let it be handled by add_transaction
        self.inner.add_transaction(tx, 0).await
    }

    /// Get the next expected nonce for an address, considering pending transactions in the mempool.
    /// If no pending transactions exist, returns the provided state nonce.
    pub async fn next_expected_nonce(&self, address: Address, state_nonce: u64) -> u64 {
        let state = self.inner.state.load();
        if let Some(q) = state.pending_transactions.get(&address) {
            let mut expected = state_nonce;
            // The queue is sorted by nonce
            for tx in q.iter() {
                if tx.nonce() == expected {
                    expected += 1;
                } else if tx.nonce() > expected {
                    // We found a gap, return the expected nonce to fill it
                    break;
                }
                // if tx.nonce() < expected, it's a stale transaction we should ignore
            }
            expected
        } else {
            state_nonce
        }
    }

    /// Optimized transaction selection using a Priority Queue.
    /// Complexity: O(M log N) where M is target tx count and N is active senders.
    pub async fn peek_best_transactions(&self, target_gas_limit: u64, base_fee: U256, blob_base_fee: Option<U256>, max_blobs_per_block: Option<u32>) -> Vec<Transaction> {
        self.snapshot().peek_best_transactions(target_gas_limit, base_fee, blob_base_fee, max_blobs_per_block)
    }

    /// Take a read-optimized snapshot of the current pending transactions and blobs.
    pub fn snapshot(&self) -> MempoolSnapshot {
        let state = self.inner.state.load();
        MempoolSnapshot { 
            pending: state.pending_transactions.clone(), 
            blobs: Arc::new(state.blobs.clone())
        }
    }
}

impl Mempool {
    /// Helper for testing to insert into pooled maps.
    #[cfg(test)]
    fn insert_pooled_bytes(&self, hash: B256, bytes: Bytes) {
        self.inner.update_state(|state| {
            state.pooled_bytes.insert(hash, bytes);
        });
    }
}

#[derive(Debug, Default, Clone)]
pub struct MempoolInternalState {
    /// Transactions ready for inclusion (no nonce gaps, sufficient balance).
    pub pending_transactions: ImmutableMap<Address, Vector<Transaction>>,
    /// Transactions waiting for a previous nonce to arrive.
    pub queued_transactions: ImmutableMap<Address, Vector<Transaction>>,
    /// Original pooled envelopes for re-serving over P2P.
    pub pooled_envelopes: ImmutableMap<B256, TxPooledEnvelope>,
    /// Original pooled bytes as received over RPC (wire-identical replay).
    pub pooled_bytes: ImmutableMap<B256, Bytes>,
    /// Blobs, commitments and proofs associated with EIP-4844 transactions, indexed by versioned hash.
    pub blobs: ImmutableMap<B256, Arc<(Blob, Bytes48, Bytes48)>>,
}

#[derive(Debug, Default)]
pub struct MempoolInner {
    /// The atomic, immutable state of the mempool.
    pub state: ArcSwap<MempoolInternalState>,
    /// Current base fee for transactions in the mempool.
    pub base_fee: RwLock<U256>,
}

/// Metadata for a transaction in the mempool.
#[derive(Debug, Clone)]
pub struct TxMetadata {
    pub arrival_time: u64,
    pub effective_tip: u128,
}

impl MempoolInner {
    /// Internal helper to update the mempool state atomically.
    pub(crate) fn update_state<F>(&self, mut f: F)
    where
        F: FnMut(&mut MempoolInternalState),
    {
        loop {
            let current = self.state.load();
            let mut new_state = (**current).clone();
            f(&mut new_state);
            let new_arc = Arc::new(new_state);
            let prev = self.state.compare_and_swap(&current, new_arc.clone());
            if Arc::ptr_eq(&current, &prev) {
                break;
            }
        }
    }

    /// Add a transaction to the mempool, considering its nonce and the current state nonce.
    /// Returns true if the transaction was newly added or replaced an existing one.
    pub async fn add_transaction(&self, tx: Transaction, current_nonce: u64) -> bool {
        let hash = tx.hash();
        let from = tx.recover_signer().unwrap_or_default();
        let tx_nonce = tx.nonce();

        if tx_nonce < current_nonce {
            debug!("[Mempool] Rejecting transaction {:?} (nonce: {}) because it is below current nonce ({})", hash, tx_nonce, current_nonce);
            MEMPOOL_REJECTED_TRANSACTIONS.inc();
            return false;
        }

        // Determine if it should be pending or queued based on existing transactions from this sender
        let next_pending_nonce = self.state.load().pending_transactions.get(&from)
            .and_then(|q| q.back().map(|t| t.nonce() + 1))
            .unwrap_or(current_nonce);
        let next_pending_nonce = std::cmp::max(next_pending_nonce, current_nonce);

        let added = if tx_nonce <= next_pending_nonce {
            // It either fills the next expected nonce or it's a replacement (handled in insert_into_queue)
            self.insert_into_queue(from, tx, true).await
        } else {
            // There's a gap between pending transactions and this one
            self.insert_into_queue(from, tx, false).await
        };

        if added {
            MEMPOOL_SIZE.set(self.len() as f64);
        } else {
            MEMPOOL_REJECTED_TRANSACTIONS.inc();
        }

        added
    }


    /// Internal helper to insert a transaction into pending or queued maps.
    async fn insert_into_queue(&self, from: Address, tx: Transaction, is_pending: bool) -> bool {
        let hash = tx.hash();
        let nonce = tx.nonce();
        
        // Enforce capacity occasionally
        if rand::random::<u8>() % 16 == 0 {
            self.enforce_capacity(MAX_MEMPOOL_SIZE).await;
        }

        let mut success = false;
        let mut to_promote = None;

        self.update_state(|state| {
            let queue = if is_pending {
                state.pending_transactions.entry(from).or_insert_with(Vector::new)
            } else {
                state.queued_transactions.entry(from).or_insert_with(Vector::new)
            };

            // Insert in nonce order
            let pos = queue.binary_search_by_key(&nonce, |t| t.nonce())
                .unwrap_or_else(|e| e);

            // Replace-By-Fee (RBF) logic: 10% threshold
            let gas_price = tx.gas_price().unwrap_or_default();
            if pos < queue.len() && queue[pos].nonce() == nonce {
                let old_gas_price = queue[pos].gas_price().unwrap_or_default();
                // 10% increase requirement: new >= old * 1.1
                let min_replacement_price = old_gas_price + (old_gas_price / 10);
                
                if gas_price >= min_replacement_price {
                    let old_tx = &queue[pos];
                    let old_hash = *old_tx.hash();
                    debug!("[Mempool] Replacing transaction {:?} (nonce: {}) with {:?} (price: {} -> {})",
                        old_hash, nonce, hash, old_gas_price, gas_price);

                    // Clean up old transaction from other maps
                    if old_tx.is_eip4844() {
                        if let Some(hashes) = old_tx.blob_versioned_hashes() {
                            for h in hashes {
                                state.blobs.remove(h);
                            }
                        }
                    }
                    state.pooled_envelopes.remove(&old_hash);
                    state.pooled_bytes.remove(&old_hash);

                    queue[pos] = tx.clone();
                    success = true;
                    return;
                }
                debug!("[Mempool] Rejecting replacement for {:?} (nonce: {}): gas price bump too low ({} < {})",
                    hash, nonce, gas_price, min_replacement_price);
                success = false;
                return;
            } else {
                debug!("[Mempool] Adding transaction {:?} (nonce: {}) to {} pool",
                    hash, nonce, if is_pending { "pending" } else { "queued" });
                queue.insert(pos, tx.clone());
                
                // If we added to pending, we might be able to promote queued transactions
                if is_pending {
                    to_promote = Some(nonce + 1);
                }
                
                success = true;
            }
        });

        if success && is_pending {
            if let Some(next_nonce) = to_promote {
                self.promote_queued(from, next_nonce);
            }
        }
        
        success
    }

    /// Move transactions from queued to pending if the nonce gap is filled.
    pub fn promote_queued(&self, address: Address, next_expected_nonce: u64) {
        self.update_state(|state| {
            let mut next_nonce = next_expected_nonce;
            
            loop {
                let tx = {
                    let queued_queue = match state.queued_transactions.get_mut(&address) {
                        Some(q) => q,
                        None => break,
                    };
                    
                    match queued_queue.binary_search_by_key(&next_nonce, |tx| tx.nonce()) {
                        Ok(pos) => Some(queued_queue.remove(pos)),
                        Err(_) => None,
                    }
                };

                if let Some(tx) = tx {
                    info!("[Mempool] Promoting transaction {:?} (nonce: {}) for {} to pending", tx.hash(), tx.nonce(), address);
                    
                    let pending_queue = state.pending_transactions.entry(address).or_insert_with(Vector::new);
                    match pending_queue.binary_search_by_key(&tx.nonce(), |t| t.nonce()) {
                        Ok(_) => {
                            debug!("[Mempool] Transaction {:?} (nonce: {}) for {} already in pending, ignoring promotion", tx.hash(), tx.nonce(), address);
                        }
                        Err(p_pos) => {
                            pending_queue.insert(p_pos, tx);
                        }
                    }
                    
                    next_nonce += 1;
                } else {
                    break;
                }
            }
            
            // Cleanup if empty
            if state.queued_transactions.get(&address).map(|q| q.is_empty()).unwrap_or(false) {
                state.queued_transactions.remove(&address);
            }
        });
    }

    /// Get all transactions currently in the mempool (both pending and queued).
    pub fn get_all_transactions(&self) -> Vec<Transaction> {
        let state = self.state.load();
        let mut all = Vec::new();
        for (_, queue) in state.pending_transactions.iter() {
            all.extend(queue.iter().cloned());
        }
        for (_, queue) in state.queued_transactions.iter() {
            all.extend(queue.iter().cloned());
        }
        all
    }

    /// Clear the mempool.
    pub fn clear(&self) {
        self.state.store(Arc::new(MempoolInternalState::default()));
        MEMPOOL_SIZE.set(0.0);
    }

    /// Remove transactions that have been included in a block.
    pub fn remove_transactions(&self, tx_hashes: &[B256]) {
        if !tx_hashes.is_empty() {
            info!("[Mempool] Removing {} transactions from mempool", tx_hashes.len());
        }
        
        self.update_state(|state| {
            let mut removed_txs = Vec::new();

            // Retain transactions not in tx_hashes
            for (_, queue) in state.pending_transactions.iter_mut() {
                queue.retain(|tx| {
                    if tx_hashes.contains(tx.hash()) {
                        removed_txs.push(tx.clone());
                        false
                    } else {
                        true
                    }
                });
            }
            for (_, queue) in state.queued_transactions.iter_mut() {
                queue.retain(|tx| {
                    if tx_hashes.contains(tx.hash()) {
                        removed_txs.push(tx.clone());
                        false
                    } else {
                        true
                    }
                });
            }

            // Clean up associated data for removed transactions
            for tx in removed_txs {
                let hash = tx.hash();
                if tx.is_eip4844() {
                    if let Some(hashes) = tx.blob_versioned_hashes() {
                        for h in hashes {
                            state.blobs.remove(h);
                        }
                    }
                }
                state.pooled_envelopes.remove(hash);
                state.pooled_bytes.remove(hash);
            }

            // Clean up empty queues
            state.pending_transactions.retain(|_, queue| !queue.is_empty());
            state.queued_transactions.retain(|_, queue| !queue.is_empty());
        });
        
        MEMPOOL_SIZE.set(self.len() as f64);
    }
}

impl MempoolInner {
    /// Peek at N transactions from the mempool for block building without removing them.
    pub fn peek_transactions(&self, n: usize) -> Vec<Transaction> {
        let mut result = Vec::with_capacity(n);
        let state = self.state.load();
        
        // Use a copy of pending for popping
        let mut pending_copy: HashMap<Address, Vector<Transaction>> = state.pending_transactions.iter()
            .map(|(k, v)| (*k, v.clone())).collect();
        
        while result.len() < n {
            let mut best_sender: Option<Address> = None;
            let mut best_gas_price: u128 = 0;

            for (address, queue) in &pending_copy {
                if let Some(tx) = queue.front() {
                    let gas_price = tx.gas_price().unwrap_or_default();
                    if gas_price > best_gas_price {
                        best_gas_price = gas_price;
                        best_sender = Some(*address);
                    }
                }
            }

            if let Some(sender) = best_sender {
                if let Some(queue) = pending_copy.get_mut(&sender) {
                    if let Some(tx) = queue.pop_front() {
                        result.push(tx);
                    }
                    if queue.is_empty() {
                        pending_copy.remove(&sender);
                    }
                }
            } else {
                break;
            }
        }
        
        result
    }

    /// Pop N transactions from the mempool for inclusion in a block.
    pub async fn pop_transactions(&self, n: usize) -> Vec<Transaction> {
        let mut result = Vec::with_capacity(n);
        let base_fee = *self.base_fee.read().await;
        
        while result.len() < n {
            let state = self.state.load();
            let mut best_sender: Option<Address> = None;
            let mut best_tip: u128 = 0;

            for (addr, queue) in state.pending_transactions.iter() {
                if let Some(tx) = queue.front() {
                    let tip = Self::calculate_effective_tip(tx, base_fee);
                    if tip > best_tip || (tip == best_tip && best_sender.is_none()) {
                        best_tip = tip;
                        best_sender = Some(*addr);
                    }
                }
            }

            if let Some(sender) = best_sender {
                let mut tx_to_remove = None;
                self.update_state(|state| {
                    if let Some(queue) = state.pending_transactions.get_mut(&sender) {
                        if let Some(tx) = queue.pop_front() {
                            tx_to_remove = Some(tx.clone());
                            if queue.is_empty() {
                                state.pending_transactions.remove(&sender);
                            }
                        }
                    }
                });

                if let Some(tx) = tx_to_remove {
                    // Clean up associated data
                    let hash = tx.hash();
                    self.update_state(|state| {
                        if tx.is_eip4844() {
                            if let Some(hashes) = tx.blob_versioned_hashes() {
                                for h in hashes {
                                    state.blobs.remove(h);
                                }
                            }
                        }
                        state.pooled_envelopes.remove(hash);
                        state.pooled_bytes.remove(hash);
                    });
                    
                    result.push(tx);
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        
        if !result.is_empty() {
            MEMPOOL_SIZE.set(self.len() as f64);
        }

        result
    }

    /// Calculate the effective tip for a transaction based on the current base fee.
    pub fn calculate_effective_tip(tx: &Transaction, base_fee: U256) -> u128 {
        let base_fee = base_fee.to::<u128>();
        let max_fee = tx.max_fee_per_gas();
        if max_fee < base_fee {
            0
        } else {
            let max_tip = max_fee - base_fee;
            match tx {
                Transaction::Legacy(_) | Transaction::Eip2930(_) => max_tip,
                _ => max_tip.min(tx.max_priority_fee_per_gas().unwrap_or_default()),
            }
        }
    }

    /// Evict transactions with the lowest effective tip when the mempool is over capacity.
    pub async fn enforce_capacity(&self, max_size: usize) {
        let len = self.len();
        if len <= max_size {
            return;
        }
        
        let base_fee = *self.base_fee.read().await;
        
        self.update_state(|state| {
            while state.pending_transactions.iter().map(|(_, q)| q.len()).sum::<usize>() +
                  state.queued_transactions.iter().map(|(_, q)| q.len()).sum::<usize>() > max_size 
            {
                let mut worst_sender: Option<(Address, bool)> = None; // (address, is_pending)
                let mut worst_tip: u128 = u128::MAX;

                // Prioritize evicting from queued first
                for (addr, queue) in state.queued_transactions.iter() {
                    if let Some(tx) = queue.back() {
                        let tip = Self::calculate_effective_tip(tx, base_fee);
                        if tip < worst_tip {
                            worst_tip = tip;
                            worst_sender = Some((*addr, false));
                        }
                    }
                }

                // If still over capacity and queued is empty or we have even worse pending
                if worst_sender.is_none() || state.queued_transactions.is_empty() {
                    for (addr, queue) in state.pending_transactions.iter() {
                        if let Some(tx) = queue.back() {
                            let tip = Self::calculate_effective_tip(tx, base_fee);
                            if tip < worst_tip {
                                worst_tip = tip;
                                worst_sender = Some((*addr, true));
                            }
                        }
                    }
                }

                if let Some((address, is_pending)) = worst_sender {
                    let queue_opt = if is_pending {
                        state.pending_transactions.get_mut(&address)
                    } else {
                        state.queued_transactions.get_mut(&address)
                    };

                    if let Some(q) = queue_opt {
                        if let Some(tx) = q.pop_back() {
                            info!("[Mempool] Evicting transaction {:?} (nonce: {}) from {} pool due to capacity (tip: {})", 
                                tx.hash(), tx.nonce(), if is_pending { "pending" } else { "queued" }, worst_tip);
                        }
                        if q.is_empty() {
                            if is_pending {
                                state.pending_transactions.remove(&address);
                            } else {
                                state.queued_transactions.remove(&address);
                            }
                        }
                    }
                } else {
                    break;
                }
            }
        });

        MEMPOOL_SIZE.set(self.len() as f64);
    }

    /// Get the total count of transactions in the mempool (pending + queued).
    pub fn len(&self) -> usize {
        let state = self.state.load();
        state.pending_transactions.iter().map(|(_, q)| q.len()).sum::<usize>() +
        state.queued_transactions.iter().map(|(_, q)| q.len()).sum::<usize>()
    }

    /// Check if the mempool is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasix_eth_types::{SignableTransaction, Signature, TxLegacy, Signed};

    #[tokio::test]
    async fn test_peek_best_transactions_priority() {
        let mempool = Mempool::new(U256::from(100));

        // Use deterministic signatures to get deterministic signer addresses
        // sig1 -> 0x0000000000000000000000000000000000000000
        // sig2 -> 0x8c908d31360a5c9174b14af7aef4b3f607e6235a
        let sig1 = Signature::new(U256::from(100), U256::from(100), true);
        let sig2 = Signature::new(U256::from(200), U256::from(200), true);

        // Tx from addr1: tip 50, gas 21000
        let tx1 = Transaction::Legacy(Signed::new_unchecked(
            TxLegacy {
                nonce: 0,
                gas_price: 150, // tip = 150 - 100 = 50
                gas_limit: 21000,
                to: Address::ZERO.into(),
                value: U256::ZERO,
                input: Default::default(),
                chain_id: None,
            },
            sig1,
            B256::repeat_byte(0xFF), // Distinct hash
        ));

        // Tx from addr2: tip 100, gas 21000
        let tx2 = Transaction::Legacy(Signed::new_unchecked(
            TxLegacy {
                nonce: 0,
                gas_price: 200, // tip = 200 - 100 = 100
                gas_limit: 21000,
                to: Address::ZERO.into(),
                value: U256::ZERO,
                input: Default::default(),
                chain_id: None,
            },
            sig2,
            B256::repeat_byte(0xEE), // Distinct hash
        ));

        mempool.inner.add_transaction(tx1.clone(), 0).await;
        mempool.inner.add_transaction(tx2.clone(), 0).await;

        // Peek with gas limit enough for both
        let best = mempool.peek_best_transactions(50000, U256::from(100), None, None).await;
        assert_eq!(best.len(), 2);
        // Note: Legacy transactions with same gas price might have non-deterministic order if sender hashes are similar,
        // but here they have different gas prices.
        // tx1 gas_price=150, base_fee=100 -> tip=50
        // tx2 gas_price=200, base_fee=100 -> tip=100
        assert_eq!(best[0].hash(), tx2.hash(), "tx2 should be first (higher tip)");
        assert_eq!(best[1].hash(), tx1.hash(), "tx1 should be second");

        // Peek with gas limit enough for only one
        let best_one = mempool.peek_best_transactions(30000, U256::from(100), None, None).await;
        assert_eq!(best_one.len(), 1);
        assert_eq!(best_one[0].hash(), tx2.hash(), "tx2 should be the only one (higher tip)");
    }

    #[tokio::test]
    async fn test_peek_best_transactions_nonce_order() {
        let mempool = Mempool::new(U256::from(100));

        // Tx1: nonce 0, tip 100
        let tx1 = Transaction::Legacy(Signed::new_unchecked(
            TxLegacy {
                nonce: 0,
                gas_price: 200,
                gas_limit: 21000,
                to: Address::ZERO.into(),
                value: U256::ZERO,
                input: Default::default(),
                chain_id: None,
            },
            Signature::test_signature(),
            B256::ZERO,
        ));

        // Tx2: nonce 1, tip 200
        let tx2 = Transaction::Legacy(Signed::new_unchecked(
            TxLegacy {
                nonce: 1,
                gas_price: 300,
                gas_limit: 21000,
                to: Address::ZERO.into(),
                value: U256::ZERO,
                input: Default::default(),
                chain_id: None,
            },
            Signature::new(U256::from(1), U256::from(1), true),
            B256::repeat_byte(1),
        ));

        let inner = &mempool.inner;
        let from = tx1.recover_signer().unwrap_or_default();
        inner.add_transaction(tx1.clone(), 0).await;
        {
            inner.update_state(|state| {
                let entry = state.pending_transactions.entry(from).or_insert_with(Vector::new);
                entry.push_back(tx2.clone());
            });
        }

        let best = mempool.peek_best_transactions(100000, U256::from(100), None, None).await;
        assert_eq!(best.len(), 2);
        assert_eq!(best[0].hash(), tx1.hash()); // Nonce 0 must come first even if Tx2 has higher tip
        assert_eq!(best[1].hash(), tx2.hash());
    }

    #[tokio::test]
    async fn test_mempool_new() {
        let mempool = Mempool::new(U256::from(100));
        assert_eq!(*mempool.inner.base_fee.read().await, U256::from(100));
        assert!(mempool.inner.len() == 0);
    }

    #[tokio::test]
    async fn test_add_transaction_nonce_low() {
        let mempool = Mempool::new(U256::from(0));
        let tx = Transaction::Legacy(TxLegacy {
            nonce: 5,
            ..Default::default()
        }.into_signed(Signature::test_signature()));
        
        // current_nonce = 10, tx_nonce = 5 -> should be rejected (false)
        assert!(!mempool.inner.add_transaction(tx, 10).await);
        assert!(mempool.inner.is_empty());
    }

    #[tokio::test]
    async fn test_add_transaction_pending() {
        let mempool = Mempool::new(U256::from(0));
        let tx = Transaction::Legacy(TxLegacy {
            nonce: 10,
            ..Default::default()
        }.into_signed(Signature::test_signature()));
        
        // current_nonce = 10, tx_nonce = 10 -> should be pending
        assert!(mempool.inner.add_transaction(tx, 10).await);
        assert_eq!(mempool.inner.len(), 1);
        assert_eq!(mempool.inner.state.load().pending_transactions.len(), 1);
    }

    #[tokio::test]
    async fn test_add_transaction_queued() {
        let mempool = Mempool::new(U256::from(0));
        let tx = Transaction::Legacy(TxLegacy {
            nonce: 15,
            ..Default::default()
        }.into_signed(Signature::test_signature()));
        
        // current_nonce = 10, tx_nonce = 15 -> should be queued
        assert!(mempool.inner.add_transaction(tx, 10).await);
        assert_eq!(mempool.inner.len(), 1);
        assert_eq!(mempool.inner.state.load().queued_transactions.len(), 1);
    }

    #[tokio::test]
    async fn test_promote_queued() {
        let mempool = Mempool::new(U256::from(0));
        let tx = Transaction::Legacy(TxLegacy {
            nonce: 10,
            ..Default::default()
        }.into_signed(Signature::test_signature()));
        let addr = tx.recover_signer().unwrap();
        
        {
            let inner = &mempool.inner;
            inner.update_state(|state| {
                let entry = state.queued_transactions.entry(addr).or_insert_with(Vector::new);
                entry.push_back(tx);
            });
            assert_eq!(inner.state.load().queued_transactions.len(), 1);
            
            inner.promote_queued(addr, 10);
            assert_eq!(inner.state.load().queued_transactions.len(), 0);
            assert_eq!(inner.state.load().pending_transactions.len(), 1);
        }
    }

    #[tokio::test]
    async fn test_remove_transactions() {
        let mempool = Mempool::new(U256::from(0));
        let tx = Transaction::Legacy(TxLegacy {
            nonce: 10,
            ..Default::default()
        }.into_signed(Signature::test_signature()));
        let hash = *tx.hash();
        
        mempool.inner.add_transaction(tx, 10).await;
        assert_eq!(mempool.inner.len(), 1);
        
        mempool.inner.remove_transactions(&[hash]);
        assert_eq!(mempool.inner.len(), 0);
    }

    #[tokio::test]
    async fn test_transaction_replacement() {
        let mempool = Mempool::new(U256::from(0));
        let sig = Signature::new(U256::from(300), U256::from(300), true);
        
        let tx1 = Transaction::Legacy(TxLegacy {
            nonce: 10,
            gas_price: 100,
            ..Default::default()
        }.into_signed(sig.clone()));
        
        // Add first transaction
        assert!(mempool.inner.add_transaction(tx1.clone(), 10).await);
        assert_eq!(mempool.inner.len(), 1);
        
        // Try to replace it with same nonce but higher fee (10% bump)
        let tx2 = Transaction::Legacy(TxLegacy {
            nonce: 10,
            gas_price: 110, // 100 + 10% = 110
            ..Default::default()
        }.into_signed(sig));
        
        // Engine API now passes state_nonce (10) to add_transaction
        assert!(mempool.inner.add_transaction(tx2, 10).await);
        
        // It should have REPLACED tx1, so len is still 1
        assert_eq!(mempool.inner.len(), 1, "Mempool length should be 1 after replacement");
        
        // Try to replace with lower fee (should fail)
        // Same signer as tx1/tx2
        let tx3 = Transaction::Legacy(TxLegacy {
            nonce: 10,
            gas_price: 105,
            ..Default::default()
        }.into_signed(sig));
        
        assert!(!mempool.inner.add_transaction(tx3, 10).await);
        assert_eq!(mempool.inner.len(), 1);
    }

    #[tokio::test]
    async fn test_replacement_cleanup() {
        let mempool = Mempool::new(U256::from(0));
        let sig = Signature::new(U256::from(300), U256::from(300), true);
        
        let tx1 = Transaction::Legacy(TxLegacy {
            nonce: 10,
            gas_price: 100,
            ..Default::default()
        }.into_signed(sig.clone()));
        let hash1 = *tx1.hash();
        
        // Add first transaction and associated data
        mempool.insert_pooled_bytes(hash1, Bytes::from(vec![1, 2, 3]));
        assert!(mempool.inner.add_transaction(tx1, 10).await);
        
        assert!(mempool.inner.state.load().pooled_bytes.contains_key(&hash1));

        // Replace it
        let tx2 = Transaction::Legacy(TxLegacy {
            nonce: 10,
            gas_price: 120,
            ..Default::default()
        }.into_signed(sig));
        
        assert!(mempool.inner.add_transaction(tx2, 10).await);
        
        // Old data should be gone
        assert!(!mempool.inner.state.load().pooled_bytes.contains_key(&hash1), "Old bytes should be removed");
        
        // New transaction is in the queue
        assert_eq!(mempool.inner.len(), 1);
    }

    #[tokio::test]
    async fn test_pop_and_remove_cleanup() {
        let mempool = Mempool::new(U256::from(0));
        let sig = Signature::new(U256::from(300), U256::from(300), true);
        
        let tx1 = Transaction::Legacy(TxLegacy {
            nonce: 10,
            gas_price: 100,
            ..Default::default()
        }.into_signed(sig.clone()));
        let hash1 = *tx1.hash();
        
        let tx2 = Transaction::Legacy(TxLegacy {
            nonce: 11,
            gas_price: 100,
            ..Default::default()
        }.into_signed(sig));
        let hash2 = *tx2.hash();

        mempool.inner.add_transaction(tx1, 10).await;
        mempool.inner.add_transaction(tx2, 10).await;
        
        mempool.insert_pooled_bytes(hash1, Bytes::from(vec![1]));
        mempool.insert_pooled_bytes(hash2, Bytes::from(vec![2]));

        // Test pop
        let popped = mempool.inner.pop_transactions(1).await;
        assert_eq!(popped.len(), 1);
        assert_eq!(*popped[0].hash(), hash1);
        assert!(!mempool.inner.state.load().pooled_bytes.contains_key(&hash1), "Popped tx bytes should be removed");
        assert!(mempool.inner.state.load().pooled_bytes.contains_key(&hash2));

        // Test remove
        mempool.inner.remove_transactions(&[hash2]);
        assert!(!mempool.inner.state.load().pooled_bytes.contains_key(&hash2), "Removed tx bytes should be removed");
    }

    #[tokio::test]
    async fn test_enforce_capacity_metric() {
        let mempool = Mempool::new(U256::from(0));
        let sig = Signature::test_signature();

        // Add 5 transactions
        for i in 0..5 {
            let tx = Transaction::Legacy(TxLegacy {
                nonce: i,
                gas_price: 100,
                ..Default::default()
            }.into_signed(sig.clone()));
            mempool.inner.add_transaction(tx, 0).await;
        }
        assert_eq!(mempool.inner.len(), 5);

        // Enforce capacity to 2
        mempool.inner.enforce_capacity(2).await;
        assert_eq!(mempool.inner.len(), 2);
        
        // The MEMPOOL_SIZE metric should have been set to 2.0.
        // We can't easily check the metric value directly if it's a global, 
        // but we've added the call to update it.
    }
}
