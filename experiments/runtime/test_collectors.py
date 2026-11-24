import json
import tempfile
import unittest
from pathlib import Path

from runtime.collect_logs import derive_samples, parse_event_line
from runtime.collect_node_metrics import summarize_node_metrics
from runtime.report import system_coverage
from runtime.collect_system_metrics import (
    container_metric_samples,
    host_metric_samples,
    parse_bytes,
    parse_docker_stats,
    read_host_counters,
)


class SystemMetricTests(unittest.TestCase):
    def test_docker_stats_parse_units_and_preserve_missing_values(self):
        row = {
            "Name": "project-el-node-1-1",
            "CPUPerc": "--",
            "MemUsage": "2MiB / 4GiB",
            "BlockIO": "1kB / 2kB",
            "NetIO": "3MiB / 4MiB",
        }

        parsed = parse_docker_stats(row)

        self.assertEqual(parse_bytes("2MiB"), 2 * 1024**2)
        self.assertEqual(parsed["node_index"], 1)
        self.assertIsNone(parsed["cpu_percent"])
        self.assertEqual(parsed["memory_bytes"], 2 * 1024**2)
        self.assertEqual(parsed["disk_read_bytes"], 1000)
        self.assertEqual(parsed["network_tx_bytes"], 4 * 1024**2)

    def test_host_rates_are_derived_only_when_two_samples_exist(self):
        previous = {
            "cpu_total": 200,
            "cpu_idle": 80,
            "context_switches": 10,
            "disk_read_bytes": 100,
            "disk_write_bytes": 200,
            "network_rx_bytes": 300,
            "network_tx_bytes": 400,
        }
        current = {
            **previous,
            "cpu_total": 400,
            "cpu_idle": 180,
            "context_switches": 30,
            "disk_read_bytes": 500,
            "disk_write_bytes": 600,
            "network_rx_bytes": 900,
            "network_tx_bytes": 800,
            "load_1m": 0.1,
            "load_5m": 0.2,
            "load_15m": 0.3,
            "memory_used_bytes": 1024,
        }

        first = host_metric_samples(current, None, 0, 1000, "run-1", "123")
        sampled = host_metric_samples(current, previous, 2, 2000, "run-1", "123")
        first_names = {sample["name"] for sample in first}
        by_name = {sample["name"]: sample for sample in sampled}

        self.assertNotIn("system_cpu_utilization_percent", first_names)
        self.assertEqual(by_name["system_cpu_utilization_percent"]["value"], 50.0)
        self.assertEqual(by_name["system_disk_read_bytes_per_second"]["value"], 200.0)
        self.assertEqual(by_name["system_network_transmit_bytes_per_second"]["value"], 200.0)
        self.assertEqual(json.loads(by_name["system_load_1m"]["labels_json"])["scope"], "host")
        self.assertTrue(all(sample["node_id"] == "host" and sample["client"] == "host" for sample in sampled))

    def test_container_samples_keep_client_and_node_identity(self):
        rows = [
            {
                "Name": "compose-el-node-0-1",
                "CPUPerc": "2.50%",
                "MemUsage": "1MiB / 4GiB",
                "BlockIO": "0B / 0B",
                "NetIO": "0B / 10kB",
            }
        ]
        nodes = {0: {"node_id": "node-a", "client": "wasix-eth"}}

        samples = container_metric_samples(rows, nodes, 1000, "run-1", "123")

        self.assertEqual(len(samples), 6)
        self.assertTrue(all(sample["node_id"] == "node-a" for sample in samples))
        self.assertTrue(all(sample["client"] == "wasix-eth" for sample in samples))
        self.assertTrue(all(sample["source"] == "System" for sample in samples))

    def test_container_network_and_disk_throughput_use_counter_deltas(self):
        previous = {}
        nodes = {0: {"node_id": "node-a", "client": "wasix-eth"}}
        before = {
            "Name": "compose-el-node-0-1",
            "CPUPerc": "1.00%",
            "MemUsage": "1MiB / 4GiB",
            "BlockIO": "100B / 200B",
            "NetIO": "300B / 400B",
        }
        after = {
            **before,
            "BlockIO": "500B / 1000B",
            "NetIO": "900B / 1200B",
        }

        container_metric_samples([before], nodes, 1000, "run-1", "123", previous, 1.0)
        sampled = container_metric_samples([after], nodes, 3000, "run-1", "123", previous, 3.0)
        by_name = {sample["name"]: sample for sample in sampled}

        self.assertEqual(by_name["system_container_disk_read_bytes_per_second"]["value"], 200.0)
        self.assertEqual(by_name["system_container_network_receive_bytes_per_second"]["value"], 300.0)

    def test_proc_reader_aggregates_physical_disk_and_excludes_loopback(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / "net").mkdir()
            (root / "stat").write_text(
                "cpu 100 0 20 80 0 0 0 0 0 0\nctxt 50\n", encoding="utf-8"
            )
            (root / "meminfo").write_text(
                "MemTotal: 1000 kB\nMemAvailable: 400 kB\n", encoding="utf-8"
            )
            (root / "diskstats").write_text(
                "8 0 sda 1 0 100 0 2 0 200 0 0 0 0 0\n"
                "8 1 sda1 3 0 900 0 4 0 800 0 0 0 0 0\n",
                encoding="utf-8",
            )
            (root / "net" / "dev").write_text(
                "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n"
                "lo: 100 0 0 0 0 0 0 0 200 0 0 0 0 0 0 0\n"
                "eth0: 1000 0 0 0 0 0 0 0 2000 0 0 0 0 0 0 0\n",
                encoding="utf-8",
            )
            (root / "loadavg").write_text("0.10 0.20 0.30 1/2 3\n", encoding="utf-8")

            counters = read_host_counters(root)

        self.assertEqual(counters["memory_used_bytes"], 600 * 1024)
        self.assertEqual(counters["disk_read_bytes"], 100 * 512)
        self.assertEqual(counters["disk_write_bytes"], 200 * 512)
        self.assertEqual(counters["network_rx_bytes"], 1000)
        self.assertEqual(counters["network_tx_bytes"], 2000)


