# User Guide: Multi-Node Blockchain Network Deployment

This guide explains how to run the modular local experiment suite and links to the remaining remote deployment workflow.

## 1. Prerequisites

- **Docker & Docker Compose**: Installed on all machines.
- **Lighthouse**: Installed on all machines (for Consensus and Validator clients).
- **SSH Access**: For multi-machine deployment.
- **Wasmer CLI**: For running Wasix nodes.
- **Wine**: For running Windows nodes on Linux.

## 2. Local Experiments (Single Machine)

Use the experiment selector to install shared prerequisites once and run one or more experiment modules:

### Usage
```bash
bash experiments/run.sh
```

For runtime network and workload options, see:

```bash
bash experiments/runtime/run.sh --help
```

Runtime and empty-block share the private launcher and configuration assets in `experiments/shared/network/`; start them through their experiment modules rather than invoking the internal launcher directly.

## 3. Multi-Machine Network Deployment

The `start_remote.sh` script coordinates deployment across different physical or virtual machines via SSH.

### Usage
```bash
./scripts/v2/start_remote.sh --hosts hosts.ini --chain-id 12345 --metrics http://<metrics-server-ip>:9100 --setup
```

*(The `--setup` flag will automatically install Docker and dependencies on the remote machines if they aren't already present.)*

### Hosts Configuration (`hosts.ini`)
Create a file listing the target machines and their intended node types:
```ini
[nodes]
192.168.1.10 type=linux validator=true
192.168.1.11 type=windows validator=false
192.168.1.12 type=wasix validator=true
192.168.1.13 type=linux validator=false
```

### What it does
1. Connects to each host via SSH.
2. Synchronizes genesis and configuration files.
3. Starts the appropriate node stack on each machine.
4. Ensures the `metrics_server` can reach all nodes.

## 4. Monitoring

Once the network is started, you can monitor it via Grafana:
1. Open `http://localhost:3000` (or your metrics server IP).
2. Use the provided dashboards to view network health, execution efficiency, and consensus status.

## 5. Docker Registry Support (Optional)

Instead of building images locally on every deployment, you can build them once, push them to a registry, and have all nodes pull them.

### Building and Pushing Images
Use the `push_images.sh` script:
```bash
./scripts/v2/push_images.sh --registry your-username --tag v1.0
```

### Deploying from Registry
Use the `--registry` and `--tag` flags in the deployment scripts:

**Local:**
```bash
bash experiments/run.sh
```

**Remote:**
```bash
./scripts/v2/start_remote.sh --hosts hosts.ini --registry your-username --tag v1.0
```

## 6. Troubleshooting

- **Port Conflicts**: Ensure the ports used by the scripts (8545+, 9002+, etc.) are not occupied.
- **Logs**: Check `docker logs <container_id>` or the `.log` files in the `scripts` directory on remote hosts.
- **Connectivity**: Ensure all nodes can reach the bootnode and the `metrics_server`.
