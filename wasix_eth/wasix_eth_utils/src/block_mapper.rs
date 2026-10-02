use wasix_eth_types::rpc::{RpcBlock, RpcTransaction};
use wasix_eth_types::{Block, ChainConfig, TxEnvelope, U256};
pub struct BlockMapper;

use alloy_consensus::transaction::SignerRecoverable;
use alloy_primitives::Sealed;

impl BlockMapper {
    pub fn to_rpc_block(
        block: Block<TxEnvelope>,
        full: bool,
        _chain_config: &ChainConfig,
        total_difficulty: Option<U256>
    ) -> RpcBlock {
        let block_hash = block.header.hash_slow();
        let block_number = block.header.number;

        let header = block.header.clone();
        let body = block.body;

        let transactions = if full {
            alloy_rpc_types::eth::BlockTransactions::Full(
                body.transactions.into_iter().enumerate().map(|(idx, tx)| {
                    let signer = tx.recover_signer().unwrap_or_default();
                    let recovered = alloy_consensus::transaction::Recovered::new_unchecked(tx, signer);
                    RpcTransaction::from_transaction(recovered, alloy_rpc_types::eth::TransactionInfo {
                        block_hash: Some(block_hash),
                        block_number: Some(block_number),
                        index: Some(idx as u64),
                        ..Default::default()
                    })
                }).collect()
            )
        } else {
            alloy_rpc_types::eth::BlockTransactions::Hashes(
                body.transactions.iter().map(|tx| *tx.hash()).collect()
            )
        };

        let sealed_header = Sealed::new(header);

        RpcBlock {
            header: alloy_rpc_types::eth::Header::from_consensus(sealed_header, total_difficulty, Some(block_hash.into())),
            transactions,
            uncles: Vec::new(),
            withdrawals: body.withdrawals,
        }
    }
}