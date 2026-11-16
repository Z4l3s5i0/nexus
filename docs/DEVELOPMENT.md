# Developer Guide

This guide covers everything needed to build, run, test, and debug NEXUS (the `wasix_eth` crate).

## Prerequisites

### Required Tools

- **Rust** (stable or nightly) — install via [rustup.rs](https://rustup.rs)
- **Wasmer** — runtime for executing WASIX binaries
- **WASIX toolchain**:
  - `cargo-wasix` — Cargo subcommand for building WASIX targets
  - `wasi-sdk` — C/C++ toolchain for WASI
  - `clang` + `llvm` — required by WASIX build process

### Optional Tools

- **Docker** — for running the HIVE test suite
- **Lighthouse** — consensus client for full node integration testing

### Installing WASIX

```bash
cargo install cargo-wasix
```

Follow the [WASIX documentation](https://wasix.io/) for installing `wasi-sdk`, `clang`, and `llvm`.

## Building the Project

### Native Build

Build for the host platform:

```bash
cd wasix_eth
cargo build --release
```

The binary will be at `target/release/wasix_eth`.

### WASIX Build

Build for the WASIX target (WebAssembly):

```bash
cd wasix_eth
cargo wasix build --release
```

The WASM binary will be at `target/wasm32-wasmer-wasi/release/wasix_eth.wasm`.

### Building Specific Crates

```bash
cargo build -p wasix_eth_core --release
cargo build -p wasix_eth_rpc --release
```

## Running a Local Node

### Help

```bash
cargo run --release -- --help
```

### Dev Mode

Run a local development node with automatic block production (default: every 12 seconds):

```bash
cargo run --release -- --dev
```

Custom block production interval (e.g., 6 seconds):

```bash
cargo run --release -- --dev 6
```

### With Custom Genesis

```bash
cargo run --release -- --genesis-path /path/to/genesis.json --chain devnet
```

### With Custom Data Directory

```bash
cargo run --release -- --data-dir /path/to/data --chain devnet
```

### Full Configuration Example

```bash
cargo run --release -- \
  --data-dir ./my-node-data \
  --genesis-path ./genesis.json \
  --chain devnet \
  --dev 12 \
  --discovery-port 30303 \
  --p2p-port 30304 \
  --eth-rpc-port 8545 \
  --auth-rpc-port 8551 \
  --auth-rpc-jwt-path ./jwtsecret \
  --bootnodes enode://...@ip:30303,enode://...@ip:30303 \
  --max-peers 50 \
  --verbose 2 \
  --metrics-port 9050
```

### Running with Lighthouse (Consensus Client)

1. Generate a JWT secret for Engine API authentication:

```bash
openssl rand -hex 32 > jwtsecret
```

2. Start wasix_eth with the JWT secret:

```bash
cargo run --release -- \
  --chain devnet \
  --auth-rpc-jwt-path ./jwtsecret \
  --auth-rpc-port 8551 \
  --eth-rpc-port 8545 \
  --verbose 2
```

3. Start Lighthouse pointing to the execution client:

```bash
lighthouse --network devnet \
  --http-allow-origin "*" \
  --execution-endpoint http://localhost:8551 \
  --jwt-secrets ./jwtsecret \
  --disable-discovery \
  beacon
```

### Running WASIX Binary

```bash
wasmer run target/wasm32-wasmer-wasi/release/wasix_eth.wasm -- --dev
```

### Initializing Genesis Only

```bash
cargo run --release -- init --data-dir ./my-node-data --genesis-path ./genesis.json
```

### Importing Blocks from RLP

Import a single RLP file:

```bash
cargo run --release -- import --import-chain /path/to/blocks.rlp
```

Import from a directory of RLP files:

```bash
cargo run --release -- import --import-blocks /path/to/blocks/
```

## CLI Reference

### Subcommands

| Command | Description |
|---|---|
| `run` | Run the node (default if no subcommand given) |
| `init` | Initialize the genesis state only |
| `import` | Import blocks from RLP files |

### Common Arguments

| Argument | Default | Description |
|---|---|---|
| `--data-dir` | — | Path for persistent storage |
| `--genesis-path` | — | Path to the genesis JSON file |
| `--verbose` | `1` | Verbosity level (0: none, 1: experimental, 2: info, 3: debug) |
| `--peer-name` | `wasix-eth` | Descriptive name for the node |
| `--discovery-port` | `30303` | UDP port for Discv4 discovery |
| `--p2p-port` | `30304` | TCP port for P2P connections |
| `--eth-rpc-port` | `8545` | TCP port for Eth JSON-RPC |
| `--auth-rpc-port` | `8551` | TCP port for Auth Engine JSON-RPC |
| `--bootnodes` | — | Comma-separated Multiaddrs for bootstrapping |
| `--max-peers` | `50` | Maximum concurrent P2P connections |
| `--ext-ip` | — | External IP address |
| `--chain` | `devnet` | Chain name (mainnet, sepolia, devnet) |
| `--auth-rpc-jwt-path` | — | Path to JWT secret for Engine API |
| `--dev` | — | Enable dev mode with auto block production (optional interval in seconds) |
| `--slot-duration` | `12` | Slot duration in seconds |
| `--metrics-port` | `9050` | Port for Prometheus metrics |
| `--import-chain` | — | Path to a single RLP file containing blocks |
| `--import-blocks` | — | Path to a directory of RLP files |
| `--bootstrap-registry` | — | URL of the metrics server for auto-registration |

## Running Tests

### Unit Tests

Run all tests:

```bash
cd wasix_eth
cargo test
```

Run tests for a specific crate:

```bash
cargo test -p wasix_eth_core
cargo test -p wasix_eth_storage
```

Run a specific test:

```bash
cargo test -p wasix_eth_storage test_bytes_parsing
```

### HIVE Integration Tests

The `hive/` directory contains a custom Docker setup for the [Ethereum HIVE test suite](https://github.com/ethereum/hive).

```bash
cd hive
docker compose up
```

This runs the Engine API simulation tests against wasix_eth to verify consensus client compatibility.

## Logging and Debugging

### Verbosity Levels

The `--verbose` flag controls log output:

| Level | Output |
|---|---|
| `0` | No output |
| `1` | Experimental logging — `[EXP]` tagged events for benchmarking and analysis (default) |
| `2` | Info level — block processing, sync progress, peer connections |
| `3` | Debug level — detailed transaction and state changes |

### Log Format

Logs use structured timestamps and module prefixes:

```
[2024-01-15T10:30:00.123Z] [Engine] forkchoice_updated: head=0xabc... safe=0xdef... finalized=0x123...
[2024-01-15T10:30:00.456Z] [SyncOrchestrator] Notified of new sync targets
[2024-01-15T10:30:01.789Z] [Storage] Genesis initialized with hash: 0x...
```

### Prometheus Metrics

The node exposes Prometheus metrics on the port specified by `--metrics-port` (default: 9050). Key metrics:

- `current_head_block` — current canonical head block number
- `transactions_committed_total` — total transactions committed to the chain

### Debugging Tips

- Use `--verbose 3` for detailed debug output during development
- Check the data directory for the `redb` database files if investigating storage issues
- The `--bootstrap-registry` flag enables integration with the `metrics_server` for peer discovery in multi-node experiments

## Code Conventions and Workspace Structure

### Workspace Layout

```
wasix_eth/
├── Cargo.toml                 # Workspace definition
├── wasix_eth_app/             # CLI application
│   └── src/
│       ├── main.rs            # Entry point
│       ├── lib.rs             # Library exports
│       ├── app.rs             # App orchestration
│       ├── cli.rs             # CLI argument parsing
│       ├── node.rs            # Node lifecycle
│       └── node_components/   # Node component setup
├── wasix_eth_core/            # Core protocol logic
│   └── src/
│       ├── engine/            # Engine API implementation
│       ├── mempool/           # Transaction pool
│       ├── gossip/            # P2P transaction propagation
│       ├── sync/              # Chain synchronization
│       ├── chain_manager.rs   # Chain state management
│       ├── consensus.rs       # Consensus validation
│       └── account_manager.rs # Account management
├── wasix_eth_execution/       # EVM execution layer
│   └── src/
│       ├── executor.rs        # Transaction executor
│       ├── backend.rs         # EVM backend
│       ├── state.rs           # State management
│       ├── config.rs          # EVM configuration
│       ├── transaction.rs     # Transaction execution
│       ├── block.rs           # Block execution
│       └── execution_provider.rs
├── wasix_eth_p2p/             # P2P networking
│   └── src/
│       ├── discv4/            # Node discovery
│       ├── rlp_x/             # RLPx transport
│       ├── peer_manager.rs    # Peer management
│       └── sync_service.rs    # Sync service
├── wasix_eth_rpc/             # JSON-RPC server
│   └── src/
│       ├── eth/               # eth_* namespace
│       ├── engine/            # engine_* namespace
│       ├── admin/             # admin_* namespace
│       ├── net/               # net_* namespace
│       ├── web3/              # web3_* namespace
│       └── debug/             # debug_* namespace
├── wasix_eth_storage/         # Persistent storage
│   └── src/
│       ├── db.rs              # EthDatabase
│       ├── read.rs            # Read provider
│       ├── write.rs           # Write provider
│       ├── tables.rs          # Table definitions
│       ├── codecs.rs          # Serialization
│       └── trie.rs            # Merkle Patricia Trie
├── wasix_eth_types/           # Shared types and traits
│   └── src/
│       └── (type definitions, traits, genesis, engine types)
├── wasix_eth_utils/           # Utilities
│   └── src/
│       ├── logging.rs         # Logging setup
│       ├── metrics.rs         # Prometheus metrics
│       ├── mappers/           # Block/tx/engine mappers
│       └── identity.rs        # Node identity
└── testing/                   # Test utilities
    └── src/
        └── lib.rs
```

### Code Style

- Follow standard Rust conventions (rustfmt)
- Use `clippy` for linting: `cargo clippy --workspace`
- Module names use snake_case
- Public types and functions use PascalCase and snake_case respectively
- Log messages use module prefixes: `[Engine]`, `[Storage]`, `[SyncOrchestrator]`

### Dependencies

Key external dependencies:

- `alloy-*` ecosystem — Ethereum types and protocol definitions
- `evm` — EVM execution engine
- `tokio` — async runtime
- `jsonrpsee` — JSON-RPC server
- `redb` — embedded key-value database
- `prometheus` — metrics collection
- `clap` — CLI argument parsing
- `dashmap` — concurrent hash map for in-memory state
