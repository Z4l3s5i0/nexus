#!/usr/bin/env python3
import argparse
import json
import re
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

try:
    from .metrics_api import ingest_samples, metric_sample
except ImportError:
    from metrics_api import ingest_samples, metric_sample


BYTE_UNITS = {
    "B": 1,
    "kB": 1000,
    "MB": 1000**2,
    "GB": 1000**3,
    "TB": 1000**4,
    "KiB": 1024,
    "MiB": 1024**2,
    "GiB": 1024**3,
    "TiB": 1024**4,
}
BYTE_VALUE = re.compile(r"^\s*(\d+(?:\.\d+)?)\s*(B|kB|MB|GB|TB|KiB|MiB|GiB|TiB)\s*$")
CONTAINER_NAME = re.compile(r"(?:^|[-_])el-node-(\d+)(?:[-_]|$)")
DISK_PARTITION = re.compile(r"(?:sd[a-z]+|vd[a-z]+|xvd[a-z]+)\d+$|(?:nvme\d+n\d+|mmcblk\d+)p\d+$")


def parse_bytes(value):
    match = BYTE_VALUE.fullmatch(value)
    if not match:
        return None
    return float(match.group(1)) * BYTE_UNITS[match.group(2)]


def parse_pair(value):
    if not isinstance(value, str):
        return None, None
    parts = value.split("/", 1)
    if len(parts) != 2:
        return None, None
    return parse_bytes(parts[0]), parse_bytes(parts[1])


def parse_docker_stats(row):
    name = row.get("Name")
    if not isinstance(name, str):
        return None
    cpu = None
    try:
        cpu_text = row.get("CPUPerc", "").strip().rstrip("%")
        cpu = float(cpu_text) if cpu_text else None
    except (TypeError, ValueError):
        pass
    memory, _ = parse_pair(row.get("MemUsage"))
    disk_read, disk_write = parse_pair(row.get("BlockIO"))
    network_rx, network_tx = parse_pair(row.get("NetIO"))
    index = CONTAINER_NAME.search(name)
    if not index:
        return None
    return {
        "container_name": name,
        "node_index": int(index.group(1)),
        "cpu_percent": cpu,
        "memory_bytes": memory,
        "disk_read_bytes": disk_read,
        "disk_write_bytes": disk_write,
        "network_rx_bytes": network_rx,
        "network_tx_bytes": network_tx,
    }


def _read_ints(line):
    try:
        return [int(value) for value in line.split()]
    except ValueError:
        return []


def read_host_counters(proc_root=Path("/proc")):
    proc_root = Path(proc_root)
    counters = {}
    stat_lines = (proc_root / "stat").read_text(encoding="utf-8").splitlines()
    cpu_line = next((line for line in stat_lines if line.startswith("cpu ")), None)
    if cpu_line:
        cpu_values = _read_ints(cpu_line[4:])
        if len(cpu_values) >= 4:
            counters["cpu_total"] = sum(cpu_values)
            counters["cpu_idle"] = cpu_values[3] + (cpu_values[4] if len(cpu_values) > 4 else 0)
    ctxt_line = next((line for line in stat_lines if line.startswith("ctxt ")), None)
    if ctxt_line:
        ctxt = _read_ints(ctxt_line[5:])
        if ctxt:
            counters["context_switches"] = ctxt[0]

    memory = {}
    for line in (proc_root / "meminfo").read_text(encoding="utf-8").splitlines():
        key, separator, value = line.partition(":")
        if separator:
            parts = value.split()
            if parts and parts[0].isdigit():
                memory[key] = int(parts[0]) * (1024 if len(parts) > 1 and parts[1] == "kB" else 1)
    if "MemTotal" in memory and "MemAvailable" in memory:
        counters["memory_used_bytes"] = memory["MemTotal"] - memory["MemAvailable"]

    disk_read_sectors = 0
    disk_write_sectors = 0
    found_disk = False
    for line in (proc_root / "diskstats").read_text(encoding="utf-8").splitlines():
        values = line.split()
        if len(values) < 10:
            continue
        device = values[2]
        if device.startswith(("loop", "ram", "dm-")) or DISK_PARTITION.fullmatch(device):
            continue
        try:
            disk_read_sectors += int(values[5])
            disk_write_sectors += int(values[9])
            found_disk = True
        except ValueError:
            continue
    if found_disk:
        counters["disk_read_bytes"] = disk_read_sectors * 512
        counters["disk_write_bytes"] = disk_write_sectors * 512

    network_rx = 0
    network_tx = 0
    found_network = False
    for line in (proc_root / "net" / "dev").read_text(encoding="utf-8").splitlines()[2:]:
        interface, separator, values = line.partition(":")
        fields = values.split()
        if separator and interface.strip() != "lo" and len(fields) >= 9:
            try:
                network_rx += int(fields[0])
                network_tx += int(fields[8])
                found_network = True
            except ValueError:
                continue
    if found_network:
        counters["network_rx_bytes"] = network_rx
        counters["network_tx_bytes"] = network_tx

    try:
        load_values = (proc_root / "loadavg").read_text(encoding="utf-8").split()
        counters["load_1m"], counters["load_5m"], counters["load_15m"] = map(float, load_values[:3])
    except (OSError, ValueError):
        pass
    return counters


