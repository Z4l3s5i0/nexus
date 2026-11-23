#!/usr/bin/env python3
"""Allocate workload targets in native, Geth, Reth, WASIX index order."""

import sys
from decimal import Decimal, InvalidOperation, ROUND_FLOOR


def allocate(counts, percent):
    if len(counts) != 4 or any(not isinstance(n, int) or n < 0 for n in counts):
        raise ValueError("node counts must be nonnegative integers")
    total = sum(counts)
    if total == 0:
        raise ValueError("at least one execution node is required")
    try:
        percentage = Decimal(percent)
    except InvalidOperation as exc:
        raise ValueError("workload percentage must be a number from 0 to 100") from exc
    if not percentage.is_finite() or not 0 <= percentage <= 100:
        raise ValueError("workload percentage must be a number from 0 to 100")
    targets = int((Decimal(total) * percentage / 100).to_integral_value(rounding=ROUND_FLOOR))
    if percentage > 0 and targets == 0:
        targets = 1
    allocations = [targets * count // total for count in counts]
    remainders = [targets * count % total for count in counts]
    order = sorted(range(4), key=lambda i: (-remainders[i], i))
    for i in order[:targets - sum(allocations)]:
        allocations[i] += 1
    return total, targets, *allocations


def main(args):
    if len(args) != 5 or any(not n.isascii() or not n.isdecimal() for n in args[:4]):
        raise ValueError("node counts must be nonnegative integers")
    result = allocate([int(n) for n in args[:4]], args[4])
    print(*result)


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except ValueError as exc:
        print(f"Error: {exc}", file=sys.stderr)
        sys.exit(1)