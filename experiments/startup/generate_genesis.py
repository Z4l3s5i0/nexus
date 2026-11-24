#!/usr/bin/env python3
import argparse
import json
from pathlib import Path


def genesis(chain_id):
    return {
        "config": {
            "chainId": chain_id,
            "ethash": {},
            "clique": {"period": 1, "epoch": 30000},
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
            "shanghaiTime": 0,
            "cancelContracts": {},
        },
        "difficulty": "1",
        "gasLimit": "0x1fffffffffffff",
        "alloc": {
            "f39Fd6e51aad88F6F4ce6aB8827279cffFb92266": {
                "balance": "0x200000000000000000000000000000000000000000000000000000000000000"
            }
        },
    }


def main():
    parser = argparse.ArgumentParser(description="Generate the isolated one-node benchmark genesis.")
    parser.add_argument("--chain-id", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.chain_id < 1:
        parser.error("--chain-id must be positive")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(genesis(args.chain_id), indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()