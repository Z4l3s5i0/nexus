use crate::codecs::Table;
use crate::read::DatabaseReadProvider;
use crate::tables::*;
use crate::write::DatabaseWriteProvider;
use alloy_primitives::{Sealable, KECCAK256_EMPTY};
use redb::{Database, ReadableDatabase};
use std::path::Path;
use std::sync::Arc;
use wasix_eth_types::genesis::GenesisConfiguration;
use wasix_eth_types::{
    HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE,
    WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, WITHDRAWAL_REQUEST_PREDEPLOY_CODE,
    CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS, CONSOLIDATION_REQUEST_PREDEPLOY_CODE,
    proofs, Block, BlockBody, Header, Result, TrieAccount, B256, B64, EMPTY_OMMER_ROOT_HASH, U256, Hardfork, Address, BlobsBundleV1
};
use wasix_eth_utils::{debug, info};
use alloy_rlp::Encodable;
use alloy_trie::EMPTY_ROOT_HASH;
use crate::read_traits::{MetadataProvider, AccountProvider};
use wasix_eth_types::constants;
use crate::write_traits::{AccountWriter, BlockWriter, BytecodeWriter, HeaderWriter, MetadataWriter, StorageWriter};

pub struct EthDatabase {
    inner: Arc<Database>,
}

