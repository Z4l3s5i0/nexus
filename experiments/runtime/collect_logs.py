#!/usr/bin/env python3
import argparse
import json
import os
import re
import shlex
import sys
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

try:
    from .metrics_api import ingest_samples, metric_sample
except ImportError:
    from metrics_api import ingest_samples, metric_sample


EVENT = re.compile(r"\[EXP\]\s+([A-Z][A-Z0-9_]*)(?:\s+(.*))?")
TIMESTAMP = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z?")
TRANSACTION_HASH = re.compile(r"^0x[0-9a-fA-F]{64}$")


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


def _number(attributes, key):
    try:
        value = float(attributes[key])
    except (KeyError, TypeError, ValueError):
        return None
    return value if value >= 0 else None


def _block_number(attributes):
    value = attributes.get("number")
    if value is None:
        return None
    try:
        return int(value, 0)
    except (TypeError, ValueError):
        try:
            return int(float(value))
        except (TypeError, ValueError, OverflowError):
            return None


def _sample(event, name, value, labels=None):
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


def derive_samples(events, workload_windows):
    samples = []
    submitted = defaultdict(set)
    committed = defaultdict(set)
    transaction_receive_time = {}
    transaction_senders = {}
    active_blocks = {}
    block_transactions = {}
    imported_blocks = defaultdict(int)
    event_counts = defaultdict(int)

    for event in events:
        name = event["event"]
        attributes = event["attributes"]
        node_id = event["node_id"]
        event_counts[name] += 1
        if name == "TX_RPC_RECV":
            tx_hash = attributes.get("hash", "")
            if TRANSACTION_HASH.fullmatch(tx_hash):
                tx_hash = tx_hash.lower()
                window = workload_windows.get(node_id)
                if (
                    window is not None
                    and event["timestamp_ms"] is not None
                    and window["start_ms"] <= event["timestamp_ms"] <= window["end_ms"]
                ):
                    submitted[node_id].add(tx_hash)
                if event["timestamp_ms"] is not None:
                    transaction_receive_time[(node_id, tx_hash)] = event["timestamp_ms"]
        elif name == "TX_EXEC_START":
            tx_hash = attributes.get("hash", "")
            if TRANSACTION_HASH.fullmatch(tx_hash):
                tx_hash = tx_hash.lower()
                active = active_blocks.get(node_id)
                if active is not None:
                    active["transactions"].add(tx_hash)
                transaction_senders[(node_id, tx_hash)] = attributes.get("sender", "unavailable")
                received_at = transaction_receive_time.get((node_id, tx_hash))
                if received_at is not None and event["timestamp_ms"] is not None:
                    elapsed = event["timestamp_ms"] - received_at
                    if elapsed >= 0:
                        sample = _sample(
                            event,
                            "log_submit_to_execution_ms",
                            elapsed,
                            {
                                "transaction_hash": tx_hash,
                                "sender": attributes.get("sender", "unavailable"),
                                "runtime_variant": event.get("client_variant", "unknown"),
                            },
                        )
                        if sample:
                            samples.append(sample)
        elif name == "TX_EXEC_END":
            tx_hash = attributes.get("hash", "").lower()
            sender = transaction_senders.get((node_id, tx_hash), "unavailable")
            gas_used = _number(attributes, "gas_used")
            elapsed = _number(attributes, "elapsed_ms")
            labels = {
                "transaction_hash": tx_hash,
                "sender": sender,
                "runtime_variant": event.get("client_variant", "unknown"),
            }
            if gas_used is not None:
                sample = _sample(event, "log_transaction_gas_used", gas_used, {**labels, "unit": "gas"})
                if sample:
                    samples.append(sample)
            if elapsed is not None:
                sample = _sample(event, "log_transaction_elapsed_ms", elapsed, {**labels, "unit": "ms"})
                if sample:
                    samples.append(sample)
        elif name == "BLOCK_EXEC_START":
            active_blocks[node_id] = {
                "number": _block_number(attributes),
                "transactions": set(),
            }
            sample = _sample(
                event,
                "log_block_number",
                _number(attributes, "number"),
                {"event": name, "runtime_variant": event.get("client_variant", "unknown")},
            )
            if sample:
                samples.append(sample)
        elif name == "BLOCK_EXEC_END":
            active = active_blocks.pop(node_id, None)
            if active is not None and active["number"] is not None:
                ended_number = _block_number(attributes)
                if ended_number is None or ended_number == active["number"]:
                    block_transactions[(node_id, active["number"])] = active["transactions"]
            sample = _sample(
                event,
                "log_block_elapsed_ms",
                _number(attributes, "elapsed_ms"),
                {"event": name, "runtime_variant": event.get("client_variant", "unknown")},
            )
            if sample:
                samples.append(sample)
        elif name == "IMPORT_END":
            tx_count = _number(attributes, "txs")
            block_number = _block_number(attributes)
            if tx_count is not None:
                sample = _sample(
                    event,
                    "log_block_transaction_count",
                    tx_count,
                    {"event": name, "runtime_variant": event.get("client_variant", "unknown")},
                )
                if sample:
                    samples.append(sample)
            if block_number is not None:
                imported_blocks[node_id] += 1
                committed[node_id].update(
                    block_transactions.get((node_id, block_number), set()) & submitted[node_id]
                )
                sample = _sample(
                    event,
                    "log_block_number",
                    block_number,
                    {"event": name, "runtime_variant": event.get("client_variant", "unknown")},
                )
                if sample:
                    samples.append(sample)


    node_events = defaultdict(list)
    for event in events:
        node_events[event["node_id"]].append(event)
    for node_id, node_samples in node_events.items():
        representative = node_samples[-1]
        tx_count = len(committed.get(node_id, set()))
        submitted_count = len(submitted.get(node_id, set()))
        window = workload_windows.get(node_id)
        workload_duration = (
            (window["end_ms"] - window["start_ms"]) / 1000
            if window is not None else 0
        )
        if workload_duration > 0 and submitted_count > 0 and imported_blocks.get(node_id, 0) > 0:
            labels = {
                "basis": "unique submitted workload transactions confirmed in imported blocks / recorded spam window",
                "committed_transactions": tx_count,
                "submitted_transactions": submitted_count,
                "workload_window_seconds": workload_duration,
                "runtime_variant": representative.get("client_variant", "unknown"),
            }
            sample = _sample(
                representative,
                "log_tps",
                tx_count / workload_duration,
                labels,
            )
            if sample:
                samples.append(sample)
        if submitted_count > 0 and imported_blocks.get(node_id, 0) > 0:
            sample = _sample(
                representative,
                "log_commit_to_submit_ratio",
                tx_count / submitted_count,
                {
                    "committed_transactions": tx_count,
                    "submitted_transactions": submitted_count,
                    "basis": "unique workload hashes in imported blocks / unique workload hashes received during spam window",
                    "workload_window_seconds": workload_duration,
                    "runtime_variant": representative.get("client_variant", "unknown"),
                },
            )
            if sample:
                samples.append(sample)

    coverage = {
        "transaction_hash": sum(
            1 for event in events if event["event"] == "TX_EXEC_START" and TRANSACTION_HASH.fullmatch(event["attributes"].get("hash", ""))
        ),
        "sender": sum(
            1 for event in events if event["event"] == "TX_EXEC_START" and event["attributes"].get("sender")
        ),
        "gas_used": sum(1 for sample in samples if sample["name"] == "log_transaction_gas_used"),
        "elapsed_time": sum(1 for sample in samples if sample["name"] in ("log_transaction_elapsed_ms", "log_block_elapsed_ms", "log_submit_to_execution_ms")),
        "block_number": sum(1 for sample in samples if sample["name"] == "log_block_number"),
        "transaction_count": sum(1 for sample in samples if sample["name"] == "log_block_transaction_count"),
        "peer_id": sum(1 for event in events if event["attributes"].get("peer")),
        "download_type": sum(1 for event in events if event["event"].startswith("DOWNLOAD_")),
        "tps": sum(1 for sample in samples if sample["name"] == "log_tps"),
        "commit_to_submit_ratio": sum(1 for sample in samples if sample["name"] == "log_commit_to_submit_ratio"),
        "workload_window_nodes": len(workload_windows),
        "workload_submitted_transactions": sum(len(values) for values in submitted.values()),
        "workload_committed_transactions": sum(len(values) for values in committed.values()),
        "event_counts": dict(sorted(event_counts.items())),
    }
    return samples, coverage


