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
    proofs, Block, BlockBody, ConsensusBlockBody, Header, Result, TrieAccount, B256, B64,
    EMPTY_OMMER_ROOT_HASH, U256, Hardfork, Address, BlobsBundleV1, Transaction, eip4895::Withdrawals,
};
use wasix_eth_utils::{debug, info};
use alloy_rlp::Encodable;
use crate::read_traits::MetadataProvider;
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
        wtx.open_table(HashedStorages::definition())?;
        wtx.open_table(TrieNodes::definition())?;
        wtx.open_table(Metadata::definition())?;
        wtx.open_table(Payloads::definition())?;
        wtx.open_table(Forkchoice::definition())?;
        wtx.open_table(ActivePeers::definition())?;

        wtx.commit()?;
        Ok(())
    }

    pub fn init_genesis(&self, genesis: GenesisConfiguration) -> Result<()> {
        let read_provider = DatabaseReadProvider::new(self.inner.clone());
        let genesis_hash_meta = read_provider.get_metadata("genesis_hash".to_string())?;

        if genesis_hash_meta.is_none() {
            info!("[Storage] Initializing genesis block");
            let write_provider = DatabaseWriteProvider::new(self.inner.clone());

            // 1. Process allocations and calculate state root
            let state_root = self.calculate_genesis_state(&write_provider, &genesis)?;
            
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
                withdrawals_root: if fork >= Hardfork::Shanghai { Some(proofs::calculate_withdrawals_root(&[])) } else { None },
                gas_used: genesis.gas_used.unwrap_or(U256::ZERO).to(),
                parent_hash: genesis.parent_hash.unwrap_or_default(),
                ommers_hash: EMPTY_OMMER_ROOT_HASH,
                logs_bloom: Default::default(),
                blob_gas_used: if fork >= Hardfork::Cancun { Some(genesis.blob_gas_used.map(|v| v.to::<u64>()).unwrap_or(0)) } else { None },
                excess_blob_gas: if fork >= Hardfork::Cancun { Some(genesis.excess_blob_gas.map(|v| v.to::<u64>()).unwrap_or(0)) } else { None },
                parent_beacon_block_root: if fork >= Hardfork::Cancun { Some(B256::ZERO) } else { None },
                ..Default::default()
            };

            let genesis_header_clone = genesis_header.clone();
            let block_number = genesis_header_clone.number;

            debug!("[Storage] Genesis header (RLP): {:?}", wasix_eth_types::hex::encode(alloy_rlp::encode(&genesis_header)));
            let genesis_hash = genesis_header.clone().seal_slow();
            debug!("[Storage] Genesis hash (calculated): {:?}", genesis_hash);

            // 3. Persist Block and Metadata
            write_provider.insert_header(genesis_hash.hash(), genesis_header_clone)?;
            write_provider.set_canonical(block_number, genesis_hash.hash())?;
            write_provider.insert_block_hash(genesis_hash.hash(), block_number)?;
            write_provider.insert_header_number(genesis_hash.hash(), block_number)?;
            
            let body = BlockBody {
                transactions: Vec::new(),
                ommers: Vec::new(),
                withdrawals: if fork >= Hardfork::Shanghai { Some(Vec::new()) } else { None },
            };
            write_provider.insert_block_body(genesis_hash.hash(), block_number, body.clone())?;
            write_provider.update_forkchoice(genesis_hash.hash(), None, None)?;
            write_provider.insert_header_td(genesis_hash.hash(), genesis_header.difficulty)?;

            // Also insert into Payloads table for consistency in some lookups
            let payload_id = wasix_eth_types::PayloadId::new([0u8; 8]);
            write_provider.add_payload(
                payload_id,
                Block {
                    header: genesis_header,
                    body: ConsensusBlockBody {
                        transactions: body.transactions,
                        ommers: body.ommers,
                        withdrawals: body.withdrawals.map(|w| wasix_eth_types::eip4895::Withdrawals::new(w.to_vec())),
                    },
                },
                Vec::new(),
                Vec::new(),
                BlobsBundleV1::default(),
            )?;

            write_provider.set_metadata("chain_id".to_string(), genesis.config.chain_id.to_be_bytes().to_vec().into())?;
            write_provider.set_metadata("genesis_hash".to_string(), genesis_hash.hash().as_slice().to_vec().into())?;
            write_provider.set_metadata("canonical_0".to_string(), genesis_hash.hash().as_slice().to_vec().into())?;

            debug!("[Storage] Genesis state root (calculated): {:?}", state_root);
            info!("[Storage] Genesis initialized with hash: {:?}", genesis_hash.hash());
        } else {
            debug!("[Storage] Genesis already initialized");
        }

        // Always ensure chain_config is set in metadata if we have it
        let read_provider = DatabaseReadProvider::new(self.inner.clone());
        if read_provider.get_metadata("chain_config".to_string())?.is_none() {
             let write_provider = DatabaseWriteProvider::new(self.inner.clone());
             let config_json = serde_json::to_vec(&genesis.config)?;
             write_provider.set_metadata("chain_config".to_string(), config_json.into())?;
        }

        Ok(())
    }

    fn calculate_genesis_state(&self, write_provider: &DatabaseWriteProvider, genesis: &GenesisConfiguration) -> Result<B256> {
        let batch = write_provider.begin_batch()?;
        
        let timestamp = genesis.timestamp.unwrap_or(U256::ZERO).to::<u64>();
        let number = genesis.number.unwrap_or(U256::ZERO).to::<u64>();
        let fork = Hardfork::get_active_fork(&genesis.config, number, timestamp);
        let eip161 = fork >= Hardfork::SpuriousDragon;
        let beneficiary = genesis.coinbase.unwrap_or(Address::ZERO);

        debug!("[Execution] GENESIS STATE CALCULATION BEGIN");
        debug!("[Execution] GENESIS CONFIG: coinbase={:?}, number={}, timestamp={}, fork={:?}, eip161={}", beneficiary, number, timestamp, fork, eip161);

        for (address, account) in &genesis.alloc {
            let addr = *address;
            
            // debug!(
            //     "[Execution] GENESIS ALLOC ACCOUNT:
            //         addr={:?}
            //         nonce={}
            //         balance={}
            //         code_len={}",
            //     addr,
            //     account.nonce.unwrap_or(0),
            //     account.balance,
            //     account.code.as_ref().map(|c| c.len()).unwrap_or(0)
            // );

            for (slot, value) in account.storage.as_ref().unwrap_or(&Default::default()) {
                let slot_b256 = *slot;
                let val_u256 = (*value).into();
                
                // debug!(
                //     "[Execution] STORAGE DIFF:
                //         addr={:?}
                //         slot={:?}
                //         old={:?}
                //         new={:?}",
                //     addr,
                //     slot_b256,
                //     U256::ZERO,
                //     val_u256,
                // );

                batch.update_storage(addr, slot_b256, val_u256)?;
            }

            let storage_root = batch.calculate_storage_root(addr, None)?;

            let trie_account = TrieAccount {
                nonce: account.nonce.unwrap_or(0),
                balance: account.balance,
                storage_root,
                code_hash: account.code.as_ref().map(|c| {
                    let hash = alloy_primitives::keccak256(c);
                    batch.insert_bytecode(hash, c.clone()).unwrap();
                    hash
                }).unwrap_or(KECCAK256_EMPTY),
            };

            if addr == Address::ZERO {
                let mut encoded = Vec::new();
                trie_account.encode(&mut encoded);
            }

            batch.update_account(addr, trie_account)?;
        }

        let state_root = batch.calculate_state_root(eip161, None)?;
        batch.commit()?;
        
        debug!(
            "[Execution] GENESIS STATE ROOT:
                root={:?}",
            state_root,
        );

        Ok(state_root)
    }

    pub fn calculate_state_root(&self, is_eip161: bool) -> Result<B256> {
        let write_provider = DatabaseWriteProvider::new(self.inner.clone());
        write_provider.calculate_state_root(is_eip161, None)
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
