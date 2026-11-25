#!/usr/bin/env python3
"""Run controlled transaction-count sweeps and record the actual included blocks."""

import argparse
import csv
import json
import os
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlparse

from block_collection import (
    block_details,
    block_number,
    receipts_by_block,
    rpc_call,
    wait_for_next_block,
    wait_for_receipts,
)
from transaction_submission import run_cast, submit_burst, submit_contract_deployment


DEFAULT_COUNTS = (0, 1, 5, 10, 20, 50, 100)
CAST_IMAGE = "ghcr.io/foundry-rs/foundry:v1.7.1"
BYTECODE_FILE = Path(__file__).resolve().parents[2] / "diablo_workloads" / "bytecode.csv"
WORKLOAD_CALLS = {
    "counter": "Add()",
    "dota": "update()",
    "uber": "checkDistance()",
}
INCLUSION_COLUMNS = (
    "experiment_id", "client", "client_variant", "node_id", "transaction_type", "workload",
    "target_tx_count", "attempt", "tx_hash", "sender", "to_address", "submitted_at_utc",
    "included_at_utc", "inclusion_latency_ms", "block_number", "block_timestamp", "gas_used", "status",
)


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


def verify_rpc_url(rpc_url, allow_remote):
    parsed = urlparse(rpc_url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise ValueError("--rpc-url must be an HTTP(S) URL")
    if not allow_remote and parsed.hostname.lower() not in ("localhost", "127.0.0.1", "::1"):
        raise ValueError("remote RPC is disabled; pass --allow-remote explicitly to use it")


def create_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rpc-url", default="http://127.0.0.1:8545", help="execution JSON-RPC endpoint")
    parser.add_argument("--cast-rpc-url", default="http://el-node-0:8545", help="RPC endpoint from the Cast container network")
    parser.add_argument("--cast-network", default="", help="Docker network for the bundled Foundry Cast client")
    parser.add_argument("--cast-image", default=CAST_IMAGE)
    parser.add_argument("--cast-bin", default="cast", help="host Foundry Cast executable if installed")
    parser.add_argument("--private-key", default=None, help="funded test account key (or set EB_PRIVATE_KEY)")
    parser.add_argument("--counts", type=parse_counts, default=list(DEFAULT_COUNTS))
    parser.add_argument("--blocks-per-count", type=int, default=3)
    parser.add_argument("--max-attempts", type=int, default=5)
    parser.add_argument("--receipt-timeout", type=int, default=180)
    parser.add_argument("--block-timeout", type=int, default=120)
    parser.add_argument("--poll-interval", type=float, default=1.0)
    parser.add_argument("--chain-id", type=int, default=23456)
    parser.add_argument("--experiment-id", default="empty-block")
    parser.add_argument("--client", default="wasix-eth")
    parser.add_argument("--client-variant", default="unknown")
    parser.add_argument("--node-id", default="el-node-0")
    parser.add_argument("--workload", choices=("none", *WORKLOAD_CALLS), default="none")
    parser.add_argument("--output", type=Path, default=Path("block_samples.csv"))
    parser.add_argument("--inclusion-output", type=Path, default=None)
    parser.add_argument("--allow-remote", action="store_true")
    parser.add_argument("--validate-only", action="store_true")
    return parser


def validate_options(args, private_key):
    verify_rpc_url(args.rpc_url, args.allow_remote)
    needs_signer = any(count > 0 for count in args.counts) or args.workload != "none"
    if needs_signer and not private_key:
        raise ValueError("provide --private-key or set EB_PRIVATE_KEY for transactions or contract deployment")
    if private_key and not re.fullmatch(r"(?:0x)?[0-9a-fA-F]{64}", private_key):
        raise ValueError("private key must be 32-byte hexadecimal")
    if args.blocks_per_count < 1 or args.max_attempts < 1:
        raise ValueError("--blocks-per-count and --max-attempts must be positive")
    if args.receipt_timeout < 1 or args.block_timeout < 1 or args.poll_interval <= 0:
        raise ValueError("timeouts must be positive")
    if needs_signer and not shutil.which(args.cast_bin) and not shutil.which("docker"):
        raise ValueError("Foundry Cast is unavailable; install cast or Docker")
    if args.chain_id < 1:
        raise ValueError("--chain-id must be positive")


def write_sample(writer, output_file, args, target, sample, attempt, number, actual, timestamp, hashes, submitted_hashes):
    matched = actual == target
    writer.writerow(
        {
            "experiment_id": args.experiment_id,
            "network": str(args.chain_id),
            "client": args.client,
            "client_variant": args.client_variant,
            "node_id": args.node_id,
            "target_tx_count": target,
            "sample": sample if matched else "",
            "attempt": attempt,
            "block_number": number,
            "actual_tx_count": actual,
            "block_timestamp": timestamp,
            "execution_time_ms": "",
            "transaction_hashes": ";".join(hashes),
            "submitted_transaction_hashes": ";".join(submitted_hashes),
            "submitted_tx_count": len(submitted_hashes),
            "included_test_tx_count": len(hashes),
            "matched": str(matched).lower(),
        }
    )
    output_file.flush()
    return matched


def load_workload_bytecode(workload):
    if workload not in WORKLOAD_CALLS:
        raise ValueError(f"unsupported contract workload: {workload}")
    bytecode_by_file = {}
    with BYTECODE_FILE.open(newline="", encoding="utf-8") as source:
        for row in csv.reader(source):
            if len(row) < 2:
                continue
            filename, bytecode = row[0].strip(), row[1].strip()
            if filename and bytecode:
                bytecode_by_file[filename] = bytecode
    bytecode = bytecode_by_file.get(f"{workload}.sol", "")
    if not re.fullmatch(r"(?:0x)?[0-9a-fA-F]+", bytecode) or len(bytecode.removeprefix("0x")) % 2:
        raise ValueError(f"valid deployment bytecode for {workload}.sol was not found in {BYTECODE_FILE}")
    return bytecode


def write_inclusion_sample(writer, output_file, args, transaction_type, tx_hash, receipt, submitted_at,
                           observed_at, sender, to_address, target, attempt, block_timestamp_cache=None):
    block_number_value = receipt.get("blockNumber")
    block_number_int = block_number(block_number_value) if block_number_value else None
    block_timestamp = ""
    if block_number_int is not None:
        if block_timestamp_cache is None:
            block_timestamp_cache = {}
        if block_number_int not in block_timestamp_cache:
            try:
                block = rpc_call(args.rpc_url, "eth_getBlockByNumber", [block_number_value, False], 15)
                block_timestamp_cache[block_number_int] = int(block["timestamp"], 16) if block else ""
            except (OSError, RuntimeError, TimeoutError, KeyError, TypeError, ValueError):
                block_timestamp_cache[block_number_int] = ""
        block_timestamp = block_timestamp_cache[block_number_int]
    latency_ms = max(0.0, (observed_at["monotonic"] - submitted_at["monotonic"]) * 1000)
    writer.writerow(
        {
            "experiment_id": args.experiment_id,
            "client": args.client,
            "client_variant": args.client_variant,
            "node_id": args.node_id,
            "transaction_type": transaction_type,
            "workload": args.workload if args.workload != "none" else "",
            "target_tx_count": target if target is not None else "",
            "attempt": attempt if attempt is not None else "",
            "tx_hash": tx_hash,
            "sender": sender,
            "to_address": to_address or "",
            "submitted_at_utc": datetime.fromtimestamp(submitted_at["epoch"], timezone.utc).isoformat(),
            "included_at_utc": datetime.fromtimestamp(observed_at["epoch"], timezone.utc).isoformat(),
            "inclusion_latency_ms": f"{latency_ms:.3f}",
            "block_number": block_number_int if block_number_int is not None else "",
            "block_timestamp": block_timestamp,
            "gas_used": int(receipt.get("gasUsed", "0x0"), 16),
            "status": receipt.get("status", ""),
        }
    )
    output_file.flush()
    return latency_ms


def run(args):
    private_key = args.private_key or os.environ.get("EB_PRIVATE_KEY", "")
    validate_options(args, private_key)
    if args.validate_only:
        return 0

    output = args.output
    output.parent.mkdir(parents=True, exist_ok=True)
    inclusion_output = args.inclusion_output or output.parent / "transaction_inclusion_samples.csv"
    if output.exists():
        raise ValueError(f"output file already exists: {output}")
    if inclusion_output.exists():
        raise ValueError(f"output file already exists: {inclusion_output}")

    cast_bin = shutil.which(args.cast_bin)
    cast_rpc_url = args.rpc_url if cast_bin else args.cast_rpc_url
    sender = None
    if any(count > 0 for count in args.counts) or args.workload != "none":
        sender = run_cast(
            cast_bin,
            ["wallet", "address", "--private-key", private_key],
            30,
            args.cast_network,
            args.cast_image,
        )
        if not re.fullmatch(r"0x[0-9a-fA-F]{40}", sender):
            raise RuntimeError("cast could not derive a valid sender address from the private key")

    actual_chain_id = int(rpc_call(args.rpc_url, "eth_chainId", [], 15), 16)
    if actual_chain_id != args.chain_id:
        raise RuntimeError(f"RPC chain ID {actual_chain_id} does not match configured chain ID {args.chain_id}")

    columns = (
        "experiment_id", "network", "client", "client_variant", "node_id", "target_tx_count", "sample", "attempt",
        "block_number", "actual_tx_count", "block_timestamp", "execution_time_ms",
        "transaction_hashes", "submitted_transaction_hashes", "submitted_tx_count",
        "included_test_tx_count", "matched",
    )
    incomplete_counts = 0
    with output.open("x", newline="", encoding="utf-8") as output_file, inclusion_output.open(
        "x", newline="", encoding="utf-8"
    ) as inclusion_file:
        writer = csv.DictWriter(output_file, fieldnames=columns)
        inclusion_writer = csv.DictWriter(inclusion_file, fieldnames=INCLUSION_COLUMNS)
        writer.writeheader()
        inclusion_writer.writeheader()
        output_file.flush()
        inclusion_file.flush()

        contract_address = None
        block_timestamp_cache = {}
        if args.workload != "none":
            bytecode = load_workload_bytecode(args.workload)
            nonce = block_number(rpc_call(args.rpc_url, "eth_getTransactionCount", [sender, "pending"], 15))
            gas_price = int(rpc_call(args.rpc_url, "eth_gasPrice", [], 15), 16)
            deployment_hash, submitted_at = submit_contract_deployment(
                cast_bin,
                args.cast_network,
                args.cast_image,
                cast_rpc_url,
                private_key,
                nonce,
                bytecode,
                args.receipt_timeout,
                gas_price,
            )
            deployment_receipts, deployment_observed = wait_for_receipts(
                args.rpc_url,
                [deployment_hash],
                args.receipt_timeout,
                args.poll_interval,
                include_observation_times=True,
            )
            deployment_receipt = deployment_receipts[deployment_hash]
            contract_address = deployment_receipt.get("contractAddress")
            deployment_latency = write_inclusion_sample(
                inclusion_writer,
                inclusion_file,
                args,
                "contract_deployment",
                deployment_hash,
                deployment_receipt,
                submitted_at,
                deployment_observed[deployment_hash],
                sender,
                contract_address,
                None,
                None,
                block_timestamp_cache,
            )
            if not isinstance(contract_address, str) or not re.fullmatch(r"0x[0-9a-fA-F]{40}", contract_address):
                raise RuntimeError(f"deployment transaction {deployment_hash} did not return a contract address")
            deployed_code = rpc_call(args.rpc_url, "eth_getCode", [contract_address, "latest"], 15)
            if not deployed_code or deployed_code == "0x":
                raise RuntimeError(f"contract deployment at {contract_address} was not confirmed by eth_getCode")
            deployment_metadata = {
                "workload": args.workload,
                "bytecode_file": str(BYTECODE_FILE),
                "contract_address": contract_address,
                "transaction_hash": deployment_hash,
                "block_number": block_number(deployment_receipt["blockNumber"]),
                "inclusion_latency_ms": round(deployment_latency, 3),
                "confirmed": True,
            }
            (output.parent / "contract_deployment.json").write_text(
                json.dumps(deployment_metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8"
            )
            metadata_path = output.parent / "metadata.json"
            if metadata_path.is_file():
                metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
                metadata["contract_deployment"] = deployment_metadata
                metadata["workload"] = args.workload
                metadata_path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            print(f"Deployed {args.workload} contract at {contract_address} in block {deployment_metadata['block_number']}")

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
                        if write_sample(writer, output_file, args, target, successful + 1, attempt,
                                        number, actual, timestamp, [], []):
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
                    nonce = block_number(
                        rpc_call(args.rpc_url, "eth_getTransactionCount", [sender, "pending"], 15)
                    )
                    gas_price = int(rpc_call(args.rpc_url, "eth_gasPrice", [], 15), 16)
                    submitted = submit_burst(
                        cast_bin,
                        args.cast_network,
                        args.cast_image,
                        cast_rpc_url,
                        sender,
                        private_key,
                        nonce,
                        target,
                        args.receipt_timeout,
                        gas_price,
                        to_address=contract_address,
                        function_signature=WORKLOAD_CALLS.get(args.workload),
                        include_timestamps=True,
                    )
                    submitted_hashes = [item["hash"] for item in submitted]
                    receipts, observed_times = wait_for_receipts(
                        args.rpc_url,
                        submitted_hashes,
                        args.receipt_timeout,
                        args.poll_interval,
                        include_observation_times=True,
                        allow_failed_receipts=args.workload != "none",
                    )
                    for item in submitted:
                        write_inclusion_sample(
                            inclusion_writer,
                            inclusion_file,
                            args,
                            "contract_call" if args.workload != "none" else "value_transfer",
                            item["hash"],
                            receipts[item["hash"]],
                            item["submitted_at"],
                            observed_times[item["hash"]],
                            sender,
                            contract_address,
                            target,
                            attempt,
                            block_timestamp_cache,
                        )
                    hashes_by_block = receipts_by_block(receipts)
                    for number, included_hashes in sorted(hashes_by_block.items()):
                        if number in seen_blocks:
                            continue
                        seen_blocks.add(number)
                        actual, timestamp = block_details(args.rpc_url, number, 15)
                        if write_sample(writer, output_file, args, target, successful + 1, attempt,
                                        number, actual, timestamp, included_hashes, submitted_hashes):
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
                print(
                    f"Warning: only {successful}/{args.blocks_per_count} matching samples for {target} tx.",
                    file=sys.stderr,
                )

    print(f"Block samples written to {output}")
    print(f"Transaction inclusion samples written to {inclusion_output}")
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