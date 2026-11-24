#!/usr/bin/env python3
import argparse
import json
import sys
from collections import Counter
from pathlib import Path
from urllib.error import URLError
from urllib.request import urlopen


RPC_METRICS = {
    "node_status": "ethereum_rpc_up",
    "latest_block": "ethereum_latest_block",
    "chain_id": "ethereum_chain_id",
    "peer_count": "ethereum_peer_count",
    "gas_price": "ethereum_gas_price",
    "syncing_status": "ethereum_syncing",
}


def summarize_node_metrics(metadata, samples, experiment_id):
    current_samples = [sample for sample in samples if sample.get("experiment_id") == experiment_id]
    counts = Counter(sample.get("source", "unknown") for sample in current_samples)
    nodes = []
    for node in metadata.get("nodes", []):
        node_id = node.get("node_id") or f"el-node-{node['index']}"
        node_samples = [sample for sample in current_samples if sample.get("node_id") == node_id]
        rpc_names = {
            sample.get("name")
            for sample in node_samples
            if sample.get("source") in ("Derived", "JsonRpc")
        }
        prometheus_names = sorted(
            {
                sample.get("name")
                for sample in node_samples
                if sample.get("source") == "PrometheusScrape" and sample.get("name")
            }
        )
        nodes.append(
            {
                "node_id": node_id,
                "client": node.get("client", "unavailable"),
                "runtime_variant": node.get("variant") or node.get("type"),
                "registered": node.get("registered", False),
                "rpc_derived": {
                    field: {
                        "metric": metric_name,
                        "status": "observed" if metric_name in rpc_names else "unavailable",
                    }
                    for field, metric_name in RPC_METRICS.items()
                },
                "prometheus": {
                    "status": "raw_series_observed" if prometheus_names else "unavailable",
                    "raw_series_names": prometheus_names,
                    "normalized_equivalence": "not_asserted",
                },
            }
        )
    return {
        "experiment_id": experiment_id,
        "sample_counts_by_source": dict(sorted(counts.items())),
        "nodes": nodes,
        "notes": [
            "RPC fields are marked observed only when the corresponding derived sample exists for this run and node.",
            "Prometheus names are retained as exposed; raw-series presence does not assert equivalent client semantics.",
            "Missing observations are unavailable, not zero.",
        ],
    }


def collect(api_url, metadata_path, output_path, experiment_id, timeout):
    metadata = json.loads(Path(metadata_path).read_text(encoding="utf-8"))
    summary = summarize_node_metrics(metadata, [], experiment_id)
    try:
        with urlopen(api_url.rstrip("/") + "/api/metrics/latest", timeout=timeout) as response:
            samples = json.load(response)
        summary = summarize_node_metrics(metadata, samples, experiment_id)
    except (OSError, ValueError, URLError) as exc:
        summary["collection_error"] = str(exc)
        print(f"Could not query latest metrics: {exc}", file=sys.stderr)
    Path(output_path).write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description="Record RPC and raw Prometheus coverage for each runtime node.")
    parser.add_argument("--metadata", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--experiment-id", required=True)
    parser.add_argument("--api-url", default="http://127.0.0.1:9100")
    parser.add_argument("--timeout", type=float, default=10)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    collect(args.api_url, args.metadata, args.output, args.experiment_id, args.timeout)


if __name__ == "__main__":
    main()