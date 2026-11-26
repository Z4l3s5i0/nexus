import csv
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

    def test_contract_workload_loads_bytecode_and_maps_calls(self):
        with tempfile.TemporaryDirectory() as temp:
            bytecode_file = pathlib.Path(temp) / "bytecode.csv"
            bytecode_file.write_text("counter.sol,608060\ndota.sol,608061\nuber_v2.sol,608062\n", encoding="utf-8")
            with patch.object(run_experiment, "BYTECODE_FILE", bytecode_file):
                self.assertEqual(run_experiment.load_workload_bytecode("counter"), "608060")
                self.assertEqual(run_experiment.load_workload_bytecode("uber_v2"), "608062")

        self.assertEqual(run_experiment.WORKLOAD_CALLS, {
            "counter": "Add()",
            "dota": "update()",
            "uber": "checkDistance()",
            "uber_v2": "checkDistance()",
        })

    def test_uber_v2_contract_workload_is_selectable(self):
        args = run_experiment.create_parser().parse_args(["--workload", "uber_v2"])

        self.assertEqual(args.workload, "uber_v2")

    def test_contract_deployment_gas_limit_constant(self):
        self.assertEqual(transaction_submission.CONTRACT_DEPLOYMENT_GAS_LIMIT, 600_000_000)

    def test_contract_gas_limit_defaults_and_requires_positive_value(self):
        args = run_experiment.create_parser().parse_args(["--counts", "0"])
        self.assertEqual(args.contract_gas_limit, 250_000)

        args = run_experiment.create_parser().parse_args(["--counts", "0", "--contract-gas-limit", "0"])
        with self.assertRaisesRegex(ValueError, "--contract-gas-limit must be positive"):
            run_experiment.validate_options(args, "")

    def test_contract_workload_requires_a_valid_key(self):
        args = run_experiment.create_parser().parse_args(["--counts", "0", "--workload", "counter"])
        with patch.object(run_experiment.shutil, "which", return_value="cast"):
            with self.assertRaisesRegex(ValueError, "private-key"):
                run_experiment.validate_options(args, "")

    def test_block_transaction_count_uses_actual_rpc_payload(self):
        block = {"transactions": ["0x1", "0x2"]}
        self.assertEqual(block_collection.block_transaction_count(block), 2)
        with self.assertRaisesRegex(RuntimeError, "transaction list"):
            block_collection.block_transaction_count({"transactions": None})

    def test_receipt_polling_records_first_observation_time(self):
        tx_hash = "0x" + "a" * 64
        receipt = {"status": "0x1", "blockNumber": "0x2"}
        with patch.object(block_collection, "rpc_call", return_value=receipt):
            with patch.object(block_collection.time, "monotonic", return_value=12.5):
                with patch.object(block_collection.time, "time", return_value=100.25):
                    receipts, observed = block_collection.wait_for_receipts(
                        "http://localhost", [tx_hash], 5, 0.1, include_observation_times=True
                    )

        self.assertEqual(receipts[tx_hash], receipt)
        self.assertEqual(observed[tx_hash], {"monotonic": 12.5, "epoch": 100.25})

    def test_receipt_polling_retries_transaction_indexing_in_progress(self):
        tx_hash = "0x" + "a" * 64
        receipt = {"status": "0x1", "blockNumber": "0x2"}
        indexing_error = RuntimeError(
            "RPC eth_getTransactionReceipt failed: transaction indexing is in progress"
        )
        with patch.object(block_collection, "rpc_call", side_effect=[indexing_error, receipt]) as rpc:
            with patch.object(block_collection.time, "monotonic", side_effect=[0.0, 0.1, 0.2]):
                with patch.object(block_collection.time, "time", return_value=100.25):
                    with patch.object(block_collection.time, "sleep") as sleep:
                        receipts = block_collection.wait_for_receipts(
                            "http://localhost", [tx_hash], 5, 0.1
                        )

        self.assertEqual(receipts, {tx_hash: receipt})
        self.assertEqual(rpc.call_count, 2)
        sleep.assert_called_once_with(0.1)

    def test_receipt_polling_propagates_other_rpc_errors(self):
        error = RuntimeError("RPC eth_getTransactionReceipt failed: invalid params")
        with patch.object(block_collection, "rpc_call", side_effect=error):
            with self.assertRaisesRegex(RuntimeError, "invalid params"):
                block_collection.wait_for_receipts("http://localhost", ["0xhash"], 5, 0.1)

    def test_receipt_polling_can_record_reverted_transaction_inclusion(self):
        tx_hash = "0x" + "a" * 64
        receipt = {"status": "0x0", "blockNumber": "0x2"}
        with patch.object(block_collection, "rpc_call", return_value=receipt):
            with patch.object(block_collection.time, "monotonic", return_value=12.5):
                with patch.object(block_collection.time, "time", return_value=100.25):
                    receipts, observed = block_collection.wait_for_receipts(
                        "http://localhost", [tx_hash], 5, 0.1,
                        include_observation_times=True, allow_failed_receipts=True,
                    )

        self.assertEqual(receipts[tx_hash], receipt)
        self.assertEqual(observed[tx_hash], {"monotonic": 12.5, "epoch": 100.25})

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
        self.assertIn("-i", docker_run.call_args.args[0])
        self.assertEqual(docker_run.call_args.args[0][-2:], ["foundry:test", "-s"])
        self.assertIn("--legacy", docker_run.call_args.kwargs["input"])
        self.assertIn("--gas-price 2500000000", docker_run.call_args.kwargs["input"])

    def test_docker_cast_streams_large_deployment_bytecode_over_stdin(self):
        bytecode = "0x" + "ab" * 70_000
        tx_hash = "0x" + "a" * 64
        result = subprocess.CompletedProcess([], 0, stdout=tx_hash, stderr="")
        with patch.object(transaction_submission.shutil, "which", return_value="docker"):
            with patch.object(transaction_submission.subprocess, "run", return_value=result) as docker_run:
                output = transaction_submission.run_cast(
                    None, ["send", "--create", bytecode], 30, "suite-net", "foundry:test",
                )

        command = docker_run.call_args.args[0]
        self.assertEqual(output, tx_hash)
        self.assertLess(max(map(len, command)), 128_000)
        self.assertEqual(docker_run.call_args.kwargs["input"], f"cast send --create {bytecode}")

    def test_docker_cast_burst_captures_each_container_submission_time(self):
        tx_hash = "0x" + "a" * 64
        result = type(
            "Result",
            (),
            {"returncode": 0, "stdout": f"SUBMITTED_AT=1000.5\nSubmitted transaction: {tx_hash}\n", "stderr": ""},
        )()
        with patch.object(transaction_submission.shutil, "which", return_value="docker"):
            with patch.object(transaction_submission.subprocess, "run", return_value=result) as docker_run:
                with patch.object(transaction_submission.time, "monotonic", return_value=20.0):
                    with patch.object(transaction_submission.time, "time", return_value=1000.0):
                        submitted = transaction_submission.submit_burst(
                            None, "network", "image", "http://node:8545", "0xsender", "0xkey", 7,
                            1, 30, "2500000000", to_address="0xcontract", function_signature="Add()",
                            include_timestamps=True,
                        )

        self.assertEqual(submitted[0]["hash"], tx_hash)
        self.assertEqual(submitted[0]["submitted_at"], {"monotonic": 20.5, "epoch": 1000.5})
        self.assertIn("date +%s.%N", docker_run.call_args.kwargs["input"])

    def test_contract_deployment_submits_bytecode_with_cast_create(self):
        tx_hash = "0x" + "a" * 64
        sent_at = {"monotonic": 1.0, "epoch": 50.0}
        with patch.object(transaction_submission, "run_cast", return_value=(tx_hash, sent_at)) as cast:
            result = transaction_submission.submit_contract_deployment(
                "cast", "network", "image", "http://localhost:8545", "0xkey", 4, "0x608060", 30, 7,
            )

        self.assertEqual(result, (tx_hash, sent_at))
        arguments = cast.call_args.args[1]
        self.assertEqual(arguments[-2:], ["--create", "0x608060"])
        legacy_index = arguments.index("--legacy")
        gas_limit_index = arguments.index("--gas-limit")
        gas_price_index = arguments.index("--gas-price")
        create_index = arguments.index("--create")
        self.assertLess(legacy_index, create_index)
        self.assertLess(gas_limit_index, create_index)
        self.assertLess(gas_price_index, create_index)
        self.assertEqual(arguments[gas_limit_index + 1], "600000000")
        self.assertEqual(arguments[gas_price_index + 1], "7")

    def test_workload_calls_target_the_contract_and_capture_send_time(self):
        tx_hash = "0x" + "a" * 64
        sent_at = {"monotonic": 1.5, "epoch": 50.25}
        with patch.object(transaction_submission, "run_cast", return_value=(tx_hash, sent_at)) as cast:
            submitted = transaction_submission.submit_burst(
                "cast", "network", "image", "http://localhost:8545", "0xsender", "0xkey", 7,
                1, 30, "2500000000", to_address="0xcontract", function_signature="update()",
                include_timestamps=True, contract_gas_limit=420_000,
            )

        self.assertEqual(submitted, [{
            "hash": tx_hash,
            "submitted_at": {"monotonic": 1.5, "epoch": 50.25},
        }])
        arguments = cast.call_args.args[1]
        self.assertEqual(arguments[:3], ["send", "0xcontract", "update()"])
        self.assertNotIn("--value", arguments)
        self.assertIn("--gas-limit", arguments)
        gas_limit_index = arguments.index("--gas-limit")
        self.assertEqual(arguments[gas_limit_index + 1], "420000")

    def test_contract_deployment_precedes_calls_and_is_recorded_separately(self):
        sender = "0x" + "1" * 40
        address = "0x" + "2" * 40
        deployment_hash = "0x" + "a" * 64
        call_hash = "0x" + "b" * 64
        submitted_at = {"monotonic": 10.0, "epoch": 100.0}
        observed_at = {"monotonic": 10.1, "epoch": 100.1}
        events = []

        with tempfile.TemporaryDirectory() as temp:
            output = pathlib.Path(temp) / "block_samples.csv"
            latency_output = pathlib.Path(temp) / "transaction_inclusion_samples.csv"
            bytecode_file = pathlib.Path(temp) / "bytecode.csv"
            bytecode_file.write_text("counter.sol,608060\n", encoding="utf-8")
            args = run_experiment.create_parser().parse_args([
                "--counts", "1", "--blocks-per-count", "1", "--max-attempts", "1",
                "--private-key", "0x" + "3" * 64, "--workload", "counter",
                "--output", str(output), "--inclusion-output", str(latency_output),
            ])

            def rpc(rpc_url, method, params, timeout):
                if method == "eth_chainId":
                    return "0x5ba0"
                if method == "eth_getTransactionCount":
                    return "0x1" if len([event for event in events if event == "nonce"]) == 0 else "0x2"
                if method == "eth_gasPrice":
                    return "0x3"
                if method == "eth_getCode":
                    events.append("verify")
                    return "0x6000"
                if method == "eth_getBlockByNumber":
                    number = int(params[0], 16)
                    transactions = [deployment_hash] if number == 1 else [call_hash]
                    return {"timestamp": "0x64", "transactions": transactions}
                raise AssertionError(method)

            def wait_for_receipts(
                rpc_url, hashes, timeout, poll_interval, include_observation_times=False, allow_failed_receipts=False
            ):
                tx_hash = hashes[0]
                if tx_hash == deployment_hash:
                    receipt = {
                        "status": "0x1", "blockNumber": "0x1", "contractAddress": address, "gasUsed": "0x100",
                    }
                else:
                    receipt = {"status": "0x1", "blockNumber": "0x2", "gasUsed": "0x200"}
                return {tx_hash: receipt}, {tx_hash: observed_at}

            def submit_burst(*call_args, **call_kwargs):
                events.append("calls")
                self.assertEqual(call_kwargs["to_address"], address)
                self.assertEqual(call_kwargs["function_signature"], "Add()")
                return [{"hash": call_hash, "submitted_at": submitted_at}]

            with patch.object(run_experiment, "BYTECODE_FILE", bytecode_file):
                with patch.object(run_experiment.shutil, "which", return_value="cast"):
                    with patch.object(run_experiment, "run_cast", return_value=sender):
                        with patch.object(run_experiment, "rpc_call", side_effect=rpc):
                            with patch.object(block_collection, "rpc_call", side_effect=rpc):
                                with patch.object(run_experiment, "submit_contract_deployment", return_value=(deployment_hash, submitted_at)):
                                    with patch.object(run_experiment, "submit_burst", side_effect=submit_burst):
                                        with patch.object(run_experiment, "wait_for_receipts", side_effect=wait_for_receipts):
                                            result = run_experiment.run(args)

            with latency_output.open(newline="", encoding="utf-8") as source:
                latency_rows = list(csv.DictReader(source))
            deployment = json.loads((pathlib.Path(temp) / "contract_deployment.json").read_text(encoding="utf-8"))

        self.assertEqual(result, 0)
        self.assertLess(events.index("verify"), events.index("calls"))
        self.assertEqual([row["transaction_type"] for row in latency_rows], ["contract_deployment", "contract_call"])
        self.assertEqual(deployment["contract_address"], address)
        self.assertTrue(deployment["confirmed"])

    def test_completed_burst_split_across_blocks_counts_as_a_sample(self):
        sender = "0x" + "1" * 40
        hashes = ["0x" + "a" * 64, "0x" + "b" * 64]
        submitted_at = [
            {"hash": hashes[0], "submitted_at": {"monotonic": 10.0, "epoch": 100.0}},
            {"hash": hashes[1], "submitted_at": {"monotonic": 10.1, "epoch": 100.1}},
        ]
        observed_at = {
            hashes[0]: {"monotonic": 10.2, "epoch": 100.2},
            hashes[1]: {"monotonic": 11.5, "epoch": 101.5},
        }

        with tempfile.TemporaryDirectory() as temp:
            output = pathlib.Path(temp) / "block_samples.csv"
            inclusion_output = pathlib.Path(temp) / "transaction_inclusion_samples.csv"
            burst_output = pathlib.Path(temp) / "transaction_burst_samples.csv"
            args = run_experiment.create_parser().parse_args([
                "--counts", "2", "--blocks-per-count", "1", "--max-attempts", "1",
                "--private-key", "0x" + "3" * 64,
                "--output", str(output), "--inclusion-output", str(inclusion_output),
            ])

            def rpc(rpc_url, method, params, timeout):
                if method == "eth_chainId":
                    return "0x5ba0"
                if method == "eth_getTransactionCount":
                    return "0x1"
                if method == "eth_gasPrice":
                    return "0x3"
                if method == "eth_getBlockByNumber":
                    number = int(params[0], 16)
                    included_hash = hashes[number - 2]
                    return {"timestamp": "0x64", "transactions": [included_hash]}
                raise AssertionError(method)

            receipts = {
                hashes[0]: {"status": "0x1", "blockNumber": "0x2", "gasUsed": "0x5208"},
                hashes[1]: {"status": "0x1", "blockNumber": "0x3", "gasUsed": "0x5208"},
            }
            observed = {tx_hash: observed_at[tx_hash] for tx_hash in hashes}

            with patch.object(run_experiment.shutil, "which", return_value="cast"):
                with patch.object(run_experiment, "run_cast", return_value=sender):
                    with patch.object(run_experiment, "rpc_call", side_effect=rpc):
                        with patch.object(run_experiment, "block_details", return_value=(1, 100)):
                            with patch.object(run_experiment, "submit_burst", return_value=submitted_at):
                                with patch.object(run_experiment, "wait_for_receipts", return_value=(receipts, observed)):
                                    result = run_experiment.run(args)

            self.assertTrue(burst_output.is_file())
            with burst_output.open(newline="", encoding="utf-8") as source:
                burst_rows = list(csv.DictReader(source))
            with output.open(newline="", encoding="utf-8") as source:
                block_rows = list(csv.DictReader(source))

        self.assertEqual(result, 0)
        self.assertEqual(len(burst_rows), 1)
        self.assertEqual(burst_rows[0]["completed"], "true")
        self.assertEqual(burst_rows[0]["blocks_spanned"], "2")
        self.assertEqual(burst_rows[0]["block_distribution"], "2:1;3:1")
        self.assertAlmostEqual(float(burst_rows[0]["burst_completion_latency_ms"]), 1500.0)
        self.assertEqual(len(block_rows), 2)
        self.assertTrue(all(row["matched"] == "false" for row in block_rows))

    def test_report_summarizes_completed_bursts_and_split_block_latency(self):
        with tempfile.TemporaryDirectory() as temp:
            output_dir = pathlib.Path(temp)
            metadata = {
                "experiment_id": "run-1", "chain_id": 100, "client": "wasix-eth",
                "sweep": {"transaction_counts": [100], "blocks_per_count": 1},
            }
            (output_dir / "metadata.json").write_text(json.dumps(metadata), encoding="utf-8")
            (output_dir / "samples.csv").write_text(
                "target_tx_count,block_number,actual_tx_count,matched,execution_time_ms\n"
                "100,22,78,false,\n100,23,22,false,\n",
                encoding="utf-8",
            )
            (output_dir / "transaction_burst_samples.csv").write_text(
                "transaction_type,target_tx_count,attempt,completed,blocks_spanned,burst_completion_latency_ms\n"
                "contract_call,100,1,true,2,1300\n",
                encoding="utf-8",
            )
            (output_dir / "transaction_inclusion_samples.csv").write_text(
                "transaction_type,target_tx_count,attempt,inclusion_latency_ms,status\n"
                "contract_call,100,1,100,0x1\ncontract_call,100,1,1300,0x1\n",
                encoding="utf-8",
            )

            report = empty_block_report.build_report(output_dir)

        target_summary = report["by_target_transaction_count"]["100"]
        spread_latency = report["transaction_inclusion_latency_by_target"]["contract_call"]["100"]["multiple_blocks"]
        self.assertEqual(target_summary["completed_bursts"], 1)
        self.assertEqual(target_summary["missing_bursts"], 0)
        self.assertEqual(target_summary["exact_block_matches"], 0)
        self.assertEqual(spread_latency["observations"], 2)
        self.assertEqual(spread_latency["mean_ms"], 700)
        self.assertEqual(spread_latency["burst_completion_latency_ms"]["mean"], 1300)

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

    def test_rpc_derived_metrics_include_transaction_inclusion_latency(self):
        samples, coverage = collect_block_metrics.derive_block_samples(
            [],
            {"chain_id": 100, "client": "geth"},
            "run-1",
            [{
                "transaction_type": "contract_call",
                "workload": "dota",
                "target_tx_count": "5",
                "attempt": "2",
                "tx_hash": "0xabc",
                "block_number": "8",
                "included_at_epoch": "100.5",
                "inclusion_latency_ms": "27.25",
            }],
        )

        latency = next(sample for sample in samples if sample["name"] == "empty_block_transaction_inclusion_latency_ms")
        self.assertEqual(latency["value"], 27.25)
        self.assertEqual(json.loads(latency["labels_json"])["workload"], "dota")
        self.assertEqual(coverage["fields"]["transaction_inclusion_latency"], "observed")

    def test_rpc_derived_metrics_include_multi_block_burst_completion(self):
        samples, coverage = collect_block_metrics.derive_block_samples(
            [],
            {"chain_id": 100, "client": "wasix-eth"},
            "run-1",
            burst_rows=[{
                "transaction_type": "contract_call",
                "workload": "uber_v2",
                "target_tx_count": "100",
                "attempt": "2",
                "blocks_spanned": "2",
                "completed": "true",
                "burst_completion_latency_ms": "1500.5",
                "completed_at_utc": "1970-01-01T00:01:40.500000+00:00",
            }],
        )

        latency = next(
            sample for sample in samples
            if sample["name"] == "empty_block_transaction_burst_completion_latency_ms"
        )
        spans = next(
            sample for sample in samples
            if sample["name"] == "empty_block_transaction_burst_blocks_spanned"
        )
        self.assertEqual(latency["value"], 1500.5)
        self.assertEqual(spans["value"], 2.0)
        self.assertEqual(json.loads(latency["labels_json"])["workload"], "uber_v2")
        self.assertEqual(coverage["fields"]["transaction_burst_completion_latency"], "observed")

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
                    "workload": "none",
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
            (output_dir / "transaction_inclusion_samples.csv").write_text(
                "transaction_type,workload,inclusion_latency_ms,status\ncontract_call,dota,27.25,0x0\n",
                encoding="utf-8",
            )

            report = empty_block_report.build_report(output_dir)

        self.assertEqual(report["by_target_transaction_count"]["0"]["missing_matches"], 1)
        self.assertEqual(report["by_target_transaction_count"]["1"]["expected_matches"], 2)
        self.assertEqual(report["transaction_inclusion_latency"]["contract_call"]["observations"], 1)
        self.assertEqual(report["transaction_inclusion_latency"]["contract_call"]["failed_observations"], 1)
        self.assertEqual(report["transaction_inclusion_latency"]["contract_call"]["mean_ms"], 27.25)
        self.assertEqual(report["run_status"]["status"], 2)
        self.assertFalse(report["metrics_database_archived"])


if __name__ == "__main__":
    unittest.main()