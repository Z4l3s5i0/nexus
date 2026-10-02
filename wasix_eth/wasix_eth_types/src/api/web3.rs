use jsonrpsee::proc_macros::rpc;
use crate::error::RpcResult;

#[rpc(server, client)]
pub trait Web3Api {
    #[method(name = "web3_clientVersion")]
    async fn client_version(&self) -> RpcResult<String>;
}
