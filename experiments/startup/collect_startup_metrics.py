#!/usr/bin/env python3
import argparse
import json
import os
import sys
from datetime import datetime, timezone
from pathlib import Path

RUNTIME_HELPERS = Path(__file__).resolve().parents[1] / "runtime"
sys.path.insert(0, str(RUNTIME_HELPERS))
from metrics_api import ingest_samples, metric_sample


def build_sample(result):
    if not result.get("ready") or result.get("startup_time_ms") is None:
        return None
    ready_at = result.get("ready_at")
    try:
        timestamp = datetime.fromisoformat(ready_at.replace("Z", "+00:00")) if ready_at else datetime.now(timezone.utc)
    except ValueError:
        timestamp = datetime.now(timezone.utc)
    if timestamp.tzinfo is None:
        timestamp = timestamp.replace(tzinfo=timezone.utc)
    return metric_sample(
        int(timestamp.timestamp() * 1000),
        result["experiment_id"],
        result["node_id"],
        str(result["chain_id"]),
        result["client"],
        "Derived",
        "startup_time_ms",
        float(result["startup_time_ms"]),
        {
            "startup_config": result["config"],
            "client_variant": result["client_variant"],
            "iteration": str(result["iteration"]),
            "image": result.get("image", "unavailable"),
            "image_id": result.get("image_id", "unavailable"),
            "basis": "time from client launch to first successful eth_chainId response",
        },
    )


def collect(result_path, samples_path, coverage_path, api_url, token):
    result = json.loads(Path(result_path).read_text(encoding="utf-8"))
    sample = build_sample(result)
    if sample:
        with Path(samples_path).open("a", encoding="utf-8") as output:
            output.write(json.dumps(sample, sort_keys=True, allow_nan=False) + "\n")
    error = None
    try:
        ingest_samples(api_url, token, [sample] if sample else [])
    except (RuntimeError, ValueError) as exc:
        error = str(exc)
        print(f"Startup metric saved locally but not ingested: {exc}", file=sys.stderr)

    coverage_path = Path(coverage_path)
    if coverage_path.is_file():
        coverage = json.loads(coverage_path.read_text(encoding="utf-8"))
    else:
        coverage = {"experiment_id": result["experiment_id"], "iterations": []}
    coverage["iterations"].append(
        {
            "config": result["config"],
            "iteration": result["iteration"],
            "node_id": result["node_id"],
            "client": result["client"],
            "status": "observed" if sample else "unavailable",
            "startup_time_ms": result.get("startup_time_ms"),
            "error": result.get("error"),
            "ingestion_error": error,
        }
    )
    coverage_path.write_text(json.dumps(coverage, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return sample, coverage["iterations"][-1]


def main():
    parser = argparse.ArgumentParser(description="Store observed startup duration and explicit missing-readiness status.")
    parser.add_argument("--result", required=True)
    parser.add_argument("--samples", required=True)
    parser.add_argument("--coverage", required=True)
    parser.add_argument("--api-url", default="http://127.0.0.1:9100")
    parser.add_argument("--token", default=None)
    args = parser.parse_args()
    token = args.token if args.token is not None else os.environ.get("METRICS_INGEST_TOKEN", "")
    collect(args.result, args.samples, args.coverage, args.api_url, token)


if __name__ == "__main__":
    main()