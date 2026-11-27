#!/usr/bin/env python3
import argparse
import json
import subprocess
from pathlib import Path


def image_info(reference):
    result = subprocess.run(
        ["docker", "image", "inspect", "--format", "{{.Id}}", reference],
        text=True,
        capture_output=True,
        check=False,
    )
    return {"reference": reference, "image_id": result.stdout.strip() or None, "available": result.returncode == 0}


def main():
    parser = argparse.ArgumentParser(description="Capture selected Hive client and runner image identities.")
    parser.add_argument("--output", required=True)
    parser.add_argument("--clients", required=True)
    parser.add_argument("--hive-image", default="hive-custom:latest")
    parser.add_argument("--registry", default="docker.io/z4l3s5i0")
    args = parser.parse_args()
    clients = [client for client in args.clients.split(",") if client]
    images = {"hive": image_info(args.hive_image)}
    if "go-ethereum_v1.15.11" in clients:
        images["go-ethereum_v1.15.11-base"] = image_info("ethereum/client-go:v1.15.11")
    if "reth_v2.5.2" in clients:
        images["reth_v2.5.2-base"] = image_info("ghcr.io/paradigmxyz/reth:v2.5.2")
    if "wasix-r-eth" in clients:
        images["wasix-r-eth-base"] = image_info(f"{args.registry}/wasix-eth-linux:latest")
    if "wasix-w-eth" in clients:
        images["wasix-w-eth-base"] = image_info(f"{args.registry}/wasix-eth-wasix:latest")
    metadata = {
        "simulation": "ethereum/engine",
        "clients": clients,
        "images": images,
    }
    path = Path(args.output)
    path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()