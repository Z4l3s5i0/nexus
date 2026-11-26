#!/usr/bin/env python3
import argparse
import csv
import re
from datetime import datetime, timezone
from pathlib import Path


FIELDS = ("recorded_at_utc", "experiment", "run_id", "status", "exit_code", "problem", "log_file")
FAILURE_MARKERS = re.compile(r"error:|exception|traceback|failed|timed out|could not", re.IGNORECASE)


def failure_reason(log_path, exit_code, explicit_problem):
    if explicit_problem:
        return explicit_problem
    if log_path and Path(log_path).is_file():
        lines = Path(log_path).read_text(encoding="utf-8", errors="replace").splitlines()
        for line in reversed(lines):
            if FAILURE_MARKERS.search(line):
                return line.strip()[:500]
    return f"Command exited with status {exit_code}; see the run log for details."


def main():
    parser = argparse.ArgumentParser(description="Append one experiment outcome to a suite overview CSV.")
    parser.add_argument("--overview", required=True, type=Path)
    parser.add_argument("--experiment", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--status", required=True, choices=("passed", "failed", "skipped"))
    parser.add_argument("--exit-code", required=True, type=int)
    parser.add_argument("--log-file", default="")
    parser.add_argument("--problem", default="")
    args = parser.parse_args()

    args.overview.parent.mkdir(parents=True, exist_ok=True)
    exists = args.overview.is_file()
    with args.overview.open("a", newline="", encoding="utf-8") as overview:
        writer = csv.DictWriter(overview, fieldnames=FIELDS)
        if not exists:
            writer.writeheader()
        writer.writerow(
            {
                "recorded_at_utc": datetime.now(timezone.utc).isoformat(),
                "experiment": args.experiment,
                "run_id": args.run_id,
                "status": args.status,
                "exit_code": args.exit_code,
                "problem": failure_reason(args.log_file, args.exit_code, args.problem)
                if args.status != "passed"
                else "",
                "log_file": args.log_file,
            }
        )


if __name__ == "__main__":
    main()