use crate::block::BlockProcessor;
use evm::backend::InMemoryEnvironment;
pub use evm::standard::TransactValueCallCreate;
use evm::standard::{Config, EtableResolver, ExecutionEtable, GasometerEtable, Invoker};
use evm_precompile::StandardPrecompileSet;
use sha2::{Digest, Sha256};
use wasix_eth_storage::read_traits::{AccountProvider, ChainProvider, HeaderProvider};
use wasix_eth_storage::write::BatchWriter;
use wasix_eth_storage::write_traits::{AccountWriter, ChangeSetWriter, TransactionWriter, HashedStorageWriter};
use wasix_eth_types::consensus_constants::{DEPOSIT_CONTRACT_ADDRESS, MAX_BLOB_GAS_PER_BLOCK, TARGET_BLOB_GAS_PER_BLOCK};
use wasix_eth_types::*;
use wasix_eth_types::consensus_constants as constants;
use wasix_eth_utils::{debug, info};

use crate::config::prepare_execution_env;
pub use crate::executor::{EvmExecutor, TransactionExecutionResult};

pub trait ExecutionProvider: Send + Sync {
    fn execute_block(&self, block: Block<Transaction>) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)>;
    fn execute_block_with_commit(&self, block: Block<Transaction>, commit: bool) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)>;
    fn execute_block_with_state_root(&self, block: Block<Transaction>, commit: bool, state_root: Option<B256>) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)>;
    fn execute_block_with_state_root_and_building(&self, block: Block<Transaction>, commit: bool, state_root: Option<B256>, building: bool) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)>;
    fn execute_block_with_batch(&self, block: Block<Transaction>, batch: &BatchWriter, state_root: Option<B256>, building: bool) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)>;
    fn execute_block_for_payload(
        &self,
        transactions: Vec<Transaction>,
        _header: &Header,
        _attributes: &PayloadAttributes,
        _base_fee: Option<u64>,
    ) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)>;
    fn run_execution(
        &self,
        transactions: Vec<Transaction>,
        block: Block<Transaction>,
        _read_only: bool,
    ) -> Result<(Vec<TransactionExecutionResult>, Block<Transaction>)>;
    fn run_execution_with_state_root(
        &self,
        transactions: Vec<Transaction>,
        block: Block<Transaction>,
        apply_changes: bool,
        state_root: Option<B256>,
    ) -> Result<(Vec<TransactionExecutionResult>, Block<Transaction>)>;

    fn run_simulation_with_state_root(
        &self,
        transactions: Vec<Transaction>,
        block: Block<Transaction>,
        state_root: Option<B256>,
    ) -> Result<(Vec<TransactionExecutionResult>, Block<Transaction>)>;
}

pub struct EthExecutionProvider {
    pub read_storage: wasix_eth_storage::read::DatabaseReadProvider,
    write_storage: wasix_eth_storage::write::DatabaseWriteProvider,
    executor: EvmExecutor,
}

impl EthExecutionProvider {
    pub fn new(
        read_storage: wasix_eth_storage::read::DatabaseReadProvider,
        write_storage: wasix_eth_storage::write::DatabaseWriteProvider,
    ) -> Self {
        Self {
            read_storage,
            write_storage,
            executor: EvmExecutor,
        }
    }

    fn prepare_execution_env(
        &self,
        header: &Header,
    ) -> Result<(ChainConfig, Hardfork, Config, InMemoryEnvironment)> {
        let chain_id = self.read_storage.chain_id().unwrap_or(1);
        let chain_config = self.read_storage.chain_config()?.unwrap_or_else(|| ChainConfig {
            chain_id,
            ..Default::default()
        });

        let parent_td = if header.number == 0 {
            Some(U256::ZERO)
        } else {
            self.read_storage.header_td(header.parent_hash).ok().flatten()
        };

        let (fork, config, env) = prepare_execution_env(chain_id, &chain_config, header, parent_td);

        Ok((chain_config, fork, config, env))
    }




