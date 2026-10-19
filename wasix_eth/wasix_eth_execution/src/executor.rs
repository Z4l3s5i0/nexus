use evm::backend::{InMemoryEnvironment, OverlayedBackend, OverlayedChangeSet, RuntimeBaseBackend};
use evm::interpreter::etable::Chained;
use evm::interpreter::runtime::RuntimeBackend;
use evm::standard::{Config, EtableResolver, ExecutionEtable, GasometerEtable, Invoker, TransactArgs, TransactArgsCallCreate, TransactGasPrice, TransactValueCallCreate};
use evm::uint::{H160, U256 as EvmU256};
use wasix_eth_storage::read_traits::{AccountProvider, BytecodeProvider};
use wasix_eth_storage::write::BatchWriter;
use wasix_eth_types::*;
use wasix_eth_utils::info;

use crate::backend::SputnikBackend;
use crate::state::StateApplier;
use crate::transaction::TransactionExecutor;
use evm_precompile::StandardPrecompileSet;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct SystemCallResult {
    pub output: Bytes,
    pub used_gas: u64,
}

#[derive(Debug)]
pub struct TxDebugTrace {
    pub tx_hash: B256,
    pub gas_used: u64,
    pub cumulative_gas: u64,

    pub created_accounts: Vec<Address>,
    pub deleted_accounts: Vec<Address>,
    pub touched_accounts: Vec<Address>,

    pub storage_writes: Vec<(Address, B256, U256)>,

    pub logs: usize,

    pub output_len: usize,
}

#[derive(Debug, Clone)]
pub struct TransactionExecutionResult {
    pub output: Bytes,
    pub gas_used: u64,
    pub receipt: Receipt,
    pub receipt_meta: ReceiptMeta,
    pub call_create: TransactValueCallCreate,
}

pub struct EvmExecutor;

impl EvmExecutor {

    /// SputnikVM's `transact` returns `used_gas` which represents the total transaction gas used.
    /// This includes both the intrinsic gas (base cost + calldata + optional contract creation surcharge)
    /// and the execution gas (gas consumed by EVM opcodes), after applying gas refunds.
    /// This value should directly correspond to the `gasUsed` field in an Ethereum transaction receipt.
    pub async fn execute_transaction(
        &self,
        tx: &Transaction,
        batch: &BatchWriter,
        env: &InMemoryEnvironment,
        config: &Config,
        invoker: &Invoker<'_, '_, EtableResolver<'_, '_, StandardPrecompileSet, Chained<GasometerEtable<'_>, ExecutionEtable<'_>>>>,
        fork: Hardfork,
        beneficiary: Address,
        _base_fee: Option<u64>,
        cumulative_gas_used: &mut u64,
        state_root: Option<B256>,
    ) -> Result<TransactionExecutionResult> {
        let executor = TransactionExecutor::new(batch, env, config, invoker, fork);
        executor.execute_transaction(tx, beneficiary, cumulative_gas_used, state_root).await
    }

