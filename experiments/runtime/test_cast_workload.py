import json
import pathlib
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch


EXPERIMENTS_DIR = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EXPERIMENTS_DIR))

from runtime import cast_workload


class CastRuntimeWorkloadTests(unittest.TestCase):
    def test_scenario_names_cover_all_runtime_workloads(self):
        for scenario in ("counter", "dota", "uber", "uber_v2", "value-transfer"):
            with self.subTest(scenario=scenario):
                self.assertEqual(cast_workload.scenario_name(f"exp/{scenario}"), scenario)

        with self.assertRaisesRegex(ValueError, "unsupported Cast runtime scenario"):
            cast_workload.scenario_name("exp/unknown")

    def test_runtime_runner_normalizes_prefixed_value_transfer_before_contract_setup(self):
        runner = pathlib.Path(__file__).with_name("run.sh").read_text(encoding="utf-8")

        self.assertIn('WORKLOAD_SCENARIO="${WORKLOAD_TYPE#exp/}"', runner)
        self.assertIn('if [[ "$WORKLOAD_SCENARIO" != "value-transfer" ]]', runner)

    def test_wallet_output_parsing(self):
        private_key = "a" * 64
        address = "0x" + "b" * 40
        self.assertEqual(cast_workload.wallet_private_key(f"Private key: 0x{private_key}"), private_key)
        self.assertEqual(cast_workload.wallet_address(f"Address: {address}"), address)

    def test_rpc_quantity_adds_context_when_a_request_times_out(self):
        with patch.object(cast_workload, "rpc_call", side_effect=TimeoutError("timed out")) as rpc:
            with self.assertRaisesRegex(TimeoutError, "eth_estimateGas.*120 seconds"):
                cast_workload.rpc_quantity(
                    "http://node", "eth_estimateGas", [], timeout=120
                )

        rpc.assert_called_once_with("http://node", "eth_estimateGas", [], 120)

    def test_account_derivation_uses_nonoverlapping_node_ranges(self):
        keys = ["0x" + format(index, "064x") for index in (1, 2)]
        addresses = ["0x" + format(index, "040x") for index in (3, 4)]
        with patch.object(cast_workload, "run_cast_batch", side_effect=[keys, addresses]) as cast_batch:
            accounts = cast_workload.derive_accounts(2, 2, "seed phrase", "suite-net", "foundry:test")

        self.assertEqual(accounts, [(key[2:], address) for key, address in zip(keys, addresses)])
        commands = cast_batch.call_args_list[0].args[3]
        self.assertEqual([command[-1] for command in commands], ["10004", "10005"])

    def test_value_transfer_uses_round_robin_accounts_and_pending_nonces(self):
        args = SimpleNamespace(tps=2, duration=2)
        accounts = [
            ("key-one", "0x" + "1" * 40),
            ("key-two", "0x" + "2" * 40),
        ]
        with patch.object(cast_workload, "rpc_quantity", side_effect=[3, 8, 2_500_000_000]):
            transactions = cast_workload.runtime_transactions(args, "value-transfer", accounts, None, "rpc")

        self.assertEqual([transaction["nonce"] for transaction in transactions], [3, 8, 4, 9])
        self.assertEqual([transaction["private_key"] for transaction in transactions], ["key-one", "key-two"] * 2)
        self.assertTrue(all(transaction["to_address"] == cast_workload.VALUE_TRANSFER_TO for transaction in transactions))
        self.assertTrue(all(transaction["value"] == "1" for transaction in transactions))
        self.assertTrue(all(transaction["gas_limit"] == 21_000 for transaction in transactions))

    def test_contract_workload_uses_existing_call_signature_and_fixed_gas(self):
        args = SimpleNamespace(tps=1, duration=1)
        accounts = [("key-one", "0x" + "1" * 40)]
        with patch.object(cast_workload, "rpc_quantity", side_effect=[0, 2_500_000_000]):
            transactions = cast_workload.runtime_transactions(
                args, "counter", accounts, "0x" + "2" * 40, "rpc"
            )

        self.assertEqual(transactions[0]["to_address"], "0x" + "2" * 40)
        self.assertEqual(transactions[0]["function_signature"], "Add()")
        self.assertEqual(transactions[0]["gas_limit"], cast_workload.CONTRACT_CALL_GAS_LIMIT)

    def test_setup_funds_each_derived_account_with_cast_batch(self):
        args = SimpleNamespace(
            node_index=0,
            accounts=2,
            chain_id=12345,
            network="suite-net",
            cast_image="foundry:test",
            private_key="a" * 64,
        )
        sender = "0x" + "1" * 40
        accounts = [("key-one", "0x" + "2" * 40), ("key-two", "0x" + "3" * 40)]
        with patch.object(cast_workload, "cast_sender", return_value=sender):
            with patch.object(cast_workload, "derive_accounts", return_value=accounts):
                with patch.object(cast_workload, "rpc_quantity", side_effect=[12345, 99, 5]):
                    with patch.object(cast_workload, "submit_transactions", return_value=["hash-1", "hash-2"]) as submit:
                        with patch.object(cast_workload, "wait_for_receipts") as receipts:
                            actual_accounts, contract_address = cast_workload.prepare_accounts(
                                args, "host-rpc", "container-rpc", "seed", "value-transfer"
                            )

        self.assertEqual(actual_accounts, accounts)
        self.assertIsNone(contract_address)
        transactions = submit.call_args.args[4]
        self.assertEqual([transaction["to_address"] for transaction in transactions], [address for _, address in accounts])
        self.assertEqual([transaction["nonce"] for transaction in transactions], [5, 6])
        self.assertTrue(all(transaction["value"] == "100ether" for transaction in transactions))
        receipts.assert_called_once()

    def test_shared_contract_deployment_uses_rpc_estimated_gas(self):
        args = SimpleNamespace(
            node_index=0,
            chain_id=12345,
            network="suite-net",
            cast_image="foundry:test",
            private_key="a" * 64,
        )
        sender = "0x" + "1" * 40
        account = ("key-one", "0x" + "2" * 40)
        contract_address = "0x" + "3" * 40
        with patch.object(cast_workload, "cast_sender", return_value=sender):
            with patch.object(
                cast_workload,
                "rpc_quantity",
                side_effect=[12345, 99, 2_500_000_000, 420_000],
            ) as rpc:
                with patch.object(
                    cast_workload,
                    "wait_for_receipts",
                    return_value={"deploy-hash": {"contractAddress": contract_address, "blockNumber": "0x64"}},
                ):
                    with patch.object(cast_workload, "load_workload_bytecode", return_value="6000"):
                        with patch.object(cast_workload, "rpc_call", return_value="0x6000"):
                            with patch.object(
                                cast_workload,
                                "submit_contract_deployment",
                                return_value=("deploy-hash", None),
                            ) as deploy:
                                result = cast_workload.deploy_workload_contract(
                                    args, "host-rpc", "container-rpc", "counter"
                                )

        self.assertEqual(result, (contract_address, 100))
        self.assertEqual(
            rpc.call_args_list[-1].args,
            ("host-rpc", "eth_estimateGas", [{"from": sender, "data": "0x6000"}]),
        )
        self.assertEqual(
            rpc.call_args_list[-1].kwargs,
            {"timeout": cast_workload.DEPLOYMENT_ESTIMATE_TIMEOUT_SECONDS},
        )
        self.assertEqual(deploy.call_args.args[-1], 420_000)

    def test_uber_v2_deployment_uses_large_gas_limit_instead_of_rpc_estimate(self):
        args = SimpleNamespace(
            node_index=0,
            chain_id=12345,
            network="suite-net",
            cast_image="foundry:test",
            private_key="a" * 64,
        )
        sender = "0x" + "1" * 40
        contract_address = "0x" + "3" * 40
        with patch.object(cast_workload, "cast_sender", return_value=sender):
            with patch.object(
                cast_workload,
                "rpc_quantity",
                side_effect=[12345, 99, 950_000_000, 30_000_000],
            ) as rpc:
                with patch.object(
                    cast_workload,
                    "wait_for_receipts",
                    return_value={"deploy-hash": {"contractAddress": contract_address, "blockNumber": "0x64"}},
                ):
                    with patch.object(cast_workload, "load_workload_bytecode", return_value="6000"):
                        with patch.object(cast_workload, "rpc_call", return_value="0x6000"):
                            with patch.object(
                                cast_workload,
                                "submit_contract_deployment",
                                return_value=("deploy-hash", None),
                            ) as deploy:
                                result = cast_workload.deploy_workload_contract(
                                    args, "host-rpc", "container-rpc", "uber_v2"
                                )

        self.assertEqual(result, (contract_address, 100))
        self.assertNotIn("eth_estimateGas", [call.args[1] for call in rpc.call_args_list])
        self.assertEqual(deploy.call_args.args[-1], 600_000_000)

    def test_contract_workload_reuses_address_without_deploying(self):
        args = SimpleNamespace(
            node_index=1,
            accounts=1,
            chain_id=12345,
            network="suite-net",
            cast_image="foundry:test",
            private_key="a" * 64,
        )
        sender = "0x" + "1" * 40
        account = ("key-one", "0x" + "2" * 40)
        contract_address = "0x" + "3" * 40
        with patch.object(cast_workload, "cast_sender", return_value=sender):
            with patch.object(cast_workload, "derive_accounts", return_value=[account]):
                with patch.object(cast_workload, "rpc_quantity", side_effect=[12345, 2_500_000_000, 5]):
                    with patch.object(cast_workload, "has_contract_code", return_value=True) as has_code:
                        with patch.object(cast_workload, "submit_transactions", return_value=["funding-hash"]) as submit:
                            with patch.object(cast_workload, "wait_for_receipts") as receipts:
                                with patch.object(cast_workload, "deploy_workload_contract") as deploy:
                                    actual_accounts, actual_contract = cast_workload.prepare_accounts(
                                        args,
                                        "host-rpc",
                                        "container-rpc",
                                        "seed",
                                        "counter",
                                        contract_address,
                                    )

        self.assertEqual(actual_accounts, [account])
        self.assertEqual(actual_contract, contract_address)
        has_code.assert_called_once_with("host-rpc", contract_address)
        self.assertEqual(
            [transaction["to_address"] for transaction in submit.call_args.args[4]],
            [account[1]],
        )
        receipts.assert_called_once()
        deploy.assert_not_called()

    def test_deploy_only_writes_shared_contract_address(self):
        args = SimpleNamespace(
            node_index=0,
            chain_id=12345,
            network="suite-net",
            cast_image="foundry:test",
            private_key="a" * 64,
            tps=4,
            duration=300,
            accounts=4,
            start_delay=30,
            scenario="counter",
            deploy_contract=True,
            contract_address_file=None,
            contract_address=None,
            run_id=None,
            window_file=None,
        )
        contract_address = "0x" + "3" * 40
        with tempfile.TemporaryDirectory() as temp_dir:
            args.contract_address_file = pathlib.Path(temp_dir) / "contract_address.txt"
            args.deployment_info_file = pathlib.Path(temp_dir) / "deployment.json"
            with patch.dict("os.environ", {"EXPERIMENT_WALLET_MNEMONIC": "seed phrase"}):
                with patch.object(
                    cast_workload,
                    "deploy_workload_contract",
                    return_value=(contract_address, 100),
                ) as deploy:
                    self.assertEqual(cast_workload.run(args), 0)

            self.assertEqual(args.contract_address_file.read_text(encoding="utf-8"), contract_address + "\n")
            self.assertEqual(
                json.loads(args.deployment_info_file.read_text(encoding="utf-8")),
                {"contract_address": contract_address, "blockNumber": 100},
            )
            deploy.assert_called_once()


if __name__ == "__main__":
    unittest.main()