def capture_events(args, metadata):
    events = []
    for node in metadata.get("nodes", []):
        index = node["index"]
        node_id = node.get("node_id") or f"el-node-{index}"
        log_path = Path(args.logs_dir) / f"el-node-{index}.log"
        if not log_path.is_file():
            continue
        for line in log_path.read_text(encoding="utf-8", errors="replace").splitlines():
            event = parse_event_line(
                line,
                args.experiment_id,
                node_id,
                node.get("client") or "unavailable",
                str(metadata.get("chain_id", "unknown")),
                node.get("variant") or node.get("type") or "unknown",
            )
            if event:
                events.append(event)
    return events


def load_workload_windows(directory, metadata, experiment_id):
    windows = {}
    directory = Path(directory)
    for node in metadata.get("nodes", []):
        index = node["index"]
        node_id = node.get("node_id") or f"el-node-{index}"
        path = directory / f"node_{index}.json"
        try:
            window = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, UnicodeError, json.JSONDecodeError) as exc:
            raise ValueError(f"cannot read workload window {path}: {exc}") from exc
        if not isinstance(window, dict):
            raise ValueError(f"workload window {path} must contain a JSON object")
        if window.get("experiment_id") != experiment_id or window.get("node_index") != index:
            raise ValueError(f"workload window identity mismatch: {path}")
        if window.get("exit_code") != 0:
            raise ValueError(f"workload command failed for node {index} (exit {window.get('exit_code')})")
        start_ms = window.get("start_ms")
        end_ms = window.get("end_ms")
        if (
            isinstance(start_ms, bool) or not isinstance(start_ms, int)
            or isinstance(end_ms, bool) or not isinstance(end_ms, int)
            or end_ms <= start_ms
        ):
            raise ValueError(f"workload window has invalid bounds: {path}")
        windows[node_id] = {"start_ms": start_ms, "end_ms": end_ms}
    return windows


