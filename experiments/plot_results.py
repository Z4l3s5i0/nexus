#!/usr/bin/env python3
"""Generate client-comparison plots from saved experiment reports."""

import argparse
import csv
import html
import json
import math
import os
import re
import sys
from collections import defaultdict
from pathlib import Path

try:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    import pandas as pd
    import seaborn as sns
except ImportError:
    matplotlib = None
    plt = None
    pd = None
    sns = None


MODULES = ("runtime", "empty-block", "startup", "compliance", "size")
CLIENT_ORDER = ("WASIX", "NEXUS native", "Geth", "Reth")
CLIENT_COLORS = {
    "WASIX": "#e76f51",
    "NEXUS native": "#4361ee",
    "Geth": "#2a9d8f",
    "Reth": "#8d5fd3",
    "NEXUS WASIX (Wasmer)": "#f4a261",
}
PALETTE = ("#577590", "#90be6d", "#f9c74f", "#f9844a", "#277da1", "#b56576")


class Accumulator:
    def __init__(self):
        self.total = 0.0
        self.weight = 0.0
        self.observations = 0

    def add(self, value, weight=1.0, observations=1):
        try:
            value = float(value)
            weight = float(weight)
        except (TypeError, ValueError):
            return
        if not math.isfinite(value) or not math.isfinite(weight) or weight <= 0:
            return
        self.total += value * weight
        self.weight += weight
        self.observations += int(observations)

    @property
    def mean(self):
        return self.total / self.weight if self.weight else None


class Chart:
    def __init__(self, module, key, title, x_label, y_label, group="", percent=False, note="", kind="bar",
                 lower_is_better=False, color_min=None, color_max=None):
        self.module = module
        self.key = key
        self.title = title
        self.x_label = x_label
        self.y_label = y_label
        self.group = group
        self.percent = percent
        self.note = note
        self.kind = kind
        self.lower_is_better = lower_is_better
        self.color_min = color_min
        self.color_max = color_max
        self.workload_duration_seconds = None
        self.workload_tps = None
        self.workload_counts_available = None
        self.points = defaultdict(Accumulator)

    def add(self, category, series, value, weight=1.0, observations=1):
        self.points[(str(category), str(series))].add(value, weight, observations)

    def categories(self):
        values = {category for category, _series in self.points if self.points[(category, _series)].weight}
        try:
            return sorted(values, key=lambda value: float(value))
        except ValueError:
            return sorted(values, key=str.casefold)

    def series(self):
        values = {series for _category, series in self.points if self.points[(_category, series)].weight}
        known = [label for label in CLIENT_ORDER if label in values]
        return known + sorted(values - set(known), key=str.casefold)


def safe_slug(value):
    slug = re.sub(r"[^A-Za-z0-9._-]+", "-", str(value).strip()).strip("-.")
    return slug or "comparison"


def client_label(client="", variant=None, config=None):
    client_text = str(client or "").strip()
    variant_text = str(variant or "").strip()
    config_text = str(config or "").strip()
    combined = " ".join((client_text, variant_text, config_text)).lower().replace("_", "-")
    normalized_client = client_text.lower().replace("_", "-")
    is_nexus = normalized_client in {"wasix-eth", "nexus", "native", "wasix"} or "wasix-eth" in normalized_client

    if "nexus-linux" in combined:
        return "NEXUS native"
    if "wasmer" in combined:
        return "NEXUS WASIX (Wasmer)"
    if "geth" in combined:
        return "Geth"
    if "reth" in combined:
        return "Reth"
    if is_nexus and "native" in combined:
        return "NEXUS native"
    if is_nexus:
        if normalized_client == "wasix-eth" and not variant_text and not config_text:
            return "NEXUS"
        if normalized_client in {"wasix-r-eth", "wasix-w-eth"}:
            return normalized_client.upper()
        return "WASIX"
    if "wasix" in combined:
        return "WASIX"
    if client_text:
        return client_text.replace("_", "-").upper()
    return config_text.replace("_", "-").upper() or "Unknown client"


def read_json(path, default=None):
    try:
        with Path(path).open(encoding="utf-8") as source:
            return json.load(source)
    except (OSError, ValueError):
        return default


def iter_reports(results_dir, module, warnings):
    module_dir = Path(results_dir) / module
    if not module_dir.is_dir():
        return
    for report_path in sorted(module_dir.glob("*/report.json")):
        report = read_json(report_path)
        if not isinstance(report, dict):
            warnings.append(f"Skipping unreadable report: {report_path}")
            continue
        run_status = read_json(report_path.parent / "run_status.json", {}) or {}
        if not run_status:
            run_status = report.get("run_status", {}) or {}
        status = str(run_status.get("status", "")).lower()
        try:
            exit_code = int(run_status.get("exit_code", 0))
        except (TypeError, ValueError):
            exit_code = 0
        if status in {"failed", "error", "interrupted", "cancelled", "canceled", "skipped", "aborted"} or exit_code != 0:
            warnings.append(f"Skipping non-successful {module} run: {report_path.parent.name}")
            continue
        metadata = read_json(report_path.parent / "metadata.json", {}) or {}
        yield report_path.parent, report, metadata


def add_chart(charts, module, key, title, x_label, y_label, category, series, value,
              weight=1.0, observations=1, group="", percent=False, note="", kind="bar",
              lower_is_better=False, color_min=None, color_max=None):
    chart_id = (module, key)
    chart = charts.get(chart_id)
    if chart is None:
        chart = Chart(
            module, key, title, x_label, y_label, group, percent, note, kind,
            lower_is_better, color_min, color_max,
        )
        charts[chart_id] = chart
    chart.add(category, series, value, weight, observations)


