#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
source "$SCRIPT_DIR/defaults.sh"
NETWORK_SCRIPT="$SCRIPT_DIR/network.sh"
RUN_ID="${EXPERIMENT_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
OUTPUT_ROOT="${EXPERIMENTS_OUTPUT_ROOT:-$REPO_ROOT/experiment-results}"
CLIENT=wasix
CHAIN_ID=23456
COUNTS="0,1,5,10,20,50,100"
BLOCKS_PER_COUNT=3
MAX_ATTEMPTS=5
RECEIPT_TIMEOUT=180
BLOCK_TIMEOUT=120
POLL_INTERVAL=1
WORKLOAD=none
CONTRACT_GAS_LIMIT=250000
PRIVATE_KEY="${EB_PRIVATE_KEY:-$EMPTY_BLOCK_DEFAULT_PRIVATE_KEY}"
RESET_NETWORK=false
NETWORK_STARTED=false
SYSTEM_COLLECTOR_PID=""
OUTPUT_DIR=""

usage() {
    cat <<EOF
Usage: bash experiments/empty-block/run.sh [options]

Options:
  --client TYPE            native, wasix, geth, or reth (default: $CLIENT)
  --chain-id ID            Local chain ID (default: $CHAIN_ID)
  --counts LIST            Comma-separated transaction counts (default: $COUNTS)
  --blocks-per-count N     Completed burst samples (or exact empty blocks) per count (default: $BLOCKS_PER_COUNT)
  --max-attempts N         Bursts/empty blocks observed per count (default: $MAX_ATTEMPTS)
  --receipt-timeout SECS   Receipt wait timeout (default: $RECEIPT_TIMEOUT)
  --block-timeout SECS     New block timeout (default: $BLOCK_TIMEOUT)
  --poll-interval SECS     RPC poll interval (default: $POLL_INTERVAL)
  --workload TYPE         none, counter, dota, uber, or uber_v2 contract calls (default: $WORKLOAD)
  --contract-gas-limit N  Gas limit per contract call (default: $CONTRACT_GAS_LIMIT)
  --private-key KEY        Override the default local Anvil key; EB_PRIVATE_KEY is also supported
  --reset-network          Allow the local launcher to remove pre-existing generated data
  --help                   Show this help

Nonzero counts use the pre-funded local Anvil test key unless overridden. Never use it on a live network.
EOF
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --client) CLIENT="$2"; shift 2 ;;
        --chain-id) CHAIN_ID="$2"; shift 2 ;;
        --counts) COUNTS="$2"; shift 2 ;;
        --blocks-per-count) BLOCKS_PER_COUNT="$2"; shift 2 ;;
        --max-attempts) MAX_ATTEMPTS="$2"; shift 2 ;;
        --receipt-timeout) RECEIPT_TIMEOUT="$2"; shift 2 ;;
        --block-timeout) BLOCK_TIMEOUT="$2"; shift 2 ;;
        --poll-interval) POLL_INTERVAL="$2"; shift 2 ;;
        --workload) WORKLOAD="$2"; shift 2 ;;
        --contract-gas-limit) CONTRACT_GAS_LIMIT="$2"; shift 2 ;;
        --private-key) PRIVATE_KEY="$2"; shift 2 ;;
        --reset-network) RESET_NETWORK=true; shift ;;
        --help|-h) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

case "$CLIENT" in
    native|linux) CLIENT_LABEL=wasix-eth ;;
    wasix) CLIENT_LABEL=wasix-eth ;;
    geth) CLIENT_LABEL=geth ;;
    reth) CLIENT_LABEL=reth ;;
    *) echo "Error: --client must be native, wasix, geth, or reth." >&2; exit 2 ;;
esac
case "$WORKLOAD" in
    none|counter|dota|uber|uber_v2) ;;
    *) echo "Error: --workload must be none, counter, dota, uber, or uber_v2." >&2; exit 2 ;;
