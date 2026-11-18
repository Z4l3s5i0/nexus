#!/usr/bin/env python3
"""Post-process genesis.json for Geth v1.10.14 (Arrow Glacier) compatibility.

The ethereum-genesis-generator produces a modern genesis.json with:
1. Plain JSON numbers for balance, nonce, gasLimit, etc.
2. Post-Arrow Glacier config fields (shanghaiTime, cancunTime, etc.).

Geth v1.10.14's HexOrDecimal64 type accepts hex strings or decimal strings,
but NOT plain JSON numbers. This script fixes both issues.
"""

import json
import sys


# Fields in config that v1.10.14 does not recognize — safe to remove.
REMOVE_CONFIG_FIELDS = {
    "shanghaiTime",
    "cancunTime",
    "pragueTime",
    "osakaTime",
    "bogotaTime",
    "verkleTime",
    "depositContractAddress",
    "blobSchedule",
    "terminalTotalDifficultyPassed",
    "mergeNetsplitBlock",
}

# Top-level fields that must be hex strings in Geth v1.10.14.
TOP_LEVEL_HEX_FIELDS = {
    "gasLimit",
    "nonce",
    "timestamp",
    "number",
    "gasUsed",
}

# Top-level fields that use *math.HexOrDecimal256 in Geth v1.10.14 via gencodec.
# These accept hex strings, decimal strings, or plain JSON numbers.
# We use hex strings for consistency.
TOP_LEVEL_HEX_256_FIELDS = {
    "difficulty",
}

# Config fork-block fields that are *big.Int in Geth v1.10.14.
# Go 1.17's *big.Int.UnmarshalJSON only accepts plain JSON numbers,
# NOT hex strings or decimal strings. These must remain as plain integers.
CONFIG_BIG_INT_FIELDS = {
    "homesteadBlock",
    "eip150Block",
    "eip155Block",
    "eip158Block",
    "byzantiumBlock",
    "constantinopleBlock",
    "petersburgBlock",
    "istanbulBlock",
    "muirGlacierBlock",
    "berlinBlock",
    "londonBlock",
    "arrowGlacierBlock",
    "grayGlacierBlock",
    "mergeForkBlock",
}


def to_hex(value):
    """Convert an int or plain-number string to a hex-prefixed string."""
    if isinstance(value, str):
        if value.startswith("0x") or value.startswith("0X"):
            return value
        # Already a decimal string like "123" — Geth accepts this, but
        # convert to hex for consistency.
        return hex(int(value))
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return hex(int(value))
    return value


def fix_alloc(alloc_entry):
    """Convert numeric fields in an alloc entry to the correct format.

    In Geth v1.10.14, balance is *math.HexOrDecimal256 (via gencodec) which
    accepts hex strings. Nonce is math.HexOrDecimal64 which accepts hex strings.
    """
    if "balance" in alloc_entry:
        alloc_entry["balance"] = to_hex(alloc_entry["balance"])
    if "nonce" in alloc_entry:
        alloc_entry["nonce"] = to_hex(alloc_entry["nonce"])
    # storage and code are already hex strings from the generator — leave them.
    return alloc_entry


def fix_genesis(path: str) -> None:
    with open(path, "r") as f:
        genesis = json.load(f)

    # --- Fix config section ---
    config = genesis.get("config", {})

    # Remove unrecognized fields
    for field in REMOVE_CONFIG_FIELDS:
        config.pop(field, None)

    # Convert config fork block fields to plain integers for *big.Int compatibility.
    # Go 1.17's *big.Int.UnmarshalJSON only accepts plain JSON numbers, not strings.
    for field in CONFIG_BIG_INT_FIELDS:
        if field in config:
            val = config[field]
            if isinstance(val, str):
                if val.startswith("0x") or val.startswith("0X"):
                    config[field] = int(val, 16)
                else:
                    config[field] = int(val)
            elif isinstance(val, float):
                config[field] = int(val)

    # terminalTotalDifficulty in Geth v1.10.14 uses *big.Int which accepts plain
    # integers or hex strings, but the generator may output it as a string already.
    # Convert to a plain integer for maximum compatibility with v1.10.14.
    if "terminalTotalDifficulty" in config:
        val = config["terminalTotalDifficulty"]
        if isinstance(val, str):
            # Strip 0x prefix if present and convert to int
            config["terminalTotalDifficulty"] = int(val, 16) if val.startswith("0x") else int(val)
        elif isinstance(val, (int, float)):
            config["terminalTotalDifficulty"] = int(val)

    genesis["config"] = config

    # --- Fix top-level numeric fields ---
    for field in TOP_LEVEL_HEX_FIELDS:
        if field in genesis:
            genesis[field] = to_hex(genesis[field])

    # Ensure *math.HexOrDecimal256 fields are hex strings.
    for field in TOP_LEVEL_HEX_256_FIELDS:
        if field in genesis:
            genesis[field] = to_hex(genesis[field])

    # --- Fix alloc section ---
    alloc = genesis.get("alloc", {})
    for address in alloc:
        alloc[address] = fix_alloc(alloc[address])

    # Write back
    with open(path, "w") as f:
        json.dump(genesis, f, indent=2)
        f.write("\n")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <genesis.json>", file=sys.stderr)
        sys.exit(1)
    fix_genesis(sys.argv[1])
    print(f"Fixed {sys.argv[1]} for Geth v1.10.14 compatibility.")