def host_metric_samples(current, previous, elapsed_seconds, timestamp_ms, experiment_id, network):
    samples = []
    scope = {"scope": "host"}

    def add(name, value, unit):
        if value is not None:
            samples.append(
                metric_sample(
                    timestamp_ms,
                    experiment_id,
                    "host",
                    network,
                    "host",
                    "System",
                    name,
                    float(value),
                    {**scope, "unit": unit},
                )
            )

    for field, name in (("load_1m", "system_load_1m"), ("load_5m", "system_load_5m"), ("load_15m", "system_load_15m")):
        add(name, current.get(field), "load_average")
    add("system_context_switches_total", current.get("context_switches"), "count")
    add("system_memory_used_bytes", current.get("memory_used_bytes"), "bytes")

    if previous is not None and elapsed_seconds > 0:
        total_delta = current.get("cpu_total", 0) - previous.get("cpu_total", 0)
        idle_delta = current.get("cpu_idle", 0) - previous.get("cpu_idle", 0)
        if total_delta > 0 and 0 <= idle_delta <= total_delta:
            add("system_cpu_utilization_percent", 100.0 * (total_delta - idle_delta) / total_delta, "percent")
        for field, name in (
            ("disk_read_bytes", "system_disk_read_bytes_per_second"),
            ("disk_write_bytes", "system_disk_write_bytes_per_second"),
            ("network_rx_bytes", "system_network_receive_bytes_per_second"),
            ("network_tx_bytes", "system_network_transmit_bytes_per_second"),
            ("context_switches", "system_context_switches_per_second"),
        ):
            if field in current and field in previous:
                delta = current[field] - previous[field]
                if delta >= 0:
                    add(name, delta / elapsed_seconds, "per_second")
    return samples


def container_metric_samples(
    rows,
    nodes_by_index,
    timestamp_ms,
    experiment_id,
    network,
    previous=None,
    sample_time=None,
):
    samples = []
    for row in rows:
        parsed = parse_docker_stats(row)
        if parsed is None:
            continue
        node = nodes_by_index.get(parsed["node_index"], {})
        node_id = node.get("node_id") or f"el-node-{parsed['node_index']}"
        client = node.get("client") or "unavailable"
        labels = {"scope": "container", "container_name": parsed["container_name"]}
        variant = node.get("variant") or node.get("type")
        if variant:
            labels["runtime_variant"] = variant
        for field, name, unit in (
            ("cpu_percent", "system_container_cpu_utilization_percent", "percent"),
            ("memory_bytes", "system_container_memory_usage_bytes", "bytes"),
            ("disk_read_bytes", "system_container_disk_read_bytes", "bytes"),
            ("disk_write_bytes", "system_container_disk_write_bytes", "bytes"),
            ("network_rx_bytes", "system_container_network_receive_bytes", "bytes"),
            ("network_tx_bytes", "system_container_network_transmit_bytes", "bytes"),
        ):
            value = parsed.get(field)
            if value is not None:
                samples.append(
                    metric_sample(
                        timestamp_ms,
                        experiment_id,
                        node_id,
                        network,
                        client,
                        "System",
                        name,
                        float(value),
                        {**labels, "unit": unit},
                    )
                )
        if previous is not None and sample_time is not None:
            previous_values = previous.get(parsed["container_name"], {})
            for field, name in (
                ("disk_read_bytes", "system_container_disk_read_bytes_per_second"),
                ("disk_write_bytes", "system_container_disk_write_bytes_per_second"),
                ("network_rx_bytes", "system_container_network_receive_bytes_per_second"),
                ("network_tx_bytes", "system_container_network_transmit_bytes_per_second"),
            ):
                value = parsed.get(field)
                prior = previous_values.get(field)
                if value is not None and prior is not None:
                    delta, elapsed = value - prior[0], sample_time - prior[1]
                    if delta >= 0 and elapsed > 0:
                        samples.append(
                            metric_sample(
                                timestamp_ms,
                                experiment_id,
                                node_id,
                                network,
                                client,
                                "System",
                                name,
                                delta / elapsed,
                                {**labels, "unit": "bytes_per_second"},
                            )
                        )
            previous[parsed["container_name"]] = {
                field: (parsed[field], sample_time)
                for field in ("disk_read_bytes", "disk_write_bytes", "network_rx_bytes", "network_tx_bytes")
                if parsed.get(field) is not None
            }
    return samples