    pub fn finalize_block_header_with_requests(
        &self,
        block: &mut Block<Transaction>,
        receipts: &[Receipt],
        cumulative_gas_used: u64,
        calculated_root: B256,
        fork: Hardfork,
        building: bool,
    ) -> Result<()> {
        let consensus_receipts: Vec<ConsensusReceipt> = receipts.iter().map(|r| r.receipt.clone()).collect();
        let transactions_root = proofs::calculate_transaction_root(block.body.transactions.iter());
        let receipts_root = proofs::calculate_receipt_root(consensus_receipts.iter());

        debug!("[Execution] Calculated transactions_root: {:?}", transactions_root);
        debug!("[Execution] Calculated receipts_root: {:?}", receipts_root);
        
        let mut logs_bloom = Bloom::default();
        for receipt in receipts {
            logs_bloom.accrue_bloom(&receipt.logs_bloom);
        }

        // Determine if we should update or validate based on the presence of values.
        // We use a "building" heuristic: if key fields are missing or at their default "empty" values, we fill them.
        let is_building = building || block.header.state_root == B256::ZERO 
            || (block.header.gas_used == 0 && cumulative_gas_used > 0);

        if is_building {
            info!("[Execution] Header building mode: updating fields");
            block.header.gas_used = cumulative_gas_used;
            block.header.transactions_root = transactions_root;
            block.header.receipts_root = receipts_root;
            block.header.logs_bloom = logs_bloom;
            block.header.state_root = calculated_root;

            if fork >= Hardfork::Shanghai && block.header.withdrawals_root.is_none() {
                let withdrawals = block.body.withdrawals.as_ref().map(|w| w.as_slice()).unwrap_or(&[]);
                block.header.withdrawals_root = Some(proofs::calculate_withdrawals_root(withdrawals));
            }
            if fork >= Hardfork::Cancun {
                // Calculate actual blob gas used
                let mut blob_gas_used = 0u64;
                for tx in &block.body.transactions {
                    if let Transaction::Eip4844(s) = tx {
                        blob_gas_used += s.tx().blob_versioned_hashes().unwrap_or_default().len() as u64 * DATA_GAS_PER_BLOB;
                    }
                }

                if block.header.blob_gas_used.is_none() {
                    block.header.blob_gas_used = Some(blob_gas_used);
                }
                if block.header.excess_blob_gas.is_none() {
                    block.header.excess_blob_gas = Some(0);
                }
                if block.header.parent_beacon_block_root.is_none() {
                    block.header.parent_beacon_block_root = Some(B256::ZERO);
                }
            }

            /*
            if fork >= Hardfork::Prague {
                if block.header.requests_hash.is_none() {
                    // Collect deposits and calculate requests hash
                    let deposits = self.collect_deposits(receipts);
                    let mut requests = Vec::new();
                    
                    // EIP-6110: Deposit request type 0
                    for deposit in deposits {
                        let mut deposit_buf = Vec::new();
                        eip6110_utils::encode_deposit_request(&deposit, &mut deposit_buf);

                        let mut request_buf = Vec::with_capacity(1 + deposit_buf.len());
                        request_buf.push(0x00u8);
                        request_buf.extend_from_slice(&deposit_buf);

                        requests.push(request_buf);
                    }

                    // Add additional requests (like EIP-7002, EIP-7251)
                    for req in additional_requests {
                        requests.push(req.clone());
                    }

                    // EIP-7685: Requests hash is sha256(sha256(request_0) ++ sha256(request_1) ++ ...)
                    block.header.requests_hash = Some(self.calculate_requests_hash(fork, &requests));
                }
            }
            */
        } else {
            info!("[Execution] Header validation mode");



            /*
            if fork >= Hardfork::Prague {
                // Collect deposits and calculate requests hash
                let deposits = self.collect_deposits(receipts);
                let mut requests = Vec::new();

                // EIP-6110: Deposit request type 0
                for deposit in deposits {
                    use alloy_rlp::Encodable;
                    let mut deposit_buf = Vec::new();
                    eip6110_utils::encode_deposit_request(&deposit, &mut deposit_buf);

                    let mut request_buf = Vec::new();
                    0u8.encode(&mut request_buf);
                    request_buf.extend_from_slice(&deposit_buf);

                    requests.push(request_buf);
                }

                // Add additional requests (like EIP-7002)
                for req in additional_requests {
                    requests.push(req.clone());
                }

                // EIP-7685: Requests hash is sha256(sha256(request_type_0_data) ++ sha256(request_type_1_data) ++ ...)
                let requests_hash = self.calculate_requests_hash(fork, &requests);

                if block.header.requests_hash != Some(requests_hash) {
                    return Err(anyhow::anyhow!(
                        "Requests hash mismatch for block {}: expected {:?}, calculated {:?}",
                        block.header.number,
                        block.header.requests_hash,
                        requests_hash
                    ));
                }
            }
            */
            
            if !building && block.header.gas_used != cumulative_gas_used {
                info!("[Execution] GAS MISMATCH: expected={}, calculated={}. Diff={}",
                    block.header.gas_used, cumulative_gas_used, block.header.gas_used.saturating_sub(cumulative_gas_used));
                return Err(anyhow::anyhow!(
                    "Gas used mismatch for block {}: expected {}, calculated {}",
                    block.header.number,
                    block.header.gas_used,
                    cumulative_gas_used
                ));
            }
            if block.header.transactions_root != transactions_root {
                return Err(anyhow::anyhow!(
                    "Transactions root mismatch for block {}: expected {:?}, calculated {:?}",
                    block.header.number,
                    block.header.transactions_root,
                    transactions_root
                ));
            }
            if block.header.receipts_root != receipts_root {
                return Err(anyhow::anyhow!(
                    "Receipts root mismatch for block {}: expected {:?}, calculated {:?}",
                    block.header.number,
                    block.header.receipts_root,
                    receipts_root
                ));
            }
            if block.header.logs_bloom != logs_bloom {
                return Err(anyhow::anyhow!(
                    "Logs bloom mismatch for block {}",
                    block.header.number
                ));
            }
            if block.header.state_root != calculated_root {
                return Err(anyhow::anyhow!(
                    "State root mismatch for block {}: expected {:?}, calculated {:?}",
                    block.header.number,
                    block.header.state_root,
                    calculated_root
                ));
            }

            if fork >= Hardfork::Shanghai {
                let withdrawals = block.body.withdrawals.as_ref().map(|w| w.as_slice()).unwrap_or(&[]);
                let withdrawals_root = proofs::calculate_withdrawals_root(withdrawals);
                if block.header.withdrawals_root != Some(withdrawals_root) {
                    return Err(anyhow::anyhow!(
                        "Withdrawals root mismatch for block {}: expected {:?}, calculated {:?}",
                        block.header.number,
                        block.header.withdrawals_root,
                        withdrawals_root
                    ));
                }
            }

        }

        Ok(())
    }

