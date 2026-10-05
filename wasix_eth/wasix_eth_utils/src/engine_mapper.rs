use wasix_eth_types::Bytes;
use wasix_eth_types::Encodable2718;
use wasix_eth_types::ExecutionPayloadBodyV1;
use wasix_eth_types::ExecutionPayloadEnvelopeV2;
use wasix_eth_types::ExecutionPayloadEnvelopeV3;
use wasix_eth_types::ExecutionPayloadEnvelopeV4;
use wasix_eth_types::BlobsBundleV1;
use wasix_eth_types::ExecutionPayloadFieldV2;
use wasix_eth_types::ExecutionPayloadV1;
use wasix_eth_types::ExecutionPayloadV2;
use wasix_eth_types::ExecutionPayloadV3;
use wasix_eth_types::ExecutionPayloadV4;
use wasix_eth_types::Block;
use wasix_eth_types::proofs;
use wasix_eth_types::eip4895;
use wasix_eth_types::BlockBody;
use wasix_eth_types::Header;
use wasix_eth_types::Transaction;
use wasix_eth_types::B64;
use wasix_eth_types::EMPTY_OMMER_ROOT_HASH;
use wasix_eth_types::U256;

use wasix_eth_types::B256;
use wasix_eth_types::Hardfork;
use wasix_eth_types::ChainConfig;
use wasix_eth_types::{TxEip4844Variant, Signed, TxEip4844};

/// Encode a transaction for inclusion in an execution payload.
/// For EIP-4844 blob transactions, this strips the sidecar and encodes
/// only the consensus (non-network) format, as required by the Engine API spec.
fn encode_tx_for_payload(tx: &Transaction) -> Vec<u8> {
    match tx {
        Transaction::Eip4844(signed_tx) => {
            // Strip sidecar: re-create a Signed<TxEip4844Variant> with just TxEip4844
            let inner_tx: TxEip4844 = match signed_tx.tx() {
                TxEip4844Variant::TxEip4844(tx) => tx.clone(),
                TxEip4844Variant::TxEip4844WithSidecar(tx_sidecar) => tx_sidecar.tx.clone(),
            };
            let stripped = Signed::new_unchecked(
                TxEip4844Variant::<wasix_eth_types::BlobTransactionSidecarVariant>::TxEip4844(inner_tx),
                *signed_tx.signature(),
                *signed_tx.hash(),
            );
            let mut out = Vec::new();
            stripped.encode_2718(&mut out);
            out
        }
        _ => {
            let mut out = Vec::new();
            tx.encode_2718(&mut out);
            out
        }
    }
}

pub struct EngineMapper;

impl EngineMapper {
    pub fn to_execution_payload_v1(block: &Block<Transaction>, chain_config: &ChainConfig) -> ExecutionPayloadV1 {
        let fork = Hardfork::get_active_fork(chain_config, block.header.number, block.header.timestamp);
        ExecutionPayloadV1 {
            parent_hash: block.header.parent_hash,
            fee_recipient: block.header.beneficiary,
            state_root: block.header.state_root,
            receipts_root: block.header.receipts_root,
            logs_bloom: block.header.logs_bloom,
            prev_randao: block.header.mix_hash,
            block_number: block.header.number,
            gas_limit: block.header.gas_limit,
            gas_used: block.header.gas_used,
            timestamp: block.header.timestamp,
            extra_data: block.header.extra_data.clone(),
            base_fee_per_gas: if fork >= Hardfork::London { 
                U256::from(block.header.base_fee_per_gas.unwrap_or_default()) 
            } else { 
                U256::ZERO 
            },
            block_hash: block.header.hash_slow(),
            transactions: block.body.transactions.iter().map(|tx| {
                encode_tx_for_payload(tx).into()
            }).collect(),
        }
    }

    pub fn to_execution_payload_v2(block: &Block<Transaction>, chain_config: &ChainConfig) -> ExecutionPayloadV2 {
        ExecutionPayloadV2 {
            payload_inner: Self::to_execution_payload_v1(block, chain_config),
            withdrawals: block.body.withdrawals.as_ref().map(|w| w.0.iter().map(|wi| eip4895::Withdrawal {
                index: wi.index,
                validator_index: wi.validator_index,
                address: wi.address,
                amount: wi.amount,
            }).collect::<Vec<_>>()).unwrap_or_default(),
        }
    }
    pub fn to_execution_payload_envelope_v2(execution_payload_v2: ExecutionPayloadV2, block_value: U256, fork: Hardfork) -> ExecutionPayloadEnvelopeV2 {
        ExecutionPayloadEnvelopeV2 {
            execution_payload: if fork >= Hardfork::Shanghai {
                ExecutionPayloadFieldV2::V2(execution_payload_v2.into())
            } else {
                ExecutionPayloadFieldV2::V1(execution_payload_v2.payload_inner)
            },
            block_value,
        }
    }

    pub fn to_execution_payload_v3(block: &Block<Transaction>, chain_config: &ChainConfig) -> ExecutionPayloadV3 {
        ExecutionPayloadV3 {
            payload_inner: Self::to_execution_payload_v2(block, chain_config),
            blob_gas_used: block.header.blob_gas_used.unwrap_or_default(),
            excess_blob_gas: block.header.excess_blob_gas.unwrap_or_default(),
        }
    }

    pub fn to_execution_payload_envelope_v3(execution_payload_v3: ExecutionPayloadV3, block_value: U256, blobs_bundle: BlobsBundleV1) -> ExecutionPayloadEnvelopeV3 {
        ExecutionPayloadEnvelopeV3 {
            execution_payload: execution_payload_v3.into(),
            block_value,
            blobs_bundle,
            should_override_builder: false,
        }
    }

