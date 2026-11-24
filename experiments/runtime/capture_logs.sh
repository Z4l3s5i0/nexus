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

for ((index = 0; index < NODE_COUNT; index++)); do
    container="$("${compose[@]}" -f "$REPO_ROOT/docker-compose.v2.yml" ps -q "el-node-$index")"
    if [[ -z "$container" ]]; then
        echo "No running container found for el-node-$index; leaving its log unavailable." >&2
        continue
    fi
    if ! docker logs --timestamps "$container" > "$OUTPUT_DIR/el-node-$index.log" 2> "$OUTPUT_DIR/el-node-$index.stderr"; then
        echo "Could not capture logs for el-node-$index." >&2
        rm -f "$OUTPUT_DIR/el-node-$index.log"
    fi
done