#!/bin/bash
set -e

# Startup Time Benchmark Script
# Measures cold-start time for different EVM client configurations:
#   - geth (Docker)
#   - reth (Docker)
#   - nexus linux (Docker)  — Rust-compiled nexus inside Docker
#   - nexus wasix + Docker  — WASM-compiled nexus run via Wasmer inside Docker
#   - nexus wasix + Wasmer  — WASM-compiled nexus run via Wasmer directly (no Docker)
#
# For each configuration the script:
#   1. Starts the process from a cold state
#   2. Measures the wall-clock time until the JSON-RPC endpoint becomes responsive
#   3. Repeats the measurement a configurable number of iterations
#   4. Reports min / max / mean / median startup time

# ── Defaults ──────────────────────────────────────────────────────────────────

ITERATIONS=5
GETH_IMAGE="ethereum/client-go@sha256:798b7eb1bcef6d4be7576232beea63bf291450f48b025dc8fb5c6e37840e4364"
RETH_IMAGE="ghcr.io/paradigmxyz/reth:latest"
NEXUS_LINUX_IMAGE="docker.io/z4l3s5i0/wasix-eth-linux:clean"       # e.g. docker.io/z4l3s5i0/wasix-eth-linux:latest
NEXUS_WASIX_IMAGE="docker.io/z4l3s5i0/wasix-eth-wasix:clean"       # e.g. docker.io/z4l3s5i0/wasix-eth-wasix:latest
NEXUS_WASIX_WASM=""        # path to wasix_eth.wasi.wasm (for bare wasmer)
WASMER_BIN="wasmer"        # path to wasmer binary
CHAIN_ID=12345
RESULTS_DIR="startup_benchmarks"
LOG_FILE=""

# ── Usage ─────────────────────────────────────────────────────────────────────

usage() {
    cat <<EOF
Usage: $0 [options]

Measure Docker / Wasmer cold-start time for various EVM client configurations.

Options:
  --iterations N         Number of iterations per config (default: $ITERATIONS)
  --geth-image IMAGE     Geth Docker image (default: $GETH_IMAGE)
  --reth-image IMAGE     Reth Docker image (default: $RETH_IMAGE)
  --nexus-linux-image I  Nexus Linux Docker image (required for linux tests)
  --nexus-wasix-image I  Nexus Wasix Docker image (required for wasix+docker tests)
  --nexus-wasix-wasm P   Path to wasix_eth.wasi.wasm (required for wasix+wasmer tests)
  --wasmer-bin P         Path to wasmer binary (default: $WASMER_BIN)
  --chain-id ID          Chain ID for genesis (default: $CHAIN_ID)
  --results-dir DIR      Output directory (default: $RESULTS_DIR)
  --configs CFGS         Comma-separated list of configs to run.
                         Available: geth, reth, nexus-linux, nexus-wasix-docker, nexus-wasmer
                         Default: all (that have images set)
  --help                 Show this help message.
EOF
    exit 0
}

# ── Argument parsing ──────────────────────────────────────────────────────────

CONFIGS=""

while [[ "$#" -gt 0 ]]; do
    case $1 in
        --iterations) ITERATIONS="$2"; shift ;;
        --geth-image) GETH_IMAGE="$2"; shift ;;
        --reth-image) RETH_IMAGE="$2"; shift ;;
        --nexus-linux-image) NEXUS_LINUX_IMAGE="$2"; shift ;;
        --nexus-wasix-image) NEXUS_WASIX_IMAGE="$2"; shift ;;
        --nexus-wasix-wasm) NEXUS_WASIX_WASM="$2"; shift ;;
        --wasmer-bin) WASMER_BIN="$2"; shift ;;
        --chain-id) CHAIN_ID="$2"; shift ;;
        --results-dir) RESULTS_DIR="$2"; shift ;;
        --configs) CONFIGS="$2"; shift ;;
        --help) usage ;;
        *) echo "Error: unknown option: $1" >&2; usage ;;
    esac
    shift
done

