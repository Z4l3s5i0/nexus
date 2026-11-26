#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
WASMER_DIR="${WASMER_DIR:-$HOME/.wasmer}"
export PATH="$CARGO_HOME/bin:$WASMER_DIR/bin:$PATH"
RUN_ID="${EXPERIMENT_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
RESULTS_PARENT="${EXPERIMENTS_OUTPUT_ROOT:-$REPO_ROOT/experiment-results}/startup"
ITERATIONS=5
GETH_IMAGE="ethereum/client-go@sha256:798b7eb1bcef6d4be7576232beea63bf291450f48b025dc8fb5c6e37840e4364"
RETH_IMAGE="ghcr.io/paradigmxyz/reth@sha256:68e76b32ad9ac1a3de54984d6f15bbce2047a92a771ff4efbcd08ee8f9dbd4d1"
NEXUS_LINUX_IMAGE="wasix-eth-linux:latest"
NEXUS_WASIX_IMAGE="wasix-eth-wasix:latest"
NEXUS_WASIX_WASM="${NEXUS_WASIX_WASM:-${EXPERIMENTS_WASIX_ETH_ROOT:-$REPO_ROOT/wasix_eth}/target/wasm32-wasmer-wasi/release/wasix_eth.wasi.wasm}"
WASMER_BIN="wasmer"
CHAIN_ID=12345
GENESIS_GAS_LIMIT="0x1fffffffffffff"
GENESIS_BASE_FEE_PER_GAS="0x3b9aca00"
TERMINAL_TOTAL_DIFFICULTY="0x0"
PREMINE_ADDRESS="f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
PREMINE_BALANCE="0x200000000000000000000000000000000000000000000000000000000000000"
TIMEOUT=60
METRICS_INTERVAL=0.2
CONFIGS=""
OUTPUT_DIR=""
KEEP_FAILED_CONTAINERS=false
RESET_NETWORK=false
METRICS_STARTED=false
SYSTEM_COLLECTOR_PID=""
CURRENT_STATE_FILE=""
RUN_STATUS=0

startup_progress() {
    printf 'Startup progress: %s\n' "$*"
}

usage() {
    cat <<EOF
Usage: bash experiments/startup/run.sh [options]

Options:
  --iterations N          Repetitions per configuration (default: $ITERATIONS)
  --geth-image IMAGE      Geth image
  --reth-image IMAGE      Reth image
  --nexus-linux-image I   NEXUS Linux image
  --nexus-wasix-image I   NEXUS WASIX Docker image
  --nexus-wasix-wasm PATH NEXUS WASIX module for host Wasmer
  --wasmer-bin PATH       Wasmer executable (default: $WASMER_BIN)
  --chain-id ID           Isolated genesis chain ID (default: $CHAIN_ID)
  --genesis-gas-limit N   Genesis gas limit in decimal or hex (default: $GENESIS_GAS_LIMIT)
  --base-fee-per-gas N    Genesis base fee in decimal or hex (default: $GENESIS_BASE_FEE_PER_GAS)
  --terminal-total-difficulty N  Genesis TTD in decimal or hex (default: $TERMINAL_TOTAL_DIFFICULTY)
  --premine-address ADDR  Account funded in genesis (default: $PREMINE_ADDRESS)
  --premine-balance N     Premine balance in wei, decimal or hex (default: $PREMINE_BALANCE)
  --timeout SECONDS       Readiness timeout (default: $TIMEOUT)
  --metrics-interval SEC  System metric sample interval (default: $METRICS_INTERVAL)
  --results-dir DIR       Parent output directory (default: $RESULTS_PARENT)
  --configs LIST          Comma-separated client configs; default selects available Docker configs
  --keep-failed-containers Preserve failed Docker clients for manual RPC/log inspection
  --reset-network         Clear stopped shared-network data before this run
  --help                  Show this help

Available configs: geth, reth, nexus-linux, nexus-wasix-docker, nexus-wasmer
EOF
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --iterations) ITERATIONS="$2"; shift 2 ;;
        --geth-image) GETH_IMAGE="$2"; shift 2 ;;
        --reth-image) RETH_IMAGE="$2"; shift 2 ;;
        --nexus-linux-image) NEXUS_LINUX_IMAGE="$2"; shift 2 ;;
        --nexus-wasix-image) NEXUS_WASIX_IMAGE="$2"; shift 2 ;;
        --nexus-wasix-wasm) NEXUS_WASIX_WASM="$2"; shift 2 ;;
        --wasmer-bin) WASMER_BIN="$2"; shift 2 ;;
        --chain-id) CHAIN_ID="$2"; shift 2 ;;
        --genesis-gas-limit) GENESIS_GAS_LIMIT="$2"; shift 2 ;;
        --base-fee-per-gas) GENESIS_BASE_FEE_PER_GAS="$2"; shift 2 ;;
        --terminal-total-difficulty) TERMINAL_TOTAL_DIFFICULTY="$2"; shift 2 ;;
        --premine-address) PREMINE_ADDRESS="$2"; shift 2 ;;
        --premine-balance) PREMINE_BALANCE="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        --metrics-interval) METRICS_INTERVAL="$2"; shift 2 ;;
        --results-dir) RESULTS_PARENT="$2"; shift 2 ;;
        --configs) CONFIGS="$2"; shift 2 ;;
        --keep-failed-containers) KEEP_FAILED_CONTAINERS=true; shift ;;
        --reset-network) RESET_NETWORK=true; shift ;;
        --help|-h) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

