#!/usr/bin/env python3
"""Run the runtime transaction workload with Foundry Cast."""

import argparse
import json
import os
import re
import sys
import time
from pathlib import Path


EMPTY_BLOCK_DIR = Path(__file__).resolve().parents[1] / "empty-block"
sys.path.insert(0, str(EMPTY_BLOCK_DIR))

from block_collection import rpc_call, wait_for_receipts
from run_experiment import WORKLOAD_CALLS, load_workload_bytecode
from transaction_submission import (
    run_cast,
    run_cast_batch,
    submit_contract_deployment,
    submit_transaction_schedule,
    submit_transactions,
)


CAST_IMAGE = "ghcr.io/foundry-rs/foundry:v1.7.1"
START_DELAY_SECONDS = 30
FUNDING_AMOUNT = "100ether"
FUNDING_RECEIPT_TIMEOUT = 180
CONTRACT_CALL_GAS_LIMIT = 250_000
VALUE_TRANSFER_TO = "0x000000000000000000000000000000000000dead"
MNEMONIC_KEY_INDEX_OFFSET = 10_000


def create_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--node-index", type=int, required=True)
    parser.add_argument("--chain-id", type=int, required=True)
    parser.add_argument("--network", required=True, help="Docker network for the bundled Cast client")
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--window-file", type=Path, required=True)
    parser.add_argument("--scenario", required=True)
    parser.add_argument("--tps", type=int, required=True)
    parser.add_argument("--duration", type=int, required=True)
    parser.add_argument("--accounts", type=int, required=True)
    parser.add_argument("--private-key", required=True)
    parser.add_argument("--cast-image", default=CAST_IMAGE)
    parser.add_argument("--start-delay", type=int, default=START_DELAY_SECONDS)
    return parser


def scenario_name(scenario):
    name = scenario.removeprefix("exp/")
    if name != "value-transfer" and name not in WORKLOAD_CALLS:
        raise ValueError(f"unsupported Cast runtime scenario: {scenario}")
    return name


def wallet_private_key(output):
    matches = re.findall(r"(?:0x)?([0-9a-fA-F]{64})(?![0-9a-fA-F])", output)
    if not matches:
        raise RuntimeError("Cast did not derive a valid runtime account private key")
    return matches[-1]


def wallet_address(output):
    matches = re.findall(r"0x[0-9a-fA-F]{40}(?![0-9a-fA-F])", output)
    if not matches:
        raise RuntimeError("Cast did not derive a valid runtime account address")
    return matches[-1]


def derive_accounts(node_index, count, mnemonic, network, cast_image):
    first_index = MNEMONIC_KEY_INDEX_OFFSET + node_index * count
    key_commands = [
        ["wallet", "private-key", mnemonic, str(first_index + index)]
        for index in range(count)
    ]
    private_keys = [
        wallet_private_key(output)
        for output in run_cast_batch(None, network, cast_image, key_commands, 60)
    ]
    address_commands = [["wallet", "address", "--private-key", key] for key in private_keys]
    addresses = [
        wallet_address(output)
        for output in run_cast_batch(None, network, cast_image, address_commands, 60)
    ]
    return list(zip(private_keys, addresses))


def rpc_quantity(rpc_url, method, params):
    value = rpc_call(rpc_url, method, params, 30)
    try:
        return int(value, 16)
    except (TypeError, ValueError) as exc:
        raise RuntimeError(f"RPC {method} returned an invalid quantity") from exc


def cast_sender(private_key, network, cast_image, rpc_url):
    output = run_cast(
        None,
        ["wallet", "address", "--private-key", private_key],
        30,
        network,
        cast_image,
    )
    return wallet_address(output)


def prepare_accounts(args, rpc_url, cast_rpc_url, mnemonic, scenario):
    sender = cast_sender(args.private_key, args.network, args.cast_image, cast_rpc_url)
    accounts = derive_accounts(args.node_index, args.accounts, mnemonic, args.network, args.cast_image)
    if any(address.lower() == sender.lower() for _, address in accounts):
        raise RuntimeError("derived workload accounts overlap the funded runtime sender")

    chain_id = rpc_quantity(rpc_url, "eth_chainId", [])
    if chain_id != args.chain_id:
        raise RuntimeError(f"RPC chain ID {chain_id} does not match configured chain ID {args.chain_id}")

    gas_price = rpc_quantity(rpc_url, "eth_gasPrice", [])
    funding_nonce = rpc_quantity(rpc_url, "eth_getTransactionCount", [sender, "pending"])
    funding_transactions = [
        {
            "sender": sender,
            "private_key": args.private_key,
            "to_address": address,
            "value": FUNDING_AMOUNT,
            "gas_limit": 21_000,
            "gas_price": gas_price,
            "nonce": funding_nonce + index,
        }
        for index, (_, address) in enumerate(accounts)
    ]
    funding_hashes = submit_transactions(
        None, args.network, args.cast_image, cast_rpc_url, funding_transactions, 300
    )
    wait_for_receipts(rpc_url, funding_hashes, FUNDING_RECEIPT_TIMEOUT, 0.25)
    print(f"Cast funded {len(accounts)} workload account(s).", flush=True)

    contract_address = None
    if scenario != "value-transfer":
        nonce = rpc_quantity(rpc_url, "eth_getTransactionCount", [sender, "pending"])
        bytecode = load_workload_bytecode(scenario)
        deployment_hash, _ = submit_contract_deployment(
            None,
            args.network,
            args.cast_image,
            cast_rpc_url,
            args.private_key,
            nonce,
            bytecode,
            FUNDING_RECEIPT_TIMEOUT,
            gas_price,
        )
        deployment_receipt = wait_for_receipts(
            rpc_url, [deployment_hash], FUNDING_RECEIPT_TIMEOUT, 0.25
        )[deployment_hash]
        contract_address = deployment_receipt.get("contractAddress")
        if not isinstance(contract_address, str) or not re.fullmatch(r"0x[0-9a-fA-F]{40}", contract_address):
            raise RuntimeError(f"Cast contract deployment {deployment_hash} did not return a valid address")
        deployed_code = rpc_call(rpc_url, "eth_getCode", [contract_address, "latest"], 30)
        if not deployed_code or deployed_code == "0x":
            raise RuntimeError(f"contract deployment at {contract_address} was not confirmed by eth_getCode")
        print(f"Cast deployed {scenario} contract at {contract_address}.", flush=True)

    return accounts, contract_address