class LogMetricTests(unittest.TestCase):
    def test_actual_execution_events_produce_tx_gas_elapsed_and_peer_metrics(self):
        tx_hash = "0x" + "a" * 64
        client = "wasix-eth"
        lines = [
            f"2026-01-01T00:00:00.000Z [EXP] TX_RPC_RECV hash={tx_hash}",
            f"2026-01-01T00:00:00.010Z [EXP] TX_EXEC_START hash={tx_hash} sender=0x1234 nonce=1",
            f"2026-01-01T00:00:00.020Z [EXP] TX_EXEC_END hash={tx_hash} success=true reverted=false gas_used=21000 cumulative_gas=21000 elapsed_ms=8",
            "2026-01-01T00:00:00.030Z [EXP] BLOCK_EXEC_START number=7 hash=0xabc parent=0xdef tx_count=1",
            "2026-01-01T00:00:00.040Z [EXP] BLOCK_EXEC_END number=7 hash=0xabc elapsed_ms=12",
            "2026-01-01T00:00:00.050Z [EXP] IMPORT_END number=7 txs=1 exec_ms=8 root_ms=2 persist_ms=1",
            "2026-01-01T00:00:00.060Z [EXP] DOWNLOAD_HEADERS_START peer=peer-7 start=7 amount=10",
        ]
        events = [
            parse_event_line(line, "run-1", "node-a", client, "123")
            for line in lines
        ]

        samples, coverage = derive_samples(events, workload_duration=2)
        by_name = {sample["name"]: sample for sample in samples}

        self.assertEqual(by_name["log_transaction_gas_used"]["value"], 21000.0)
        self.assertEqual(by_name["log_transaction_elapsed_ms"]["value"], 8.0)
        self.assertEqual(by_name["log_submit_to_execution_ms"]["value"], 10.0)
        self.assertEqual(by_name["log_block_transaction_count"]["value"], 1.0)
        self.assertEqual(by_name["log_tps"]["value"], 0.5)
        self.assertEqual(by_name["log_commit_to_submit_ratio"]["value"], 1.0)
        self.assertEqual(coverage["peer_id"], 1)
        self.assertEqual(coverage["download_type"], 1)
        self.assertEqual(json.loads(by_name["log_transaction_gas_used"]["labels_json"])["sender"], "0x1234")

    def test_missing_structured_events_produce_no_zero_samples(self):
        samples, coverage = derive_samples([], workload_duration=1)

        self.assertEqual(samples, [])
        self.assertEqual(coverage["gas_used"], 0)
        self.assertEqual(coverage["transaction_hash"], 0)


class NodeMetricCoverageTests(unittest.TestCase):
    def test_coverage_reports_unavailable_rpc_and_raw_prometheus_names(self):
        metadata = {
            "nodes": [
                {"index": 0, "node_id": "node-a", "client": "geth", "registered": True},
                {"index": 1, "node_id": None, "client": "wasix-eth", "registered": False},
            ]
        }
        samples = [
            {
                "experiment_id": "run-1",
                "node_id": "node-a",
                "client": "geth",
                "source": "Derived",
                "name": "ethereum_latest_block",
            },
            {
                "experiment_id": "run-1",
                "node_id": "node-a",
                "client": "geth",
                "source": "PrometheusScrape",
                "name": "geth_chain_head_block",
            },
        ]

        summary = summarize_node_metrics(metadata, samples, "run-1")

        self.assertEqual(summary["nodes"][0]["rpc_derived"]["latest_block"]["status"], "observed")
        self.assertEqual(summary["nodes"][0]["rpc_derived"]["gas_price"]["status"], "unavailable")
        self.assertEqual(summary["nodes"][0]["prometheus"]["raw_series_names"], ["geth_chain_head_block"])
        self.assertEqual(summary["nodes"][1]["prometheus"]["status"], "unavailable")

    def test_system_coverage_keeps_host_and_container_scope_separate(self):
        metadata = {"nodes": [{"index": 0, "node_id": "node-a", "client": "wasix-eth"}]}
        samples = [
            {"node_id": "node-a", "name": "system_container_cpu_utilization_percent"},
            {"node_id": "host", "name": "system_load_1m"},
        ]

        coverage = system_coverage(metadata, samples)

        self.assertEqual(coverage["nodes"][0]["fields"]["cpu"], "observed")
        self.assertEqual(coverage["nodes"][0]["fields"]["memory"], "unavailable")
        self.assertEqual(coverage["host"]["fields"]["load_averages"], "partial")
        self.assertEqual(coverage["host"]["fields"]["disk_io"], "unavailable")


if __name__ == "__main__":
    unittest.main()