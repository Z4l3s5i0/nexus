#!/usr/bin/env python3
import argparse
import json
from datetime import datetime, timezone
from pathlib import Path


CLIENTS = {
    "geth": "geth",
    "reth": "reth",
    "nexus-linux": "wasix-eth",
    "nexus-wasix-docker": "wasix-eth",
    "nexus-wasmer": "wasix-eth",
}


def build_metadata(run_id, chain_id, iterations, configs):
    nodes = []
    for config in configs:
        for iteration in range(1, iterations + 1):
            nodes.append(
                {
                    "index": len(nodes),
                    "node_id": f"startup-{config}-{iteration}",
                    "client": CLIENTS[config],
                    "type": config,
                    "variant": config,
                    "registered": False,
                }
            )
    return {
        "experiment_id": run_id,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "chain_id": chain_id,
        "iterations": iterations,
        "configurations": configs,
        "nodes": nodes,
    }


def build_iteration_identity(run_id, chain_id, config, iteration):
    return {
        "experiment_id": run_id,
        "chain_id": chain_id,
        "config": config,
        "iteration": iteration,
        "client": CLIENTS[config],
        "client_variant": config,
        "node_id": f"startup-{config}-{iteration}",
        "network": str(chain_id),
    }


def main():
    parser = argparse.ArgumentParser(description="Write startup run and iteration provenance.")
    parser.add_argument("--scope", choices=("run", "iteration"), required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--chain-id", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--iterations", type=int, default=1)
    parser.add_argument("--configs", default="")
    parser.add_argument("--config", choices=tuple(CLIENTS))
    parser.add_argument("--iteration", type=int, default=1)
    args = parser.parse_args()
    if args.scope == "run":
        configs = [value for value in args.configs.split(",") if value]
        data = build_metadata(args.run_id, args.chain_id, args.iterations, configs)
    else:
        if args.config is None:
            parser.error("--config is required for iteration metadata")
        data = build_iteration_identity(args.run_id, args.chain_id, args.config, args.iteration)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()