def collect_empty_block(charts, run_dir, report, metadata):
    client = client_label(
        metadata.get("client") or report.get("client"),
        metadata.get("client_variant"),
    )
    workload = str(metadata.get("workload") or report.get("workload") or "none")
    transaction_types = report.get("transaction_inclusion_latency", {})
    if workload.lower() in {"", "none", "null"} and len(transaction_types) == 1:
        workload = next(iter(transaction_types))
    elif workload.lower() in {"", "none", "null"}:
        workload = "all-transactions"
    label = workload.replace("_", "-")
    workload_slug = safe_slug(label.lower())

    for transaction_type, targets in report.get("transaction_inclusion_latency_by_target", {}).items():
        if not isinstance(targets, dict):
            continue
        chart_workload = label
        for target, spreads in targets.items():
            if not isinstance(spreads, dict):
                continue
            for spread_data in spreads.values():
                if not isinstance(spread_data, dict):
                    continue
                try:
                    value = float(spread_data["mean_ms"])
                    observations = int(spread_data.get("observations", 0))
                    category = int(target)
                except (KeyError, TypeError, ValueError):
                    continue
                if observations <= 0:
                    continue
                add_chart(
                    charts, "empty-block", f"{workload_slug}-inclusion-latency",
                    f"{chart_workload.replace('-', ' ').title()} transaction inclusion latency",
                    "Target transactions", "Mean inclusion latency (ms)", category, client,
                    value, observations, observations, chart_workload,
                    note="Latency is measured from send acknowledgment to first receipt observation.",
                    kind="knee",
                )

    for target, summary in report.get("by_target_transaction_count", {}).items():
        if not isinstance(summary, dict):
            continue
        try:
            category = int(target)
        except (TypeError, ValueError):
            continue

        burst = summary.get("burst_completion_latency_ms", {})
        if isinstance(burst, dict):
            try:
                value = float(burst["mean"])
                observations = int(burst.get("observations", 0))
            except (KeyError, TypeError, ValueError):
                value, observations = None, 0
            if value is not None and observations > 0:
                add_chart(
                    charts, "empty-block", f"{workload_slug}-burst-completion",
                    f"{label.replace('-', ' ').title()} burst completion latency",
                    "Target transactions", "Mean burst completion latency (ms)", category, client,
                    value, observations, observations, label,
                    note="Burst latency runs from first send acknowledgment to last receipt observation.",
                    kind="knee",
                )

        try:
            expected = int(summary.get("expected_matches") or 0)
            matches = int(summary.get("exact_block_matches", summary.get("matched", 0)) or 0)
        except (TypeError, ValueError):
            expected, matches = 0, 0
        if expected > 0:
            add_chart(
                charts, "empty-block", f"{workload_slug}-exact-match-rate",
                f"{label.replace('-', ' ').title()} exact block match rate heatmap",
                "Target transactions", "Exact-match rate (%)", category, client,
                matches / expected, expected, expected, label, percent=True, kind="heatmap",
            )

        try:
            expected_bursts = int(summary.get("expected_bursts") or 0)
            completed_bursts = int(summary.get("completed_bursts") or 0)
        except (TypeError, ValueError):
            expected_bursts, completed_bursts = 0, 0
        if expected_bursts > 0:
            add_chart(
                charts, "empty-block", f"{workload_slug}-burst-success-rate",
                f"{label.replace('-', ' ').title()} completed burst rate",
                "Target transactions", "Completed burst rate (%)", category, client,
                completed_bursts / expected_bursts, expected_bursts, expected_bursts,
                label, percent=True, kind="heatmap",
            )

    burst_path = Path(run_dir) / "transaction_burst_samples.csv"
    if burst_path.is_file():
        with burst_path.open(newline="", encoding="utf-8") as source:
            for burst in csv.DictReader(source):
                if str(burst.get("completed", "")).lower() != "true":
                    continue
                try:
                    target = int(burst["target_tx_count"])
                    blocks_spanned = int(burst["blocks_spanned"])
                except (KeyError, TypeError, ValueError):
                    continue
                if target <= 0 or blocks_spanned < 1:
                    continue
                add_chart(
                    charts, "empty-block", f"{workload_slug}-blocks-spanned",
                    f"{label.replace('-', ' ').title()} mean blocks per completed burst heatmap",
                    "Target transactions", "Mean blocks spanned per completed burst",
                    target, client, blocks_spanned, group=label,
                    note="completed bursts only; green=1 block (best), red=5+ (worst, capped).",
                    kind="heatmap", lower_is_better=True, color_min=1, color_max=5,
                )


def runtime_cohort(metadata, nodes, report):
    workload = metadata.get("workload") or {}
    scenario = str(workload.get("scenario") or report.get("scenario") or "unknown")
    client_counts = defaultdict(int)
    counts = metadata.get("counts") or {}
    if isinstance(counts, dict):
        for client_type, count in counts.items():
            try:
                count = int(count)
            except (TypeError, ValueError):
                continue
            if count <= 0:
                continue
            label = client_label(
                "wasix-eth" if client_type in {"native", "wasix"} else client_type,
                client_type,
            )
            client_counts[label] += count
    if not client_counts:
        for node in metadata.get("nodes", []):
            label = client_label(
                node.get("client"), node.get("variant") or node.get("type"),
            )
            client_counts[label] += 1

    node_count = sum(client_counts.values()) or len(nodes)
    ratio_divisor = math.gcd(*client_counts.values()) if client_counts else 1
    mix = " + ".join(
        f"{client_counts[client] // ratio_divisor} {client}"
        for client in sorted(client_counts, key=_client_sort)
    ) or "Unknown mix"
    cohort_mix = "single-client runs" if len(client_counts) == 1 else f"mixed-client mix: {mix}"
    profile = []
    for key, label in (
        ("tps", "TPS/node"),
        ("duration_seconds", "s"),
        ("wait_seconds", "wait s"),
        ("accounts_per_spammer", "accounts/node"),
    ):
        value = workload.get(key)
        if value is not None:
            profile.append(f"{value} {label}")
    group = " · ".join((scenario, cohort_mix, *profile))
    slug = safe_slug("-".join((scenario, cohort_mix, *profile))).lower()
    return node_count, group, slug


def runtime_workload_counts(run_dir):
    counts_by_node = defaultdict(list)
    samples_path = Path(run_dir) / "log_metric_samples.jsonl"
    if not samples_path.is_file():
        return counts_by_node
    with samples_path.open(encoding="utf-8") as source:
        for line in source:
            try:
                sample = json.loads(line)
                if sample.get("name") != "log_tps":
                    continue
                labels = sample.get("labels_json") or "{}"
                if isinstance(labels, str):
                    labels = json.loads(labels)
                node_id = str(sample.get("node_id", ""))
                committed = int(labels["committed_transactions"])
                if node_id and committed >= 0:
                    counts_by_node[node_id].append(committed)
            except (json.JSONDecodeError, KeyError, TypeError, ValueError):
                continue
    return counts_by_node


