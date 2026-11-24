# Modular experiment suite

`experiments/run.sh` is the primary entry point for the five experiment modules. It prompts for a selection, runs the shared setup once, then dispatches modules sequentially in the order entered. Select `6` (or `all`) for the standard order: runtime, empty-block, startup, compliance, and size comparison. Select `7` to enter evaluation mode and choose which saved experiment results to reevaluate without rerunning workloads or setup.

```bash
bash experiments/run.sh
```

Enter a comma-separated selection such as `3,1,5` to run startup, runtime, then size comparison. For reevaluation, enter `7` and then select `1–5`, a comma-separated list, or `6` for all module types. Evaluation regenerates reports for every saved run directory under the selected modules in `experiment-results/`; the saved samples and measurements are reused, and no clients, workloads, Hive suites, or source measurements are rerun. Missing/incomplete results are reported as errors while other selected runs are still checked. `bash experiments/run.sh --help` displays the menu. `experiments/setup.sh` can also be run directly; without the runner’s selection it prepares dependencies for all five modules.

Each experiment run writes result artifacts to `experiment-results/<module>/<run-id>/`. The runner exports a common `EXPERIMENT_RUN_ID` and output root to all selected modules. Generated result directories are ignored by Git; source scripts under `experiments/` are tracked.

Every invocation of `experiments/run.sh` also creates `experiment-results/logs/<session-id>/`. `runner.log` records the menu, selected modules, configuration messages, dispatch messages, and terminal output. A run invocation records shared setup output and its exit status in the single `setup.log`; each selected experiment has its own subfolder and `run-<run-id>.log`, so one module's output cannot be mistaken for another's. Evaluation-only runs write each report invocation to that experiment's `evaluation-<number>.log` and do not create a setup log. Standard output and errors remain visible in the terminal while being captured; command arguments are intentionally omitted from log headers to avoid recording secrets such as the empty-block private key. Detailed client, workload, Hive, and collector logs remain in their experiment result folders.

To keep a suite running after an SSH or terminal connection is lost, choose detached execution when starting it:

```bash
bash experiments/run.sh --detach
```

The runner still collects the experiment selection and configuration interactively, then starts the suite in its own session before shared setup begins. It prints a session ID and commands to follow or stop that session; the same commands can be run later with `bash experiments/run.sh --attach <session-id>` to reattach to its console log or `bash experiments/run.sh --stop <session-id>` to stop the suite and its child processes. Without `--detach`, execution remains attached to the current terminal as before.

## Interactive run configuration

After selecting experiments, the runner displays each selected module's proposed defaults in the same order. Press Enter (or answer `n`) to accept that module's defaults, or answer `y` to customize its settings; every custom field also accepts Enter to keep its displayed default. The prompts and validation finish before the shared setup runs, and evaluation mode (`7`) does not ask for run configuration.

| Module | Configurable run settings |
|---|---|
| Runtime | Native, WASIX, Geth, and Reth node counts (the client mix); one or more Contender workload scenarios; spammer accounts and target TPS per workload node; workload and collection-wait durations; chain ID; runtime iterations; whether generated network state may be cleared. Every workload/iteration pair is saved under a distinct run ID. |
| Empty block | Client, chain ID, transaction-count groups, samples per group, maximum attempts, receipt/block timeouts, RPC poll interval, and network reset permission. A nonzero transaction group requires a funded disposable test key; if `EB_PRIVATE_KEY` is not set, the runner prompts for it without echoing input. The key is passed only to the empty-block run, not shared setup. |
| Startup | Client configurations, iterations per configuration, chain ID, readiness timeout, system sampling interval, client images, optional NEXUS WASIX module, and Wasmer executable. |
| Compliance | Custom Hive clients, Hive image, parallelism, per-client timeout, and whether to build or reuse existing client/Hive images. |
| Size | Comma-separated `LABEL=DIR` source trees and `LABEL=FILE` binaries; paths containing commas are not supported by this prompt format. |

The runtime scenario field accepts comma-separated Contender scenario paths such as `exp/uber,exp/counter,exp/dota`; each workload scenario runs separately for every requested runtime iteration, with its own network lifecycle, metadata, and result directory. Startup and empty-block iteration settings retain their module-specific meanings (repetitions per client configuration and samples per transaction-count group respectively).

## Modules

| Module | Entry point | Purpose and outputs |
|---|---|---|
| Runtime | `experiments/runtime/run.sh` | Starts a local mixed-client network, runs Contender workloads, records RPC/Prometheus samples, host/container metrics, structured logs, and reports availability by node/client. |
| Empty block | `experiments/empty-block/run.sh` | Runs an exact transaction-count sweep, records actual block transaction counts and execution times, and writes `samples.csv`, raw events, derived samples, and coverage reports. |
| Startup | `experiments/startup/run.sh` | Repeats isolated client starts, measures time to the first successful `eth_chainId` response, samples system resources, and reports per-configuration statistics. |
| Compliance | `experiments/compliance/run.sh` | Builds the custom Hive image and runs `ethereum/engine` for the selected `wasix-r-eth` and/or `wasix-w-eth` client. Workspaces, command lines, and per-client logs are retained. |
| Size | `experiments/size/run.sh` | Uses upstream `scc` to measure source-code lines and records compiled binary bytes. Missing source trees or binaries are reported as unavailable. |

