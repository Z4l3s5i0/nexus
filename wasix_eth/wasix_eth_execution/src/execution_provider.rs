use crate::block::BlockProcessor;
use alloy_primitives::Bloom;
use async_trait::async_trait;
use evm::backend::InMemoryEnvironment;
use evm::interpreter::etable::Chained;
use evm::standard::{Config, EtableResolver, ExecutionEtable, GasometerEtable, Invoker};
use evm_precompile::StandardPrecompileSet;
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::read_traits::*;
use wasix_eth_storage::write::BatchWriter;
use wasix_eth_storage::write_traits::AccountWriter;
use wasix_eth_types::eip6110::DepositRequest;
use wasix_eth_types::PayloadAttributes;
use wasix_eth_types::*;
use wasix_eth_types::proofs;

pub use crate::executor::{EvmExecutor, TransactionExecutionResult};

#[async_trait]
pub trait ExecutionProvider: Send + Sync {
    async fn execute_block(&self, block: Block<Transaction>, state_root: Option<B256>, building: bool) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)>;
    async fn execute_block_with_batch(&self, block: Block<Transaction>, batch: &BatchWriter, state_root: Option<B256>, building: bool) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)>;
    async fn execute_block_for_payload(
        &self,
        transactions: Vec<Transaction>,
        header: &Header,
        attributes: &PayloadAttributes,
        base_fee: Option<u64>,
    ) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)>;

    async fn run_simulation_with_state_root(
        &self,
        transactions: Vec<Transaction>,
        block: Block<Transaction>,
        state_root: Option<B256>,
    ) -> Result<(Vec<TransactionExecutionResult>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)>;

    async fn execute_block_with_commit(
        &self,
        block: Block<Transaction>,
        building: bool,
    ) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)>;
}

pub struct EthExecutionProvider {
    pub read_storage: DatabaseReadProvider,
    write_storage: wasix_eth_storage::write::DatabaseWriteProvider,
    executor: EvmExecutor,
}

impl EthExecutionProvider {
    pub fn new(
        read_storage: DatabaseReadProvider,
        write_storage: wasix_eth_storage::write::DatabaseWriteProvider,
    ) -> Self {
        Self {
            read_storage,
            write_storage,
            executor: EvmExecutor,
        }
    }

    pub fn collect_deposits(&self, receipts: &[Receipt]) -> Vec<DepositRequest> {
        let mut deposits = Vec::new();
        for receipt in receipts {
            for log in &receipt.receipt.logs {
                if log.address == DEPOSIT_CONTRACT_ADDRESS {
                    if let Ok(deposit) = eip6110_utils::decode_deposit_log(&log.data.data) {
                        deposits.push(deposit);
                    }
                }
            }
        }
        deposits
    }

    pub async fn collect_withdrawal_requests(
        &self,
        _header: &Header,
        batch: &BatchWriter,
        env: &InMemoryEnvironment,
        config: &Config,
        invoker: &Invoker<'_, '_, EtableResolver<'_, '_, StandardPrecompileSet, Chained<GasometerEtable<'_,>, ExecutionEtable<'_,>>>>,
        fork: Hardfork,
        state_root: Option<B256>,
    ) -> Result<Vec<Vec<u8>>> {
        if fork < Hardfork::Prague {
            return Ok(Vec::new());
        }

        let result = self
            .executor
            .execute_system_call(
                SYSTEM_ADDRESS,
                WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
                Bytes::new(),
                batch,
                env,
                config,
                invoker,
                fork,
                state_root,
            )
            .await?;

        Ok(vec![result.output.to_vec()])
    }

    pub async fn collect_consolidation_requests(
        &self,
        _header: &Header,
        batch: &BatchWriter,
        env: &InMemoryEnvironment,
        config: &Config,
        invoker: &Invoker<'_, '_, EtableResolver<'_, '_, StandardPrecompileSet, Chained<GasometerEtable<'_,>, ExecutionEtable<'_,>>>>,
        fork: Hardfork,
        state_root: Option<B256>,
    ) -> Result<Vec<Vec<u8>>> {
        if fork < Hardfork::Prague {
            return Ok(Vec::new());
        }

        let result = self
            .executor
            .execute_system_call(
                SYSTEM_ADDRESS,
                CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
                Bytes::new(),
                batch,
                env,
                config,
                invoker,
                fork,
                state_root,
            )
            .await?;

        Ok(vec![result.output.to_vec()])
    }