    pub fn collect_deposits(&self, receipts: &[Receipt]) -> Vec<eip6110::DepositRequest> {
        let mut deposits = Vec::new();
        for receipt in receipts {
            for log in &receipt.receipt.logs {
                if log.address == DEPOSIT_CONTRACT_ADDRESS {
                    if let Some(topic0) = log.data.topics().first() {
                        if *topic0 == eip6110_utils::DEPOSIT_EVENT_SIGNATURE {
                            if let Ok(deposit) = eip6110_utils::decode_deposit_log(log.data.data()) {
                                deposits.push(deposit);
                            }
                        }
                    }
                }
            }
        }
        deposits
    }

    pub fn calculate_requests_hash(&self, fork: Hardfork, requests: &[Vec<u8>]) -> B256 {
        if requests.is_empty() {
            return constants::EMPTY_REQUESTS_HASH;
        }

        // Filter out requests with only request_type (len <= 1)
        let filtered_requests: Vec<_> = requests.iter()
            .filter(|r| r.len() > 1)
            .collect();

        if filtered_requests.is_empty() {
            return constants::EMPTY_REQUESTS_HASH;
        }

        let mut intermediate_hashes = Vec::new();

        if fork >= Hardfork::Prague {
            // Group by request_type (first byte) and maintain ascending order of types
            let mut type_to_requests: std::collections::BTreeMap<u8, Vec<&Vec<u8>>> = std::collections::BTreeMap::new();
            for &req in &filtered_requests {
                let ty = req[0];
                type_to_requests.entry(ty).or_default().push(req);
            }

            for (_ty, reqs) in type_to_requests {
                for r in reqs {
                    let mut hasher = Sha256::new();
                    hasher.update(r);
                    intermediate_hashes.push(hasher.finalize());
                }
            }
        } else {
            // No sorting/grouping for pre-Prague (though requests_hash shouldn't really exist)
            for &r in &filtered_requests {
                let mut hasher = Sha256::new();
                hasher.update(r);
                intermediate_hashes.push(hasher.finalize());
            }
        }

        let mut final_hasher = Sha256::new();
        for h in intermediate_hashes {
            final_hasher.update(&h);
        }
        B256::from_slice(&final_hasher.finalize())
    }