Modules own their run lifecycle and experiment-specific collection/reporting scripts. Runtime and empty-block share the internal launcher and config/monitoring assets in `experiments/shared/network/`; neither module delegates to a legacy `scripts/v2` entry point.

### Runtime

The runtime module accepts `--linux-nodes`, `--wasix-nodes`, `--geth-nodes`, `--reth-nodes`, workload duration/TPS, and other local-run options; see `bash experiments/runtime/run.sh --help`. Its default network is two native and two WASIX NEXUS nodes. It builds from the local source tree and uses the metrics-server image prepared by the one-time setup.

Prometheus keeps host node-exporter data under `job="node-exporter-host",scope="host"`. Native and WASIX NEXUS containers run their own node-exporter endpoints, discovered as `job="node-exporter-containers"` with `node_index`, `node_service`, `client`, `runtime_variant`, and `scope="node_container_exporter"` labels. Every execution client’s own Prometheus endpoint is scraped as `job="execution-node-metrics",scope="execution_node"`; the target port is `9050 + node_index`, matching the generated client configuration. cAdvisor scrapes every execution-node container—including Geth and Reth—and adds `node_index`, `node_service`, `client`, `runtime_variant`, `container_name`, and `scope="container"` to container resource series. For example, query `container_memory_working_set_bytes{job="cadvisor",node_index="0"}`, `node_cpu_seconds_total{job="node-exporter-containers",node_index="0"}`, or a client metric with `job="execution-node-metrics",node_index="0"` in Prometheus at `http://localhost:9091`.

### Empty block

The empty-block module defaults to the `wasix` client and transaction counts `0,1,5,10,20,50,100`. Nonzero counts require a funded local test key supplied with `EB_PRIVATE_KEY` or `--private-key`; use a disposable account. `--reset-network` explicitly permits removal of pre-existing generated `data_v2/` and `startup_v2/` state. On successful archival, the module removes only its generated network state so the next selected module can start cleanly; failed runs retain state for diagnosis.

### Startup

The default startup configurations are Geth, Reth, NEXUS native, and NEXUS in Docker/WASIX. NEXUS Docker configurations use local `wasix-eth-linux:latest` and `wasix-eth-wasix:latest` images; setup reuses those images when present and otherwise builds them from the local `target/` binaries. A host Wasmer configuration is included only when `NEXUS_WASIX_WASM` points to a module and Wasmer is available, or when selected explicitly with `--configs nexus-wasmer`. `--timeout`, `--iterations`, `--configs`, image paths, and other options are listed by `bash experiments/startup/run.sh --help`.

The measured endpoint is the first successful `eth_chainId` response. Geth, Reth, and host-Wasmer data initialization is outside the timed interval; initialization for the WASIX-in-Docker configuration remains part of its timed launch, matching the prior benchmark behavior. Image references and IDs are recorded for reproducibility.

### Compliance

The compliance module builds `z4l3s5i0/wasix-eth-linux:latest` and/or `z4l3s5i0/wasix-eth-wasix:latest`, then builds `hive-custom:latest` from `hive/hive.dockerfile`. The Hive command uses `--sim ethereum/engine`, the selected client, `--sim.parallelism`, and `--docker.pull=false`, following `hive/README.md`. Per-client results and logs are kept separately; any client failure makes the compliance stage fail.

### Size comparison

The source measurement uses Boyter's upstream `scc` v4.1.0 and sums its machine-readable `Code` counts; `.git`, dependency/vendor, build-output, generated, and test paths are excluded. The default source tree covers `wasix_eth/`; default binary candidates are `wasix_eth/target/release/wasix_eth` and `wasix_eth/target/wasm32-wasmer-wasi/release/wasix_eth.wasi.wasm`. Setup builds these NEXUS binaries once when needed for a selected module. Provide additional or different paths with repeated `--source LABEL=DIR` and `--binary LABEL=FILE` options, or use `GETH_SOURCE_DIR`, `RETH_SOURCE_DIR`, `WASIX_ETH_NATIVE_BINARY`, `WASIX_ETH_WASIX_BINARY`, `GETH_BINARY`, and `RETH_BINARY` environment variables. The report lists missing inputs as unavailable, never as size zero. Docker image size is intentionally omitted because image layers include runtime dependencies and are not comparable to compiled client size.

## Dependency and setup notes