esac
[[ "$CHAIN_ID" =~ ^[1-9][0-9]*$ ]] || { echo "Error: --chain-id must be a positive integer." >&2; exit 2; }
[[ "$BLOCKS_PER_COUNT" =~ ^[1-9][0-9]*$ && "$MAX_ATTEMPTS" =~ ^[1-9][0-9]*$ ]] || {
    echo "Error: blocks-per-count and max-attempts must be positive integers." >&2; exit 2;
}
[[ "$RECEIPT_TIMEOUT" =~ ^[1-9][0-9]*$ && "$BLOCK_TIMEOUT" =~ ^[1-9][0-9]*$ ]] || {
    echo "Error: receipt and block timeouts must be positive integers." >&2; exit 2;
}
[[ "$POLL_INTERVAL" =~ ^[0-9]+([.][0-9]+)?$ ]] && awk "BEGIN { exit !($POLL_INTERVAL > 0) }" || {
    echo "Error: poll interval must be positive." >&2; exit 2;
}
[[ "$CONTRACT_GAS_LIMIT" =~ ^[1-9][0-9]*$ ]] || {
    echo "Error: contract gas limit must be a positive integer." >&2; exit 2;
}
if [[ ! "$RUN_ID" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]]; then
    echo "Error: EXPERIMENT_RUN_ID contains unsupported characters." >&2
    exit 2
fi

export EB_PRIVATE_KEY="$PRIVATE_KEY"
python3 "$SCRIPT_DIR/run_experiment.py" \
    --validate-only --counts "$COUNTS" --private-key "$PRIVATE_KEY" \
    --workload "$WORKLOAD" \
    --contract-gas-limit "$CONTRACT_GAS_LIMIT" \
    --rpc-url "http://127.0.0.1:8545" --chain-id "$CHAIN_ID" \
    --blocks-per-count "$BLOCKS_PER_COUNT" --max-attempts "$MAX_ATTEMPTS" \
    --receipt-timeout "$RECEIPT_TIMEOUT" --block-timeout "$BLOCK_TIMEOUT" \
    --poll-interval "$POLL_INTERVAL"

if [[ -z "${METRICS_INGEST_TOKEN:-}" ]]; then
    METRICS_INGEST_TOKEN="$(openssl rand -hex 32)"
fi
EXPERIMENT_RUN_ID="$RUN_ID"
METRICS_SERVER_BIND=127.0.0.1
EXPERIMENT_NETWORK_CLEAN_DATA=false
export METRICS_INGEST_TOKEN EXPERIMENT_RUN_ID METRICS_SERVER_BIND RESET_NETWORK EXPERIMENT_NETWORK_CLEAN_DATA
CLIENT="$CLIENT"
CHAIN_ID="$CHAIN_ID"
export CLIENT CHAIN_ID

bash "$NETWORK_SCRIPT" check
mkdir -p "$OUTPUT_ROOT/empty-block"
OUTPUT_DIR="$OUTPUT_ROOT/empty-block/$RUN_ID"
if ! mkdir "$OUTPUT_DIR" 2>/dev/null; then
    echo "Error: output directory already exists: $OUTPUT_DIR" >&2
    exit 1
fi
mkdir -p "$OUTPUT_DIR/logs"

cleanup() {
    local status=$?
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
echo "Starting $CLIENT empty-block network for run $RUN_ID..."
bash "$NETWORK_SCRIPT" start

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

READY=false
for attempt in $(seq 1 90); do
    response="$(curl -fsS --max-time 3 -H 'Content-Type: application/json' \
        --data-binary '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
        http://127.0.0.1:8545 2>/dev/null || true)"
    if python3 -c 'import json,sys; print("ok" if json.loads(sys.argv[1]).get("result") is not None else "")' "$response" 2>/dev/null | grep -q '^ok$'; then
        READY=true
        break
    fi
    sleep 2
done
if [[ "$READY" != true ]]; then
    echo "Error: execution node did not expose JSON-RPC on port 8545." >&2
    exit 1
fi

python3 "$SCRIPT_DIR/capture_metadata.py" \
    --output "$OUTPUT_DIR/metadata.json" --experiment-id "$RUN_ID" \
    --chain-id "$CHAIN_ID" --client-label "$CLIENT_LABEL" --client-variant "$CLIENT" \
    --workload "$WORKLOAD" \
    --counts "$COUNTS" --blocks-per-count "$BLOCKS_PER_COUNT" --max-attempts "$MAX_ATTEMPTS"
NODE_ID="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8")).get("node_id") or "el-node-0")' "$OUTPUT_DIR/metadata.json")"
NETWORK_NAME="$(docker compose -f "$REPO_ROOT/docker-compose.v2.yml" ps -q el-node-0 | xargs -r docker inspect --format '{{range $name, $_ := .NetworkSettings.Networks}}{{$name}}{{end}}')"
if [[ -z "$NETWORK_NAME" ]]; then
    echo "Error: could not determine the empty-block Docker network." >&2
    exit 1
fi

