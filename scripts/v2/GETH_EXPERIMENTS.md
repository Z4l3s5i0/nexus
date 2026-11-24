### Running Geth experiments

Use the modular runtime experiment from the repository root on Ubuntu/Debian
Linux with Docker Compose. Shared setup installs dependencies and the network
launcher stores generated artifacts under the repository root. Start the
interactive workflow with `bash experiments/run.sh`, or run a customized
runtime stage directly after setup as shown below.

Start with a short Geth-only network, then try Geth plus WASIX:

```sh
EXPERIMENTS_SELECTED=runtime bash experiments/setup.sh
bash experiments/runtime/run.sh --linux-nodes 0 --geth-nodes 1 --wasix-nodes 0 --chain-id 1001 --duration 30 --wait-duration 30 --reset-network
bash experiments/runtime/run.sh --linux-nodes 0 --geth-nodes 1 --wasix-nodes 1 --chain-id 1002 --duration 30 --wait-duration 30 --reset-network
```

Check Lighthouse Engine authentication, validator blocks, both clients' peer
logs, active entries at `http://localhost:9100/api/nodes`, and chain progress
before running a workload. The launcher checks Geth's chain ID and registered
execution peer connections, but these checks alone do not prove consensus or
workload correctness. Run a three-client mix as well before a full sweep.

```sh
bash experiments/runtime/run.sh --linux-nodes 1 --geth-nodes 1 --wasix-nodes 1 --duration 30 --wait-duration 30 --chain-id 1003
```

Sweep entries are `native WASIX Geth`; old two-count entries imply zero Geth.
Node indices and startup order are native, Geth, WASIX. Workload allocation
uses the largest remainder with ties in that order; any positive percentage
selects at least one node. The default sweep includes the original ratios,
ten Geth, and five Geth plus five WASIX. Workload parameters remain unchanged.

The one-time setup prepares the metrics-server image and Geth image before the
runtime module starts. Geth is pinned by digest by the shared network launcher;
native and WASIX image references are also documented in `experiments/README.md`.

Archives contain `metadata.json`, registry records, the stopped metrics-server
database, and any captured Geth exposition. Analysis is not automatically run.
See `GETH_METRICS.md` for unverified metric mappings and instrumentation gaps.
Genesis compatibility, peer interoperability, chain progress, Geth metric
inventory, archive readability and cleanup isolation still require live checks.