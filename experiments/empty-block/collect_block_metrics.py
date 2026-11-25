#!/usr/bin/env python3
import argparse
import csv
import json
import os
import sys
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

RUNTIME_HELPERS = Path(__file__).resolve().parents[1] / "runtime"
sys.path.insert(0, str(RUNTIME_HELPERS))
from metrics_api import ingest_samples, metric_sample


def derive_block_samples(rows, metadata, experiment_id, latency_rows=()):
    client = metadata.get("client", "unavailable")
    node_id = metadata.get("node_id") or "el-node-0"
    network = str(metadata["chain_id"])
    client_variant = metadata.get("client_variant", "unknown")
    samples = []
    blocks = {}
    attempts = defaultdict(lambda: {"submitted": set(), "included": set(), "timestamp_ms": None})

    for row in rows:
        try:
            number = int(row["block_number"])
            actual = int(row["actual_tx_count"])
            block_timestamp = int(row["block_timestamp"])
            target = int(row["target_tx_count"])
            attempt = int(row["attempt"])
        except (KeyError, TypeError, ValueError):
            continue
        timestamp_ms = block_timestamp * 1000
        blocks[number] = {"actual": actual, "timestamp": block_timestamp, "timestamp_ms": timestamp_ms}
        attempt_data = attempts[(target, attempt)]
        attempt_data["timestamp_ms"] = max(attempt_data["timestamp_ms"] or 0, timestamp_ms)
        for tx_hash in row.get("submitted_transaction_hashes", "").split(";"):
            if tx_hash:
                attempt_data["submitted"].add(tx_hash.lower())
        for tx_hash in row.get("transaction_hashes", "").split(";"):
            if tx_hash:
                attempt_data["included"].add(tx_hash.lower())

        labels = {
            "block_number": str(number),
            "target_tx_count": str(target),
            "attempt": str(attempt),
            "basis": "eth_getBlockByNumber",
            "runtime_variant": client_variant,
        }
        samples.append(
            metric_sample(timestamp_ms, experiment_id, node_id, network, client, "Derived",
                          "empty_block_actual_transaction_count", actual, labels)
        )
        samples.append(
            metric_sample(timestamp_ms, experiment_id, node_id, network, client, "Derived",
                          "empty_block_target_transaction_count", target, labels)
        )
        samples.append(
            metric_sample(timestamp_ms, experiment_id, node_id, network, client, "Derived",
                          "empty_block_block_matched", 1 if actual == target else 0, labels)
        )
        samples.append(
            metric_sample(timestamp_ms, experiment_id, node_id, network, client, "Derived",
                          "empty_block_timestamp_seconds", block_timestamp, labels)
        )

    previous_timestamp = None
    for number, block in sorted(blocks.items()):
        if previous_timestamp is not None and block["timestamp"] > previous_timestamp:
            interval = block["timestamp"] - previous_timestamp
            samples.append(
                metric_sample(
                    block["timestamp_ms"], experiment_id, node_id, network, client, "Derived",
                    "empty_block_tps", block["actual"] / interval,
                    {
                        "block_number": str(number),
                        "basis": "block transactions / timestamp interval",
                        "runtime_variant": client_variant,
                    },
                )
            )
        previous_timestamp = block["timestamp"]

    for (target, attempt), attempt_data in attempts.items():
        submitted = len(attempt_data["submitted"])
        if target <= 0 or submitted == 0 or attempt_data["timestamp_ms"] is None:
            continue
        included = len(attempt_data["included"])
        samples.append(
            metric_sample(
                attempt_data["timestamp_ms"], experiment_id, node_id, network, client, "Derived",
                "empty_block_commit_to_submit_ratio", included / submitted,
                {
                    "target_tx_count": str(target),
                    "attempt": str(attempt),
                    "submitted_transactions": str(submitted),
                    "included_transactions": str(included),
                    "basis": "receipt-confirmed submitted hashes / submitted hashes",
                    "runtime_variant": client_variant,
                },
            )
        )

    for row in latency_rows:
        try:
            latency_ms = float(row["inclusion_latency_ms"])
            if row.get("included_at_epoch"):
                timestamp_ms = int(float(row["included_at_epoch"]) * 1000)
            elif row.get("included_at_utc"):
                timestamp_ms = int(datetime.fromisoformat(row["included_at_utc"].replace("Z", "+00:00")).timestamp() * 1000)
            else:
                timestamp_ms = int(float(row.get("block_timestamp", 0)) * 1000)
            if latency_ms < 0 or timestamp_ms <= 0:
                continue
        except (KeyError, TypeError, ValueError, OverflowError):
            continue
        labels = {
            "transaction_type": row.get("transaction_type", "unknown"),
            "workload": row.get("workload", "") or "none",
            "transaction_status": (
                "failed" if str(row.get("status") or "").lower() in ("0x0", "0")
                else "success" if str(row.get("status") or "").lower() in ("0x1", "1")
                else "unknown"
            ),
            "basis": "submission start to first receipt observation",
            "runtime_variant": client_variant,
        }
        if row.get("target_tx_count"):
            labels["target_tx_count"] = row["target_tx_count"]
        if row.get("attempt"):
            labels["attempt"] = row["attempt"]
        if row.get("block_number"):
            labels["block_number"] = row["block_number"]
        samples.append(
            metric_sample(
                timestamp_ms,
                experiment_id,
                node_id,
                network,
                client,
                "Derived",
                "empty_block_transaction_inclusion_latency_ms",
                latency_ms,
                labels,
            )
        )

    names = {sample["name"] for sample in samples}
    fields = (
        "actual_transaction_count", "target_transaction_count", "matched_block",
        "block_timestamp", "tps", "commit_to_submit_ratio", "transaction_inclusion_latency",
    )
    coverage = {
        field: "observed" if metric_name in names else "unavailable"
        for field, metric_name in zip(
            fields,
            (
                "empty_block_actual_transaction_count", "empty_block_target_transaction_count",
                "empty_block_block_matched", "empty_block_timestamp_seconds", "empty_block_tps",
                "empty_block_commit_to_submit_ratio", "empty_block_transaction_inclusion_latency_ms",
            ),
        )
    }
    return samples, {"fields": coverage, "sample_count": len(samples)}


