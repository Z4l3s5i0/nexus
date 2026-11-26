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
            throughput_svg = (output / "runtime" / "throughput.svg").read_text(encoding="utf-8")
            data_path = output / "plot_data.csv"
            with data_path.open(newline="", encoding="utf-8") as source:
                data = list(csv.DictReader(source))
            self.assertEqual(result["charts"], 12)
            self.assertTrue((output / "empty-block" / "value-transfer-inclusion-latency.svg").is_file())
            self.assertTrue((output / "runtime" / "throughput.svg").is_file())
            self.assertTrue((output / "startup" / "startup-time.svg").is_file())
            self.assertTrue((output / "size" / "source-lines.svg").is_file())
            self.assertTrue((output / "compliance" / "hive-pass-rate.svg").is_file())
            self.assertIn("WASIX", throughput_svg)
            self.assertIn("Geth", throughput_svg)
            throughput = [row for row in data if row["metric"] == "Runtime confirmed workload throughput"]
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

    def test_missing_values_are_not_rendered_as_zero(self):
        chart = plot_results.Chart("runtime", "test", "Missing data", "Client", "TPS")
        chart.add("WASIX", "WASIX", 0)
        chart.add("Reth", "Reth", None)
        svg = plot_results.render_svg(chart)

        self.assertIn("WASIX: 0", svg)
        self.assertNotIn("Reth: 0", svg)


if __name__ == "__main__":
    unittest.main()