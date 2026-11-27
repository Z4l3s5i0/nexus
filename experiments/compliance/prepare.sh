#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CLIENTS="wasix-w-eth,wasix-r-eth,go-ethereum_v1.15.11,reth_v2.5.2"
HIVE_IMAGE="hive-custom:latest"
BASE_REGISTRY="docker.io/z4l3s5i0"
OUTPUT_DIR=""
SKIP_BASE_BUILD=false
SKIP_HIVE_BUILD=false

usage() {
    echo "Usage: bash prepare.sh --output-dir DIR [--clients LIST] [--hive-image IMAGE] [--skip-base-build] [--skip-hive-build]" >&2
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --output-dir) OUTPUT_DIR="$2"; shift 2 ;;
        --clients) CLIENTS="$2"; shift 2 ;;
        --hive-image) HIVE_IMAGE="$2"; shift 2 ;;
        --skip-base-build) SKIP_BASE_BUILD=true; shift ;;
        --skip-hive-build) SKIP_HIVE_BUILD=true; shift ;;
        --help|-h) usage; exit 0 ;;
        *) usage; exit 2 ;;
    esac
done

[[ -n "$OUTPUT_DIR" ]] || { usage; exit 2; }
docker info >/dev/null 2>&1 || { echo "Error: Docker daemon is unavailable." >&2; exit 1; }
mkdir -p "$OUTPUT_DIR/build-logs"
IFS=',' read -r -a clients <<< "$CLIENTS"
has_native=false
has_wasix=false
for client in "${clients[@]}"; do
    case "$client" in
        wasix-r-eth) has_native=true ;;
        wasix-w-eth) has_wasix=true ;;
        go-ethereum_v1.15.11|reth_v2.5.2) ;;
        *) echo "Error: unsupported Hive client '$client'." >&2; exit 2 ;;
    esac
done

NATIVE_IMAGE="$BASE_REGISTRY/wasix-eth-linux:latest"
WASIX_IMAGE="$BASE_REGISTRY/wasix-eth-wasix:latest"
if [[ "$SKIP_BASE_BUILD" != true ]]; then
    if [[ "$has_native" == true ]]; then
        echo "Building Hive native client base: $NATIVE_IMAGE"
        docker build -t "$NATIVE_IMAGE" -f "$REPO_ROOT/wasix_eth/Dockerfile.linux" "$REPO_ROOT/wasix_eth" \
            > "$OUTPUT_DIR/build-logs/wasix-r-eth-base.log" 2>&1
    fi
    if [[ "$has_wasix" == true ]]; then
        echo "Building Hive WASIX client base: $WASIX_IMAGE"
        docker build -t "$WASIX_IMAGE" -f "$REPO_ROOT/wasix_eth/Dockerfile.wasix" "$REPO_ROOT/wasix_eth" \
            > "$OUTPUT_DIR/build-logs/wasix-w-eth-base.log" 2>&1
    fi
else
    if [[ "$has_native" == true ]]; then docker image inspect "$NATIVE_IMAGE" >/dev/null; fi
    if [[ "$has_wasix" == true ]]; then docker image inspect "$WASIX_IMAGE" >/dev/null; fi
fi

if [[ "$SKIP_HIVE_BUILD" != true ]]; then
    echo "Building Hive runner image: $HIVE_IMAGE"
    docker build -f "$REPO_ROOT/hive/hive.dockerfile" -t "$HIVE_IMAGE" "$REPO_ROOT" \
        > "$OUTPUT_DIR/build-logs/hive-custom.log" 2>&1
else
    docker image inspect "$HIVE_IMAGE" >/dev/null
fi

echo "Hive image preparation complete."