# ── Determine which configs to run ────────────────────────────────────────────

ALL_CONFIGS="geth reth nexus-linux nexus-wasix-docker nexus-wasmer"

if [ -z "$CONFIGS" ]; then
    # Auto-select based on available images
    CONFIGS=""
    echo "Auto-selecting configurations based on available images..."
    for cfg in $ALL_CONFIGS; do
        case $cfg in
            geth) CONFIGS="$CONFIGS geth" ;;
            reth) CONFIGS="$CONFIGS reth" ;;
            nexus-linux)
                if [ -n "$NEXUS_LINUX_IMAGE" ]; then
                    CONFIGS="$CONFIGS nexus-linux"
                else
                    echo "  Skipping nexus-linux: --nexus-linux-image not set"
                fi
                ;;
            nexus-wasix-docker)
                if [ -n "$NEXUS_WASIX_IMAGE" ]; then
                    CONFIGS="$CONFIGS nexus-wasix-docker"
                else
                    echo "  Skipping nexus-wasix-docker: --nexus-wasix-image not set"
                fi
                ;;
            nexus-wasmer)
                if [ -n "$NEXUS_WASIX_WASM" ] && command -v "$WASMER_BIN" &>/dev/null; then
                    CONFIGS="$CONFIGS nexus-wasmer"
                else
                    echo "  Skipping nexus-wasmer: --nexus-wasix-wasm not set or wasmer not found"
                fi
                ;;
        esac
    done
    CONFIGS=$(echo "$CONFIGS" | xargs)  # trim whitespace
fi

if [ -z "$CONFIGS" ]; then
    echo "Error: no configurations to run. Provide at least one image via flags." >&2
    exit 1
fi

echo "Configurations to benchmark: $CONFIGS"

# ── Helpers ───────────────────────────────────────────────────────────────────

mkdir -p "$RESULTS_DIR"

# Generate a minimal genesis.json for a single-node network
generate_genesis() {
    local out_dir="$1"
    mkdir -p "$out_dir"
    cat > "$out_dir/genesis.json" <<GENESIS
{
  "config": {
    "chainId": $CHAIN_ID,
    "ethash": {},
    "clique": { "period": 1, "epoch": 30000 },
    "homesteadBlock": 0,
    "eip150Block": 0,
    "eip155Block": 0,
    "eip158Block": 0,
    "byzantiumBlock": 0,
    "constantinopleBlock": 0,
    "petersburgBlock": 0,
    "istanbulBlock": 0,
    "muirGlacierBlock": 0,
    "berlinBlock": 0,
    "londonBlock": 0,
    "shanghaiTime": 0,
    "cancelContracts": {}
  },
  "difficulty": "1",
  "gasLimit": "0x1fffffffffffff",
  "alloc": {
    "f39Fd6e51aad88F6F4ce6aB8827279cffFb92266": {
      "balance": "0x200000000000000000000000000000000000000000000000000000000000000"
    }
  }
}
GENESIS
}

# Check if a JSON-RPC endpoint responds to eth_chainId
rpc_ready() {
    local url="$1"
    local body='{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}'
    curl -sf --max-time 2 -H 'Content-Type: application/json' \
        --data-binary "$body" "$url" 2>/dev/null | \
        python3 -c "import sys,json; d=json.load(sys.stdin); assert 'result' in d" 2>/dev/null
}

# Get docker compose command
get_compose_cmd() {
    if docker compose version &>/dev/null; then
        echo "docker compose"
    elif docker-compose version &>/dev/null; then
        echo "docker-compose"
    else
        echo "docker compose"
    fi
}

# Compute statistics from a list of milliseconds (one per line)
compute_stats() {
    python3 -c '
import sys, statistics
vals = [float(x) for x in sys.stdin if x.strip()]
if not vals:
    print("no data")
    sys.exit(1)
vals.sort()
print(f"  iterations: {len(vals)}")
print(f"  min:   {vals[0]:.1f} ms")
print(f"  max:   {vals[-1]:.1f} ms")
print(f"  mean:  {statistics.mean(vals):.1f} ms")
print(f"  median:{statistics.median(vals):.1f} ms")
if len(vals) >= 2:
    print(f"  stdev: {statistics.stdev(vals):.1f} ms")
'
}

