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

capture_service_logs "el-node-0"
capture_service_logs "cl-node-0"
capture_service_logs "vc-node-0" true