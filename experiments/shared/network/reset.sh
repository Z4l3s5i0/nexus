#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${EXPERIMENTS_NETWORK_REPO_ROOT:-$(cd "$SCRIPT_DIR/../../.." && pwd)}"
COMPOSE_FILE="$REPO_ROOT/docker-compose.v2.yml"
DOCKER_BIN="${DOCKER_BIN:-docker}"

compose() {
    if "$DOCKER_BIN" compose version >/dev/null 2>&1; then
        "$DOCKER_BIN" compose -f "$COMPOSE_FILE" "$@"
    elif command -v docker-compose >/dev/null 2>&1; then
        docker-compose -f "$COMPOSE_FILE" "$@"
    else
        echo "Error: Docker Compose is required to reset generated network data." >&2
        return 1
    fi
}

if [[ -f "$COMPOSE_FILE" ]]; then
    if ! running="$(compose ps -q)"; then
        echo "Error: could not check whether the shared network is running; refusing to reset its data." >&2
        exit 1
    fi
    if [[ -n "$running" ]]; then
        echo "Error: a shared experiment network is already running; stop it before resetting generated network data." >&2
        exit 1
    fi
    compose down --volumes --remove-orphans
fi

if ! rm -rf "$REPO_ROOT/startup_v2" "$REPO_ROOT/data_v2"; then
    sudo rm -rf "$REPO_ROOT/startup_v2" "$REPO_ROOT/data_v2"
fi