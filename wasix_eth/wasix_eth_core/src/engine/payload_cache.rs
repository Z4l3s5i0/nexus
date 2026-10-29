use dashmap::DashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use wasix_eth_types::{Address, B256, Bytes, PayloadExecutionResult, TrieAccount, U256};
use wasix_eth_storage::read_traits::StateProvider;

use std::sync::Arc;

#[derive(Clone)]
pub struct PayloadExecutionCache {
    results: Arc<DashMap<B256, PayloadExecutionResult>>,
    order: Arc<Mutex<VecDeque<B256>>>,
    max_size: usize,
}

impl PayloadExecutionCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            results: Arc::new(DashMap::new()),
            order: Arc::new(Mutex::new(VecDeque::new())),
            max_size,
        }
    }

    pub fn insert(&self, hash: B256, result: PayloadExecutionResult) {
        if self.results.contains_key(&hash) {
            self.results.insert(hash, result);
            return;
        }

        let mut order = self.order.lock().unwrap();
        if order.len() >= self.max_size {
            if let Some(oldest) = order.pop_front() {
                self.results.remove(&oldest);
            }
        }
        order.push_back(hash);
        self.results.insert(hash, result);
    }

    pub fn get(&self, hash: &B256) -> Option<PayloadExecutionResult> {
        self.results.get(hash).map(|r| r.value().clone())
    }
}

impl StateProvider for PayloadExecutionCache {
    fn trie_node_from_cache(&self, root: B256, hash: B256) -> anyhow::Result<Option<Bytes>> {
        if let Some(res) = self.results.get(&root) {
            for (node_hash, node_bytes) in &res.trie_nodes {
                if *node_hash == hash {
                    return Ok(Some(node_bytes.clone()));
                }
            }
        }
        Ok(None)
    }

    fn account_from_cache(&self, root: B256, address: Address) -> anyhow::Result<Option<TrieAccount>> {
        if let Some(res) = self.results.get(&root) {
            for (addr, acc) in &res.new_accounts {
                if *addr == address {
                    return Ok(Some(acc.clone()));
                }
            }
        }
        Ok(None)
    }

    fn storage_from_cache(&self, root: B256, address: Address, slot: B256) -> anyhow::Result<Option<U256>> {
        if let Some(res) = self.results.get(&root) {
            for (addr, s_slot, val) in &res.new_storages {
                if *addr == address && *s_slot == slot {
                    return Ok(Some(*val));
                }
            }
        }
        Ok(None)
    }

    fn bytecode_from_cache(&self, root: B256, code_hash: B256) -> anyhow::Result<Option<Bytes>> {
        if let Some(res) = self.results.get(&root) {
            for (hash, code) in &res.new_bytecodes {
                if *hash == code_hash {
                    return Ok(Some(code.clone()));
                }
            }
        }
        Ok(None)
    }

    fn plain_state(&self, _address: Address) -> anyhow::Result<Option<Bytes>> { Ok(None) }
    fn hashed_state(&self, _hash: B256) -> anyhow::Result<Option<Bytes>> { Ok(None) }
    fn trie_node(&self, _hash: B256) -> anyhow::Result<Option<Bytes>> { Ok(None) }
}
