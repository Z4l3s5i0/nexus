# Experiment and Benchmarking Guide

This document describes the experimental framework for NEXUS, including multi-node deployment, workload generation, metrics collection, log analysis, and the HIVE test suite integration.

## Overview

The project includes a comprehensive experimentation pipeline for benchmarking NEXUS nodes against each other and against reference implementations (e.g., Geth). The framework supports:

- **Multi-node local deployment** — Docker Compose-based networks with mixed node types (native Rust, WASIX, Geth)
- **Automated workload generation** — Contender-based transaction spam and campaign scenarios
- **Metrics collection** — Prometheus + Grafana monitoring via `metrics_server`
- **Log analysis** — Automated log parsing for latency, throughput, and correctness metrics
- **Database export** — `db_exporter` tool for extracting node state for post-hoc analysis
- **HIVE integration** — Docker-based Ethereum engine client test suite

## Experimental Workflow

A typical experiment follows these steps:

```
1. Setup → 2. Deploy Network → 3. Run Workload → 4. Collect Logs → 5. Analyze Results
```

### 1. Setup

Run `scripts/v2/setup.sh` to build all required Docker images and prepare the environment:

```bash
./scripts/v2/setup.sh
```

This builds the NEXUS WASIX and native images, the metrics server image, and verifies dependencies.

### 2. Deploy Network

Use `scripts/v2/start_local.sh` to deploy a multi-node Docker Compose network:

```bash
./scripts/v2/start_local.sh \
  --linux-nodes 2 \
  --wasix-nodes 2 \
  --geth-nodes 0 \
  --chain-id 12345 \
  --cleanup
```

**Options:**

| Flag | Description | Default |
|---|---|---|
| `--linux-nodes N` | Number of native Rust nodes | 2 |
| `--wasix-nodes N` | Number of WASIX nodes | 2 |
| `--geth-nodes N` | Number of Geth nodes | 0 |
| `--chain-id ID` | Unique chain ID | 12345 |
| `--cleanup` | Remove existing data before starting | false |
| `--setup` | Run setup.sh before starting | false |
| `--registry URL` | Docker registry for images | — |
| `--tag TAG` | Image tag | latest |
| `--metrics-image IMAGE` | Pre-built metrics image | — |

The script:
1. Generates network configuration via `config_gen.sh`
2. Copies monitoring configuration (Prometheus + Grafana)
3. Builds or pulls Docker images
4. Creates a `docker-compose.v2.yml` file
5. Starts all containers on a shared Docker network

Container naming convention: `el-node-0`, `el-node-1`, etc. Each node gets incrementing RPC ports (8545, 8546, ...) and P2P ports (9000, 9001, ...).

### 3. Run Workload

Use `scripts/v2/run_workload.sh` to generate transaction load using the Contender tool:

```bash
./scripts/v2/run_workload.sh \
  --rpc http://host.docker.internal:8545 \
  --pk "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80" \
  --scenario simple \
  --tps 10 \
  --duration 60 \
  --accounts 10 \
  --network wasm
```

**Options:**

| Flag | Description | Default |
|---|---|---|
| `--rpc URL` | Target node RPC URL | `http://host.docker.internal:8545` |
| `--pk KEY` | Private key for signing | Dev account key |
| `--scenario NAME` | Scenario: `simple`, `stress`, `uniV2`, etc. | simple |
| `--campaign NAME` | Campaign TOML file (overrides --scenario) | — |
| `--tps N` | Transactions per second | 10 |
| `--duration N` | Duration in seconds | 60 |
| `--accounts N` | Accounts per agent | 10 |
| `--min-balance B` | Minimum account balance | 100eth |
| `--gas-limit N` | Gas limit override | — |
| `--network NAME` | Docker network name | — |
| `--state-dir DIR` | Contender state directory | `.contender_state` |
| `--skip-funding` | Skip account funding | false |
| `--run-id ID` | Label containers for experiment | — |

The script runs two phases:
1. **Setup** — Deploys contracts and funds accounts via Contender
2. **Execution** — Spams transactions at the specified TPS for the given duration

