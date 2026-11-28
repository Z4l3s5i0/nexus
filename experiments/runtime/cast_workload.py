#!/usr/bin/env python3
"""Run the runtime transaction workload with Foundry Cast."""

import argparse
import json
import os
import re
import sys
import time
from pathlib import Path
from urllib.error import URLError


EMPTY_BLOCK_DIR = Path(__file__).resolve().parents[1] / "empty-block"
sys.path.insert(0, str(EMPTY_BLOCK_DIR))

from block_collection import rpc_call, wait_for_receipts
from run_experiment import WORKLOAD_CALLS, load_workload_bytecode
from transaction_submission import (
    CONTRACT_DEPLOYMENT_GAS_LIMIT,
    run_cast,
    run_cast_batch,
    submit_contract_deployment,
    submit_transaction_schedule,
    submit_transactions,
)
try:
    from .contract_readiness import has_contract_code
except ImportError:
    from contract_readiness import has_contract_code


CAST_IMAGE = "ghcr.io/foundry-rs/foundry:v1.7.1"
START_DELAY_SECONDS = 30
FUNDING_AMOUNT = "100ether"
FUNDING_RECEIPT_TIMEOUT = 180
RPC_TIMEOUT_SECONDS = 30
DEPLOYMENT_ESTIMATE_TIMEOUT_SECONDS = 120
CONTRACT_CALL_GAS_LIMIT = 250_000
VALUE_TRANSFER_TO = "0x000000000000000000000000000000000000dead"
MNEMONIC_KEY_INDEX_OFFSET = 10_000


def create_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--node-index", type=int, required=True)
    parser.add_argument("--chain-id", type=int, required=True)
    parser.add_argument("--network", required=True, help="Docker network for the bundled Cast client")
    parser.add_argument("--run-id")
    parser.add_argument("--window-file", type=Path)
    parser.add_argument("--scenario", default="exp/uber")
    parser.add_argument("--tps", type=int, default=4)
    parser.add_argument("--duration", type=int, default=300)
    parser.add_argument("--accounts", type=int, default=4)
    parser.add_argument("--private-key", required=True)
    parser.add_argument("--cast-image", default=CAST_IMAGE)
    parser.add_argument("--start-delay", type=int, default=START_DELAY_SECONDS)
    parser.add_argument("--deploy-contract", action="store_true")
    parser.add_argument("--contract-address")
    parser.add_argument("--contract-address-file", type=Path)
    parser.add_argument("--deployment-info-file", type=Path)
    parser.add_argument("--submission-manifest-file", type=Path)
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


def rpc_call_with_context(rpc_url, method, params, timeout):
    try:
        return rpc_call(rpc_url, method, params, timeout)
    except TimeoutError as exc:
        raise TimeoutError(
            f"RPC {method} request to {rpc_url} timed out after {timeout} seconds"
        ) from exc
    except URLError as exc:
        if not isinstance(exc.reason, TimeoutError):
            raise
        raise TimeoutError(
            f"RPC {method} request to {rpc_url} timed out after {timeout} seconds"
        ) from exc


def rpc_quantity(rpc_url, method, params, timeout=RPC_TIMEOUT_SECONDS):
    value = rpc_call_with_context(rpc_url, method, params, timeout)
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


def deploy_workload_contract(args, rpc_url, cast_rpc_url, scenario):
    if scenario == "value-transfer":
        raise ValueError("value-transfer workloads do not use a contract")

    sender = cast_sender(args.private_key, args.network, args.cast_image, cast_rpc_url)
    chain_id = rpc_quantity(rpc_url, "eth_chainId", [])
    if chain_id != args.chain_id:
        raise RuntimeError(f"RPC chain ID {chain_id} does not match configured chain ID {args.chain_id}")

    nonce = rpc_quantity(rpc_url, "eth_getTransactionCount", [sender, "pending"])
    gas_price = rpc_quantity(rpc_url, "eth_gasPrice", [])
    bytecode = load_workload_bytecode(scenario)
    if scenario == "uber_v2":
        deployment_gas_limit = CONTRACT_DEPLOYMENT_GAS_LIMIT
    else:
        deployment_gas_limit = rpc_quantity(
            rpc_url,
            "eth_estimateGas",
            [{"from": sender, "data": "0x" + bytecode.removeprefix("0x")}],
            timeout=DEPLOYMENT_ESTIMATE_TIMEOUT_SECONDS,
        )
    if deployment_gas_limit < 1:
        raise RuntimeError("RPC eth_estimateGas returned an invalid deployment gas estimate")
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
        deployment_gas_limit,
    )
    deployment_receipt = wait_for_receipts(
        rpc_url, [deployment_hash], FUNDING_RECEIPT_TIMEOUT, 0.25
    )[deployment_hash]
    contract_address = deployment_receipt.get("contractAddress")
    if not isinstance(contract_address, str) or not re.fullmatch(r"0x[0-9a-fA-F]{40}", contract_address):
        raise RuntimeError(f"Cast contract deployment {deployment_hash} did not return a valid address")
    deployed_code = rpc_call_with_context(
        rpc_url, "eth_getCode", [contract_address, "latest"], RPC_TIMEOUT_SECONDS
    )
    if not deployed_code or deployed_code == "0x":
        raise RuntimeError(f"contract deployment at {contract_address} was not confirmed by eth_getCode")
    print(f"Cast deployed {scenario} contract at {contract_address}.", flush=True)
    try:
        deployment_block = int(deployment_receipt["blockNumber"], 16)
    except (KeyError, TypeError, ValueError) as exc:
        raise RuntimeError(f"Cast contract deployment {deployment_hash} did not return a valid block number") from exc
    return contract_address, deployment_block


