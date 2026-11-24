#!/usr/bin/env python3
import argparse
import json
from pathlib import Path


def read_results(path):
    results = []
    if not path.is_file():
        return results
    with path.open(encoding="utf-8") as source:
        for line in source:
            try:
                results.append(json.loads(line))
            except json.JSONDecodeError:
                continue
    return results


def build_report(output_dir, run_id):
    output_dir = Path(output_dir)
    metadata_path = output_dir / "metadata.json"
    metadata = json.loads(metadata_path.read_text(encoding="utf-8")) if metadata_path.is_file() else {}
    results = read_results(output_dir / "results.jsonl")
    report = {
        "experiment_id": run_id,
        "simulation": "ethereum/engine",
        "clients": metadata.get("clients", []),
        "images": metadata.get("images", {}),
        "results": results,
        "passed": sum(result.get("exit_code") == 0 for result in results),
        "failed": sum(result.get("exit_code") != 0 for result in results),
        "notes": [
            "Hive ran the existing ethereum/engine simulation against the selected custom clients.",
            "The runner used --docker.pull=false so locally built client images were used.",
            "Per-client stdout/stderr logs and Hive workspaces are retained under this output directory.",
        ],
    }
    (output_dir / "report.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    lines = [
        f"# Hive compliance run {run_id}",
        "",
        "- Simulation: `ethereum/engine`",
        f"- Clients passed: `{report['passed']}`",
        f"- Clients failed: `{report['failed']}`",
        "",
        "## Client results",
        "",
    ]
    if results:
        for result in results:
            lines.append(
                f"- `{result['client']}`: **{result['status']}** (exit `{result['exit_code']}`); "
                f"log: `{Path(result['log_path']).relative_to(output_dir)}`."
            )
            if result.get("error"):
                lines.append(f"  - {result['error']}")
    else:
        lines.append("- No Hive client results were recorded.")
    lines.extend(["", "See `metadata.json` for image IDs and each client workspace for Hive artifacts."])
    (output_dir / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    return report


def main():
    parser = argparse.ArgumentParser(description="Summarize Hive ethereum/engine client results.")
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--run-id")
    args = parser.parse_args()
    build_report(args.output_dir, args.run_id or Path(args.output_dir).name)


if __name__ == "__main__":
    main()