### 4. Collect Logs

Use `scripts/v2/collect_results.sh` or `scripts/v2/aggregate_logs.sh` to gather logs from all nodes:

```bash
./scripts/v2/aggregate_logs.sh
```

This collects container logs into a centralized directory for analysis.

### 5. Analyze Results

Use `scripts/v2/analyze_experiment.sh` to process logs and export database state:

```bash
./scripts/v2/analyze_experiment.sh /path/to/experiment_dir [chain_id]
```

The analysis pipeline:
1. **Metrics server data** — Exports `experiments`, `nodes`, `metric_samples`, `rpc_observations` tables from the metrics database
2. **EL node data** — Exports `headers`, `block_bodies`, `transactions`, `metadata` from each node's `redb` database using `db_exporter`
3. **Log evaluation** — Parses `[EXP]` tagged log lines to compute:
   - Transaction propagation latency (peer-to-peer)
   - Block propagation latency
   - Execution times per node
   - Gas usage statistics
4. **Plots** — Generates visualizations from metrics data (currently commented out)

## Script Reference

### Core Scripts

| Script | Purpose |
|---|---|
| `start_local.sh` | Deploy local Docker Compose network |
| `run_workload.sh` | Run Contender-based workload |
| `analyze_experiment.sh` | Analyze experiment logs and databases |
| `aggregate_logs.sh` | Collect logs from all containers |
| `collect_results.sh` | Gather experiment results |
| `cleanup.sh` | Stop and remove containers, clean data |
| `setup.sh` | Build images and prepare environment |
| `config_gen.sh` | Generate node configuration files |
| `evaluate.sh` | Shell-based log evaluation |
| `evaluate_logs.py` | Python-based log evaluation with metadata support |
| `run_experiment.sh` | Single experiment runner |
| `run_experiments_loop.sh` | Batch experiment runner |
| `push_contender.sh` | Push Contender Docker image |
| `push_linux.sh` | Push native node Docker image |
| `push_wasix.sh` | Push WASIX node Docker image |
| `push_metrics.sh` | Push metrics server Docker image |

### Analysis Scripts

| Script | Purpose |
|---|---|
| `evaluate_logs.py` | Parse `[EXP]` log events, compute latencies and throughput |
| `plot_metrics.py` | Generate plots from exported metrics JSON |
| `workload_allocation.py` | Calculate workload distribution across nodes |
| `write_experiment_metadata.py` | Write experiment metadata JSON |
| `db_exporter/` | Rust tool for exporting `redb` database tables to JSON |

### Monitoring

| File | Purpose |
|---|---|
| `monitoring/prometheus.yml` | Prometheus scrape configuration |
| `monitoring/grafana/` | Grafana dashboards and provisioning |

## Diablo Workloads

The `diablo_workloads/` directory contains Solidity contracts used as benchmark workloads, each with increasing computational complexity:

### Counter (`counter.sol`)

**Complexity:** Low — Simple state increment

```solidity
contract Counter {
    int private count = 0;
    function Add() public { count += 1; }
}
```

Measures basic transaction throughput and block production latency with minimal computation.

### Dota (`dota.sol`)

**Complexity:** Medium — Array iteration with boundary checks

```solidity
contract DecentralizedDota {
    int public number_of_players = 10;
    uint[] public team_positions_x;
    uint[] public team_positions_y;
    uint public maxlen = 250;

    function update() public {
        for(uint i = 0; i < uint(number_of_players); i++) {
            team_positions_x[i] += 1;
            team_positions_y[i] += 1;
            // Boundary wrapping
        }
    }
}
```

Measures performance with array reads/writes and loop-based computation.

### Uber (`uber.sol`)

**Complexity:** High — Distance calculation with square root

```solidity
contract ContractUber {
    uint public number_of_drivers = 10000;
    uint[] public driver_x;
    uint[] public driver_y;
    uint[] public client_x;
    uint[] public client_y;

    function checkDistance() public {
        // Iterate all drivers, compute Euclidean distance
        // Uses iterative square root algorithm
        // Find closest driver to client
    }
}
```

