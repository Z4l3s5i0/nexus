use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[command(flatten)]
    pub common: CommonArgs,
}

#[derive(Parser, Debug, Clone)]
pub struct CommonArgs {
    /// Path for persistent storage
    #[arg(long)]
    pub data_dir: Option<PathBuf>,

    /// Path to the genesis JSON file
    #[arg(long)]
    pub genesis_path: Option<PathBuf>,

    /// Verbosity level (0: none, 1: exp, 2: info, 3: debug)
    #[arg(long, default_value_t = 1)]
    pub verbose: u8,

    /// Descripive name for the node, used in storage filenames
    #[arg(long, default_value = "wasix-eth")]
    pub peer_name: Option<String>,

    /// UDP port for discovery
    #[arg(long, default_value_t = 30303)]
    pub discovery_port: u16,

    /// TCP port for P2P (gossip, blocks)
    #[arg(long, default_value_t = 30304)]
    pub p2p_port: u16,

    /// TCP port for Eth JSON-RPC
    #[arg(long, default_value_t = 8545)]
    pub eth_rpc_port: u16,

    /// TCP port for Auth Engine JSON-RPC
    #[arg(long, default_value_t = 8551)]
    pub auth_rpc_port: u16,

    /// Comma-separated list of Multiaddrs for bootstrapping
    #[arg(long, value_delimiter = ',')]
    pub bootnodes: Vec<String>,

    /// Maximum number of concurrent P2P connections
    #[arg(long, default_value_t = 50)]
    pub max_peers: usize,

    /// External IP (optional)
    #[arg(long)]
    pub ext_ip: Option<std::net::IpAddr>,

    /// Chain name (mainnet, sepolia, devnet)
    #[arg(long, default_value = "devnet")]
    pub chain: String,

    /// Path to the JWT secret for the Auth Engine JSON-RPC
    #[arg(long)]
    pub auth_rpc_jwt_path: Option<PathBuf>,

    /// Interval in seconds for automatic block production in dev mode
    #[arg(long, default_missing_value = "12", num_args = 0..=1)]
    pub dev: Option<u64>,

    /// Slot duration in seconds
    #[arg(long, default_value_t = 12)]
    pub slot_duration: u64,

    /// Descriptive name for the node, used in storage filenames
    #[arg(long, default_value_t = 9050)]
    pub metrics_port: u16,

    /// Path to a single RLP file containing blocks to import
    #[arg(long)]
    pub import_chain: Option<PathBuf>,

    /// Path to a directory containing .rlp files to import
    #[arg(long)]
    pub import_blocks: Option<PathBuf>,

    /// URL of the metrics server for auto-registration and bootstrap peer discovery
    #[arg(long)]
    pub bootstrap_registry: Option<String>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Run the node
    Run {
        #[command(flatten)]
        common: CommonArgs,
    },
    /// Initialize the genesis state
    Init {
        #[command(flatten)]
        common: CommonArgs,
    },
    /// Import blocks from RLP files
    Import {
        #[command(flatten)]
        common: CommonArgs,
    },
}


