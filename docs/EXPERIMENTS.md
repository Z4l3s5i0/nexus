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

## Scripts Overview

All experiment scripts live in `scripts/v2/`. They fall into four categories:

| Category | Scripts |
|---|---|
| **Orchestration** | `run_experiment.sh`, `run_experiments_loop.sh` |
| **Infrastructure** | `setup.sh`, `start_local.sh`, `cleanup.sh`, `config_gen.sh` |
| **Workload & Collection** | `run_workload.sh`, `collect_results.sh`, `aggregate_logs.sh`, `manage_logs.sh`, `log_processor.sh` |
| **Analysis** | `analyze_experiment.sh`, `analyze_all_experiments.sh`, `evaluate_logs.py`, `evaluate.sh`, `plot_metrics.py`, `write_experiment_metadata.py`, `workload_allocation.py` |
| **Image Push** | `push_contender.sh`, `push_linux.sh`, `push_wasix.sh`, `push_metrics.sh` |
| **Utilities** | `common.sh`, `db_exporter/` (Rust tool) |

### Orchestration Scripts

These are the main entry points for running experiments.

#### `run_experiment.sh` — Single Experiment Runner

Orchestrates the full lifecycle of one experiment run: deploys the network, starts the workload, waits for completion, collects results, and cleans up.

```bash
./scripts/v2/run_experiment.sh \
  --linux-nodes 3 \
  --wasix-nodes 3 \
  --geth-nodes 0 \
  --workload-type exp/uber \
  --tps 4 \
  --spammer-accounts 4 \
  --duration 120 \
  --wait-duration 900 \
  --chain-id 1000
```

**Options:**

| Flag | Description | Default |
|---|---|---|
| `--linux-nodes N` | Number of native Rust nodes | 5 |
| `--wasix-nodes N` | Number of WASIX nodes | 5 |
| `--geth-nodes N` | Number of Geth nodes | 0 |
| `--workload-type TYPE` | Workload scenario (e.g., `exp/uber`, `exp/counter`, `exp/dota`) | exp/uber |
| `--tps N` | Transactions per second | 4 |
| `--spammer-accounts N` | Accounts per spammer | 4 |
| `--duration N` | Workload duration in seconds | 300 |
| `--wait-duration N` | Additional wait after workload | 1800 |
| `--workload-percent N` | Percent of nodes receiving workload | 100 |
| `--chain-id ID` | Unique chain ID | 10 |
| `--metrics-image IMAGE` | Pre-built metrics image | — |

**What it does (full lifecycle):**
1. **Deploys network** — Calls `start_local.sh` internally with the given node configuration
2. **Starts log processor** — Launches `manage_logs.sh` to stream container logs to `logs_dump/`
3. **Waits for stabilization** — 60s for non-Geth networks, 120s+ per Geth node for state trie initialization
4. **Starts workloads** — Launches `run_workload.sh` for each target node
5. **Waits for completion** — Sleeps for `duration` + `wait-duration` seconds
6. **Stops workloads** — Terminates workload processes and Contender containers
7. **Stops log processor** — Flushes and stops the background log stream
8. **Collects results** — Runs `collect_results.sh` to archive all data to `experiments/exp_YYYYMMDD_HHMMSS/`
9. **Cleans up** — Calls `cleanup.sh` to stop and remove all containers

The workload node allocation uses largest-remainder rounding via `workload_allocation.py` to distribute the workload percentage proportionally across node types.

#### `run_experiments_loop.sh` — Batch Experiment Runner

Runs `run_experiment.sh` repeatedly across a matrix of workload types and node ratio combinations.

```bash
./scripts/v2/run_experiments_loop.sh
```

By default it iterates over three workload types (`exp/uber`, `exp/counter`, `exp/dota`) and seven node ratio configurations (pure Linux, pure WASIX, pure Geth, and various mixes), producing 21 total experiment combinations.

**Custom ratios:**

```bash
./scripts/v2/run_experiments_loop.sh --ratios '3 4 3; 0 5 5'
```

Each ratio entry is `LINUX WASIX [GETH]` (Geth defaults to 0). Entries are separated by semicolons.

**Logging output:**

All console output is automatically logged to a file (default: `experiments.log`). The log file also shows live output on the terminal via `tee`. To use a custom log file name:

```bash
LOG_FILE=my_run.log ./scripts/v2/run_experiments_loop.sh
```

**Running on a remote server:**

To keep the script running after disconnecting from the server, use `nohup`:

```bash
nohup ./scripts/v2/run_experiments_loop.sh > /dev/null 2>&1 &
```

- `nohup` prevents the process from being killed when you log out.
- `> /dev/null 2>&1` discards the direct output (it is already going to the log file via `tee`).
- `&` runs it in the background.

To check progress after reconnecting:

```bash
tail -f experiments.log
```

