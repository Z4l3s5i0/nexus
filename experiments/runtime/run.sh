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

runtime_progress() {
    printf 'Runtime progress: %s\n' "$*"
}

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
  --workload-type NAME   Cast runtime scenario (default: $WORKLOAD_TYPE)
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
EXPERIMENT_NETWORK_CLEAN_DATA=true
export EXPERIMENT_NETWORK_CLEAN_DATA
export CHAIN_ID LINUX_NODES WASIX_NODES GETH_NODES RETH_NODES

bash "$NETWORK_SCRIPT" check
mkdir -p "$OUTPUT_ROOT/runtime"
OUTPUT_DIR="$OUTPUT_ROOT/runtime/$RUN_ID"
if ! mkdir "$OUTPUT_DIR" 2>/dev/null; then
    echo "Error: output directory already exists: $OUTPUT_DIR" >&2
    exit 1
fi
mkdir -p "$OUTPUT_DIR/logs" "$OUTPUT_DIR/workloads"
NODE_COUNT=$((LINUX_NODES + WASIX_NODES + GETH_NODES + RETH_NODES))
CLIENT_LOGS_CAPTURED=false

cleanup() {
    local status=$?
    if [[ -n "$OUTPUT_DIR" && -d "$OUTPUT_DIR" ]]; then
        python3 "$REPO_ROOT/experiments/record_run_status.py" \
            --output "$OUTPUT_DIR/run_status.json" --exit-code "$status" || true
    fi
    if [[ -n "$SYSTEM_COLLECTOR_PID" ]] && kill -0 "$SYSTEM_COLLECTOR_PID" 2>/dev/null; then
        runtime_progress "Stopping system metrics collection."
        kill "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
        wait "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
        runtime_progress "System metrics collection stopped."
    fi
    if [[ "$status" -ne 0 && "$NETWORK_STARTED" == true && "$CLIENT_LOGS_CAPTURED" != true && "$NODE_COUNT" -gt 0 ]]; then
        runtime_progress "Capturing client logs after the failed runtime run."
        if ! bash "$SCRIPT_DIR/capture_logs.sh" --output-dir "$OUTPUT_DIR/logs" --nodes "$NODE_COUNT"; then
            runtime_progress "Client log capture after failure could not be completed."
        fi
        CLIENT_LOGS_CAPTURED=true
    fi
    if [[ "$NETWORK_STARTED" == true ]]; then
        runtime_progress "Tearing down the runtime network."
        if bash "$NETWORK_SCRIPT" stop; then
            runtime_progress "Runtime network teardown finished."
        else
            runtime_progress "Runtime network teardown could not be confirmed."
        fi
    fi
    return "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

NETWORK_STARTED=true
runtime_progress "Starting the runtime network."
bash "$NETWORK_SCRIPT" start
runtime_progress "Runtime network started; waiting for nodes and metrics to become ready."

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
runtime_progress "Runtime network and execution nodes are ready."
runtime_progress "Waiting 60 seconds for execution nodes to settle before starting workloads."
sleep 60

python3 "$SCRIPT_DIR/capture_metadata.py" \
    --output "$OUTPUT_DIR/metadata.json" \
    --experiment-id "$RUN_ID" --chain-id "$CHAIN_ID" \
    --linux-nodes "$LINUX_NODES" --wasix-nodes "$WASIX_NODES" \
    --geth-nodes "$GETH_NODES" --reth-nodes "$RETH_NODES" \
    --workload-type "$WORKLOAD_TYPE" --duration "$DURATION" \
    --wait-duration "$WAIT_DURATION" --tps "$TPS" --accounts "$ACCOUNTS"
runtime_progress "Run metadata recorded."

MONITOR_DURATION=$((DURATION + WAIT_DURATION + 60))
python3 "$SCRIPT_DIR/collect_system_metrics.py" \
    --metadata "$OUTPUT_DIR/metadata.json" \
    --output "$OUTPUT_DIR/system_metric_samples.jsonl" \
    --experiment-id "$RUN_ID" --network "$CHAIN_ID" \
    --duration "$MONITOR_DURATION" --interval 1 \
    > "$OUTPUT_DIR/system_collector.stdout" 2> "$OUTPUT_DIR/system_collector.stderr" &
SYSTEM_COLLECTOR_PID=$!
runtime_progress "System metrics collection started."

FIRST_NODE="$(docker compose -f "$REPO_ROOT/docker-compose.v2.yml" ps -q el-node-0)"
NETWORK_NAME="$(docker inspect --format '{{range $name, $_ := .NetworkSettings.Networks}}{{$name}}{{end}}' "$FIRST_NODE")"
if [[ -z "$NETWORK_NAME" ]]; then
    echo "Error: could not determine the runtime Docker network." >&2
    exit 1
fi

CONTRACT_ADDRESS=""
WORKLOAD_SCENARIO="${WORKLOAD_TYPE#exp/}"
if [[ "$WORKLOAD_SCENARIO" != "value-transfer" ]]; then
    CONTRACT_ADDRESS_FILE="$OUTPUT_DIR/workloads/contract_address.txt"
    runtime_progress "Deploying the shared $WORKLOAD_TYPE contract and checking it across execution nodes."
    if ! python3 "$SCRIPT_DIR/deploy_contract.py" \
        --workload-script "$SCRIPT_DIR/workload.sh" \
        --chain-id "$CHAIN_ID" --network "$NETWORK_NAME" \
        --scenario "$WORKLOAD_TYPE" --accounts "$ACCOUNTS" \
        --linux-nodes "$LINUX_NODES" --geth-nodes "$GETH_NODES" \
        --reth-nodes "$RETH_NODES" --wasix-nodes "$WASIX_NODES" \
        --output-address "$CONTRACT_ADDRESS_FILE" \
        --output-info "$OUTPUT_DIR/workloads/contract_deployment.json" \
        --log-dir "$OUTPUT_DIR/workloads" \
        > "$OUTPUT_DIR/workloads/contract_setup.log" 2>&1; then
        cat "$OUTPUT_DIR/workloads/contract_setup.log" >&2
        exit 1
    fi
    CONTRACT_ADDRESS="$(<"$CONTRACT_ADDRESS_FILE")"
    if [[ ! "$CONTRACT_ADDRESS" =~ ^0x[0-9a-fA-F]{40}$ ]]; then
        echo "Error: shared contract setup did not produce a valid address." >&2
        exit 1
    fi
    runtime_progress "Shared contract deployed and verified at $CONTRACT_ADDRESS."
fi

WORKLOAD_PIDS=()
runtime_progress "Workload $WORKLOAD_TYPE spam starting on $NODE_COUNT nodes ($DURATION seconds at $TPS TPS per node)."
for ((index = 0; index < NODE_COUNT; index++)); do
    WORKLOAD_ARGS=(
        bash "$SCRIPT_DIR/workload.sh"
        --node-index "$index" --chain-id "$CHAIN_ID" --network "$NETWORK_NAME" \
        --run-id "$RUN_ID" \
        --window-file "$OUTPUT_DIR/workload_windows/node_$index.json" \
        --submission-manifest-file "$OUTPUT_DIR/workloads/node_$index.submissions.json" \
        --scenario "$WORKLOAD_TYPE" --tps "$TPS" --duration "$DURATION" --accounts "$ACCOUNTS"
    )
    if [[ -n "$CONTRACT_ADDRESS" ]]; then
        WORKLOAD_ARGS+=(--contract-address "$CONTRACT_ADDRESS")
    fi
    "${WORKLOAD_ARGS[@]}" > "$OUTPUT_DIR/workloads/node_$index.log" 2>&1 &
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
    runtime_progress "Workload $WORKLOAD_TYPE spam failed on at least one node."
    exit 1
fi
runtime_progress "Workload $WORKLOAD_TYPE spam finished on all $NODE_COUNT nodes."

RECEIPT_PIDS=()
for ((index = 0; index < NODE_COUNT; index++)); do
    python3 "$SCRIPT_DIR/collect_receipts.py" \
        --rpc-url "http://127.0.0.1:$((8545 + index))" \
        --manifest "$OUTPUT_DIR/workloads/node_$index.submissions.json" \
        --output "$OUTPUT_DIR/workloads/node_$index.receipts.json" \
        --timeout 120 \
        > "$OUTPUT_DIR/workloads/node_$index.receipt_collection.log" 2>&1 &
    RECEIPT_PIDS+=("$!")
done
runtime_progress "Separate receipt collection started for accepted workload transactions."

if (( WAIT_DURATION > 0 )); then
    runtime_progress "Workload finished; commencing wait time of $WAIT_DURATION seconds for blocks and metrics."
    sleep "$WAIT_DURATION"
    runtime_progress "Wait time achieved."
else
    runtime_progress "No post-workload wait configured; continuing to collection."
fi

for pid in "${RECEIPT_PIDS[@]}"; do
    if wait "$pid"; then
        :
    else
        status=$?
        echo "Warning: receipt collection process exited with status $status; the workload result is preserved." >&2
    fi
done
runtime_progress "Separate receipt collection finished."

if [[ -n "$SYSTEM_COLLECTOR_PID" ]]; then
    if kill -0 "$SYSTEM_COLLECTOR_PID" 2>/dev/null; then
        runtime_progress "Stopping system metrics collection."
        kill "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
        wait "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
        SYSTEM_COLLECTOR_PID=""
        runtime_progress "System metrics collection stopped."
    elif wait "$SYSTEM_COLLECTOR_PID"; then
        SYSTEM_COLLECTOR_PID=""
        runtime_progress "System metrics collection finished."
    else
        echo "Warning: system metrics collector exited unexpectedly; local samples are preserved." >&2
        SYSTEM_COLLECTOR_PID=""
    fi
fi

runtime_progress "Collecting execution, consensus, and validation client logs and node metrics."
bash "$SCRIPT_DIR/capture_logs.sh" --output-dir "$OUTPUT_DIR/logs" --nodes "$NODE_COUNT"
CLIENT_LOGS_CAPTURED=true
python3 "$SCRIPT_DIR/collect_logs.py" \
    --metadata "$OUTPUT_DIR/metadata.json" --logs-dir "$OUTPUT_DIR/logs" \
    --workload-windows "$OUTPUT_DIR/workload_windows" \
    --output-dir "$OUTPUT_DIR" --experiment-id "$RUN_ID"
python3 "$SCRIPT_DIR/collect_node_metrics.py" \
    --metadata "$OUTPUT_DIR/metadata.json" \
    --output "$OUTPUT_DIR/node_metric_coverage.json" --experiment-id "$RUN_ID"
runtime_progress "Client logs and node metrics collected."

runtime_progress "Archiving runtime metrics data."
bash "$NETWORK_SCRIPT" stop-metrics
if [[ -f "$REPO_ROOT/data_v2/metrics-server/metrics.redb" ]]; then
    mkdir -p "$OUTPUT_DIR/metrics_server_data"
    cp "$REPO_ROOT/data_v2/metrics-server/metrics.redb" "$OUTPUT_DIR/metrics_server_data/metrics.redb"
else
    echo "Error: metrics-server database was not found; runtime metrics were not archived." >&2
    exit 1
fi
runtime_progress "Runtime metrics data archived."
runtime_progress "Generating the runtime report."
python3 "$SCRIPT_DIR/report.py" --output-dir "$OUTPUT_DIR"
runtime_progress "Runtime report generated."
runtime_progress "Tearing down the runtime network."
bash "$NETWORK_SCRIPT" stop
NETWORK_STARTED=false
runtime_progress "Runtime network teardown finished."

echo "Runtime experiment results: $OUTPUT_DIR"