# ── Benchmark functions ───────────────────────────────────────────────────────

# Each function receives a temp dir and an iteration number.
# It starts the node, waits for RPC, records time in ms, and stops the node.
# The time in ms is printed to stdout.

bench_geth() {
    local work_dir="$1"
    local genesis="$work_dir/genesis.json"
    local data_dir="$work_dir/geth-data"
    local jwt_hex="$work_dir/jwt.hex"
    mkdir -p "$data_dir"
    openssl rand -hex 32 > "$jwt_hex"
    local rpc_port=$((8545 + RANDOM % 1000))

    # Init genesis (not timed — setup only)
    docker run --rm \
        -v "$work_dir:/data" \
        "$GETH_IMAGE" \
        --datadir=/data/geth-data init /data/genesis.json >/dev/null 2>&1

    # Time the full docker run + RPC ready
    local start_ms end_ms elapsed_ms
    start_ms=$(date +%s%3N)

    docker run --rm -d \
        --name bench-geth \
        -v "$work_dir:/data" \
        -p "$rpc_port:8545" \
        "$GETH_IMAGE" \
        --datadir=/data/geth-data \
        --networkid="$CHAIN_ID" \
        --http --http.addr=0.0.0.0 --http.port=8545 \
        --http.api=eth,net,web3 \
        --http.vhosts='*' \
        --authrpc.addr=0.0.0.0 --authrpc.port=8551 \
        --authrpc.vhosts='*' \
        --authrpc.jwtsecret=/data/jwt.hex \
        --maxpeers=0 \
        --nodiscover \
        >/dev/null 2>&1

    for i in $(seq 1 120); do
        if rpc_ready "http://127.0.0.1:$rpc_port"; then
            break
        fi
        sleep 0.5
    done
    end_ms=$(date +%s%3N)
    elapsed_ms=$((end_ms - start_ms))

    docker stop bench-geth >/dev/null 2>&1 || true
    docker rm bench-geth >/dev/null 2>&1 || true
    echo "$elapsed_ms"
}

bench_reth() {
    local work_dir="$1"
    local genesis="$work_dir/genesis.json"
    local data_dir="$work_dir/reth-data"
    local jwt_hex="$work_dir/jwt.hex"
    mkdir -p "$data_dir"
    openssl rand -hex 32 > "$jwt_hex"
    local rpc_port=$((8545 + RANDOM % 1000))

    # Init genesis (not timed — setup only)
    docker run --rm \
        -v "$work_dir:/data" \
        "$RETH_IMAGE" \
        init \
        --chain /data/genesis.json \
        --datadir /data/reth-data \
        >/dev/null 2>&1 || true

    # Time the full docker run + RPC ready
    local start_ms end_ms elapsed_ms
    start_ms=$(date +%s%3N)

    docker run --rm -d \
        --name bench-reth \
        -v "$work_dir:/data" \
        -p "$rpc_port:8545" \
        "$RETH_IMAGE" \
        node \
        --datadir /data/reth-data \
        --http \
        --http.addr 0.0.0.0 \
        --http.port 8545 \
        --authrpc.addr 0.0.0.0 \
        --authrpc.port 8551 \
        --authrpc.jwtsecret /data/jwt.hex \
        --max-outbound-peers 0 \
        --max-inbound-peers 0 \
        >/dev/null 2>&1

    for i in $(seq 1 120); do
        if rpc_ready "http://127.0.0.1:$rpc_port"; then
            break
        fi
        sleep 0.5
    done
    end_ms=$(date +%s%3N)
    elapsed_ms=$((end_ms - start_ms))

    docker stop bench-reth >/dev/null 2>&1 || true
    docker rm bench-reth >/dev/null 2>&1 || true
    echo "$elapsed_ms"
}

