use std::sync::Arc;
use tokio::sync::mpsc;
use redb::Database;
use wasix_eth_types::Result;
use wasix_eth_utils::error;

use crate::write::request::WriteRequest;
use crate::write::provider::DatabaseWriteProvider;
use crate::write::batch::{BatchWriter, BatchTransaction};
use crate::read::DatabaseReadProvider;
use crate::codecs::Table;
use crate::tables::{ActivePeers};
use crate::write_traits::{HeaderWriter, BlockWriter, TransactionWriter, AccountWriter, StorageWriter, BytecodeWriter, StateWriter, ChangeSetWriter, MetadataWriter};

pub struct WriteCoordinator {
    db: Arc<Database>,
    rx: mpsc::UnboundedReceiver<WriteRequest>,
    provider: DatabaseWriteProvider,
    batch_timeout: std::time::Duration,
    max_batch_size: usize,
}

impl WriteCoordinator {
    pub fn new(db: Arc<Database>, rx: mpsc::UnboundedReceiver<WriteRequest>, provider: DatabaseWriteProvider) -> Self {
        Self {
            db,
            rx,
            provider,
            batch_timeout: std::time::Duration::from_millis(50),
            max_batch_size: 100,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        loop {
            let mut requests = Vec::new();

            // Wait for the first request
            match self.rx.recv().await {
                Some(req) => requests.push(req),
                None => return Ok(()), // Channel closed
            }

            // Collect more requests until timeout or max batch size
            let start = std::time::Instant::now();
            while requests.len() < self.max_batch_size && start.elapsed() < self.batch_timeout {
                tokio::select! {
                    Some(req) = self.rx.recv() => {
                        requests.push(req);
                    }
                    _ = tokio::time::sleep(self.batch_timeout - start.elapsed()) => {
                        break;
                    }
                }
            }

            // Process the batch
            if let Err(e) = self.process_batch(requests).await {
                error!("[WriteCoordinator] Failed to process batch: {}", e);
            }
        }
    }

    async fn process_batch(&self, requests: Vec<WriteRequest>) -> Result<()> {
        let batch_writer = self.provider.begin_write_batch()?;
        let mut responders = Vec::with_capacity(requests.len());

        for req in requests {
            match req {
                WriteRequest::InsertHeader { hash, header, resp } => {
                    let res = batch_writer.insert_header(hash, header).await;
                    responders.push((resp, res));
                }
                WriteRequest::InsertHeaderTD { hash, td, resp } => {
                    let res = batch_writer.insert_header_td(hash, td).await;
                    responders.push((resp, res));
                }
                WriteRequest::UpdateForkchoice { head, safe, finalized, resp } => {
                    let res = batch_writer.update_forkchoice(head, safe, finalized).await;
                    responders.push((resp, res));
                }
                WriteRequest::InsertHeaderNumber { hash, number, resp } => {
                    let res = batch_writer.insert_header_number(hash, number).await;
                    responders.push((resp, res));
                }
                WriteRequest::InsertBlock { block, receipts, metas, resp } => {
                    let res = batch_writer.insert_block(block, receipts, metas).await;
                    responders.push((resp, res));
                }
                WriteRequest::InsertBlockBody { hash, number, body, resp } => {
                    let res = batch_writer.insert_block_body(hash, number, body).await;
                    responders.push((resp, res));
                }
                WriteRequest::SetCanonical { number, hash, resp } => {
                    let res = batch_writer.set_canonical(number, hash).await;
                    responders.push((resp, res));
                }
                WriteRequest::RemoveCanonical { number, resp } => {
                    let res = batch_writer.remove_canonical(number).await;
                    responders.push((resp, res));
                }
                WriteRequest::InsertTransaction { hash, tx, resp } => {
                    let res = batch_writer.insert_transaction(hash, tx).await;
                    responders.push((resp, res));
                }
                WriteRequest::InsertReceipt { block_hash, index, receipt, resp } => {
                    let res = batch_writer.insert_receipt(block_hash, index, receipt).await;
                    responders.push((resp, res));
                }
                WriteRequest::InsertReceiptMeta { block_hash, index, meta, resp } => {
                    let res = batch_writer.insert_receipt_meta(block_hash, index, meta).await;
                    responders.push((resp, res));
                }
                WriteRequest::RemoveReceiptMeta { block_hash, index, resp } => {
                    let res = batch_writer.remove_receipt_meta(block_hash, index).await;
                    responders.push((resp, res));
                }
                WriteRequest::InsertTransactionLookup { hash, block_hash, index, resp } => {
                    let res = batch_writer.insert_transaction_lookup(hash, block_hash, index).await;
                    responders.push((resp, res));
                }
                WriteRequest::UpdateAccount { address, account, resp } => {
                    let res = batch_writer.update_account(address, account).await;
                    responders.push((resp, res));
                }
                WriteRequest::RemoveAccount { address, resp } => {
                    let res = batch_writer.remove_account(address).await;
                    responders.push((resp, res));
                }
                WriteRequest::UpdateStorage { address, slot, value, resp } => {
                    let res = batch_writer.update_storage(address, slot, value).await;
                    responders.push((resp, res));
                }
                WriteRequest::InsertBytecode { code_hash, bytecode, resp } => {
                    let res = batch_writer.insert_bytecode(code_hash, bytecode).await;
                    responders.push((resp, res));
                }
                WriteRequest::UpdateTrieNode { hash, node, resp } => {
                    let res = batch_writer.update_trie_node(hash, node).await;
                    responders.push((resp, res));
                }
                WriteRequest::UpdatePlainState { address, state, resp } => {
                    let res = batch_writer.update_plain_state(address, state).await;
                    responders.push((resp, res));
                }
                WriteRequest::RemovePlainState { address, resp } => {
                    let res = batch_writer.remove_plain_state(address).await;
                    responders.push((resp, res));
                }
                WriteRequest::UpdateHashedState { hash, state, resp } => {
                    let res = batch_writer.update_hashed_state(hash, state).await;
                    responders.push((resp, res));
                }
                WriteRequest::RegisterPeer { peer, resp } => {
                    let res = (|| async {
                        if let BatchTransaction::Write(wtx) = &batch_writer.tx {
                            let mut table = wtx.open_table(ActivePeers::definition())?;
                            table.insert(peer.peer_id.clone(), peer).map(|_| ()).map_err(|e| e.into())
                        } else {
                            Ok(())
                        }
                    })().await;
                    responders.push((resp, res));
                }
                WriteRequest::RemovePeer { peer_id, resp } => {
                    let res = (|| async {
                        if let BatchTransaction::Write(wtx) = &batch_writer.tx {
                            let mut table = wtx.open_table(ActivePeers::definition())?;
                            table.remove(peer_id).map(|_| ()).map_err(|e| e.into())
                        } else {
                            Ok(())
                        }
                    })().await;
                    responders.push((resp, res));
                }
                WriteRequest::ResetStorage { address, resp } => {
                    let res = batch_writer.reset_storage(address).await;
                    responders.push((resp, res));
                }
                WriteRequest::SetMetadata { key, value, resp } => {
                    let res = batch_writer.set_metadata(key, value).await;
                    responders.push((resp, res));
                }
                WriteRequest::InsertAccountChangeSet { number, change_set, resp } => {
                    let res = batch_writer.insert_account_change_set(number, change_set).await;
                    responders.push((resp, res));
                }
                WriteRequest::InsertStorageChangeSet { number, change_set, resp } => {
                    let res = batch_writer.insert_storage_change_set(number, change_set).await;
                    responders.push((resp, res));
                }
                WriteRequest::RemoveChangeSet { number, resp } => {
                    let res = batch_writer.remove_change_set(number).await;
                    responders.push((resp, res));
                }
                WriteRequest::InsertBlockFull {
                    block,
                    receipts,
                    metas,
                    td,
                    account_changes,
                    storage_changes,
                    resp
                } => {
                    let hash = block.header.hash_slow();
                    let number = block.header.number;
                    let res = (|| async {
                        batch_writer.insert_block(block, receipts, metas.clone()).await?;
                        batch_writer.insert_header_td(hash, td).await?;
                        batch_writer.insert_transaction_lookup_for_block(hash, number).await?;
                        batch_writer.remove_payload_by_block_hash(hash).await?;

                        // Insert ChangeSets
                        batch_writer.insert_account_change_set(number, account_changes).await?;
                        batch_writer.insert_storage_change_set(number, storage_changes).await?;

                        Ok(())
                    })().await;
                    responders.push((resp, res));
                }
                WriteRequest::AddPayload { id, block, receipts, metas, bundle, resp } => {
                    let res = batch_writer.add_payload(id, block, receipts, metas, bundle).await;
                    responders.push((resp, res));
                }
                WriteRequest::RemovePayloadByBlockHash { hash, resp } => {
                    let res = batch_writer.remove_payload_by_block_hash(hash).await;
                    responders.push((resp, res));
                }
            }
        }

        let commit_res = batch_writer.commit();
        for (resp, res) in responders {
            if commit_res.is_err() {
                let _ = resp.send(Err(anyhow::anyhow!("Batch commit failed")));
            } else {
                let _ = resp.send(res);
            }
        }

        commit_res?;
        Ok(())
    }
}