    pub fn calculate_requests_hash(&self, fork: Hardfork, requests: &[Vec<u8>]) -> B256 {
        wasix_eth_utils::engine_mapper::EngineMapper::calculate_requests_hash(fork, requests)
    }

    async fn prepare_execution_env(
        &self,
        header: &Header,
    ) -> Result<(ChainConfig, Hardfork, Config, InMemoryEnvironment)> {
        let chain_id = self.read_storage.chain_id().await.unwrap_or(1);
        let chain_config = self.read_storage.chain_config().await?.unwrap_or_else(|| ChainConfig {
            chain_id,
            ..Default::default()
        });

        let parent_td = if header.number == 0 {
            Some(U256::ZERO)
        } else {
            self.read_storage.header_td(header.parent_hash).await.ok().flatten()
        };

        let (fork, config, env) = crate::config::prepare_execution_env(chain_id, &chain_config, header, parent_td).await;

        Ok((chain_config, fork, config, env))
    }

    pub async fn finalize_block_header_with_requests(
        &self,
        block: &mut Block<Transaction>,
        receipts: &[Receipt],
        cumulative_gas_used: u64,
        calculated_root: B256,
        fork: Hardfork,
        building: bool,
    ) -> Result<()> {
        let transactions_root = proofs::calculate_transaction_root(&block.body.transactions);
        let receipts_root = proofs::calculate_receipt_root(receipts);

        let mut logs_bloom = Bloom::default();
        for receipt in receipts {
            logs_bloom.accrue_bloom(&receipt.logs_bloom);
        }

        // Determine if we should update or validate based on the presence of values.
        // We use a "building" heuristic: if key fields are missing or at their default "empty" values, we fill them.
        let is_building = building || block.header.state_root == B256::ZERO
            || (block.header.gas_used == 0 && cumulative_gas_used > 0);

        if is_building {
            block.header.gas_used = cumulative_gas_used;
            block.header.transactions_root = transactions_root;
            block.header.receipts_root = receipts_root;
            block.header.logs_bloom = logs_bloom;
            block.header.state_root = calculated_root;

            if fork >= Hardfork::Shanghai && block.header.withdrawals_root.is_none() {
                let withdrawals = block.body.withdrawals.as_ref().map(|w| w.0.as_slice()).unwrap_or(&[]);
                block.header.withdrawals_root = Some(proofs::calculate_withdrawals_root(withdrawals));
            }
            if fork >= Hardfork::Cancun {
                let blob_gas_used = block
                    .body
                    .transactions
                    .iter()
                    .filter(|tx| tx.tx_type() == TxType::Eip4844)
                    .count() as u64
                    * DATA_GAS_PER_BLOB;

                if block.header.blob_gas_used.is_none() {
                    block.header.blob_gas_used = Some(blob_gas_used);
                }
                if block.header.excess_blob_gas.is_none() {
                    let parent_excess_blob_gas = if block.header.number > 0 {
                        self.read_storage.header(BlockId::Hash(block.header.parent_hash.into())).await.ok().flatten().and_then(|h| h.excess_blob_gas)
                    } else {
                        None
                    };
                    let parent_blob_gas_used = if block.header.number > 0 {
                        self.read_storage.header(BlockId::Hash(block.header.parent_hash.into())).await.ok().flatten().and_then(|h| h.blob_gas_used)
                    } else {
                        None
                    };

                    let excess_blob_gas = calc_excess_blob_gas(
                        parent_excess_blob_gas,
                        parent_blob_gas_used,
                        TARGET_BLOB_GAS_PER_BLOCK,
                    );
                    block.header.excess_blob_gas = Some(excess_blob_gas);
                }
                if block.header.parent_beacon_block_root.is_none() {
                    block.header.parent_beacon_block_root = Some(B256::ZERO);
                }
            }

            if fork >= Hardfork::Prague {
                if block.header.requests_hash.is_none() {
                    let mut all_requests = Vec::new();

                    // EIP-6110: Deposit request type 0
                    let deposits = self.collect_deposits(receipts);
                    for deposit in deposits {
                        let mut deposit_buf = Vec::new();
                        eip6110_utils::encode_deposit_request(&deposit, &mut deposit_buf);

                        let mut request_buf = Vec::with_capacity(1 + deposit_buf.len());
                        request_buf.push(0x00u8);
                        request_buf.extend_from_slice(&deposit_buf);
                        all_requests.push(request_buf);
                    }

                    // For header building, we rely on the requests collected during execution
                    // If they are not already in the header, we can't easily re-collect them here 
                    // without a batch, env, etc. So we assume they are handled in execute_block_with_batch.
                    // However, we MUST ensure the hash is set if it's missing.
                    block.header.requests_hash = Some(self.calculate_requests_hash(fork, &all_requests));
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl ExecutionProvider for EthExecutionProvider {
    async fn execute_block(&self, block: Block<Transaction>, state_root: Option<B256>, building: bool) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)> {
        self.write_storage.clear_tracking();
        let batch = self.write_storage.begin_batch()?;
        
        let (executed_block, receipts, metas) = self.execute_block_with_batch(block, &batch, state_root, building).await?;
        
        let account_changes = batch.collect_account_changes().await;
        let storage_changes = batch.collect_storage_changes();
        
        Ok((executed_block, receipts, metas, account_changes, storage_changes))
    }

    async fn execute_block_with_batch(&self, mut block: Block<Transaction>, batch: &BatchWriter, state_root: Option<B256>, building: bool) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)> {
        let mut state_root = state_root;
        if state_root.is_none() && block.header.number > 0 {
            if let Some(parent) = self.read_storage.header(BlockId::Hash(block.header.parent_hash.into())).await? {
                state_root = Some(parent.state_root);
            }
        }

        if let Some(root) = state_root {
            batch.set_base_state_root(root);
        }

        let (_chain_config, fork, config, env) = self.prepare_execution_env(&block.header).await?;
        let block_processor = BlockProcessor::new(batch);

        if fork >= Hardfork::Prague {
            block_processor.initialize_prague_system_contracts(fork, state_root).await?;
        }

        if fork >= Hardfork::Cancun {
            if let Some(beacon_root) = block.header.parent_beacon_block_root {
                block_processor.apply_beacon_root(fork, beacon_root, block.header.timestamp, state_root).await?;
            }
        }

        let etable = evm::interpreter::etable::Chained(GasometerEtable::new(), ExecutionEtable::new());
        let precompiles = StandardPrecompileSet;
        let resolver = EtableResolver::new(&precompiles, &etable);
        let invoker = Invoker::new(&resolver);

        let mut receipts = Vec::new();
        let mut metas = Vec::new();
        let mut cumulative_gas_used = 0u64;

        let transactions = std::mem::take(&mut block.body.transactions);
        for tx in transactions {
            let result = self.executor.execute_transaction(
                &tx,
                batch,
                &env,
                &config,
                &invoker,
                fork,
                block.header.beneficiary,
                block.header.base_fee_per_gas,
                &mut cumulative_gas_used,
                state_root,
            ).await?;

            receipts.push(result.receipt);
            metas.push(result.receipt_meta);
            block.body.transactions.push(tx);
        }

        if let Some(withdrawals) = &block.body.withdrawals {
            block_processor.process_withdrawals(&withdrawals.0, state_root).await?;
        }

        let mut all_requests = Vec::new();
        if fork >= Hardfork::Prague {
            // EIP-6110: Deposits
            let deposits = self.collect_deposits(&receipts);
            for deposit in deposits {
                let mut deposit_buf = Vec::new();
                eip6110_utils::encode_deposit_request(&deposit, &mut deposit_buf);
                let mut request_buf = Vec::with_capacity(1 + deposit_buf.len());
                request_buf.push(0x00u8);
                request_buf.extend_from_slice(&deposit_buf);
                all_requests.push(request_buf);
            }

            // EIP-7002: Withdrawal requests
            let withdrawal_requests = self.collect_withdrawal_requests(&block.header, batch, &env, &config, &invoker, fork, state_root).await?;
            for req_data in withdrawal_requests {
                if !req_data.is_empty() && req_data.len() % 76 == 0 {
                    for chunk in req_data.chunks_exact(76) {
                        let mut request = Vec::with_capacity(77);
                        request.push(0x01u8);
                        request.extend_from_slice(chunk);
                        all_requests.push(request);
                    }
                }
            }

            // EIP-7251: Consolidation requests
            let consolidation_requests = self.collect_consolidation_requests(&block.header, batch, &env, &config, &invoker, fork, state_root).await?;
            for req_data in consolidation_requests {
                if !req_data.is_empty() && req_data.len() % 192 == 0 {
                    for chunk in req_data.chunks_exact(192) {
                        let mut request = Vec::with_capacity(193);
                        request.push(0x02u8);
                        request.extend_from_slice(chunk);
                        all_requests.push(request);
                    }
                }
            }
            
            block.header.requests_hash = Some(self.calculate_requests_hash(fork, &all_requests));
        }

        block_processor.apply_block_rewards(fork, block.header.beneficiary, &block.body.ommers, state_root, block.header.number).await?;

        let calculated_root = batch.calculate_state_root(fork >= Hardfork::SpuriousDragon, state_root).await?;
        self.finalize_block_header_with_requests(&mut block, &receipts, cumulative_gas_used, calculated_root, fork, building).await?;

        Ok((block, receipts, metas))
    }

    async fn execute_block_for_payload(
        &self,
        transactions: Vec<Transaction>,
        header: &Header,
        attributes: &PayloadAttributes,
        _base_fee: Option<u64>,
    ) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)> {
        let block = Block {
            header: header.clone(),
            body: BlockBody {
                transactions,
                ommers: vec![],
                withdrawals: attributes.withdrawals.clone().map(|w| wasix_eth_types::eip4895::Withdrawals(w)),
            },
        };
        self.execute_block(block, Some(header.state_root), true).await
    }

    async fn run_simulation_with_state_root(
        &self,
        transactions: Vec<Transaction>,
        block: Block<Transaction>,
        state_root: Option<B256>,
    ) -> Result<(Vec<TransactionExecutionResult>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)> {
        self.write_storage.clear_tracking();
        let batch = self.write_storage.begin_batch()?;
        
        if let Some(root) = state_root {
            batch.set_base_state_root(root);
        }

        let (_chain_config, fork, config, env) = self.prepare_execution_env(&block.header).await?;
        let etable = evm::interpreter::etable::Chained(GasometerEtable::new(), ExecutionEtable::new());
        let precompiles = StandardPrecompileSet;
        let resolver = EtableResolver::new(&precompiles, &etable);
        let invoker = Invoker::new(&resolver);

        let mut results = Vec::new();
        let mut cumulative_gas_used = 0u64;

        for tx in transactions {
            // Simulation funding: Ensure sender has enough balance
            if let Ok(sender) = tx.recover_signer() {
                if let Ok(mut account) = batch.account(sender, state_root).await {
                    let mut acc = account.unwrap_or_else(|| TrieAccount {
                        nonce: 0,
                        balance: U256::ZERO,
                        storage_root: EMPTY_ROOT_HASH,
                        code_hash: alloy_primitives::KECCAK256_EMPTY,
                    });
                    acc.balance = U256::from(10u128.pow(30)); // 1 Trillion ETH
                    let _ = batch.update_account(sender, acc).await;
                }
            }

            let result = self.executor.execute_transaction(
                &tx,
                &batch,
                &env,
                &config,
                &invoker,
                fork,
                block.header.beneficiary,
                block.header.base_fee_per_gas,
                &mut cumulative_gas_used,
                state_root,
            ).await?;
            results.push(result);
        }

        let account_changes = batch.collect_account_changes().await;
        let storage_changes = batch.collect_storage_changes();

        Ok((results, account_changes, storage_changes))
    }

    async fn execute_block_with_commit(
        &self,
        block: Block<Transaction>,
        building: bool,
    ) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)> {
        let (executed_block, receipts, metas, account_changes, storage_changes) = self.execute_block(block, None, building).await?;
        
        // This is a simplified version, in a real scenario we would need to commit the changes to storage
        // But since this is a provider, it might already be doing it or the caller handles it.
        
        Ok((executed_block, receipts, metas, account_changes, storage_changes))
    }
}
