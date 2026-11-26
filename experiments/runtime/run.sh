#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
NETWORK_SCRIPT="$SCRIPT_DIR/network.sh"
RUN_ID="${EXPERIMENT_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
OUTPUT_ROOT="${EXPERIMENTS_OUTPUT_ROOT:-$REPO_ROOT/experiment-results}"
CHAIN_ID=12345
LINUX_NODES=2
WASIX_NODES=2
GETH_NODES=0
RETH_NODES=0
DURATION=300
WAIT_DURATION=1800
WORKLOAD_TYPE="exp/uber"
TPS=4
ACCOUNTS=4
RESET_NETWORK=false
NETWORK_STARTED=false
SYSTEM_COLLECTOR_PID=""
OUTPUT_DIR=""

usage() {
    cat <<EOF
Usage: bash experiments/runtime/run.sh [options]

Options:
  --chain-id ID          Local network chain ID (default: $CHAIN_ID)
  --linux-nodes N        Native client nodes (default: $LINUX_NODES)
  --wasix-nodes N        WASIX client nodes (default: $WASIX_NODES)
  --geth-nodes N         Geth nodes (default: $GETH_NODES)
  --reth-nodes N         Reth nodes (default: $RETH_NODES)
  --duration SECONDS     Workload duration (default: $DURATION)
  --wait-duration SECS   Post-workload collection wait (default: $WAIT_DURATION)
  --workload-type NAME   Contender scenario (default: $WORKLOAD_TYPE)
  --tps N                Target TPS per workload node (default: $TPS)
  --accounts N           Accounts per workload (default: $ACCOUNTS)
  --reset-network        Allow the local launcher to clear existing generated node data
  --help                 Show this help
EOF
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --chain-id) CHAIN_ID="$2"; shift 2 ;;
        --linux-nodes) LINUX_NODES="$2"; shift 2 ;;
        --wasix-nodes) WASIX_NODES="$2"; shift 2 ;;
        --geth-nodes) GETH_NODES="$2"; shift 2 ;;
        --reth-nodes) RETH_NODES="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --wait-duration) WAIT_DURATION="$2"; shift 2 ;;
        --workload-type) WORKLOAD_TYPE="$2"; shift 2 ;;
        --tps) TPS="$2"; shift 2 ;;
        --accounts) ACCOUNTS="$2"; shift 2 ;;
        --reset-network) RESET_NETWORK=true ; shift ;;
        --help|-h) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

for value in "$CHAIN_ID" "$DURATION" "$TPS" "$ACCOUNTS"; do
    [[ "$value" =~ ^[1-9][0-9]*$ ]] || { echo "Error: numeric options must be positive integers." >&2; exit 2; }
done
for value in "$LINUX_NODES" "$WASIX_NODES" "$GETH_NODES" "$RETH_NODES" "$WAIT_DURATION"; do
    [[ "$value" =~ ^[0-9]+$ ]] || { echo "Error: node counts and wait duration must be nonnegative integers." >&2; exit 2; }
done
if (( LINUX_NODES + WASIX_NODES + GETH_NODES + RETH_NODES == 0 )); then
    echo "Error: at least one node is required." >&2
    exit 2
fi
if [[ ! "$RUN_ID" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]]; then
    echo "Error: EXPERIMENT_RUN_ID contains unsupported characters." >&2
    exit 2
fi

if [[ -z "${METRICS_INGEST_TOKEN:-}" ]]; then
    METRICS_INGEST_TOKEN="$(openssl rand -hex 32)"
fi
export METRICS_INGEST_TOKEN EXPERIMENT_RUN_ID="$RUN_ID"
METRICS_SERVER_BIND=127.0.0.1
export METRICS_SERVER_BIND
export RESET_NETWORK
EXPERIMENT_NETWORK_CLEAN_DATA=false
export EXPERIMENT_NETWORK_CLEAN_DATA
export CHAIN_ID LINUX_NODES WASIX_NODES GETH_NODES RETH_NODES

bash "$NETWORK_SCRIPT" check
mkdir -p "$OUTPUT_ROOT/runtime"
OUTPUT_DIR="$OUTPUT_ROOT/runtime/$RUN_ID"
if ! mkdir "$OUTPUT_DIR" 2>/dev/null; then
    echo "Error: output directory already exists: $OUTPUT_DIR" >&2
    exit 1
fi
mkdir -p "$OUTPUT_DIR/logs" "$OUTPUT_DIR/workloads" "$OUTPUT_DIR/contender_state"

cleanup() {
    local status=$?
    if [[ -n "$OUTPUT_DIR" && -d "$OUTPUT_DIR" ]]; then
        python3 "$REPO_ROOT/experiments/record_run_status.py" \
            --output "$OUTPUT_DIR/run_status.json" --exit-code "$status" || true
    fi
    if [[ -n "$SYSTEM_COLLECTOR_PID" ]] && kill -0 "$SYSTEM_COLLECTOR_PID" 2>/dev/null; then
        kill "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
        wait "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
    fi
    if [[ "$NETWORK_STARTED" == true ]]; then
        bash "$NETWORK_SCRIPT" stop || true
    fi
    return "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

NETWORK_STARTED=true
echo "Starting runtime network for run $RUN_ID..."
bash "$NETWORK_SCRIPT" start

NODE_COUNT=$((LINUX_NODES + WASIX_NODES + GETH_NODES + RETH_NODES))
READY=false
for attempt in $(seq 1 90); do
    if curl -fsS --max-time 2 http://127.0.0.1:9100/health >/dev/null; then
        READY=true
        break
    fi
    sleep 2
done
if [[ "$READY" != true ]]; then
    echo "Error: metrics server did not become ready." >&2
    exit 1
fi

for ((index = 0; index < NODE_COUNT; index++)); do
    port=$((8545 + index))
    ready=false
    for attempt in $(seq 1 90); do
        response="$(curl -fsS --max-time 3 -H 'Content-Type: application/json' \
            --data-binary '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
            "http://127.0.0.1:$port" 2>/dev/null || true)"
        if python3 -c 'import json,sys; print("ok" if json.loads(sys.argv[1]).get("result") is not None else "")' "$response" 2>/dev/null | grep -q '^ok$'; then
            ready=true
            break
        fi
        sleep 2
    done
    if [[ "$ready" != true ]]; then
        echo "Error: execution node $index did not expose JSON-RPC on port $port." >&2
        exit 1
    fi