    pub fn collect_withdrawal_requests(&self, header: &Header) -> Result<Vec<Vec<u8>>> {
        let (_chain_config, fork, config, env) = self.prepare_execution_env(header)?;
        if fork < Hardfork::Prague {
            return Ok(Vec::new());
        }

        let state_root = if header.number > 0 {
            self.read_storage.header(BlockId::Hash(header.parent_hash.into()))?
                .map(|h| h.state_root)
        } else {
            None
        };

        let batch = self.write_storage.begin_batch()?;
        
        let etable = evm::interpreter::etable::Chained(GasometerEtable::new(), ExecutionEtable::new());
        let precompiles = StandardPrecompileSet;
        let resolver = EtableResolver::new(&precompiles, &etable);
        let invoker = Invoker::new(&resolver);

        let sc7002 = self.executor.execute_system_call(
            SYSTEM_ADDRESS,
            WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
            Bytes::new(),
            &batch,
            &env,
            &config,
            &invoker,
            fork,
            state_root,
        )?;

        let mut withdrawal_requests = Vec::new();
        let requests_data = sc7002.output;
        if !requests_data.is_empty() {
            if requests_data.len() % 76 != 0 {
                return Err(anyhow::anyhow!("EIP-7002 system call returned malformed data length: {}", requests_data.len()));
            }

            for chunk in requests_data.chunks_exact(76) {
                let mut request = Vec::with_capacity(77);
                request.push(constants::WITHDRAWAL_REQUEST_TYPE);
                request.extend_from_slice(chunk);
                withdrawal_requests.push(request);
            }
        }
        Ok(withdrawal_requests)
    }

    pub fn collect_consolidation_requests(&self, header: &Header) -> Result<Vec<Vec<u8>>> {
        let (_chain_config, fork, config, env) = self.prepare_execution_env(header)?;
        if fork < Hardfork::Prague {
            return Ok(Vec::new());
        }

        let state_root = if header.number > 0 {
            self.read_storage.header(BlockId::Hash(header.parent_hash.into()))?
                .map(|h| h.state_root)
        } else {
            None
        };

        let batch = self.write_storage.begin_batch()?;
        
        let etable = evm::interpreter::etable::Chained(GasometerEtable::new(), ExecutionEtable::new());
        let precompiles = StandardPrecompileSet;
        let resolver = EtableResolver::new(&precompiles, &etable);
        let invoker = Invoker::new(&resolver);

        let sc7251 = self.executor.execute_system_call(
            SYSTEM_ADDRESS,
            CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
            Bytes::new(),
            &batch,
            &env,
            &config,
            &invoker,
            fork,
            state_root,
        )?;

        let mut consolidation_requests = Vec::new();
        let requests_data = sc7251.output;
        if !requests_data.is_empty() {
            // EIP-7251: Each consolidation request is 88 bytes.
            if requests_data.len() % 88 != 0 {
                return Err(anyhow::anyhow!("EIP-7251 system call returned malformed data length: {}", requests_data.len()));
            }

            for chunk in requests_data.chunks_exact(88) {
                let mut request = Vec::with_capacity(89);
                request.push(constants::CONSOLIDATION_REQUEST_TYPE);
                request.extend_from_slice(chunk);
                consolidation_requests.push(request);
            }
        }
        Ok(consolidation_requests)
    }

