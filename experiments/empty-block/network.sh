#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
NETWORK_START="$REPO_ROOT/experiments/shared/network/launch.sh"
COMPOSE_FILE="$REPO_ROOT/docker-compose.v2.yml"

compose() {
    if docker compose version >/dev/null 2>&1; then
        docker compose -f "$COMPOSE_FILE" "$@"
    else
        docker-compose -f "$COMPOSE_FILE" "$@"
    fi
}

has_generated_data() {
    for directory in "$REPO_ROOT/data_v2" "$REPO_ROOT/startup_v2"; do
        if [[ -d "$directory" && -n "$(ls -A "$directory" 2>/dev/null)" ]]; then
            return 0
        fi
    done
    return 1
}

check() {
    if [[ -f "$COMPOSE_FILE" ]]; then
        local running
        running="$(compose ps -q 2>/dev/null || true)"
        if [[ -n "$running" ]]; then
            echo "Error: a v2 experiment network is already running; stop it before starting empty-block." >&2
            return 1
        fi
    fi
    if [[ "${RESET_NETWORK:-false}" != true ]] && has_generated_data; then
        echo "Error: generated v2 network data exists; use --reset-network to allow its removal." >&2
        return 1
    fi
}

start() {
    local options=(
        --chain-id "$CHAIN_ID"
        --linux-nodes 0
        --wasix-nodes 0
        --geth-nodes 0
        --reth-nodes 0
        --metrics-image wasix-eth-metrics:latest
    )
    case "$CLIENT" in
        native|linux) options+=(--linux-nodes 1) ;;
        wasix) options+=(--wasix-nodes 1) ;;
        geth) options+=(--geth-nodes 1) ;;
        reth) options+=(--reth-nodes 1) ;;
        *) echo "Error: unsupported client '$CLIENT'." >&2; return 2 ;;
    esac
    if [[ "${RESET_NETWORK:-false}" == true ]]; then
        options+=(--cleanup)
    fi
    cd "$REPO_ROOT"
    GENESIS_DELAY=0 bash "$NETWORK_START" "${options[@]}"
}

stop() {
    if [[ ! -f "$COMPOSE_FILE" ]]; then
        return 0
    fi
    compose down --remove-orphans
    if [[ "${EXPERIMENT_NETWORK_CLEAN_DATA:-false}" == true ]]; then
        if [[ "$EUID" -eq 0 ]]; then
            rm -rf "$REPO_ROOT/startup_v2" "$REPO_ROOT/data_v2"
        else
            sudo rm -rf "$REPO_ROOT/startup_v2" "$REPO_ROOT/data_v2"
        fi
    fi
}

case "${1:-}" in
    check) check ;;
    start) start ;;
    stop) stop ;;
    stop-metrics) compose stop metrics-server ;;
    *) echo "Usage: bash network.sh {check|start|stop|stop-metrics}" >&2; exit 2 ;;
esac