done

python3 "$SCRIPT_DIR/capture_metadata.py" \
    --output "$OUTPUT_DIR/metadata.json" \
    --experiment-id "$RUN_ID" --chain-id "$CHAIN_ID" \
    --linux-nodes "$LINUX_NODES" --wasix-nodes "$WASIX_NODES" \
    --geth-nodes "$GETH_NODES" --reth-nodes "$RETH_NODES" \
    --workload-type "$WORKLOAD_TYPE" --duration "$DURATION" \
    --wait-duration "$WAIT_DURATION" --tps "$TPS" --accounts "$ACCOUNTS"

MONITOR_DURATION=$((DURATION + WAIT_DURATION + 60))
python3 "$SCRIPT_DIR/collect_system_metrics.py" \
    --metadata "$OUTPUT_DIR/metadata.json" \
    --output "$OUTPUT_DIR/system_metric_samples.jsonl" \
    --experiment-id "$RUN_ID" --network "$CHAIN_ID" \
    --duration "$MONITOR_DURATION" --interval 1 \
    > "$OUTPUT_DIR/system_collector.stdout" 2> "$OUTPUT_DIR/system_collector.stderr" &
SYSTEM_COLLECTOR_PID=$!

FIRST_NODE="$(docker compose -f "$REPO_ROOT/docker-compose.v2.yml" ps -q el-node-0)"
NETWORK_NAME="$(docker inspect --format '{{range $name, $_ := .NetworkSettings.Networks}}{{$name}}{{end}}' "$FIRST_NODE")"
if [[ -z "$NETWORK_NAME" ]]; then
    echo "Error: could not determine the runtime Docker network." >&2
    exit 1
fi

WORKLOAD_PIDS=()
for ((index = 0; index < NODE_COUNT; index++)); do
    bash "$SCRIPT_DIR/workload.sh" \
        --node-index "$index" --chain-id "$CHAIN_ID" --network "$NETWORK_NAME" \
        --run-id "$RUN_ID" --state-dir "$OUTPUT_DIR/contender_state/node_$index" \
        --window-file "$OUTPUT_DIR/workload_windows/node_$index.json" \
        --scenario "$WORKLOAD_TYPE" --tps "$TPS" --duration "$DURATION" --accounts "$ACCOUNTS" \
        > "$OUTPUT_DIR/workloads/node_$index.log" 2>&1 &
    WORKLOAD_PIDS+=("$!")
done

WORKLOAD_FAILED=false
for pid in "${WORKLOAD_PIDS[@]}"; do
    if wait "$pid"; then
        :
    else
        status=$?
        echo "A workload process failed with status $status." >&2
        WORKLOAD_FAILED=true
    fi
done
if [[ "$WORKLOAD_FAILED" == true ]]; then
    exit 1
fi
if (( WAIT_DURATION > 0 )); then
    echo "Waiting $WAIT_DURATION seconds for the final blocks and metrics to settle..."
    sleep "$WAIT_DURATION"
fi

if [[ -n "$SYSTEM_COLLECTOR_PID" ]]; then
    if kill -0 "$SYSTEM_COLLECTOR_PID" 2>/dev/null; then
        kill "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
        wait "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
        SYSTEM_COLLECTOR_PID=""
    elif wait "$SYSTEM_COLLECTOR_PID"; then
        SYSTEM_COLLECTOR_PID=""
    else
        echo "Warning: system metrics collector exited unexpectedly; local samples are preserved." >&2
        SYSTEM_COLLECTOR_PID=""
    fi
fi

bash "$SCRIPT_DIR/capture_logs.sh" --output-dir "$OUTPUT_DIR/logs" --nodes "$NODE_COUNT"
python3 "$SCRIPT_DIR/collect_logs.py" \
    --metadata "$OUTPUT_DIR/metadata.json" --logs-dir "$OUTPUT_DIR/logs" \
    --workload-windows "$OUTPUT_DIR/workload_windows" \
    --output-dir "$OUTPUT_DIR" --experiment-id "$RUN_ID"
python3 "$SCRIPT_DIR/collect_node_metrics.py" \
    --metadata "$OUTPUT_DIR/metadata.json" \
    --output "$OUTPUT_DIR/node_metric_coverage.json" --experiment-id "$RUN_ID"

bash "$NETWORK_SCRIPT" stop-metrics
if [[ -f "$REPO_ROOT/data_v2/metrics-server/metrics.redb" ]]; then
    mkdir -p "$OUTPUT_DIR/metrics_server_data"
    cp "$REPO_ROOT/data_v2/metrics-server/metrics.redb" "$OUTPUT_DIR/metrics_server_data/metrics.redb"
else
    echo "Error: metrics-server database was not found; runtime metrics were not archived." >&2
    exit 1
fi
python3 "$SCRIPT_DIR/report.py" --output-dir "$OUTPUT_DIR"
EXPERIMENT_NETWORK_CLEAN_DATA=true
export EXPERIMENT_NETWORK_CLEAN_DATA
bash "$NETWORK_SCRIPT" stop
NETWORK_STARTED=false

echo "Runtime experiment results: $OUTPUT_DIR"