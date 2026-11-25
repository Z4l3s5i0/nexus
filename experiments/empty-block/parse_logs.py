#!/usr/bin/env python3
import argparse
import csv
import json
import os
import re
import shlex
import sys
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

RUNTIME_HELPERS = Path(__file__).resolve().parents[1] / "runtime"
sys.path.insert(0, str(RUNTIME_HELPERS))
from metrics_api import ingest_samples, metric_sample


EVENT = re.compile(r"\[EXP\]\s+([A-Z][A-Z0-9_]*)(?:\s+(.*))?")
TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z?")
TRANSACTION_HASH = re.compile(r"^0x[0-9a-fA-F]{64}$")
EXECUTION_TIME = re.compile(r"Block (\d+) executed in ([\d.]+)(m?s)")


def parse_timestamp(line):
    match = TIMESTAMP.search(line)
    if not match:
        return None
    value = match.group(0)
    if value.endswith("Z"):
        value = value[:-1] + "+00:00"
    try:
        timestamp = datetime.fromisoformat(value)
    except ValueError:
        return None
    if timestamp.tzinfo is None:
        timestamp = timestamp.replace(tzinfo=timezone.utc)
    return int(timestamp.timestamp() * 1000)


def parse_attributes(value):
    attributes = {}
    try:
        tokens = shlex.split(value or "")
    except ValueError:
        return attributes
    for token in tokens:
        key, separator, item = token.partition("=")
        if separator and key:
            attributes[key] = item.strip(",")
    return attributes


def parse_event_line(line, experiment_id, node_id, client, network, client_variant="unknown"):
    match = EVENT.search(line)
    if not match:
        return None
    return {
        "timestamp_ms": parse_timestamp(line),
        "experiment_id": experiment_id,
        "node_id": node_id,
        "client": client,
        "client_variant": client_variant,
        "network": network,
        "event": match.group(1),
        "attributes": parse_attributes(match.group(2)),
        "raw": line.rstrip("\r\n"),
    }


def _numeric(attributes, name):
    try:
        value = float(attributes[name])
    except (KeyError, TypeError, ValueError):
        return None
    return value if value >= 0 else None


def load_execution_times(log_path):
    execution_times = {}
    if not log_path.is_file():
        return execution_times
    with log_path.open(encoding="utf-8", errors="replace") as log_file:
        for line in log_file:
            match = EVENT.search(line)
            if match and match.group(1) == "BLOCK_EXEC_END":
                attributes = parse_attributes(match.group(2))
                try:
                    number = int(attributes["number"])
                    value = float(attributes["elapsed_ms"])
                except (KeyError, TypeError, ValueError):
                    continue
                execution_times[number] = value
                continue
            match = EXECUTION_TIME.search(line)
            if match:
                value = float(match.group(2))
                execution_times[int(match.group(1))] = value * 1000 if match.group(3) == "s" else value
    return execution_times


def _metric(event, name, value, labels=None):
    if value is None or event["timestamp_ms"] is None:
        return None
    return metric_sample(
        event["timestamp_ms"],
        event["experiment_id"],
        event["node_id"],
        event["network"],
        event["client"],
        "Log",
        name,
        float(value),
        labels,
    )


def derive_log_samples(events, relevant_hashes, selected_blocks):
    relevant_hashes = {tx_hash.lower() for tx_hash in relevant_hashes}
    selected_blocks = {int(number) for number in selected_blocks}
    samples = []
    received_at = {}
    senders = {}
    event_counts = Counter(event["event"] for event in events)
    coverage = Counter()

    for event in events:
        name = event["event"]
        attributes = event["attributes"]
        tx_hash = attributes.get("hash", "").lower()
        number = attributes.get("number")

        if name == "TX_RPC_RECV" and tx_hash in relevant_hashes and event["timestamp_ms"] is not None:
            received_at[(event["node_id"], tx_hash)] = event["timestamp_ms"]
        elif name == "TX_EXEC_START" and tx_hash in relevant_hashes:
            coverage["transaction_hash"] += 1
            sender = attributes.get("sender")
            if sender:
                senders[(event["node_id"], tx_hash)] = sender
                coverage["sender"] += 1
            started_at = event["timestamp_ms"]
            received = received_at.get((event["node_id"], tx_hash))
            if received is not None and started_at is not None and started_at >= received:
                sample = _metric(
                    event,
                    "log_submit_to_execution_ms",
                    started_at - received,
                    {
                        "transaction_hash": tx_hash,
                        "sender": sender or "unavailable",
                        "runtime_variant": event.get("client_variant", "unknown"),
                    },
                )
                if sample:
                    samples.append(sample)
        elif name == "TX_EXEC_END" and tx_hash in relevant_hashes:
            sender = senders.get((event["node_id"], tx_hash), "unavailable")
            gas_used = _numeric(attributes, "gas_used")
            elapsed = _numeric(attributes, "elapsed_ms")
            for metric_name, value, unit in (
                ("log_transaction_gas_used", gas_used, "gas"),
                ("log_transaction_elapsed_ms", elapsed, "ms"),
            ):
                sample = _metric(
                    event,
                    metric_name,
                    value,
                    {"transaction_hash": tx_hash, "sender": sender, "unit": unit,
                     "runtime_variant": event.get("client_variant", "unknown")},
                )
                if sample:
                    samples.append(sample)
            if gas_used is not None:
                coverage["gas_used"] += 1
            if elapsed is not None:
                coverage["elapsed_time"] += 1
        elif name == "BLOCK_EXEC_END" and number is not None:
            try:
                block_number = int(number)
            except ValueError:
                continue
            if block_number in selected_blocks:
                sample = _metric(
                    event,
                    "log_block_execution_time_ms",
                    _numeric(attributes, "elapsed_ms"),
                    {"block_number": str(block_number),
                     "runtime_variant": event.get("client_variant", "unknown")},
                )
                if sample:
                    samples.append(sample)
                    coverage["elapsed_time"] += 1
        elif name == "IMPORT_END":
            try:
                block_number = int(number)
            except (TypeError, ValueError):
                continue
            if block_number not in selected_blocks:
                continue
            tx_count = _numeric(attributes, "txs")
            for metric_name, value in (
                ("log_import_block_number", block_number),
                ("log_import_transaction_count", tx_count),
            ):
                sample = _metric(
                    event, metric_name, value,
                    {"block_number": str(block_number), "runtime_variant": event.get("client_variant", "unknown")},
                )
                if sample:
                    samples.append(sample)
            coverage["block_number"] += 1
            if tx_count is not None:
                coverage["transaction_count"] += 1

        if attributes.get("peer"):
            coverage["peer_id"] += 1
        if name.startswith("DOWNLOAD_"):
            coverage["download_type"] += 1

    requested = (
        "transaction_hash", "sender", "gas_used", "elapsed_time", "block_number",
        "transaction_count", "peer_id", "download_type",
    )
    return samples, {
        "event_counts": dict(sorted(event_counts.items())),
        "counts": {field: coverage[field] for field in requested},
        "requested_fields": {
            field: "available" if coverage[field] else "unavailable in captured structured events"
            for field in requested
        },
    }


