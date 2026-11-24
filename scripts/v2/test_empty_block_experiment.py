import io
import json
import sys
import tempfile
import unittest
from argparse import ArgumentTypeError
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run_empty_block_experiment as experiment


class EmptyBlockExperimentTests(unittest.TestCase):
    def test_default_sweep_values(self):
        self.assertEqual(experiment.parse_counts("0, 1,5,10,20,50,100"), [0, 1, 5, 10, 20, 50, 100])

    def test_invalid_sweep_values_are_rejected(self):
        for value in ("", "1,-1", "1,one", "1,1"):
            with self.subTest(value=value), self.assertRaises(ArgumentTypeError):
                experiment.parse_counts(value)

    def test_remote_rpc_requires_explicit_opt_in(self):
        with self.assertRaisesRegex(ValueError, "remote RPC is disabled"):
            experiment.verify_rpc_url("http://example.test:8545", False)
        experiment.verify_rpc_url("http://example.test:8545", True)

    def test_empty_only_run_does_not_require_a_signing_key_or_cast(self):
        args = experiment.create_parser().parse_args(["--counts", "0"])
        with patch.object(experiment.shutil, "which", return_value=None):
            experiment.validate_options(args, "")

    def test_nonempty_sweep_requires_a_signing_key(self):
        args = experiment.create_parser().parse_args(["--counts", "0,1"])
        with self.assertRaisesRegex(ValueError, "private-key"):
            experiment.validate_options(args, "")

    def test_block_transaction_count_uses_actual_block_payload(self):
        self.assertEqual(experiment.block_transaction_count({"transactions": ["0x1", "0x2"]}), 2)
        with self.assertRaises(RuntimeError):
            experiment.block_transaction_count({"transactions": None})

    def test_execution_log_times_are_normalized_to_milliseconds(self):
        with tempfile.TemporaryDirectory() as directory:
            log_path = Path(directory) / "execution.log"
            log_path.write_text(
                "INFO [PayloadProcessor] Block 1 executed in 10.5ms\n"
                "INFO [PayloadProcessor] Block 2 executed in 1.25s\n",
                encoding="utf-8",
            )
            self.assertEqual(experiment.load_execution_times(log_path), {1: 10.5, 2: 1250.0})

    def test_rpc_call_returns_result_and_surfaces_rpc_errors(self):
        success = io.BytesIO(json.dumps({"jsonrpc": "2.0", "id": 1, "result": "0x7b"}).encode())
        with patch.object(experiment, "urlopen", return_value=success):
            self.assertEqual(experiment.rpc_call("http://localhost", "eth_chainId", [], 1), "0x7b")

        failure = io.BytesIO(json.dumps({"jsonrpc": "2.0", "id": 2,
                                         "error": {"code": -1, "message": "bad request"}}).encode())
        with patch.object(experiment, "urlopen", return_value=failure):
            with self.assertRaisesRegex(RuntimeError, "bad request"):
                experiment.rpc_call("http://localhost", "eth_chainId", [], 1)


if __name__ == "__main__":
    unittest.main()