def collect_runtime(charts, run_dir, report, metadata):
    nodes = {
        str(node.get("node_id")): node
        for node in metadata.get("nodes", [])
        if node.get("node_id")
    }
    node_count, cohort, cohort_slug = runtime_cohort(metadata, nodes, report)
    outcomes = report.get("workload_outcomes", {})
    workload = metadata.get("workload") or {}
    try:
        workload_duration = float(workload.get("duration_seconds"))
    except (TypeError, ValueError):
        workload_duration = 0
    try:
        workload_tps = float(workload.get("tps"))
    except (TypeError, ValueError):
        workload_tps = None
    workload_counts = runtime_workload_counts(run_dir)
    for metric, chart_key, title, y_label in (
        ("tps", "throughput", "Runtime confirmed workload throughput by cluster size", "Confirmed workload TPS per node"),
        ("commit_to_submit_ratio", "commit-ratio", "Runtime commit-to-submit ratio by cluster size", "Commit-to-submit ratio (%)"),
    ):
        summary = outcomes.get(metric, {})
        for node_sample in summary.get("per_node", []):
            try:
                value = float(node_sample["mean"])
                observations = int(node_sample.get("observations", 1))
            except (KeyError, TypeError, ValueError):
                continue
            if observations <= 0:
                continue
            node_id = str(node_sample.get("node_id", ""))
            committed_counts = workload_counts.get(node_id, [])
            if metric == "tps" and committed_counts and workload_duration > 0:
                value = sum(committed_counts) / len(committed_counts) / workload_duration
                observations = len(committed_counts)
            node = nodes.get(str(node_sample.get("node_id")), {})
            label = client_label(
                node.get("client") or node_sample.get("client"),
                node.get("variant") or node.get("type"),
            )
            if metric == "commit_to_submit_ratio" and value > 1:
                value /= 100.0
            add_chart(
                charts, "runtime", f"{chart_key}-{cohort_slug}", title,
                "Cluster size (nodes)", y_label, node_count, label, value, 1, observations,
                cohort, percent=metric == "commit_to_submit_ratio",
                note=(
                    "Confirmed transaction counts are divided by configured workload duration; older reports without archived counts retain reported TPS."
                    if metric == "tps" else
                    "Each node contributes one run mean; single-client runs share a cohort, while mixed-client proportions remain separate."
                ),
                kind="line" if metric == "tps" else "heatmap",
            )
            if metric == "tps":
                throughput_chart = charts[("runtime", f"{chart_key}-{cohort_slug}")]
                if throughput_chart.workload_counts_available is None:
                    throughput_chart.workload_counts_available = True
                throughput_chart.workload_counts_available = (
                    throughput_chart.workload_counts_available
                    and bool(committed_counts)
                    and workload_duration > 0
                )
                if workload_duration > 0:
                    throughput_chart.workload_duration_seconds = workload_duration
                if workload_tps is not None:
                    throughput_chart.workload_tps = workload_tps

    node_resource = defaultdict(Accumulator)
    samples_path = run_dir / "system_metric_samples.jsonl"
    wanted = {
        "system_container_cpu_utilization_percent": ("container-cpu", "Runtime average container CPU by cluster size", "CPU utilization (%)"),
        "system_container_memory_usage_bytes": ("container-memory", "Runtime average container memory by cluster size", "Memory usage (MiB)"),
    }
    if samples_path.is_file():
        with samples_path.open(encoding="utf-8") as source:
            for line in source:
                try:
                    sample = json.loads(line)
                    metric = sample.get("name")
                    if metric not in wanted:
                        continue
                    labels = json.loads(sample.get("labels_json") or "{}")
                    if labels.get("scope") not in (None, "container"):
                        continue
                    node_id = str(sample.get("node_id", ""))
                    node = nodes.get(node_id, {})
                    label = client_label(
                        node.get("client") or sample.get("client"),
                        node.get("variant") or node.get("type") or labels.get("runtime_variant"),
                    )
                    value = float(sample["value"])
                    if metric == "system_container_memory_usage_bytes":
                        value /= 1024 * 1024
                    node_resource[(metric, label, node_id)].add(value)
                except (json.JSONDecodeError, KeyError, TypeError, ValueError):
                    continue

    for (metric, label, _node_id), values in node_resource.items():
        if values.mean is None:
            continue
        chart_key, title, y_label = wanted[metric]
        add_chart(
            charts, "runtime", f"{chart_key}-{cohort_slug}", title,
            "Cluster size (nodes)", y_label, node_count, label, values.mean, 1, values.observations,
            cohort,
            note="Per-node means use container-scoped samples; host-wide measurements are excluded.",
            kind="heatmap",
        )


def collect_startup(charts, report):
    for config, summary in report.get("configurations", {}).items():
        startup = summary.get("startup_time_ms", {})
        try:
            ready = int(summary.get("iterations_ready", 0) or 0)
            requested = int(summary.get("iterations_requested", 0) or 0)
        except (KeyError, TypeError, ValueError):
            ready, requested = 0, 0
        try:
            mean = float(startup["mean"])
        except (KeyError, TypeError, ValueError):
            mean = None
        label = client_label(config=config)
        if mean is not None and ready > 0:
            add_chart(
                charts, "startup", "startup-time", "Startup readiness time by client",
                "Client configuration", "Mean time to matching eth_chainId response (ms)",
                label, "Mean startup time", mean, ready, ready,
                note="Run means are weighted by the number of ready iterations; failed starts are omitted.",
            )
        if requested > 0:
            add_chart(
                charts, "startup", "readiness-rate", "Startup readiness success rate",
                "Client configuration", "Ready iterations (%)", label, "Readiness rate",
                ready / requested, requested, requested, percent=True,
            )


def collect_size(charts, report):
    for report_key, chart_key, title, unit in (
        ("source_code", "source-lines", "Source code size by client", "Code lines"),
        ("compiled_binaries", "binary-bytes", "Compiled binary size by client", "Binary size (bytes)"),
    ):
        for row in report.get(report_key, []):
            if str(row.get("status", "")).lower() not in {"measured", "available", "ok"}:
                continue
            try:
                value = float(row["value"])
            except (KeyError, TypeError, ValueError):
                continue
            label = client_label(client=row.get("label"))
            add_chart(
                charts, "size", chart_key, title, "Client/source label", unit,
                label, unit, value, 1, 1,
            )


def collect_compliance(charts, report):
    for result in report.get("results", []):
        try:
            passed = int(result.get("exit_code")) == 0
        except (TypeError, ValueError):
            passed = str(result.get("status", "")).lower() in {"passed", "pass", "success"}
        label = client_label(client=result.get("client"))
        add_chart(
            charts, "compliance", "hive-pass-rate", "Hive ethereum/engine compliance pass rate",
            "Client", "Passing client runs (%)", label, "Pass rate", 1.0 if passed else 0.0,
            1, 1, percent=True,
            note="Compliance is a pass/fail protocol check, not a throughput benchmark.",
        )


def _client_sort(value):
    if value in CLIENT_ORDER:
        return (0, CLIENT_ORDER.index(value))
    return (1, value.casefold())


def _format_number(value):
    if abs(value) >= 10000:
        return f"{value:,.0f}"
    if abs(value) >= 100:
        return f"{value:,.1f}".rstrip("0").rstrip(".")
    return f"{value:.2f}".rstrip("0").rstrip(".")


