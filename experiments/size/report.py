#!/usr/bin/env python3
import argparse
import csv
import json
from datetime import datetime, timezone
from pathlib import Path


def read_measurements(path):
    with Path(path).open(newline="", encoding="utf-8") as source:
        return list(csv.DictReader(source, delimiter="\t"))


def build_report(output_dir, run_id):
    output_dir = Path(output_dir)
    sources = read_measurements(output_dir / "source_size.tsv")
    binaries = read_measurements(output_dir / "binary_size.tsv")
    report = {
        "experiment_id": run_id,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "source_code": sources,
        "compiled_binaries": binaries,
        "notes": [
            "Source size is the scc v4.1.0 code-line count with test, generated, build, and vendor content excluded.",
            "Binary size is the byte length of the supplied compiled file; no build is performed by this module.",
            "A missing source path or binary is explicitly unavailable rather than a zero-sized measurement.",
            "Docker image size is omitted because it includes unrelated runtime layers and is not equivalent to compiled client size.",
        ],
    }
    (output_dir / "report.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    lines = [
        f"# Source and binary size comparison {run_id}",
        "",
        "## Source code lines",
        "",
        "| Label | Code lines | Status | Source |",
        "|---|---:|---|---|",
    ]
    for row in sources:
        value = row.get("value") or "unavailable"
        lines.append(f"| `{row['label']}` | {value} | {row['status']} | `{row['source']}` |")
    lines.extend(["", "## Compiled binary bytes", "", "| Label | Bytes | Status | Binary |", "|---|---:|---|---|"])
    for row in binaries:
        value = row.get("value") or "unavailable"
        lines.append(f"| `{row['label']}` | {value} | {row['status']} | `{row['source']}` |")
    lines.extend(["", "### Notes", ""])
    lines.extend(f"- {note}" for note in report["notes"])
    (output_dir / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    return report


def main():
    parser = argparse.ArgumentParser(description="Report source and compiled binary size measurements.")
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--run-id", required=True)
    args = parser.parse_args()
    build_report(args.output_dir, args.run_id)


if __name__ == "__main__":
    main()