#!/usr/bin/env python3
"""Collect transaction receipts after runtime spam has completed."""

import argparse
import concurrent.futures
import json
import math
import sys
import time
from pathlib import Path

EMPTY_BLOCK_DIR = Path(__file__).resolve().parents[1] / "empty-block"
sys.path.insert(0, str(EMPTY_BLOCK_DIR))

from block_collection import rpc_call


def collect_receipts(rpc_url, submissions, timeout=120, poll_interval=1, concurrency=32):
    if timeout < 0 or poll_interval < 0 or concurrency < 1:
        raise ValueError("receipt collection timeout and polling interval must be nonnegative")

    pending = {item["hash"]: item for item in submissions}
    receipts = {}
    errors = {}
    observed_times = {}
    deadline = time.monotonic() + timeout

    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
        while pending:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break

            futures = {
                executor.submit(
                    rpc_call,
                    rpc_url,
                    "eth_getTransactionReceipt",
                    [tx_hash],
                    min(remaining, 10),
                ): tx_hash
                for tx_hash in pending
            }
            try:
                completed = concurrent.futures.as_completed(futures, timeout=remaining)
                for future in completed:
                    tx_hash = futures[future]
                    try:
                        receipt = future.result()
                    except Exception as exc:
                        errors[tx_hash] = str(exc)
                        continue
                    if receipt is not None:
                        receipts[tx_hash] = receipt
                        observed_times[tx_hash] = time.time()
                        errors.pop(tx_hash, None)
            except concurrent.futures.TimeoutError:
                for future in futures:
                    future.cancel()

            pending = {tx_hash: item for tx_hash, item in pending.items() if tx_hash not in receipts}
            if pending:
                remaining = deadline - time.monotonic()
                if remaining > 0:
                    time.sleep(min(poll_interval, remaining))

    receipt_blocks = {
        int(receipt["blockNumber"], 16)
        for receipt in receipts.values()
        if isinstance(receipt.get("blockNumber"), str)
    }
    block_timestamps = {}
    block_deadline = time.monotonic() + min(timeout, 30)
    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
        futures = {
            executor.submit(
                rpc_call,
                rpc_url,
                "eth_getBlockByNumber",
                [hex(block_number), False],
                min(max(block_deadline - time.monotonic(), 0.1), 10),
            ): block_number
            for block_number in receipt_blocks
        }
        try:
            for future in concurrent.futures.as_completed(
                futures, timeout=max(0, block_deadline - time.monotonic())
            ):
                block_number = futures[future]
                try:
                    block = future.result()
                    if block is not None and isinstance(block.get("timestamp"), str):
                        block_timestamps[block_number] = int(block["timestamp"], 16)
                except Exception:
                    continue
        except concurrent.futures.TimeoutError:
            for future in futures:
                future.cancel()

    included = []
    for submission in submissions:
        tx_hash = submission["hash"]
        receipt = receipts.get(tx_hash)
        if receipt is None:
            continue
        submitted_at = float(submission["submitted_at_epoch"])
        observed_at = observed_times[tx_hash]
        block_number = (
            int(receipt["blockNumber"], 16)
            if isinstance(receipt.get("blockNumber"), str)
            else None
        )
        block_timestamp = block_timestamps.get(block_number)
        included.append(
            {
                "transaction_hash": tx_hash,
                "submitted_at_epoch": submitted_at,
                "receipt_observed_at_epoch": observed_at,
                "block_number": block_number,
                "block_timestamp_epoch": block_timestamp,
                "inclusion_latency_ms": max(0.0, (block_timestamp - submitted_at) * 1000)
                if block_timestamp is not None
                else None,
                "status": receipt.get("status"),
                "gas_used": receipt.get("gasUsed"),
            }
        )

    latencies = sorted(
        item["inclusion_latency_ms"]
        for item in included
        if item["inclusion_latency_ms"] is not None
    )

    def percentile(value):
        if not latencies:
            return None
        index = max(0, min(len(latencies) - 1, math.ceil(len(latencies) * value) - 1))
        return latencies[index]

    return {
        "submitted_transactions": len(submissions),
        "receipts_observed": len(receipts),
        "missing_transactions": [tx_hash for tx_hash in pending],
        "receipt_errors": {tx_hash: errors[tx_hash] for tx_hash in pending if tx_hash in errors},
        "inclusion_latency_ms": {
            "measured_transactions": len(latencies),
            "mean": sum(latencies) / len(latencies) if latencies else None,
            "p50": percentile(0.5),
            "p95": percentile(0.95),
            "max": max(latencies) if latencies else None,
        },
        "transactions": included,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rpc-url", required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=120)
    args = parser.parse_args()
    try:
        manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
        result = collect_receipts(args.rpc_url, manifest["submitted_transactions"], timeout=args.timeout)
        result["node_index"] = manifest.get("node_index")
        result["planned_transactions"] = manifest.get("planned_transactions")
        if isinstance(result["planned_transactions"], int):
            result["not_accepted_transactions"] = max(
                0, result["planned_transactions"] - result["submitted_transactions"]
            )
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(
            f"Observed {result['receipts_observed']} receipt(s) for "
            f"{result['submitted_transactions']} accepted transaction(s); "
            f"{len(result['missing_transactions'])} remained unconfirmed.",
            flush=True,
        )
        return 0
    except Exception as exc:
        print(f"Receipt collection failed: {exc}", file=sys.stderr, flush=True)
        return 1


if __name__ == "__main__":
    sys.exit(main())