def estimate_knee(chart, series):
    points = []
    for category in chart.categories():
        value = chart.points[(category, series)].mean
        if value is None:
            continue
        try:
            x_value = float(category)
        except ValueError:
            return None
        x_value = math.log10(x_value + 1) if x_value >= 0 else x_value
        points.append((category, x_value, value))
    if len(points) < 4:
        return None

    x_start, y_start = points[0][1:]
    x_end, y_end = points[-1][1:]
    x_span = x_end - x_start
    y_min = min(point[2] for point in points)
    y_span = max(point[2] for point in points) - y_min
    if x_span <= 0 or y_span <= 0:
        return None

    y_start = (y_start - y_min) / y_span
    y_end = (y_end - y_min) / y_span
    slope = (y_end - y_start)
    denominator = math.sqrt(1 + slope * slope)
    candidate = None
    maximum_distance = 0.05
    for category, x_value, value in points[1:-1]:
        x_normalized = (x_value - x_start) / x_span
        y_normalized = (value - y_min) / y_span
        distance = abs(y_normalized - (y_start + slope * x_normalized)) / denominator
        if distance > maximum_distance:
            candidate = category
            maximum_distance = distance
    return candidate


def _heatmap_color(value, minimum, maximum, percent, lower_is_better=False):
    span = maximum - minimum
    position = min(1.0, max(0.0, (value - minimum) / span)) if span else 0.0
    if lower_is_better:
        stops = (
            (0.0, (102, 189, 99)),
            (0.5, (255, 239, 163)),
            (1.0, (239, 125, 111)),
        )
    elif percent:
        stops = ((0.0, (239, 125, 111)), (0.5, (255, 239, 163)), (1.0, (102, 189, 99)))
    else:
        stops = ((0.0, (255, 247, 236)), (1.0, (179, 0, 0)))
    for index in range(len(stops) - 1):
        start, start_rgb = stops[index]
        end, end_rgb = stops[index + 1]
        if position <= end:
            fraction = (position - start) / (end - start)
            rgb = tuple(round(left + (right - left) * fraction) for left, right in zip(start_rgb, end_rgb))
            return "#" + "".join(f"{channel:02x}" for channel in rgb)
    return "#" + "".join(f"{channel:02x}" for channel in stops[-1][1])


