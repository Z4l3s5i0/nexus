# User Guide: Multi-Node Blockchain Network Deployment

This guide explains how to use the deployment scripts to start a blockchain network either locally or across multiple machines.

## 1. Prerequisites

- **Docker & Docker Compose**: Installed on all machines.
- **Lighthouse**: Installed on all machines (for Consensus and Validator clients).
- **SSH Access**: For multi-machine deployment.
- **Wasmer CLI**: For running Wasix nodes.
- **Wine**: For running Windows nodes on Linux.

## 2. Local Network Deployment (Single Machine)

The `start_local.sh` script automates the process of creating a network using Docker containers on your local machine.

### Usage
```bash
./scripts/v2/start_local.sh --nodes 4 --linux 50 --windows 25 --wasix 25 --chain-id 12345 --setup
```

*(The `--setup` flag ensures Docker and all required local tools are installed and ready.)*

### Parameters
- `--nodes`: Total number of execution nodes to start.
- `--linux`: Percentage of nodes running the native Linux version.
- `--windows`: Percentage of nodes running the Windows version via Wine.
- `--wasix`: Percentage of nodes running the Wasix version via Wasmer.
- `--chain-id`: Unique ID for the network experiment.
- `--cleanup`: (Optional) Deletes old data directories before starting.

### What it does
1. Generates genesis and configuration for the specified `chain-id`.
2. Generates validator keys and JWT secrets for all nodes.
3. Spawns the requested number of nodes using Docker.
4. Each node includes:
   - An Execution Client (`wasix_eth`).
   - A Consensus Client (Lighthouse Beacon Node).
   - (For 50% of nodes) A Validator Client (Lighthouse).
5. Registers all nodes with the `metrics_server` automatically.

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
./scripts/v2/start_local.sh --nodes 4 --registry your-username --tag v1.0
```

**Remote:**
```bash
./scripts/v2/start_remote.sh --hosts hosts.ini --registry your-username --tag v1.0
```

## 6. Troubleshooting

- **Port Conflicts**: Ensure the ports used by the scripts (8545+, 9002+, etc.) are not occupied.
- **Logs**: Check `docker logs <container_id>` or the `.log` files in the `scripts` directory on remote hosts.
- **Connectivity**: Ensure all nodes can reach the bootnode and the `metrics_server`.
