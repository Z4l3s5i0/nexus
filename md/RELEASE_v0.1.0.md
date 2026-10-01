# Release Statement v0.1.0: WASIX-Based Ethereum Execution Client

## Introduction
This is the first release (v0.1.0) of the **WASIX-Based Ethereum Execution Client**. This project aims to provide a fully functional Ethereum execution client that runs entirely within a WebAssembly (Wasm) environment leveraging **WASIX** (WASI eXtended). By utilizing WASIX, this client benefits from POSIX-like capabilities such as multi-threading and networking, which are traditionally difficult in standard Wasm environments, while maintaining the portability and security of WebAssembly.

---

## Current Progress (What is Done)
The client currently implements the foundational components required for an execution client:

- **EVM Execution**: Full EVM implementation using a custom backend compatible with WASIX.
- **Transient Storage**: Using WASIX filesystem for a transient storage layer, allowing for persistent storage via a mapped volume.
- **JSON-RPC API**: A significant subset of the standard Ethereum JSON-RPC API, including:
  - `eth_*` (Accounts, Gas Price, Syncing, Sending Transactions, Calls, Gas Estimation).
  - `engine_*` (Engine API for Consensus Client integration).
- **P2P Networking (Prototype)**: Basic P2P infrastructure including Peer Discovery and Gossip protocols (implemented via JSON-RPC/HTTP due to WASIX networking constraints).
- **Mempool**: Transaction mempool with prioritization logic.
- **Multi-threading**: Enabled via WASIX threads, allowing concurrent RPC handling and background processing.

---

## Missing Features (Roadmap to v1.0.0)
To reach full production readiness as a standard Ethereum execution client, the following components are still in development:

- **Full DevP2P Support**: Standard RLPx and Discv5 protocols are currently proxied or simplified.
- **Comprehensive Syncing**: Implementation of Snap Sync and Fast Sync mechanisms to synchronize with existing networks from scratch.
- **Advanced State Management**: Transitioning from a basic key-value store to a full Merkle Patricia Trie (MPT) with state pruning.
- **Consensus Client Interoperability**: While the Engine API is present, extensive testing with clients like Lighthouse is still missing.
- **Full EIP Compliance**: Continuous updates to support the latest Ethereum hard forks (e.g., Cancun, Prague).
- **Frontend**: Integrated web frontend for logs and real-time monitoring of the node's state.
- **Persistent Storage**: Disk-backed storage that survives process restarts (using WASIX filesystem volumes).

---

## Installation & Running


### Prerequisites for Building
1.  **Rust**: Installed via [rustup](https://rustup.rs/).
2.  **Wasmer CLI**: The recommended runner for WASIX. Installed via [WASMER](https://github.com/wasmerio/wasmer-install)
3.  **WASIX** Installed via [WASIX](https://github.com/wasix-org/cargo-wasix)
4.  **WASI-SDK**: Installed via [wasi-sdk](https://github.com/WebAssembly/wasi-sdk).
5.  **Clang+LLVM**: Installed via [Clang+LLVM](https://releases.llvm.org/)

### Build
From the `wasix-based-evm` directory:
```bash
cargo wasix build --release 
```

### Prerequisites for Running
1.  **Wasmer CLI**: The recommended runner for WASIX. Installed via [WASMER](https://github.com/wasmerio/wasmer-install)
2.  **OUR Release**: The latest release of the client.

### Run
The easiest way to run the client is using the provided `wasmer.toml` configuration:
```bash
wasmer run . --enable-threads --net --volume <host-startup>:<guest-startup> -- [FLAGS]
```

Alternatively, you can run the compiled `.wasm` file directly from the root of the project:
```bash
wasmer run target/wasm32-wasmer-wasi/release/wasix-based-evm.wasi.wasm --enable-threads --net --volume <host-startup>:<guest-startup> -- [FLAGS]
```
By default the volume is set as ./data-dir:./data-dir

---

## Command Line Flags

The client supports the following flags for configuration:

| Flag | Description                                               | Default |
| :--- |:----------------------------------------------------------| :--- |
| `--discovery-port` | TCP port for peer discovery                               | `9001` |
| `--p2p-port` | TCP port for P2P communication (gossip, blocks)           | `9002` |
| `--eth-rpc-port` | TCP port for Standard Ethereum JSON-RPC                   | `8545` |
| `--auth-rpc-port` | TCP port for Auth Engine JSON-RPC                         | `8551` |
| `--frontend-port` | TCP port for the integrated web dashboard                 | `3000` |
| `--bootnodes` | Comma-separated list of Multiaddrs for bootstrapping (fromat < ip-address > : < discovery-port > )     | - |
| `--max-peers` | Maximum number of concurrent P2P connections              | `50` |
| `--data-dir` | Path for persistent storage (mapped to a WASIX volume, must be same as for wasmer guest data-dir). Genesis file, to be put in data-dir/genesis/genesis.json  | `data-dir` |
| `--chain` | Chain name                                                | `devnet` |
| `--verbose` | Verbosity level (0: none, 1: info, 2: debug)              | `1` |
| `--ext-ip` | Outwards facing ip address. If not provided, inward facing ip address is used (127.0.0.1) | `-` |
| `--dev` | Enable dev mode with automatic block production (seconds) | `12` |
