#!/usr/bin/env python3
import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

RUNTIME_HELPERS = Path(__file__).resolve().parents[1] / "runtime"
sys.path.insert(0, str(RUNTIME_HELPERS.parent))
from runtime.collect_system_metrics import host_metric_samples, parse_pair, read_host_counters
from runtime.metrics_api import ingest_samples, metric_sample


def process_counters(pid, proc_root=Path("/proc")):
    process_dir = Path(proc_root) / str(pid)
    stat = (process_dir / "stat").read_text(encoding="utf-8")
    closing_parenthesis = stat.rfind(")")
    if closing_parenthesis < 0:
        raise ValueError(f"invalid proc stat for pid {pid}")
    fields = stat[closing_parenthesis + 2 :].split()
    if len(fields) < 22:
        raise ValueError(f"incomplete proc stat for pid {pid}")
    counters = {
        "cpu_ticks": int(fields[11]) + int(fields[12]),
        "memory_rss_bytes": int(fields[21]) * os.sysconf("SC_PAGE_SIZE"),
    }
    try:
        for line in (process_dir / "io").read_text(encoding="utf-8").splitlines():
            key, separator, value = line.partition(":")
            if separator and key in ("read_bytes", "write_bytes"):
                counters[f"disk_{key}"] = int(value.strip())
    except OSError:
        pass
    return counters


def _labels(sample, config, iteration):
    labels = json.loads(sample["labels_json"]) if sample.get("labels_json") else {}
    labels["startup_config"] = config
    labels["iteration"] = str(iteration)
    sample["labels_json"] = json.dumps(labels, sort_keys=True)
    return sample


def process_samples(current, previous, elapsed, pid, identity, timestamp_ms):
    output = []

    def add(name, value, unit):
        if value is not None:
            output.append(
                metric_sample(
                    timestamp_ms,
                    identity["experiment_id"],
                    identity["node_id"],
                    identity["network"],
                    identity["client"],
                    "System",
                    name,
                    float(value),
                    {"scope": "process", "pid": str(pid), "unit": unit,
                     "startup_config": identity["config"], "iteration": str(identity["iteration"])},
                )
            )

    add("system_process_memory_rss_bytes", current.get("memory_rss_bytes"), "bytes")
    add("system_process_disk_read_bytes", current.get("disk_read_bytes"), "bytes")
    add("system_process_disk_write_bytes", current.get("disk_write_bytes"), "bytes")
    if previous is not None and elapsed > 0:
        ticks = current.get("cpu_ticks", 0) - previous.get("cpu_ticks", 0)
        if ticks >= 0:
            add("system_process_cpu_utilization_percent", ticks / os.sysconf("SC_CLK_TCK") / elapsed * 100, "percent_of_one_core")
        for field, name in (
            ("disk_read_bytes", "system_process_disk_read_bytes_per_second"),
            ("disk_write_bytes", "system_process_disk_write_bytes_per_second"),
        ):
            if field in current and field in previous:
                delta = current[field] - previous[field]
                if delta >= 0:
                    add(name, delta / elapsed, "bytes_per_second")
    return output


def _container_row(container_name, rows):
    return next((row for row in rows if row.get("Name", "").lstrip("/") == container_name), None)


def container_samples(row, previous, sample_time, identity, timestamp_ms):
    if row is None:
        return []
    try:
        cpu_text = row.get("CPUPerc", "").strip().rstrip("%")
        cpu = float(cpu_text) if cpu_text else None
    except (TypeError, ValueError):
        cpu = None
    memory, _ = parse_pair(row.get("MemUsage"))
    disk_read, disk_write = parse_pair(row.get("BlockIO"))
    network_rx, network_tx = parse_pair(row.get("NetIO"))
    current = {
        "disk_read_bytes": disk_read,
        "disk_write_bytes": disk_write,
        "network_rx_bytes": network_rx,
        "network_tx_bytes": network_tx,
    }
    labels = {
        "scope": "container",
        "container_name": row.get("Name", "").lstrip("/"),
        "startup_config": identity["config"],
        "iteration": str(identity["iteration"]),
    }
    output = []

    def add(name, value, unit):
        if value is not None:
            output.append(
                metric_sample(
                    timestamp_ms,
                    identity["experiment_id"],
                    identity["node_id"],
                    identity["network"],
                    identity["client"],
                    "System",
                    name,
                    float(value),
                    {**labels, "unit": unit},
                )
            )

    for field, name, value, unit in (
        ("cpu", "system_container_cpu_utilization_percent", cpu, "percent"),
        ("memory", "system_container_memory_usage_bytes", memory, "bytes"),
        ("disk_read_bytes", "system_container_disk_read_bytes", disk_read, "bytes"),
        ("disk_write_bytes", "system_container_disk_write_bytes", disk_write, "bytes"),
        ("network_rx_bytes", "system_container_network_receive_bytes", network_rx, "bytes"),
        ("network_tx_bytes", "system_container_network_transmit_bytes", network_tx, "bytes"),
    ):
        add(name, value, unit)
    if previous is not None:
        prior = previous.get("values", {})
        elapsed = sample_time - previous.get("time", sample_time)
        if elapsed > 0:
            for field, name in (
                ("disk_read_bytes", "system_container_disk_read_bytes_per_second"),
                ("disk_write_bytes", "system_container_disk_write_bytes_per_second"),
                ("network_rx_bytes", "system_container_network_receive_bytes_per_second"),
                ("network_tx_bytes", "system_container_network_transmit_bytes_per_second"),
            ):
                value = current[field]
                if value is not None and field in prior and value >= prior[field]:
                    add(name, (value - prior[field]) / elapsed, "bytes_per_second")
    next_previous = {field: value for field, value in current.items() if value is not None}
    return output, {"values": next_previous, "time": sample_time}


