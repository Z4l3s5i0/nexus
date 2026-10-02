use serde::{Deserialize, Serialize};
use jsonrpsee::proc_macros::rpc;
use crate::error::RpcResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub id: String,
    pub name: String,
    pub enode: String,
    pub enr: Option<String>,
    pub ip: String,
    pub ports: NodePorts,
    pub listen_addr: String,
    pub protocols: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodePorts {
    pub discovery: u16,
    pub listener: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerInfo {
    pub id: String,
    pub name: String,
    pub enode: String,
    pub remote_address: String,
    pub local_address: String,
    pub protocols: serde_json::Value,
}

#[rpc(server, client)]
pub trait AdminApi {
    #[method(name = "admin_nodeInfo")]
    async fn node_info(&self) -> RpcResult<NodeInfo>;

    #[method(name = "admin_addPeer")]
    async fn add_peer(&self, enode: String) -> RpcResult<bool>;

    #[method(name = "admin_peers")]
    async fn peers(&self) -> RpcResult<Vec<PeerInfo>>;
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct PeerEntry {
    pub peer_id: String,
    pub discovery_addr: std::net::SocketAddr,
    pub p2p_addr: std::net::SocketAddr,
}

#[derive(Clone)]
pub struct PeerInfoDetailed {
    pub discovery_addr: std::net::SocketAddr,
    pub p2p_addr: std::net::SocketAddr,
    pub discovery_url: String,
    pub p2p_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloResponse {
    pub peer_id: String,
    pub discovery_addr: String,
    pub p2p_addr: String,
}
