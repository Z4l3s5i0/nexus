use anyhow::{Result, Context};
use clap::{Parser, Subcommand};
use redb::{Database, TableDefinition, ReadableDatabase, ReadableTable, TableHandle};
use serde_json::{Value, json};
use wasix_eth_storage::codecs::{RlpValue, Table};
use wasix_eth_storage::tables::*;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Export metrics-server or EL node redb to JSON
    Export {
        /// Path to the redb file
        #[arg(short, long)]
        db_path: String,
        /// Table to export
        #[arg(short, long)]
        table: String,
        /// Type of database (metrics, el)
        #[arg(short, long, default_value = "metrics")]
        db_type: String,
    },
    /// List tables in a redb file
    List {
        /// Path to the redb file
        #[arg(short, long)]
        db_path: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Export { db_path, table, db_type } => {
            if db_type == "el" {
                export_el(&db_path, &table)?;
            } else {
                export_metrics(&db_path, &table)?;
            }
        }
        Commands::List { db_path } => {
            list_tables(&db_path)?;
        }
    }

    Ok(())
}

fn list_tables(db_path: &str) -> Result<()> {
    let mut db = Database::open(db_path).context("Failed to open database")?;
    
    // Repair database if corrupted
    match db.check_integrity() {
        Ok(true) => (),
        Ok(false) => println!("Database {} was repaired automatically", db_path),
        Err(e) => eprintln!("Warning: integrity check failed for {}: {}", db_path, e),
    }

    println!("Tables in {}:", db_path);
    
    if let Ok(write_txn) = db.begin_write() {
        if let Ok(tables) = write_txn.list_tables() {
            for table_handle in tables {
                println!("  - {}", table_handle.name());
            }
        }
    }
    Ok(())
}

fn export_metrics(db_path: &str, table_name: &str) -> Result<()> {
    let mut db = Database::open(db_path).context("Failed to open database")?;
    
    // Repair database if corrupted
    match db.check_integrity() {
        Ok(true) => (),
        Ok(false) => println!("Database {} was repaired automatically", db_path),
        Err(e) => eprintln!("Warning: integrity check failed for {}: {}", db_path, e),
    }

    let read_txn = db.begin_read()?;
    
    let definition: TableDefinition<&str, &str> = TableDefinition::new(table_name);
    let table = read_txn.open_table(definition).context(format!("Table {} not found", table_name))?;
    
    let mut results = Vec::new();
    for item in table.iter()? {
        let (key, value) = item?;
        let key_str = key.value();
        let val_str = value.value();
        
        // Try to parse value as JSON
        let json_val: Value = serde_json::from_str(val_str).unwrap_or(Value::String(val_str.to_string()));
        
        results.push(json!({
            "key": key_str,
            "value": json_val
        }));
    }

    if results.is_empty() {
        eprintln!("Warning: Table {} is empty in {}", table_name, db_path);
    }
    
    println!("{}", serde_json::to_string_pretty(&results)?);
    Ok(())
}

fn export_el(db_path: &str, table_name: &str) -> Result<()> {
    let mut db = Database::open(db_path).context("Failed to open database")?;
    
    // Repair database if corrupted
    match db.check_integrity() {
        Ok(true) => (),
        Ok(false) => println!("Database {} was repaired automatically", db_path),
        Err(e) => eprintln!("Warning: integrity check failed for {}: {}", db_path, e),
    }

    let read_txn = db.begin_read()?;

    macro_rules! try_export {
        ($table_struct:ident) => {
            if table_name == $table_struct::NAME {
                let table = read_txn.open_table($table_struct::definition())?;
                let mut results = Vec::new();
                for item in table.iter()? {
                    let (key, value) = item?;
                    // We just want to export as hex if we don't want to deal with complex JSON serialization of all types
                    // but we can try to use Debug if we want something readable.
                    // For now, let's stick to hex for values to be safe and consistent with previous tool.
                    results.push(json!({
                        "key_debug": format!("{:?}", key.value()),
                        "value_debug": format!("{:?}", value.value())
                    }));
                }
                println!("{}", serde_json::to_string_pretty(&results)?);
                return Ok(());
            }
        };
    }
    try_export!(Headers);
    try_export!(HeaderTD);
    try_export!(BlockBodies);
    try_export!(Transactions);
    try_export!(Receipts);
    try_export!(ReceiptsMeta);
    try_export!(CanonicalHeads);
    try_export!(HeaderNumbers);
    try_export!(TransactionLookup);
    try_export!(Accounts);
    try_export!(Storages);
    try_export!(Bytecodes);
    try_export!(AccountChangeSets);
    try_export!(StorageChangeSets);
    try_export!(TrieNodes);
    try_export!(TrieNodeRefs);
    try_export!(Metadata);
    try_export!(Payloads);
    try_export!(Forkchoice);
    try_export!(ActivePeers);
    try_export!(StateBranches);

    Err(anyhow::anyhow!("Table {} not supported or recognized", table_name))
}
