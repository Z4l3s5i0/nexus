use wasix_eth_types::consensus_constants::MAX_INIT_CODE_SIZE;
use wasix_eth_types::*;
use wasix_eth_storage::write::BatchWriter;
use wasix_eth_storage::read_traits::AccountProvider;
use wasix_eth_storage::write_traits::AccountWriter;
use wasix_eth_utils::{debug, exp};
use evm::backend::{OverlayedBackend, OverlayedChangeSet, InMemoryEnvironment, RuntimeBaseBackend, RuntimeEnvironment, RuntimeBackend};
use wasix_eth_types::TransactionTrait as alloy_consensus_Transaction;
use wasix_eth_types::SignerRecoverable;
use evm::uint::{H160, H256, U256 as EvmU256};
use evm::standard::{Config, EtableResolver, ExecutionEtable, GasometerEtable, Invoker, TransactArgs, TransactArgsCallCreate, TransactGasPrice, TransactValue, TransactValueCallCreate};
use evm::interpreter::etable::Chained;
use evm::transact;
use crate::backend::SputnikBackend;
use crate::state::StateApplier;
use crate::executor::TransactionExecutionResult;
use anyhow::Result;
use std::collections::HashSet;
use evm_precompile::StandardPrecompileSet;

fn logs_bloom<'a>(logs: impl Iterator<Item = &'a ConsensusReceiptLog>) -> Bloom {
    let mut bloom = Bloom::default();
    for log in logs {
        bloom.accrue_bloom(log.address.0);
        for topic in &log.data.topics() {
            bloom.accrue_bloom(topic.as_slice());
        }
    }
    bloom
}

pub struct TransactionExecutor<'a> {
    pub batch: &'a BatchWriter,
    pub env: &'a InMemoryEnvironment,
    pub config: &'a Config,
    pub invoker: &'a Invoker<'a, 'a, EtableResolver<'a, 'a, StandardPrecompileSet, Chained<GasometerEtable<'a>, ExecutionEtable<'a>>>>,
    pub fork: Hardfork,
}

impl<'a> TransactionExecutor<'a> {
    pub fn new(
        batch: &'a BatchWriter,
        env: &'a InMemoryEnvironment,
        config: &'a Config,
        invoker: &'a Invoker<'a, 'a, EtableResolver<'a, 'a, StandardPrecompileSet, Chained<GasometerEtable<'a>, ExecutionEtable<'a>>>>,
        fork: Hardfork,
    ) -> Self {
        Self { batch, env, config, invoker, fork }
    }

