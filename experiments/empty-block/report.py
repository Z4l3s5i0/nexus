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
    latency_rows = []
    latency_path = output_dir / "transaction_inclusion_samples.csv"
    if latency_path.is_file():
        with latency_path.open(newline="", encoding="utf-8") as source:
            latency_rows = list(csv.DictReader(source))
    burst_rows = []
    burst_path = output_dir / "transaction_burst_samples.csv"
    burst_data_available = burst_path.is_file()
    if burst_path.is_file():
        with burst_path.open(newline="", encoding="utf-8") as source:
            burst_rows = list(csv.DictReader(source))
    rows = []
    samples_path = output_dir / "samples.csv"
    if samples_path.is_file():
        with samples_path.open(newline="", encoding="utf-8") as source:
            rows = [row for row in csv.DictReader(source) if row.get("block_number")]

    grouped = {}
    for row in rows:
        target = row.get("target_tx_count", "unknown")
        summary = grouped.setdefault(
            target,
            {"blocks": 0, "matched": 0, "actual_tx_counts": [], "execution_times_ms": [], "completed_bursts": 0},
        )
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

    burst_completion_by_target = {}
    for row in burst_rows:
        target = row.get("target_tx_count", "unknown")
        summary = grouped.setdefault(
            target,
            {"blocks": 0, "matched": 0, "actual_tx_counts": [], "execution_times_ms": [], "completed_bursts": 0},
        )
        summary["burst_attempts"] = summary.get("burst_attempts", 0) + 1
        if row.get("completed", "").lower() == "true":
            summary["completed_bursts"] += 1
        try:
            latency = float(row["burst_completion_latency_ms"])
        except (KeyError, TypeError, ValueError):
            continue
        burst_completion_by_target.setdefault(target, []).append(latency)

    for summary in grouped.values():
        times = summary.pop("execution_times_ms")
        summary["execution_time_ms"] = {
            "observed": bool(times),
            "mean": statistics.mean(times) if times else None,
        }
        summary["exact_block_matches"] = summary["matched"]
    latencies_by_type = {}
    burst_spread = {}
    burst_completion_by_attempt = {}
    for row in burst_rows:
        try:
            target = str(int(row["target_tx_count"]))
            attempt = str(int(row["attempt"]))
            blocks_spanned = int(row["blocks_spanned"])
        except (KeyError, TypeError, ValueError):
            continue
        spread = "multiple_blocks" if blocks_spanned > 1 else "single_block" if blocks_spanned == 1 else "unknown"
        transaction_type = row.get("transaction_type") or "unknown"
        burst_spread[(transaction_type, target, attempt)] = spread
        try:
            completion_latency = float(row["burst_completion_latency_ms"])
        except (KeyError, TypeError, ValueError):
            continue
        burst_completion_by_attempt[(transaction_type, target, spread, attempt)] = completion_latency

    latency_by_target = {}
    for row in latency_rows:
        try:
            latency = float(row["inclusion_latency_ms"])
        except (KeyError, TypeError, ValueError):
            continue
        transaction_type = row.get("transaction_type") or "unknown"
        summary = latencies_by_type.setdefault(
            transaction_type, {"values": [], "workloads": set(), "failed_observations": 0}
        )
        summary["values"].append(latency)
        summary["workloads"].add(row.get("workload") or "none")
        if str(row.get("status") or "").lower() in ("0x0", "0"):
            summary["failed_observations"] += 1
        try:
            target = str(int(row["target_tx_count"]))
            attempt = str(int(row["attempt"]))
        except (KeyError, TypeError, ValueError):
            continue
        spread = burst_spread.get((transaction_type, target, attempt), "unknown")
        target_summary = latency_by_target.setdefault(transaction_type, {}).setdefault(target, {}).setdefault(
            spread, {"values": [], "failed_observations": 0}
        )
        target_summary["values"].append(latency)
        if str(row.get("status") or "").lower() in ("0x0", "0"):
            target_summary["failed_observations"] += 1
    transaction_inclusion_latency = {}
    for transaction_type, latency_data in latencies_by_type.items():
        latencies = latency_data["values"]
        ordered = sorted(latencies)
        p95_index = max(0, (95 * len(ordered) + 99) // 100 - 1)
        transaction_inclusion_latency[transaction_type] = {
            "observations": len(ordered),
            "failed_observations": latency_data["failed_observations"],
            "workload": ", ".join(sorted(latency_data["workloads"])) or metadata.get("workload", "none"),
            "mean_ms": statistics.mean(ordered),
            "median_ms": statistics.median(ordered),
            "p95_ms": ordered[p95_index],
        }
    transaction_inclusion_latency_by_target = {}
    for transaction_type, targets in latency_by_target.items():
        transaction_inclusion_latency_by_target[transaction_type] = {}
        for target, spreads in targets.items():
            transaction_inclusion_latency_by_target[transaction_type][target] = {}
            for spread, latency_data in spreads.items():
                ordered = sorted(latency_data["values"])
                p95_index = max(0, (95 * len(ordered) + 99) // 100 - 1)
                completion_values = [
                    latency
                    for (kind, burst_target, burst_spread_name, _attempt), latency in burst_completion_by_attempt.items()
                    if kind == transaction_type and burst_target == target and burst_spread_name == spread
                ]
                transaction_inclusion_latency_by_target[transaction_type][target][spread] = {
                    "observations": len(ordered),
                    "failed_observations": latency_data["failed_observations"],
                    "mean_ms": statistics.mean(ordered),
                    "median_ms": statistics.median(ordered),
                    "p95_ms": ordered[p95_index],
                    "burst_completion_latency_ms": {
                        "observations": len(completion_values),
                        "mean": statistics.mean(completion_values) if completion_values else None,
                        "median": statistics.median(completion_values) if completion_values else None,
                        "max": max(completion_values) if completion_values else None,
                    },
                }
    sweep = metadata.get("sweep", {})
    expected_samples = sweep.get("blocks_per_count", 0)
    for target in sweep.get("transaction_counts", []):
        summary = grouped.setdefault(
            str(target),
            {
                "blocks": 0,
                "matched": 0,
                "actual_tx_counts": [],
                "execution_time_ms": {"observed": False, "mean": None},
                "completed_bursts": 0,
            },
        )
        summary["expected_matches"] = expected_samples
        summary["missing_matches"] = max(0, expected_samples - summary["matched"])
        summary["exact_block_matches"] = summary["matched"]
        summary["expected_bursts"] = expected_samples if target > 0 and burst_data_available else None
        summary["missing_bursts"] = (
            max(0, summary["expected_bursts"] - summary["completed_bursts"])
            if summary["expected_bursts"] is not None else None
        )
        summary["burst_completion_latency_ms"] = {
            "observations": len(burst_completion_by_target.get(str(target), [])),
            "mean": (
                statistics.mean(burst_completion_by_target[str(target)])
                if burst_completion_by_target.get(str(target)) else None
            ),
            "median": (
                statistics.median(burst_completion_by_target[str(target)])
                if burst_completion_by_target.get(str(target)) else None
            ),
        }

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
        "workload": metadata.get("workload", "none"),
        "contract_deployment": metadata.get("contract_deployment"),
        "transaction_inclusion_latency": transaction_inclusion_latency,
        "transaction_inclusion_latency_by_target": transaction_inclusion_latency_by_target,
        "transaction_burst_sample_count": len(burst_rows),
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
            "Positive transaction-count samples are completed submitted bursts; bursts may span multiple blocks, which remain individually recorded.",
            "Burst completion latency runs from the first burst submission start until the final receipt is first observed; receipt polling interval contributes delay.",
            "Older result directories without transaction_burst_samples.csv cannot provide completed-burst or cross-block grouping statistics.",
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
        f"- Contract workload: `{metadata.get('workload', 'none')}`",
        f"- Run exit status: `{run_status.get('status', 0)}`",
        f"- Block samples: `{len(rows)}`",
        "",
        "## Transaction-count sweep",
        "",
    ]
    if grouped:
        for target, summary in sorted(grouped.items(), key=lambda item: int(item[0])):
            if int(target) == 0:
                lines.append(
                    f"- Target `{target}`: {summary['matched']}/{summary.get('expected_matches', summary['blocks'])} exact empty-block matches "
                    f"({summary.get('missing_matches', 0)} missing; {summary['blocks']} block observations); "
                    f"observed block transaction counts `{summary['actual_tx_counts']}`."
                )
            else:
                if summary.get("expected_bursts") is None:
                    lines.append(
                        f"- Target `{target}`: completed-burst data unavailable; {summary['exact_block_matches']} individual blocks "
                        f"exactly matched the target; observed block transaction counts `{summary['actual_tx_counts']}`."
                    )
                else:
                    lines.append(
                        f"- Target `{target}`: {summary['completed_bursts']}/{summary['expected_bursts']} completed bursts "
                        f"({summary['missing_bursts']} missing); {summary['exact_block_matches']} individual blocks exactly matched the target; "
                        f"observed block transaction counts `{summary['actual_tx_counts']}`; "
                        f"mean burst completion `{summary['burst_completion_latency_ms']['mean']}` ms."
                    )
    else:
        lines.append("- No block samples were written.")
    lines.extend(["", "## Transaction inclusion latency", ""])
    if transaction_inclusion_latency:
        for transaction_type, summary in sorted(transaction_inclusion_latency.items()):
            lines.append(
                f"- `{transaction_type}` ({summary['workload']}): {summary['observations']} observations; "
                f"mean `{summary['mean_ms']:.3f}` ms, median `{summary['median_ms']:.3f}` ms, "
                f"p95 `{summary['p95_ms']:.3f}` ms; {summary['failed_observations']} reverted."
            )
        lines.append(
            "Latency is measured from the client-side submission start until the first receipt poll observes inclusion; "
            "the configured polling interval contributes observation delay."
        )
    else:
        lines.append("- No transaction inclusion latency observations were recorded.")
    lines.extend(["", "### Inclusion latency by target and block spread", ""])
    if transaction_inclusion_latency_by_target:
        for transaction_type, targets in sorted(transaction_inclusion_latency_by_target.items()):
            for target, spreads in sorted(targets.items(), key=lambda item: int(item[0])):
                for spread, summary in sorted(spreads.items()):
                    completion = summary["burst_completion_latency_ms"]
                    lines.append(
                        f"- `{transaction_type}`, target `{target}`, `{spread}`: {summary['observations']} transaction(s); "
                        f"mean `{summary['mean_ms']:.3f}` ms, median `{summary['median_ms']:.3f}` ms, "
                        f"p95 `{summary['p95_ms']:.3f}` ms; burst completion mean `{completion['mean']}` ms "
                        f"across {completion['observations']} burst(s)."
                    )
    else:
        lines.append("- No transaction latencies could be grouped by burst and block spread.")
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