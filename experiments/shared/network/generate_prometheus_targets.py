#!/usr/bin/env python3
import argparse
import json
from pathlib import Path


NODE_EXPORTER_TYPES = {
    "linux": ("wasix-eth", "native"),
    "wasix": ("wasix-eth", "wasix"),
}
VALID_TYPES = {"linux", "wasix", "geth", "reth"}


def build_target_groups(node_types):
    node_exporter_targets = []
    execution_metrics_targets = []
    for index, node_type in enumerate(node_types):
        if node_type not in VALID_TYPES:
            raise ValueError(f"unsupported execution node type: {node_type}")
        client, variant = NODE_EXPORTER_TYPES.get(node_type, (node_type, node_type))
        service = f"el-node-{index}"
        labels = {
            "client": client,
            "node_index": str(index),
            "node_service": service,
            "runtime_variant": variant,
        }
        execution_metrics_targets.append(
            {
                "targets": [f"{service}:{9050 + index}"],
                "labels": {**labels, "scope": "execution_node"},
            }
        )
        if node_type in NODE_EXPORTER_TYPES:
            node_exporter_targets.append(
                {
                    "targets": [f"{service}:9100"],
                    "labels": {**labels, "scope": "node_container_exporter"},
                }
            )
    return node_exporter_targets, execution_metrics_targets


def write_target_groups(path, targets):
    output = Path(path)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(targets, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description="Generate Prometheus file-discovery targets for node exporters.")
    parser.add_argument("--node-exporter-output", required=True)
    parser.add_argument("--execution-metrics-output", required=True)
    parser.add_argument("--node-types", nargs="+", required=True)
    args = parser.parse_args()
    try:
        node_exporter_targets, execution_metrics_targets = build_target_groups(args.node_types)
    except ValueError as exc:
        parser.error(str(exc))
    write_target_groups(args.node_exporter_output, node_exporter_targets)
    write_target_groups(args.execution_metrics_output, execution_metrics_targets)


if __name__ == "__main__":
    main()