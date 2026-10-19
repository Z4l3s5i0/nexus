use std::sync::Arc;
use alloy_primitives::B256;
use alloy_rpc_types::RpcBlockHash;
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::{BlockProvider, HeaderProvider};
use wasix_eth_types::{BlockId, BlockNumberOrTag, ChainManager, ForkchoiceState, ForkchoiceUpdated, Header};
use wasix_eth_types::error::{RpcError, RpcResult};
use wasix_eth_utils::{debug, error, warn};

#[derive(Clone)]
pub struct ForkchoiceValidator {
    read_storage: DatabaseReadProvider,
    chain: Arc<dyn ChainManager>,
}

impl ForkchoiceValidator {
    pub fn new(read_storage: DatabaseReadProvider, chain: Arc<dyn ChainManager>) -> Self {
        Self { read_storage, chain }
    }

    pub(crate) async fn check_finalized_block(&self, forkchoice_state: ForkchoiceState, header: Header) -> Option<RpcResult<ForkchoiceUpdated>> {
        if forkchoice_state.finalized_block_hash != B256::ZERO {
            let target_hash = if forkchoice_state.safe_block_hash != B256::ZERO {
                forkchoice_state.safe_block_hash
            } else {
                forkchoice_state.head_block_hash
            };

            let finalized_header = if let Some(h) = self.read_storage.header(BlockId::Hash(RpcBlockHash::from(forkchoice_state.finalized_block_hash))).await.ok().flatten() {
                Some(h)
            } else if let Some((p, _, _, _)) = self.read_storage.get_payload_by_block_hash(forkchoice_state.finalized_block_hash).await {
                Some(p.header.clone())
            } else {
                let genesis_hash = self.read_storage.block_hash(0).await.unwrap_or(None);
                if genesis_hash == Some(forkchoice_state.finalized_block_hash) {
                    self.read_storage.header(BlockId::Number(BlockNumberOrTag::Number(0))).await.ok().flatten()
                } else {
                    None
                }
            };

            if let Some(fh) = finalized_header {
                let target_num = if target_hash == forkchoice_state.head_block_hash {
                    header.number
                } else {
                    let h_opt = if let Some(h) = self.read_storage.header(BlockId::Hash(RpcBlockHash::from(target_hash))).await.ok().flatten() {
                        Some(h)
                    } else if let Some((p, _, _, _)) = self.read_storage.get_payload_by_block_hash(target_hash).await {
                        Some(p.header.clone())
                    } else {
                        let genesis_hash = self.read_storage.block_hash(0).await.unwrap_or(None);
                        if genesis_hash == Some(target_hash) {
                            self.read_storage.header(BlockId::Number(BlockNumberOrTag::Number(0))).await.ok().flatten()
                        } else {
                            None
                        }
                    };
                    h_opt.map(|h| h.number).unwrap_or(0) // Should be known if we reached here
                };

                debug!("[Engine] Found finalized block #{} with target_num #{}", fh.number, target_num);
                if fh.number > target_num || !self.is_ancestor(target_hash, forkchoice_state.finalized_block_hash).await {
                    warn!("[Engine] Inconsistent forkchoice: finalizedBlockHash {:?} (#{}) is not ancestor of target {:?} (#{})",
                                 forkchoice_state.finalized_block_hash, fh.number, target_hash, target_num);
                    return Some(Err(RpcError::InvalidForkchoiceState("finalizedBlockHash is not an ancestor of safeBlockHash or headBlockHash".to_string())));
                }
            } else {
                // finalizedBlockHash unknown
                warn!("[Engine] Inconsistent forkchoice: finalizedBlockHash {:?} is unknown while head is known", forkchoice_state.finalized_block_hash);
                return Some(Err(RpcError::InvalidForkchoiceState("finalizedBlockHash unknown".to_string())));
            }
        }
        None
    }

    pub(crate) async fn check_safe_block(&self, forkchoice_state: ForkchoiceState, header: &Header) -> Option<RpcResult<ForkchoiceUpdated>> {
        if forkchoice_state.safe_block_hash != B256::ZERO {
            let safe_header = if let Some(h) = self.read_storage.header(BlockId::Hash(RpcBlockHash::from(forkchoice_state.safe_block_hash))).await.ok().flatten() {
                Some(h)
            } else if let Some((p, _, _, _)) = self.read_storage.get_payload_by_block_hash(forkchoice_state.safe_block_hash).await {
                Some(p.header.clone())
            } else {
                let genesis_hash = self.read_storage.block_hash(0).await.unwrap_or(None);
                if genesis_hash == Some(forkchoice_state.safe_block_hash) {
                    self.read_storage.header(BlockId::Number(BlockNumberOrTag::Number(0))).await.ok().flatten()
                } else {
                    None
                }
            };

            if let Some(sh) = safe_header {
                debug!("[Engine] Found safe block #{}", sh.number);
                if sh.number > header.number || !self.is_ancestor(forkchoice_state.head_block_hash, forkchoice_state.safe_block_hash).await {
                    warn!("[Engine] Inconsistent forkchoice: safeBlockHash {:?} (#{}) is not ancestor of headBlockHash {:?} (#{})",
                                forkchoice_state.safe_block_hash, sh.number, forkchoice_state.head_block_hash, header.number);
                    return Some(Err(RpcError::InvalidForkchoiceState("safeBlockHash is not an ancestor of headBlockHash".to_string())));
                }
            } else {
                // safeBlockHash unknown but head is known - this is inconsistent
                warn!("[Engine] Inconsistent forkchoice: safeBlockHash {:?} is unknown while head is known", forkchoice_state.safe_block_hash);
                return Some(Err(RpcError::InvalidForkchoiceState("safeBlockHash unknown".to_string())));
            }
        }
        None
    }
    pub(crate) async fn check_header_without_headheader(&self, forkchoice_state: ForkchoiceState) -> Option<RpcResult<ForkchoiceUpdated>> {
        // head_header is None but head_status was Valid. This should only happen if determine_payload_status
        // is more permissive than our header lookup.
        let genesis_hash = self.read_storage.block_hash(0).await.unwrap_or(None);
        if Some(forkchoice_state.head_block_hash) != genesis_hash {
            error!("[Engine] Head block known but header not found: {:?}", forkchoice_state.head_block_hash);
            return Some(Err(RpcError::InvalidForkchoiceState("headBlockHash header not found".to_string())));
        }

        debug!("[Engine] Head block is GENESIS, validating safe/finalized against it");
        // If it's genesis, we know header.number = 0.
        if forkchoice_state.safe_block_hash != B256::ZERO {
            if forkchoice_state.safe_block_hash != forkchoice_state.head_block_hash {
                // If head is genesis and safe is not ZERO and not head, it can't be an ancestor.
                warn!("[Engine] Inconsistent forkchoice: head is genesis but safe is {:?}", forkchoice_state.safe_block_hash);
                return Some(Err(RpcError::InvalidForkchoiceState("safeBlockHash is not an ancestor of headBlockHash".to_string())));
            }
        }
        if forkchoice_state.finalized_block_hash != B256::ZERO {
            if forkchoice_state.finalized_block_hash != forkchoice_state.head_block_hash {
                warn!("[Engine] Inconsistent forkchoice: head is genesis but finalized is {:?}", forkchoice_state.finalized_block_hash);
                return Some(Err(RpcError::InvalidForkchoiceState("finalizedBlockHash is not an ancestor of headBlockHash".to_string())));
            }
        }
        None
    }

    async fn is_ancestor(&self, head: B256, target: B256) -> bool {
        self.chain.is_ancestor(head, target).await
    }
}