    pub fn execute_transaction(
        &self,
        tx: &Transaction,
        beneficiary: Address,
        cumulative_gas_used: &mut u64,
        state_root: Option<B256>,
    ) -> Result<TransactionExecutionResult> {
        let recovered = tx.clone().try_into_recovered().map_err(|e| anyhow::anyhow!("Failed to recover signer: {}", e))?;
        let sender = recovered.signer();
        
        self.validate_transaction_with_state(tx, sender, state_root)?;

        let start_time = std::time::Instant::now();

        let recovered = tx.clone().try_into_recovered().map_err(|e| anyhow::anyhow!("Failed to recover signer: {}", e))?;
        let sender = recovered.signer();
        let tx_hash_final = recovered.hash();

        exp!("[EXP] TX_EXEC_START hash={:?} sender={:?} nonce={}", tx_hash_final, sender, recovered.nonce());

        let mut backend = self.prepare_backend(tx, sender, state_root)?;
        let sender_h160 = H160::from_slice(sender.as_slice());
        let balance_before = backend.balance(sender_h160);

        debug!("[Execution] STARTING TX {:?} (sender={:?}, balance={}, fork={:?}, nonce={})", tx_hash_final, sender, balance_before, self.fork, recovered.nonce());

        if self.fork >= Hardfork::Cancun {
            if let Transaction::Eip4844(s) = tx {
                let blob_hashes: Vec<H256> = s.tx().blob_versioned_hashes().iter().flat_map(|h| h.iter()).map(|h| H256::from_slice(h.as_slice())).collect();
                backend.environment.blob_versioned_hashes = blob_hashes;
            }
        }

        let mut overlay = OverlayedBackend::new(backend, &self.config.runtime);
        
        // EIP-4844 and EIP-7702 pre-execution logic
        self.handle_pre_execution_eip_logic(tx, sender, &mut overlay)?;

        let args = self.prepare_transact_args(tx, sender, &overlay);
        
        debug!("[Execution] Transaction {:?} Args details: caller={:?}, gas_limit={}, gas_price={:?}, value={}", tx_hash_final, args.caller, args.gas_limit, args.gas_price, args.value);
        let sender_h160 = H160::from_slice(sender.as_slice());
        let balance_before_overlay = overlay.balance(sender_h160);
        debug!("[Execution] Sender balance in overlay before transact: {}", balance_before_overlay);

        let eip161 = self.fork >= Hardfork::SpuriousDragon;
        let effective_gas_price = args.gas_price.effective_gas_price(self.config, &overlay);
        let reward_rate = args.gas_price.coinbase_reward(EvmU256::from(1), self.config, &overlay);
        let is_coinbase = sender_h160 == overlay.block_coinbase();

        let (result, tx_failed) = match transact(args.clone(), None, &mut overlay, self.invoker) {
            Ok(res) => (res, false),
            Err(e) => {
                debug!("[Execution] Transaction {:?} EVM execution error: {:?}. Args: caller={:?}, value={}, gas_limit={}, gas_price={:?}", tx_hash_final, e, args.caller, args.value, args.gas_limit, args.gas_price);
                let res = self.handle_transact_error(tx, e, &args, sender_h160, balance_before, effective_gas_price, reward_rate, is_coinbase, &overlay)?;
                (res, true)
            }
        };

        let tx_reverted = !tx_failed && match &result.call_create {
            TransactValueCallCreate::Call { succeed, .. } => !matches!(succeed, evm::interpreter::ExitSucceed::Returned | evm::interpreter::ExitSucceed::Stopped),
            TransactValueCallCreate::Create { succeed, .. } => !matches!(succeed, evm::interpreter::ExitSucceed::Returned | evm::interpreter::ExitSucceed::Stopped),
        };
        let tx_success = !tx_failed && !tx_reverted;

        let (_backend_final, changeset) = overlay.deconstruct();
        let tx_gas_used = result.used_gas.as_u64();
        
        // Post-execution gas adjustments (EIP-7702, EIP-7623, etc)
        let elapsed = start_time.elapsed();
        debug!("[Execution) Transaction {:?} finished: success={}, reverted={}, gas_evm={}, cumulative={}, elapsed={:?}", tx_hash_final, tx_success, tx_reverted, tx_gas_used, *cumulative_gas_used + tx_gas_used, elapsed);

        exp!("[EXP] TX_EXEC_END hash={:?} success={} reverted={} gas_used={} cumulative_gas={} elapsed_ms={}", 
            tx_hash_final, tx_success, tx_reverted, tx_gas_used, *cumulative_gas_used + tx_gas_used, elapsed.as_millis());

        *cumulative_gas_used += tx_gas_used;

        let consensus_logs = if tx_success {
            self.process_logs(&changeset)
        } else {
            Vec::new()
        };
        H160::from_slice(beneficiary.as_slice());

        let state_applier = StateApplier::new(self.batch);
        state_applier.apply_changeset(&changeset, eip161, state_root)?;

        // Ensure beneficiary exists (EIP-158/161)
        self.batch.account(beneficiary, state_root)?;
        let intermediate_root = self.batch.calculate_state_root(eip161, state_root)?;
        let logs_bloom = logs_bloom(consensus_logs.iter());

        let status = if self.fork >= Hardfork::Byzantium {
            Eip658Value::Eip658(tx_success)
        } else {
            Eip658Value::PostState(intermediate_root)
        };

        let receipt = Receipt {
            tx_type: tx.ty(),
            receipt: ConsensusReceipt {
                status,
                cumulative_gas_used: *cumulative_gas_used,
                logs: consensus_logs,
            },
            logs_bloom,
        };

        let (output, contract_address) = match &result.call_create {
            TransactValueCallCreate::Call { retval, .. } => (Bytes::from(retval.clone()), None),
            TransactValueCallCreate::Create { address, .. } => {
                if !tx_success {
                    (Bytes::new(), None)
                } else {
                    (Bytes::from(address.as_bytes().to_vec()), Some(Address::from_slice(address.as_bytes())))
                }
            }
        };

        let receipt_meta = ReceiptMeta { contract_address };

        Ok(TransactionExecutionResult {
            output,
            gas_used: tx_gas_used,
            receipt,
            receipt_meta,
            call_create: result.call_create,
        })
    }