[[ "$ITERATIONS" =~ ^[1-9][0-9]*$ && "$CHAIN_ID" =~ ^[1-9][0-9]*$ ]] || {
    echo "Error: iterations and chain ID must be positive integers." >&2
    exit 2
}
for value in "$GENESIS_GAS_LIMIT" "$GENESIS_BASE_FEE_PER_GAS" "$TERMINAL_TOTAL_DIFFICULTY" "$PREMINE_BALANCE"; do
    [[ "$value" =~ ^(0[xX][0-9a-fA-F]+|[0-9]+)$ ]] || {
        echo "Error: genesis numeric values must be decimal or hexadecimal integers." >&2
        exit 2
    }
done
[[ "$PREMINE_ADDRESS" =~ ^(0x)?[0-9a-fA-F]{40}$ ]] || {
    echo "Error: premine address must be a 20-byte hexadecimal address." >&2
    exit 2
}
[[ "$TIMEOUT" =~ ^[0-9]+([.][0-9]+)?$ ]] && awk "BEGIN { exit !($TIMEOUT > 0) }" || {
    echo "Error: timeout must be positive." >&2
    exit 2
}
[[ "$METRICS_INTERVAL" =~ ^[0-9]+([.][0-9]+)?$ ]] && awk "BEGIN { exit !($METRICS_INTERVAL > 0) }" || {
    echo "Error: metrics interval must be positive." >&2
    exit 2
}
if [[ ! "$RUN_ID" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]]; then
    echo "Error: EXPERIMENT_RUN_ID contains unsupported characters." >&2
    exit 2
fi

if [[ -z "$CONFIGS" ]]; then
    CONFIGS="geth,reth,nexus-linux,nexus-wasix-docker"
    if [[ -f "$NEXUS_WASIX_WASM" ]] && command -v "$WASMER_BIN" >/dev/null 2>&1; then
        CONFIGS+=",nexus-wasmer"
    fi
fi
IFS=',' read -r -a CONFIG_LIST <<< "$CONFIGS"
declare -A SEEN_CONFIGS=()
for config in "${CONFIG_LIST[@]}"; do
    case "$config" in
        geth|reth|nexus-linux|nexus-wasix-docker|nexus-wasmer) ;;
        *) echo "Error: unknown startup configuration '$config'." >&2; exit 2 ;;
    esac
    if [[ -n "${SEEN_CONFIGS[$config]:-}" ]]; then
        echo "Error: duplicate startup configuration '$config'." >&2
        exit 2
    fi
    SEEN_CONFIGS[$config]=1