    fn ocean_execution_core_logic(
        &self,
        transactions: Vec<Transaction>,
        mut block: Block<Transaction>,
        apply_changes: bool,
        state_root: Option<B256>,
        is_simulation: bool,
    ) -> Result<(Vec<TransactionExecutionResult>, Block<Transaction>)> {
        let batch = self.write_storage.begin_batch()?;

        let mut results = Vec::new();
        let mut receipts = Vec::new();
        let mut cumulative_gas_used = 0u64;

        let (_chain_config, fork, config, env) = self.prepare_execution_env(&block.header)?;
        let is_eip161 = fork >= Hardfork::SpuriousDragon;

        let etable = evm::interpreter::etable::Chained(GasometerEtable::new(), ExecutionEtable::new());
        let precompiles = StandardPrecompileSet;
        let resolver = EtableResolver::new(&precompiles, &etable);
        let invoker = Invoker::new(&resolver);

        let block_processor = BlockProcessor::new(&batch);

        for tx in transactions {
            if is_simulation {
                if let Some(from) = tx.recover_signer() {
                    let mut acc = batch.account(from, state_root)?.unwrap_or_default();
                    acc.balance = alloy_primitives::U256::from(10u128.pow(30));
                    batch.update_account(from, acc)?;
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
            )?;

            receipts.push(result.receipt.clone());
            results.push(result);
            block.body.transactions.push(tx);
        }

        if apply_changes {
            if let Some(withdrawals) = &block.body.withdrawals {
                block_processor.process_withdrawals(withdrawals, state_root)?;
            }

            block_processor.apply_block_rewards(fork, block.header.beneficiary, &block.body.ommers, state_root, block.header.number)?;
            
            let calculated_root = batch.calculate_state_root(is_eip161, state_root)?;
            self.finalize_block_header_with_requests(&mut block, &receipts, cumulative_gas_used, calculated_root, fork, is_simulation)?;
            drop(block_processor);
            batch.commit()?;
        } else {
            if let Some(withdrawals) = &block.body.withdrawals {
                block_processor.process_withdrawals(withdrawals, state_root)?;
            }

            block_processor.apply_block_rewards(fork, block.header.beneficiary, &block.body.ommers, state_root, block.header.number)?;

            let calculated_root = batch.calculate_state_root(is_eip161, state_root)?;
            self.finalize_block_header_with_requests(&mut block, &receipts, cumulative_gas_used, calculated_root, fork, is_simulation)?;
        }

        Ok((results, block))
    }
}

impl ExecutionProvider for EthExecutionProvider {
    fn execute_block(&self, block: Block<Transaction>) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)> {
        self.execute_block_with_commit(block, true)
    }