def render_line_svg(chart):
    categories = chart.categories()
    series = sorted(chart.series(), key=_client_sort)
    if not categories or not series:
        return None

    width, height = 1120, 650
    left, right, top, bottom = 95, 35, 125, 150
    plot_width = width - left - right
    plot_height = height - top - bottom
    plot_bottom = top + plot_height
    try:
        raw_x = [float(category) for category in categories]
    except ValueError:
        raw_x = list(range(len(categories)))
    x_values = [math.log10(value + 1) if chart.kind == "knee" and value >= 0 else value for value in raw_x]
    min_x, max_x = min(x_values), max(x_values)
    x_span = max_x - min_x or 1.0
    plotted = [
        accumulator.mean * (100 if chart.percent else 1)
        for accumulator in chart.points.values()
        if accumulator.mean is not None
    ]
    maximum = 100.0 if chart.percent else max(plotted, default=1.0) * 1.12
    maximum = maximum if maximum > 0 else 1.0

    def x_for(value):
        return left + ((value - min_x) / x_span) * plot_width

    def y_for(value):
        return plot_bottom - (value / maximum) * plot_height

    lines = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img">',
        f"<title>{html.escape(chart.title)}</title>",
        '<rect width="100%" height="100%" fill="#ffffff"/>',
        f'<text x="{width / 2:.0f}" y="42" text-anchor="middle" font-family="Arial,sans-serif" font-size="25" font-weight="bold" fill="#17212b">{html.escape(chart.title)}</text>',
        f'<text x="{width / 2:.0f}" y="72" text-anchor="middle" font-family="Arial,sans-serif" font-size="13" fill="#536471">{html.escape(chart.group)}</text>' if chart.group else "",
    ]
    for tick in range(6):
        value = maximum * tick / 5
        y = y_for(value)
        suffix = "%" if chart.percent else ""
        lines.append(f'<line x1="{left}" y1="{y:.1f}" x2="{width - right}" y2="{y:.1f}" stroke="#dce3e8"/>')
        lines.append(f'<text x="{left - 12}" y="{y + 5:.1f}" text-anchor="end" font-family="Arial,sans-serif" font-size="12" fill="#52616b">{html.escape(_format_number(value))}{suffix}</text>')

    for index, name in enumerate(series):
        color = CLIENT_COLORS.get(name, PALETTE[index % len(PALETTE)])
        coordinates = []
        for category_index, category in enumerate(categories):
            accumulator = chart.points[(category, name)]
            if accumulator.mean is None:
                continue
            value = accumulator.mean * (100 if chart.percent else 1)
            x, y = x_for(x_values[category_index]), y_for(value)
            coordinates.append((category, x, y, accumulator.observations, value))
        if len(coordinates) > 1:
            path = " ".join(f"{x:.1f},{y:.1f}" for _category, x, y, _count, _value in coordinates)
            lines.append(f'<polyline points="{path}" fill="none" stroke="{color}" stroke-width="3" stroke-linejoin="round"/>')
        knee = estimate_knee(chart, name) if chart.kind == "knee" else None
        for category, x, y, observations, value in coordinates:
            tooltip = f"{name}: {_format_number(value)}{'%' if chart.percent else ''} (n={observations})"
            lines.append(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="5" fill="{color}" stroke="#ffffff" stroke-width="1.5"><title>{html.escape(tooltip)}</title></circle>')
            if category == knee:
                lines.append(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="9" fill="none" stroke="#17212b" stroke-width="2"><title>Exploratory knee candidate for {html.escape(name)} at {html.escape(category)} transactions</title></circle>')

    for index, category in enumerate(categories):
        x = x_for(x_values[index])
        y = plot_bottom + 25
        rotate = len(category) > 8 or len(categories) > 8
        transform = f' transform="rotate(-35 {x:.1f} {y:.1f})"' if rotate else ""
        anchor = "end" if rotate else "middle"
        lines.append(f'<text x="{x:.1f}" y="{y:.1f}" text-anchor="{anchor}" font-family="Arial,sans-serif" font-size="12" fill="#34434d"{transform}>{html.escape(category)}</text>')

    legend_x, legend_y = left, 100
    for index, name in enumerate(series):
        color = CLIENT_COLORS.get(name, PALETTE[index % len(PALETTE)])
        lines.append(f'<line x1="{legend_x}" y1="{legend_y - 4}" x2="{legend_x + 16}" y2="{legend_y - 4}" stroke="{color}" stroke-width="3"/>')
        lines.append(f'<text x="{legend_x + 21}" y="{legend_y}" font-family="Arial,sans-serif" font-size="13" fill="#34434d">{html.escape(name)}</text>')
        legend_x += min(245, 35 + len(name) * 8)
        if legend_x > width - 190:
            legend_x, legend_y = left, legend_y + 22
    x_label = chart.x_label + (" (log10(target + 1))" if chart.kind == "knee" else "")
    if chart.kind == "knee":
        note = "Exploratory knee marker: greatest normalized deviation from the endpoint chord; not a capacity guarantee."
    else:
        note = chart.note
    lines.extend([
        f'<text x="{left + plot_width / 2:.1f}" y="{height - 28}" text-anchor="middle" font-family="Arial,sans-serif" font-size="14" fill="#34434d">{html.escape(x_label)}</text>',
        f'<text x="24" y="{top + plot_height / 2:.1f}" text-anchor="middle" font-family="Arial,sans-serif" font-size="14" fill="#34434d" transform="rotate(-90 24 {top + plot_height / 2:.1f})">{html.escape(chart.y_label)}</text>',
    ])
    if note:
        lines.append(f'<text x="{width / 2}" y="{height - 7}" text-anchor="middle" font-family="Arial,sans-serif" font-size="10" fill="#64727c">{html.escape(note)}</text>')
    lines.append("</svg>")
    return "\n".join(line for line in lines if line)


def render_heatmap_svg(chart):
    categories = chart.categories()
    series = sorted(chart.series(), key=_client_sort)
    if not categories or not series:
        return None

    cell_width, cell_height = 88, 52
    left, top = 170, 125
    width = max(760, left + cell_width * len(categories) + 60)
    height = max(420, top + cell_height * len(series) + 125)
    values = [
        accumulator.mean * (100 if chart.percent else 1)
        for accumulator in chart.points.values()
        if accumulator.mean is not None
    ]
    minimum = chart.color_min if chart.color_min is not None else 0.0
    maximum = chart.color_max if chart.color_max is not None else (100.0 if chart.percent else max(values, default=1.0))
    maximum = maximum if maximum > minimum else minimum + 1.0
    lines = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img">',
        f"<title>{html.escape(chart.title)}</title>",
        '<rect width="100%" height="100%" fill="#ffffff"/>',
        f'<text x="{width / 2:.0f}" y="42" text-anchor="middle" font-family="Arial,sans-serif" font-size="24" font-weight="bold" fill="#17212b">{html.escape(chart.title)}</text>',
        f'<text x="{width / 2:.0f}" y="72" text-anchor="middle" font-family="Arial,sans-serif" font-size="13" fill="#536471">{html.escape(chart.group)}</text>' if chart.group else "",
        f'<text x="{left / 2:.0f}" y="{top + cell_height * len(series) / 2:.1f}" text-anchor="middle" font-family="Arial,sans-serif" font-size="14" fill="#34434d">Client</text>',
    ]
    for category_index, category in enumerate(categories):
        x = left + category_index * cell_width
        lines.append(f'<text x="{x + cell_width / 2}" y="{top - 14}" text-anchor="middle" font-family="Arial,sans-serif" font-size="12" fill="#34434d">{html.escape(category)}</text>')
    for series_index, name in enumerate(series):
        y = top + series_index * cell_height
        lines.append(f'<text x="{left - 12}" y="{y + cell_height / 2 + 5}" text-anchor="end" font-family="Arial,sans-serif" font-size="13" fill="#34434d">{html.escape(name)}</text>')
        for category_index, category in enumerate(categories):
            x = left + category_index * cell_width
            accumulator = chart.points[(category, name)]
            if accumulator.mean is None:
                lines.append(f'<rect x="{x}" y="{y}" width="{cell_width}" height="{cell_height}" fill="#f0f2f4" stroke="#ffffff"><title>{html.escape(name)}: unavailable</title></rect>')
                continue
            value = accumulator.mean * (100 if chart.percent else 1)
            color = _heatmap_color(value, minimum, maximum, chart.percent, chart.lower_is_better)
            display = f"{_format_number(value)}%" if chart.percent else _format_number(value)
            lines.append(f'<rect x="{x}" y="{y}" width="{cell_width}" height="{cell_height}" fill="{color}" stroke="#ffffff" stroke-width="2"><title>{html.escape(name)} at {html.escape(category)}: {html.escape(display)} (n={accumulator.observations})</title></rect>')
            lines.append(f'<text x="{x + cell_width / 2}" y="{y + cell_height / 2 + 5}" text-anchor="middle" font-family="Arial,sans-serif" font-size="13" fill="#17212b">{html.escape(display)}</text>')
    legend_y = top + cell_height * len(series) + 36
    maximum_suffix = "%" if chart.percent else "+" if chart.lower_is_better else ""
    lines.extend([
        f'<defs><linearGradient id="heatmap-scale"><stop offset="0%" stop-color="{_heatmap_color(minimum, minimum, maximum, chart.percent, chart.lower_is_better)}"/><stop offset="100%" stop-color="{_heatmap_color(maximum, minimum, maximum, chart.percent, chart.lower_is_better)}"/></linearGradient></defs>',
        f'<rect x="{left}" y="{legend_y}" width="220" height="14" fill="url(#heatmap-scale)"/>',
        f'<text x="{left}" y="{legend_y + 32}" font-family="Arial,sans-serif" font-size="11" fill="#52616b">{html.escape(_format_number(minimum))}</text>',
        f'<text x="{left + 220}" y="{legend_y + 32}" text-anchor="end" font-family="Arial,sans-serif" font-size="11" fill="#52616b">{html.escape(_format_number(maximum))}{maximum_suffix}</text>',
        f'<text x="{left + cell_width * len(categories) / 2:.1f}" y="{height - 18}" text-anchor="middle" font-family="Arial,sans-serif" font-size="14" fill="#34434d">{html.escape(chart.x_label)}</text>',
    ])
    if chart.note:
        lines.append(f'<text x="{width / 2}" y="{height - 4}" text-anchor="middle" font-family="Arial,sans-serif" font-size="10" fill="#64727c">{html.escape(chart.note)}</text>')
    lines.append("</svg>")
    return "\n".join(line for line in lines if line)


def render_svg(chart):
    if chart.kind == "heatmap":
        return render_heatmap_svg(chart)
    if chart.kind in {"line", "knee"}:
        return render_line_svg(chart)
    return render_bar_svg(chart)


def render_plot(chart):
    if sns is not None and chart.kind in {"line", "knee", "heatmap"}:
        return render_seaborn(chart)
    return render_svg(chart)


def render_seaborn(chart):
    categories = chart.categories()
    series = sorted(chart.series(), key=_client_sort)
    rows = []
    for category_index, category in enumerate(categories):
        for name in series:
            value = chart.points[(category, name)].mean
            if value is None:
                continue
            try:
                x_value = float(category)
            except ValueError:
                x_value = float(category_index)
            if chart.kind == "knee" and x_value >= 0:
                x_value = math.log10(x_value + 1)
            rows.append({
                "category": category,
                "series": name,
                "value": value * (100 if chart.percent else 1),
                "x_value": x_value,
            })
    if not rows:
        return None

    frame = pd.DataFrame(rows)
    palette = {name: CLIENT_COLORS.get(name, PALETTE[index % len(PALETTE)]) for index, name in enumerate(series)}
    sns.set_theme(style="whitegrid", context="notebook")
    figure, axis = plt.subplots(figsize=(11.2, 6.5), constrained_layout=True)
    if chart.kind == "heatmap":
        matrix = frame.pivot(index="series", columns="category", values="value").reindex(index=series, columns=categories)
        fmt = ".0f" if chart.percent else ".1f"
        minimum = chart.color_min if chart.color_min is not None else 0
        maximum = chart.color_max if chart.color_max is not None else (100 if chart.percent else max(frame["value"].max(), 1))
        sns.heatmap(
            matrix, annot=True, fmt=fmt, mask=matrix.isna(),
            cmap="RdYlGn_r" if chart.lower_is_better else "RdYlGn" if chart.percent else "YlOrRd",
            vmin=minimum, vmax=maximum,
            linewidths=0.6, linecolor="white", cbar_kws={"label": chart.y_label}, ax=axis,
        )
        if chart.lower_is_better:
            colorbar = axis.collections[0].colorbar
            colorbar.set_ticks([minimum, maximum])
            colorbar.set_ticklabels([_format_number(minimum), f"{_format_number(maximum)}+"])
        axis.set_xlabel(chart.x_label)
        axis.set_ylabel("Client")
    else:
        sns.lineplot(
            data=frame, x="x_value", y="value", hue="series", hue_order=series,
            palette=palette, marker="o", sort=True, ci=None, ax=axis,
        )
        ticks = frame.groupby("category", sort=False)["x_value"].first()
        axis.set_xticks(ticks.to_list(), labels=ticks.index.to_list())
        axis.set_xlabel(chart.x_label + (" (log10(target + 1))" if chart.kind == "knee" else ""))
        if chart.kind == "knee":
            for name in series:
                category = estimate_knee(chart, name)
                if category is None:
                    continue
                row = frame[(frame["series"] == name) & (frame["category"] == category)].iloc[0]
                axis.scatter(row["x_value"], row["value"], marker="X", s=95, color=palette[name], edgecolor="#17212b", zorder=5)
    axis.set_title(chart.title)
    axis.set_ylabel(chart.y_label)
    if chart.group:
        axis.text(0.5, 1.02, chart.group, ha="center", va="bottom", transform=axis.transAxes, color="#536471")
    notes = []
    if chart.kind == "knee":
        notes.append("X marks an exploratory knee candidate (greatest deviation from the endpoint chord), not a capacity guarantee.")
    if chart.note:
        notes.append(chart.note)
    if notes:
        figure.text(0.5, -0.02, " ".join(notes), ha="center", fontsize=8, color="#64727c")
    import io

    destination = io.StringIO()
    figure.savefig(destination, format="svg", bbox_inches="tight")
    plt.close(figure)
    return destination.getvalue()


def render_bar_svg(chart):
    categories = chart.categories()
    series = sorted(chart.series(), key=_client_sort)
    if not categories or not series:
        return None

    width, height = 1120, 650
    left, right, top, bottom = 95, 35, 125, 150
    plot_width = width - left - right
    plot_height = height - top - bottom
    plot_bottom = top + plot_height
    plotted = [
        chart.points[(category, name)].mean * (100 if chart.percent else 1)
        for category in categories for name in series
        if chart.points[(category, name)].mean is not None
    ]
    maximum = 100.0 if chart.percent else max(plotted, default=1.0) * 1.12
    if maximum <= 0:
        maximum = 1.0

    def y_for(value):
        return plot_bottom - (value / maximum) * plot_height

    lines = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img">',
        f"<title>{html.escape(chart.title)}</title>",
        '<rect width="100%" height="100%" fill="#ffffff"/>',
        f'<text x="{width / 2:.0f}" y="42" text-anchor="middle" font-family="Arial,sans-serif" font-size="25" font-weight="bold" fill="#17212b">{html.escape(chart.title)}</text>',
        f'<text x="{width / 2:.0f}" y="72" text-anchor="middle" font-family="Arial,sans-serif" font-size="14" fill="#536471">{html.escape(chart.group)}</text>' if chart.group else "",
    ]

    for tick in range(6):
        value = maximum * tick / 5
        y = y_for(value)
        displayed = value
        suffix = "%" if chart.percent else ""
        lines.append(f'<line x1="{left}" y1="{y:.1f}" x2="{width - right}" y2="{y:.1f}" stroke="#dce3e8" stroke-width="1"/>')
        lines.append(f'<text x="{left - 12}" y="{y + 5:.1f}" text-anchor="end" font-family="Arial,sans-serif" font-size="12" fill="#52616b">{html.escape(_format_number(displayed))}{suffix}</text>')

    group_width = plot_width / len(categories)
    series_count = max(1, len(series))
    bar_width = min(42.0, group_width * 0.72 / series_count)
    for category_index, category in enumerate(categories):
        center = left + group_width * (category_index + 0.5)
        used_width = bar_width * series_count
        group_start = center - used_width / 2
        for series_index, name in enumerate(series):
            accumulator = chart.points[(category, name)]
            value = accumulator.mean
            if value is None:
                continue
            visual_value = value * (100 if chart.percent else 1)
            y = y_for(visual_value)
            bar_height = max(1.0, plot_bottom - y)
            x = group_start + series_index * bar_width
            color = CLIENT_COLORS.get(name, PALETTE[series_index % len(PALETTE)])
            tooltip = f"{name}: {_format_number(visual_value)}{'%' if chart.percent else ''} (n={accumulator.observations})"
            lines.append(f'<rect x="{x:.1f}" y="{y:.1f}" width="{max(2.0, bar_width - 3):.1f}" height="{bar_height:.1f}" rx="2" fill="{color}"><title>{html.escape(tooltip)}</title></rect>')
        display_category = category if len(category) <= 23 else category[:20] + "..."
        label_x = center
        label_y = plot_bottom + 25
        rotate = len(category) > 12 or len(categories) > 8
        transform = f' transform="rotate(-35 {label_x:.1f} {label_y:.1f})"' if rotate else ""
        anchor = "end" if rotate else "middle"
        lines.append(f'<text x="{label_x:.1f}" y="{label_y:.1f}" text-anchor="{anchor}" font-family="Arial,sans-serif" font-size="12" fill="#34434d"{transform}><title>{html.escape(category)}</title>{html.escape(display_category)}</text>')

    legend_y = 100 if chart.group else 82
    legend_x = left
    for index, name in enumerate(series):
        color = CLIENT_COLORS.get(name, PALETTE[index % len(PALETTE)])
        item_width = min(245, 20 + len(name) * 8)
        lines.append(f'<rect x="{legend_x}" y="{legend_y - 12}" width="13" height="13" rx="2" fill="{color}"/>')
        lines.append(f'<text x="{legend_x + 19}" y="{legend_y}" font-family="Arial,sans-serif" font-size="13" fill="#34434d">{html.escape(name)}</text>')
        legend_x += item_width
        if legend_x > width - 190:
            legend_x = left
            legend_y += 23

    lines.extend([
        f'<text x="{left + plot_width / 2:.1f}" y="{height - 28}" text-anchor="middle" font-family="Arial,sans-serif" font-size="14" fill="#34434d">{html.escape(chart.x_label)}</text>',
        f'<text x="24" y="{top + plot_height / 2:.1f}" text-anchor="middle" font-family="Arial,sans-serif" font-size="14" fill="#34434d" transform="rotate(-90 24 {top + plot_height / 2:.1f})">{html.escape(chart.y_label)}</text>',
    ])
    if chart.note:
        lines.append(f'<text x="{width / 2}" y="{height - 7}" text-anchor="middle" font-family="Arial,sans-serif" font-size="11" fill="#64727c">{html.escape(chart.note)}</text>')
    lines.append("</svg>")
    return "\n".join(line for line in lines if line)