To check if the process is still running:

```bash
ps aux | grep run_experiments_loop
```

### Infrastructure Scripts

| Script | Purpose |
|---|---|
| `setup.sh` | Build all Docker images (NEXUS native, WASIX, metrics server) |
| `start_local.sh` | Deploy a Docker Compose network with mixed node types |
| `cleanup.sh` | Stop and remove all containers, clean up data directories |
| `config_gen.sh` | Generate node TOML configuration files for a given chain ID and node count |

`start_local.sh` accepts `--linux-nodes`, `--wasix-nodes`, `--geth-nodes`, `--chain-id`, `--cleanup`, `--setup`, `--registry`, `--tag`, and `--metrics-image`. It generates config, copies monitoring files, creates `docker-compose.v2.yml`, and starts all containers. Nodes are named `el-node-0`, `el-node-1`, … with incrementing RPC (8545+) and P2P (9000+) ports.

### Workload & Collection Scripts

| Script | Purpose |
|---|---|
| `run_workload.sh` | Run Contender-based transaction workload against a target RPC endpoint |
| `collect_results.sh` | Archive all experiment data (logs, databases, state) to a timestamped directory |
| `aggregate_logs.sh` | Consolidate split log files into per-service aggregated logs |
| `manage_logs.sh` | Start/stop the background log processor |
| `log_processor.sh` | Continuously stream Docker container logs to `logs_dump/` |

`run_workload.sh` runs two phases: **Setup** (deploys contracts, funds accounts) and **Execution** (spams transactions at the specified TPS). Key options: `--rpc`, `--pk`, `--scenario`, `--tps`, `--duration`, `--accounts`, `--network`.

### Analysis Scripts

| Script | Purpose |
|---|---|
| `analyze_experiment.sh` | Analyze one experiment directory: export databases, evaluate logs, generate plots |
| `analyze_all_experiments.sh` | Run `analyze_experiment.sh` on every experiment directory in a given path |
| `evaluate_logs.py` | Parse `[EXP]` log events; compute tx/block propagation latency, execution times, gas stats |
| `evaluate.sh` | Shell-based fallback for log evaluation |
| `plot_metrics.py` | Generate PNG plots from exported metrics JSON (transactions, mempool, blocks, peers, etc.) |
| `write_experiment_metadata.py` | Write `experiment_metadata.json` with node types, workload params, and targets |
| `workload_allocation.py` | Calculate workload node distribution across types using largest-remainder rounding |
| `db_exporter/` | Rust tool to export `redb` database tables (metrics server and EL node data) to JSON |

`analyze_experiment.sh <experiment_dir> [chain_id]` performs:
1. **Metrics server export** — `experiments`, `nodes`, `metric_samples`, `rpc_observations` tables to JSON
2. **EL node export** — `headers`, `block_bodies`, `transactions`, `metadata` per node via `db_exporter`
3. **Log evaluation** — Parses `[EXP]` tagged lines for propagation latency and execution stats
4. **Plots** — Generates visualizations (currently commented out)

`analyze_all_experiments.sh <base_dir> [chain_id]` iterates over all subdirectories and runs the single-experiment analyzer on each one.

## Monitoring

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
- **mix** — NEXUS as a mix of both above-mentioned runtimes

These results document performance characteristics including transaction throughput, block propagation latency, and resource usage across the different execution environments.

## Running a New Experiment

### Single Experiment

The easiest way to run an experiment end-to-end is `run_experiment.sh`:

```bash
./scripts/v2/run_experiment.sh \
  --linux-nodes 3 \
  --wasix-nodes 3 \
  --workload-type exp/uber \
  --tps 4 \
  --duration 120 \
  --wait-duration 900
```

This handles the entire lifecycle: network deployment, workload execution, result collection, and cleanup.

### Batch Experiments

For running many parameter combinations, use `run_experiments_loop.sh` (see the **Orchestration Scripts** section above for details on custom ratios, logging, and remote server execution).

### Manual Workflow

For fine-grained control, run the steps individually:

```bash
# 1. Setup and deploy
./scripts/v2/start_local.sh --linux-nodes 2 --wasix-nodes 2 --cleanup

# 2. Run a workload
./scripts/v2/run_workload.sh \
  --rpc http://host.docker.internal:8545 \
  --scenario simple \
  --tps 20 \
  --duration 120 \
  --network wasm

# 3. Collect and analyze
./scripts/v2/collect_results.sh
./scripts/v2/analyze_experiment.sh ./experiments/exp_YYYYMMDD_HHMMSS
```

### Remote Deployment

For remote cluster deployment, use the push scripts (`push_linux.sh`, `push_wasix.sh`, `push_metrics.sh`) to upload images to a registry, then deploy using the same pattern as `start_local.sh` but targeting remote hosts.