    fn execute_block_with_commit(&self, block: Block<Transaction>, commit: bool) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)> {
        self.execute_block_with_state_root(block, commit, None)
    }

    fn execute_block_with_state_root(&self, block: Block<Transaction>, commit: bool, state_root: Option<B256>) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)> {
        self.execute_block_with_state_root_and_building(block, commit, state_root, false)
    }

    fn execute_block_with_state_root_and_building(&self, block: Block<Transaction>, commit: bool, state_root: Option<B256>, building: bool) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)> {
        debug!("[Execution] execute_block_with_state_root_and_building: block {} commit={} state_root={:?} building={}", block.header.number, commit, state_root, building);
        self.write_storage.clear_tracking();
        let batch = self.write_storage.begin_batch()?;
        
        let (executed_block, receipts, metas) = match self.execute_block_with_batch(block.clone(), &batch, state_root, building) {
            Ok(res) => res,
            Err(e) => {
                // Batch will be dropped here, and redb::WriteTransaction will be aborted
                return Err(e);
            }
        };
        
        if commit {
            debug!("[Execution] execute_block_with_state_root_and_building: committing batch for block {}", block.header.number);

            // Persist ChangeSets
            let account_changes = batch.collect_account_changes();
            let storage_changes = batch.collect_storage_changes();
            batch.insert_account_change_set(executed_block.header.number, account_changes.iter().map(|(addr, acc)| (*addr, acc.clone())).collect())?;
            batch.insert_storage_change_set(executed_block.header.number, storage_changes.iter().map(|(key, val)| (*key, *val)).collect())?;

            for (i, meta) in metas.iter().enumerate() {
                batch.insert_receipt_meta(executed_block.header.hash_slow(), i as u64, meta.clone())?;
            }

            batch.commit()?;
        }
        
        Ok((executed_block, receipts, metas))
    }

    fn execute_block_with_batch(&self, mut block: Block<Transaction>, batch: &BatchWriter, state_root: Option<B256>, building: bool) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)> {
        let mut state_root = state_root;
        if state_root.is_none() && block.header.number > 0 {
            // Look up the parent header to get the correct state root for the reorged path
            if let Some(parent) = self.read_storage.header(BlockId::Hash(block.header.parent_hash.into()))? {
                state_root = Some(parent.state_root);
            } else {
                return Err(anyhow::anyhow!("Parent state root not found for block {} (parent: {})", block.header.number, block.header.parent_hash));
            }
        }

        if let Some(root) = state_root {
            batch.set_base_state_root(root);
        }

        let mut receipts = Vec::new();
        let mut metas = Vec::new();
        let mut cumulative_gas_used = 0u64;

        let (chain_config, fork, config, env) = self.prepare_execution_env(&block.header)?;
        
        // let is_eip161 = fork >= Hardfork::SpuriousDragon;

        let block_processor = BlockProcessor::new(batch);

        // EIP-4788: Beacon block root in the Ethereum state
        if fork >= Hardfork::Cancun {
            if let Some(beacon_root) = block.header.parent_beacon_block_root {
                block_processor.apply_beacon_root(fork, beacon_root, block.header.timestamp, state_root)?;
                debug!("[Execution] EIP-4788 system call finished");
            }
        }


        let etable = evm::interpreter::etable::Chained(GasometerEtable::new(), ExecutionEtable::new());
        let precompiles = StandardPrecompileSet;
        let resolver = EtableResolver::new(&precompiles, &etable);
        let invoker = Invoker::new(&resolver);

        let mut blob_gas_used = 0u64;
        let mut final_transactions = Vec::new();

        for (tx_idx, tx) in block.body.transactions.iter().enumerate() {
            let tx_blob_gas = if fork >= Hardfork::Cancun {
                if let Transaction::Eip4844(s) = tx {
                    s.tx().blob_versioned_hashes().unwrap_or_default().len() as u64 * DATA_GAS_PER_BLOB
                } else {
                    0
                }
            } else {
                0
            };

            let result = match self.executor.execute_transaction(
                tx,
                batch,
                &env,
                &config,
                &invoker,
                fork,
                block.header.beneficiary,
                block.header.base_fee_per_gas,
                &mut cumulative_gas_used,
                state_root,
            ) {
                Ok(res) => res,
                Err(e) => {
                    if building {
                        debug!("[Execution] Skipping transaction {} during block building due to error: {:?}", tx_idx, e);
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            };
            
            debug!("[Execution] Transaction {} finished: hash={:?}, gas_used={}, cumulative={}", tx_idx, tx.hash(), result.gas_used, cumulative_gas_used);
            receipts.push(result.receipt);
            metas.push(result.receipt_meta);
            blob_gas_used += tx_blob_gas;
            final_transactions.push(tx.clone());
        }

        block.body.transactions = final_transactions;

        if fork >= Hardfork::Cancun {
            let max = if let Some(params) = fork.blob_params(&chain_config) {
                params.max_blob_count * DATA_GAS_PER_BLOB
            } else {
                MAX_BLOB_GAS_PER_BLOCK
            };
            if blob_gas_used > max {
                 return Err(anyhow::anyhow!("Block blob gas used {} exceeds limit {}", blob_gas_used, max));
            }
            if let Some(header_blob_gas_used) = block.header.blob_gas_used {
                if header_blob_gas_used != blob_gas_used {
                    return Err(anyhow::anyhow!("Block blob gas used mismatch: header={}, calculated={}", header_blob_gas_used, blob_gas_used));
                }
            }
        }

        // Note: do not log gas_used_total yet; post-block system calls may contribute


        if let Some(withdrawals) = &block.body.withdrawals {
            if !withdrawals.is_empty() {
                block_processor.process_withdrawals(withdrawals, state_root)?;
            }
        }

        block_processor.apply_block_rewards(fork, block.header.beneficiary, &block.body.ommers, state_root, block.header.number)?;


        let calculated_root = batch.calculate_state_root(fork >= Hardfork::SpuriousDragon, state_root)?;
        debug!("[Execution] State root calculated: {:?}", calculated_root);

        self.finalize_block_header_with_requests(&mut block, &receipts, cumulative_gas_used, calculated_root, fork, building)?;
        info!("[Execution] Block header finalized for block {}", block.header.number);
        Ok((block, receipts, metas))
    }

    fn execute_block_for_payload(
        &self,
        transactions: Vec<Transaction>,
        parent_header: &Header,
        attributes: &PayloadAttributes,
        base_fee: Option<u64>,
    ) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)> {
        let chain_id = self.read_storage.chain_id().unwrap_or(1);
        let chain_config = self.read_storage.chain_config()?.unwrap_or_else(|| ChainConfig {
            chain_id,
            ..Default::default()
        });

        let number = parent_header.number + 1;
        let timestamp = attributes.timestamp;
        let fork = Hardfork::get_active_fork(&chain_config, number, timestamp);
        let _is_eip161 = fork >= Hardfork::SpuriousDragon;

        let difficulty = if fork >= Hardfork::Paris {
            U256::ZERO
        } else {
            // Simplified difficulty calculation for pre-Paris
            // In a real network this would use the parent's difficulty and timestamp
            parent_header.difficulty
        };

        let blob_gas_used = if fork >= Hardfork::Cancun {
            let mut total_blob_gas = 0u64;
            for tx in &transactions {
                if let Transaction::Eip4844(tx_4844) = tx {
                    total_blob_gas += tx_4844.tx().blob_versioned_hashes().map(|h| h.len()).unwrap_or(0) as u64 * DATA_GAS_PER_BLOB;
                }
            }
            Some(total_blob_gas)
        } else {
            None
        };

        let excess_blob_gas = if fork >= Hardfork::Cancun {
            let params = fork.blob_params(&chain_config);
            let target = params.map(|p| p.target_blob_count * DATA_GAS_PER_BLOB).unwrap_or(TARGET_BLOB_GAS_PER_BLOCK);
            Some(calc_excess_blob_gas(parent_header.excess_blob_gas, parent_header.blob_gas_used, target))
        } else {
            None
        };

        let mut header = Header {
            parent_hash: parent_header.hash_slow(),
            beneficiary: attributes.suggested_fee_recipient,
            state_root: B256::ZERO, // Will be set after execution
            transactions_root: B256::ZERO, // Will be set after execution
            receipts_root: B256::ZERO, // Will be set after execution
            logs_bloom: Bloom::default(),
            difficulty,
            number,
            gas_limit: parent_header.gas_limit,
            gas_used: 0,
            timestamp,
            extra_data: Bytes::new(),
            mix_hash: if fork >= Hardfork::Paris { attributes.prev_randao } else { parent_header.mix_hash },
            nonce: if fork >= Hardfork::Paris { B64::ZERO } else { B64::from(0x42u64) },
            base_fee_per_gas: base_fee,
            withdrawals_root: if fork >= Hardfork::Shanghai { Some(B256::ZERO) } else { None },
            blob_gas_used,
            excess_blob_gas,
            parent_beacon_block_root: if fork >= Hardfork::Cancun { attributes.parent_beacon_block_root } else { None },
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            ..Default::default()
        };

        let body = BlockBody {
            transactions,
            ommers: Vec::new(),
            withdrawals: if fork >= Hardfork::Shanghai {
                Some(eip4895::Withdrawals::new(attributes.withdrawals.clone().unwrap_or_default().into_iter().map(|w| eip4895::Withdrawal {
                    index: w.index,
                    validator_index: w.validator_index,
                    address: w.address,
                    amount: w.amount,
                }).collect()))
            } else {
                None
            },
        };

        if let Some(withdrawals_root) = &mut header.withdrawals_root {
             if let Some(withdrawals) = &body.withdrawals {
                 *withdrawals_root = proofs::calculate_withdrawals_root(withdrawals);
             }
        }

        let block = Block {
            header,
            body,
        };

        // Reuse execute_block logic but don't commit
        self.execute_block_with_state_root_and_building(block, false, Some(parent_header.state_root), true)
    }

    fn run_execution(
        &self,
        transactions: Vec<Transaction>,
        block: Block<Transaction>,
        apply_changes: bool,
    ) -> Result<(Vec<TransactionExecutionResult>, Block<Transaction>)> {
        self.run_execution_with_state_root(transactions, block, apply_changes, None)
    }

    fn run_execution_with_state_root(
        &self,
        transactions: Vec<Transaction>,
        block: Block<Transaction>,
        apply_changes: bool,
        state_root: Option<B256>,
    ) -> Result<(Vec<TransactionExecutionResult>, Block<Transaction>)> {
        self.ocean_execution_core_logic(transactions, block, apply_changes, state_root, false)
    }

    fn run_simulation_with_state_root(
        &self,
        transactions: Vec<Transaction>,
        block: Block<Transaction>,
        state_root: Option<B256>,
    ) -> Result<(Vec<TransactionExecutionResult>, Block<Transaction>)> {
        self.ocean_execution_core_logic(transactions, block, false, state_root, true)
    }
}