def runtime_comparison_rows(charts):
    rows = []
    for chart in charts:
        if chart.module != "runtime" or not chart.key.startswith("throughput-"):
            continue
        for category in chart.categories():
            clients = chart.series()
            for first_index, client_a in enumerate(clients):
                value_a = chart.points[(category, client_a)].mean
                if value_a is None:
                    continue
                for client_b in clients[first_index + 1:]:
                    value_b = chart.points[(category, client_b)].mean
                    if value_b is None:
                        continue
                    change = (
                        f"{(value_b - value_a) / abs(value_a) * 100:+.1f}%"
                        if value_a != 0 else f"{value_b - value_a:+.2f} TPS/node"
                    )
                    duration = chart.workload_duration_seconds
                    target_tps = chart.workload_tps
                    target_per_node = (
                        duration * target_tps
                        if duration is not None and target_tps is not None else None
                    )
                    try:
                        target_cluster = target_per_node * float(category) if target_per_node is not None else None
                        cluster_nodes = float(category)
                    except (TypeError, ValueError):
                        target_cluster = None
                        cluster_nodes = None
                    counts_available = chart.workload_counts_available is True
                    single_client = "single-client runs" in chart.group
                    tx_per_node_a = value_a * duration if duration is not None and counts_available else None
                    tx_per_node_b = value_b * duration if duration is not None and counts_available else None
                    rows.append({
                        "cohort": chart.group,
                        "nodes": category,
                        "client_a": client_a,
                        "client_b": client_b,
                        "value_a": _format_number(value_a),
                        "value_b": _format_number(value_b),
                        "transactions_per_node_a": _format_number(tx_per_node_a) if tx_per_node_a is not None else "unavailable",
                        "transactions_per_node_b": _format_number(tx_per_node_b) if tx_per_node_b is not None else "unavailable",
                        "transactions_cluster_a": _format_number(tx_per_node_a * cluster_nodes) if tx_per_node_a is not None and cluster_nodes is not None and single_client else "unavailable",
                        "transactions_cluster_b": _format_number(tx_per_node_b * cluster_nodes) if tx_per_node_b is not None and cluster_nodes is not None and single_client else "unavailable",
                        "target_transactions_per_node": target_per_node,
                        "target_transactions_cluster": target_cluster,
                        "change": change,
                    })
    return rows


