#!/usr/bin/env python3
import argparse
import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlparse
from urllib.request import urlopen


REPO_ROOT = Path(__file__).resolve().parents[2]


def container_image(service):
    try:
        container = subprocess.check_output(
            ["docker", "compose", "-f", str(REPO_ROOT / "docker-compose.v2.yml"), "ps", "-q", service],
            text=True,
            timeout=15,
        ).strip()
        if not container:
            return None
        inspected = json.loads(subprocess.check_output(["docker", "inspect", container], text=True, timeout=15))[0]
        return {"reference": inspected["Config"]["Image"], "image_id": inspected["Image"]}
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as exc:
        print(f"Warning: cannot inspect image for {service}: {exc}", file=sys.stderr)
        return None


def registered_nodes(api_url, timeout):
    try:
        with urlopen(api_url.rstrip("/") + "/api/nodes", timeout=timeout) as response:
            return json.load(response)
    except (OSError, ValueError) as exc:
        raise RuntimeError(f"cannot read node registry: {exc}") from exc


def capture(args):
    types = ["native"] * args.linux_nodes
    types += ["geth"] * args.geth_nodes
    types += ["reth"] * args.reth_nodes
    types += ["wasix"] * args.wasix_nodes
    nodes_by_port = {}
    for node in registered_nodes(args.api_url, args.timeout):
        try:
            port = urlparse(node.get("rpc_url", "")).port
        except ValueError:
            continue
        if port is not None and node.get("chain_id") == args.chain_id:
            nodes_by_port[port] = node

    nodes = []
    for index, client_type in enumerate(types):
        port = 8545 + index
        registered = nodes_by_port.get(port, {})
        client = registered.get("client")
        if not client:
            client = {"geth": "geth", "reth": "reth"}.get(client_type, "wasix-eth")
        nodes.append(
            {
                "index": index,
                "type": client_type,
                "client": client,
                "node_id": registered.get("id"),
                "rpc_port": port,
                "rpc_url": f"http://127.0.0.1:{port}",
                "registered_rpc_url": registered.get("rpc_url"),
                "metrics_url": registered.get("metrics_url"),
                "registered": bool(registered),
                "workload_target": True,
                "image": container_image(f"el-node-{index}"),
            }
        )

    metadata = {
        "experiment_id": args.experiment_id,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "chain_id": args.chain_id,
        "counts": {
            "native": args.linux_nodes,
            "geth": args.geth_nodes,
            "reth": args.reth_nodes,
            "wasix": args.wasix_nodes,
        },
        "workload": {
            "scenario": args.workload_type,
            "duration_seconds": args.duration,
            "wait_seconds": args.wait_duration,
            "tps": args.tps,
            "accounts_per_spammer": args.accounts,
        },
        "metrics_server_image": container_image("metrics-server"),
        "nodes": nodes,
    }
    with open(args.output, "w", encoding="utf-8") as output:
        json.dump(metadata, output, indent=2, sort_keys=True)
        output.write("\n")


def main():
    parser = argparse.ArgumentParser(description="Capture run, node, client, and image identities.")
    parser.add_argument("--output", required=True)
    parser.add_argument("--experiment-id", required=True)
    parser.add_argument("--chain-id", type=int, required=True)
    parser.add_argument("--linux-nodes", type=int, required=True)
    parser.add_argument("--geth-nodes", type=int, required=True)
    parser.add_argument("--reth-nodes", type=int, required=True)
    parser.add_argument("--wasix-nodes", type=int, required=True)
    parser.add_argument("--workload-type", required=True)
    parser.add_argument("--duration", type=int, required=True)
    parser.add_argument("--wait-duration", type=int, required=True)
    parser.add_argument("--tps", type=int, required=True)
    parser.add_argument("--accounts", type=int, required=True)
    parser.add_argument("--api-url", default="http://127.0.0.1:9100")
    parser.add_argument("--timeout", type=float, default=5)
    args = parser.parse_args()
    capture(args)


if __name__ == "__main__":
    main()