def docker_stats():
    result = subprocess.run(
        ["docker", "stats", "--no-stream", "--format", "{{json .}}"],
        capture_output=True,
        text=True,
        check=False,
        timeout=30,
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
    metadata = json.loads(Path(args.metadata).read_text(encoding="utf-8"))
    nodes_by_index = {node["index"]: node for node in metadata.get("nodes", [])}
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    token = args.token
    started = time.monotonic()
    previous = None
    previous_time = None
    previous_containers = {}
    failures = 0

    with output.open("a", encoding="utf-8") as samples_file:
        while time.monotonic() - started < args.duration:
            iteration_started = time.monotonic()
            timestamp_ms = int(datetime.now(timezone.utc).timestamp() * 1000)
            batch = []
            try:
                current = read_host_counters(args.proc_root)
                now = time.monotonic()
                batch.extend(
                    host_metric_samples(
                        current,
                        previous,
                        now - previous_time if previous_time is not None else 0,
                        timestamp_ms,
                        args.experiment_id,
                        args.network,
                    )
                )
                previous = current
                previous_time = now
            except (OSError, ValueError) as exc:
                print(f"Host system sample unavailable: {exc}", file=sys.stderr)

            try:
                rows = docker_stats()
                container_sample_time = time.monotonic()
                batch.extend(
                    container_metric_samples(
                        rows,
                        nodes_by_index,
                        timestamp_ms,
                        args.experiment_id,
                        args.network,
                        previous_containers,
                        container_sample_time,
                    )
                )
            except (OSError, RuntimeError, subprocess.SubprocessError) as exc:
                print(f"Container system sample unavailable: {exc}", file=sys.stderr)

            for sample in batch:
                samples_file.write(json.dumps(sample, allow_nan=False) + "\n")
            samples_file.flush()
            try:
                ingest_samples(args.api_url, token, batch)
            except (RuntimeError, ValueError) as exc:
                failures += 1
                print(f"System samples saved locally but not ingested: {exc}", file=sys.stderr)

            remaining = args.interval - (time.monotonic() - iteration_started)
            if remaining > 0:
                time.sleep(min(remaining, max(0, args.duration - (time.monotonic() - started))))

    if failures:
        print(f"System metrics collection finished with {failures} ingestion failure(s).", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description="Collect per-container and host runtime metrics.")
    parser.add_argument("--metadata", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--experiment-id", required=True)
    parser.add_argument("--network", required=True)
    parser.add_argument("--api-url", default="http://127.0.0.1:9100")
    parser.add_argument("--token", default=None)
    parser.add_argument("--duration", type=float, required=True)
    parser.add_argument("--interval", type=float, default=1.0)
    parser.add_argument("--proc-root", type=Path, default=Path("/proc"))
    args = parser.parse_args()
    if args.duration <= 0 or args.interval <= 0:
        parser.error("--duration and --interval must be positive")
    if args.token is None:
        import os

        args.token = os.environ.get("METRICS_INGEST_TOKEN", "")
    collect(args)


if __name__ == "__main__":
    main()