def collect(args):
    metadata = json.loads(Path(args.metadata).read_text(encoding="utf-8"))
    rows = []
    input_path = Path(args.samples)
    if input_path.is_file():
        with input_path.open(newline="", encoding="utf-8") as source:
            rows = list(csv.DictReader(source))
    latency_rows = []
    latency_path = Path(args.samples).with_name("transaction_inclusion_samples.csv")
    if latency_path.is_file():
        with latency_path.open(newline="", encoding="utf-8") as source:
            latency_rows = list(csv.DictReader(source))
    samples, coverage = derive_block_samples(rows, metadata, args.experiment_id, latency_rows)
    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    with (output_dir / "derived_metric_samples.jsonl").open("w", encoding="utf-8") as output:
        for sample in samples:
            output.write(json.dumps(sample, sort_keys=True, allow_nan=False) + "\n")
    ingestion_error = None
    try:
        ingest_samples(args.api_url, args.token, samples)
    except (RuntimeError, ValueError) as exc:
        ingestion_error = str(exc)
        print(f"Block-derived samples were saved locally but not ingested: {exc}", file=sys.stderr)
    coverage["ingestion_error"] = ingestion_error
    (output_dir / "block_metric_coverage.json").write_text(
        json.dumps(coverage, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def main():
    parser = argparse.ArgumentParser(description="Create RPC-derived block, TPS, and transaction-ratio samples.")
    parser.add_argument("--metadata", required=True)
    parser.add_argument("--samples", required=True)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--experiment-id", required=True)
    parser.add_argument("--api-url", default="http://127.0.0.1:9100")
    parser.add_argument("--token", default=None)
    args = parser.parse_args()
    if args.token is None:
        args.token = os.environ.get("METRICS_INGEST_TOKEN", "")
    collect(args)


if __name__ == "__main__":
    main()