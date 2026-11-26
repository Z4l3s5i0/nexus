#!/usr/bin/env python3
import argparse
import json
from pathlib import Path


def record_status(output, exit_code):
    status = "passed" if exit_code == 0 else "failed"
    path = Path(output)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps({"status": status, "exit_code": exit_code}, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def main():
    parser = argparse.ArgumentParser(description="Record the final status of an experiment run.")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--exit-code", required=True, type=int)
    args = parser.parse_args()
    record_status(args.output, args.exit_code)


if __name__ == "__main__":
    main()