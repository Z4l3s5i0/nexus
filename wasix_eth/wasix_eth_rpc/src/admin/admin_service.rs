use std::sync::Arc;
use wasix_eth_p2p::PeerManager;
use wasix_eth_types::admin::{NodeInfo, NodePorts};
use wasix_eth_types::{U256, hex};
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::{BlockProvider, HeaderProvider};
use serde_json::json;
use wasix_eth_types::sync::P2pSession;

#[derive(Clone)]
pub struct AdminService {
    peer_manager: Arc<PeerManager>,
    read_provider: Arc<DatabaseReadProvider>,
    ext_ip: String,
}

impl AdminService {
    pub fn new(
        peer_manager: Arc<PeerManager>,
        read_provider: Arc<DatabaseReadProvider>,
        ext_ip: Option<String>,
    ) -> Self {
        let registry_ext_ip = peer_manager.registry.ext_ip.map(|ip| ip.to_string());
        Self {
            peer_manager,
            read_provider,
            ext_ip: ext_ip.or(registry_ext_ip).unwrap_or_else(|| "127.0.0.1".to_string()),
        }
    }

    pub async fn node_info(&self) -> wasix_eth_types::Result<NodeInfo> {
        let identity = self.peer_manager.registry.local_identity();
        let pubkey = identity.public_key_b512();
        let peer_id_hex = hex::encode(pubkey.0);
        
        let p2p_port = self.peer_manager.registry.p2p_port();
        let disc_port = self.peer_manager.registry.discovery_port();
        
        let enode = format!("enode://{}@{}:{}?discport={}", peer_id_hex, self.ext_ip, p2p_port, disc_port);
        
        let enr = if let Some(disc) = self.peer_manager.registry.get_discovery_service_v4().await {
            let enr_struct = disc.get_local_enr_struct().await;
            Some(enr_struct.to_base64())
        } else {
            None
        };
        
        let head_hash = self.read_provider.forkchoice("head").await?
            .unwrap_or_default();
        let head_td = self.read_provider.header_td(head_hash).await?
            .unwrap_or(U256::ZERO);
        let genesis_hash = self.read_provider.block_hash(0).await?
            .unwrap_or_default();

        let node_info = NodeInfo {
            id: peer_id_hex,
            name: "wasix-eth/v0.1.0".to_string(),
            enode,
            enr,
            ip: self.ext_ip.clone(),
            ports: NodePorts {
                discovery: disc_port,
                listener: p2p_port,
            },
            listen_addr: format!("{}:{}", self.ext_ip, p2p_port),
            protocols: json!({
                "eth": {
                    "network": self.peer_manager.registry.network_id,
                    "difficulty": head_td,
                    "genesis": genesis_hash,
                    "head": head_hash,
                }
            }),
        };

        wasix_eth_utils::debug!("[RPC] admin_nodeInfo result: {}", serde_json::to_string(&node_info).unwrap_or_default());

        Ok(node_info)
    }

    pub async fn add_peer(&self, enode: String) -> wasix_eth_types::Result<bool> {
        self.peer_manager.dial_enode(&enode);
        Ok(true)
    }

    pub async fn peers(&self) -> wasix_eth_types::Result<Vec<wasix_eth_types::admin::PeerInfo>> {
        let sessions = self.peer_manager.registry.get_all_sessions().await;
        let mut peers = Vec::new();

        for session in sessions {
            let status = session.eth_status().await;
            let best_height = session.best_height().await;
            
            let id = "unknown".to_string(); // We don't easily have the original pubkey hex here without extra storage
            
            peers.push(wasix_eth_types::admin::PeerInfo {
                id: id.clone(),
                name: "unknown".to_string(),
                enode: "unknown".to_string(),
                remote_address: "unknown".to_string(),
                local_address: "unknown".to_string(),
                protocols: json!({
                    "eth": {
                        "version": 68,
                        "difficulty": status.as_ref().map(|s| s.total_difficulty).unwrap_or_default(),
                        "head": status.as_ref().map(|s| s.blockhash).unwrap_or_default(),
                        "height": best_height,
                    }
                }),
            });
        }

        Ok(peers)
    }
}
