#!/usr/bin/env python3
import argparse
import csv
import json
import statistics
from pathlib import Path

RUNTIME_HELPERS = Path(__file__).resolve().parents[1] / "runtime"
import sys

sys.path.insert(0, str(RUNTIME_HELPERS.parent))
from runtime.report import read_jsonl, system_coverage


def build_report(output_dir):
    output_dir = Path(output_dir)
    metadata = json.loads((output_dir / "metadata.json").read_text(encoding="utf-8"))
    run_status_path = output_dir / "run_status.json"
    run_status = json.loads(run_status_path.read_text(encoding="utf-8")) if run_status_path.is_file() else {"status": 0}
    block_coverage_path = output_dir / "block_metric_coverage.json"
    block_coverage = json.loads(block_coverage_path.read_text(encoding="utf-8")) if block_coverage_path.is_file() else {}
    log_coverage_path = output_dir / "log_metrics_coverage.json"
    log_coverage = json.loads(log_coverage_path.read_text(encoding="utf-8")) if log_coverage_path.is_file() else {}
    node_coverage_path = output_dir / "node_metric_coverage.json"
    node_coverage = json.loads(node_coverage_path.read_text(encoding="utf-8")) if node_coverage_path.is_file() else {}
    system_samples = read_jsonl(output_dir / "system_metric_samples.jsonl")
    log_samples = read_jsonl(output_dir / "log_metric_samples.jsonl")
    derived_samples = read_jsonl(output_dir / "derived_metric_samples.jsonl")
    rows = []
    samples_path = output_dir / "samples.csv"
    if samples_path.is_file():
        with samples_path.open(newline="", encoding="utf-8") as source:
            rows = [row for row in csv.DictReader(source) if row.get("block_number")]

    grouped = {}
    for row in rows:
        target = row.get("target_tx_count", "unknown")
        summary = grouped.setdefault(target, {"blocks": 0, "matched": 0, "actual_tx_counts": [], "execution_times_ms": []})
        summary["blocks"] += 1
        summary["matched"] += row.get("matched") == "true"
        try:
            summary["actual_tx_counts"].append(int(row["actual_tx_count"]))
        except (KeyError, TypeError, ValueError):
            pass
        try:
            if row.get("execution_time_ms"):
                summary["execution_times_ms"].append(float(row["execution_time_ms"]))
        except ValueError:
            pass

    for summary in grouped.values():
        times = summary.pop("execution_times_ms")
        summary["execution_time_ms"] = {
            "observed": bool(times),
            "mean": statistics.mean(times) if times else None,
        }
    sweep = metadata.get("sweep", {})
    expected_samples = sweep.get("blocks_per_count", 0)
    for target in sweep.get("transaction_counts", []):
        summary = grouped.setdefault(
            str(target),
            {"blocks": 0, "matched": 0, "actual_tx_counts": [], "execution_time_ms": {"observed": False, "mean": None}},
        )
        summary["expected_matches"] = expected_samples
        summary["missing_matches"] = max(0, expected_samples - summary["matched"])

    coverage = {
        "run_status": run_status,
        "experiment_id": metadata["experiment_id"],
        "chain_id": metadata["chain_id"],
        "client": metadata["client"],
        "client_variant": metadata.get("client_variant"),
        "node_id": metadata.get("node_id") or "el-node-0",
        "registered": metadata.get("registered", False),
        "sample_count": len(rows),
        "by_target_transaction_count": grouped,
        "block_metric_coverage": block_coverage.get("fields", {}),
        "log_metric_coverage": log_coverage.get("requested_fields", {}),
        "node_metric_coverage": node_coverage.get("nodes", []),
        "system_metric_coverage": system_coverage(metadata, system_samples),
        "local_metric_samples": {
            "system": len(system_samples),
            "log": len(log_samples),
            "derived_block": len(derived_samples),
        },
        "notes": [
            "Block transaction counts and timestamps come from JSON-RPC block responses.",
            "Execution duration, transaction gas/sender, and peer/download events are included only when structured client logs provide them.",
            "Raw Prometheus names are preserved; semantic equivalence is not inferred.",
            "Unavailable observations are reported as unavailable, not zero.",
        ],
    }
    database_available = (output_dir / "metrics_server_data" / "metrics.redb").is_file()
    coverage["metrics_database_archived"] = database_available
    (output_dir / "report.json").write_text(json.dumps(coverage, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    lines = [
        f"# Empty-block experiment {metadata['experiment_id']}",
        "",
        f"- Client: `{metadata['client']}`",
        f"- Client variant: `{metadata.get('client_variant', 'unavailable')}`",
        f"- Node: `{coverage['node_id']}` (registered: `{metadata.get('registered', False)}`)",
        f"- Chain ID: `{metadata['chain_id']}`",
        f"- Run exit status: `{run_status.get('status', 0)}`",
        f"- Block samples: `{len(rows)}`",
        "",
        "## Transaction-count sweep",
        "",
    ]
    if grouped:
        for target, summary in sorted(grouped.items(), key=lambda item: int(item[0])):
            lines.append(
                f"- Target `{target}`: {summary['matched']}/{summary.get('expected_matches', summary['blocks'])} exact matches "
                f"({summary.get('missing_matches', 0)} missing; {summary['blocks']} blocks observed); "
                f"observed block transaction counts `{summary['actual_tx_counts']}`; "
                f"execution time `{summary['execution_time_ms']['mean']}` ms (unavailable if `null`)."
            )
    else:
        lines.append("- No block samples were written.")
    lines.extend(["", "## Metric availability", ""])
    for category, fields in (
        ("RPC-derived block metrics", coverage["block_metric_coverage"]),
        ("Structured log metrics", coverage["log_metric_coverage"]),
    ):
        lines.append(f"- {category}:")
        if fields:
            lines.extend(f"  - `{field}`: {status}" for field, status in sorted(fields.items()))
        else:
            lines.append("  - unavailable")
    for node in coverage["node_metric_coverage"]:
        raw_count = len(node["prometheus"]["raw_series_names"])
        lines.append(
            f"- Node `{node['node_id']}` (`{node['client']}`): Prometheus `{node['prometheus']['status']}` "
            f"({raw_count} raw names; normalized equivalence not asserted)."
        )
    if database_available:
        lines.extend(["", "RPC and Prometheus samples are archived in `metrics_server_data/metrics.redb`."])
    else:
        lines.extend(["", "The metrics-server database was not archived; see local sample files and collector diagnostics."])
    (output_dir / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    return coverage


def main():
    parser = argparse.ArgumentParser(description="Summarize empty-block sweep and metric coverage.")
    parser.add_argument("--output-dir", required=True)
    args = parser.parse_args()
    build_report(args.output_dir)


if __name__ == "__main__":
    main()