def empty_block_insights(charts):
    completion = [
        accumulator.mean
        for chart in charts
        if chart.module == "empty-block" and chart.key.endswith("-burst-success-rate")
        for accumulator in chart.points.values()
        if accumulator.mean is not None
    ]
    exact_match = [
        accumulator.mean
        for chart in charts
        if chart.module == "empty-block"
        and chart.key.endswith("-exact-match-rate")
        and not chart.key.startswith("all-transactions-")
        for accumulator in chart.points.values()
        if accumulator.mean is not None
    ]
    insights = []
    if completion:
        minimum, maximum = min(completion), max(completion)
        if minimum == maximum:
            range_text = f"{_format_number(minimum * 100)}%"
        else:
            range_text = f"{_format_number(minimum * 100)}–{_format_number(maximum * 100)}%"
        insights.append(
            f"Completed-burst rates range from {range_text} across {len(completion)} "
            "workload/client/target cells in successful reports."
        )
    if exact_match:
        below = sum(value < 1 - 1e-9 for value in exact_match)
        insights.append(
            f"Exact target transaction counts were not observed in one block in {below} "
            f"of {len(exact_match)} plotted workload/client/target cells; the lowest rate was "
            f"{_format_number(min(exact_match) * 100)}%. This is distinct from completing the burst."
        )
    return insights


