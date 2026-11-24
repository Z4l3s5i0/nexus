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


def capture(args):
    try:
        with urlopen(args.api_url.rstrip("/") + "/api/nodes", timeout=args.timeout) as response:
            registered_nodes = json.load(response)
    except (OSError, ValueError) as exc:
        raise RuntimeError(f"cannot read node registry: {exc}") from exc

    registered = None
    for node in registered_nodes:
        try:
            port = urlparse(node.get("rpc_url", "")).port
        except ValueError:
            continue
        if port == args.rpc_port and node.get("chain_id") == args.chain_id:
            registered = node
            break

    image = None
    try:
        container = subprocess.check_output(
            ["docker", "compose", "-f", str(REPO_ROOT / "docker-compose.v2.yml"), "ps", "-q", "el-node-0"],
            text=True,
            timeout=15,
        ).strip()
        if container:
            inspected = json.loads(subprocess.check_output(["docker", "inspect", container], text=True, timeout=15))[0]
            image = {"reference": inspected["Config"]["Image"], "image_id": inspected["Image"]}
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as exc:
        print(f"Warning: cannot inspect execution-client image: {exc}", file=sys.stderr)

    metadata = {
        "experiment_id": args.experiment_id,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "chain_id": args.chain_id,
        "client": registered.get("client") if registered else args.client_label,
        "client_variant": args.client_variant,
        "sweep": {
            "transaction_counts": [int(value) for value in args.counts.split(",")],
            "blocks_per_count": args.blocks_per_count,
            "max_attempts": args.max_attempts,
        },
        "node_id": registered.get("id") if registered else None,
        "registered": registered is not None,
        "rpc_url": f"http://127.0.0.1:{args.rpc_port}",
        "registered_rpc_url": registered.get("rpc_url") if registered else None,
        "metrics_url": registered.get("metrics_url") if registered else None,
        "image": image,
        "nodes": [
            {
                "index": 0,
                "node_id": registered.get("id") if registered else None,
                "client": registered.get("client") if registered else args.client_label,
                "variant": args.client_variant,
                "registered": registered is not None,
            }
        ],
    }
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description="Capture empty-block run and client identity.")
    parser.add_argument("--output", required=True)
    parser.add_argument("--experiment-id", required=True)
    parser.add_argument("--chain-id", type=int, required=True)
    parser.add_argument("--client-label", required=True)
    parser.add_argument("--client-variant", required=True)
    parser.add_argument("--counts", required=True)
    parser.add_argument("--blocks-per-count", type=int, required=True)
    parser.add_argument("--max-attempts", type=int, required=True)
    parser.add_argument("--rpc-port", type=int, default=8545)
    parser.add_argument("--api-url", default="http://127.0.0.1:9100")
    parser.add_argument("--timeout", type=float, default=5)
    args = parser.parse_args()
    capture(args)


if __name__ == "__main__":
    main()