    fn validate_transaction_with_state(&self, tx: &Transaction, sender: Address, state_root: Option<B256>) -> Result<()> {
        let account = self.batch.account(sender, state_root)?.unwrap_or_default();
        if tx.nonce() != account.nonce {
            return Err(anyhow::anyhow!("Nonce mismatch for sender {:?}: expected {}, got {}", sender, account.nonce, tx.nonce()));
        }

        if let Some(tx_chain_id) = tx.chain_id() {
            if tx_chain_id != self.env.chain_id.low_u64() {
                return Err(anyhow::anyhow!("Invalid chain ID: expected {}, got {}", self.env.chain_id.low_u64(), tx_chain_id));
            }
        }

        if let Transaction::Eip1559(s) = tx {
            let base_fee = self.env.block_base_fee_per_gas.low_u64();
            if s.max_fee_per_gas() < base_fee as u128 {
                return Err(anyhow::anyhow!("Max fee per gas too low: max={}, base={}", s.max_fee_per_gas(), base_fee));
            }
            if s.max_fee_per_gas() < s.max_priority_fee_per_gas().unwrap_or_default() {
                return Err(anyhow::anyhow!("Max fee per gas less than max priority fee per gas"));
            }
        }

        if self.fork >= Hardfork::Cancun {
            if let Transaction::Eip4844(s) = tx {
                let blob_base_fee = self.env.blob_base_fee_per_gas.low_u64();
                if s.max_fee_per_blob_gas() < Some(blob_base_fee as u128) {
                    return Err(anyhow::anyhow!("Max fee per blob gas too low: max={:?}, base={}", s.max_fee_per_blob_gas(), blob_base_fee));
                }
            }
        }

        if let Transaction::Eip7702(_) = tx {
            if self.fork < Hardfork::Prague {
                return Err(anyhow::anyhow!("EIP-7702 transactions are not supported in this fork"));
            }
        }

        if self.fork >= Hardfork::Shanghai {
            if tx.to().is_none() && tx.input().len() > MAX_INIT_CODE_SIZE as usize {
                return Err(anyhow::anyhow!("Initcode size exceeds maximum limit ({} > {})", tx.input().len(), MAX_INIT_CODE_SIZE));
            }
        }

        Ok(())
    }


    fn prepare_backend(&self, tx: &Transaction, sender: Address, state_root: Option<B256>) -> Result<SputnikBackend<'a>> {
        let mut hot_accounts = HashSet::new();
        let mut hot_storage = HashSet::new();
        let mut access_list = Vec::new();

        if self.fork >= Hardfork::Berlin {
            // Pre-warm precompiles
            let max_precompile = if self.fork >= Hardfork::Prague { 0x13 } else if self.fork >= Hardfork::Cancun { 0x0a } else { 0x09 };
            for i in 1..=max_precompile {
                let mut addr = [0u8; 20];
                addr[19] = i as u8;
                hot_accounts.insert(H160::from_slice(&addr));
            }

            if let Some(al) = tx.access_list() {
                access_list = al.0.iter().map(|a| (H160::from_slice(a.address.as_slice()), a.storage_keys.iter().map(|k: &B256| H256::from_slice(k.as_slice())).collect::<Vec<H256>>())).collect();
            }

            hot_accounts.insert(H160::from_slice(sender.as_slice()));
            if let Some(to) = tx.to() {
                hot_accounts.insert(H160::from_slice(to.as_slice()));
            }


            for (addr, keys) in &access_list {
                hot_accounts.insert(*addr);
                for key in keys {
                    hot_storage.insert((*addr, *key));
                }
            }
        }

