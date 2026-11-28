import json
import pathlib
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch


EXPERIMENTS_DIR = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EXPERIMENTS_DIR))

from runtime import contract_readiness
from runtime import deploy_contract


class ContractReadinessTests(unittest.TestCase):
    def test_deployment_candidates_cover_each_node_type_before_same_type_peers(self):
        node_types = contract_readiness.build_node_types(2, 2, 0, 1)

        self.assertEqual(node_types, ["native", "native", "geth", "geth", "wasix"])
        self.assertEqual(contract_readiness.deployment_candidate_indices(node_types), [0, 2, 4, 1, 3])

    def test_contract_code_check_includes_recent_historical_blocks(self):
        calls = []

        def rpc(url, method, params, timeout):
            calls.append((method, params))
            if method == "eth_blockNumber":
                return "0x10"
            if params[1] == "latest":
                return "0x"
            if params[1] == "0xf":
                return "0x6000"
            return "0x"

        with patch.object(contract_readiness, "rpc_call", side_effect=rpc):
            available = contract_readiness.has_contract_code("http://node", "0xcontract", 10)

        self.assertTrue(available)
        self.assertEqual(calls[1], ("eth_getCode", ["0xcontract", "latest"]))
        self.assertEqual(calls[2], ("eth_getCode", ["0xcontract", "0xf"]))

    def test_missing_contract_waits_until_four_blocks_then_requests_fallback(self):
        node = {"index": 0, "type": "native", "rpc_url": "http://node"}
        with patch.object(contract_readiness, "has_contract_code", return_value=False):
            with patch.object(contract_readiness, "rpc_call", side_effect=["0xa", "0xd", "0xe"]) as rpc:
                with patch.object(contract_readiness.time, "sleep"):
                    available, visibility = contract_readiness.wait_for_contract_visibility(
                        [node], "http://node", "0xcontract", 10, timeout=30, poll_interval=0
                    )

        self.assertFalse(available)
        self.assertEqual(visibility, {0: False})
        self.assertEqual(rpc.call_count, 3)

    def test_visibility_timeout_before_four_blocks_is_not_reported_as_missing_code(self):
        node = {"index": 0, "type": "native", "rpc_url": "http://node"}
        with patch.object(contract_readiness, "has_contract_code", return_value=False):
            with patch.object(contract_readiness, "rpc_call", return_value="0xa"):
                with self.assertRaisesRegex(TimeoutError, "did not advance to block 14"):
                    contract_readiness.wait_for_contract_visibility(
                        [node], "http://node", "0xcontract", 10, timeout=0, poll_interval=0
                    )

    def test_visibility_requires_every_node(self):
        nodes = [
            {"index": 0, "type": "native", "rpc_url": "http://native"},
            {"index": 1, "type": "geth", "rpc_url": "http://geth"},
        ]
        with patch.object(
            contract_readiness, "has_contract_code", side_effect=[True, False, True, True]
        ):
            with patch.object(contract_readiness, "rpc_call", return_value="0xb"):
                available, visibility = contract_readiness.wait_for_contract_visibility(
                    nodes, "http://native", "0xcontract", 10, timeout=30, poll_interval=0
                )

        self.assertTrue(available)
        self.assertEqual(visibility, {0: True, 1: True})

    def test_failed_initial_visibility_redeploys_on_an_alternate_node_type(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            root = pathlib.Path(temp_dir)
            args = SimpleNamespace(
                linux_nodes=2,
                geth_nodes=2,
                reth_nodes=0,
                wasix_nodes=0,
                output_address=root / "contract_address.txt",
                output_info=root / "contract_deployment.json",
                log_dir=root / "workloads",
                workload_script=root / "workload.sh",
                chain_id=12345,
                network="suite-net",
                scenario="exp/counter",
                accounts=4,
                visibility_timeout=1,
            )

            def deploy_side_effect(deploy_args, node_index, address_path, info_path, log_path):
                address_path.write_text(f"0x{node_index + 1:040x}\n", encoding="utf-8")
                info_path.write_text(json.dumps({"blockNumber": 100}), encoding="utf-8")
                return 0

            with patch.object(deploy_contract, "deploy_on_node", side_effect=deploy_side_effect) as deploy:
                with patch.object(
                    deploy_contract,
                    "wait_for_contract_visibility",
                    side_effect=[
                        (False, {0: True, 1: True, 2: False, 3: False}),
                        (True, {0: True, 1: True, 2: True, 3: True}),
                    ],
                ):
                    self.assertEqual(deploy_contract.run(args), 0)

            self.assertEqual([call.args[1] for call in deploy.call_args_list], [0, 2])
            self.assertEqual(args.output_address.read_text(encoding="utf-8"), f"0x{3:040x}\n")
            self.assertEqual(json.loads(args.output_info.read_text(encoding="utf-8"))["node_type"], "geth")


if __name__ == "__main__":
    unittest.main()