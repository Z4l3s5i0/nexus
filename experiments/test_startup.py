import importlib.util
import io
import json
import pathlib
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch


STARTUP = pathlib.Path(__file__).resolve().parent / "startup"
sys.path.insert(0, str(STARTUP))

import benchmark_client
import collect_system_metrics
import collect_startup_metrics
import generate_genesis
import metadata


def load_report_module():
    spec = importlib.util.spec_from_file_location("startup_report", STARTUP / "report.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


startup_report = load_report_module()


class StartupBenchmarkTests(unittest.TestCase):
    def test_genesis_and_client_metadata_are_explicit(self):
        value = generate_genesis.genesis(12345)
        run = metadata.build_metadata("run-1", 12345, 2, ["geth", "nexus-wasmer"])

        self.assertEqual(value["config"]["chainId"], 12345)
        self.assertIn("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266", value["alloc"])
        wasmer_node = next(node for node in run["nodes"] if node["type"] == "nexus-wasmer")
        self.assertEqual(wasmer_node["client"], "wasix-eth")
        self.assertEqual(wasmer_node["variant"], "nexus-wasmer")
        self.assertEqual(wasmer_node["node_id"], "startup-nexus-wasmer-1")

    def test_readiness_probe_accepts_rpc_result_not_http_response_alone(self):
        response = io.BytesIO(json.dumps({"jsonrpc": "2.0", "id": 1, "result": "0x3039"}).encode())
        with patch.object(benchmark_client, "urlopen", return_value=response):
            self.assertTrue(benchmark_client.rpc_ready(8545))
        response = io.BytesIO(json.dumps({"jsonrpc": "2.0", "id": 1, "error": {"message": "not ready"}}).encode())
        with patch.object(benchmark_client, "urlopen", return_value=response):
            self.assertFalse(benchmark_client.rpc_ready(8545))

    def test_startup_timer_begins_after_client_preparation(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            genesis_path = root / "genesis.json"
            genesis_path.write_text("{}", encoding="utf-8")
            events = []
            args = SimpleNamespace(
                config="nexus-linux",
                iteration=1,
                run_id="run-1",
                chain_id=12345,
                work_dir=str(root / "work"),
                genesis=str(genesis_path),
                state_file=str(root / "state.json"),
                result_file=str(root / "result.json"),
                log_file=str(root / "client.log"),
                rpc_port=8545,
                timeout=1,
                poll_interval=0.01,
                geth_image="geth",
                reth_image="reth",
                nexus_linux_image="nexus-linux",
                nexus_wasix_image="nexus-wasix",
                nexus_wasix_wasm="",
                wasmer_bin="wasmer",
                prepare_only=False,
                skip_preparation=False,
            )

            def prepare(*_args):
                events.append("prepare")

            def launch(*_args):
                events.append("launch")
                return {"container_name": "startup-run-1", "pid": 10, "image": "nexus-linux", "image_id": "sha256:test"}

            def clock():
                events.append("clock")
                return 100 if events.count("clock") == 1 else 200

            with patch.object(benchmark_client, "prepare_config", side_effect=prepare):
                with patch.object(benchmark_client, "launch_config", side_effect=launch):
                    with patch.object(benchmark_client, "rpc_ready", return_value=True):
                        with patch.object(benchmark_client.time, "perf_counter_ns", side_effect=clock):
                            status = benchmark_client.run(args)
            result = json.loads((root / "result.json").read_text(encoding="utf-8"))

        self.assertEqual(status, 0)
        self.assertEqual(events[:3], ["prepare", "clock", "launch"])
        self.assertTrue(result["ready"])
        self.assertEqual(result["startup_time_ms"], 0.0001)

    def test_proc_counters_and_process_rates_keep_attribution(self):
        with tempfile.TemporaryDirectory() as temp:
            process_dir = pathlib.Path(temp) / "123"
            process_dir.mkdir()
            fields = ["S"] + ["0"] * 21
            fields[11] = "100"
            fields[12] = "50"
            fields[21] = "4"
            (process_dir / "stat").write_text(f"123 (client name) {' '.join(fields)}\n", encoding="utf-8")
            (process_dir / "io").write_text("read_bytes: 1000\nwrite_bytes: 2000\n", encoding="utf-8")
            counters = collect_system_metrics.process_counters(123, pathlib.Path(temp))

        identity = {
            "experiment_id": "run-1", "node_id": "startup-nexus-wasmer-1",
            "network": "12345", "client": "wasix-eth", "config": "nexus-wasmer", "iteration": 1,
        }
        previous = {"cpu_ticks": 100, "disk_read_bytes": 1000, "disk_write_bytes": 2000}
        samples = collect_system_metrics.process_samples(counters, previous, 1, 123, identity, 1000)
        by_name = {sample["name"]: sample for sample in samples}

        self.assertEqual(counters["cpu_ticks"], 150)
        self.assertEqual(counters["disk_read_bytes"], 1000)
        self.assertEqual(by_name["system_process_disk_read_bytes_per_second"]["value"], 0.0)
        self.assertEqual(by_name["system_process_cpu_utilization_percent"]["node_id"], "startup-nexus-wasmer-1")
        self.assertEqual(json.loads(by_name["system_process_cpu_utilization_percent"]["labels_json"])["startup_config"], "nexus-wasmer")

    def test_container_stats_produce_per_client_network_rate(self):
        identity = {
            "experiment_id": "run-1", "node_id": "startup-geth-1", "network": "12345",
            "client": "geth", "config": "geth", "iteration": 1,
        }
        row = {
            "Name": "startup-run-geth-1", "CPUPerc": "1.50%", "MemUsage": "2MiB / 4GiB",
            "BlockIO": "100B / 200B", "NetIO": "300B / 400B",
        }
        previous = {"values": {"network_rx_bytes": 100}, "time": 1.0}

        samples, next_previous = collect_system_metrics.container_samples(row, previous, 3.0, identity, 2000)
        by_name = {sample["name"]: sample for sample in samples}

        self.assertEqual(by_name["system_container_network_receive_bytes_per_second"]["value"], 100.0)
        self.assertEqual(by_name["system_container_cpu_utilization_percent"]["client"], "geth")
        self.assertEqual(next_previous["time"], 3.0)

    def test_startup_time_samples_are_derived_and_unready_runs_are_unavailable(self):
        result = {
            "experiment_id": "run-1", "node_id": "startup-geth-1", "chain_id": 12345,
            "client": "geth", "config": "geth", "client_variant": "geth", "iteration": 1,
            "ready": True, "startup_time_ms": 125.5, "ready_at": "2026-01-01T00:00:00Z",
            "image": "geth:test", "image_id": "sha256:test",
        }

        sample = collect_startup_metrics.build_sample(result)
        unavailable = collect_startup_metrics.build_sample({**result, "ready": False, "startup_time_ms": None})

        self.assertEqual(sample["name"], "startup_time_ms")
        self.assertEqual(sample["source"], "Derived")
        self.assertEqual(sample["node_id"], "startup-geth-1")
        self.assertEqual(sample["value"], 125.5)
        self.assertIsNone(unavailable)

    def test_startup_report_summarizes_repetitions_and_missing_metrics(self):
        with tempfile.TemporaryDirectory() as temp:
            output_dir = pathlib.Path(temp)
            (output_dir / "iterations" / "geth" / "1").mkdir(parents=True)
            (output_dir / "iterations" / "geth" / "2").mkdir(parents=True)
            metadata_value = {
                "experiment_id": "run-1", "chain_id": 12345, "iterations": 2,
                "nodes": [{"index": 0, "node_id": "startup-geth-1", "client": "geth", "variant": "geth"}],
            }
            (output_dir / "metadata.json").write_text(json.dumps(metadata_value), encoding="utf-8")
            first = {"config": "geth", "iteration": 1, "client": "geth", "node_id": "startup-geth-1", "ready": True, "startup_time_ms": 100, "image": "geth", "image_id": "sha256:a"}
            second = {**first, "iteration": 2, "node_id": "startup-geth-2", "startup_time_ms": 200, "image_id": "sha256:b"}
            (output_dir / "iterations" / "geth" / "1" / "result.json").write_text(json.dumps(first), encoding="utf-8")
            (output_dir / "iterations" / "geth" / "2" / "result.json").write_text(json.dumps(second), encoding="utf-8")
            (output_dir / "system_metric_samples.jsonl").write_text(
                json.dumps({"node_id": "startup-geth-1", "name": "system_container_cpu_utilization_percent"}) + "\n",
                encoding="utf-8",
            )

            report = startup_report.build_report(output_dir)

        self.assertEqual(report["configurations"]["geth"]["startup_time_ms"]["mean"], 150.0)
        self.assertEqual(report["configurations"]["geth"]["iterations_unavailable"], 0)
        self.assertEqual(report["system_metric_coverage"]["nodes"][0]["fields"]["cpu"], "observed")
        self.assertFalse(report["metrics_database_archived"])


if __name__ == "__main__":
    unittest.main()