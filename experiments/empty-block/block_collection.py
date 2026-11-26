import json
import time
from collections import defaultdict
from urllib.request import Request, urlopen


RPC_ID = 0


def rpc_call(rpc_url, method, params, timeout):
    global RPC_ID
    RPC_ID += 1
    body = json.dumps(
        {"jsonrpc": "2.0", "id": RPC_ID, "method": method, "params": params}
    ).encode("utf-8")
    request = Request(rpc_url, data=body, headers={"Content-Type": "application/json"})
    with urlopen(request, timeout=timeout) as response:
        payload = json.loads(response.read())
    if payload.get("error"):
        raise RuntimeError(f"RPC {method} failed: {payload['error'].get('message', payload['error'])}")
    if "result" not in payload:
        raise RuntimeError(f"RPC {method} returned no result")
    return payload["result"]


def block_number(value):
    return int(value, 16)


def block_transaction_count(block):
    transactions = block.get("transactions")
    if not isinstance(transactions, list):
        raise RuntimeError("block response did not contain a transaction list")
    return len(transactions)


def block_details(rpc_url, number, timeout):
    block = rpc_call(rpc_url, "eth_getBlockByNumber", [hex(number), False], timeout)
    if block is None:
        raise RuntimeError(f"block {number} was not available from the RPC endpoint")
    return block_transaction_count(block), int(block["timestamp"], 16)


def wait_for_receipts(
    rpc_url, tx_hashes, timeout, poll_interval, include_observation_times=False, allow_failed_receipts=False
):
    deadline = time.monotonic() + timeout
    receipts = {}
    observed_times = {}
    while len(receipts) < len(tx_hashes):
        for tx_hash in tx_hashes:
            if tx_hash in receipts:
                continue
            try:
                receipt = rpc_call(rpc_url, "eth_getTransactionReceipt", [tx_hash], min(timeout, 30))
            except RuntimeError as exc:
                if "transaction indexing is in progress" not in str(exc).lower():
                    raise
                continue
            if receipt is not None:
                if receipt.get("status") not in (None, "0x1") and not allow_failed_receipts:
                    raise RuntimeError(f"transaction {tx_hash} failed")
                receipts[tx_hash] = receipt
                observed_times[tx_hash] = {"monotonic": time.monotonic(), "epoch": time.time()}
        if len(receipts) < len(tx_hashes):
            if time.monotonic() >= deadline:
                missing = len(tx_hashes) - len(receipts)
                raise TimeoutError(f"timed out waiting for {missing} transaction receipt(s)")
            time.sleep(poll_interval)
    if include_observation_times:
        return receipts, observed_times
    return receipts


def wait_for_next_block(rpc_url, previous_number, timeout, poll_interval):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        latest = block_number(rpc_call(rpc_url, "eth_blockNumber", [], min(timeout, 30)))
        if latest > previous_number:
            return list(range(previous_number + 1, latest + 1))
        time.sleep(poll_interval)
    raise TimeoutError(f"no new block was produced within {timeout} seconds")


def receipts_by_block(receipts):
    grouped = defaultdict(list)
    for tx_hash, receipt in receipts.items():
        grouped[block_number(receipt["blockNumber"])].append(tx_hash)
    return {number: sorted(hashes) for number, hashes in grouped.items()}