Measures performance under heavy computation: large array iteration, arithmetic operations, and iterative square root calculation.

### Workload YAML Configs

| File | Description |
|---|---|
| `other-workload-dota.yaml` | Diablo workload config for Dota contract |
| `other-workload-uber.yaml` | Diablo workload config for Uber contract |
| `other-workload-football.yaml` | Diablo workload config for Football variant |

## Metrics Collection

### Metrics Server

The `metrics_server/` is a standalone Rust service that provides:

**Endpoints:**

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

**Storage backends:**
- `memory` — In-memory storage (development)
- `redb` — Embedded key-value store (production, default: `metrics_server/data/metrics.redb`)

**Node registration:**

```bash
curl -X POST http://127.0.0.1:9100/api/nodes/register \
  -H "Authorization: Bearer dev-token-123" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "node-3",
    "network": "devnet",
    "client": "wasix_eth",
    "rpc_url": "http://127.0.0.1:28545",
    "p2p_addr": "127.0.0.1:29002",
    "enode": "enode://pubkey@127.0.0.1:29002"
  }'
```

**Bootstrap peer discovery:**

```bash
curl "http://127.0.0.1:9100/api/bootstrap/nodes?network=devnet&limit=16"
```

### Prometheus + Grafana

The monitoring stack is configured in `scripts/v2/monitoring/`:

- **Prometheus** scrapes `/metrics` from all nodes and the metrics server
- **Grafana** provides dashboards for real-time visualization
- Configuration is automatically copied to `startup_v2/` by `start_local.sh`

Access Grafana at `http://localhost:3000` (default credentials from provisioning config).

## HIVE Test Suite

The `hive/` directory contains a custom HIVE (Hive Private Ethereum Test Environment) setup for Ethereum engine client testing.

### Building

```bash
docker build -f ./hive/hive.dockerfile -t hive-custom .
```

### Running Tests

**WASIX Wasm-Ethereum Client:**
```bash
docker run --rm \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v $(pwd)/workspace:/hive/workspace \
  hive-custom \
  --sim ethereum/engine \
  --client wasix-w-eth \
  --sim.parallelism 5
```

**WASIX Rust-Ethereum Client:**
```bash
docker run --rm \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v $(pwd)/workspace:/hive/workspace \
  hive-custom \
  --sim ethereum/engine \
  --client wasix-r-eth \
  --sim.parallelism 20
```

### Hiveview (Results Viewer)

```bash
docker run --rm -it -p 8080:8080 \
  -v $(pwd)/workspace:/hive/workspace \
  --entrypoint ./hiveview hive-custom \
  --serve --logdir /hive/workspace/logs
```

Access the results viewer at `http://localhost:8080`.

## Thesis Results

The `thesis_results/` directory contains benchmark result PDFs and failure reports comparing three build variants:

- **wasix** — NEXUS running in WASIX WebAssembly environment
- **rust** — NEXUS running as native Rust binary
- **wasm** — NEXUS running in alternative WASM runtime

These results document performance characteristics including transaction throughput, block propagation latency, and resource usage across the different execution environments.

## Running a New Experiment

### Quick Start

```bash
# 1. Setup and deploy a 4-node network
./scripts/v2/start_local.sh --linux-nodes 2 --wasix-nodes 2 --cleanup

# 2. Run a workload against node 0
./scripts/v2/run_workload.sh \
  --rpc http://host.docker.internal:8545 \
  --scenario simple \
  --tps 20 \
  --duration 120 \
  --network wasm

# 3. Collect and analyze results
./scripts/v2/aggregate_logs.sh
./scripts/v2/analyze_experiment.sh ./experiment_output
```

### Batch Experiments

Use `run_experiments_loop.sh` to run multiple experiments with different parameters:

```bash
./scripts/v2/run_experiments_loop.sh
```

This script iterates over predefined configurations, running each experiment sequentially and collecting results.

### Remote Deployment

For remote cluster deployment, use the push scripts to upload images to a registry, then deploy using the remote deployment scripts (following the same pattern as `start_local.sh` but targeting remote hosts).
