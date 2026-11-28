#!/usr/bin/env python3
"""Deploy a runtime workload contract and verify it across execution nodes."""

import argparse
import json
import subprocess
import sys
from pathlib import Path

try:
    from .contract_readiness import (
        build_node_types,
        deployment_candidate_indices,
        wait_for_contract_visibility,
    )
except ImportError:
    from contract_readiness import (
        build_node_types,
        deployment_candidate_indices,
        wait_for_contract_visibility,
    )


def deploy_on_node(args, node_index, address_path, info_path, log_path):
    command = [
        "bash",
        str(args.workload_script),
        "--node-index",
        str(node_index),
        "--chain-id",
        str(args.chain_id),
        "--network",
        args.network,
        "--scenario",
        args.scenario,
        "--accounts",
        str(args.accounts),
        "--deploy-contract",
        "--contract-address-file",
        str(address_path),
        "--deployment-info-file",
        str(info_path),
    ]
    with log_path.open("w", encoding="utf-8") as log:
        result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=False)
    return result.returncode


def deployment_nodes(args):
    node_types = build_node_types(
        args.linux_nodes, args.geth_nodes, args.reth_nodes, args.wasix_nodes
    )
    return [
        {
            "index": index,
            "type": node_type,
            "rpc_url": f"http://127.0.0.1:{8545 + index}",
        }
        for index, node_type in enumerate(node_types)
    ]


def run(args):
    nodes = deployment_nodes(args)
    if not nodes:
        raise ValueError("at least one execution node is required")
    node_types = [node["type"] for node in nodes]
    args.output_address.parent.mkdir(parents=True, exist_ok=True)
    args.output_info.parent.mkdir(parents=True, exist_ok=True)
    args.log_dir.mkdir(parents=True, exist_ok=True)

    for node_index in deployment_candidate_indices(node_types):
        attempt_address = args.log_dir / f"contract_address_node_{node_index}.txt"
        attempt_info = args.log_dir / f"contract_deployment_node_{node_index}.json"
        attempt_log = args.log_dir / f"contract_setup_node_{node_index}.log"
        if deploy_on_node(args, node_index, attempt_address, attempt_info, attempt_log):
            print(
                f"Contract deployment from node {node_index} failed; "
                f"see {attempt_log}.",
                file=sys.stderr,
                flush=True,
            )
            continue

        try:
            contract_address = attempt_address.read_text(encoding="utf-8").strip()
            deployment = json.loads(attempt_info.read_text(encoding="utf-8"))
            deployment_block = int(deployment["blockNumber"])
        except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as exc:
            print(
                f"Contract deployment from node {node_index} returned invalid metadata: {exc}.",
                file=sys.stderr,
                flush=True,
            )
            continue

        source_url = nodes[node_index]["rpc_url"]
        print(
            f"Contract deployed from node {node_index} ({nodes[node_index]['type']}) "
            f"at block {deployment_block}; checking visibility on all {len(nodes)} nodes.",
            flush=True,
        )
        visible, node_visibility = wait_for_contract_visibility(
            nodes,
            source_url,
            contract_address,
            deployment_block,
            blocks_to_wait=4,
            timeout=args.visibility_timeout,
        )
        if visible:
            args.output_address.write_text(contract_address + "\n", encoding="utf-8")
            args.output_info.write_text(
                json.dumps(
                    {
                        "node_index": node_index,
                        "node_type": nodes[node_index]["type"],
                        "contract_address": contract_address,
                        "block_number": deployment_block,
                    },
                    indent=2,
                    sort_keys=True,
                )
                + "\n",
                encoding="utf-8",
            )
            print(f"Shared contract is visible on all nodes at {contract_address}.", flush=True)
            return 0

        missing = [str(index) for index, available in node_visibility.items() if not available]
        print(
            f"Contract at {contract_address} was not visible on node(s) "
            f"{', '.join(missing)} after four blocks; trying another node type or node.",
            file=sys.stderr,
            flush=True,
        )

    raise RuntimeError(
        "could not deploy a shared workload contract visible on every execution node; "
        f"see deployment logs in {args.log_dir}"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workload-script", type=Path, required=True)
    parser.add_argument("--chain-id", type=int, required=True)
    parser.add_argument("--network", required=True)
    parser.add_argument("--scenario", required=True)
    parser.add_argument("--accounts", type=int, required=True)
    parser.add_argument("--linux-nodes", type=int, required=True)
    parser.add_argument("--geth-nodes", type=int, required=True)
    parser.add_argument("--reth-nodes", type=int, required=True)
    parser.add_argument("--wasix-nodes", type=int, required=True)
    parser.add_argument("--output-address", type=Path, required=True)
    parser.add_argument("--output-info", type=Path, required=True)
    parser.add_argument("--log-dir", type=Path, required=True)
    parser.add_argument("--visibility-timeout", type=float, default=240)
    args = parser.parse_args()
    try:
        return run(args)
    except Exception as exc:
        print(f"Contract deployment failed: {exc}", file=sys.stderr, flush=True)
        return 1


if __name__ == "__main__":
    sys.exit(main())