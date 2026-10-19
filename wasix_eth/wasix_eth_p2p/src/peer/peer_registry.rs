use anyhow::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::{HeaderProvider, PeerDiscoveryProvider};
use wasix_eth_storage::write::DatabaseWriteProvider;
use wasix_eth_storage::write_traits::PeerDiscoveryWriter;
use wasix_eth_types::sync::{P2pSession, PeerProvider};
use wasix_eth_types::{async_trait, BlockId, ChainConfig, ChainManager, PeerEntry};
use wasix_eth_utils::identity::Identity;
use wasix_eth_utils::metrics::CONNECTED_PEERS;
use wasix_eth_utils::{debug, info};

use crate::rlpx::PeerSession;
use alloy_primitives::B256;
use std::collections::HashMap;
use tokio::sync::{mpsc, Mutex, MutexGuard};
use tokio::time::{timeout, Duration};
use wasix_eth_types::p2p::{DisconnectReason, StatusMessage};

#[derive(Clone)]
pub struct RegisteredSession {
    pub session_id: u64,
    pub session: Arc<PeerSession>,
}

pub struct DisconnectEvent {
    pub peer_id: String,
    pub session_id: u64,
}

#[derive(Clone)]
pub struct PeerRegistry {
    pub read_provider: DatabaseReadProvider,
    write_provider: DatabaseWriteProvider,
    local_identity: Identity,
    discovery_port: u16,
    p2p_port: u16,
    pub ext_ip: Option<std::net::IpAddr>,
    active_sessions: Arc<Mutex<HashMap<String, RegisteredSession>>>,
    pub network_id: u64,
    pub genesis_hash: B256,
    pub chain_config: ChainConfig,
    pub chain_manager: Arc<dyn ChainManager>,
    discovery_service_v4: Arc<Mutex<Option<Arc<crate::discovery::v4_service::DiscoveryV4Service>>>>,
    gossip_tx: Arc<Mutex<Option<mpsc::Sender<wasix_eth_types::p2p::GossipMessage>>>>,
    disconnect_tx: mpsc::Sender<DisconnectEvent>,
    next_session_id: Arc<AtomicU64>,
}

#[async_trait]
impl PeerProvider for PeerRegistry {
    async fn get_active_peers(&self) -> Result<Vec<PeerEntry>> {
        let mut peers = self.read_provider.get_active_peers().await?;
        let Some(sessions) = self.active_sessions_guard().await else {
            return Ok(peers);
        };

        for (peer_id, session) in sessions.iter() {
            if !peers.iter().any(|p| p.peer_id == *peer_id) {
                peers.push(PeerEntry {
                    peer_id: peer_id.clone(),
                    discovery_addr: session.session.remote_addr,
                    p2p_addr: session.session.remote_addr,
                });
            }
        }
        Ok(peers)
    }

    async fn get_session(&self, peer_id: &str) -> Option<Arc<dyn P2pSession>> {
        let sessions = self.active_sessions_guard().await?;
        sessions.get(peer_id).cloned().map(|s| s.session as Arc<dyn P2pSession>)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn disconnect_peer(&self, peer_id: &str) -> Result<()> {
        // This is a manual disconnect, we might not have the session_id here easily.
        // But for manual disconnects from RPC/etc, we can just send a special 0 session_id or similar,
        // or just look it up.
        let registry = self.clone();
        let peer_id_owned = peer_id.to_string();
        tokio::spawn(async move {
            if let Some(sessions) = registry.active_sessions_guard().await {
                if let Some(reg) = sessions.get(&peer_id_owned) {
                    let _ = registry.disconnect_tx.try_send(DisconnectEvent {
                        peer_id: peer_id_owned,
                        session_id: reg.session_id,
                    });
                }
            }
        });
        Ok(())
    }
}

impl PeerRegistry {
    async fn active_sessions_guard(&self) -> Option<MutexGuard<'_, HashMap<String, RegisteredSession>>> {
        match timeout(Duration::from_secs(2), self.active_sessions.lock()).await {
            Ok(guard) => Some(guard),
            Err(_) => {
                debug!("[P2P Registry] Timed out waiting for active_sessions lock");
                None
            }
        }
    }

