use alloy_consensus::transaction::SignerRecoverable;
use wasix_eth_types::{Address, B256, TxEnvelope, Header, TxKind, TransactionTrait};
use wasix_eth_types::rpc::{RpcTransaction, RpcReceipt};
use wasix_eth_types::consensus::{Receipt as WrappedReceipt, ReceiptMeta};
use wasix_eth_types::error::RpcError;

pub struct TransactionMapper;

impl TransactionMapper {
    pub fn to_rpc_transaction(tx: TxEnvelope, block_ref: Option<(u64, B256, u64)>, header: Option<Header>) -> RpcTransaction {
        let (block_number, block_hash, transaction_index) = match block_ref {
            Some((num, hash, index)) => (Some(num), Some(hash), Some(index)),
            None => (None, None, None),
        };
        
        let signer = tx.recover_signer().unwrap_or_default();
        let recovered = alloy_consensus::transaction::Recovered::new_unchecked(tx, signer);
        
        let tx_info = alloy_rpc_types::eth::TransactionInfo {
            block_hash,
            block_number,
            index: transaction_index,
            ..Default::default()
        };
        
        let mut rpc_tx = RpcTransaction::from_transaction(recovered, tx_info);
        
        if let Some(h) = header {
            rpc_tx.block_timestamp = Some(h.timestamp);
        }
        rpc_tx
    }

    pub fn to_rpc_receipt(
        wrapped_receipt: WrappedReceipt,
        _meta: Option<ReceiptMeta>,
        block_ref: Option<(u64, B256, u64)>,
        transaction: Option<&TxEnvelope>,
        tx_hash_override: Option<B256>,
        gas_used: u64,
        base_fee: Option<u64>,
        blob_gas_price: Option<u128>
    ) -> RpcReceipt {
        let (block_number, block_hash, transaction_index) = match block_ref {
            Some((num, hash, index)) => (Some(num), Some(hash), Some(index)),
            None => (None, None, None),
        };

        let receipt = wrapped_receipt.receipt;
        let logs_bloom = wrapped_receipt.logs_bloom;

        let transaction_hash = tx_hash_override.or(transaction.map(|tx| tx.hash()).copied()).unwrap_or_default();
        let from = transaction.map(|tx| tx.recover_signer().unwrap_or_default()).unwrap_or_default();
        let to = transaction.and_then(|tx| match tx.kind() {
            TxKind::Call(to) => Some(to),
            TxKind::Create => None,
        });

        let rpc_logs: Vec<alloy_rpc_types::eth::Log> = receipt.logs.iter().enumerate().map(|(i, l)| {
            alloy_rpc_types::eth::Log {
                inner: l.clone(),
                block_number,
                block_hash,
                transaction_hash: Some(transaction_hash),
                transaction_index,
                log_index: Some(i as u64),
                removed: false,
                block_timestamp: None,
            }
        }).collect();

        let rpc_receipt = alloy_rpc_types::eth::Receipt {
            status: receipt.status.into(),
            cumulative_gas_used: receipt.cumulative_gas_used as u64,
            logs: rpc_logs,
        };

        let inner = match transaction {
            Some(tx) => match tx {
                TxEnvelope::Legacy(_) => alloy_rpc_types::eth::ReceiptEnvelope::Legacy(alloy_consensus::ReceiptWithBloom {
                    receipt: rpc_receipt,
                    logs_bloom,
                }),
                TxEnvelope::Eip2930(_) => alloy_rpc_types::eth::ReceiptEnvelope::Eip2930(alloy_consensus::ReceiptWithBloom {
                    receipt: rpc_receipt,
                    logs_bloom,
                }),
                TxEnvelope::Eip1559(_) => alloy_rpc_types::eth::ReceiptEnvelope::Eip1559(alloy_consensus::ReceiptWithBloom {
                    receipt: rpc_receipt,
                    logs_bloom,
                }),
                TxEnvelope::Eip4844(_) => alloy_rpc_types::eth::ReceiptEnvelope::Eip4844(alloy_consensus::ReceiptWithBloom {
                    receipt: rpc_receipt,
                    logs_bloom,
                }),
                _ => alloy_rpc_types::eth::ReceiptEnvelope::Legacy(alloy_consensus::ReceiptWithBloom {
                    receipt: rpc_receipt,
                    logs_bloom,
                }),
            },
            None => alloy_rpc_types::eth::ReceiptEnvelope::Legacy(alloy_consensus::ReceiptWithBloom {
                receipt: rpc_receipt,
                logs_bloom,
            }),
        };

        let (b_gas_used, b_gas_price) = if let Some(TxEnvelope::Eip4844(signed_tx)) = transaction {
            let hashes = signed_tx.tx().blob_versioned_hashes();
            let used = hashes.as_ref().map(|h| h.len()).unwrap_or(0) as u64 * 131072;
            (Some(used), blob_gas_price)
        } else {
            (None, None)
        };

        RpcReceipt {
            inner,
            transaction_hash,
            transaction_index: transaction_index.map(|i| i),
            block_hash,
            block_number,
            from,
            to,
            effective_gas_price: transaction.map(|tx| {
                let max_fee = tx.max_fee_per_gas();
                if let Some(base_fee) = base_fee {
                    let priority_fee = tx.max_priority_fee_per_gas().unwrap_or(max_fee);
                    (base_fee as u128 + std::cmp::min(priority_fee, max_fee.saturating_sub(base_fee as u128))) as u128
                } else {
                    max_fee
                }
            }).unwrap_or(0),
            gas_used,
            contract_address: _meta.and_then(|m| m.contract_address),
            blob_gas_used: b_gas_used,
            blob_gas_price: b_gas_price,
        }
    }

    pub fn parse_address(addr: &str) -> Result<Address, RpcError> {
        addr.parse().map_err(|e| RpcError::InvalidParams(format!("Invalid address: {}", e)))
    }
}
