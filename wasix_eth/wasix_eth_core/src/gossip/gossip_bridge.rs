use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use wasix_eth_utils::info;
use crate::{EngineEvent, GossipProvider};
use std::collections::HashSet;
use wasix_eth_types::{B256, Transaction};
use tokio::time::{interval, Duration};

pub struct GossipBridge {
    gossip: Arc<dyn GossipProvider>,
    seen_hashes: Mutex<HashSet<B256>>,
    pending_txs: Mutex<Vec<Transaction>>,
}

impl GossipBridge {
    pub fn new(gossip: Arc<dyn GossipProvider>) -> Self {
        Self { 
            gossip,
            seen_hashes: Mutex::new(HashSet::with_capacity(2048)),
            pending_txs: Mutex::new(Vec::with_capacity(128)),
        }
    }

    pub async fn run(&self, mut event_rx: broadcast::Receiver<EngineEvent>) {
        info!("[GossipBridge] Starting Engine-to-P2P event bridge");
        
        let mut broadcast_interval = interval(Duration::from_millis(50));
        
        loop {
            tokio::select! {
                event_res = event_rx.recv() => {
                    match event_res {
                        Ok(event) => {
                            match event {
                                EngineEvent::NewTransaction { tx, is_local } => {
                                    let hash = *tx.hash();
                                    let mut seen = self.seen_hashes.lock().await;
                                    
                                    if seen.contains(&hash) {
                                        continue;
                                    }

                                    // For local transactions, we always broadcast.
                                    // For remote transactions, we broadcast only if we haven't seen them before.
                                    if is_local || !seen.contains(&hash) {
                                        if seen.len() > 10000 {
                                            seen.clear();
                                        }
                                        seen.insert(hash);
                                        drop(seen);
                                        
                                        let mut pending = self.pending_txs.lock().await;
                                        pending.push(tx);
                                        if pending.len() >= 128 {
                                            let txs = std::mem::replace(&mut *pending, Vec::with_capacity(128));
                                            drop(pending);
                                            self.broadcast_tx_batch(txs).await;
                                        }
                                    }
                                }
                                EngineEvent::NewBlock { .. } => {
                                    // Block gossip is disabled. 
                                    // New blocks are transmitted via Consensus Client and P2P on-demand syncing.
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    }
                }
                _ = broadcast_interval.tick() => {
                    let mut pending = self.pending_txs.lock().await;
                    if !pending.is_empty() {
                        let txs = std::mem::replace(&mut *pending, Vec::with_capacity(128));
                        drop(pending);
                        self.broadcast_tx_batch(txs).await;
                    }
                }
            }
        }
    }

    async fn broadcast_tx_batch(&self, txs: Vec<Transaction>) {
        if txs.is_empty() {
            return;
        }
        
        if txs.len() == 1 {
            info!("[GossipBridge] Broadcasting 1 transaction");
            self.gossip.broadcast_transaction(&txs[0]).await;
        } else {
            info!("[GossipBridge] Broadcasting batch of {} transactions", txs.len());
            self.gossip.broadcast_new_pooled_transaction_hashes(txs).await;
        }
    }
}