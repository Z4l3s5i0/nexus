#!/usr/bin/env python3
import argparse
import csv
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

RUNTIME_HELPERS = Path(__file__).resolve().parents[1] / "runtime"
sys.path.insert(0, str(RUNTIME_HELPERS.parent))
from runtime.report import read_jsonl, system_coverage


def _status(names, groups):
    counts = [sum(name in names for name in group) for group in groups]
    if any(count == len(group) for count, group in zip(counts, groups)):
        return "observed"
    if any(count for count in counts):
        return "partial"
    return "unavailable"


def startup_system_coverage(results, samples, metadata):
    names_by_node = defaultdict(set)
    for sample in samples:
        names_by_node[sample["node_id"]].add(sample["name"])
    nodes = []
    for result in results:
        names = names_by_node.get(result["node_id"], set())
        nodes.append(
            {
                "config": result["config"],
                "iteration": result["iteration"],
                "node_id": result["node_id"],
                "client": result["client"],
                "fields": {
                    "cpu": _status(names, [["system_process_cpu_utilization_percent"], ["system_container_cpu_utilization_percent"]]),
                    "memory": _status(names, [["system_process_memory_rss_bytes"], ["system_container_memory_usage_bytes"]]),
                    "disk_io": _status(
                        names,
                        [
                            ["system_process_disk_read_bytes", "system_process_disk_write_bytes"],
                            ["system_container_disk_read_bytes", "system_container_disk_write_bytes"],
                        ],
                    ),
                    "network_throughput": _status(
                        names,
                        [["system_container_network_receive_bytes_per_second", "system_container_network_transmit_bytes_per_second"]],
                    ),
                },
            }
        )
    return {"nodes": nodes, "host": system_coverage(metadata, samples)["host"]}


def _read_results(output_dir):
    results = []
    for result_path in sorted((Path(output_dir) / "iterations").glob("*/*/result.json")):
        try:
            results.append(json.loads(result_path.read_text(encoding="utf-8")))
        except (OSError, ValueError):
            continue
    return results


def build_report(output_dir, expected_iterations):
    output_dir = Path(output_dir)
    results = _read_results(output_dir)
    system_samples = read_jsonl(output_dir / "system_metric_samples.jsonl")
    startup_samples = read_jsonl(output_dir / "startup_metric_samples.jsonl")
    metadata_path = output_dir / "metadata.json"
    metadata = json.loads(metadata_path.read_text(encoding="utf-8")) if metadata_path.is_file() else {"nodes": []}
    startup_coverage_path = output_dir / "startup_time_coverage.json"
    startup_coverage = json.loads(startup_coverage_path.read_text(encoding="utf-8")) if startup_coverage_path.is_file() else {"iterations": []}
    grouped = defaultdict(list)
    for result in results:
        if result.get("ready") and result.get("startup_time_ms") is not None:
            grouped[result["config"]].append(float(result["startup_time_ms"]))

    summary = {}
    configs = sorted({result["config"] for result in results})
    for config in configs:
        times = grouped.get(config, [])
        config_results = [result for result in results if result["config"] == config]
        summary[config] = {
            "iterations_requested": expected_iterations,
            "iterations_completed": len(config_results),
            "iterations_ready": len(times),
            "iterations_unavailable": max(0, expected_iterations - len(times)),
            "startup_time_ms": {
                "min": min(times) if times else None,
                "max": max(times) if times else None,
                "mean": statistics.mean(times) if times else None,
                "median": statistics.median(times) if times else None,
                "stdev": statistics.stdev(times) if len(times) > 1 else None,
            },
            "image_references": sorted({result.get("image", "") for result in config_results if result.get("image")}),
            "image_ids": sorted({result.get("image_id", "") for result in config_results if result.get("image_id")}),
            "errors": [result["error"] for result in config_results if result.get("error")],
        }

    system_status = startup_system_coverage(results, system_samples, metadata)
    database_available = (output_dir / "metrics_server_data" / "metrics.redb").is_file()
    report = {
        "experiment_id": metadata.get("experiment_id"),
        "chain_id": metadata.get("chain_id"),
        "iterations_requested": expected_iterations,
        "configurations": summary,
        "system_metric_coverage": system_status,
        "system_sample_count": len(system_samples),
        "startup_time_sample_count": len(startup_samples),
        "startup_time_coverage": startup_coverage.get("iterations", []),
        "metrics_database_archived": database_available,
        "notes": [
            "Startup timing ends at the first successful eth_chainId response.",
            "Geth, Reth, and host Wasmer initialization is excluded; WASIX-in-Docker initialization remains inside the timed launch, matching the prior benchmark.",
            "Host load/context-switch values remain host-scoped; process/container attribution is reported separately.",
            "Unavailable metrics and failed readiness checks are not represented as zero.",
        ],
    }
    (output_dir / "report.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    results_csv = output_dir / "results.csv"
    with results_csv.open("w", newline="", encoding="utf-8") as destination:
        columns = (
            "config", "iteration", "client", "client_variant", "node_id", "ready",
            "startup_time_ms", "image", "image_id", "error",
        )
        writer = csv.DictWriter(destination, fieldnames=columns)
        writer.writeheader()
        for result in results:
            writer.writerow({field: result.get(field) for field in columns})

    lines = [
        f"# Startup benchmark {metadata.get('experiment_id')}",
        "",
        f"- Chain ID: `{metadata.get('chain_id')}`",
        f"- Iterations per configuration: `{expected_iterations}`",
        f"- System samples archived locally: `{len(system_samples)}`",
        f"- Startup-time samples stored: `{len(startup_samples)}`",
        "",
        "## Startup time statistics",
        "",
    ]
    for config, values in sorted(summary.items()):
        stats = values["startup_time_ms"]
        lines.append(
            f"- `{config}`: `{values['iterations_ready']}/{expected_iterations}` ready; "
            f"min `{stats['min']}`, max `{stats['max']}`, mean `{stats['mean']}`, "
            f"median `{stats['median']}`, stdev `{stats['stdev']}` ms (null means unavailable)."
        )
        for error in values["errors"]:
            lines.append(f"  - Failure: {error}")
    missing_startup_metrics = sum(
        entry.get("status") != "observed" for entry in startup_coverage.get("iterations", [])
    )
    lines.append(f"- Startup-time observations unavailable: `{missing_startup_metrics}`.")
    lines.extend(["", "## System metric availability", ""])
    for node in system_status["nodes"]:
        fields = ", ".join(f"{field}={status}" for field, status in sorted(node["fields"].items()))
        lines.append(f"- `{node['config']}` iteration `{node['iteration']}` (`{node['client']}`): {fields}.")
    host = ", ".join(f"{field}={status}" for field, status in sorted(system_status["host"]["fields"].items()))
    lines.append(f"- Host-scoped: {host}.")
    lines.append("")
    lines.append(
        "Metrics-server database: `metrics_server_data/metrics.redb`." if database_available
        else "Metrics-server database was not archived; see collector logs and local JSONL samples."
    )
    (output_dir / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    return report


def main():
    parser = argparse.ArgumentParser(description="Summarize startup times and system metric coverage.")
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--iterations", type=int, required=True)
    args = parser.parse_args()
    build_report(args.output_dir, args.iterations)


if __name__ == "__main__":
    main()