def runtime_transactions(args, scenario, accounts, contract_address, rpc_url):
    account_nonces = [
        rpc_quantity(rpc_url, "eth_getTransactionCount", [address, "pending"])
        for _, address in accounts
    ]
    gas_price = rpc_quantity(rpc_url, "eth_gasPrice", [])
    transactions = []
    for index in range(args.tps * args.duration):
        account_index = index % len(accounts)
        private_key, _ = accounts[account_index]
        if scenario == "value-transfer":
            destination = VALUE_TRANSFER_TO
            signature = None
            gas_limit = 21_000
            value = "1"
        else:
            destination = contract_address
            signature = WORKLOAD_CALLS[scenario]
            gas_limit = CONTRACT_CALL_GAS_LIMIT
            value = "0"
        transactions.append(
            {
                "private_key": private_key,
                "to_address": destination,
                "function_signature": signature,
                "gas_limit": gas_limit,
                "gas_price": gas_price,
                "nonce": account_nonces[account_index],
                "value": value,
            }
        )
        account_nonces[account_index] += 1
    return transactions


def write_workload_window(args, start_ms, end_ms, exit_code):
    args.window_file.parent.mkdir(parents=True, exist_ok=True)
    window = {
        "experiment_id": args.run_id,
        "node_index": args.node_index,
        "start_ms": start_ms,
        "end_ms": end_ms,
        "exit_code": exit_code,
    }
    args.window_file.write_text(json.dumps(window, sort_keys=True) + "\n", encoding="utf-8")


def run(args):
    if args.node_index < 0 or args.chain_id < 1:
        raise ValueError("node index must be nonnegative and chain ID must be positive")
    if min(args.tps, args.duration, args.accounts) < 1 or args.start_delay < 0:
        raise ValueError("TPS, duration, and account count must be positive; start delay must be nonnegative")
    if not re.fullmatch(r"[0-9a-fA-F]{64}", args.private_key):
        raise ValueError("runtime sender private key must contain 32-byte hexadecimal data")
    mnemonic = os.environ.get("EXPERIMENT_WALLET_MNEMONIC", "")
    if not mnemonic:
        raise ValueError("EXPERIMENT_WALLET_MNEMONIC is required to derive workload accounts")

    scenario = scenario_name(args.scenario)
    host_rpc_url = f"http://127.0.0.1:{8545 + args.node_index}"
    cast_rpc_url = f"http://el-node-{args.node_index}:{8545 + args.node_index}"
    accounts, contract_address = prepare_accounts(
        args, host_rpc_url, cast_rpc_url, mnemonic, scenario
    )
    print(f"Cast setup complete; waiting {args.start_delay} seconds before transaction spam.", flush=True)
    time.sleep(args.start_delay)

    start_ms = time.time_ns() // 1_000_000
    exit_code = 0
    try:
        transactions = runtime_transactions(args, scenario, accounts, contract_address, host_rpc_url)
        print(
            f"Cast spam starting: {len(transactions)} transactions at {args.tps} TPS for {args.duration} seconds.",
            flush=True,
        )
        hashes = submit_transaction_schedule(
            args.network,
            args.cast_image,
            cast_rpc_url,
            transactions,
            args.tps,
            args.duration,
            args.duration + 180,
        )
        print(f"Cast spam submitted {len(hashes)} transaction(s).", flush=True)
    except Exception as exc:
        print(f"Cast workload failed: {exc}", file=sys.stderr, flush=True)
        exit_code = 1
    end_ms = time.time_ns() // 1_000_000
    write_workload_window(args, start_ms, end_ms, exit_code)
    return exit_code


def main():
    args = create_parser().parse_args()
    try:
        return run(args)
    except Exception as exc:
        print(f"Cast setup failed: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())