IFS=',' read -r -a count_values <<< "$COUNTS"
MONITOR_DURATION=$(( ${#count_values[@]} * BLOCKS_PER_COUNT * MAX_ATTEMPTS * BLOCK_TIMEOUT + ${#count_values[@]} * RECEIPT_TIMEOUT + 120 ))
python3 "$REPO_ROOT/experiments/runtime/collect_system_metrics.py" \
    --metadata "$OUTPUT_DIR/metadata.json" \
    --output "$OUTPUT_DIR/system_metric_samples.jsonl" \
    --experiment-id "$RUN_ID" --network "$CHAIN_ID" \
    --duration "$MONITOR_DURATION" --interval 1 \
    > "$OUTPUT_DIR/system_collector.stdout" 2> "$OUTPUT_DIR/system_collector.stderr" &
SYSTEM_COLLECTOR_PID=$!

RUN_STATUS=0
python3 "$SCRIPT_DIR/run_experiment.py" \
    --rpc-url "http://127.0.0.1:8545" --cast-rpc-url "http://el-node-0:8545" \
    --cast-network "$NETWORK_NAME" --private-key "$PRIVATE_KEY" \
    --chain-id "$CHAIN_ID" --experiment-id "$RUN_ID" \
    --client "$CLIENT_LABEL" --client-variant "$CLIENT" --node-id "$NODE_ID" \
    --workload "$WORKLOAD" \
    --contract-gas-limit "$CONTRACT_GAS_LIMIT" \
    --counts "$COUNTS" --blocks-per-count "$BLOCKS_PER_COUNT" --max-attempts "$MAX_ATTEMPTS" \
    --receipt-timeout "$RECEIPT_TIMEOUT" --block-timeout "$BLOCK_TIMEOUT" \
    --poll-interval "$POLL_INTERVAL" --output "$OUTPUT_DIR/block_samples.csv" \
    --inclusion-output "$OUTPUT_DIR/transaction_inclusion_samples.csv" \
    || RUN_STATUS=$?

if [[ -n "$SYSTEM_COLLECTOR_PID" ]]; then
    if kill -0 "$SYSTEM_COLLECTOR_PID" 2>/dev/null; then
        kill "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
        wait "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
    else
        wait "$SYSTEM_COLLECTOR_PID" || true
    fi
    SYSTEM_COLLECTOR_PID=""
fi

bash "$SCRIPT_DIR/capture_logs.sh" "$OUTPUT_DIR/logs"
python3 "$SCRIPT_DIR/parse_logs.py" \
    --metadata "$OUTPUT_DIR/metadata.json" --logs-dir "$OUTPUT_DIR/logs" \
    --block-samples "$OUTPUT_DIR/block_samples.csv" --output-dir "$OUTPUT_DIR" \
    --experiment-id "$RUN_ID" --client "$CLIENT_LABEL"
python3 "$SCRIPT_DIR/collect_block_metrics.py" \
    --metadata "$OUTPUT_DIR/metadata.json" --samples "$OUTPUT_DIR/samples.csv" \
    --output-dir "$OUTPUT_DIR" --experiment-id "$RUN_ID"
python3 "$REPO_ROOT/experiments/runtime/collect_node_metrics.py" \
    --metadata "$OUTPUT_DIR/metadata.json" --output "$OUTPUT_DIR/node_metric_coverage.json" \
    --experiment-id "$RUN_ID"
printf '{"status":%s}\n' "$RUN_STATUS" > "$OUTPUT_DIR/run_status.json"

bash "$NETWORK_SCRIPT" stop-metrics
if [[ -f "$REPO_ROOT/data_v2/metrics-server/metrics.redb" ]]; then
    mkdir -p "$OUTPUT_DIR/metrics_server_data"
    cp "$REPO_ROOT/data_v2/metrics-server/metrics.redb" "$OUTPUT_DIR/metrics_server_data/metrics.redb"
else
    echo "Error: metrics-server database was not found; empty-block metrics were not archived." >&2
    exit 1
fi
python3 "$SCRIPT_DIR/report.py" --output-dir "$OUTPUT_DIR"
EXPERIMENT_NETWORK_CLEAN_DATA=true
export EXPERIMENT_NETWORK_CLEAN_DATA
bash "$NETWORK_SCRIPT" stop
NETWORK_STARTED=false

echo "Empty-block experiment results: $OUTPUT_DIR"
if (( RUN_STATUS != 0 )); then
    exit "$RUN_STATUS"
fi