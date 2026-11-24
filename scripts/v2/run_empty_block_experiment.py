#!/usr/bin/env python3
"""Measure execution blocks with controlled transaction-count bursts.

Run against an idle local test network. Each positive count is submitted as a
nonce-ordered burst and the script records the actual transaction count in the
mined block; it does not assume that a burst fits in one block.

Example:
    EB_PRIVATE_KEY=<funded-test-key> python3 run_empty_block_experiment.py \
        --rpc-url http://127.0.0.1:8545 --blocks-per-count 10 \
        --execution-log /path/to/aggregated_el_node.log
"""

import argparse
import csv
import json
import os
import re
import shutil
import subprocess
import sys
import time
from collections import defaultdict
from datetime import datetime
from pathlib import Path
from urllib.parse import urlparse
from urllib.request import Request, urlopen


DEFAULT_COUNTS = (0, 1, 5, 10, 20, 50, 100)
TX_HASH_RE = re.compile(r"0x[0-9a-fA-F]{64}")
EXECUTION_TIME_RE = re.compile(r"Block (\d+) executed in ([\d.]+)(m?s)")
RPC_ID = 0


def parse_counts(value):
    try:
        counts = [int(item.strip()) for item in value.split(",")]
    except ValueError as exc:
        raise argparse.ArgumentTypeError("counts must be comma-separated nonnegative integers") from exc
    if not counts or any(count < 0 for count in counts):
        raise argparse.ArgumentTypeError("counts must be comma-separated nonnegative integers")
    if len(set(counts)) != len(counts):
        raise argparse.ArgumentTypeError("transaction counts must not contain duplicates")
    return counts


def rpc_call(rpc_url, method, params, timeout):
    global RPC_ID
    RPC_ID += 1
    body = json.dumps({"jsonrpc": "2.0", "id": RPC_ID, "method": method, "params": params}).encode()
    request = Request(rpc_url, data=body, headers={"Content-Type": "application/json"})
    with urlopen(request, timeout=timeout) as response:
        payload = json.loads(response.read())
    if payload.get("error"):
        raise RuntimeError(f"RPC {method} failed: {payload['error'].get('message', payload['error'])}")
    if "result" not in payload:
        raise RuntimeError(f"RPC {method} returned no result")
    return payload["result"]


def block_transaction_count(block):
    transactions = block.get("transactions")
    if not isinstance(transactions, list):
        raise RuntimeError("block response did not contain a transaction list")
    return len(transactions)


def block_number(value):
    return int(value, 16)


def block_timestamp(value):
    return int(value, 16)


def load_execution_times(log_path):
    execution_times = {}
    if log_path is None:
        return execution_times
    with Path(log_path).open("r", encoding="utf-8", errors="replace") as log_file:
        for line in log_file:
            match = EXECUTION_TIME_RE.search(line)
            if match:
                value = float(match.group(2))
                execution_times[int(match.group(1))] = value * 1000 if match.group(3) == "s" else value
    return execution_times


def run_cast(cast_bin, args, timeout):
    result = subprocess.run([cast_bin, *args], text=True, capture_output=True, timeout=timeout)
    if result.returncode:
        # Do not echo command arguments: they include the private key.
        message = result.stderr.strip() or result.stdout.strip() or "unknown cast error"
        raise RuntimeError(f"cast command failed: {message}")
    return result.stdout.strip()


def transaction_hash(output):
    hashes = TX_HASH_RE.findall(output)
    if not hashes:
        raise RuntimeError("cast did not return a transaction hash")
    return hashes[-1]


