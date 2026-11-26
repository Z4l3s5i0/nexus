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
    def __init__(self, module, key, title, x_label, y_label, group="", percent=False, note=""):
        self.module = module
        self.key = key
        self.title = title
        self.x_label = x_label
        self.y_label = y_label
        self.group = group
        self.percent = percent
        self.note = note
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
              weight=1.0, observations=1, group="", percent=False, note=""):
    chart_id = (module, key)
    chart = charts.get(chart_id)
    if chart is None:
        chart = Chart(module, key, title, x_label, y_label, group, percent, note)
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
                )

        try:
            expected = int(summary.get("expected_matches") or 0)
            matches = int(summary.get("exact_block_matches", summary.get("matched", 0)) or 0)
        except (TypeError, ValueError):
            expected, matches = 0, 0
        if expected > 0:
            add_chart(
                charts, "empty-block", f"{workload_slug}-exact-match-rate",
                f"{label.replace('-', ' ').title()} exact block match rate",
                "Target transactions", "Exact-match rate (%)", category, client,
                matches / expected, expected, expected, label, percent=True,
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
                label, percent=True,
            )


def collect_runtime(charts, run_dir, report, metadata):
    nodes = {
        str(node.get("node_id")): node
        for node in metadata.get("nodes", [])
        if node.get("node_id")
    }
    scenario = str((metadata.get("workload") or {}).get("scenario") or report.get("scenario") or "unknown")
    outcomes = report.get("workload_outcomes", {})
    for metric, chart_key, title, y_label in (
        ("tps", "throughput", "Runtime confirmed workload throughput", "Mean confirmed TPS per node"),
        ("commit_to_submit_ratio", "commit-ratio", "Runtime commit-to-submit ratio", "Commit-to-submit ratio (%)"),
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
            node = nodes.get(str(node_sample.get("node_id")), {})
            label = client_label(
                node.get("client") or node_sample.get("client"),
                node.get("variant") or node.get("type"),
            )
            if metric == "commit_to_submit_ratio" and value > 1:
                value /= 100.0
            add_chart(
                charts, "runtime", chart_key, title, "Workload scenario", y_label,
                scenario, label, value, 1, 1, percent=metric == "commit_to_submit_ratio",
                note="Each node contributes its report mean; bars compare client nodes in the same workload.",
            )

    node_resource = defaultdict(Accumulator)
    samples_path = run_dir / "system_metric_samples.jsonl"
    if samples_path.is_file():
        wanted = {
            "system_container_cpu_utilization_percent": ("container-cpu", "Runtime average container CPU", "CPU utilization (%)"),
            "system_container_memory_usage_bytes": ("container-memory", "Runtime average container memory", "Memory usage (MiB)"),
        }
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
            charts, "runtime", chart_key, title, "Workload scenario", y_label,
            scenario, label, values.mean, 1, 1,
            note="Per-node mean of container-scoped system samples; host-wide values are excluded.",
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


def render_svg(chart):
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
        svg = render_svg(chart)
        if svg is None:
            continue
        module_dir = output_dir / chart.module
        module_dir.mkdir(parents=True, exist_ok=True)
        (module_dir / f"{safe_slug(chart.key)}.svg").write_text(svg + "\n", encoding="utf-8")
        rendered.append(chart)
    write_plot_data(output_dir, rendered)
    write_index(output_dir, rendered, report_counts, warnings)
    return {"charts": len(rendered), "reports": report_counts, "warnings": warnings}


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
    print(f"Open {output_dir / 'index.html'} to browse the comparisons.")
    for module, count in result["reports"].items():
        if args.module is None or module in args.module:
            print(f"  {module}: {count} successful report(s)")
    for warning in result["warnings"]:
        print(f"Warning: {warning}", file=sys.stderr)


if __name__ == "__main__":
    main()