done
if [[ -n "${SEEN_CONFIGS[nexus-wasmer]:-}" ]]; then
    [[ -n "$NEXUS_WASIX_WASM" && -f "$NEXUS_WASIX_WASM" ]] || {
        echo "Error: nexus-wasmer requires --nexus-wasix-wasm pointing to an existing module." >&2
        exit 2
    }
    command -v "$WASMER_BIN" >/dev/null 2>&1 || {
        echo "Error: Wasmer executable not found: $WASMER_BIN" >&2
        exit 2
    }
fi

if [[ "$RESET_NETWORK" == true ]]; then
    bash "$REPO_ROOT/experiments/shared/network/reset.sh"
fi

if [[ -z "${METRICS_INGEST_TOKEN:-}" ]]; then
    METRICS_INGEST_TOKEN="$(openssl rand -hex 32)"
fi
export METRICS_INGEST_TOKEN EXPERIMENT_RUN_ID="$RUN_ID"

mkdir -p "$RESULTS_PARENT"
OUTPUT_DIR="$RESULTS_PARENT/$RUN_ID"
if ! mkdir "$OUTPUT_DIR" 2>/dev/null; then
    echo "Error: output directory already exists: $OUTPUT_DIR" >&2
    exit 1
fi
mkdir -p "$OUTPUT_DIR/iterations"

