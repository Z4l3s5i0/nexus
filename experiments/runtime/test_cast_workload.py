import pathlib
import sys
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

    def test_wallet_output_parsing(self):
        private_key = "a" * 64
        address = "0x" + "b" * 40
        self.assertEqual(cast_workload.wallet_private_key(f"Private key: 0x{private_key}"), private_key)
        self.assertEqual(cast_workload.wallet_address(f"Address: {address}"), address)

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


if __name__ == "__main__":
    unittest.main()