impl EthDatabase {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Database::create(path)?;
        let eth_db = Self {
            inner: Arc::new(db),
        };
        eth_db.init_tables()?;
        Ok(eth_db)
    }

    pub fn init_tables(&self) -> Result<()> {
        let wtx = self.inner.begin_write()?;
        
        // Initialize all tables
        wtx.open_table(Headers::definition())?;
        wtx.open_table(HeaderTD::definition())?;
        wtx.open_table(BlockBodies::definition())?;
        wtx.open_table(Transactions::definition())?;
        wtx.open_table(Receipts::definition())?;
        wtx.open_table(ReceiptsMeta::definition())?;
        wtx.open_table(CanonicalHeads::definition())?;
        wtx.open_table(HeaderNumbers::definition())?;
        wtx.open_table(TransactionLookup::definition())?;
        wtx.open_table(Accounts::definition())?;
        wtx.open_table(Storages::definition())?;
        wtx.open_table(Bytecodes::definition())?;
        wtx.open_table(AccountChangeSets::definition())?;
        wtx.open_table(StorageChangeSets::definition())?;
        wtx.open_table(PlainState::definition())?;
        wtx.open_table(HashedState::definition())?;
        wtx.open_table(TrieNodes::definition())?;
        wtx.open_table(Metadata::definition())?;
        wtx.open_table(PayloadsByHash::definition())?;
        wtx.open_table(PayloadIdToBlockHash::definition())?;
        wtx.open_table(Forkchoice::definition())?;
        wtx.open_table(ActivePeers::definition())?;

        wtx.commit()?;
        Ok(())
    }

    pub async fn init_genesis(&self, genesis: GenesisConfiguration) -> Result<()> {
        let read_provider = DatabaseReadProvider::new(self.inner.clone());
        let genesis_hash_meta = read_provider.get_metadata("genesis_hash".to_string()).await?;

        if genesis_hash_meta.is_none() {
            info!("[Storage] Initializing genesis block");
            let write_provider = DatabaseWriteProvider::new(self.inner.clone());

            // 1. Process allocations and calculate state root
            let state_root = self.calculate_genesis_state(&write_provider, &genesis).await?;
            
            debug!("[Storage] Genesis initialized with state root: {:?}", state_root);

            // 2. Build Genesis Block
            let timestamp = genesis.timestamp.unwrap_or(U256::ZERO).to::<u64>();
            let number = genesis.number.unwrap_or(U256::ZERO).to::<u64>();

            let fork = Hardfork::get_active_fork(&genesis.config, number, timestamp);

            let base_fee_per_gas = if fork >= Hardfork::London {
                Some(genesis.base_fee_per_gas.map(|v| v.to::<u64>()).unwrap_or(1_000_000_000))
            } else {
                None
            };

            let genesis_header = Header {
                number,
                timestamp,
                gas_limit: genesis.gas_limit.unwrap_or(U256::ZERO).to(),
                state_root,
                beneficiary: genesis.coinbase.unwrap_or_default(),
                difficulty: genesis.difficulty.unwrap_or_default(),
                mix_hash: genesis.mix_hash.unwrap_or_default(),
                nonce: B64::from(genesis.nonce.unwrap_or_default().to::<u64>()),
                base_fee_per_gas,
                extra_data: genesis.extra_data.clone().unwrap_or_default(),
                transactions_root: alloy_trie::EMPTY_ROOT_HASH,
                receipts_root: alloy_trie::EMPTY_ROOT_HASH,
                withdrawals_root: if fork >= Hardfork::Shanghai { 
                    Some(genesis.withdrawals_root.unwrap_or_else(|| proofs::calculate_withdrawals_root(&[]))) 
                } else { 
                    None 
                },
                gas_used: genesis.gas_used.unwrap_or(U256::ZERO).to(),
                parent_hash: genesis.parent_hash.unwrap_or_default(),
                ommers_hash: EMPTY_OMMER_ROOT_HASH,
                logs_bloom: Default::default(),
                blob_gas_used: if fork >= Hardfork::Cancun { Some(genesis.blob_gas_used.map(|v| v.to::<u64>()).unwrap_or(0)) } else { None },
                excess_blob_gas: if fork >= Hardfork::Cancun { Some(genesis.excess_blob_gas.map(|v| v.to::<u64>()).unwrap_or(0)) } else { None },
                parent_beacon_block_root: if fork >= Hardfork::Cancun { Some(genesis.parent_beacon_block_root.unwrap_or(B256::ZERO)) } else { None },
                requests_hash: if fork >= Hardfork::Prague { Some(genesis.requests_hash.unwrap_or(constants::EMPTY_REQUESTS_HASH)) } else { None },
                block_access_list_hash: None,
                slot_number: None,
            };

            let genesis_header_clone = genesis_header.clone();
            let block_number = genesis_header_clone.number;

            debug!("[Storage] Genesis header object: {:?}", genesis_header);
            debug!("[Storage] Genesis header (RLP): {:?}", wasix_eth_types::hex::encode(alloy_rlp::encode(&genesis_header)));
            let genesis_hash = genesis_header.clone().seal_slow();
            debug!("[Storage] Genesis hash (calculated): {:?}", genesis_hash);
            debug!("[Storage] Genesis hash (hash_slow): {:?}", genesis_header.hash_slow());

            // 3. Persist Block and Metadata
            write_provider.insert_header(genesis_hash.hash(), genesis_header_clone).await?;
            write_provider.set_canonical(block_number, genesis_hash.hash()).await?;
            write_provider.insert_block_hash(genesis_hash.hash(), block_number).await?;
            write_provider.insert_header_number(genesis_hash.hash(), block_number).await?;
            
            let body = BlockBody {
                transactions: Vec::new(),
                ommers: Vec::new(),
                withdrawals: if fork >= Hardfork::Shanghai { Some(wasix_eth_types::eip4895::Withdrawals::new(Vec::new())) } else { None },
            };
            write_provider.insert_block_body(genesis_hash.hash(), block_number, body.clone()).await?;
            debug!("[Storage] Genesis block body persisted for hash: {:?}", genesis_hash.hash());
            write_provider.update_forkchoice(genesis_hash.hash(), None, None).await?;
            write_provider.insert_header_td(genesis_hash.hash(), genesis_header.difficulty).await?;

            // Also insert into Payloads table for consistency in some lookups
            let payload_id = wasix_eth_types::PayloadId::new([0u8; 8]);
            write_provider.add_payload(payload_id, Block { header: genesis_header, body }, Vec::new(), Vec::new(), BlobsBundleV1::default()).await?;

            write_provider.set_metadata("chain_id".to_string(), genesis.config.chain_id.to_be_bytes().to_vec().into()).await?;
            write_provider.set_metadata("genesis_hash".to_string(), genesis_hash.hash().as_slice().to_vec().into()).await?;
            write_provider.set_metadata("canonical_0".to_string(), genesis_hash.hash().as_slice().to_vec().into()).await?;

            debug!("[Storage] Genesis state root (calculated): {:?}", state_root);
            info!("[Storage] Genesis initialized with hash: {:?}", genesis_hash.hash());
        } else {
            debug!("[Storage] Genesis already initialized");
        }

        // Always ensure chain_config is set in metadata if we have it
        let read_provider = DatabaseReadProvider::new(self.inner.clone());
        if read_provider.get_metadata("chain_config".to_string()).await?.is_none() {
             let write_provider = DatabaseWriteProvider::new(self.inner.clone());
             let config_json = serde_json::to_vec(&genesis.config)?;
             write_provider.set_metadata("chain_config".to_string(), config_json.into()).await?;
        }

        Ok(())
    }

    async fn calculate_genesis_state(&self, write_provider: &DatabaseWriteProvider, genesis: &GenesisConfiguration) -> Result<B256> {
        let batch = write_provider.begin_write_batch()?;
        
        let timestamp = genesis.timestamp.unwrap_or(U256::ZERO).to::<u64>();
        let number = genesis.number.unwrap_or(U256::ZERO).to::<u64>();
        let fork = Hardfork::get_active_fork(&genesis.config, number, timestamp);
        let eip161 = fork >= Hardfork::SpuriousDragon;
        let beneficiary = genesis.coinbase.unwrap_or(Address::ZERO);

        debug!("[Execution] GENESIS STATE CALCULATION BEGIN");
        debug!("[Execution] GENESIS CONFIG: coinbase={:?}, number={}, timestamp={}, fork={:?}, eip161={}", beneficiary, number, timestamp, fork, eip161);

        if fork >= Hardfork::Prague {
            async fn ensure_code(batch: &crate::write::BatchWriter, addr: Address, code: &[u8]) -> Result<()> {
                let account = batch.account(addr, None).await?;
                let is_new = account.is_none();
                let mut contract_acc = account.unwrap_or(TrieAccount {
                    nonce: 1,
                    balance: U256::ZERO,
                    storage_root: alloy_trie::EMPTY_ROOT_HASH,
                    code_hash: KECCAK256_EMPTY,
                });
                let code_hash = alloy_primitives::keccak256(code);
                if is_new || contract_acc.code_hash != code_hash {
                    contract_acc.code_hash = code_hash;
                    if is_new { contract_acc.nonce = 1; }
                    batch.insert_bytecode(code_hash, code.to_vec().into()).await?;
                    batch.update_account(addr, contract_acc).await?;
                }
                Ok(())
            }

            ensure_code(&batch, HISTORY_STORAGE_ADDRESS, &HISTORY_STORAGE_CODE).await?;
            batch.mark_account_touched(HISTORY_STORAGE_ADDRESS).await?;
            ensure_code(&batch, WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, &WITHDRAWAL_REQUEST_PREDEPLOY_CODE).await?;
            batch.mark_account_touched(WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS).await?;
            ensure_code(&batch, CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS, &CONSOLIDATION_REQUEST_PREDEPLOY_CODE).await?;
            batch.mark_account_touched(CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS).await?;
            debug!("[Execution] Prague system contracts initialized");
        }

        for (address, account) in &genesis.alloc {
            let addr = *address;
            
            for (slot, value) in account.storage.as_ref().unwrap_or(&Default::default()) {
                let slot_b256 = *slot;
                let val_u256 = (*value).into();
                
                batch.update_storage(addr, slot_b256, val_u256).await?;
            }

            let code_hash = if let Some(c) = &account.code {
                let hash = alloy_primitives::keccak256(c);
                batch.insert_bytecode(hash, c.clone()).await?;
                hash
            } else {
                KECCAK256_EMPTY
            };

            let trie_acc = TrieAccount {
                nonce: account.nonce.unwrap_or(0),
                balance: account.balance,
                storage_root: alloy_trie::EMPTY_ROOT_HASH,
                code_hash,
            };

            debug!("[Execution] Adding account to genesis trie: addr={:?}, balance={}, nonce={}, storage={:?}, code={:?}", addr, trie_acc.balance, trie_acc.nonce, trie_acc.storage_root, trie_acc.code_hash);

            batch.update_account(addr, trie_acc).await?;
            batch.mark_account_touched(addr).await?;
        }

        let state_root = batch.calculate_state_root(eip161, None).await?;
        batch.commit()?;
        
        debug!("[Execution] GENESIS STATE ROOT CALCULATED: {:?}", state_root);

        Ok(state_root)
    }

    pub async fn calculate_state_root(&self, is_eip161: bool) -> Result<B256> {
        let write_provider = DatabaseWriteProvider::new(self.inner.clone());
        write_provider.calculate_state_root(is_eip161, None).await
    }

    pub fn inner(&self) -> Arc<Database> {
        self.inner.clone()
    }

    pub fn begin_read(&self) -> Result<redb::ReadTransaction> {
        Ok(self.inner.begin_read()?)
    }

    pub fn begin_write(&self) -> Result<redb::WriteTransaction> {
        Ok(self.inner.begin_write()?)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use wasix_eth_types::Bytes;

    #[test]
    fn test_bytes_parsing() {
        let chain_id: u64 = 31133;
        let s = chain_id.to_string();
        println!("Testing parsing '{}' as Bytes", s);
        let res = Bytes::from_str(&s);
        assert!(res.is_err(), "Should fail parsing decimal string as Bytes");
        assert!(res.unwrap_err().to_string().contains("odd number of digits"));
    }

    #[test]
    fn test_chain_id_encoding() {
        let chain_id: u64 = 31133;
        let bytes: Bytes = chain_id.to_be_bytes().to_vec().into();
        assert_eq!(bytes.len(), 8);
        assert_eq!(u64::from_be_bytes(bytes.as_ref().try_into().unwrap()), chain_id);
    }
}