def verify_rpc_url(rpc_url, allow_remote):
    parsed = urlparse(rpc_url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise ValueError("--rpc-url must be an HTTP(S) URL")
    if not allow_remote and parsed.hostname.lower() not in ("localhost", "127.0.0.1", "::1"):
        raise ValueError("remote RPC is disabled; use a loopback URL or pass --allow-remote explicitly")


def wait_for_receipts(rpc_url, tx_hashes, timeout, poll_interval):
    deadline = time.monotonic() + timeout
    receipts = {}
    while len(receipts) < len(tx_hashes):
        for tx_hash in tx_hashes:
            if tx_hash in receipts:
                continue
            receipt = rpc_call(rpc_url, "eth_getTransactionReceipt", [tx_hash], min(timeout, 30))
            if receipt is not None:
                if receipt.get("status") not in (None, "0x1"):
                    raise RuntimeError(f"transaction {tx_hash} failed")
                receipts[tx_hash] = receipt
        if len(receipts) < len(tx_hashes):
            if time.monotonic() >= deadline:
                missing = len(tx_hashes) - len(receipts)
                raise TimeoutError(f"timed out waiting for {missing} transaction receipt(s)")
            time.sleep(poll_interval)
    return receipts


def block_details(rpc_url, number, timeout):
    block = rpc_call(rpc_url, "eth_getBlockByNumber", [hex(number), False], timeout)
    if block is None:
        raise RuntimeError(f"block {number} was not available from the RPC endpoint")
    return block_transaction_count(block), block_timestamp(block["timestamp"])


def wait_for_next_block(rpc_url, previous_number, timeout, poll_interval):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        latest = block_number(rpc_call(rpc_url, "eth_blockNumber", [], min(timeout, 30)))
        if latest > previous_number:
            return list(range(previous_number + 1, latest + 1))
        time.sleep(poll_interval)
    raise TimeoutError(f"no new block was produced within {timeout} seconds")


def create_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rpc-url", default="http://127.0.0.1:8545", help="local execution JSON-RPC URL")
    parser.add_argument("--private-key", default=None, help="funded test account key (or set EB_PRIVATE_KEY)")
    parser.add_argument("--counts", type=parse_counts, default=list(DEFAULT_COUNTS),
                        help="transaction-count sweep (default: 0,1,5,10,20,50,100)")
    parser.add_argument("--blocks-per-count", type=int, default=3, help="number of matching samples per count")
    parser.add_argument("--max-attempts", type=int, default=5,
                        help="maximum bursts per nonzero count, or empty blocks observed per zero count")
    parser.add_argument("--receipt-timeout", type=int, default=180, help="seconds to wait for each burst to mine")
    parser.add_argument("--block-timeout", type=int, default=120, help="seconds to wait for a new empty block")
    parser.add_argument("--poll-interval", type=float, default=1.0, help="seconds between RPC polls")
    parser.add_argument("--output", type=Path, default=None, help="CSV output path")
    parser.add_argument("--execution-log", type=Path, default=None,
                        help="optional EL log for the RPC node, used to attach block execution times")
    parser.add_argument("--cast-bin", default="cast", help="Foundry cast executable")
    parser.add_argument("--allow-remote", action="store_true", help="allow sending test transactions to a non-loopback RPC")
    return parser


def validate_options(args, private_key):
    verify_rpc_url(args.rpc_url, args.allow_remote)
    needs_transactions = any(count > 0 for count in args.counts)
    if needs_transactions and not private_key:
        raise ValueError("provide --private-key or set EB_PRIVATE_KEY for nonzero transaction counts")
    if private_key and not re.fullmatch(r"(?:0x)?[0-9a-fA-F]{64}", private_key):
        raise ValueError("private key must be 32-byte hexadecimal")
    if args.blocks_per_count < 1 or args.max_attempts < 1:
        raise ValueError("--blocks-per-count and --max-attempts must be positive")
    if args.receipt_timeout < 1 or args.block_timeout < 1 or args.poll_interval <= 0:
        raise ValueError("timeouts must be positive")
    if needs_transactions and not shutil.which(args.cast_bin):
        raise ValueError(f"cast executable not found: {args.cast_bin}")
    if args.execution_log is not None and not args.execution_log.is_file():
        raise ValueError(f"execution log not found: {args.execution_log}")


def write_sample(writer, output_file, target, sample, attempt, number, actual, timestamp, hashes, execution_ms):
    matched = actual == target
    writer.writerow({
        "target_tx_count": target,
        "sample": sample if matched else "",
        "attempt": attempt,
        "block_number": number,
        "actual_tx_count": actual,
        "block_timestamp": timestamp,
        "execution_time_ms": "" if execution_ms is None else execution_ms,
        "transaction_hashes": ";".join(hashes),
        "matched": str(matched).lower(),
    })
    output_file.flush()
    return matched


def run(args):
    private_key = args.private_key or os.environ.get("EB_PRIVATE_KEY", "")
    validate_options(args, private_key)
    cast_bin = shutil.which(args.cast_bin)
    output = args.output or Path(f"empty_block_samples_{datetime.now().strftime('%Y%m%d_%H%M%S')}.csv")
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.exists():
        raise ValueError(f"output file already exists: {output}")

    sender = None
    if any(count > 0 for count in args.counts):
        sender = run_cast(cast_bin, ["wallet", "address", "--private-key", private_key], 30)
        if not re.fullmatch(r"0x[0-9a-fA-F]{40}", sender):
            raise RuntimeError("cast could not derive a valid sender address from the private key")
    rpc_call(args.rpc_url, "eth_chainId", [], 15)

    columns = ("target_tx_count", "sample", "attempt", "block_number", "actual_tx_count",
               "block_timestamp", "execution_time_ms", "transaction_hashes", "matched")
    incomplete_counts = 0
    with output.open("x", newline="", encoding="utf-8") as output_file:
        writer = csv.DictWriter(output_file, fieldnames=columns)
        writer.writeheader()
        output_file.flush()

        for target in args.counts:
            successful = 0
            seen_blocks = set()
            if target == 0:
                head = block_number(rpc_call(args.rpc_url, "eth_blockNumber", [], 15))
                max_observations = args.blocks_per_count * args.max_attempts
                for attempt in range(1, max_observations + 1):
                    for number in wait_for_next_block(args.rpc_url, head, args.block_timeout, args.poll_interval):
                        head = number
                        if number in seen_blocks:
                            continue
                        seen_blocks.add(number)
                        actual, timestamp = block_details(args.rpc_url, number, 15)
                        execution_ms = load_execution_times(args.execution_log).get(number)
                        if write_sample(writer, output_file, target, successful + 1, attempt,
                                        number, actual, timestamp, [], execution_ms):
                            successful += 1
                            print(f"0 tx: matched empty block {number} ({successful}/{args.blocks_per_count})")
                        else:
                            print(f"0 tx: block {number} contained {actual} transaction(s)")
                        if successful >= args.blocks_per_count:
                            break
                    if successful >= args.blocks_per_count:
                        break
            else:
                for attempt in range(1, args.max_attempts + 1):
                    nonce = block_number(rpc_call(args.rpc_url, "eth_getTransactionCount", [sender, "pending"], 15))
                    tx_hashes = []
                    for offset in range(target):
                        output_text = run_cast(cast_bin, [
                            "send", sender, "--value", "0", "--gas-limit", "21000",
                            "--nonce", str(nonce + offset), "--async", "--rpc-url", args.rpc_url,
                            "--private-key", private_key,
                        ], args.receipt_timeout)
                        tx_hashes.append(transaction_hash(output_text))

                    receipts = wait_for_receipts(args.rpc_url, tx_hashes, args.receipt_timeout, args.poll_interval)
                    hashes_by_block = defaultdict(list)
                    for tx_hash, receipt in receipts.items():
                        hashes_by_block[block_number(receipt["blockNumber"])].append(tx_hash)

                    for number, included_hashes in sorted(hashes_by_block.items()):
                        if number in seen_blocks:
                            continue
                        seen_blocks.add(number)
                        actual, timestamp = block_details(args.rpc_url, number, 15)
                        execution_ms = load_execution_times(args.execution_log).get(number)
                        matched = actual == target
                        sample = successful + 1 if matched else ""
                        write_sample(writer, output_file, target, sample, attempt,
                                     number, actual, timestamp, included_hashes, execution_ms)
                        if matched:
                            successful += 1
                            print(f"{target} tx: matched block {number} ({successful}/{args.blocks_per_count})")
                        else:
                            print(f"{target} tx: burst landed in block {number} with {actual} total transaction(s)")
                        if successful >= args.blocks_per_count:
                            break
                    if successful >= args.blocks_per_count:
                        break

            if successful < args.blocks_per_count:
                incomplete_counts += 1
                print(f"Warning: only {successful}/{args.blocks_per_count} matching samples for {target} tx.")

    print(f"Block samples written to {output}")
    if incomplete_counts:
        print(f"Could not collect the requested number of exact samples for {incomplete_counts} count(s).", file=sys.stderr)
        return 2
    return 0


def main():
    parser = create_parser()
    args = parser.parse_args()
    try:
        return run(args)
    except (OSError, RuntimeError, TimeoutError, ValueError, subprocess.SubprocessError) as exc:
        print(f"Error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())