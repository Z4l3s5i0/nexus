use std::sync::Arc;
use std::time::Duration;
use wasix_eth_types::sync::PeerProvider;
use wasix_eth_types::p2p::{GetBlockHeaders, BlockHashOrNumber, GetBlockBodies, RequestPair, GetPooledTransactions};
use alloy_consensus::Block as ConsensusBlock;
use alloy_consensus::TxEnvelope as Transaction;
use alloy_primitives::{B256, U256};
use rand::Rng;
use rand::seq::IndexedRandom;
use wasix_eth_utils::{debug, exp};

pub struct Downloader {
    pub(crate) peer_provider: Arc<dyn PeerProvider>,
}

impl Downloader {
    pub fn new(peer_provider: Arc<dyn PeerProvider>) -> Self {
        Self { peer_provider }
    }

    pub async fn get_best_peer(&self) -> Option<(String, U256, u64)> {
        let peers = self.peer_provider.get_active_peers().await.ok()?;
        let mut best_peer = None;
        let mut max_td = U256::ZERO;
        let mut max_height = 0u64;

        for peer_info in peers.iter() {
            if let Some(session) = self.peer_provider.get_session(&peer_info.peer_id).await {
                let td = session.eth_status().await.map(|s| s.total_difficulty).unwrap_or(U256::ZERO);
                let height = session.best_height().await;
                
                // debug!("[Downloader] Peer {} has TD {} and height {}", peer_info.peer_id, td, height);

                // Prioritize TD first, then height.
                // If TD is zero (Eth69), we compare only by height.
                let is_better = if td > max_td {
                    true
                } else if td == max_td && td > U256::ZERO {
                    height > max_height
                } else if td == U256::ZERO {
                    height > max_height
                } else {
                    false
                };

                if is_better {
                    max_td = td;
                    max_height = height;
                    best_peer = Some((peer_info.peer_id.clone(), td, height));
                }
            }
        }
        best_peer
    }

    pub async fn get_any_peer(&self) -> Option<String> {
        let peers = self.peer_provider.get_active_peers().await.ok()?;
        if peers.is_empty() {
            return None;
        }
        
        let mut rng = rand::thread_rng();
        peers.choose(&mut rng).map(|p| p.peer_id.clone())
        //peers.first().map(|p| p.peer_id.clone())
    }

    pub async fn download_headers(&self, peer_id: &str, start: u64, amount: u64) -> anyhow::Result<Vec<wasix_eth_types::Header>> {
        exp!("[EXP] DOWNLOAD_HEADERS_START peer={} start={} amount={}", peer_id, start, amount);
        let session = self.peer_provider.get_session(peer_id).await
            .ok_or_else(|| anyhow::anyhow!("Session not found for peer {}", peer_id))?;

        let timeout = Duration::from_secs(90);
        let response = tokio::time::timeout(timeout, session.get_block_headers(RequestPair {
            request_id: rand::random(),
            message: GetBlockHeaders {
                block: BlockHashOrNumber::Number(start),
                amount,
                skip: 0,
                reverse: false,
            },
        })).await
        .map_err(|_| anyhow::anyhow!("get_block_headers timed out after {:?}", timeout))?
        .map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("channel closed") || err_str.contains("Session closed") {
                let _ = self.peer_provider.disconnect_peer(peer_id);
            }
            e
        })?;

        let res = Ok(response.message.0);
        if let Ok(headers) = &res {
            exp!("[EXP] DOWNLOAD_HEADERS_END peer={} count={}", peer_id, headers.len());
        }
        res
    }

    pub async fn download_bodies(&self, peer_id: &str, hashes: Vec<B256>) -> anyhow::Result<Vec<wasix_eth_types::BlockBody<Transaction>>> {
        exp!("[EXP] DOWNLOAD_BODIES_START peer={} count={}", peer_id, hashes.len());
        let session = self.peer_provider.get_session(peer_id).await
            .ok_or_else(|| anyhow::anyhow!("Session not found for peer {}", peer_id))?;

        let timeout = Duration::from_secs(90);

        let response = tokio::time::timeout(timeout, session.get_block_bodies(RequestPair {
            request_id: rand::random(),
            message: GetBlockBodies(hashes),
        })).await
        .map_err(|_| anyhow::anyhow!("get_block_bodies timed out after {:?}", timeout))?
        .map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("channel closed") || err_str.contains("Session closed") {
                let _ = self.peer_provider.disconnect_peer(peer_id);
            }
            e
        })?;

        let res = Ok(response.message.0);
        if let Ok(bodies) = &res {
            exp!("[EXP] DOWNLOAD_BODIES_END peer={} count={}", peer_id, bodies.len());
        }
        res
    }

    pub async fn download_block_by_hash(&self, peer_id: &str, hash: B256) -> anyhow::Result<ConsensusBlock<Transaction>> {
        let session = self.peer_provider.get_session(peer_id).await
            .ok_or_else(|| anyhow::anyhow!("Session not found for peer {}", peer_id))?;

        let response_headers = session.get_block_headers(RequestPair {
            request_id: rand::random(),
            message: GetBlockHeaders {
                block: BlockHashOrNumber::Hash(hash),
                amount: 1,
                skip: 0,
                reverse: false,
            },
        }).await?;

        let header = response_headers.message.0.into_iter().next()
            .ok_or_else(|| anyhow::anyhow!("No header returned for block hash {:?}", hash))?;

        let response_bodies = session.get_block_bodies(RequestPair {
            request_id: rand::random(),
            message: GetBlockBodies(vec![hash]),
        }).await?;

        let body = response_bodies.message.0.into_iter().next()
            .ok_or_else(|| anyhow::anyhow!("No body returned for block hash {:?}", hash))?;

        Ok(ConsensusBlock {
            header,
            body,
        })
    }

    pub async fn download_pooled_transactions(&self, peer_id: &str, hashes: Vec<B256>) -> anyhow::Result<Vec<wasix_eth_types::TxPooledEnvelope>> {
        let session = self.peer_provider.get_session(peer_id).await
            .ok_or_else(|| anyhow::anyhow!("Session not found for peer {}", peer_id))?;

        let response = session.get_pooled_transactions(RequestPair {
            request_id: rand::random(),
            message: GetPooledTransactions(hashes),
        }).await.map_err(|e| {
            let err_str = e.to_string();
            if err_str.contains("channel closed") || err_str.contains("Session closed") {
                let _ = self.peer_provider.disconnect_peer(peer_id);
            }
            e
        })?;

        let res = Ok(response.message.0);
        if let Ok(txs) = &res {
            exp!("[EXP] DOWNLOAD_POOLED_TXS_END peer={} count={}", peer_id, txs.len());
        }
        res
    }
}
