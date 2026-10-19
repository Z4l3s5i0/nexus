use wasix_eth_types::*;
use wasix_eth_storage::read::DatabaseReadProvider;
use wasix_eth_storage::write::DatabaseWriteProvider;
use anyhow::Result;
use tokio::sync::{mpsc, oneshot};
use wasix_eth_storage::write::BatchWriter;
use crate::execution_provider::{ExecutionProvider, EthExecutionProvider};
use crate::executor::TransactionExecutionResult;
use async_trait::async_trait;

pub enum ExecutionRequest {
    ExecuteBlock {
        block: Block<Transaction>,
        state_root: Option<B256>,
        building: bool,
        resp: oneshot::Sender<Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)>>,
    },
    ExecuteBlockForPayload {
        transactions: Vec<Transaction>,
        header: Header,
        attributes: PayloadAttributes,
        base_fee: Option<u64>,
        resp: oneshot::Sender<Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)>>,
    },
    RunSimulationWithStateRoot {
        transactions: Vec<Transaction>,
        block: Block<Transaction>,
        state_root: Option<B256>,
        resp: oneshot::Sender<Result<(Vec<TransactionExecutionResult>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)>>,
    },
    ExecuteBlockWithCommit {
        block: Block<Transaction>,
        building: bool,
        resp: oneshot::Sender<Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)>>,
    },
}

#[derive(Clone)]
pub struct ExecutionOrchestrator {
    tx: mpsc::Sender<ExecutionRequest>,
}

impl ExecutionOrchestrator {
    pub fn new(
        read_storage: DatabaseReadProvider,
        write_storage: DatabaseWriteProvider,
    ) -> Self {
        let (tx, mut rx) = mpsc::channel(32);
        
        let eth_exec = EthExecutionProvider::new(read_storage, write_storage);
        
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
                
            rt.block_on(async move {
                while let Some(req) = rx.recv().await {
                    match req {
                        ExecutionRequest::ExecuteBlock { block, state_root, building, resp } => {
                            let result = eth_exec.execute_block(block, state_root, building).await;
                            let _ = resp.send(result);
                        }
                        ExecutionRequest::ExecuteBlockForPayload { transactions, header, attributes, base_fee, resp } => {
                            let result = eth_exec.execute_block_for_payload(transactions, &header, &attributes, base_fee).await;
                            let _ = resp.send(result);
                        }
                        ExecutionRequest::RunSimulationWithStateRoot { transactions, block, state_root, resp } => {
                            let result = eth_exec.run_simulation_with_state_root(transactions, block, state_root).await;
                            let _ = resp.send(result);
                        }
                        ExecutionRequest::ExecuteBlockWithCommit { block, building, resp } => {
                            let result = eth_exec.execute_block_with_commit(block, building).await;
                            let _ = resp.send(result);
                        }
                    }
                }
            });
        });

        Self { tx }
    }
}

#[async_trait]
impl ExecutionProvider for ExecutionOrchestrator {
    async fn execute_block(&self, block: Block<Transaction>, state_root: Option<B256>, building: bool) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(ExecutionRequest::ExecuteBlock { block, state_root, building, resp: tx }).await?;
        rx.await?
    }

    async fn execute_block_with_batch(&self, _block: Block<Transaction>, _batch: &BatchWriter, _state_root: Option<B256>, _building: bool) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>)> {
        Err(anyhow::anyhow!("execute_block_with_batch not supported by orchestrator"))
    }

    async fn execute_block_for_payload(
        &self,
        transactions: Vec<Transaction>,
        header: &Header,
        attributes: &PayloadAttributes,
        base_fee: Option<u64>,
    ) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(ExecutionRequest::ExecuteBlockForPayload { 
            transactions, 
            header: header.clone(), 
            attributes: attributes.clone(), 
            base_fee, 
            resp: tx 
        }).await?;
        rx.await?
    }

    async fn run_simulation_with_state_root(
        &self,
        transactions: Vec<Transaction>,
        block: Block<Transaction>,
        state_root: Option<B256>,
    ) -> Result<(Vec<TransactionExecutionResult>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(ExecutionRequest::RunSimulationWithStateRoot { transactions, block, state_root, resp: tx }).await?;
        rx.await?
    }

    async fn execute_block_with_commit(
        &self,
        block: Block<Transaction>,
        building: bool,
    ) -> Result<(Block<Transaction>, Vec<Receipt>, Vec<ReceiptMeta>, Vec<(Address, Option<Bytes>)>, Vec<(Address, B256, U256)>)> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(ExecutionRequest::ExecuteBlockWithCommit { block, building, resp: tx }).await?;
        rx.await?
    }
}