bench_nexus_linux() {
    local work_dir="$1"
    local genesis="$work_dir/genesis.json"
    local data_dir="$work_dir/nexus-data"
    local jwt_hex="$work_dir/jwt.hex"
    mkdir -p "$data_dir"
    openssl rand -hex 32 > "$jwt_hex"
    local rpc_port=$((8545 + RANDOM % 1000))

    # Time the full docker run + RPC ready
    local start_ms end_ms elapsed_ms
    start_ms=$(date +%s%3N)

    docker run --rm -d \
        --name bench-nexus-linux \
        -v "$work_dir:/data" \
        -p "$rpc_port:$rpc_port" \
        "$NEXUS_LINUX_IMAGE" \
        --data-dir /data/nexus-data \
        --genesis-path /data/genesis.json \
        --eth-rpc-port "$rpc_port" \
        --auth-rpc-port "$((rpc_port + 1))" \
        --auth-rpc-jwt-path /data/jwt.hex \
        --verbose 0 \
        >/dev/null 2>&1

    for i in $(seq 1 120); do
        if rpc_ready "http://127.0.0.1:$rpc_port"; then
            break
        fi
        sleep 0.5
    done
    end_ms=$(date +%s%3N)
    elapsed_ms=$((end_ms - start_ms))

    docker stop bench-nexus-linux >/dev/null 2>&1 || true
    docker rm bench-nexus-linux >/dev/null 2>&1 || true
    echo "$elapsed_ms"
}

bench_nexus_wasix_docker() {
    local work_dir="$1"
    local genesis="$work_dir/genesis.json"
    local data_dir="$work_dir/nexus-data"
    local jwt_hex="$work_dir/jwt.hex"
    mkdir -p "$data_dir"
    openssl rand -hex 32 > "$jwt_hex"
    local rpc_port=$((8545 + RANDOM % 1000))

    # Time the full docker run + wasmer init + wasmer run + RPC ready
    local start_ms end_ms elapsed_ms
    start_ms=$(date +%s%3N)

    # The wasix Dockerfile has wasmer + the wasm binary.
    # We override entrypoint to run init then run.
    docker run --rm -d \
        --name bench-nexus-wasix-docker \
        -v "$work_dir:/data" \
        -p "$rpc_port:$rpc_port" \
        "$NEXUS_WASIX_IMAGE" \
        sh -c "wasmer run /app/wasix_eth.wasm --enable-async-threads --net \
            --volume /data/nexus-data:/app/data \
            --volume /data:/app/startup \
            -- init --data-dir /app/data --genesis-path /app/startup/genesis.json \
            --eth-rpc-port $rpc_port --auth-rpc-port $((rpc_port + 1)) \
            --auth-rpc-jwt-path /app/startup/jwt.hex --verbose 0 && \
            wasmer run /app/wasix_eth.wasm --enable-async-threads --net \
            --volume /data/nexus-data:/app/data \
            --volume /data:/app/startup \
            -- run --data-dir /app/data --genesis-path /app/startup/genesis.json \
            --eth-rpc-port $rpc_port --auth-rpc-port $((rpc_port + 1)) \
            --auth-rpc-jwt-path /app/startup/jwt.hex --verbose 0" \
        >/dev/null 2>&1

    for i in $(seq 1 120); do
        if rpc_ready "http://127.0.0.1:$rpc_port"; then
            break
        fi
        sleep 0.5
    done
    end_ms=$(date +%s%3N)
    elapsed_ms=$((end_ms - start_ms))

    docker stop bench-nexus-wasix-docker >/dev/null 2>&1 || true
    docker rm bench-nexus-wasix-docker >/dev/null 2>&1 || true
    echo "$elapsed_ms"
}

