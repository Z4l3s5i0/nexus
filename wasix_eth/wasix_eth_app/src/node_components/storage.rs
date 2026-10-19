use wasix_eth_storage::{EthDatabase, read::DatabaseReadProvider, write::DatabaseWriteProvider};
use wasix_eth_types::genesis::GenesisConfiguration;
use std::path::PathBuf;
use std::sync::Arc;
use std::error::Error;

pub struct StorageConfig {
    pub data_dir: PathBuf,
    pub storage_name: String,
}

#[derive(Clone)]
pub struct StoragePayload {
    pub read_provider: Arc<DatabaseReadProvider>,
    pub write_provider: Arc<DatabaseWriteProvider>,
}

impl StoragePayload {
    pub async fn new(config: StorageConfig, genesis_config: GenesisConfiguration) -> Result<Self, Box<dyn Error>> {
        let db_path = config.data_dir.join(&config.storage_name);
        let eth_db = EthDatabase::open(&db_path)?;
        eth_db.init_genesis(genesis_config).await?;

        let read_provider = Arc::new(DatabaseReadProvider::new(eth_db.inner()));
        let write_provider = Arc::new(DatabaseWriteProvider::new(eth_db.inner()));

        Ok(Self {
            read_provider,
            write_provider,
        })
    }

    pub fn new_no_init(config: StorageConfig) -> Result<Self, Box<dyn Error>> {
        let db_path = config.data_dir.join(&config.storage_name);
        let eth_db = EthDatabase::open(&db_path)?;

        let read_provider = Arc::new(DatabaseReadProvider::new(eth_db.inner()));
        let write_provider = Arc::new(DatabaseWriteProvider::new(eth_db.inner()));

        Ok(Self {
            read_provider,
            write_provider,
        })
    }
}
