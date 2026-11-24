#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ACTION="${1:-}"
RUN_ID="${2:-}"
DATA_DIR="${3:-}"

if [[ ! "$RUN_ID" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]]; then
    echo "Usage: bash metrics_server.sh {start|stop} RUN_ID [DATA_DIR]" >&2
    exit 2
fi
RUN_SLUG="${RUN_ID//[^A-Za-z0-9_.-]/-}"
METRICS_CONTAINER="startup-metrics-${RUN_SLUG:0:100}"

case "$ACTION" in
    start)
        [[ -n "$DATA_DIR" ]] || { echo "DATA_DIR is required for start." >&2; exit 2; }
        if curl -fsS --max-time 1 http://127.0.0.1:9100/health >/dev/null 2>&1; then
            echo "Error: port 9100 is already serving a metrics API." >&2
            exit 1
        fi
        if ! docker image inspect wasix-eth-metrics:latest >/dev/null 2>&1; then
            echo "Error: shared metrics image is missing; run experiments/setup.sh first." >&2
            exit 1
        fi
        mkdir -p "$DATA_DIR"
        docker run --rm -d \
            --name "$METRICS_CONTAINER" \
            --stop-signal SIGINT \
            -p 127.0.0.1:9100:9100 \
            -e "METRICS_INGEST_TOKEN=${METRICS_INGEST_TOKEN:?missing ingestion token}" \
            -e "EXPERIMENT_RUN_ID=$RUN_ID" \
            -v "$(cd "$DATA_DIR" && pwd):/app/data" \
            wasix-eth-metrics:latest run --config config/docker.toml >/dev/null
        ready=false
        for attempt in $(seq 1 30); do
            if curl -fsS --max-time 2 http://127.0.0.1:9100/health >/dev/null 2>&1; then
                ready=true
                break
            fi
            sleep 1
        done
        if [[ "$ready" != true ]]; then
            docker logs "$METRICS_CONTAINER" >&2 || true
            docker stop "$METRICS_CONTAINER" >/dev/null 2>&1 || true
            exit 1
        fi
        echo "$METRICS_CONTAINER"
        ;;
    stop)
        if docker inspect "$METRICS_CONTAINER" >/dev/null 2>&1; then
            if [[ -n "$DATA_DIR" ]]; then
                docker logs "$METRICS_CONTAINER" > "$DATA_DIR/metrics-server.log" 2>&1 || true
            fi
            docker stop --time 120 "$METRICS_CONTAINER" >/dev/null
            docker rm "$METRICS_CONTAINER" >/dev/null 2>&1 || true
        fi
        ;;
    *)
        echo "Usage: bash metrics_server.sh {start|stop} RUN_ID [DATA_DIR]" >&2
        exit 2
        ;;
esac