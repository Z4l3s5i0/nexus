# NEXUS

[![Setup Environment](https://github.com/Z4l3s5i0/nexus/actions/workflows/setup.yml/badge.svg)](https://github.com/Z4l3s5i0/nexus/actions/workflows/setup.yml)
[![Build & Test Native](https://github.com/Z4l3s5i0/nexus/actions/workflows/build-test-native.yml/badge.svg)](https://github.com/Z4l3s5i0/nexus/actions/workflows/build-test-native.yml)
[![Build & Test WASIX](https://github.com/Z4l3s5i0/nexus/actions/workflows/build-test-wasix.yml/badge.svg)](https://github.com/Z4l3s5i0/nexus/actions/workflows/build-test-wasix.yml)

> An Ethereum execution client written in Rust, running inside a WebAssembly (WASIX) environment.

**NEXUS** (powered by the `wasix_eth` crate) implements the core Ethereum protocol — EVM execution, block processing, P2P networking (RLPx + Discv4), JSON-RPC APIs (including the Engine API for consensus client integration), mempool management, chain synchronization, and reorg handling — while being portable across platforms thanks to WebAssembly.

## Key Features

- **EVM Execution** — Full Ethereum Virtual Machine execution via the `evm` crate, with state management and transaction processing
- **P2P Networking** — RLPx transport protocol with ECIES handshake and Discv4 node discovery
- **JSON-RPC & Engine API** — Partially implemented `eth_*`, `engine_*`, `admin_*`, `net_*`, `web3_*`, and `debug_*` namespaces for consensus client integration (e.g., Lighthouse)
- **Mempool & Gossip** — Transaction pool management with P2P propagation
- **Chain Synchronization** — Block-by-block sync with downloader, processor, and orchestrator
- **Reorg Handling** — Automatic detection and handling of chain reorganizations
- **Persistent Storage** — `redb` key-value store with codecs, trie implementation, and read/write providers
- **WASIX Portability** — Runs natively or compiled to WebAssembly via WASIX for cross-platform deployment
- **Metrics & Observability** — Prometheus metrics collection and structured logging

## Quick Start

### Prerequisites

- **Rust** (stable or nightly) — [rustup.rs](https://rustup.rs)
- **Wasmer** — for running WASIX binaries
- **WASIX toolchain** (`cargo-wasix`, `wasi-sdk`, `clang` + `llvm`) — see [DEVELOPMENT.md](docs/DEVELOPMENT.md) for details

### Build

**Native build:**

```bash
cd wasix_eth
cargo build --release
```

**WASIX build:**

```bash
cd wasix_eth
cargo wasix build --release
```

### Run

**Native execution:**

```bash
cargo run --release -- --help
```

**WASIX execution:**

```bash
wasmer run --release target/wasm32-wasmer-wasi/release/wasix_eth.wasm -- --help
```

**Run a local dev node:**

```bash
cargo run --release -- --dev
```

See [DEVELOPMENT.md](docs/DEVELOPMENT.md) for detailed instructions on running with a custom genesis, integrating with a consensus client (Lighthouse), and debugging.

## Architecture

The node follows a layered architecture:

```
CLI (wasix_eth_app)
  → Node (storage + execution + network + sync)
    → Core (Engine, ChainManager, Mempool, Gossip, Sync)
      → Execution (EVM Backend, State, Transaction)
      → Storage (redb-backed Database)
    → P2P (Discv4 Discovery, RLPx Transport, PeerManager)
    → RPC (eth, engine, admin, net, web3, debug)
```

### Workspace Crates

| Crate | Purpose |
|---|---|
| `wasix_eth_app` | CLI application binary; entry point, node lifecycle, RPC server setup, bootstrap registry integration |
| `wasix_eth_core` | Core protocol logic: `Engine` (payload processing, forkchoice, canonicality tracking, reorgs), `ChainManager`, `Mempool`, `Gossip`, `Sync` (downloader, processor, orchestrator), `Consensus` |
| `wasix_eth_execution` | EVM execution layer: `Executor`, `Backend`, `State`, block/transaction execution, config |
| `wasix_eth_p2p` | P2P networking: Discv4 discovery, RLPx transport (handshake, crypto, framing), `PeerManager`, `SyncService` |
| `wasix_eth_rpc` | JSON-RPC server: `eth_*`, `engine_*`, `admin_*`, `net_*`, `web3_*`, `debug_*` namespaces via `RpcServerFacade` |
| `wasix_eth_storage` | Persistent storage: `EthDatabase` (backed by `redb` key-value store), read/write providers, codecs, tables, trie |
| `wasix_eth_types` | Shared types and traits: `ChainManager`, `SyncProvider`, genesis config, engine types, RPC API definitions, re-exports from `alloy-*` |
| `wasix_eth_utils` | Utilities: logging, metrics (Prometheus), block/transaction/engine mappers, identity |

### Additional Components

| Directory | Purpose |
|---|---|
| `metrics_server/` | Standalone Rust service for Prometheus metrics collection, node registration, and bootstrap peer discovery |
| `experiments/` | Modular runtime, empty-block, startup, Hive compliance, and size experiments; start with `bash experiments/run.sh` |
| `scripts/v2/` | Legacy low-level deployment, workload, and analysis utilities referenced by the modular experiment suite |
| `hive/` | Custom HIVE test suite Docker setup for Ethereum engine client testing |
| `diablo_workloads/` | Solidity contracts (counter, dota, uber) and workload YAML configs for benchmarking |
| `thesis_results/` | Benchmark result PDFs and failure reports (comparing wasix, rust, and wasm builds) |

## CI & Hive Testing

Every push to `dev` triggers a sequential CI pipeline:

1. **[Setup Environment](.github/workflows/setup.yml)** — Checks for existing build tools (Rust, `wasixcc`, `cargo-wasix`, `lld`) and installs only what's missing. Verifies all tools are correctly configured, then triggers the next stage.

2. **[Build & Test Native](.github/workflows/build-test-native.yml)** — Compiles the native Linux binary (`cargo build --release`), builds the Docker image (`z4l3s5i0/wasix-eth-linux:latest`), then runs the Hive Ethereum Engine API simulation test suite against the `wasix-r-eth` client using a custom Hive runner (`hive/hive.dockerfile`). Tests execute with `--docker.pull=false` so the freshly built local image is used directly. After completion, triggers the WASIX stage.

3. **[Build & Test WASIX](.github/workflows/build-test-wasix.yml)** — Sets up the full WASIX toolchain, compiles the WASIX binary (`cargo wasix build --release`), builds the Docker image (`z4l3s5i0/wasix-eth-wasix:latest`), then runs the Hive Ethereum Engine API simulation test suite against the `wasix-w-eth` client. Tests execute with `--docker.pull=false` so the freshly built local image is used directly. After completion, triggers cleanup.

4. **[Cleanup](.github/workflows/cleanup.yml)** — Removes Docker images and workspace logs to free disk space.

All workflows upload test logs as artifacts for inspection.

### Test Results

| Runtime | Client | Tests Passed | Tests Failed | Total Tests |
|---|---|---|---|---|
| Native | wasix-r-eth | — | — | — |
| WASIX | wasix-w-eth | — | — | — |

*Test results are updated after each CI run. See the workflow run logs for detailed per-test results.*

## Documentation

### Core Documentation

| Document | Description |
|---|---|
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | Internal design: crate dependencies, data flows, Engine API, sync, reorg handling |
| [DEVELOPMENT.md](docs/DEVELOPMENT.md) | Developer guide: prerequisites, building, running, testing, debugging |
| [NETWORKING.md](docs/NETWORKING.md) | P2P stack: Discv4, RLPx transport, peer management, gossip |
| [experiments/README.md](experiments/README.md) | Primary modular experiment workflow, metric matrix, implementation status, and known gaps |
| [EXPERIMENTS.md](docs/EXPERIMENTS.md) | Legacy script reference and link to the current modular workflow |
| [RPC_API.md](docs/RPC_API.md) | JSON-RPC API reference: all supported endpoints |
| [STORAGE.md](docs/STORAGE.md) | Storage layer: `redb` schema, codecs, providers, trie |

### Existing Documentation

| Document | Description |
|---|---|
| [md/RELEASE_v0.1.0.md](md/RELEASE_v0.1.0.md) | Release notes for v0.1.0 |
| [md/RELEASE_v0.2.0.md](md/RELEASE_v0.2.0.md) | Release notes for v0.2.0 |
| [md/USER_GUIDE.md](md/USER_GUIDE.md) | Multi-node deployment guide |
| [md/TODO.md](md/TODO.md) | Project TODO list |

## License

[Apache License 2.0](LICENSE)

## Author

Z4l3s5i0