def parse_logs(args):
    metadata = json.loads(Path(args.metadata).read_text(encoding="utf-8"))
    node_id = metadata.get("node_id") or "el-node-0"
    client = metadata.get("client", args.client)
    network = str(metadata["chain_id"])
    client_variant = metadata.get("client_variant", "unknown")
    logs_dir = Path(args.logs_dir)
    log_path = logs_dir / "el-node-0.log"
    events = []
    if log_path.is_file():
        for line in log_path.read_text(encoding="utf-8", errors="replace").splitlines():
            event = parse_event_line(line, args.experiment_id, node_id, client, network, client_variant)
            if event:
                events.append(event)

    block_rows = []
    input_csv = Path(args.block_samples)
    if input_csv.is_file():
        with input_csv.open(newline="", encoding="utf-8") as source:
            block_rows = list(csv.DictReader(source))
    relevant_hashes = set()
    selected_blocks = set()
    for row in block_rows:
        selected_blocks.add(row["block_number"])
        for field in ("transaction_hashes", "submitted_transaction_hashes"):
            relevant_hashes.update(tx_hash for tx_hash in row.get(field, "").split(";") if tx_hash)
    inclusion_path = input_csv.with_name("transaction_inclusion_samples.csv")
    if inclusion_path.is_file():
        with inclusion_path.open(newline="", encoding="utf-8") as source:
            for row in csv.DictReader(source):
                if row.get("tx_hash"):
                    relevant_hashes.add(row["tx_hash"])
                if row.get("block_number"):
                    selected_blocks.add(row["block_number"])

    execution_times = load_execution_times(log_path)
    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    output_csv = output_dir / "samples.csv"
    if block_rows:
        fieldnames = list(block_rows[0])
        if "execution_time_ms" not in fieldnames:
            fieldnames.append("execution_time_ms")
        with output_csv.open("w", newline="", encoding="utf-8") as destination:
            writer = csv.DictWriter(destination, fieldnames=fieldnames)
            writer.writeheader()
            for row in block_rows:
                if not row.get("execution_time_ms"):
                    try:
                        row["execution_time_ms"] = execution_times.get(int(row["block_number"]), "")
                    except (TypeError, ValueError):
                        row["execution_time_ms"] = ""
                writer.writerow(row)
    else:
        output_csv.write_text("\n", encoding="utf-8")

    samples, coverage = derive_log_samples(events, relevant_hashes, selected_blocks)
    with (output_dir / "events.jsonl").open("w", encoding="utf-8") as event_file:
        for event in events:
            event_file.write(json.dumps(event, sort_keys=True) + "\n")
    with (output_dir / "log_metric_samples.jsonl").open("w", encoding="utf-8") as sample_file:
        for sample in samples:
            sample_file.write(json.dumps(sample, sort_keys=True, allow_nan=False) + "\n")

    ingestion_error = None
    try:
        ingest_samples(args.api_url, args.token, samples)
    except (RuntimeError, ValueError) as exc:
        ingestion_error = str(exc)
        print(f"Log-derived samples were saved locally but not ingested: {exc}", file=sys.stderr)
    coverage["ingestion_error"] = ingestion_error
    coverage["structured_events_available"] = bool(events)
    (output_dir / "log_metrics_coverage.json").write_text(
        json.dumps(coverage, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def main():
    parser = argparse.ArgumentParser(description="Parse structured execution-client log fields for empty-block samples.")
    parser.add_argument("--metadata", required=True)
    parser.add_argument("--logs-dir", required=True)
    parser.add_argument("--block-samples", required=True)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--experiment-id", required=True)
    parser.add_argument("--client", default="unavailable")
    parser.add_argument("--api-url", default="http://127.0.0.1:9100")
    parser.add_argument("--token", default=None)
    args = parser.parse_args()
    if args.token is None:
        args.token = os.environ.get("METRICS_INGEST_TOKEN", "")
    parse_logs(args)


if __name__ == "__main__":
    main()