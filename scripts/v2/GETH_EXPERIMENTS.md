### Running Geth experiments

Run the scripts from a dedicated experiment working directory on Linux with
Docker Compose, Python 3, curl, OpenSSL and the existing v2 dependencies.
The working directory must not contain unrelated `data_v2`, `startup_v2`,
`logs_dump` or `docker-compose.v2.yml`: the launcher and cleanup own these
paths. `run_experiment.sh` also runs the existing `setup.sh` and uses `sudo`.

Start with a short Geth-only network, then try Geth plus WASIX:

```sh
bash /path/to/scripts/v2/start_local.sh --linux-nodes 0 --geth-nodes 1 --wasix-nodes 0 --chain-id 1001 --cleanup --registry docker.io/z4l3s5i0
bash /path/to/scripts/v2/cleanup.sh
bash /path/to/scripts/v2/start_local.sh --linux-nodes 0 --geth-nodes 1 --wasix-nodes 1 --chain-id 1002 --cleanup --registry docker.io/z4l3s5i0
```

Check Lighthouse Engine authentication, validator blocks, both clients' peer
logs, active entries at `http://localhost:9100/api/nodes`, and chain progress
before running a workload. The launcher checks Geth's chain ID and registered
execution peer connections, but these checks alone do not prove consensus or
workload correctness. Run a three-client mix as well before a full sweep.

```sh
bash /path/to/scripts/v2/run_experiment.sh --linux-nodes 1 --geth-nodes 1 --wasix-nodes 1 --duration 30 --wait-duration 30 --workload-percent 100 --chain-id 1003
bash /path/to/scripts/v2/run_experiments_loop.sh --ratios '3 4 3; 0 5 5'
```

Sweep entries are `native WASIX Geth`; old two-count entries imply zero Geth.
Node indices and startup order are native, Geth, WASIX. Workload allocation
uses the largest remainder with ties in that order; any positive percentage
selects at least one node. The default sweep includes the original ratios,
ten Geth, and five Geth plus five WASIX. Workload parameters remain unchanged.

The launcher rebuilds the local metrics-server image even when execution images
are pulled from a registry. Use `--metrics-image IMAGE` on the launcher or
single-experiment script only when an explicitly chosen prebuilt image includes
the required scraper changes. Geth is pinned by digest in `start_local.sh`;
native and WASIX image references remain as before.

Archives contain `metadata.json`, registry records, the stopped metrics-server
database, and any captured Geth exposition. Analysis is not automatically run.
See `GETH_METRICS.md` for unverified metric mappings and instrumentation gaps.
Genesis compatibility, peer interoperability, chain progress, Geth metric
inventory, archive readability and cleanup isolation still require live checks.