        Ok(SputnikBackend {
            read_provider: self.batch,
            storage_provider: self.batch,
            environment: self.env.clone(),
            state_root,
            transient_storage: std::collections::HashMap::new(),
            hot_accounts,
            hot_storage,
            origin: H160::from_slice(sender.as_slice()),
        })
    }

    fn handle_pre_execution_eip_logic(&self, tx: &Transaction, sender: Address, overlay: &mut OverlayedBackend<'a, SputnikBackend<'a>>) -> Result<()> {
        if self.fork >= Hardfork::Cancun {
            if let Transaction::Eip4844(s) = tx {

                let blob_gas_used = s.tx().blob_versioned_hashes().unwrap_or_default().len() as u64 * wasix_eth_types::DATA_GAS_PER_BLOB;
                let blob_base_fee = self.env.blob_base_fee_per_gas.as_u64();
                let blob_fee = alloy_primitives::U256::from(blob_gas_used) * alloy_primitives::U256::from(blob_base_fee);
                
                let sender_h160 = H160::from_slice(sender.as_slice());
                let balance = overlay.balance(sender_h160);
                let blob_fee_evm = alloy_u256_to_evm_u256(blob_fee);
                
                if balance < blob_fee_evm {
                    return Err(anyhow::anyhow!("Insufficient balance for blob fee"));
                }
                
                overlay.withdrawal(sender_h160, blob_fee_evm).map_err(|e| anyhow::anyhow!("Failed to deduct blob fee: {:?}", e))?;
            }
        }

        Ok(())
    }

    fn prepare_transact_args(&self, tx: &Transaction, sender: Address, _overlay: &OverlayedBackend<'a, SputnikBackend<'a>>) -> TransactArgs<'a> {
        let call_create = match tx.to() {
            Some(to) => TransactArgsCallCreate::Call { address: H160::from_slice(to.as_slice()), data: tx.input().to_vec() },
            None => TransactArgsCallCreate::Create { init_code: tx.input().to_vec(), salt: None }
        };

        let gas_price = if tx.is_eip1559() || tx.is_eip4844() {
             let base_fee = self.env.block_base_fee_per_gas;
             let max_fee = EvmU256::from(tx.max_fee_per_gas());
             let max_priority = EvmU256::from(tx.max_priority_fee_per_gas().unwrap_or_default());
             let effective_priority = if max_fee >= base_fee { std::cmp::min(max_priority, max_fee.saturating_sub(base_fee)) } else { EvmU256::zero() };
             TransactGasPrice::FeeMarket { max_priority: effective_priority, max: max_fee }
        } else {
             TransactGasPrice::Legacy(EvmU256::from(tx.gas_price().unwrap_or_default()))
        };

        TransactArgs {
            caller: H160::from_slice(sender.as_slice()),
            value: EvmU256::from_big_endian(&tx.value().to_be_bytes::<32>()),
            gas_limit: EvmU256::from(tx.gas_limit()),
            gas_price,
            access_list: tx.access_list().map(|al| al.iter().map(|a| (H160::from_slice(a.address.as_slice()), a.storage_keys.iter().map(|k: &B256| H256::from_slice(k.as_slice())).collect())).collect()).unwrap_or_default(),
            call_create,
            config: self.config,
        }
    }

    fn handle_transact_error(&self, tx: &Transaction, e: evm::interpreter::ExitError, args: &TransactArgs<'a>, sender_h160: H160, balance_before: EvmU256, effective_gas_price: EvmU256, reward_rate: EvmU256, is_coinbase: bool, overlay: &OverlayedBackend<'a, SputnikBackend<'a>>) -> Result<TransactValue> {
        use evm::interpreter::ExitError;
        let balance_after = overlay.balance(sender_h160);
        let used_gas = {
            let deduction_rate = if is_coinbase { effective_gas_price.saturating_sub(reward_rate) } else { effective_gas_price };
            let calculated = if deduction_rate > EvmU256::from(0) { 
                let spent = balance_before.saturating_sub(balance_after);
                spent / deduction_rate 
            } else { 
                EvmU256::from(0u64)
            };
            // Always charge at least intrinsic gas for failed transactions, 
            // unless it's a zero-gas-price transaction (where we still want some non-zero gas for accounting)
            let intrinsic_gas = crate::config::calculate_intrinsic_gas(tx, self.fork);
            std::cmp::max(calculated, EvmU256::from(intrinsic_gas))
        };
        let used_gas_u64 = used_gas.as_u64();
        if used_gas_u64 > args.gas_limit.as_u64() {
             debug!("[Execution] Transaction used gas ({}) exceeds limit ({})", used_gas_u64, args.gas_limit.as_u64());
        }

        match e {
            ExitError::Exception(_) | ExitError::Reverted => Ok(TransactValue {
                call_create: TransactValueCallCreate::Call { succeed: evm::interpreter::ExitSucceed::Stopped, retval: Vec::new() },
                used_gas: EvmU256::from(used_gas_u64),
            }),
            ExitError::Fatal(fatal) => Err(anyhow::anyhow!("Fatal EVM error: {:?}", fatal)),
        }
    }

    fn process_logs(&self, changeset: &OverlayedChangeSet) -> Vec<ConsensusReceiptLog> {
        changeset.logs.iter().map(|log| ConsensusReceiptLog {
            address: Address::from_slice(log.address.as_bytes()),
            data: alloy_primitives::LogData::new_unchecked(log.topics.iter().map(|t| B256::from_slice(t.as_bytes())).collect(), log.data.clone().into()),
        }).collect()
    }
}
