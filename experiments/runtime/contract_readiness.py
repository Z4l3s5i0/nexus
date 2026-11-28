#!/usr/bin/env python3
"""Check shared runtime contract visibility across execution nodes."""

import time
import sys
from pathlib import Path

EMPTY_BLOCK_DIR = Path(__file__).resolve().parents[1] / "empty-block"
sys.path.insert(0, str(EMPTY_BLOCK_DIR))

from block_collection import rpc_call


def build_node_types(linux_nodes, geth_nodes, reth_nodes, wasix_nodes):
    return (
        ["native"] * linux_nodes
        + ["geth"] * geth_nodes
        + ["reth"] * reth_nodes
        + ["wasix"] * wasix_nodes
    )


def deployment_candidate_indices(node_types):
    """Prefer one deployment attempt per node type before retrying peers."""
    candidates = []
    seen_types = set()
    for index, node_type in enumerate(node_types):
        if node_type not in seen_types:
            candidates.append(index)
            seen_types.add(node_type)
    candidates.extend(index for index in range(len(node_types)) if index not in candidates)
    return candidates


def _quantity(value):
    if not isinstance(value, str) or not value.startswith("0x"):
        raise ValueError("RPC returned an invalid block number")
    return int(value, 16)


def has_contract_code(rpc_url, contract_address, deployment_block=None, recent_blocks=4):
    """Check the head and recent historical states for deployed bytecode."""
    try:
        latest = _quantity(rpc_call(rpc_url, "eth_blockNumber", [], 10))
    except (OSError, RuntimeError, TypeError, ValueError):
        return False
    if deployment_block is not None and latest < deployment_block:
        return False

    lower_bound = max(0, latest - recent_blocks)
    if deployment_block is not None:
        lower_bound = max(lower_bound, deployment_block)
    block_tags = ["latest"] + [hex(number) for number in range(latest - 1, lower_bound - 1, -1)]
    if deployment_block is not None and deployment_block < lower_bound:
        block_tags.append(hex(deployment_block))

    for block_tag in block_tags:
        try:
            code = rpc_call(rpc_url, "eth_getCode", [contract_address, block_tag], 10)
        except (OSError, RuntimeError, TypeError, ValueError):
            continue
        if isinstance(code, str) and code not in ("", "0x", "0X"):
            return True
    return False


def wait_for_contract_visibility(
    nodes,
    deployment_rpc_url,
    contract_address,
    deployment_block,
    blocks_to_wait=4,
    timeout=240,
    poll_interval=1,
):
    """Wait for all nodes to expose the contract, up to four blocks after deployment."""
    if not nodes:
        raise ValueError("at least one execution node is required")
    if blocks_to_wait < 1 or timeout < 0 or poll_interval < 0:
        raise ValueError("block wait must be positive and time intervals nonnegative")

    target_block = deployment_block + blocks_to_wait
    deadline = time.monotonic() + timeout
    visibility = {}
    while True:
        visibility = {
            node["index"]: has_contract_code(
                node["rpc_url"], contract_address, deployment_block
            )
            for node in nodes
        }
        if all(visibility.values()):
            return True, visibility

        try:
            current_block = _quantity(
                rpc_call(deployment_rpc_url, "eth_blockNumber", [], 10)
            )
        except (OSError, RuntimeError, TypeError, ValueError):
            current_block = None
        if current_block is not None and current_block >= target_block:
            return False, visibility

        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError(
                f"deployment node did not advance to block {target_block} before the visibility timeout"
            )
        time.sleep(min(poll_interval, remaining))