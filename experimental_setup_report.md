# NEXUS Experimental Setup — Comprehensive Report

> **Project:** NEXUS (wasix_eth) — An Ethereum execution client written in Rust, running natively or inside a WebAssembly (WASIX) environment.
> **Report Date:** 2026-11-23
>
> **Status note:** This is a pre-modular implementation report. The current supported experiment workflow and script inventory are documented in [`experiments/README.md`](experiments/README.md); the network launcher, configuration, shared helpers, monitoring files, and Compose template have since moved to `experiments/shared/network/`.

---

## Table of Contents

1. [Project Overview](#1-project-overview)
2. [System Architecture](#2-system-architecture)
3. [Experimental Framework](#3-experimental-framework)
4. [Infrastructure & Network Deployment](#4-infrastructure--network-deployment)
5. [Workload Definitions](#5-workload-definitions)
6. [Metrics Collection Pipeline](#6-metrics-collection-pipeline)
7. [Metrics Gathered](#7-metrics-gathered)
8. [Log-Based Experiment Events](#8-log-based-experiment-events)
9. [Analysis & Evaluation](#9-analysis--evaluation)
10. [Hive Test Suite](#10-hive-test-suite)
11. [Additional Experiments](#11-additional-experiments)
12. [Thesis Results](#12-thesis-results)
13. [File Inventory](#13-file-inventory)

---

## 1. Project Overview

NEXUS is an independent Ethereum execution client implemented in Rust with a unique dual-target design: it runs both as a **native Rust binary** and as a **WASIX WebAssembly module**. This makes it the only Ethereum client among the three major implementations (Geth, Reth, Nexus) that targets WebAssembly environments.

### Core Capabilities
- **EVM Execution** — Full Ethereum Virtual Machine via the `evm` crate (SputnikVM)
- **P2P Networking** — Custom RLPx transport with ECIES handshake and Discv4 node discovery
- **JSON-RPC & Engine API** — `eth_*`, `engine_*`, `admin_*`, `net_*`, `web3_*`, `debug_*` namespaces
- **Mempool & Gossip** — Transaction pool with P2P propagation and optimistic transaction inclusion
- **Chain Synchronization** — Event-driven sync orchestrator with downloader, processor, and registry
- **Reorg Handling** — Change-set-based revert with fork switch classification (synchronous vs background)
- **Persistent Storage** — `redb` embedded B+tree key-value store (pure Rust, no C dependencies)
- **Budget-Aware Payload Building** — Continuous background building with EWMA-based cost model

### Workspace Crates

| Crate | File Path | Purpose |
|---|---|---|
| `wasix_eth_app` | `wasix_eth/wasix_eth_app/` | CLI binary; entry point, node lifecycle, RPC server, bootstrap registry |
| `wasix_eth_core` | `wasix_eth/wasix_eth_core/` | Engine API, ChainManager, Mempool, Gossip, Sync, Consensus |
| `wasix_eth_execution` | `wasix_eth/wasix_eth_execution/` | EVM execution: Executor, Backend, State, block/transaction execution |
| `wasix_eth_p2p` | `wasix_eth/wasix_eth_p2p/` | Discv4 discovery, RLPx transport, PeerManager, SyncService |
| `wasix_eth_rpc` | `wasix_eth/wasix_eth_rpc/` | JSON-RPC server facade for all namespaces |
| `wasix_eth_storage` | `wasix_eth/wasix_eth_storage/` | EthDatabase (redb-backed), providers, codecs, tables, trie |
| `wasix_eth_types` | `wasix_eth/wasix_eth_types/` | Shared types, traits, genesis config, alloy re-exports |
| `wasix_eth_utils` | `wasix_eth/wasix_eth_utils/` | Logging, metrics (Prometheus), mappers, identity |
| `testing` | `wasix_eth/testing/` | Integration tests, RLP fixtures, genesis config |

---

## 2. System Architecture

```
CLI (wasix_eth_app)
  → Node (storage + execution + network + sync)
    → Core (Engine, ChainManager, Mempool, Gossip, Sync)
      → Execution (EVM Backend, State, Transaction)
      → Storage (redb-backed Database — 21 tables)
    → P2P (Discv4 Discovery, RLPx Transport, PeerManager)
    → RPC (eth, engine, admin, net, web3, debug)
```

### Storage Layer — 21 `redb` Tables

| Category | Tables |
|---|---|
| **Chain Data** | `Headers`, `HeaderTD`, `BlockBodies`, `Transactions`, `Receipts`, `ReceiptsMeta` |
| **Chain Indexes** | `CanonicalHeads`, `HeaderNumbers`, `TransactionLookup` |
| **State** | `Accounts`, `Storages`, `Bytecodes` |
| **State Changes** | `AccountChangeSets`, `StorageChangeSets` |
| **State Indexes** | `TrieNodes`, `TrieNodeRefs` |
| **Metadata** | `Metadata`, `Payloads`, `Forkchoice`, `StateBranches`, `ActivePeers` |

---

## 3. Experimental Framework

The project includes a comprehensive experimentation pipeline for benchmarking NEXUS nodes against each other and against reference implementations (Geth, Reth).

### 3.1 Experiment Orchestration

The experiment framework supports:
- **Multi-node local deployment** — Docker Compose-based networks with mixed node types
- **Automated workload generation** — Contender-based transaction spam and campaign scenarios
- **Metrics collection** — Prometheus + Grafana monitoring via `metrics_server`
- **Log analysis** — Automated log parsing for latency, throughput, and correctness metrics
- **Database export** — `db_exporter` tool for extracting node state for post-hoc analysis
- **Hive integration** — Docker-based Ethereum engine client test suite

### 3.2 Script Categories

The following inventory records the pre-modular script layout:

| Category | Scripts | Purpose |
|---|---|---|
| **Orchestration** | `run_experiment.sh`, `run_experiments_loop.sh` | Single and batch experiment runners |
| **Infrastructure** | `setup.sh`, `start_local.sh`, `cleanup.sh`, `config_gen.sh` | Network deployment and configuration |
| **Workload & Collection** | `run_workload.sh`, `collect_results.sh`, `aggregate_logs.sh`, `manage_logs.sh`, `log_processor.sh` | Workload execution and data collection |
| **Analysis** | `analyze_experiment.sh`, `analyze_all_experiments.sh`, `evaluate_logs.py`, `evaluate.sh`, `plot_metrics.py`, `write_experiment_metadata.py`, `workload_allocation.py` | Result analysis and visualization |
| **Image Push** | `push_contender.sh`, `push_linux.sh`, `push_wasix.sh`, `push_metrics.sh` | Docker image registry push |
| **Utilities** | `common.sh`, `db_exporter/` | Shared functions and database export tool |

### 3.3 Single Experiment Lifecycle (`run_experiment.sh`)

The full lifecycle of one experiment run:

1. **Deploy network** — Calls `start_local.sh` with given node configuration
2. **Start log processor** — Launches `manage_logs.sh` to stream container logs to `logs_dump/`
3. **Wait for stabilization** — 60s for non-Geth/Reth networks, 120s+ per Geth/Reth node
4. **Start workloads** — Launches `run_workload.sh` for each target node
5. **Wait for completion** — Sleeps for `duration` + `wait-duration` seconds
6. **Stop workloads** — Terminates workload processes and Contender containers
7. **Stop log processor** — Flushes and stops the background log stream
8. **Collect results** — Runs `collect_results.sh` to archive all data to `experiments/exp_YYYYMMDD_HHMMSS/`
9. **Clean up** — Calls `cleanup.sh` to stop and remove all containers

### 3.4 Batch Experiment Runner (`run_experiments_loop.sh`)

Runs experiments across a matrix of workload types and node ratio combinations. By default:
- **3 workload types**: `exp/uber`, `exp/counter`, `exp/dota`
- **7 node ratio configurations**: pure Linux, pure WASIX, pure Geth, and various mixes
- **21 total experiment combinations**

### 3.5 Experiment Parameters

| Parameter | Default | Description |
|---|---|---|
| `--linux-nodes N` | 5 | Number of native Rust nodes |
| `--wasix-nodes N` | 5 | Number of WASIX nodes |
| `--geth-nodes N` | 0 | Number of Geth nodes |
| `--reth-nodes N` | 0 | Number of Reth nodes |
| `--workload-type TYPE` | `exp/uber` | Workload scenario |
| `--tps N` | 4 | Transactions per second |
| `--spammer-accounts N` | 4 | Accounts per spammer |
| `--duration N` | 300 | Workload duration in seconds |
| `--wait-duration N` | 1800 | Additional wait after workload |
| `--workload-percent N` | 100 | Percent of nodes receiving workload |
| `--chain-id ID` | 10 | Unique chain ID |

---

## 4. Infrastructure & Network Deployment

### 4.1 Network Deployment (`start_local.sh`)

Deploys a Docker Compose network with mixed node types. Supports 4 node types:

| Node Type | Image | Description |
|---|---|---|
| **Linux (Native)** | `wasix-eth-linux:latest` | NEXUS compiled as native Rust binary |
| **WASIX** | `wasix-eth-wasix:latest` | NEXUS compiled to WASIX WebAssembly |
| **Geth** | `ethereum/client-go` (pinned) | Reference Go implementation |
| **Reth** | `ghcr.io/paradigmxyz/reth:latest` | Reference Rust implementation |

### 4.2 Network Topology

- Nodes are named `el-node-0`, `el-node-1`, … with incrementing ports
- **RPC ports**: 8545+ (per node)
- **Auth RPC ports**: 8551+ (per node)
- **P2P ports**: 9000+ (per node)
- **Discovery ports**: 30303+ (per node)
- **Metrics ports**: 9090+ (per node)
- Each node has a paired **Consensus Client** (Lighthouse beacon node: `cl-node-N`)
- Selected nodes have a **Validator Client** (`vc-node-N`)

### 4.3 Monitoring Stack

| Component | File | Purpose |
|---|---|---|
| **Prometheus** | `scripts/v2/monitoring/prometheus.yml` | Scrapes `/metrics` from all nodes and metrics server |
| **Grafana** | `scripts/v2/monitoring/grafana/` | Dashboards and provisioning for real-time visualization |
| **Node Exporter** | Built into each node container | System-level metrics (CPU, memory, disk) |

### 4.4 Docker Compose Template

Template file: `scripts/v2/templates/docker-compose.yml.template`
Generated file: `docker-compose.v2.yml`

Services deployed:
- `metrics-server` — Central metrics collection and bootstrap registry
- `prometheus` — Time-series metrics storage
- `grafana` — Visualization dashboards
- `node-exporter` — Host-level metrics
- `el-node-0` … `el-node-N` — Execution client nodes
- `cl-node-0` … `cl-node-N` — Consensus client (Lighthouse) nodes
- `vc-node-0` … `vc-node-N` — Validator client nodes (for validator nodes)

### 4.5 Configuration Generation (`config_gen.sh`)

Generates node TOML configuration files for a given chain ID and node count, including:
- Genesis configuration
- JWT secrets for each node
- Validator keys and passwords
- Network parameters

---

## 5. Workload Definitions

Workloads are generated using **Contender** (`docker.io/z4l3s5i0/contender:latest`), a Docker-based transaction workload generator.

### 5.1 Diablo Workload Contracts

Three Solidity contracts with increasing computational complexity:

#### Counter (`diablo_workloads/counter.sol`)
- **Complexity:** Low — Simple state increment
- **Function:** `Add()` — Increments a private counter by 1
- **Purpose:** Measures basic transaction throughput and block production latency with minimal computation

#### Dota (`diablo_workloads/dota.sol`)
- **Complexity:** Medium — Array iteration with boundary checks
- **State:** 10 players with X/Y position arrays (maxlen = 250)
- **Function:** `update()` — Iterates all players, increments positions, applies boundary wrapping
- **Purpose:** Measures performance with array reads/writes and loop-based computation

#### Uber (`diablo_workloads/uber.sol`)
- **Complexity:** High — Distance calculation with iterative square root
- **State:** 10,000 drivers with X/Y position arrays (maxlen = 10,000)
- **Function:** `checkDistance()` — Iterates all drivers, computes Euclidean distance using iterative square root, finds closest driver
- **Purpose:** Measures performance under heavy computation: large array iteration, arithmetic operations, and iterative square root

### 5.2 Workload YAML Configurations

| File | Description |
|---|---|
| `diablo_workloads/other-workload-dota.yaml` | Diablo workload config for Dota contract |
| `diablo_workloads/other-workload-uber.yaml` | Diablo workload config for Uber contract (10,000 accounts, 10 geographic locations, time-varying TPS) |
| `diablo_workloads/other-workload-football.yaml` | Diablo workload config for Football variant |

### 5.3 Workload Execution (`run_workload.sh`)

Two-phase execution:
1. **Setup Phase** — Deploys contracts, funds accounts via Contender `setup` command
2. **Execution Phase** — Spams transactions at specified TPS via Contender `spam` command

Key parameters:
- `--rpc` — Target RPC endpoint
- `--pk` — Private key for transaction signing
- `--scenario` — Scenario name (e.g., `exp/uber`, `exp/counter`, `exp/dota`)
- `--tps` — Transactions per second
- `--duration` — Duration in seconds
- `--accounts` — Number of accounts per agent
- `--seed` — Deterministic seed for reproducibility
- `--network` — Docker network to join
- `--state-dir` — Directory for Contender state persistence

---

## 6. Metrics Collection Pipeline

### 6.1 Metrics Server (`metrics_server/`)

A standalone Rust service that provides centralized metrics collection, node registration, and bootstrap peer discovery.

**Key Files:**

| File | Purpose |
|---|---|
| `metrics_server/src/main.rs` | Entry point |
| `metrics_server/src/app.rs` | Application setup and lifecycle |
| `metrics_server/src/config.rs` | Configuration parsing |
| `metrics_server/src/model.rs` | Data models (Node, MetricSample, RpcObservation, Experiment) |
| `metrics_server/src/cli.rs` | CLI argument parsing |
| `metrics_server/src/api/handlers.rs` | HTTP request handlers |
| `metrics_server/src/api/routes.rs` | Route definitions |
| `metrics_server/src/collector/rpc_collector.rs` | JSON-RPC metrics collection |
| `metrics_server/src/collector/prometheus_scraper.rs` | Prometheus endpoint scraping |
| `metrics_server/src/collector/scheduler.rs` | Collection scheduling |
| `metrics_server/src/storage/memory.rs` | In-memory storage backend |
| `metrics_server/src/storage/redb_store.rs` | `redb` persistent storage backend |
| `metrics_server/src/telemetry/registry.rs` | Prometheus metric registry (22 metrics) |
| `metrics_server/src/ethereum/rpc_client.rs` | Ethereum JSON-RPC client |
| `metrics_server/src/ethereum/types.rs` | Ethereum type parsing |
| `metrics_server/src/bootstrap/prober.rs` | Bootstrap peer health probing |
| `metrics_server/src/time.rs` | Time utilities |
| `metrics_server/src/error.rs` | Error types |

**Configuration Files:**

| File | Purpose |
|---|---|
| `metrics_server/config/example.toml` | Example configuration |
| `metrics_server/config/docker.toml` | Docker-specific configuration |
| `metrics_server/config/empty_nodes.toml` | Empty node configuration |

**Storage:**
- Default: `metrics_server/data/metrics.redb` (redb embedded database)
- Alternative: In-memory storage (development)

### 6.2 Metrics Collection Methods

The metrics server collects data through two channels:

#### JSON-RPC Collection (`rpc_collector.rs`)
Queries 6 RPC methods per node at configurable intervals:
1. `eth_chainId` → `ethereum_chain_id`
2. `eth_blockNumber` → `ethereum_latest_block`
3. `eth_syncing` → `ethereum_syncing`
4. `eth_gasPrice` → `ethereum_gas_price`
5. `net_peerCount` → `ethereum_peer_count`
6. `web3_clientVersion` → (logged as observation)

Each RPC call records an `RpcObservation` with:
- Timestamp, experiment ID, node ID, network, client
- Method, success/failure, latency (ms), value, error

#### Prometheus Scraping (`prometheus_scraper.rs`)
Scrapes `/metrics` endpoints from nodes to collect the node-level Prometheus metrics defined in `wasix_eth_utils/src/metrics.rs`.

### 6.3 Metric Sources

| Source | Description |
|---|---|
| `JsonRpc` | Metrics derived from JSON-RPC queries |
| `PrometheusScrape` | Metrics scraped from node `/metrics` endpoints |
| `Derived` | Computed metrics (e.g., deltas, averages) |
| `Internal` | Metrics server's own operational metrics |

### 6.4 API Endpoints

| Endpoint | Method | Description |
|---|---|---|
| `/health` | GET | Health check |
| `/metrics` | GET | Prometheus metrics endpoint |
| `/api/nodes` | GET | List configured nodes |
| `/api/nodes/register` | POST | Dynamic node registration |
| `/api/bootstrap/nodes` | GET | Get healthy bootstrap peers |
| `/api/experiments` | GET | List historical experiments |
| `/api/metrics/latest` | GET | Latest metrics for all nodes |
| `/api/metrics/range` | GET | Query metric historical range |

---

## 7. Metrics Gathered

### 7.1 Node-Level Prometheus Metrics (from `wasix_eth_utils/src/metrics.rs`)

12 metrics exposed by each NEXUS node:

| Metric | Type | Description |
|---|---|---|
| `sync_status` | Gauge | Sync status (1: synced, 0: syncing, -1: stalled) |
| `current_head_block` | Gauge | Current head block height |
| `connected_peers` | Gauge | Number of currently connected peers |
| `blocks_imported_total` | Gauge | Total number of blocks successfully imported |
| `transactions_committed_total` | Gauge | Total number of successfully committed transactions |
| `mempool_size` | Gauge | Current number of transactions in the mempool |
| `mempool_rejected_transactions_total` | Counter | Total transactions rejected by the mempool |
| `evicted_transactions_total` | Counter | Total transactions evicted from the mempool |
| `gossip_messages_received_total` | Counter | Total gossip messages received |
| `p2p_messages_sent_bytes_total` | Counter | Total bytes sent over the P2P network |
| `rpc_requests_total` | Counter | Total number of RPC requests received |
| `sync_target_height` | Gauge | Target block height the node is syncing towards |

### 7.2 Metrics Server Telemetry (from `telemetry/registry.rs`)

22 Prometheus metrics exposed by the metrics server:

#### Collection Internals
| Metric | Type | Labels | Description |
|---|---|---|---|
| `metrics_server_collection_ticks_total` | Counter | — | Total collection cycles completed |
| `metrics_server_collection_errors_total` | Counter | `node_id`, `source` | Total collection errors |

#### RPC Metrics
| Metric | Type | Labels | Description |
|---|---|---|---|
| `metrics_server_rpc_requests_total` | Counter | `node_id`, `method` | Total RPC requests from metrics server |
| `metrics_server_rpc_errors_total` | Counter | `node_id`, `method` | Total RPC errors |
| `metrics_server_rpc_latency_ms` | Gauge | `node_id`, `method` | RPC latency in milliseconds |

#### Ethereum State (RPC-Derived)
| Metric | Type | Labels | Description |
|---|---|---|---|
| `ethereum_rpc_up` | Gauge | `node_id`, `network`, `client` | Node RPC up status (0/1) |
| `ethereum_latest_block` | Gauge | `node_id`, `network`, `client` | Latest block number |
| `ethereum_chain_id` | Gauge | `node_id`, `network`, `client` | Chain ID |
| `ethereum_peer_count` | Gauge | `node_id`, `network`, `client` | Peer count |
| `ethereum_gas_price` | Gauge | `node_id`, `network`, `client` | Gas price |
| `ethereum_syncing` | Gauge | `node_id`, `network`, `client` | Syncing status |

#### Node-Level Metrics (Prometheus-Scraped)
| Metric | Type | Labels | Description |
|---|---|---|---|
| `sync_status` | Gauge | `node_id`, `network`, `client` | Sync status |
| `current_head_block` | Gauge | `node_id`, `network`, `client` | Head block height |
| `connected_peers` | Gauge | `node_id`, `network`, `client` | Connected peers |
| `blocks_imported_total` | Counter | `node_id`, `network`, `client` | Blocks imported |
| `transactions_committed_total` | Counter | `node_id`, `network`, `client` | Transactions committed |
| `mempool_size` | Gauge | `node_id`, `network`, `client` | Mempool size |
| `mempool_rejected_transactions_total` | Counter | `node_id`, `network`, `client` | Rejected transactions |
| `evicted_transactions_total` | Counter | `node_id`, `network`, `client` | Evicted transactions |
| `gossip_messages_received_total` | Counter | `node_id`, `network`, `client` | Gossip messages |
| `p2p_messages_sent_bytes_total` | Counter | `node_id`, `network`, `client` | P2P bytes sent |
| `rpc_requests_total` | Counter | `node_id`, `network`, `client` | RPC requests |
| `sync_target_height` | Gauge | `node_id`, `network`, `client` | Sync target height |

#### Bootstrap Registry
| Metric | Type | Labels | Description |
|---|---|---|---|
| `metrics_server_bootstrap_nodes` | Gauge | `status`, `network` | Number of bootstrap nodes |
| `metrics_server_bootstrap_registration_requests_total` | Counter | `result` | Registration requests |
| `metrics_server_bootstrap_probe_total` | Counter | `result` | Bootstrap probes |

### 7.3 Block Execution Time & Transaction Latency

Two critical performance metrics are explicitly tracked across all experiments:

#### Block Execution Time

Block execution time measures the wall-clock duration from the start of block processing to its completion (before persistence). It is captured by the `[EXP] BLOCK_EXEC_END` log event field `elapsed_ms`.

| Metric | Source | Description |
|---|---|---|
| **Avg block exec time** | Log: `BLOCK_EXEC_END.elapsed_ms` | Mean block processing time |
| **P50 block exec time** | Log: `BLOCK_EXEC_END.elapsed_ms` | Median block processing time |
| **P95 block exec time** | Log: `BLOCK_EXEC_END.elapsed_ms` | 95th percentile block processing time |
| **P99 block exec time** | Log: `BLOCK_EXEC_END.elapsed_ms` | 99th percentile block processing time |
| **Max block exec time** | Log: `BLOCK_EXEC_END.elapsed_ms` | Worst-case block processing time |

Block execution time varies by:
- Number of transactions in the block
- Computational complexity of transactions (Counter < Dota < Uber)
- Client implementation (Geth vs Reth vs Nexus native vs Nexus WASIX)
- Node resource contention (CPU, memory, disk I/O)

#### Transaction Latency

Transaction latency measures the end-to-end time from transaction submission to inclusion in a mined block. It is computed from log events:

| Metric | Calculation | Description |
|---|---|---|
| **Inclusion latency** | `TX_RPC_RECV` / `P2P_RECV_TX` → first `BLOCK_EXEC_END` containing that tx | Time from receipt to block inclusion |
| **Execution latency** | `TX_EXEC_START` → `TX_EXEC_END` | EVM execution time per transaction |
| **P2P propagation latency** | First `P2P_RECV_TX` → subsequent `P2P_RECV_TX` for same tx hash | Time for tx to propagate across nodes |

Transaction latency is reported as:
- Average, median, P95, P99 per workload type
- Broken down by client type (native vs WASIX vs Geth vs Reth)
- Correlated with mempool size and network congestion

### 7.4 Node Exporter System Metrics

Prometheus Node Exporter provides host-level and container-level system metrics. Each NEXUS node container runs an embedded `prometheus-node-exporter` on port 9100, and a standalone Node Exporter service monitors the host.

**Key metrics used for analysis:**

| Category | Metric | Purpose |
|---|---|---|
| **CPU utilization** | `node_cpu_seconds_total` | Overall CPU usage per node |
| **CPU load** | `node_load1`, `node_load5`, `node_load15` | System load averages |
| **Memory usage** | `node_memory_MemAvailable_bytes` / `node_memory_MemTotal_bytes` | Memory pressure |
| **Cache hit/miss** | `node_memory_Cached_bytes` | Page cache effectiveness |
| **Disk I/O** | `node_disk_read_bytes_total`, `node_disk_written_bytes_total` | Storage throughput |
| **Disk I/O wait** | `node_disk_io_time_seconds_total` | I/O bottleneck detection |
| **Network throughput** | `node_network_receive_bytes_total`, `node_network_transmit_bytes_total` | P2P bandwidth |
| **Network drops** | `node_network_receive_drop_total`, `node_network_transmit_drop_total` | Packet loss |
| **Filesystem** | `node_filesystem_avail_bytes` | Disk space monitoring |
| **Processes** | `node_procs_running`, `node_procs_blocked` | Process contention |

### 7.5 dstat Complementary Metrics

`dstat` provides real-time, human-readable system resource statistics as a complementary monitoring tool alongside Node Exporter.

| Category | Metrics |
|---|---|
| **CPU** | `%usr`, `%sys`, `%idl`, `%wai`, `%hiq`, `%siq` |
| **Memory** | `used`, `free`, `buff`, `cache` |
| **Disk** | `read`, `writ` (KB/s) |
| **Network** | `recv`, `send` (KB/s) |
| **System** | `intr`, `cswt`, `running`, `sleeping`, `thr` |

### 7.6 Stored Data Models

#### MetricSample
- `timestamp_ms`, `experiment_id`, `node_id`, `network`, `client`
- `source` (JsonRpc/PrometheusScrape/Derived/Internal)
- `name`, `value`, `labels_json`

#### RpcObservation
- `timestamp_ms`, `experiment_id`, `node_id`, `network`, `client`
- `method`, `success`, `latency_ms`, `value_json`, `error`

---

## 8. Log-Based Experiment Events

NEXUS nodes emit structured `[EXP]` tagged log events when run with `--verbose 3`. These are parsed by `evaluate_logs.py` to compute additional metrics.

### 8.1 Event Types (from `wasix_eth/EXP_LOGS_README.md`)

#### Transaction Execution
| Event | Fields | Description |
|---|---|---|
| `TX_EXEC_START` | `hash`, `sender`, `nonce` | Transaction begins EVM execution |
| `TX_EXEC_END` | `hash`, `success`, `gas_used`, `cumulative_gas`, `elapsed_ms` | Transaction execution complete |

#### Block Execution
| Event | Fields | Description |
|---|---|---|
| `BLOCK_EXEC_START` | `number`, `hash`, `parent`, `tx_count` | Block processing begins |
| `BLOCK_EXEC_END` | `number`, `hash`, `elapsed_ms` | Block execution complete (before persistence) |

#### P2P Networking
| Event | Fields | Description |
|---|---|---|
| `P2P_RECV_TX` | `peer`, `hash` | Transaction received via gossip |
| `P2P_SEND_TX` | `peer`, `hash` | Transaction broadcast to peer |
| `P2P_RECV_BLOCK` | `peer`, `number`, `hash` | Block received via P2P |
| `P2P_SEND_BLOCK` | `peer`, `number`, `hash` | Block broadcast to peer |

#### RPC Reception
| Event | Fields | Description |
|---|---|---|
| `TX_RPC_RECV` | `hash` | Transaction received via RPC |
| `BLOCK_RPC_RECV` | `hash` | Block received via RPC |

#### Sync & Downloads
| Event | Fields | Description |
|---|---|---|
| `DOWNLOAD_HEADERS_START` / `END` | — | Header sync latency tracking |
| `DOWNLOAD_BODIES_START` / `END` | — | Block body download latency |
| `DOWNLOAD_POOLED_TXS_END` | — | Pooled transaction download complete |

### 8.2 Log Processing Pipeline

1. **Log Collection** — `log_processor.sh` continuously streams Docker container logs to `logs_dump/`
2. **Log Management** — `manage_logs.sh` starts/stops the background log processor
3. **Log Aggregation** — `aggregate_logs.sh` consolidates split log files into per-service aggregated logs
4. **Log Evaluation** — `evaluate_logs.py` parses `[EXP]` tagged lines and computes metrics

---

## 9. Analysis & Evaluation

### 9.1 Log-Based Metrics (from `evaluate_logs.py`)

Computed from `[EXP]` log events:

| Metric | Calculation | Description |
|---|---|---|
| **Inclusion Latency** | `P2P_RECV_TX` → first `TX_EXEC_START` / `BLOCK_EXEC_END` containing that tx | Time from transaction receipt to execution |
| **Block Propagation Latency** | First `P2P_RECV_BLOCK` → subsequent `P2P_RECV_BLOCK` for same block hash | Time for block to propagate across nodes |
| **Avg Transaction Execution Time** | Sum of `TX_EXEC_END.elapsed_ms` / count | Average EVM execution time per transaction |
| **Avg Block Execution Time** | Sum of `BLOCK_EXEC_END.elapsed_ms` / count | Average block processing time |
| **Total Transactions Executed** | Count of `TX_EXEC_START` events | Total transactions processed |
| **Total Blocks Executed** | Count of `BLOCK_EXEC_END` events | Total blocks processed |
| **Per-Node Stats** | `tx_received`, `tx_sent`, `blocks_received`, `blocks_sent`, `blocks_executed`, `tx_executed`, `total_gas`, `total_tx_exec_ms`, `total_block_exec_ms` | Per-node breakdown |

### 9.2 Database Export (`db_exporter`)

Rust tool that exports `redb` database tables to JSON for post-hoc analysis.

**Supported EL Node Tables (19):**
`Headers`, `HeaderTD`, `BlockBodies`, `Transactions`, `Receipts`, `ReceiptsMeta`, `CanonicalHeads`, `HeaderNumbers`, `TransactionLookup`, `Accounts`, `Storages`, `Bytecodes`, `AccountChangeSets`, `StorageChangeSets`, `PlainState`, `HashedState`, `TrieNodes`, `Metadata`, `Payloads`, `Forkchoice`, `ActivePeers`

**Supported Metrics Server Tables:**
`experiments`, `nodes`, `metric_samples`, `rpc_observations`

### 9.3 Analysis Scripts

| Script | Purpose |
|---|---|
| `analyze_experiment.sh` | Analyze one experiment: export databases, evaluate logs, generate plots |
| `analyze_all_experiments.sh` | Run analysis on all experiment directories |
| `evaluate_logs.py` | Parse `[EXP]` log events; compute latency and execution metrics |
| `plot_metrics.py` | Generate PNG plots from exported metrics JSON |
| `write_experiment_metadata.py` | Write `experiment_metadata.json` with node types and workload params |
| `workload_allocation.py` | Calculate workload node distribution (largest-remainder rounding) |

### 9.4 Output Artifacts

- `experiment_results.json` — Full metrics from log evaluation
- `experiment_metadata.json` — Node types, workload parameters, targets
- `experiments/exp_YYYYMMDD_HHMMSS/` — Archived experiment data (logs, databases, state)
- PNG plots — Visualizations of transactions, mempool, blocks, peers, etc.

---

## 10. Hive Test Suite

Hive tests constitute a **separate category of experiments** from the workload-based throughput and latency experiments. While workload experiments measure performance under load, Hive tests validate **protocol compliance** and **interoperability** with the broader Ethereum ecosystem.

### 10.1 Overview

Custom HIVE (Hive Private Ethereum Test Environment) setup for Ethereum engine client compliance testing. Hive runs a suite of simulations that test various aspects of Ethereum protocol behavior, including:
- **Engine API compliance** — `ethereum/engine` simulation tests fork choice, payload building, and block proposal workflows
- **P2P protocol compliance** — Network message handling, peer discovery, and protocol negotiation
- **Consensus behavior** — Chain reorganization, finality, and attestation handling

### 10.2 Files

| File | Purpose |
|---|---|
| `hive/hive.dockerfile` | Main Dockerfile that clones ethereum/hive, compiles hive and hiveview, includes custom clients |
| `hive/README.md` | Setup and usage guide |

### 10.3 Custom Clients

#### wasix-r-eth (Native Rust)
| File | Purpose |
|---|---|
| `hive/clients/wasix-r-eth/Dockerfile` | Docker image based on `z4l3s5i0/wasix-eth-linux:latest` |
| `hive/clients/wasix-r-eth/hive.yaml` | Client definition (role: `eth1`) |
| `hive/clients/wasix-r-eth/wasix-eth.sh` | Startup script for Hive |
| `hive/clients/wasix-r-eth/enode.sh` | Node ID retrieval script |
| `hive/clients/wasix-r-eth/genesis.json` | Default genesis configuration |
| `hive/clients/wasix-r-eth/mapper.jq` | JSON field mapping for Hive |

#### wasix-w-eth (WASIX WebAssembly)
| File | Purpose |
|---|---|
| `hive/clients/wasix-w-eth/Dockerfile` | Docker image based on `z4l3s5i0/wasix-eth-wasix:latest` |
| `hive/clients/wasix-w-eth/hive.yaml` | Client definition (role: `eth1`) |
| `hive/clients/wasix-w-eth/wasix-eth.sh` | Startup script for Hive |
| `hive/clients/wasix-w-eth/enode.sh` | Node ID retrieval script |
| `hive/clients/wasix-w-eth/genesis.json` | Default genesis configuration |
| `hive/clients/wasix-w-eth/mapper.jq` | JSON field mapping for Hive |

### 10.4 Test Execution

Hive tests are run independently for each client variant:

**WASIX client (WASM):**
```bash
docker run --rm \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v $(pwd)/workspace:/hive/workspace \
  hive-custom \
  --sim ethereum/engine \
  --client wasix-w-eth \
  --sim.parallelism 5 \
  --docker.pull=false
```

**Native client (Rust):**
```bash
docker run --rm \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v $(pwd)/workspace:/hive/workspace \
  hive-custom \
  --sim ethereum/engine \
  --client wasix-r-eth \
  --sim.parallelism 20 \
  --docker.pull=false
```

**Results viewer:**
```bash
docker run --rm -it -p 8080:8080 -v $(pwd)/workspace:/hive/workspace \
  --entrypoint ./hiveview hive-custom --serve --logdir /hive/workspace/logs
```

### 10.5 Test Parameters

| Parameter | WASIX Client | Native Client | Description |
|---|---|---|---|
| **Simulation** | `ethereum/engine` | `ethereum/engine` | Engine API test suite |
| **Parallelism** | 5 | 20 | Number of concurrent test simulations |
| **Results viewer** | Hiveview at `http://localhost:8080` | Hiveview at `http://localhost:8080` | Web-based test result visualization |

The lower parallelism for the WASIX client (5 vs 20) accounts for the higher resource overhead of the Wasmer runtime, preventing resource exhaustion during concurrent test execution.

### 10.6 Metrics from Hive Tests

| Metric | Description |
|---|---|
| **Pass rate** | Percentage of tests passed per client |
| **Failure count** | Number of failed tests per client |
| **Test duration** | Time to complete the full test suite |
| **Resource usage** | CPU and memory during test execution (via Node Exporter) |
| **Comparison** | Pass/fail differential between native and WASIX variants |

---

## 11. Additional Experiments

### 11.1 Network Size Variation

The **baseline experiment** uses a network of **8 nodes**. For comparison and scalability analysis, experiments are also run at the following network sizes:

| Network Size | Purpose |
|---|---|
| **4 nodes** | Minimal network — measures overhead at small scale |
| **8 nodes** (baseline) | Default experimental configuration |
| **16 nodes** | Medium-scale scalability test |
| **32 nodes** | Large-scale scalability test |
| **64 nodes** | Maximum-scale stress test |

Each network size is tested across all workload types (Counter, Dota, Uber) and all runtime configurations (native, WASIX, mixed). This allows measuring how throughput, latency, and resource utilization scale with network size.

### 11.2 Throughput Ratios Across Network Sizes

Workload throughput is **relative to the number of nodes** per workload type. For each combination of network size and workload, we compute:

| Ratio Metric | Formula | Description |
|---|---|---|
| **Throughput per node** | Total TPS / node count | Normalized throughput independent of network size |
| **Throughput ratio** | TPS(N) / TPS(baseline) | Relative throughput compared to 8-node baseline |
| **Latency ratio** | Latency(N) / Latency(baseline) | Relative latency compared to 8-node baseline |
| **Scaling factor** | (TPS(N) - TPS(baseline)) / (N - 8) | Marginal throughput gain per additional node |

These ratios are computed for each workload type separately, as the computational complexity of each contract affects how network size impacts throughput.

### 11.3 Empty Block vs Loaded Block Execution Time

A dedicated experiment compares the execution time of **empty blocks** (blocks with zero transactions) against blocks containing a **varying number of transactions** of a single transaction type. This isolates the overhead of block processing itself from the cost of transaction execution.

**Experimental design:**

| Variable | Values | Description |
|---|---|---|
| **Block content** | Empty, N transactions | Empty blocks vs blocks with N transactions of one type |
| **Transaction type** | Counter, Dota, Uber | Each workload contract tested independently |
| **N (transaction count)** | 1, 5, 10, 20, 50, 100 | Increasing transaction density |
| **Client** | Geth, Reth, Nexus (native), Nexus (WASIX) | All four client implementations |

**Metrics measured:**

| Metric | Description |
|---|---|
| **Empty block time** | Time to execute and persist a block with no transactions |
| **Per-transaction overhead** | (Loaded block time - Empty block time) / N | Marginal cost per transaction |
| **Fixed block overhead** | Empty block execution time | Baseline cost of block processing |
| **Execution time ratio** | Loaded time / Empty time | How many times slower a loaded block is |

This experiment reveals:
- The fixed overhead of block processing (state root calculation, persistence) across clients
- How transaction execution cost scales linearly or non-linearly with block size
- Whether WASIX runtime introduces additional per-transaction overhead beyond the native runtime
- Which client has the lowest fixed block overhead

### 11.4 Startup Benchmarks

The `scripts/v2/run_startup_benchmark.sh` script measures cold-start time for each client configuration. Startup time is defined as the wall-clock duration from process start until the JSON-RPC endpoint first responds to `eth_chainId`.

**Configurations benchmarked:**

| Configuration | Description |
|---|---|
| **Geth** | Docker-based Geth startup |
| **Reth** | Docker-based Reth startup |
| **Nexus Linux** | Docker-based native Nexus startup |
| **Nexus WASIX + Docker** | WASIX Nexus run via Wasmer inside Docker |
| **Nexus Wasmer** | WASIX Nexus run via Wasmer directly (no Docker) |

**Measurement methodology:**
- Each configuration is measured for a configurable number of iterations (default: 5)
- A fresh genesis is generated for each iteration to ensure cold-start conditions
- Statistics reported: min, max, mean, median, and standard deviation (ms)
- Results are saved as CSV (`startup_benchmarks/results.csv`) and summary text (`startup_benchmarks/summary.txt`)

**Why startup time matters:**
- Fast startup is critical for container-based deployments and horizontal scaling
- WASIX/Wasmer adds an additional runtime layer that may increase cold-start latency
- Docker overhead vs bare Wasmer overhead can be compared

### 11.5 Resource Utilization — Node Exporter & dstat

Resource utilization is measured continuously during all experiments using two complementary tools.

#### Node Exporter Metrics

Prometheus Node Exporter (`prom/node-exporter:v1.8.1`) runs alongside each experiment, collecting host-level and per-container system metrics. Each NEXUS node container also runs an embedded `prometheus-node-exporter` instance on port 9100.

**Key Node Exporter metrics used:**

| Category | Metric | Description |
|---|---|---|
| **CPU** | `node_cpu_seconds_total` | Total CPU time spent in each mode (user, system, idle, iowait) |
| **CPU** | `node_load1`, `node_load5`, `node_load15` | System load averages |
| **Memory** | `node_memory_MemTotal_bytes` | Total physical memory |
| **Memory** | `node_memory_MemAvailable_bytes` | Available memory |
| **Memory** | `node_memory_MemFree_bytes` | Free memory |
| **Memory** | `node_memory_Cached_bytes` | Memory used by page cache |
| **Memory** | `node_memory_Buffers_bytes` | Memory used by kernel buffers |
| **Disk I/O** | `node_disk_read_bytes_total` | Total bytes read from disk |
| **Disk I/O** | `node_disk_written_bytes_total` | Total bytes written to disk |
| **Disk I/O** | `node_disk_io_time_seconds_total` | Time spent on I/O operations |
| **Network** | `node_network_receive_bytes_total` | Total bytes received |
| **Network** | `node_network_transmit_bytes_total` | Total bytes transmitted |
| **Network** | `node_network_receive_drop_total` | Dropped incoming packets |
| **Network** | `node_network_transmit_drop_total` | Dropped outgoing packets |
| **Filesystem** | `node_filesystem_avail_bytes` | Available filesystem space |
| **Filesystem** | `node_filesystem_size_bytes` | Total filesystem size |
| **Processes** | `node_procs_running` | Number of running processes |
| **Processes** | `node_procs_blocked` | Number of blocked processes |

**Derived metrics from Node Exporter:**

| Derived Metric | Calculation | Purpose |
|---|---|---|
| **CPU utilization %** | (1 - idle_cpu / total_cpu) × 100 | Overall CPU usage per node |
| **Memory utilization %** | (total - available) / total × 100 | Memory pressure per node |
| **I/O wait %** | iowait_cpu / total_cpu × 100 | Disk I/O bottleneck indicator |
| **Disk throughput (MB/s)** | Δ(write_bytes) / Δ(time) | Write amplification analysis |
| **Network throughput (MB/s)** | Δ(receive_bytes + transmit_bytes) / Δ(time) | P2P bandwidth usage |

#### dstat Metrics

`dstat` provides real-time system resource statistics as a complementary, human-readable monitoring tool. It captures the same dimensions as Node Exporter but with different granularity and presentation.

**dstat metrics collected:**

| Category | Metrics | Description |
|---|---|---|
| **CPU** | `%usr`, `%sys`, `%idl`, `%wai`, `%hiq`, `%siq` | CPU time distribution |
| **Memory** | `used`, `free`, `buff`, `cache` | Memory usage in MB |
| **Disk** | `read`, `writ` | Disk I/O in KB/s |
| **Network** | `recv`, `send` | Network throughput in KB/s |
| **Interrupts** | `intr` | Hardware interrupt rate |
| **Context switches** | `cswt` | Context switch rate |
| **Processes** | `running`, `sleeping`, `thr` | Process state counts |

**Why both tools:**
- Node Exporter provides precise, Prometheus-integrated time-series data for post-hoc analysis
- dstat provides real-time, human-readable snapshots for quick diagnosis and correlation
- Together they cover both automated analysis and manual inspection needs

### 11.6 Code Size & Binary Size Comparison

A static comparison of the three client implementations measures lines of code and compiled binary sizes.

**Lines of code comparison:**

| Client | Language | Measurement | Description |
|---|---|---|---|
| **Geth** | Go | Total Go source lines | Includes all packages in go-ethereum |
| **Reth** | Rust | Total Rust source lines | Includes all crates in the Reth workspace |
| **Nexus** | Rust | Total Rust source lines | Includes all crates in the Nexus workspace |

Lines of code are counted excluding:
- Auto-generated files (build scripts, codegen)
- Test files (unless explicitly included for comparison)
- Blank lines and comments (using `cloc` or equivalent)

**Binary size comparison:**

| Build Target | Artifact | Description |
|---|---|---|
| **Geth** | Compiled Go binary | `geth` executable size |
| **Reth** | Compiled Rust binary | `reth` executable size |
| **Nexus Native** | Compiled Rust binary | `wasix_eth` native executable size |
| **Nexus WASIX** | WASM module | `wasix_eth.wasi.wasm` module size |
| **Docker image** | Full container image | Geth, Reth, Nexus Linux, Nexus WASIX image sizes |

**Why this matters:**
- Binary size affects deployment time, storage requirements, and container pull latency
- WASM module size is particularly relevant for edge deployments
- Lines of code correlate with maintenance burden and attack surface
- Rust vs Go compilation characteristics (AOT vs JIT-like optimizations)

### 11.7 WASM Runtime Justification — Why Wasmer

The project uses **Wasmer** as the WebAssembly runtime for executing NEXUS in WASIX mode. This choice is driven by the following considerations:

#### WASIX Runtime Availability

**WASIX** (WebAssembly eXtended ISAs) is a specification that extends WebAssembly with POSIX-like system calls, enabling general-purpose applications — including network servers and file-system-aware programs — to run in WASM environments. Among the major WASM runtimes, **Wasmer is the only runtime with production-grade WASIX support**.

| Runtime | WASIX Support | Status |
|---|---|---|
| **Wasmer** | Full WASIX support | Production-ready |
| **Wasmtime** | Limited WASI preview2 | No WASIX support |
| **WasmEdge** | WASI-only | No WASIX support |
| **Spidermonkey WASI** | WASI-only | No WASIX support |

Since NEXUS requires full POSIX semantics (file I/O, networking, async threads) to function as an Ethereum client, WASIX is a hard requirement. Wasmer is the only viable runtime.

#### Wasmer-Specific Features Used

| Feature | Flag | Purpose |
|---|---|---|
| **Async threads** | `--enable-async-threads` | Multi-threaded execution for P2P and RPC |
| **Networking** | `--net` | Socket-based P2P communication |
| **Volume mounts** | `--volume` | Persistent storage access for redb database |
| **WASIX syscalls** | Built-in | File system, process, and network operations |

#### Performance Trade-offs

Using Wasmer introduces a runtime overhead compared to native execution. This overhead is measured and reported in the experiments:
- **Startup time** — Wasmer must load and compile the WASM module before execution
- **Per-transaction cost** — Each EVM opcode execution goes through the Wasmer interpreter/JIT
- **Memory overhead** — Wasmer's sandbox adds memory management layers

These trade-offs are quantified in the throughput and latency experiments, where Nexus (WASIX) is compared against Nexus (native), Geth, and Reth.

---

## 12. Thesis Results

### 12.1 Benchmark Results

The `thesis_results/` directory contains benchmark result PDFs comparing three build variants across three workloads:

| Runtime | Workload | File |
|---|---|---|
| **wasix** (WASIX WebAssembly) | Dota | `thesis_results/wasix_dota.pdf` |
| **wasix** (WASIX WebAssembly) | Fifa | `thesis_results/wasix_fifa.pdf` |
| **wasix** (WASIX WebAssembly) | Uber | `thesis_results/wasix_uber.pdf` |
| **rust** (Native Rust) | Dota | `thesis_results/rust_dota.pdf` |
| **rust** (Native Rust) | Fifa | `thesis_results/rust_fifa.pdf` |
| **rust** (Native Rust) | Uber | `thesis_results/rust_uber.pdf` |
| **mix** (Mixed network) | Dota | `thesis_results/mix_dota.pdf` |
| **mix** (Mixed network) | Fifa | `thesis_results/mix_fifa.pdf` |
| **mix** (Mixed network) | Uber | `thesis_results/mix_uber.pdf` |

### 12.2 Failure Reports

| File | Purpose |
|---|---|
| `thesis_results/failures_report.json` | Summary of test failures |
| `thesis_results/failures_detailed.json` | Detailed failure analysis |

### 12.3 Measured Performance Characteristics

The benchmark results document:
- Transaction throughput across runtimes
- Block propagation latency
- Resource usage (CPU, memory) across execution environments
- Comparison of native vs WASIX vs mixed-network performance

---

## 13. File Inventory

### 13.1 Documentation

| File | Description |
|---|---|
| `README.md` | Project overview, architecture, quick start |
| `docs/ARCHITECTURE.md` | Internal design: crate dependencies, data flows, Engine API, sync, reorg handling |
| `docs/DEVELOPMENT.md` | Developer guide: prerequisites, building, running, testing |
| `docs/EXPERIMENTS.md` | Experiment and benchmarking guide |
| `docs/NETWORKING.md` | P2P stack: Discv4, RLPx, peer management, gossip |
| `docs/RPC_API.md` | JSON-RPC API reference |
| `docs/STORAGE.md` | Storage layer: redb schema, codecs, providers, trie |
| `wasix_eth/EXP_LOGS_README.md` | Experiment logging format and usage |
| `wasix_eth_comparison_report.md` | Architectural comparison: Nexus vs Geth vs Reth |
| `md/RELEASE_v0.1.0.md` | Release notes v0.1.0 |
| `md/RELEASE_v0.2.0.md` | Release notes v0.2.0 |
| `md/USER_GUIDE.md` | Multi-node deployment guide |
| `md/TODO.md` | Project TODO list |
| `metrics_server/README.md` | Metrics server documentation |
| `hive/README.md` | Hive test suite guide |
| `scripts/v2/GETH_EXPERIMENTS.md` | Geth-specific experiment notes |
| `scripts/v2/GETH_METRICS.md` | Geth metrics documentation |

### 13.2 Experiment Scripts (`scripts/v2/`)

| File | Category | Description |
|---|---|---|
| `run_experiment.sh` | Orchestration | Single experiment runner (full lifecycle) |
| `run_experiments_loop.sh` | Orchestration | Batch experiment runner (matrix of configs) |
| `setup.sh` | Infrastructure | Build all Docker images |
| `start_local.sh` | Infrastructure | Deploy Docker Compose network |
| `cleanup.sh` | Infrastructure | Stop and remove containers |
| `config_gen.sh` | Infrastructure | Generate node TOML configs |
| `run_workload.sh` | Workload | Run Contender-based workload |
| `collect_results.sh` | Collection | Archive experiment data |
| `aggregate_logs.sh` | Collection | Consolidate split log files |
| `manage_logs.sh` | Collection | Start/stop background log processor |
| `log_processor.sh` | Collection | Stream container logs to `logs_dump/` |
| `analyze_experiment.sh` | Analysis | Analyze one experiment directory |
| `analyze_all_experiments.sh` | Analysis | Analyze all experiments |
| `evaluate_logs.py` | Analysis | Parse `[EXP]` log events |
| `evaluate.sh` | Analysis | Shell-based log evaluation |
| `plot_metrics.py` | Analysis | Generate PNG plots |
| `write_experiment_metadata.py` | Analysis | Write experiment metadata JSON |
| `workload_allocation.py` | Analysis | Workload node distribution |
| `test_experiment_analysis.py` | Testing | Tests for experiment analysis |
| `test_shell_syntax.py` | Testing | Shell syntax validation |
| `test_workload_allocation.py` | Testing | Workload allocation tests |
| `push_contender.sh` | Image Push | Push Contender image |
| `push_linux.sh` | Image Push | Push Linux image (with optional node-exporter) |
| `push_wasix.sh` | Image Push | Push WASIX image (with optional node-exporter) |
| `push_metrics.sh` | Image Push | Push metrics server image |
| `run_startup_benchmark.sh` | Benchmark | Cold-start time benchmark (5 configs, CSV + summary output) |
| `common.sh` | Utility | Shared shell functions |
| `get_failures.ps1` | Utility | PowerShell failure extraction |
| `get_failures_v2.ps1` | Utility | PowerShell failure extraction v2 |
| `db_exporter/src/main.rs` | Utility | Rust database export tool |
| `db_exporter/Cargo.toml` | Utility | db_exporter Cargo config |
| `templates/docker-compose.yml.template` | Template | Docker Compose template |
| `monitoring/prometheus.yml` | Monitoring | Prometheus scrape config (prometheus, node-exporter, metrics-server) |
| `monitoring/grafana/dashboards/exemplary-dashboard.json` | Monitoring | Grafana dashboard |
| `monitoring/grafana/provisioning/dashboards/provider.yaml` | Monitoring | Grafana dashboard provisioning |
| `monitoring/grafana/provisioning/datasources/prometheus.yaml` | Monitoring | Grafana datasource provisioning |

### 13.3 Legacy Scripts (`scripts/`)

| File | Description |
|---|---|
| `analyze_slots.py` | Slot analysis |
| `analyze_stacked_faceted.sh` | Stacked faceted analysis |
| `check_and_run_hive.sh` | Hive test checker |
| `extract_block_times.py` | Block time extraction |
| `plot_non_stacked_faceted.py` | Non-stacked faceted plotting |
| `plot_stacked_faceted.py` | Stacked faceted plotting |

### 13.4 Diablo Workloads (`diablo_workloads/`)

| File | Description |
|---|---|
| `counter.sol` | Low-complexity contract (simple increment) |
| `dota.sol` | Medium-complexity contract (array iteration, boundary checks) |
| `uber.sol` | High-complexity contract (distance calc, iterative sqrt, 10K drivers) |
| `other-workload-dota.yaml` | Contender workload config for Dota |
| `other-workload-uber.yaml` | Contender workload config for Uber |
| `other-workload-football.yaml` | Contender workload config for Football |

### 13.5 Hive Test Suite (`hive/`)

| File | Description |
|---|---|
| `hive.dockerfile` | Main Hive Docker image |
| `README.md` | Setup guide |
| `clients/wasix-r-eth/Dockerfile` | Native client Docker image |
| `clients/wasix-r-eth/hive.yaml` | Native client definition |
| `clients/wasix-r-eth/wasix-eth.sh` | Native client startup |
| `clients/wasix-r-eth/enode.sh` | Native client node ID retrieval |
| `clients/wasix-r-eth/genesis.json` | Native client genesis |
| `clients/wasix-r-eth/mapper.jq` | Native client JSON mapping |
| `clients/wasix-w-eth/Dockerfile` | WASIX client Docker image |
| `clients/wasix-w-eth/hive.yaml` | WASIX client definition |
| `clients/wasix-w-eth/wasix-eth.sh` | WASIX client startup |
| `clients/wasix-w-eth/enode.sh` | WASIX client node ID retrieval |
| `clients/wasix-w-eth/genesis.json` | WASIX client genesis |
| `clients/wasix-w-eth/mapper.jq` | WASIX client JSON mapping |

### 13.6 Metrics Server (`metrics_server/`)

| File | Description |
|---|---|
| `Cargo.toml` | Rust package config |
| `Dockerfile` | Docker image |
| `docker-compose.yml` | Docker Compose config |
| `README.md` | Documentation |
| `config/example.toml` | Example configuration |
| `config/docker.toml` | Docker configuration |
| `config/empty_nodes.toml` | Empty node config |
| `data/metrics.redb` | Persistent metrics database |
| `src/main.rs` | Entry point |
| `src/app.rs` | Application setup |
| `src/config.rs` | Configuration parsing |
| `src/cli.rs` | CLI arguments |
| `src/model.rs` | Data models |
| `src/error.rs` | Error types |
| `src/time.rs` | Time utilities |
| `src/api/mod.rs` | API module |
| `src/api/handlers.rs` | HTTP handlers |
| `src/api/routes.rs` | Route definitions |
| `src/collector/mod.rs` | Collector module |
| `src/collector/rpc_collector.rs` | JSON-RPC collector |
| `src/collector/prometheus_scraper.rs` | Prometheus scraper |
| `src/collector/scheduler.rs` | Collection scheduler |
| `src/storage/mod.rs` | Storage module |
| `src/storage/memory.rs` | In-memory backend |
| `src/storage/redb_store.rs` | redb persistent backend |
| `src/telemetry/mod.rs` | Telemetry module |
| `src/telemetry/registry.rs` | Prometheus metric registry |
| `src/ethereum/mod.rs` | Ethereum module |
| `src/ethereum/rpc_client.rs` | JSON-RPC client |
| `src/ethereum/types.rs` | Ethereum type parsing |
| `src/bootstrap/mod.rs` | Bootstrap module |
| `src/bootstrap/prober.rs` | Peer health prober |

### 13.7 Thesis Results (`thesis_results/`)

| File | Description |
|---|---|
| `wasix_dota.pdf` | WASIX × Dota benchmark |
| `wasix_fifa.pdf` | WASIX × Fifa benchmark |
| `wasix_uber.pdf` | WASIX × Uber benchmark |
| `rust_dota.pdf` | Native × Dota benchmark |
| `rust_fifa.pdf` | Native × Fifa benchmark |
| `rust_uber.pdf` | Native × Uber benchmark |
| `mix_dota.pdf` | Mixed × Dota benchmark |
| `mix_fifa.pdf` | Mixed × Fifa benchmark |
| `mix_uber.pdf` | Mixed × Uber benchmark |
| `failures_report.json` | Failure summary |
| `failures_detailed.json` | Detailed failures |

### 13.8 NEXUS Core (`wasix_eth/`)

| File | Description |
|---|---|
| `Cargo.toml` | Workspace config |
| `Dockerfile.linux` | Native Docker image |
| `Dockerfile.wasix` | WASIX Docker image |
| `EXP_LOGS_README.md` | Experiment logging docs |
| `experiment_results.json` | Experiment results |
| `wasix_eth_app/` | CLI application crate |
| `wasix_eth_core/` | Core protocol logic crate |
| `wasix_eth_execution/` | EVM execution crate |
| `wasix_eth_p2p/` | P2P networking crate |
| `wasix_eth_rpc/` | JSON-RPC server crate |
| `wasix_eth_storage/` | Storage layer crate |
| `wasix_eth_types/` | Shared types crate |
| `wasix_eth_utils/` | Utilities crate (metrics, logging, mappers) |
| `testing/` | Integration tests crate |

---

## Summary

The NEXUS experimental framework provides a complete pipeline for benchmarking Ethereum execution clients across native and WebAssembly environments. The setup supports:

- **4 node types** (Native Rust, WASIX, Geth, Reth) in mixed networks
- **5 network sizes** (4, 8, 16, 32, 64 nodes) with 8-node baseline
- **3 workload complexities** (Counter, Dota, Uber) with increasing computational demand
- **Throughput ratios** normalized per node and relative to baseline across network sizes
- **50+ metrics** across node-level Prometheus, RPC-derived, server-level telemetry, Node Exporter system metrics, and dstat
- **Block execution time** and **transaction latency** as first-class performance metrics
- **10 structured log events** for fine-grained latency and throughput analysis
- **Empty vs loaded block** experiments to isolate fixed block overhead from transaction cost
- **Startup benchmarks** across 5 client configurations (Geth, Reth, Nexus native, Nexus WASIX+Docker, Nexus Wasmer)
- **Resource utilization** monitoring via Node Exporter and dstat (CPU, memory, cache, disk I/O, network)
- **Code size and binary size** comparison across client implementations
- **Hive compliance testing** as a separate category for protocol compliance validation
- **Automated analysis** with database export, log parsing, and plot generation
- **Persistent storage** via `redb` for both node state and metrics data
- **Wasmer runtime** chosen as the only WASM runtime with production-grade WASIX support

The key differentiator is the ability to run the same Ethereum client code in both native and WASIX environments, enabling direct comparison of performance characteristics while controlling for all other variables.