cleanup() {
    local status=$?
    if [[ -n "$OUTPUT_DIR" && -d "$OUTPUT_DIR" ]]; then
        python3 "$REPO_ROOT/experiments/record_run_status.py" \
            --output "$OUTPUT_DIR/run_status.json" --exit-code "$status" || true
    fi
    if [[ -n "$SYSTEM_COLLECTOR_PID" ]] && kill -0 "$SYSTEM_COLLECTOR_PID" 2>/dev/null; then
        startup_progress "Stopping system metrics collection."
        kill "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
        wait "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
        startup_progress "System metrics collection stopped."
    fi
    if [[ -n "$CURRENT_STATE_FILE" && -f "$CURRENT_STATE_FILE" ]]; then
        startup_progress "Stopping the active startup client."
        if python3 "$SCRIPT_DIR/stop_client.py" --state "$CURRENT_STATE_FILE"; then
            startup_progress "Active startup client teardown finished."
        else
            startup_progress "Active startup client teardown could not be confirmed."
        fi
    fi
    if [[ "$METRICS_STARTED" == true ]]; then
        startup_progress "Stopping the startup metrics server."
        if bash "$SCRIPT_DIR/metrics_server.sh" stop "$RUN_ID" "$OUTPUT_DIR/metrics_server_data"; then
            startup_progress "Startup metrics server stopped."
        else
            startup_progress "Startup metrics server stop could not be confirmed."
        fi
    fi
    return "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

startup_progress "Starting the startup metrics server."
bash "$SCRIPT_DIR/metrics_server.sh" start "$RUN_ID" "$OUTPUT_DIR/metrics_server_data" > "$OUTPUT_DIR/metrics_server.container"
METRICS_STARTED=true
startup_progress "Startup metrics server started."
startup_progress "Recording startup run metadata."
python3 "$SCRIPT_DIR/metadata.py" --scope run --run-id "$RUN_ID" --chain-id "$CHAIN_ID" \
    --iterations "$ITERATIONS" --configs "$CONFIGS" --output "$OUTPUT_DIR/metadata.json"
startup_progress "Startup run metadata recorded."

COLLECT_DURATION="$(python3 -c 'import math,sys; print(math.ceil(float(sys.argv[1]) + 10))' "$TIMEOUT")"

print_iteration_result() {
    local config="$1" iteration="$2" result_file="$3" client_log="$4"
    local startup_time error
    startup_time="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1], encoding="utf-8")); print(d.get("startup_time_ms") if d.get("ready") and d.get("startup_time_ms") is not None else "unavailable")' "$result_file")"
    if [[ "$startup_time" == unavailable ]]; then
        startup_progress "$config iteration $iteration of $ITERATIONS finished; startup time unavailable."
        error="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1], encoding="utf-8")); print(d.get("error") or "No failure reason was recorded.")' "$result_file")"
        echo "  Failure: $error"
        echo "  Client log: $client_log"
    else
        startup_progress "$config iteration $iteration of $ITERATIONS finished in $startup_time ms."
    fi
}

for config in "${CONFIG_LIST[@]}"; do
    for ((iteration = 1; iteration <= ITERATIONS; iteration++)); do
        iteration_dir="$OUTPUT_DIR/iterations/$config/$iteration"
        work_dir="$iteration_dir/work"
        logs_dir="$iteration_dir/logs"
        state_file="$iteration_dir/state.json"
        result_file="$iteration_dir/result.json"
        identity_file="$iteration_dir/identity.json"
        system_samples="$OUTPUT_DIR/system_metric_samples.jsonl"
        client_log="$logs_dir/client.log"
        mkdir -p "$work_dir" "$logs_dir"
        CURRENT_STATE_FILE="$state_file"

        startup_progress "Preparing $config startup benchmark (iteration $iteration of $ITERATIONS)."
        python3 "$SCRIPT_DIR/generate_genesis.py" --chain-id "$CHAIN_ID" \
            --gas-limit "$GENESIS_GAS_LIMIT" --base-fee-per-gas "$GENESIS_BASE_FEE_PER_GAS" \
            --terminal-total-difficulty "$TERMINAL_TOTAL_DIFFICULTY" \
            --premine-address "$PREMINE_ADDRESS" --premine-balance "$PREMINE_BALANCE" \
            --output "$work_dir/genesis.json"
        python3 "$SCRIPT_DIR/metadata.py" --scope iteration --run-id "$RUN_ID" --chain-id "$CHAIN_ID" \
            --config "$config" --iteration "$iteration" --output "$identity_file"
        PREPARE_STATUS=0
        python3 "$SCRIPT_DIR/benchmark_client.py" \
            --config "$config" --iteration "$iteration" --run-id "$RUN_ID" --chain-id "$CHAIN_ID" \
            --work-dir "$work_dir" --genesis "$work_dir/genesis.json" \
            --state-file "$state_file" --result-file "$result_file" --log-file "$client_log" \
            --rpc-port 0 --timeout "$TIMEOUT" --geth-image "$GETH_IMAGE" --reth-image "$RETH_IMAGE" \
            --nexus-linux-image "$NEXUS_LINUX_IMAGE" --nexus-wasix-image "$NEXUS_WASIX_IMAGE" \
            --nexus-wasix-wasm "$NEXUS_WASIX_WASM" --wasmer-bin "$WASMER_BIN" --prepare-only \
            || PREPARE_STATUS=$?
        if (( PREPARE_STATUS != 0 )); then
            startup_progress "$config client preparation did not complete; recording the result."
            RUN_STATUS=1
            python3 "$SCRIPT_DIR/collect_startup_metrics.py" \
                --result "$result_file" --samples "$OUTPUT_DIR/startup_metric_samples.jsonl" \
                --coverage "$OUTPUT_DIR/startup_time_coverage.json"
            print_iteration_result "$config" "$iteration" "$result_file" "$client_log"
            startup_progress "$config iteration $iteration result recorded."
            CURRENT_STATE_FILE=""
            continue
        fi

        startup_progress "$config client preparation finished; starting the startup measurement."
        RPC_PORT="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["rpc_port"])' "$state_file")"
        python3 "$SCRIPT_DIR/collect_system_metrics.py" \
            --identity "$identity_file" --state-file "$state_file" \
            --output "$system_samples" --duration "$COLLECT_DURATION" \
            --interval "$METRICS_INTERVAL" \
            > "$iteration_dir/system_collector.stdout" 2> "$iteration_dir/system_collector.stderr" &
        SYSTEM_COLLECTOR_PID=$!
        startup_progress "System metrics collection started for $config iteration $iteration."

        BENCHMARK_STATUS=0
        python3 "$SCRIPT_DIR/benchmark_client.py" \
            --config "$config" --iteration "$iteration" --run-id "$RUN_ID" --chain-id "$CHAIN_ID" \
            --work-dir "$work_dir" --genesis "$work_dir/genesis.json" \
            --state-file "$state_file" --result-file "$result_file" --log-file "$client_log" \
            --rpc-port "$RPC_PORT" --timeout "$TIMEOUT" --geth-image "$GETH_IMAGE" --reth-image "$RETH_IMAGE" \
            --nexus-linux-image "$NEXUS_LINUX_IMAGE" --nexus-wasix-image "$NEXUS_WASIX_IMAGE" \
            --nexus-wasix-wasm "$NEXUS_WASIX_WASM" --wasmer-bin "$WASMER_BIN" --skip-preparation \
            || BENCHMARK_STATUS=$?
        if (( BENCHMARK_STATUS != 0 )); then
            RUN_STATUS=1
            startup_progress "$config startup measurement finished with a failure."
        else
            sleep 0.5
            startup_progress "$config startup measurement finished."
        fi

        CONTAINER_NAME="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8")).get("container_name") or "")' "$state_file")"
        if [[ -n "$CONTAINER_NAME" ]]; then
            docker logs "$CONTAINER_NAME" >> "$logs_dir/client.log" 2>&1 || true
        fi
        STOP_CLIENT_ARGS=(--state "$state_file")
        if [[ "$KEEP_FAILED_CONTAINERS" == true ]]; then
            STOP_CLIENT_ARGS+=(--keep-failed)
        fi
        STOP_STATUS=0
        startup_progress "Tearing down the $config startup client."
        python3 "$SCRIPT_DIR/stop_client.py" "${STOP_CLIENT_ARGS[@]}" || STOP_STATUS=$?
        if (( STOP_STATUS != 0 )); then
            RUN_STATUS=1
            startup_progress "$config startup client teardown finished with a failure."
        else
            startup_progress "$config startup client teardown finished."
            CURRENT_STATE_FILE=""
        fi
        if [[ -n "$SYSTEM_COLLECTOR_PID" ]]; then
            startup_progress "Stopping system metrics collection for $config iteration $iteration."
            if kill -0 "$SYSTEM_COLLECTOR_PID" 2>/dev/null; then
                kill "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
                wait "$SYSTEM_COLLECTOR_PID" 2>/dev/null || true
            else
                wait "$SYSTEM_COLLECTOR_PID" || true
            fi
            SYSTEM_COLLECTOR_PID=""
            startup_progress "System metrics collection stopped for $config iteration $iteration."
        fi
        startup_progress "Recording startup metrics for $config iteration $iteration."
        python3 "$SCRIPT_DIR/collect_startup_metrics.py" \
            --result "$result_file" --samples "$OUTPUT_DIR/startup_metric_samples.jsonl" \
            --coverage "$OUTPUT_DIR/startup_time_coverage.json"
        startup_progress "Startup metrics recorded for $config iteration $iteration."
        print_iteration_result "$config" "$iteration" "$result_file" "$client_log"
        startup_progress "$config iteration $iteration result recorded."
    done
done

startup_progress "Stopping the startup metrics server."
bash "$SCRIPT_DIR/metrics_server.sh" stop "$RUN_ID" "$OUTPUT_DIR/metrics_server_data"
METRICS_STARTED=false
startup_progress "Startup metrics server stopped."
if [[ ! -f "$OUTPUT_DIR/metrics_server_data/metrics.redb" ]]; then
    echo "Error: startup metrics database was not archived." >&2
    exit 1
fi
startup_progress "Generating the startup benchmark report."
python3 "$SCRIPT_DIR/report.py" --output-dir "$OUTPUT_DIR" --iterations "$ITERATIONS"
startup_progress "Startup benchmark report generated."
echo "Startup benchmark results: $OUTPUT_DIR"
if (( RUN_STATUS != 0 )); then
    exit 1
fi