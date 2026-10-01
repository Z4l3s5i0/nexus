# Release Statement v0.2.0: WASIX-Based Ethereum Execution Client

## Introduction
This is the second release (v0.2.0) of the **WASIX-Based Ethereum Execution Client**. Building on the foundations of v0.1.0, this release introduces significant improvements in consensus compatibility, configuration flexibility, and monitoring. The client continues to leverage **WASIX** (WASI eXtended) to provide a portable, secure, and multi-threaded Ethereum execution environment.

---

## What's New in v0.2.0

### Consensus & Hardfork Compliance
- **Shanghai-Capella Compatibility**: The execution client is now fully compliant with the **Lighthouse** consensus client for the **Shanghai-Capella** hardfork.
- **Withdrawals Support**: Implementation of withdrawals as per the Capella specifications.
- **Improved Genesis Support**: Genesis files are now generated using [ethereum-genesis-generator](https://github.com/eth-clients/ethereum-genesis-generator), ensuring better alignment with standard Ethereum testnets. (You can use the [`values.env`](./generate/values.env) file for reference)
- **Validator Integration**: Validators are now generated and managed using [ethstaker-deposit-cli](https://github.com/eth-educators/ethstaker-deposit-cli).

### Core Improvements & Bug Fixes
- **EVM Security**: Fixed a critical bug related to EVM double spending.
- **State Management**: Added **Memory Dump** functionality, allowing the node to save its state and load it upon restart for faster recovery.
- **Block Processing**: Improved logic for calculating block values and rewards.
- **Logging**: Enhanced observability with detailed debugging logs and structured **chrono-based** timestamps.

### Architecture & RPC
- **Refactoring**: Significant structural refactoring to adhere to the **Single Responsibility Principle (SRP)**, resulting in cleaner and more maintainable code.
- **Consolidated RPC**: All RPC methods have been moved into a unified handler for better consistency and performance.
- **Compliance**: Enhanced RPC error handling to be more compliant with Ethereum standards.
- **Security**: Implemented **JWT (JSON Web Token) checks** for the Engine API to ensure secure communication with the consensus client.

### CLI & Configuration
- **External IP**: Added the ability to manually set the external IP address via `--ext-ip`.
- **New Flags**: Several new CLI flags have been added to improve node management and interoperability:
    - `--genesis-path`: Directly specify the path to the genesis JSON file.
    - `--auth-rpc-jwt-path`: Path to the JWT secret for the Engine API (Auth Engine JSON-RPC).
    - `--peer-name`: A descriptive name for the node, used for identifying storage and logs.


---

## Example Node Run

### 1. Run Execution Client (WASIX)
```bash
wasmer run wasix-based-evm.wasi.wasm --enable-threads --net --volume ./startup:./startup -- --data-dir ./startup --ext-ip 192.168.1.152 --verbose 1 --genesis-path ./startup/genesis.json --auth-rpc-jwt-path ./startup/jwt_node1.hex --peer-name node1
```

### 2. Run Beacon Node (Lighthouse)
```bash
lighthouse bn --execution-endpoint http://192.168.1.152:8551 --execution-jwt ./startup/jwt_node1.hex --http --http-address 0.0.0.0 --http-port 5052 --testnet-dir ./startup --debug-level debug --enr-address 192.168.1.152 --enr-tcp-port 9005 --enr-udp-port 9005 --datadir /home/wasm/.lighthouse/node1
```

### 3. Import Validators and Run
**Import:**
```bash
lighthouse account validator import --directory startup/validator_keys/ --testnet-dir ./startup --datadir /home/wasm/.lighthouse/node1
```

**Run Validator Client:**
```bash
lighthouse vc --beacon-nodes http://192.168.1.152:5052 --testnet-dir ./startup --datadir /home/wasm/.lighthouse/node1 --debug-level debug --suggested-fee-recipient 0x0000000000000000000000000000000000000000
```

---

## Command Line Flags

The client supports the following flags for configuration:

| Flag | Description | Default |
| :--- | :--- | :--- |
| `--discovery-port` | TCP port for peer discovery | `9001` |
| `--p2p-port` | TCP port for P2P communication (gossip, blocks) | `9002` |
| `--eth-rpc-port` | TCP port for Standard Ethereum JSON-RPC | `8545` |
| `--auth-rpc-port` | TCP port for Auth Engine JSON-RPC | `8551` |
| `--frontend-port` | TCP port for the integrated web dashboard | `3000` |
| `--bootnodes` | Comma-separated list of Multiaddrs for bootstrapping | - |
| `--max-peers` | Maximum number of concurrent P2P connections | `50` |
| `--data-dir` | Path for persistent storage (mapped to a WASIX volume) | `data-dir` |
| `--genesis-path` | **(New)** Path to the genesis JSON file | - |
| `--chain` | Chain name (mainnet, sepolia, devnet) | `devnet` |
| `--auth-rpc-jwt-path` | **(New)** Path to the JWT secret for the Auth Engine JSON-RPC | - |
| `--verbose` | Verbosity level (0: none, 1: info, 2: debug) | `1` |
| `--ext-ip` | Outwards facing IP address | `127.0.0.1` |
| `--dev` | Enable dev mode with automatic block production (seconds) | `12` |
| `--peer-name` | **(New)** Descriptive name for the node | - |

---

## Installation & Running

### Build
From the `wasix-based-evm` directory:
```bash
cargo wasix build --release 
```

### Run
The easiest way to run the client is using the provided `wasmer.toml` configuration:
```bash
wasmer run . --enable-threads --net --volume <host-startup>:<guest-startup> -- [FLAGS]
```

---

## Immediate Next Todos (Roadmap)
The following features and improvements are planned for the immediate future:
- **Persistent Storage**: Robust, disk-backed storage solutions for long-term data retention.
- **Deployment Script**: Streamlining the setup and deployment process for new nodes.
- **Monitoring**: Monitoring integration with visualization tools.
- **Automated Testing**: Implementation of a comprehensive test suite using standard Ethereum testing tools.
