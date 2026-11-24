#!/usr/bin/env python3
import argparse
import json
from collections import Counter
from pathlib import Path


CONTAINER_SYSTEM_FIELDS = {
    "cpu": ["system_container_cpu_utilization_percent"],
    "memory": ["system_container_memory_usage_bytes"],
    "disk_io": [
        "system_container_disk_read_bytes",
        "system_container_disk_write_bytes",
        "system_container_disk_read_bytes_per_second",
        "system_container_disk_write_bytes_per_second",
    ],
    "network_throughput": [
        "system_container_network_receive_bytes_per_second",
        "system_container_network_transmit_bytes_per_second",
    ],
}
HOST_SYSTEM_FIELDS = {
    "cpu": ["system_cpu_utilization_percent"],
    "memory": ["system_memory_used_bytes"],
    "disk_io": ["system_disk_read_bytes_per_second", "system_disk_write_bytes_per_second"],
    "network_throughput": [
        "system_network_receive_bytes_per_second",
        "system_network_transmit_bytes_per_second",
    ],
    "load_averages": ["system_load_1m", "system_load_5m", "system_load_15m"],
    "context_switches": ["system_context_switches_total", "system_context_switches_per_second"],
}


def read_jsonl(path):
    if not path.is_file():
        return []
    rows = []
    with path.open(encoding="utf-8") as source:
        for line in source:
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    return rows


def system_coverage(metadata, samples):
    samples_by_node = {}
    for sample in samples:
        samples_by_node.setdefault(sample["node_id"], set()).add(sample["name"])

    def fields_status(names, expected):
        status = {}
        for field, candidates in expected.items():
            observed = sum(name in names for name in candidates)
            if observed == len(candidates):
                status[field] = "observed"
            elif observed:
                status[field] = "partial"
            else:
                status[field] = "unavailable"
        return status

    nodes = []
    for node in metadata.get("nodes", []):
        node_id = node.get("node_id") or f"el-node-{node['index']}"
        nodes.append(
            {
                "node_id": node_id,
                "client": node.get("client", "unavailable"),
                "runtime_variant": node.get("variant") or node.get("type"),
                "scope": "container",
                "fields": fields_status(samples_by_node.get(node_id, set()), CONTAINER_SYSTEM_FIELDS),
            }
        )
    return {
        "nodes": nodes,
        "host": {
            "scope": "host",
            "fields": fields_status(samples_by_node.get("host", set()), HOST_SYSTEM_FIELDS),
        },
        "note": "Host-only values are not copied to client nodes; unavailable readings are omitted, not encoded as zero.",
    }


def build_report(output_dir):
    output_dir = Path(output_dir)
    metadata = json.loads((output_dir / "metadata.json").read_text(encoding="utf-8"))
    system_samples = read_jsonl(output_dir / "system_metric_samples.jsonl")
    log_samples = read_jsonl(output_dir / "log_metric_samples.jsonl")
    log_coverage_path = output_dir / "log_metrics_coverage.json"
    log_coverage = json.loads(log_coverage_path.read_text(encoding="utf-8")) if log_coverage_path.is_file() else {}
    node_coverage_path = output_dir / "node_metric_coverage.json"
    node_coverage = json.loads(node_coverage_path.read_text(encoding="utf-8")) if node_coverage_path.is_file() else {}
    system_status = system_coverage(metadata, system_samples)
    database_available = (output_dir / "metrics_server_data" / "metrics.redb").is_file()
    counts = Counter((sample["source"], sample["client"], sample["node_id"]) for sample in system_samples + log_samples)
    report = {
        "experiment_id": metadata["experiment_id"],
        "chain_id": metadata["chain_id"],
        "nodes": metadata["nodes"],
        "local_samples": {
            "system": len(system_samples),
            "log": len(log_samples),
            "by_source_client_node": [
                {"source": key[0], "client": key[1], "node_id": key[2], "count": count}
                for key, count in sorted(counts.items())
            ],
        },
        "log_coverage": log_coverage.get("requested_fields", {}),
        "node_metric_coverage": node_coverage,
        "system_coverage": system_status,
        "metrics_database_archived": database_available,
        "notes": [
            "Raw Prometheus and JSON-RPC observations are stored in the metrics-server database with experiment, client, and node identity.",
            "System samples distinguish host-scoped values from per-container values; missing readings are omitted rather than written as zero.",
            "Log coverage depends on structured [EXP] events emitted by each client; unavailable fields are listed explicitly.",
        ],
    }
    (output_dir / "report.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    lines = [
        f"# Runtime experiment {metadata['experiment_id']}",
        "",
        f"- Chain ID: `{metadata['chain_id']}`",
        f"- Nodes: `{len(metadata['nodes'])}`",
        f"- Locally archived system samples: `{len(system_samples)}`",
        f"- Locally archived log-derived samples: `{len(log_samples)}`",
        "",
        "## Log field availability",
        "",
    ]
    if log_coverage.get("requested_fields"):
        lines.extend(f"- `{field}`: {status}" for field, status in sorted(log_coverage["requested_fields"].items()))
    else:
        lines.append("- Structured log events were unavailable; no log-derived values were substituted.")
    lines.extend(["", "## Node metric availability", ""])
    for node in node_coverage.get("nodes", []):
        rpc_status = ", ".join(
            f"{field}={measurement['status']}"
            for field, measurement in sorted(node["rpc_derived"].items())
        )
        lines.append(
            f"- `{node['node_id']}` (`{node['client']}`): RPC `{rpc_status}`; "
            f"Prometheus `{node['prometheus']['status']}` ({len(node['prometheus']['raw_series_names'])} raw names; equivalence not asserted)."
        )
    if not node_coverage.get("nodes"):
        lines.append("- Node metrics could not be queried; see `node_metric_coverage.json` for details.")
    if node_coverage.get("collection_error"):
        lines.append(f"- Metrics API query error: {node_coverage['collection_error']}")
    lines.extend(["", "## System metric availability", ""])
    for node in system_status["nodes"]:
        fields = ", ".join(f"{field}={status}" for field, status in sorted(node["fields"].items()))
        lines.append(
            f"- Container `{node['node_id']}` (`{node['client']}`, variant `{node['runtime_variant']}`): {fields}."
        )
    host_fields = ", ".join(
        f"{field}={status}" for field, status in sorted(system_status["host"]["fields"].items())
    )
    lines.append(f"- Host-scoped: {host_fields}.")
    if database_available:
        lines.extend(["", "RPC and raw Prometheus samples are in `metrics_server_data/metrics.redb`."])
    else:
        lines.extend(["", "The metrics-server database was not archived; see collector stderr and coverage files."])
    (output_dir / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    return report


def main():
    parser = argparse.ArgumentParser(description="Summarize runtime experiment outputs.")
    parser.add_argument("--output-dir", required=True)
    args = parser.parse_args()
    build_report(args.output_dir)


if __name__ == "__main__":
    main()