    pub fn to_execution_payload_v4(block: &Block<Transaction>, chain_config: &ChainConfig) -> ExecutionPayloadV4 {
        ExecutionPayloadV4 {
            payload_inner: Self::to_execution_payload_v3(block, chain_config),
            block_access_list: vec![].into(),
            slot_number: 0,
        }
    }

    pub fn to_execution_payload_envelope_v4(execution_payload_v4: ExecutionPayloadV4, block_value: U256, blobs_bundle: BlobsBundleV1, execution_requests: Vec<Bytes>) -> ExecutionPayloadEnvelopeV4 {
        ExecutionPayloadEnvelopeV4 {
            envelope_inner: ExecutionPayloadEnvelopeV3 {
                execution_payload: execution_payload_v4.payload_inner.into(),
                block_value,
                blobs_bundle,
                should_override_builder: false,
            },
            execution_requests: execution_requests.into(),
        }
    }

    pub fn to_execution_payload_body_v1(block: &Block<Transaction>) -> ExecutionPayloadBodyV1 {
        ExecutionPayloadBodyV1 {
            transactions: block.body.transactions.iter().map(|tx| {
                encode_tx_for_payload(tx).into()
            }).collect(),
            withdrawals: Option::from(block.body.withdrawals.clone().map(|w| w.0.iter().map(|wi| eip4895::Withdrawal {
                index: wi.index,
                validator_index: wi.validator_index,
                address: wi.address,
                amount: wi.amount,
            }).collect::<Vec<_>>()).unwrap_or_default()),
        }
    }

    pub fn payload_v3_to_block(
        payload: &ExecutionPayloadV3,
        transactions: Vec<Transaction>,
        parent_beacon_block_root: B256,
        chain_config: &ChainConfig,
    ) -> Block<Transaction> {
        Self::payload_v1_to_block(
            &payload.payload_inner.payload_inner,
            transactions,
            Some(payload.payload_inner.withdrawals.clone()),
            chain_config,
            Some(payload.blob_gas_used),
            Some(payload.excess_blob_gas),
            Some(parent_beacon_block_root),
        )
    }

    pub fn payload_v4_to_block(
        payload: &ExecutionPayloadV4,
        transactions: Vec<Transaction>,
        parent_beacon_block_root: B256,
        _execution_requests: Vec<Bytes>,
        chain_config: &ChainConfig,
    ) -> Block<Transaction> {
        let block = Self::payload_v3_to_block(
            &payload.payload_inner,
            transactions,
            parent_beacon_block_root,
            chain_config,
        );
        // TODO: calculate requests_hash if needed for Prague
        block
    }

    pub fn payload_v1_to_block(
        payload: &ExecutionPayloadV1,
        transactions: Vec<Transaction>,
        withdrawals: Option<Vec<alloy_rpc_types::Withdrawal>>,
        chain_config: &ChainConfig,
        blob_gas_used: Option<u64>,
        excess_blob_gas: Option<u64>,
        parent_beacon_block_root: Option<B256>,
    ) -> Block<Transaction> {
        let fork = Hardfork::get_active_fork(chain_config, payload.block_number, payload.timestamp);

        let header = Header {
            parent_hash: payload.parent_hash,
            beneficiary: payload.fee_recipient,
            state_root: payload.state_root,
            transactions_root: proofs::calculate_transaction_root(&transactions),
            receipts_root: payload.receipts_root,
            logs_bloom: payload.logs_bloom,
            mix_hash: payload.prev_randao,
            number: payload.block_number,
            gas_limit: payload.gas_limit,
            gas_used: payload.gas_used,
            timestamp: payload.timestamp,
            extra_data: payload.extra_data.clone(),
            base_fee_per_gas: if fork >= Hardfork::London { Some(payload.base_fee_per_gas.to::<u64>()) } else { None },
            withdrawals_root: if fork >= Hardfork::Shanghai {
                withdrawals.as_ref().map(|w| {
                    let withdrawals_vec: Vec<_> = w.iter().map(|wi| eip4895::Withdrawal {
                        index: wi.index,
                        validator_index: wi.validator_index,
                        address: wi.address,
                        amount: wi.amount,
                    }).collect();
                    proofs::calculate_withdrawals_root(&withdrawals_vec)
                }).or_else(|| Some(proofs::calculate_withdrawals_root(&[])))
            } else {
                None
            },
            blob_gas_used: if fork >= Hardfork::Cancun { Some(blob_gas_used.unwrap_or(0)) } else { None },
            excess_blob_gas: if fork >= Hardfork::Cancun { Some(excess_blob_gas.unwrap_or(0)) } else { None },
            parent_beacon_block_root: if fork >= Hardfork::Cancun { Some(parent_beacon_block_root.unwrap_or(B256::ZERO)) } else { None },
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            difficulty: if fork >= Hardfork::Paris { U256::ZERO } else { U256::from(0x30000) }, // Use 0x30000 as default pre-merge difficulty for Hive consistency
            nonce: if fork >= Hardfork::Paris { B64::ZERO } else { B64::from(0x42u64) }, // Arbitrary non-zero nonce for pre-Paris
            requests_hash: None,
            block_access_list_hash: None,
            slot_number: None,
        };

        Block {
            header,
            body: BlockBody {
                transactions,
                ommers: vec![],
                withdrawals: if fork >= Hardfork::Shanghai {
                    Some(withdrawals.map(|w| eip4895::Withdrawals::new(w.into_iter().map(|wi| eip4895::Withdrawal {
                        index: wi.index,
                        validator_index: wi.validator_index,
                        address: wi.address,
                        amount: wi.amount,
                    }).collect())).unwrap_or_else(|| eip4895::Withdrawals::new(vec![])))
                } else {
                    None
                },
            },
        }
    }
}