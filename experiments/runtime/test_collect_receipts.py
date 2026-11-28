import pathlib
import sys
import unittest
from unittest.mock import patch


EXPERIMENTS_DIR = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EXPERIMENTS_DIR))

from runtime import collect_receipts


class RuntimeReceiptCollectionTests(unittest.TestCase):
    def test_receipt_collection_records_observed_inclusion_latency(self):
        tx_hash = "0x" + "a" * 64
        submissions = [{"hash": tx_hash, "submitted_at_epoch": 100.0}]
        receipt = {"blockNumber": "0x2a", "status": "0x1", "gasUsed": "0x5208"}

        def rpc(url, method, params, timeout):
            if method == "eth_getTransactionReceipt":
                return receipt
            return {"timestamp": "0x65"}

        with patch.object(collect_receipts, "rpc_call", side_effect=rpc):
            with patch.object(collect_receipts.time, "time", return_value=101.5):
                result = collect_receipts.collect_receipts("http://node", submissions)

        self.assertEqual(result["submitted_transactions"], 1)
        self.assertEqual(result["receipts_observed"], 1)
        self.assertEqual(result["missing_transactions"], [])
        self.assertEqual(result["transactions"][0]["block_number"], 42)
        self.assertEqual(result["transactions"][0]["inclusion_latency_ms"], 1000)
        self.assertEqual(result["transactions"][0]["receipt_observed_at_epoch"], 101.5)
        self.assertEqual(result["inclusion_latency_ms"]["p95"], 1000)

    def test_missing_receipts_are_reported_without_raising(self):
        tx_hash = "0x" + "b" * 64

        result = collect_receipts.collect_receipts(
            "http://node", [{"hash": tx_hash, "submitted_at_epoch": 100.0}], timeout=0
        )

        self.assertEqual(result["receipts_observed"], 0)
        self.assertEqual(result["missing_transactions"], [tx_hash])


if __name__ == "__main__":
    unittest.main()