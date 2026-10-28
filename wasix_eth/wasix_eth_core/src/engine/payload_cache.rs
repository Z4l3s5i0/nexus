use dashmap::DashMap;
use std::collections::VecDeque;
use std::sync::Mutex;
use wasix_eth_types::{B256, PayloadExecutionResult};

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
