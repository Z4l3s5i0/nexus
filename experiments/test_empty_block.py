import importlib.util
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from argparse import ArgumentTypeError
from unittest.mock import patch


EMPTY_BLOCK = pathlib.Path(__file__).resolve().parent / "empty-block"
sys.path.insert(0, str(EMPTY_BLOCK))

import block_collection
import collect_block_metrics
import parse_logs
import run_experiment
import transaction_submission


def load_experiment_module(name, filename):
    spec = importlib.util.spec_from_file_location(name, EMPTY_BLOCK / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


empty_block_capture_metadata = load_experiment_module("empty_block_capture_metadata", "capture_metadata.py")
empty_block_report = load_experiment_module("empty_block_report", "report.py")


class EmptyBlockExperimentTests(unittest.TestCase):
    def test_sweep_parsing_and_remote_rpc_guard(self):
        self.assertEqual(run_experiment.parse_counts("0, 1,5,10"), [0, 1, 5, 10])
        for value in ("", "1,-1", "1,one", "1,1"):
            with self.subTest(value=value), self.assertRaises(ArgumentTypeError):
                run_experiment.parse_counts(value)
        with self.assertRaisesRegex(ValueError, "remote RPC is disabled"):
            run_experiment.verify_rpc_url("http://example.test:8545", False)
        run_experiment.verify_rpc_url("http://example.test:8545", True)

    def test_empty_only_run_needs_no_key_or_cast(self):
        args = run_experiment.create_parser().parse_args(["--counts", "0"])
        with patch.object(run_experiment.shutil, "which", return_value=None):
            run_experiment.validate_options(args, "")

    def test_nonempty_sweep_requires_a_valid_key(self):
        args = run_experiment.create_parser().parse_args(["--counts", "0,1"])
        with self.assertRaisesRegex(ValueError, "private-key"):
            run_experiment.validate_options(args, "")
        with self.assertRaisesRegex(ValueError, "32-byte hexadecimal"):
            run_experiment.validate_options(args, "not-a-key")

    def test_block_transaction_count_uses_actual_rpc_payload(self):
        block = {"transactions": ["0x1", "0x2"]}
        self.assertEqual(block_collection.block_transaction_count(block), 2)
        with self.assertRaisesRegex(RuntimeError, "transaction list"):
            block_collection.block_transaction_count({"transactions": None})

    def test_rpc_call_returns_result_and_surfaces_rpc_errors(self):
        success = io.BytesIO(json.dumps({"jsonrpc": "2.0", "id": 1, "result": "0x7b"}).encode())
        with patch.object(block_collection, "urlopen", return_value=success):
            self.assertEqual(block_collection.rpc_call("http://localhost", "eth_chainId", [], 1), "0x7b")

        failure = io.BytesIO(
            json.dumps({"jsonrpc": "2.0", "id": 2, "error": {"code": -1, "message": "bad request"}}).encode()
        )
        with patch.object(block_collection, "urlopen", return_value=failure):
            with self.assertRaisesRegex(RuntimeError, "bad request"):
                block_collection.rpc_call("http://localhost", "eth_chainId", [], 1)

    def test_transaction_hash_and_host_cast_burst_submission(self):
        tx_hashes = ["0x" + "a" * 64, "0x" + "b" * 64]
        self.assertEqual(transaction_submission.transaction_hash(f"sent {tx_hashes[0]}"), tx_hashes[0])
        with patch.object(transaction_submission, "run_cast", side_effect=[f"sent {tx_hashes[0]}", f"sent {tx_hashes[1]}"]) as cast:
            submitted = transaction_submission.submit_burst(
                "cast", "network", "image", "http://localhost:8545", "0xsender", "0xkey", 10, 2, 30,
                "2500000000",
            )
        self.assertEqual(submitted, tx_hashes)
        self.assertEqual(cast.call_count, 2)
        cast_arguments = cast.call_args_list[0].args[1]
        self.assertIn("--legacy", cast_arguments)
        gas_price_index = cast_arguments.index("--gas-price")
        self.assertEqual(cast_arguments[gas_price_index + 1], "2500000000")

    def test_docker_cast_uses_one_container_for_a_burst(self):
        tx_hashes = ["0x" + "a" * 64, "0x" + "b" * 64]
        result = subprocess.CompletedProcess([], 0, stdout="\n".join(tx_hashes), stderr="")
        with patch.object(transaction_submission.shutil, "which", return_value="docker"):
            with patch.object(transaction_submission.subprocess, "run", return_value=result) as docker_run:
                submitted = transaction_submission.submit_burst(
                    None, "suite-net", "foundry:test", "http://el-node-0:8545",
                    "0xsender", "0x" + "1" * 64, 5, 2, 30, "2500000000",
                )
        self.assertEqual(submitted, tx_hashes)
        self.assertEqual(docker_run.call_count, 1)
        self.assertIn("--network", docker_run.call_args.args[0])
        self.assertIn("--legacy", docker_run.call_args.args[0][-1])
        self.assertIn("--gas-price 2500000000", docker_run.call_args.args[0][-1])

    def test_rpc_derived_block_metrics_keep_zero_observations_and_identity(self):
        metadata = {"chain_id": 100, "client": "wasix-eth", "node_id": "node-a"}
        rows = [
            {
                "block_number": "10", "actual_tx_count": "1", "block_timestamp": "1000",
                "target_tx_count": "2", "attempt": "1", "transaction_hashes": "0xaaa",
                "submitted_transaction_hashes": "0xaaa;0xbbb",
            },
            {
                "block_number": "11", "actual_tx_count": "0", "block_timestamp": "1012",
                "target_tx_count": "2", "attempt": "1", "transaction_hashes": "",
                "submitted_transaction_hashes": "0xaaa;0xbbb",
            },
        ]

        samples, coverage = collect_block_metrics.derive_block_samples(rows, metadata, "run-1")
        by_name = {sample["name"]: sample for sample in samples}
        ratio = next(sample for sample in samples if sample["name"] == "empty_block_commit_to_submit_ratio")

        self.assertEqual(by_name["empty_block_actual_transaction_count"]["value"], 0.0)
        self.assertEqual(by_name["empty_block_tps"]["value"], 0.0)
        self.assertEqual(ratio["value"], 0.5)
        self.assertEqual(ratio["node_id"], "node-a")
        self.assertEqual(ratio["client"], "wasix-eth")
        self.assertEqual(json.loads(ratio["labels_json"])["runtime_variant"], "unknown")
        self.assertEqual(ratio["source"], "Derived")
        self.assertEqual(coverage["fields"]["commit_to_submit_ratio"], "observed")

    def test_missing_block_samples_are_unavailable_not_zero(self):
        samples, coverage = collect_block_metrics.derive_block_samples([], {"chain_id": 100}, "run-1")

        self.assertEqual(samples, [])
        self.assertTrue(all(status == "unavailable" for status in coverage["fields"].values()))

    def test_structured_logs_capture_sender_gas_execution_and_peer_fields(self):
        tx_hash = "0x" + "a" * 64
        lines = [
            f"2026-01-01T00:00:00.000Z [EXP] TX_RPC_RECV hash={tx_hash}",
            f"2026-01-01T00:00:00.010Z [EXP] TX_EXEC_START hash={tx_hash} sender=0x1234 nonce=1",
            f"2026-01-01T00:00:00.020Z [EXP] TX_EXEC_END hash={tx_hash} success=true reverted=false gas_used=21000 cumulative_gas=21000 elapsed_ms=8",
            "2026-01-01T00:00:00.030Z [EXP] BLOCK_EXEC_END number=7 hash=0xabc elapsed_ms=12",
            "2026-01-01T00:00:00.040Z [EXP] IMPORT_END number=7 txs=1 exec_ms=8 root_ms=2 persist_ms=1",
            "2026-01-01T00:00:00.050Z [EXP] DOWNLOAD_BODIES_START peer=peer-7 count=2",
        ]
        events = [parse_logs.parse_event_line(line, "run-1", "node-a", "wasix-eth", "100", "wasix") for line in lines]

        samples, coverage = parse_logs.derive_log_samples(events, [tx_hash], [7])
        by_name = {sample["name"]: sample for sample in samples}
        gas_labels = json.loads(by_name["log_transaction_gas_used"]["labels_json"])

        self.assertEqual(by_name["log_transaction_gas_used"]["value"], 21000.0)
        self.assertEqual(gas_labels["sender"], "0x1234")
        self.assertEqual(gas_labels["runtime_variant"], "wasix")
        self.assertEqual(by_name["log_transaction_elapsed_ms"]["value"], 8.0)
        self.assertEqual(by_name["log_block_execution_time_ms"]["value"], 12.0)
        self.assertEqual(coverage["counts"]["transaction_count"], 1)
        self.assertEqual(coverage["counts"]["peer_id"], 1)
        self.assertEqual(coverage["counts"]["download_type"], 1)

    def test_execution_time_parser_preserves_legacy_millisecond_normalization(self):
        with tempfile.TemporaryDirectory() as temp:
            log_path = pathlib.Path(temp) / "node.log"
            log_path.write_text(
                "INFO [PayloadProcessor] Block 1 executed in 10.5ms\n"
                "INFO [PayloadProcessor] Block 2 executed in 1.25s\n",
                encoding="utf-8",
            )
            self.assertEqual(parse_logs.load_execution_times(log_path), {1: 10.5, 2: 1250.0})

    def test_metadata_capture_preserves_registered_client_and_runtime_variant(self):
        with tempfile.TemporaryDirectory() as temp:
            output_path = pathlib.Path(temp) / "metadata.json"
            args = type(
                "Args",
                (),
                {
                    "api_url": "http://127.0.0.1:9100",
                    "timeout": 5,
                    "rpc_port": 8545,
                    "chain_id": 100,
                    "experiment_id": "run-1",
                    "client_label": "wasix-eth",
                    "client_variant": "wasix",
                    "counts": "0,1",
                    "blocks_per_count": 2,
                    "max_attempts": 3,
                    "output": output_path,
                },
            )()
            registry = io.BytesIO(
                json.dumps(
                    [{"id": "node-a", "chain_id": 100, "client": "wasix-eth", "rpc_url": "http://el-node-0:8545"}]
                ).encode()
            )
            inspect_results = [
                "container-id",
                json.dumps([{"Config": {"Image": "local/wasix-eth"}, "Image": "sha256:test"}]),
            ]
            with patch.object(empty_block_capture_metadata, "urlopen", return_value=registry):
                with patch.object(
                    empty_block_capture_metadata.subprocess,
                    "check_output",
                    side_effect=inspect_results,
                ):
                    empty_block_capture_metadata.capture(args)
            metadata = json.loads(output_path.read_text(encoding="utf-8"))

        self.assertEqual(metadata["client"], "wasix-eth")
        self.assertEqual(metadata["client_variant"], "wasix")
        self.assertEqual(metadata["node_id"], "node-a")
        self.assertEqual(metadata["sweep"]["transaction_counts"], [0, 1])

    def test_report_lists_unmatched_samples_and_archived_metric_status(self):
        with tempfile.TemporaryDirectory() as temp:
            output_dir = pathlib.Path(temp)
            metadata = {
                "experiment_id": "run-1",
                "chain_id": 100,
                "client": "wasix-eth",
                "client_variant": "wasix",
                "node_id": "node-a",
                "registered": True,
                "nodes": [{"index": 0, "node_id": "node-a", "client": "wasix-eth", "variant": "wasix"}],
                "sweep": {"transaction_counts": [0, 1], "blocks_per_count": 2},
            }
            (output_dir / "metadata.json").write_text(json.dumps(metadata), encoding="utf-8")
            (output_dir / "run_status.json").write_text('{"status":2}', encoding="utf-8")
            (output_dir / "samples.csv").write_text(
                "target_tx_count,block_number,actual_tx_count,matched,execution_time_ms\n"
                "0,10,0,true,12.5\n",
                encoding="utf-8",
            )

            report = empty_block_report.build_report(output_dir)

        self.assertEqual(report["by_target_transaction_count"]["0"]["missing_matches"], 1)
        self.assertEqual(report["by_target_transaction_count"]["1"]["expected_matches"], 2)
        self.assertEqual(report["run_status"]["status"], 2)
        self.assertFalse(report["metrics_database_archived"])


if __name__ == "__main__":
    unittest.main()