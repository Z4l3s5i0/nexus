#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT_DIR="${1:-}"
[[ -n "$OUTPUT_DIR" ]] || { echo "Usage: bash capture_logs.sh OUTPUT_DIR" >&2; exit 2; }
mkdir -p "$OUTPUT_DIR"

if docker compose version >/dev/null 2>&1; then
    compose=(docker compose)
else
    compose=(docker-compose)
fi
container="$("${compose[@]}" -f "$REPO_ROOT/docker-compose.v2.yml" ps -q el-node-0)"
if [[ -z "$container" ]]; then
    echo "No running execution client was found; logs are unavailable." >&2
    exit 0
fi
if ! docker logs --timestamps "$container" > "$OUTPUT_DIR/el-node-0.log" 2> "$OUTPUT_DIR/el-node-0.stderr"; then
    echo "Could not capture execution-client logs." >&2
    rm -f "$OUTPUT_DIR/el-node-0.log"
fi