    pub fn new(
        read_provider: DatabaseReadProvider,
        write_provider: DatabaseWriteProvider,
        local_identity: Identity,
        discovery_port: u16,
        p2p_port: u16,
        ext_ip: Option<std::net::IpAddr>,
        _bootnodes: Vec<String>,
        network_id: u64,
        genesis_hash: B256,
        chain_config: ChainConfig,
        chain_manager: Arc<dyn ChainManager>,
    ) -> Self {
        let (disconnect_tx, mut disconnect_rx) = mpsc::channel(100);
        let registry = Self {
            read_provider,
            write_provider,
            local_identity,
            discovery_port,
            p2p_port,
            ext_ip,
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
            network_id,
            genesis_hash,
            chain_config,
            chain_manager,
            discovery_service_v4: Arc::new(Mutex::new(None)),
            gossip_tx: Arc::new(Mutex::new(None)),
            disconnect_tx,
            next_session_id: Arc::new(AtomicU64::new(1)),
        };

        let registry_for_disconnect = registry.clone();
        tokio::spawn(async move {
            while let Some(event) = disconnect_rx.recv().await {
                info!("[P2P] Peer disconnected: {} (session: {})", event.peer_id, event.session_id);

                let removed_len = if let Some(mut sessions) = registry_for_disconnect.active_sessions_guard().await {
                    if let Some(current) = sessions.get(&event.peer_id) {
                        if current.session_id == event.session_id {
                            sessions.remove(&event.peer_id);
                            Some(sessions.len())
                        } else {
                            debug!("[P2P] Ignoring stale disconnect for peer {} (current: {}, event: {})", event.peer_id, current.session_id, event.session_id);
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some(len) = removed_len {
                    CONNECTED_PEERS.set(len as f64);
                    let _ = registry_for_disconnect.write_provider.remove_peer(event.peer_id);
                }
            }
        });

        registry
    }

    pub async fn set_gossip_tx(&self, tx: mpsc::Sender<wasix_eth_types::p2p::GossipMessage>) {
        let mut guard = self.gossip_tx.lock().await;
        *guard = Some(tx);
    }

    pub async fn get_gossip_tx(&self) -> Option<mpsc::Sender<wasix_eth_types::p2p::GossipMessage>> {
        let guard = self.gossip_tx.lock().await;
        guard.clone()
    }

    pub fn disconnect_tx(&self) -> mpsc::Sender<DisconnectEvent> {
        self.disconnect_tx.clone()
    }

    pub fn next_session_id(&self) -> u64 {
        self.next_session_id.fetch_add(1, Ordering::SeqCst)
    }

    pub async fn is_current_session(&self, peer_id: &str, session_id: u64) -> bool {
        if let Some(sessions) = self.active_sessions_guard().await {
            if let Some(current) = sessions.get(peer_id) {
                return current.session_id == session_id;
            }
        }
        false
    }

    pub async fn set_discovery_service_v4(&self, service: Arc<crate::discovery::v4_service::DiscoveryV4Service>) {
        let mut guard = self.discovery_service_v4.lock().await;
        *guard = Some(service);
    }

    pub async fn get_discovery_service_v4(&self) -> Option<Arc<crate::discovery::v4_service::DiscoveryV4Service>> {
        let guard = self.discovery_service_v4.lock().await;
        guard.clone()
    }

    pub fn local_peer_id(&self) -> String {
        self.local_identity.peer_id()
    }

    pub fn p2p_port(&self) -> u16 {
        self.p2p_port
    }

    pub fn discovery_port(&self) -> u16 {
        self.discovery_port
    }

    pub fn local_identity(&self) -> Identity {
        self.local_identity.clone()
    }

    pub async fn get_fork_id(&self) -> wasix_eth_types::p2p::ForkId {
        let (head_hash, head_num) = self.chain_manager.head_block().await;
        
        let head = self.read_provider.header(BlockId::Hash(head_hash.into())).await.ok().flatten();
        let head_time = head.as_ref().map(|h| h.timestamp).unwrap_or(0);

        let genesis = self.read_provider.header(BlockId::Hash(self.genesis_hash.into())).await.ok().flatten();
        let genesis_time = genesis.as_ref().map(|h| h.timestamp).unwrap_or(0);

        wasix_eth_types::p2p::ForkId::new(self.genesis_hash, &self.chain_config, head_num, head_time, genesis_time)
    }

    pub async fn get_session(&self, peer_id: &str) -> Option<Arc<dyn P2pSession>> {
        let sessions = self.active_sessions_guard().await?;
        sessions.get(peer_id).cloned().map(|s| s.session as Arc<dyn P2pSession>)
    }

    pub async fn get_all_sessions(&self) -> Vec<Arc<PeerSession>> {
        let Some(sessions) = self.active_sessions_guard().await else {
            return Vec::new();
        };
        sessions.values().map(|s| s.session.clone()).collect()
    }

    pub async fn register_session_arc(&self, peer_id: String, session_id: u64, session: Arc<PeerSession>) {
        info!("[P2P Registry] Registering session for peer {} (session_id: {})", peer_id, session_id);

        let mut reject_new = false;
        let mut disconnect_existing: Option<Arc<PeerSession>> = None;
        let mut connected_len = None;
        let mut inserted = false;

        {
            let Some(mut sessions) = self.active_sessions_guard().await else {
                let _ = session.disconnect(DisconnectReason::TooManyPeers).await;
                return;
            };

            if let Some(existing_reg) = sessions.get(&peer_id) {
                let existing = &existing_reg.session;
                let local_id = self.local_peer_id();
                let we_are_higher = local_id > peer_id;

                let keep_existing = if we_are_higher {
                    existing.is_initiator
                } else {
                    !existing.is_initiator
                };

                if keep_existing {
                    info!(
                        "[P2P Registry] Deterministic tie-break: keeping existing session for peer {}, rejecting new one (local higher: {}, existing initiator: {})",
                        peer_id, we_are_higher, existing.is_initiator
                    );
                    reject_new = true;
                } else {
                    info!(
                        "[P2P Registry] Deterministic tie-break: replacing existing session for peer {} (local higher: {}, existing initiator: {}, new initiator: {})",
                        peer_id, we_are_higher, existing.is_initiator, session.is_initiator
                    );
                    disconnect_existing = Some(existing.clone());
                    sessions.insert(peer_id.clone(), RegisteredSession { session_id, session: session.clone() });
                    connected_len = Some(sessions.len());
                    inserted = true;
                }
            } else {
                sessions.insert(peer_id.clone(), RegisteredSession { session_id, session: session.clone() });
                connected_len = Some(sessions.len());
                inserted = true;
            }
        }

        if reject_new {
            let _ = session.disconnect(DisconnectReason::AlreadyConnected).await;
            return;
        }

        if let Some(existing) = disconnect_existing {
            let _ = existing.disconnect(DisconnectReason::AlreadyConnected).await;
        }

        if inserted {
            if let Some(len) = connected_len {
                CONNECTED_PEERS.set(len as f64);
            }
        }
    }

    pub async fn cleanup_stale_peers(&self) {
        debug!("[P2P] Starting health check / cleanup cycle...");
        let sessions = self.get_all_sessions().await;
        let now = std::time::Instant::now();
        let timeout_duration = std::time::Duration::from_secs(30);

        for session in sessions {
            let last_activity = *session.last_activity.lock().await;
            if now.duration_since(last_activity) > timeout_duration {
                let block_hash = {
                    let status = session.status.lock().await;
                    status.as_ref().map(|s| match s {
                        StatusMessage::Legacy(l) => l.blockhash,
                        StatusMessage::Eth69(e) => e.blockhash,
                    })
                };

                debug!(
                    "[P2P] Peer with head {:?} stale for too long ({}s), disconnecting",
                    block_hash,
                    now.duration_since(last_activity).as_secs()
                );

                let _ = session.disconnect(DisconnectReason::PingTimeout).await;
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        }
    }
    pub async fn get_active_peers(&self) -> Result<Vec<PeerEntry>> {
        let mut peers = self.read_provider.get_active_peers().await?;
        let Some(sessions) = self.active_sessions_guard().await else {
            return Ok(peers);
        };

        for (peer_id, reg) in sessions.iter() {
            if !peers.iter().any(|p| p.peer_id == *peer_id) {
                peers.push(PeerEntry {
                    peer_id: peer_id.clone(),
                    discovery_addr: reg.session.remote_addr,
                    p2p_addr: reg.session.remote_addr,
                });
            }
        }
        Ok(peers)
    }

    pub async fn register_peer(&self, entry: PeerEntry) -> Result<()> {
        self.write_provider.register_peer(entry).await
    }

    pub async fn remove_peer(&self, peer_id: String) -> Result<()> {
        self.write_provider.remove_peer(peer_id).await
    }
}