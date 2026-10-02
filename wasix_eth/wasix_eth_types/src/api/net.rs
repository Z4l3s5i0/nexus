use jsonrpsee::proc_macros::rpc;
use crate::error::RpcResult;
use alloy_primitives::U256;

#[rpc(server, client)]
pub trait NetApi {
    #[method(name = "net_peerCount")]
    async fn peer_count(&self) -> RpcResult<U256>;
}
