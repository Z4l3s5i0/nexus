#!/usr/bin/env python3
import argparse
import csv
import json
import math
from pathlib import Path


CSV_SCHEMAS = {
    "empty-block": {
        "block_samples.csv": {"target_tx_count", "block_number", "actual_tx_count", "matched"},
        "samples.csv": {"target_tx_count", "block_number", "actual_tx_count", "matched"},
        "transaction_inclusion_samples.csv": {
            "transaction_type", "tx_hash", "submitted_at_utc", "included_at_utc",
            "inclusion_latency_ms", "latency_basis", "block_number", "status",
        },
        "transaction_burst_samples.csv": {
            "target_tx_count", "attempt", "submitted_tx_count", "included_tx_count",
            "completed", "blocks_spanned", "latency_basis",
        },
    },
    "size": {
        "source_size.tsv": {"measurement", "label", "value", "unit", "source", "status"},
        "binary_size.tsv": {"measurement", "label", "value", "unit", "source", "status"},
    },
}


def _read_json(path):
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ValueError(f"cannot read valid JSON from {path.name}: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError(f"{path.name} must contain a JSON object")
    return value


def _require_files(output_dir, names):
    missing = [name for name in names if not (output_dir / name).is_file()]
    if missing:
        raise ValueError(f"missing required artifacts: {', '.join(missing)}")


def _read_jsonl(path):
    rows = []
    try:
        with path.open(encoding="utf-8") as source:
            for line_number, line in enumerate(source, 1):
                if not line.strip():
                    continue
                try:
                    value = json.loads(line)
                except json.JSONDecodeError as exc:
                    raise ValueError(f"{path.name}:{line_number} is not valid JSON: {exc}") from exc
                if not isinstance(value, dict):
                    raise ValueError(f"{path.name}:{line_number} must be a JSON object")
                rows.append(value)
    except (OSError, UnicodeError) as exc:
        raise ValueError(f"cannot read {path.name}: {exc}") from exc
    return rows


def _validate_status(output_dir):
    status = _read_json(output_dir / "run_status.json")
    exit_code = status.get("exit_code", status.get("status"))
    if isinstance(exit_code, bool):
        raise ValueError("run_status.json has an invalid exit status")
    if isinstance(exit_code, str):
        normalized = exit_code.strip().lower()
        if normalized in ("passed", "success", "succeeded"):
            exit_code = 0
        elif normalized in ("failed", "failure"):
            exit_code = 1
        elif normalized.lstrip("-").isdigit():
            exit_code = int(normalized)
        else:
            raise ValueError("run_status.json must record a numeric exit_code or status")
    elif not isinstance(exit_code, int):
        raise ValueError("run_status.json must record an integer exit_code or status")
    if exit_code != 0:
        raise ValueError(f"saved run did not complete successfully (exit code {exit_code})")
    recorded_status = status.get("status")
    if isinstance(recorded_status, str) and recorded_status.lower() not in ("passed", "success", "succeeded"):
        raise ValueError(f"saved run status is {recorded_status!r}, not passed")


def _validate_metadata(metadata, required):
    missing = [name for name in required if name not in metadata]
    if missing:
        raise ValueError(f"metadata.json is missing: {', '.join(missing)}")


def _validate_csv(path, expected_columns, delimiter=",", allow_empty=False):
    try:
        with path.open(newline="", encoding="utf-8") as source:
            reader = csv.DictReader(source, delimiter=delimiter)
            columns = set(reader.fieldnames or ())
            rows = list(reader)
    except (OSError, UnicodeError, csv.Error) as exc:
        raise ValueError(f"cannot read {path.name}: {exc}") from exc
    missing = expected_columns - columns
    if missing:
        raise ValueError(f"{path.name} is missing columns: {', '.join(sorted(missing))}")
    if not rows and not allow_empty:
        raise ValueError(f"{path.name} has a header but no measurement rows")
    if any(None in row for row in rows):
        raise ValueError(f"{path.name} contains rows with more fields than its header")
    return rows


def _validate_runtime(output_dir):
    metadata = _read_json(output_dir / "metadata.json")
    _validate_metadata(metadata, ("experiment_id", "chain_id", "nodes"))
    if not isinstance(metadata["nodes"], list) or not metadata["nodes"]:
        raise ValueError("runtime metadata.json must contain at least one node")
    for node in metadata["nodes"]:
        if not isinstance(node, dict) or not isinstance(node.get("index"), int) or isinstance(node.get("index"), bool):
            raise ValueError("runtime metadata.json contains a node without a valid index")
    _require_files(
        output_dir,
        (
            "system_metric_samples.jsonl", "events.jsonl", "log_metric_samples.jsonl",
            "log_metrics_coverage.json", "node_metric_coverage.json",
            "metrics_server_data/metrics.redb",
        ),
    )
    for name in ("system_metric_samples.jsonl", "events.jsonl", "log_metric_samples.jsonl"):
        _read_jsonl(output_dir / name)
    _read_json(output_dir / "log_metrics_coverage.json")
    _read_json(output_dir / "node_metric_coverage.json")
    for node in metadata["nodes"]:
        index = node["index"]
        window_path = output_dir / "workload_windows" / f"node_{index}.json"
        window = _read_json(window_path)
        if window.get("experiment_id") != metadata["experiment_id"] or window.get("node_index") != index:
            raise ValueError(f"runtime workload window identity mismatch: {window_path.name}")
        start_ms = window.get("start_ms")
        end_ms = window.get("end_ms")
        if (
            window.get("exit_code") != 0
            or isinstance(start_ms, bool) or not isinstance(start_ms, int)
            or isinstance(end_ms, bool) or not isinstance(end_ms, int)
            or end_ms <= start_ms
        ):
            raise ValueError(f"runtime workload window is incomplete: {window_path.name}")


def _validate_empty_block(output_dir):
    metadata = _read_json(output_dir / "metadata.json")
    _validate_metadata(metadata, ("experiment_id", "chain_id", "sweep"))
    sweep = metadata["sweep"]
    if not isinstance(sweep, dict) or not isinstance(sweep.get("transaction_counts"), list):
        raise ValueError("empty-block metadata.json has an invalid sweep schema")
    counts = sweep["transaction_counts"]
    blocks_per_count = sweep.get("blocks_per_count")
    if not counts or not isinstance(blocks_per_count, int) or isinstance(blocks_per_count, bool) or blocks_per_count < 1:
        raise ValueError("empty-block metadata.json has invalid transaction counts or blocks_per_count")
    _require_files(
        output_dir,
        (
            "system_metric_samples.jsonl", "events.jsonl", "log_metric_samples.jsonl",
            "derived_metric_samples.jsonl", "block_metric_coverage.json",
            "log_metrics_coverage.json", "node_metric_coverage.json",
            "metrics_server_data/metrics.redb", *CSV_SCHEMAS["empty-block"],
        ),
    )
    for name in (
        "system_metric_samples.jsonl", "events.jsonl", "log_metric_samples.jsonl",
        "derived_metric_samples.jsonl",
    ):
        _read_jsonl(output_dir / name)
    for name in ("block_metric_coverage.json", "log_metrics_coverage.json", "node_metric_coverage.json"):
        _read_json(output_dir / name)
    csv_rows = {}
    for name, columns in CSV_SCHEMAS["empty-block"].items():
        csv_rows[name] = _validate_csv(
            output_dir / name,
            columns,
            allow_empty=name in ("transaction_inclusion_samples.csv", "transaction_burst_samples.csv"),
        )
    samples = csv_rows["samples.csv"]
    burst_rows = csv_rows["transaction_burst_samples.csv"]
    for target in counts:
        if not isinstance(target, int) or isinstance(target, bool) or target < 0:
            raise ValueError("empty-block transaction_counts must contain nonnegative integers")
        if target == 0:
            observed = sum(row.get("target_tx_count") == "0" and row.get("matched", "").lower() == "true" for row in samples)
        else:
            observed = sum(
                row.get("target_tx_count") == str(target) and row.get("completed", "").lower() == "true"
                for row in burst_rows
            )
        if observed < blocks_per_count:
            raise ValueError(f"empty-block target {target} has {observed}/{blocks_per_count} completed samples")


def _validate_startup(output_dir):
    metadata = _read_json(output_dir / "metadata.json")
    _validate_metadata(metadata, ("experiment_id", "chain_id", "iterations", "configurations"))
    iterations = metadata["iterations"]
    configurations = metadata["configurations"]
    if not isinstance(iterations, int) or isinstance(iterations, bool) or iterations < 1:
        raise ValueError("startup metadata.json must contain a positive iterations count")
    if not isinstance(configurations, list) or not configurations or any(not isinstance(item, str) for item in configurations):
        raise ValueError("startup metadata.json must contain at least one configuration")
    _require_files(
        output_dir,
        (
            "system_metric_samples.jsonl", "startup_metric_samples.jsonl",
            "startup_time_coverage.json", "metrics_server_data/metrics.redb",
        ),
    )
    for name in ("system_metric_samples.jsonl", "startup_metric_samples.jsonl"):
        _read_jsonl(output_dir / name)
    _read_json(output_dir / "startup_time_coverage.json")
    for config in configurations:
        for iteration in range(1, iterations + 1):
            result_path = output_dir / "iterations" / config / str(iteration) / "result.json"
            if not result_path.is_file():
                raise ValueError(f"missing startup result: iterations/{config}/{iteration}/result.json")
            result = _read_json(result_path)
            if (
                result.get("config") != config
                or result.get("iteration") != iteration
                or result.get("experiment_id") != metadata["experiment_id"]
                or result.get("chain_id") != metadata["chain_id"]
            ):
                raise ValueError(f"startup result identity mismatch: iterations/{config}/{iteration}/result.json")
            if result.get("ready") is not True:
                raise ValueError(f"startup result is not ready: {config} iteration {iteration}")
            value = result.get("startup_time_ms")
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
                raise ValueError(f"startup result has invalid startup_time_ms: {config} iteration {iteration}")


def _validate_compliance(output_dir):
    metadata = _read_json(output_dir / "metadata.json")
    _validate_metadata(metadata, ("clients",))
    clients = metadata["clients"]
    if not isinstance(clients, list) or not clients or any(not isinstance(client, str) for client in clients):
        raise ValueError("compliance metadata.json must contain a nonempty clients list")
    if len(set(clients)) != len(clients):
        raise ValueError("compliance metadata.json contains duplicate clients")
    results = _read_jsonl(output_dir / "results.jsonl")
    if len(results) != len(clients):
        raise ValueError(f"compliance expected {len(clients)} client results, found {len(results)}")
    by_client = {row.get("client"): row for row in results}
    missing = [client for client in clients if client not in by_client]
    if missing:
        raise ValueError(f"compliance results are missing clients: {', '.join(missing)}")
    for client in clients:
        result = by_client[client]
        exit_code = result.get("exit_code")
        if isinstance(exit_code, bool) or not isinstance(exit_code, int) or exit_code != 0 or result.get("status") != "passed":
            raise ValueError(f"compliance client {client} did not pass")
        log_path = result.get("log_path")
        if not isinstance(log_path, str) or not Path(log_path).is_file():
            raise ValueError(f"compliance client {client} has no readable log file")


def _validate_size(output_dir):
    for name, columns in CSV_SCHEMAS["size"].items():
        rows = _validate_csv(output_dir / name, columns, delimiter="\t")
        for row in rows:
            if not row.get("label") or row.get("status") not in ("measured", "unavailable"):
                raise ValueError(f"{name} contains a row with an invalid label or status")
            value = row.get("value", "")
            if row["status"] == "measured":
                try:
                    measurement = float(value)
                except (TypeError, ValueError) as exc:
                    raise ValueError(f"{name} contains a measured row without a numeric value") from exc
                if not math.isfinite(measurement) or measurement < 0:
                    raise ValueError(f"{name} contains an invalid measured value")
            elif value:
                raise ValueError(f"{name} must not encode unavailable measurements as values")


VALIDATORS = {
    "runtime": _validate_runtime,
    "empty-block": _validate_empty_block,
    "startup": _validate_startup,
    "compliance": _validate_compliance,
    "size": _validate_size,
}


def validate_result(experiment, output_dir):
    output_dir = Path(output_dir)
    if experiment not in VALIDATORS:
        raise ValueError(f"unknown experiment type: {experiment}")
    _require_files(output_dir, ("run_status.json",))
    _validate_status(output_dir)
    VALIDATORS[experiment](output_dir)


def main():
    parser = argparse.ArgumentParser(description="Validate saved experiment results before evaluation.")
    parser.add_argument("--experiment", required=True, choices=tuple(VALIDATORS))
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        validate_result(args.experiment, args.output_dir)
    except ValueError as exc:
        parser.error(str(exc))
    print(f"Validated complete {args.experiment} results in {args.output_dir}")


if __name__ == "__main__":
    main()