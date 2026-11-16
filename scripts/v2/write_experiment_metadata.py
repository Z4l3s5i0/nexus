#!/usr/bin/env python3
"""Capture the experiment configuration and running execution-node identities."""

import argparse
import json
import subprocess
import sys
from datetime import datetime, timezone
from urllib.parse import urlparse
from urllib.request import urlopen


def container_image(service):
    try:
        container = subprocess.check_output(
            ["docker", "compose", "-f", "docker-compose.v2.yml", "ps", "-q", service],
            text=True, timeout=15,
        ).strip()
        if not container:
            raise RuntimeError("container not found")
        data = json.loads(subprocess.check_output(
            ["docker", "inspect", container], text=True, timeout=15,
        ))[0]
        return {"reference": data["Config"]["Image"], "image_id": data["Image"]}
    except (OSError, ValueError, KeyError, subprocess.SubprocessError, RuntimeError) as exc:
        print(f"Warning: cannot inspect image for {service}: {exc}", file=sys.stderr)
        return {"reference": None, "image_id": None}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--chain-id", type=int, required=True)
    parser.add_argument("--linux", type=int, required=True)
    parser.add_argument("--geth", type=int, required=True)
    parser.add_argument("--wasix", type=int, required=True)
    parser.add_argument("--scenario", required=True)
    parser.add_argument("--duration", required=True)
    parser.add_argument("--wait-duration", required=True)
    parser.add_argument("--tps", required=True)
    parser.add_argument("--accounts", required=True)
    parser.add_argument("--percent", required=True)
    parser.add_argument("--targets", default="")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    targets = {int(value) for value in args.targets.split(",") if value}
    try:
        with urlopen("http://127.0.0.1:9100/api/nodes", timeout=5) as response:
            registry = json.load(response)
    except (OSError, ValueError) as exc:
        print(f"Warning: cannot read node registry: {exc}", file=sys.stderr)
        registry = []
    registry_by_port = {}
    for node in registry:
        try:
            port = urlparse(node.get("rpc_url", "")).port
        except ValueError:
            continue
        if port is not None and node.get("chain_id") == args.chain_id:
            registry_by_port[port] = node
    types = ["native"] * args.linux + ["geth"] * args.geth + ["wasix"] * args.wasix
    nodes = []
    for index, kind in enumerate(types):
        registered = registry_by_port.get(8545 + index, {})
        nodes.append({
            "index": index, "type": kind,
            "client": "geth" if kind == "geth" else "wasix-eth",
            "node_id": registered.get("id"), "rpc_port": 8545 + index,
            "registered_rpc_url": registered.get("rpc_url"),
            "metrics_url": registered.get("metrics_url"),
            "workload_target": index in targets,
            "image": container_image(f"el-node-{index}"),
        })
    metadata = {
        "created_at": datetime.now(timezone.utc).isoformat(),
        "chain_id": args.chain_id,
        "counts": {"native": args.linux, "geth": args.geth, "wasix": args.wasix},
        "workload": {"scenario": args.scenario, "duration_seconds": args.duration,
                     "wait_seconds": args.wait_duration, "tps": args.tps,
                     "accounts_per_spammer": args.accounts, "node_percent": args.percent,
                     "target_indices": sorted(targets)},
        "metrics_server_image": container_image("metrics-server"),
        "nodes": nodes,
    }
    with open(args.output, "w", encoding="utf-8") as output:
        json.dump(metadata, output, indent=2)
        output.write("\n")


if __name__ == "__main__":
    main()