bench_nexus_wasmer() {
    local work_dir="$1"
    local genesis="$work_dir/genesis.json"
    local data_dir="$work_dir/nexus-data"
    local jwt_hex="$work_dir/jwt.hex"
    mkdir -p "$data_dir"
    openssl rand -hex 32 > "$jwt_hex"
    local rpc_port=$((8545 + RANDOM % 1000))

    # Step 1: init in foreground (not timed — setup only)
    "$WASMER_BIN" run "$NEXUS_WASIX_WASM" --enable-async-threads --net \
        --volume "$data_dir:/app/data" \
        --volume "$work_dir:/app/startup" \
        -- init --data-dir /app/data --genesis-path /app/startup/genesis.json \
        --eth-rpc-port "$rpc_port" --auth-rpc-port "$((rpc_port + 1))" \
        --auth-rpc-jwt-path /app/startup/jwt.hex --verbose 0 \
        >/dev/null 2>&1

    # Step 2: start the run process and time it
    local start_ms end_ms elapsed_ms
    start_ms=$(date +%s%3N)

    "$WASMER_BIN" run "$NEXUS_WASIX_WASM" --enable-async-threads --net \
        --volume "$data_dir:/app/data" \
        --volume "$work_dir:/app/startup" \
        -- run --data-dir /app/data --genesis-path /app/startup/genesis.json \
        --eth-rpc-port "$rpc_port" --auth-rpc-port "$((rpc_port + 1))" \
        --auth-rpc-jwt-path /app/startup/jwt.hex --verbose 0 \
        >/dev/null 2>&1 &
    local run_pid=$!

    for i in $(seq 1 120); do
        if rpc_ready "http://127.0.0.1:$rpc_port"; then
            break
        fi
        sleep 0.5
    done
    end_ms=$(date +%s%3N)
    elapsed_ms=$((end_ms - start_ms))

    kill "$run_pid" 2>/dev/null || true
    wait "$run_pid" 2>/dev/null || true
    echo "$elapsed_ms"
}

# ── Main loop ─────────────────────────────────────────────────────────────────

echo ""
echo "=============================================="
echo "  Startup Time Benchmark"
echo "=============================================="
echo "Iterations per config: $ITERATIONS"
echo "Results directory:     $RESULTS_DIR"
echo "Chain ID:              $CHAIN_ID"
echo "Configs:               $CONFIGS"
echo "=============================================="
echo ""

# CSV header
csv_file="$RESULTS_DIR/results.csv"
echo "config,iteration,time_ms" > "$csv_file"

summary_file="$RESULTS_DIR/summary.txt"
> "$summary_file"

for cfg in $CONFIGS; do
    echo "----------------------------------------------"
    echo "Benchmarking: $cfg"
    echo "----------------------------------------------"

    times=()
    for iter in $(seq 1 "$ITERATIONS"); do
        work_dir=$(mktemp -d "$RESULTS_DIR/work_XXXXXX")
        generate_genesis "$work_dir"

        echo -n "  Iteration $iter/$ITERATIONS ... "

        # Run the appropriate benchmark function
        case $cfg in
            geth) result=$(bench_geth "$work_dir") ;;
            reth) result=$(bench_reth "$work_dir") ;;
            nexus-linux) result=$(bench_nexus_linux "$work_dir") ;;
            nexus-wasix-docker) result=$(bench_nexus_wasix_docker "$work_dir") ;;
            nexus-wasmer) result=$(bench_nexus_wasmer "$work_dir") ;;
            *) echo "Unknown config: $cfg" >&2; rm -rf "$work_dir"; continue ;;
        esac

        echo "${result} ms"
        times+=("$result")
        echo "$cfg,$iter,$result" >> "$csv_file"

        rm -rf "$work_dir"
    done

    echo ""
    echo "  Results for $cfg:"
    printf '%s\n' "${times[@]}" | compute_stats | while read -r line; do
        echo "    $line"
    done
    echo ""

    # Write summary
    {
        echo "=== $cfg ==="
        printf '%s\n' "${times[@]}" | compute_stats
        echo ""
    } >> "$summary_file"
done

echo "=============================================="
echo "  Benchmark Complete"
echo "=============================================="
echo ""
echo "Summary:   $summary_file"
echo "CSV data:  $csv_file"
echo ""
cat "$summary_file"