def write_index(output_dir, charts, report_counts, warnings):
    lines = [
        "<!doctype html>",
        '<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">',
        "<title>Experiment client comparisons</title>",
        "<style>body{font:16px Arial,sans-serif;max-width:1180px;margin:2rem auto;padding:0 1rem;color:#17212b}h1,h2{color:#243746}section{margin:2.5rem 0}figure{margin:1.5rem 0 2.5rem}img{width:100%;height:auto;border:1px solid #dce3e8}li{margin:.35rem 0}.note{color:#52616b}</style></head><body>",
        "<h1>Experiment client comparisons</h1>",
        '<p class="note">Generated from saved experiment reports. Missing observations are omitted, not drawn as zero. WASIX is highlighted for comparison; startup, compliance, source size, and runtime resource measurements have different meanings and should not be treated as one combined score.</p>',
        "<h2>Included runs</h2><ul>",
    ]
    for module in MODULES:
        lines.append(f"<li>{html.escape(module)}: {report_counts.get(module, 0)} successful report(s)</li>")
    lines.append("</ul>")
    if warnings:
        lines.extend(["<h2>Skipped runs</h2><ul>"])
        lines.extend(f"<li>{html.escape(message)}</li>" for message in warnings)
        lines.append("</ul>")

    comparisons = runtime_comparison_rows(charts)
    block_insights = empty_block_insights(charts)
    if comparisons or block_insights:
        lines.append("<section><h2>Key insights from included runs</h2>")
        if comparisons:
            lines.append(
                '<p class="note">Runtime throughput comparisons include every available client pair within the same workload profile and cluster size. '
                "Confirmed transactions per node are divided by the configured active workload duration (not total network or collection time). "
                "The target is configured TPS/node × active seconds; total target transactions also multiply by node count. Values are run means, not confidence estimates.</p>"
            )
            lines.extend([
                "<table><thead><tr><th>Workload cohort</th><th>Nodes</th><th>Client A</th><th>Client B</th>"
                "<th>Confirmed TPS/node A</th><th>Confirmed TPS/node B</th>"
                "<th>Confirmed tx/node A</th><th>Confirmed tx/node B</th>"
                "<th>Confirmed tx/cluster A</th><th>Confirmed tx/cluster B</th>"
                "<th>Target tx/node</th><th>Target tx/cluster</th><th>Relative change (B vs A)</th></tr></thead><tbody>"
            ])
            for row in comparisons:
                target_per_node = (
                    _format_number(row["target_transactions_per_node"])
                    if row["target_transactions_per_node"] is not None else "unavailable"
                )
                target_cluster = (
                    _format_number(row["target_transactions_cluster"])
                    if row["target_transactions_cluster"] is not None else "unavailable"
                )
                lines.append(
                    "<tr>"
                    f"<td>{html.escape(row['cohort'])}</td>"
                    f"<td>{html.escape(row['nodes'])}</td>"
                    f"<td>{html.escape(row['client_a'])}</td>"
                    f"<td>{html.escape(row['client_b'])}</td>"
                    f"<td>{html.escape(row['value_a'])}</td>"
                    f"<td>{html.escape(row['value_b'])}</td>"
                    f"<td>{html.escape(row['transactions_per_node_a'])}</td>"
                    f"<td>{html.escape(row['transactions_per_node_b'])}</td>"
                    f"<td>{html.escape(row['transactions_cluster_a'])}</td>"
                    f"<td>{html.escape(row['transactions_cluster_b'])}</td>"
                    f"<td>{html.escape(target_per_node)}</td>"
                    f"<td>{html.escape(target_cluster)}</td>"
                    f"<td>{html.escape(row['change'])}</td>"
                    "</tr>"
                )
            lines.append("</tbody></table>")
        if block_insights:
            lines.append("<h3>Empty-block results</h3><ul>")
            lines.extend(f"<li>{html.escape(insight)}</li>" for insight in block_insights)
            lines.append("</ul>")
            lines.append(
                '<p class="note">These summaries cover successful reports only; incomplete or failed runs '
                "are listed separately and are not treated as zero-valued measurements.</p>"
            )
        lines.append("</section>")

    for module in MODULES:
        module_charts = [chart for chart in charts if chart.module == module]
        if not module_charts:
            continue
        lines.append(f"<section><h2>{html.escape(module.replace('-', ' ').title())}</h2>")
        for chart in module_charts:
            image = f"{module}/{safe_slug(chart.key)}.svg"
            lines.append(f'<figure><figcaption><strong>{html.escape(chart.title)}</strong></figcaption><img loading="lazy" src="{html.escape(image, quote=True)}" alt="{html.escape(chart.title, quote=True)}"></figure>')
        lines.append("</section>")
    if not charts:
        lines.append("<p>No comparable report data was found for the selected modules.</p>")
    lines.append('<p class="note">The accompanying <code>plot_data.csv</code> contains the aggregate values used for every chart.</p>')
    lines.append("</body></html>")
    (output_dir / "index.html").write_text("\n".join(lines) + "\n", encoding="utf-8")


def write_plot_data(output_dir, charts):
    path = output_dir / "plot_data.csv"
    columns = ("experiment", "metric", "group", "category", "series", "value", "weight", "observations")
    with path.open("w", newline="", encoding="utf-8") as destination:
        writer = csv.DictWriter(destination, fieldnames=columns)
        writer.writeheader()
        for chart in charts:
            for (category, series), accumulator in sorted(chart.points.items()):
                if accumulator.mean is None:
                    continue
                writer.writerow({
                    "experiment": chart.module,
                    "metric": chart.title,
                    "group": chart.group,
                    "category": category,
                    "series": series,
                    "value": f"{accumulator.mean:.8g}",
                    "weight": f"{accumulator.weight:.8g}",
                    "observations": accumulator.observations,
                })


def generate_plots(results_dir, output_dir, selected_modules=None):
    results_dir = Path(results_dir)
    output_dir = Path(output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    selected = set(selected_modules or MODULES)
    charts = {}
    warnings = []
    report_counts = {module: 0 for module in MODULES}

    collectors = {
        "runtime": collect_runtime,
        "empty-block": collect_empty_block,
        "startup": lambda chart_data, _run_dir, report, _metadata: collect_startup(chart_data, report),
        "compliance": lambda chart_data, _run_dir, report, _metadata: collect_compliance(chart_data, report),
        "size": lambda chart_data, _run_dir, report, _metadata: collect_size(chart_data, report),
    }
    for module in MODULES:
        if module not in selected:
            continue
        for run_dir, report, metadata in iter_reports(results_dir, module, warnings):
            report_counts[module] += 1
            collectors[module](charts, run_dir, report, metadata)

    rendered = []
    for chart in charts.values():
        svg = render_plot(chart)
        if svg is None:
            continue
        module_dir = output_dir / chart.module
        module_dir.mkdir(parents=True, exist_ok=True)
        (module_dir / f"{safe_slug(chart.key)}.svg").write_text(svg + "\n", encoding="utf-8")
        rendered.append(chart)
    write_plot_data(output_dir, rendered)
    write_index(output_dir, rendered, report_counts, warnings)
    used_seaborn = sns is not None and any(chart.kind in {"line", "knee", "heatmap"} for chart in rendered)
    renderer = "Seaborn with SVG bars" if used_seaborn else "standalone SVG"
    return {"charts": len(rendered), "reports": report_counts, "warnings": warnings, "renderer": renderer}


def default_results_dir():
    repo_root = Path(__file__).resolve().parents[1]
    configured = os.environ.get("EXPERIMENTS_OUTPUT_ROOT")
    if configured:
        return Path(configured)
    candidates = (repo_root / "experiment-results", repo_root / "exp" / "experiment-results")
    return next((candidate for candidate in candidates if candidate.is_dir()), candidates[0])


def main():
    parser = argparse.ArgumentParser(description="Plot comparisons from saved experiment results.")
    parser.add_argument("--results-dir", type=Path, default=None, help="Results root (default: EXPERIMENTS_OUTPUT_ROOT, else experiment-results/).")
    parser.add_argument("--output-dir", type=Path, default=None, help="Plot output directory (default: <results-dir>/plots).")
    parser.add_argument("--module", action="append", choices=MODULES, help="Plot only this module; may be supplied more than once.")
    args = parser.parse_args()
    results_dir = args.results_dir or default_results_dir()
    output_dir = args.output_dir or results_dir / "plots"
    if not results_dir.is_dir():
        parser.error(f"results directory does not exist: {results_dir}")
    result = generate_plots(results_dir, output_dir, args.module)
    print(f"Generated {result['charts']} plot(s) in {output_dir}")
    print(f"Renderer: {result['renderer']} (Seaborn is optional).")
    print(f"Open {output_dir / 'index.html'} to browse the comparisons.")
    for module, count in result["reports"].items():
        if args.module is None or module in args.module:
            print(f"  {module}: {count} successful report(s)")
    for warning in result["warnings"]:
        print(f"Warning: {warning}", file=sys.stderr)


if __name__ == "__main__":
    main()