    pub async fn execute_system_call(
        &self,
        caller: Address,
        to: Address,
        data: Bytes,
        batch: &BatchWriter,
        env: &InMemoryEnvironment,
        config: &Config,
        invoker: &Invoker<'_, '_, EtableResolver<'_, '_, StandardPrecompileSet, Chained<GasometerEtable<'_>, ExecutionEtable<'_>>>>,
        fork: Hardfork,
        state_root: Option<B256>,
    ) -> Result<SystemCallResult> {
        // EIP-7002: If there is no code at WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, the corresponding block MUST be marked invalid.
        let to_h160 = H160::from_slice(to.as_slice());
        let account = batch.account(to, state_root).await?;
        let code_hash = account.as_ref().map(|a| a.code_hash).unwrap_or(alloy_primitives::KECCAK256_EMPTY);
        let code = batch.bytecode(code_hash).await?;

        if code.is_none() || code.as_ref().unwrap().is_empty() {
            return Err(anyhow::anyhow!("System call target address {:?} has no code", to));
        }

        let mut hot_accounts = HashSet::new();
        let hot_storage = HashSet::new();

        if fork >= Hardfork::Berlin {
            // Pre-warm precompiles
            let max_precompile = if fork >= Hardfork::Prague {
                0x13
            } else if fork >= Hardfork::Cancun {
                0x0a
            } else {
                0x09
            };
            for i in 1..=max_precompile {
                let mut addr = [0u8; 20];
                addr[19] = i as u8;
                hot_accounts.insert(H160::from_slice(&addr));
            }
            hot_accounts.insert(H160::from_slice(caller.as_slice()));
            hot_accounts.insert(to_h160);
        }

        let backend = SputnikBackend {
            read_provider: batch,
            storage_provider: batch,
            bytecode_provider: batch,
            environment: env.clone(),
            state_root,
            transient_storage: HashMap::new(),
            hot_accounts,
            hot_storage,
            origin: H160::from_slice(caller.as_slice()),
        };

        // EIP-7002/2935: System calls should be gasless and exempt from balance checks.
        // We use gas_price = 0, and since it's a system call, we want to bypass any funding requirements.
        // The current evm::transact still expects the caller to have enough balance for (gas_limit * gas_price) + value.
        // Since gas_price is 0 and value is 0, we don't strictly need to fund it, but we'll keep the caller pre-warmed.
        let mut overlay = OverlayedBackend::new(backend, &config.runtime);
        overlay.mark_hot(H160::from_slice(caller.as_slice()), ::evm::interpreter::runtime::TouchKind::Access);

        // Trick: set caller balance to something high to pass initial balance checks
        // We will filter this out of the changeset later if needed, but since it's an overlay
        // and we are using gas_price=0, it shouldn't actually affect the permanent state
        // unless the system call itself modifies the caller's balance.
        let caller_h160 = H160::from_slice(caller.as_slice());
        let original_caller_acc = overlay.balance(caller_h160);
        overlay.deposit(caller_h160, EvmU256::from(10u128.pow(30)));

        // We use transact to bypass EOA checks and nonce increments
        // This simulates a system call as defined in EIP-7002
        let args = TransactArgs {
            caller: H160::from_slice(caller.as_slice()),
            gas_limit: EvmU256::from(30_000_000u64),
            gas_price: TransactGasPrice::Legacy(EvmU256::zero()),
            access_list: Vec::new(),
            value: EvmU256::zero(),
            call_create: TransactArgsCallCreate::Call {
                address: to_h160,
                data: data.to_vec(),
            },
            config,
        };

        let result = evm::transact(args, None, &mut overlay, invoker)
            .map_err(|e| anyhow::anyhow!("System call execution failed: {:?}", e))?;

        info!("[Execution] System call to {:?} used {} gas", to_h160, result.used_gas);

        let retval = match result.call_create {
            evm::standard::TransactValueCallCreate::Call { retval, .. } => retval,
            _ => return Err(anyhow::anyhow!("System call returned unexpected result type")),
        };

        let (_backend_final, changeset) = overlay.deconstruct();
        info!("[Execution] System call to {:?} finished, changeset: storages={}, accounts={}", to_h160, changeset.storages.len(), changeset.balances.len());

        // Restore caller balance in changeset to avoid permanent state change
        let mut filtered_changeset = changeset;
        filtered_changeset.balances.insert(caller_h160, original_caller_acc);

        let eip161 = fork >= Hardfork::SpuriousDragon;
        let _coinbase = H160::from_slice(env.block_coinbase.as_bytes());

        self.apply_changeset(batch, &filtered_changeset, eip161, state_root).await?;

        Ok(SystemCallResult { output: Bytes::from(retval), used_gas: result.used_gas.as_u64() })
    }

    pub async fn apply_changeset(&self, batch: &BatchWriter, changeset: &OverlayedChangeSet, eip161: bool, state_root: Option<B256>) -> Result<()> {
        let applier = StateApplier::new(batch);
        applier.apply_changeset(changeset, eip161, state_root).await
    }
}