def prepare_accounts(args, rpc_url, cast_rpc_url, mnemonic, scenario, contract_address=None):
    sender = cast_sender(args.private_key, args.network, args.cast_image, cast_rpc_url)
    accounts = derive_accounts(args.node_index, args.accounts, mnemonic, args.network, args.cast_image)
    if any(address.lower() == sender.lower() for _, address in accounts):
        raise RuntimeError("derived workload accounts overlap the funded runtime sender")

    chain_id = rpc_quantity(rpc_url, "eth_chainId", [])
    if chain_id != args.chain_id:
        raise RuntimeError(f"RPC chain ID {chain_id} does not match configured chain ID {args.chain_id}")

    if scenario == "value-transfer":
        if contract_address is not None:
            raise RuntimeError("value-transfer workloads must not receive a contract address")
    else:
        if not isinstance(contract_address, str) or not re.fullmatch(r"0x[0-9a-fA-F]{40}", contract_address):
            raise RuntimeError("a valid shared contract address is required for contract workloads")
        if not has_contract_code(rpc_url, contract_address):
            raise RuntimeError(f"shared contract at {contract_address} is not available on this node")

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
    if args.deploy_contract:
        if args.contract_address_file is None:
            raise ValueError("--contract-address-file is required when deploying the shared contract")
        contract_address, deployment_block = deploy_workload_contract(
            args, host_rpc_url, cast_rpc_url, scenario
        )
        args.contract_address_file.parent.mkdir(parents=True, exist_ok=True)
        args.contract_address_file.write_text(contract_address + "\n", encoding="utf-8")
        deployment_info_file = getattr(args, "deployment_info_file", None)
        if deployment_info_file is not None:
            deployment_info_file.parent.mkdir(parents=True, exist_ok=True)
            deployment_info_file.write_text(
                json.dumps(
                    {"contract_address": contract_address, "blockNumber": deployment_block},
                    sort_keys=True,
                )
                + "\n",
                encoding="utf-8",
            )
        return 0
    if not args.run_id or args.window_file is None:
        raise ValueError("--run-id and --window-file are required for a workload")

    accounts, contract_address = prepare_accounts(
        args, host_rpc_url, cast_rpc_url, mnemonic, scenario, args.contract_address
    )
    print(f"Cast setup complete; waiting {args.start_delay} seconds before transaction spam.", flush=True)
    time.sleep(args.start_delay)

    start_ms = None
    exit_code = 0
    submitted_transactions = []
    try:
        transactions = runtime_transactions(args, scenario, accounts, contract_address, host_rpc_url)
        print(
            f"Cast spam starting: {len(transactions)} transactions at {args.tps} TPS for {args.duration} seconds.",
            flush=True,
        )
        start_ms = time.time_ns() // 1_000_000
        submitted_transactions = submit_transaction_schedule(
            args.network,
            args.cast_image,
            cast_rpc_url,
            transactions,
            args.tps,
            args.duration,
            args.duration + 180,
            include_timestamps=True,
        )
        print(
            f"Cast spam accepted {len(submitted_transactions)} of {len(transactions)} transaction(s) for submission.",
            flush=True,
        )
    except Exception as exc:
        print(f"Cast workload failed: {exc}", file=sys.stderr, flush=True)
        exit_code = 1
    now_ms = time.time_ns() // 1_000_000
    if start_ms is None:
        start_ms = now_ms
    end_ms = min(now_ms, start_ms + args.duration * 1000)
    if args.submission_manifest_file is not None:
        args.submission_manifest_file.parent.mkdir(parents=True, exist_ok=True)
        args.submission_manifest_file.write_text(
            json.dumps(
                {
                    "node_index": args.node_index,
                    "planned_transactions": args.tps * args.duration,
                    "submitted_transactions": submitted_transactions,
                },
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )
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