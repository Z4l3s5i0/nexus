import csv
import importlib.util
import json
import pathlib
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).resolve().parent / "plot_results.py"
SPEC = importlib.util.spec_from_file_location("plot_results", SCRIPT)
plot_results = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(plot_results)


def write_run(root, module, run_id, report, metadata=None):
    run_dir = root / module / run_id
    run_dir.mkdir(parents=True)
    (run_dir / "report.json").write_text(json.dumps(report), encoding="utf-8")
    (run_dir / "metadata.json").write_text(json.dumps(metadata or {}), encoding="utf-8")
    return run_dir


class PlotResultsTests(unittest.TestCase):
    def test_comparison_plots_cover_all_experiment_modules(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp) / "results"
            output = pathlib.Path(temp) / "plots"

            write_run(root, "empty-block", "wasix", {
                "client": "wasix-eth",
                "workload": "none",
                "by_target_transaction_count": {
                    "1": {
                        "expected_matches": 2,
                        "exact_block_matches": 2,
                        "expected_bursts": 2,
                        "completed_bursts": 2,
                        "burst_completion_latency_ms": {"mean": 1200, "observations": 2},
                    },
                },
                "transaction_inclusion_latency": {"value_transfer": {"workload": "none"}},
                "transaction_inclusion_latency_by_target": {
                    "value_transfer": {"1": {"single_block": {"mean_ms": 600, "observations": 2}}},
                },
            }, {"client": "wasix-eth", "client_variant": "wasix", "workload": "none"})

            runtime_dir = write_run(root, "runtime", "mixed", {
                "workload_outcomes": {
                    "tps": {"per_node": [
                        {"client": "wasix-eth", "node_id": "n", "mean": 12, "observations": 3},
                        {"client": "geth", "node_id": "g", "mean": 10, "observations": 3},
                    ]},
                    "commit_to_submit_ratio": {"per_node": [
                        {"client": "wasix-eth", "node_id": "n", "mean": 0.9, "observations": 3},
                    ]},
                },
            }, {
                "workload": {"scenario": "exp/uber"},
                "nodes": [
                    {"node_id": "n", "client": "wasix-eth", "type": "wasix"},
                    {"node_id": "g", "client": "geth", "type": "geth"},
                ],
            })
            (runtime_dir / "system_metric_samples.jsonl").write_text(
                json.dumps({"name": "system_container_cpu_utilization_percent", "node_id": "n", "client": "wasix-eth", "value": 20, "labels_json": '{"scope":"container","runtime_variant":"wasix"}'}) + "\n",
                encoding="utf-8",
            )

            write_run(root, "startup", "start", {
                "configurations": {
                    "geth": {"iterations_ready": 2, "iterations_requested": 2, "startup_time_ms": {"mean": 500, "median": 500}},
                    "nexus-linux": {"iterations_ready": 2, "iterations_requested": 2, "startup_time_ms": {"mean": 600, "median": 600}},
                    "nexus-wasix": {"iterations_ready": 1, "iterations_requested": 2, "startup_time_ms": {"mean": 800, "median": 800}},
                },
            })
            write_run(root, "size", "size", {
                "source_code": [{"label": "wasix-eth", "value": "100", "status": "measured"}],
                "compiled_binaries": [
                    {"label": "wasix-eth-native", "value": "2000", "status": "measured"},
                    {"label": "wasix-eth-wasix", "value": "2500", "status": "measured"},
                ],
            })
            write_run(root, "compliance", "hive", {
                "results": [{"client": "wasix-r-eth", "exit_code": 0}],
            })

            result = plot_results.generate_plots(root, output)
            index = (output / "index.html").read_text(encoding="utf-8")
            throughput_svgs = list((output / "runtime").glob("throughput-*.svg"))
            self.assertEqual(len(throughput_svgs), 1)
            throughput_svg = throughput_svgs[0].read_text(encoding="utf-8")
            data_path = output / "plot_data.csv"
            with data_path.open(newline="", encoding="utf-8") as source:
                data = list(csv.DictReader(source))
            self.assertEqual(result["charts"], 12)
            self.assertTrue((output / "empty-block" / "value-transfer-inclusion-latency.svg").is_file())
            self.assertTrue((output / "empty-block" / "value-transfer-exact-match-rate.svg").is_file())
            self.assertTrue(throughput_svgs[0].is_file())
            self.assertTrue((output / "startup" / "startup-time.svg").is_file())
            self.assertTrue((output / "size" / "source-lines.svg").is_file())
            self.assertTrue((output / "compliance" / "hive-pass-rate.svg").is_file())
            self.assertIn("Key insights from included runs", index)
            self.assertIn("Relative change (B vs A)", index)
            self.assertIn("Completed-burst rates range from 100%", index)
            self.assertIn("WASIX", throughput_svg)
            self.assertIn("Geth", throughput_svg)
            throughput = [row for row in data if row["metric"] == "Runtime confirmed workload throughput by cluster size"]
            self.assertEqual({row["series"] for row in throughput}, {"WASIX", "Geth"})
            self.assertTrue(all(float(row["value"]) > 0 for row in throughput))
            startup_clients = {
                row["category"] for row in data
                if row["metric"] == "Startup readiness time by client"
            }
            self.assertEqual(startup_clients, {"Geth", "NEXUS native", "WASIX"})
            binary_clients = {
                row["category"] for row in data
                if row["metric"] == "Compiled binary size by client"
            }
            self.assertEqual(binary_clients, {"NEXUS native", "WASIX"})

    def test_knee_curve_and_success_heatmap_render_without_optional_dependencies(self):
        curve = plot_results.Chart("empty-block", "latency", "Latency knee curve", "Target", "Latency", kind="knee")
        for target, latency in ((1, 1), (5, 2), (10, 10), (20, 11), (50, 12)):
            curve.add(target, "WASIX", latency)
        self.assertEqual(plot_results.estimate_knee(curve, "WASIX"), "10")
        curve_svg = plot_results.render_svg(curve)
        self.assertIn("log10(target + 1)", curve_svg)
        self.assertIn("Exploratory knee candidate", curve_svg)

        heatmap = plot_results.Chart("empty-block", "success", "Success rate", "Target", "Rate", percent=True, kind="heatmap")
        heatmap.add(1, "WASIX", 1.0)
        heatmap.add(5, "WASIX", 0.5)
        heatmap.add(1, "Geth", 0.0)
        heatmap_svg = plot_results.render_svg(heatmap)
        self.assertIn("100%", heatmap_svg)
        self.assertIn("50%", heatmap_svg)
        self.assertIn("Geth", heatmap_svg)

    def test_completed_burst_block_spread_heatmap_shows_mean_span(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            run_dir = write_run(root, "empty-block", "geth-counter", {
                "workload": "counter",
            }, {"client": "geth", "workload": "counter"})
            (run_dir / "transaction_burst_samples.csv").write_text(
                "target_tx_count,blocks_spanned,completed\n"
                "10,1,true\n"
                "10,5,true\n"
                "10,5,true\n"
                "10,8,false\n",
                encoding="utf-8",
            )
            charts = {}
            plot_results.collect_empty_block(charts, run_dir, {"workload": "counter"}, {"client": "geth", "workload": "counter"})

        chart = charts[("empty-block", "counter-blocks-spanned")]
        accumulator = chart.points[("10", "Geth")]
        self.assertAlmostEqual(accumulator.mean, 11 / 3)
        self.assertEqual(accumulator.observations, 3)
        self.assertTrue(chart.lower_is_better)
        self.assertEqual(plot_results._heatmap_color(1, 1, 5, False, True), "#66bd63")
        self.assertEqual(plot_results._heatmap_color(5, 1, 5, False, True), "#ef7d6f")
        svg = plot_results.render_heatmap_svg(chart)
        self.assertIn("3.67", svg)
        self.assertIn("5+", svg)
        self.assertIn("completed bursts", svg)

    def test_missing_values_are_not_rendered_as_zero(self):
        chart = plot_results.Chart("runtime", "test", "Missing data", "Client", "TPS")
        chart.add("WASIX", "WASIX", 0)
        chart.add("Reth", "Reth", None)
        svg = plot_results.render_svg(chart)

        self.assertIn("WASIX: 0", svg)
        self.assertNotIn("Reth: 0", svg)

    def test_runtime_charts_keep_client_mix_and_cluster_size_comparable(self):
        charts = {}
        runs = (
            ({"native": 2, "wasix": 2}, [
                {"node_id": "native-1", "client": "wasix-eth", "type": "native"},
                {"node_id": "wasix-1", "client": "wasix-eth", "type": "wasix"},
            ], [
                {"node_id": "native-1", "mean": 9},
                {"node_id": "wasix-1", "mean": 11},
            ]),
            ({"native": 4, "wasix": 4}, [
                {"node_id": "native-2", "client": "wasix-eth", "type": "native"},
                {"node_id": "wasix-2", "client": "wasix-eth", "type": "wasix"},
            ], [
                {"node_id": "native-2", "mean": 10},
                {"node_id": "wasix-2", "mean": 12},
            ]),
            ({"geth": 2, "wasix": 2}, [
                {"node_id": "geth-1", "client": "geth", "type": "geth"},
                {"node_id": "wasix-3", "client": "wasix-eth", "type": "wasix"},
            ], [
                {"node_id": "geth-1", "mean": 8},
                {"node_id": "wasix-3", "mean": 13},
            ]),
            ({"native": 3, "wasix": 1}, [
                {"node_id": "native-3", "client": "wasix-eth", "type": "native"},
                {"node_id": "wasix-4", "client": "wasix-eth", "type": "wasix"},
            ], [
                {"node_id": "native-3", "mean": 10},
                {"node_id": "wasix-4", "mean": 12},
            ]),
            ({"native": 4, "wasix": 0}, [
                {"node_id": "native-only", "client": "wasix-eth", "type": "native"},
            ], [
                {"node_id": "native-only", "mean": 7},
            ]),
            ({"native": 0, "wasix": 4}, [
                {"node_id": "wasix-only", "client": "wasix-eth", "type": "wasix"},
            ], [
                {"node_id": "wasix-only", "mean": 8},
            ]),
            ({"geth": 4}, [
                {"node_id": "geth-only", "client": "geth", "type": "geth"},
            ], [
                {"node_id": "geth-only", "mean": 6},
            ]),
            ({"reth": 4}, [
                {"node_id": "reth-only", "client": "reth", "type": "reth"},
            ], [
                {"node_id": "reth-only", "mean": 5},
            ]),
        )
        for counts, nodes, per_node in runs:
            plot_results.collect_runtime(
                charts,
                pathlib.Path("."),
                {"workload_outcomes": {"tps": {"per_node": per_node}}},
                {
                    "counts": counts,
                    "nodes": nodes,
                    "workload": {"scenario": "exp/uber", "tps": 4, "duration_seconds": 120},
                },
            )

        throughput_charts = [
            chart for (module, key), chart in charts.items()
            if module == "runtime" and key.startswith("throughput-")
        ]
        self.assertEqual(len(throughput_charts), 4)
        native_wasix = next(chart for chart in throughput_charts if "1 WASIX + 1 NEXUS native" in chart.group)
        self.assertEqual(native_wasix.categories(), ["4", "8"])
        self.assertEqual(set(native_wasix.series()), {"NEXUS native", "WASIX"})
        self.assertEqual(set(native_wasix.points), {
            ("4", "NEXUS native"), ("4", "WASIX"), ("8", "NEXUS native"), ("8", "WASIX"),
        })
        self.assertIn("4 TPS/node", native_wasix.group)
        self.assertIn("<polyline", plot_results.render_svg(native_wasix))

        single_client = next(chart for chart in throughput_charts if "single-client runs" in chart.group)
        self.assertEqual(single_client.categories(), ["4"])
        self.assertEqual(set(single_client.series()), {"NEXUS native", "WASIX", "Geth", "Reth"})
        self.assertEqual(single_client.points[("4", "NEXUS native")].mean, 7)
        self.assertEqual(single_client.points[("4", "WASIX")].mean, 8)
        self.assertEqual(single_client.points[("4", "Geth")].mean, 6)
        self.assertEqual(single_client.points[("4", "Reth")].mean, 5)

    def test_runtime_throughput_uses_configured_workload_duration(self):
        with tempfile.TemporaryDirectory() as temp:
            run_dir = write_run(pathlib.Path(temp), "runtime", "native-counter", {
                "workload_outcomes": {
                    "tps": {"per_node": [{"node_id": "node-1", "mean": 7.6, "observations": 1}]},
                },
            }, {
                "counts": {"native": 4},
                "nodes": [{"node_id": "node-1", "client": "wasix-eth", "type": "native"}],
                "workload": {
                    "scenario": "exp/counter", "tps": 17,
                    "duration_seconds": 180, "wait_seconds": 900,
                },
            })
            (run_dir / "log_metric_samples.jsonl").write_text(
                json.dumps({
                    "name": "log_tps", "node_id": "node-1",
                    "labels_json": json.dumps({
                        "committed_transactions": 3077,
                        "workload_window_seconds": 403.39,
                    }),
                }) + "\n",
                encoding="utf-8",
            )
            charts = {}
            plot_results.collect_runtime(charts, run_dir, {
                "workload_outcomes": {
                    "tps": {"per_node": [{"node_id": "node-1", "mean": 7.6, "observations": 1}]},
                },
            }, {
                "counts": {"native": 4},
                "nodes": [{"node_id": "node-1", "client": "wasix-eth", "type": "native"}],
                "workload": {
                    "scenario": "exp/counter", "tps": 17,
                    "duration_seconds": 180, "wait_seconds": 900,
                },
            })

        throughput = next(chart for chart in charts.values() if chart.key.startswith("throughput-"))
        self.assertAlmostEqual(throughput.points[("4", "NEXUS native")].mean, 3077 / 180)
        self.assertEqual(throughput.workload_duration_seconds, 180)
        rows = plot_results.runtime_comparison_rows([throughput])
        self.assertEqual(rows, [])

    def test_runtime_comparison_table_includes_every_client_pair_and_workload_target(self):
        chart = plot_results.Chart(
            "runtime", "throughput-counter", "Runtime throughput", "Nodes", "TPS/node",
            group="exp/counter · single-client runs", kind="line",
        )
        chart.workload_duration_seconds = 180
        chart.workload_tps = 17
        chart.workload_counts_available = True
        for client, value in (("WASIX", 16), ("NEXUS native", 15), ("Geth", 14), ("Reth", 13)):
            chart.add("4", client, value)

        rows = plot_results.runtime_comparison_rows([chart])

        pairs = {(row["client_a"], row["client_b"]) for row in rows}
        self.assertEqual(len(rows), 6)
        self.assertIn(("Geth", "Reth"), pairs)
        self.assertTrue(all(row["target_transactions_per_node"] == 3060 for row in rows))
        self.assertTrue(all(row["target_transactions_cluster"] == 12240 for row in rows))
        self.assertTrue(all(row["transactions_per_node_a"] != "unavailable" for row in rows))
        self.assertTrue(all(row["transactions_cluster_a"] != "unavailable" for row in rows))


if __name__ == "__main__":
    unittest.main()