def main():
    parser = argparse.ArgumentParser(description="Parse structured execution-client log events.")
    parser.add_argument("--metadata", required=True)
    parser.add_argument("--logs-dir", required=True)
    parser.add_argument("--workload-windows", required=True)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--experiment-id", required=True)
    parser.add_argument("--api-url", default="http://127.0.0.1:9100")
    parser.add_argument("--token", default=None)
    args = parser.parse_args()
    if args.token is None:
        args.token = os.environ.get("METRICS_INGEST_TOKEN", "")

    metadata = json.loads(Path(args.metadata).read_text(encoding="utf-8"))
    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    events = capture_events(args, metadata)
    workload_windows = load_workload_windows(args.workload_windows, metadata, args.experiment_id)
    samples, coverage = derive_samples(events, workload_windows)

    with (output_dir / "events.jsonl").open("w", encoding="utf-8") as event_file:
        for event in events:
            event_file.write(json.dumps(event, sort_keys=True) + "\n")
    with (output_dir / "log_metric_samples.jsonl").open("w", encoding="utf-8") as sample_file:
        for sample in samples:
            sample_file.write(json.dumps(sample, allow_nan=False, sort_keys=True) + "\n")

    ingestion_error = None
    try:
        ingest_samples(args.api_url, args.token, samples)
    except (RuntimeError, ValueError) as exc:
        ingestion_error = str(exc)
        print(f"Log samples saved locally but not ingested: {exc}", file=sys.stderr)
    coverage["ingestion_error"] = ingestion_error
    coverage["requested_fields"] = {
        field: "available" if count else "unavailable in captured structured events"
        for field, count in coverage.items()
        if field in {
            "transaction_hash", "sender", "gas_used", "elapsed_time", "block_number",
            "transaction_count", "peer_id", "download_type", "tps", "commit_to_submit_ratio",
        }
    }
    (output_dir / "log_metrics_coverage.json").write_text(
        json.dumps(coverage, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    main()