`experiments/setup.sh` supports Ubuntu/Debian Linux. It installs or verifies the shared command-line tools (`curl`, `wget`, `git`, `jq`, `openssl`, Python 3, and `lld`). When the size module is selected, setup installs or verifies upstream `scc` v4.1.0 from its official Linux release, validating the pinned SHA-256 checksum without requiring Go. When a selected module needs client binaries, setup installs Rust stable via `rustup`, `cargo-wasix`, and `wasixcc` v0.4.7; `wasixccenv` installs the compiler executables into the user Cargo bin directory and downloads the LLVM toolchain and WASIX sysroot before any `cargo +stable wasix build --release` invocation. Setup also installs or verifies Wasmer 7.4.2 under the current user's Wasmer directory; these binary directories are added to the suite process `PATH`. Docker and Compose v2 are installed/verified only when a selected stage needs them; the Docker daemon must be usable by the current user. For runtime, empty-block, and startup selections, setup pulls shared external images including cAdvisor and builds `wasix-eth-metrics:latest` once. Startup-specific NEXUS images use cached local images when available and are otherwise built from `wasix_eth/Dockerfile.linux` and `wasix_eth/Dockerfile.wasix` using the local `target/` artifacts; setup does not pull `wasix-eth-wasix` from Docker Hub. When native/WASIX target binaries are absent, setup builds them once. Hive client and Hive runner images are prepared by the compliance module.

The setup script requires network access to install the pinned `scc` release, Rust stable, `cargo-wasix`, `wasixcc` v0.4.7 and its LLVM/sysroot components, and Wasmer 7.4.2, and to pull Docker images/build the Hive runner when selected. Host Wasmer startup measurements additionally require a compatible WASIX module; external binary comparisons require the respective files. Docker-backed stages require a running daemon accessible to the current user.

## Metric coverage

| Source | Metrics | Experiments |
|---|---|---|
| Node-level Prometheus | Sync status, head block, peers, blocks imported, transactions committed, mempool size, rejected transactions, gossip messages, P2P bytes, RPC requests | Runtime, empty block |
| RPC-derived | Node status/RPC availability, latest block, chain ID, peer count, gas price, syncing status | Runtime, empty block |
| System-level | CPU utilization, memory usage, disk I/O, network throughput, load averages, context switches | Runtime, empty block, startup |
| Log-based | Transaction hash, sender, gas used, elapsed time, block number, transaction count, peer ID, download type, TPS, commit-to-submit ratio | Runtime, empty block |

System values are labeled `host`, `container`, or `process` according to attribution. Host node-exporter metrics are never labeled as container measurements; cAdvisor provides per-container CPU, memory, disk, and network metrics with node/client labels, while the NEXUS containers' own node-exporter endpoints provide node-scoped OS metrics. Load averages and context switches stay host-scoped rather than being copied to each client. Missing values are omitted and shown as unavailable/partial in reports, never substituted with zero. A real observed zero (for example, an empty block’s transaction count) remains a valid measurement.

Prometheus exposition names are retained raw and are not normalized unless client semantics are verified. The current Geth inventory and equivalence mappings are explicitly unverified in `scripts/v2/GETH_METRICS.md`; raw-series presence does not claim metric equivalence. Structured `[EXP]` log fields are currently available from the custom NEXUS clients; clients without those events show the corresponding log fields as unavailable. Empty-block TPS is derived from block transactions and block timestamp intervals; its commit-to-submit ratio uses receipt-confirmed test hashes.

## Implementation status and remaining gaps

### Implemented

- Interactive ordered dispatch, one-time selection-aware setup, shared run IDs, and isolated output folders.
- Evaluation-only dispatch through `experiments/run.sh` to regenerate selected modules' reports from saved runs without setup or workload execution.
- Runtime lifecycle, RPC status samples, raw Prometheus retention, secure metric ingestion for derived/system/log samples, host/container system collection, structured log parsing, and per-node availability reports.
- Separate host, per-node NEXUS node-exporter, and cAdvisor scrape targets, with execution-container labels preserved in Prometheus time series.
- Per-client execution metrics endpoints scraped with indexed ports and `node_index`, `client`, `runtime_variant`, and `scope` labels.
- Empty-block transaction submission, receipt polling, actual block counts, millisecond execution-time log parsing, and labeled block/log/system outputs.
- Repeated startup benchmarks, readiness timing, per-process/container and host-scoped metrics, image identity, and summary statistics.
- One-shot Hive `ethereum/engine` execution for custom clients with per-client logs and exit statuses.
- Separate `scc` source-code-line and compiled-binary size measurement scripts with unavailable-input reporting.

### Remaining limitations

- Docker-backed network, Hive, and startup end-to-end runs have not been performed as part of this code change; validate them on a Linux host with Docker available.
- cAdvisor requires privileged access to the Linux Docker host and its cgroups; validate container series on the target Docker environment, especially when using Docker Desktop/WSL.
- Prometheus metric equivalence across all clients is not verified. Geth mappings and semantics remain raw/unverified, and some client-specific counters are unavailable.
- Structured log coverage varies by client; peer/download events, transaction senders, and gas/timing fields are not guaranteed for non-NEXUS clients.
- Per-client network throughput is available from Docker stats for containers; for host Wasmer, host throughput is measured but process-level network attribution is unavailable.
- External Geth/Reth source trees and compiled binaries are not vendored. Supply paths to compare them; the report will preserve missing values explicitly.
- Host-scoped load averages/context switches and telemetry collection overhead are not equivalent to client-attributed measurements.

## Validation

```bash
python3 -m unittest discover -s experiments -p 'test*.py'
cargo +stable test --manifest-path metrics_server/Cargo.toml
```