def docker_stats():
    result = subprocess.run(
        ["docker", "stats", "--no-stream", "--format", "{{json .}}"],
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "docker stats failed")
    rows = []
    for line in result.stdout.splitlines():
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return rows


def collect(args):
    identity = json.loads(Path(args.identity).read_text(encoding="utf-8"))
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    previous_host = None
    previous_host_time = None
    previous_process = None
    previous_process_time = None
    previous_container = None
    failures = 0

    with output.open("a", encoding="utf-8") as samples_file:
        while time.monotonic() - started < args.duration:
            iteration_started = time.monotonic()
            timestamp_ms = int(datetime.now(timezone.utc).timestamp() * 1000)
            batch = []
            try:
                current_host = read_host_counters(args.proc_root)
                current_time = time.monotonic()
                host = host_metric_samples(
                    current_host,
                    previous_host,
                    current_time - previous_host_time if previous_host_time is not None else 0,
                    timestamp_ms,
                    identity["experiment_id"],
                    identity["network"],
                )
                batch.extend(_labels(sample, identity["config"], identity["iteration"]) for sample in host)
                previous_host = current_host
                previous_host_time = current_time
            except (OSError, ValueError) as exc:
                print(f"Host system metrics unavailable: {exc}", file=sys.stderr)

            state_path = Path(args.state_file)
            if state_path.is_file():
                try:
                    state = json.loads(state_path.read_text(encoding="utf-8"))
                    sample_time = time.monotonic()
                    if state.get("container_name"):
                        row = _container_row(state["container_name"], docker_stats())
                        container_output = container_samples(
                            row, previous_container, sample_time, identity, timestamp_ms
                        )
                        if container_output:
                            container_values, previous_container = container_output
                            batch.extend(container_values)
                    elif state.get("pid"):
                        current_process = process_counters(state["pid"], args.proc_root)
                        batch.extend(
                            process_samples(
                                current_process,
                                previous_process,
                                sample_time - previous_process_time if previous_process_time is not None else 0,
                                state["pid"],
                                identity,
                                timestamp_ms,
                            )
                        )
                        previous_process = current_process
                        previous_process_time = sample_time
                except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
                    print(f"Per-client startup metrics unavailable: {exc}", file=sys.stderr)

            for sample in batch:
                samples_file.write(json.dumps(sample, sort_keys=True, allow_nan=False) + "\n")
            samples_file.flush()
            try:
                ingest_samples(args.api_url, args.token, batch)
            except (RuntimeError, ValueError) as exc:
                failures += 1
                print(f"Startup system samples saved locally but not ingested: {exc}", file=sys.stderr)

            remaining = args.interval - (time.monotonic() - iteration_started)
            if remaining > 0:
                time.sleep(min(remaining, max(0, args.duration - (time.monotonic() - started))))
    if failures:
        print(f"Startup system collection had {failures} ingestion failure(s).", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description="Collect per-process/container and host startup metrics.")
    parser.add_argument("--identity", required=True)
    parser.add_argument("--state-file", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--duration", type=float, required=True)
    parser.add_argument("--interval", type=float, default=0.2)
    parser.add_argument("--proc-root", type=Path, default=Path("/proc"))
    parser.add_argument("--api-url", default="http://127.0.0.1:9100")
    parser.add_argument("--token", default=None)
    args = parser.parse_args()
    if args.duration <= 0 or args.interval <= 0:
        parser.error("duration and interval must be positive")
    if args.token is None:
        args.token = os.environ.get("METRICS_INGEST_TOKEN", "")
    collect(args)


if __name__ == "__main__":
    main()