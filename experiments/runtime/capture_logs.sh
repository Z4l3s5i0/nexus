#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT_DIR=""
NODE_COUNT=""

usage() {
    echo "Usage: bash capture_logs.sh --output-dir DIR --nodes COUNT" >&2
    exit 2
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --output-dir) OUTPUT_DIR="$2"; shift 2 ;;
        --nodes) NODE_COUNT="$2"; shift 2 ;;
        *) usage ;;
    esac
done

[[ -n "$OUTPUT_DIR" && "$NODE_COUNT" =~ ^[1-9][0-9]*$ ]] || usage
mkdir -p "$OUTPUT_DIR"

if docker compose version >/dev/null 2>&1; then
    compose=(docker compose)
else
    compose=(docker-compose)
fi

capture_service_logs() {
    local service="$1"
    local optional="${2:-false}"
    local container

    container="$("${compose[@]}" -f "$REPO_ROOT/docker-compose.v2.yml" ps --all --quiet "$service" 2>/dev/null)" || container=""
    if [[ -z "$container" ]]; then
        if [[ "$optional" != true ]]; then
            echo "No container found for $service; leaving its log unavailable." >&2
        fi
        return 0
    fi
    if ! docker logs --timestamps "$container" > "$OUTPUT_DIR/$service.log" 2> "$OUTPUT_DIR/$service.stderr"; then
        echo "Could not capture logs for $service." >&2
        rm -f "$OUTPUT_DIR/$service.log"
    fi
}

for ((index = 0; index < NODE_COUNT; index++)); do
    capture_service_logs "el-node-$index"
    capture_service_logs "cl-node-$index"
    capture_service_logs "vc-node-$index" true
done