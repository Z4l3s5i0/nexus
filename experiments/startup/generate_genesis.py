#!/usr/bin/env python3
import argparse
import json
import re
from pathlib import Path


DEFAULT_GAS_LIMIT = 0x1FFFFFFFFFFFFF
DEFAULT_BASE_FEE_PER_GAS = 1_000_000_000
DEFAULT_PREMINE_ADDRESS = "f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
DEFAULT_PREMINE_BALANCE = int("200000000000000000000000000000000000000000000000000000000000000", 16)


def genesis(
    chain_id,
    gas_limit=DEFAULT_GAS_LIMIT,
    base_fee_per_gas=DEFAULT_BASE_FEE_PER_GAS,
    terminal_total_difficulty=0,
    premine_address=DEFAULT_PREMINE_ADDRESS,
    premine_balance=DEFAULT_PREMINE_BALANCE,
):
    return {
        "config": {
            "chainId": chain_id,
            "homesteadBlock": 0,
            "eip150Block": 0,
            "eip155Block": 0,
            "eip158Block": 0,
            "byzantiumBlock": 0,
            "constantinopleBlock": 0,
            "petersburgBlock": 0,
            "istanbulBlock": 0,
            "muirGlacierBlock": 0,
            "berlinBlock": 0,
            "londonBlock": 0,
            "terminalTotalDifficulty": hex(terminal_total_difficulty),
            "terminalTotalDifficultyPassed": True,
            "mergeNetsplitBlock": 0,
            "shanghaiTime": 0,
        },
        "nonce": "0x0",
        "timestamp": "0x0",
        "extraData": "0x",
        "gasLimit": hex(gas_limit),
        "difficulty": "0x0",
        "mixHash": "0x" + "0" * 64,
        "coinbase": "0x" + "0" * 40,
        "number": "0x0",
        "gasUsed": "0x0",
        "parentHash": "0x" + "0" * 64,
        "baseFeePerGas": hex(base_fee_per_gas),
        "alloc": {
            premine_address: {"balance": hex(premine_balance)}
        },
    }


def parse_uint(value):
    try:
        parsed = int(value, 16 if value.lower().startswith("0x") else 10)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"invalid integer value: {value}") from exc
    if parsed < 0:
        raise argparse.ArgumentTypeError("value must not be negative")
    return parsed


def main():
    parser = argparse.ArgumentParser(description="Generate the isolated one-node benchmark genesis.")
    parser.add_argument("--chain-id", type=int, required=True)
    parser.add_argument("--gas-limit", type=parse_uint, default=DEFAULT_GAS_LIMIT)
    parser.add_argument("--base-fee-per-gas", type=parse_uint, default=DEFAULT_BASE_FEE_PER_GAS)
    parser.add_argument("--terminal-total-difficulty", type=parse_uint, default=0)
    parser.add_argument("--premine-address", default=DEFAULT_PREMINE_ADDRESS)
    parser.add_argument("--premine-balance", type=parse_uint, default=DEFAULT_PREMINE_BALANCE)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.chain_id < 1:
        parser.error("--chain-id must be positive")
    if args.gas_limit < 1:
        parser.error("--gas-limit must be positive")
    if not re.fullmatch(r"(?:0x)?[0-9a-fA-F]{40}", args.premine_address):
        parser.error("--premine-address must be a 20-byte hexadecimal address")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(
            genesis(
                args.chain_id,
                gas_limit=args.gas_limit,
                base_fee_per_gas=args.base_fee_per_gas,
                terminal_total_difficulty=args.terminal_total_difficulty,
                premine_address=args.premine_address,
                premine_balance=args.premine_balance,
            ),
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()