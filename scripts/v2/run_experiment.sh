#!/bin/bash
set -e

# Experiment Automation Script
# Orchestrates the full lifecycle of an experiment run.

CHAIN_ID=10
WORKLOAD_DURATION=300 # Default workload duration in seconds
WAIT_DURATION=1800 # Default wait duration after workload
LINUX_NODES=5
WASIX_NODES=5
GETH_NODES=0
METRICS_IMAGE=""
WORKLOAD_PERCENT=""
WORKLOAD_TYPE="exp/uber"
SPAMMER_ACCOUNTS=4
TPS=4

usage() {
    echo "Usage: $0 [options]"
    echo "Options:"
    echo "  --duration N     Duration to run the workload in seconds (default: $WORKLOAD_DURATION)"
    echo "  --wait-duration N Wait duration after workload in seconds (default: $WAIT_DURATION)"
    echo "  --chain-id ID    Chain ID to use (default: $CHAIN_ID)"
    echo "  --linux-nodes N  Number of rustcompiled nodes (default: $LINUX_NODES)"
    echo "  --wasix-nodes N  Number of WASM-compiled nodes (default: $WASIX_NODES)"
    echo "  --geth-nodes N   Number of Geth nodes (default: $GETH_NODES)"
    echo "  --metrics-image IMAGE Use an existing metrics image instead of building locally"
    echo "  --workload-percent N Percent of nodes to receive workload (default: 100)"
    echo "  --workload-type TYPE Workload type/scenario (default: $WORKLOAD_TYPE)"
    echo "  --spammer-accounts N Number of accounts per spammer (default: $SPAMMER_ACCOUNTS)"
    echo "  --tps N          Transactions per second (default: $TPS)"
    exit 1
}

while [[ "$#" -gt 0 ]]; do
    case $1 in
        --duration) WORKLOAD_DURATION="$2"; shift ;;
        --wait-duration) WAIT_DURATION="$2"; shift ;;
        --chain-id) CHAIN_ID="$2"; shift ;;
        --linux-nodes) LINUX_NODES="$2"; shift ;;
        --wasix-nodes) WASIX_NODES="$2"; shift ;;
        --geth-nodes) GETH_NODES="$2"; shift ;;
        --metrics-image) METRICS_IMAGE="$2"; shift ;;
        --workload-percent) WORKLOAD_PERCENT="$2"; shift ;;
        --workload-type) WORKLOAD_TYPE="$2"; shift ;;
        --spammer-accounts) SPAMMER_ACCOUNTS="$2"; shift ;;
        --tps) TPS="$2"; shift ;;
        *) usage ;;
    esac
    shift
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/common.sh"

# If WORKLOAD_PERCENT is not set, default to 100
if [ -z "$WORKLOAD_PERCENT" ]; then
    WORKLOAD_PERCENT=100
fi

# Validate counts and allocate targets with largest-remainder rounding.
ALLOCATION=$(python3 "$SCRIPT_DIR/workload_allocation.py" "$LINUX_NODES" "$GETH_NODES" "$WASIX_NODES" "$WORKLOAD_PERCENT")
read -r NODES WORKLOAD_NODES w_linux w_geth w_wasix <<< "$ALLOCATION"
c_linux=$((10#$LINUX_NODES))
c_geth=$((10#$GETH_NODES))
c_wasix=$((10#$WASIX_NODES))

echo "=== Starting Experiment Workflow ==="
echo "Chain ID: $CHAIN_ID"
echo "Workload Duration: $WORKLOAD_DURATION seconds"
echo "Wait Duration: $WAIT_DURATION seconds"
echo "Nodes: $NODES (Linux: $c_linux, Geth: $c_geth, WASM: $c_wasix)"
echo "Workload Nodes: $WORKLOAD_NODES (Linux: $w_linux, Geth: $w_geth, WASM: $w_wasix)"

# 1. Start the local network
echo "Step 1: Starting local network..."
START_OPTIONS=(--chain-id "$CHAIN_ID" --linux-nodes "$c_linux" --geth-nodes "$c_geth" --wasix-nodes "$c_wasix" --setup --cleanup --registry docker.io/z4l3s5i0)
if [ -n "$METRICS_IMAGE" ]; then START_OPTIONS+=(--metrics-image "$METRICS_IMAGE"); fi
bash "$SCRIPT_DIR/start_local.sh" "${START_OPTIONS[@]}"
COMPOSE_CMD=$(get_compose_cmd)
FIRST_EL=$($COMPOSE_CMD -f docker-compose.v2.yml ps -q el-node-0)
NETWORK_NAME=$(docker inspect --format '{{range $name, $_ := .NetworkSettings.Networks}}{{$name}}{{end}}' "$FIRST_EL")
if [ -z "$NETWORK_NAME" ]; then
    echo "Error: could not determine the experiment execution network." >&2
    exit 1
fi

# 2. Start manage_logs
echo "Step 2: Starting background log processor..."
bash "$SCRIPT_DIR/manage_logs.sh" start --chain-id "$CHAIN_ID"

# 3. Wait for network stabilization
# Geth nodes need more time for state trie initialization, especially with Path-based state scheme
# Calculate wait time based on node count and type
if [ "$c_geth" -gt 0 ]; then
    # Geth nodes: 120s base + 10s per Geth node
    STABILIZATION_WAIT=$((120 + c_geth * 10))
else
    # Non-Geth nodes: 60s is sufficient
    STABILIZATION_WAIT=60
fi
echo "Step 3: Waiting $STABILIZATION_WAIT seconds for network stabilization..."
sleep "$STABILIZATION_WAIT"

# 4. Start run_workload for each node in background
echo "Step 4: Starting workload for $WORKLOAD_NODES node(s) in background ($w_linux Linux, $w_geth Geth, $w_wasix WASM)..."
MNEMONIC="sleep moment list remain like wall lake industry canvas wonder ecology elite duck salad naive syrup frame brass utility club odor country obey pudding"
WORKLOAD_PIDS=()
BASE_PORT=8545
RUN_ID="${CHAIN_ID}-$(date +%s)-$$"

# Target indices
TARGET_INDICES=()
# Node indices follow native, Geth, WASIX order.
for ((i = 0; i < w_linux; i++)); do
    TARGET_INDICES+=("$i")
done
for ((i = 0; i < w_geth; i++)); do
    TARGET_INDICES+=("$((c_linux + i))")
done
for ((i = 0; i < w_wasix; i++)); do
    TARGET_INDICES+=("$((c_linux + c_geth + i))")
done

# State readiness check function — uses eth_syncing to detect when a node is ready to process transactions
# For private chains with no peers, Geth may report as syncing even when ready, so we also check
# if the node is responsive and on the correct chain
wait_for_node_state_ready() {
    local rpc_url=$1
    local node_index=$2
    
    echo "Checking state readiness for node $node_index at $rpc_url..."
    local max_attempts=60
    for attempt in $(seq 1 $max_attempts); do
        # Check if the node is syncing
        local sync_response
        sync_response=$(curl -fsS --max-time 3 -H 'Content-Type: application/json' \
            --data-binary '{"jsonrpc":"2.0","method":"eth_syncing","params":[],"id":1}' \
            "$rpc_url" 2>/dev/null || true)
        
        # If eth_syncing returns false, the node is fully synced and ready
        if echo "$sync_response" | python3 -c "import json,sys; d=json.load(sys.stdin); assert d.get('result') == False" 2>/dev/null; then
            echo "Node $node_index is fully synced and ready."
            return 0
        fi
        
        # If eth_syncing returns a dict (snap sync in progress), check if state sync is complete
        # During snap sync with Path-based state scheme, the response includes 'stateHealingDone'
        local snap_sync_done
        snap_sync_done=$(echo "$sync_response" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('result')
if isinstance(d, dict):
    # Path-based state scheme: check stateHealingDone
    print('true' if d.get('stateHealingDone', False) else 'false')
else:
    print('false')
" 2>/dev/null || echo "false")
        
        if [ "$snap_sync_done" = "true" ]; then
            echo "Node $node_index snap sync state healing is done."
            return 0
        fi
        
        # For private chains with no peers, check if the node is responsive and on the correct chain
        # If current block matches highest block, the node is not actively syncing and is ready
        local sync_status
        sync_status=$(echo "$sync_response" | python3 -c "
import json, sys
d = json.load(sys.stdin).get('result')
if isinstance(d, dict):
    current_block = int(d.get('currentBlock', 0), 16) if isinstance(d.get('currentBlock'), str) else d.get('currentBlock', 0)
    highest_block = int(d.get('highestBlock', 0), 16) if isinstance(d.get('highestBlock'), str) else d.get('highestBlock', 0)
    # If current block equals highest block, the node is not actively syncing
    print('ready' if current_block >= highest_block else 'syncing')
else:
    print('unknown')
" 2>/dev/null || echo "unknown")
        
        if [ "$sync_status" = "ready" ]; then
            echo "Node $node_index is not actively syncing (current block matches highest block)."
            return 0
        fi
        
        # Periodically log progress
        if [ $((attempt % 30)) -eq 0 ]; then
            echo "Node $node_index still syncing (attempt $attempt/$max_attempts)..."
        fi
        
        if [ "$attempt" -eq "$max_attempts" ]; then
            echo "Warning: Node $node_index state readiness check timed out after $((max_attempts * 2)) seconds. Proceeding anyway..."
            return 1
        fi
        sleep 2
    done
}

for i in "${TARGET_INDICES[@]}"; do
    RPC_PORT=$((BASE_PORT + i))
    RPC_URL="http://el-node-$i:$RPC_PORT"
    
    # Deriving private key for the account index i
    echo "Deriving private key for node $i..."
    PRIV_KEY=$(docker run --rm ghcr.io/foundry-rs/foundry:latest "cast wallet private-key \"$MNEMONIC\" $i" | sed -n 's/.*0x\([0-9a-fA-F]\{64\}\).*/\1/p')

    # Validate PRIV_KEY length (should be 64 for raw hex)
    if [ ${#PRIV_KEY} -ne 64 ]; then
        echo "Error: Invalid private key derived for node $i (length ${#PRIV_KEY}, expected 64): '$PRIV_KEY'"
        exit 1
    fi

    # Wait for this node's state to be ready before launching workload
    wait_for_node_state_ready "$RPC_URL" "$i"

    echo "Launching workload for node $i at $RPC_URL with seed node$i"
    # We derive a deterministic seed based on the node index
    # to avoid collisions when using the same state directory.
    SEED="0x$(printf "node$i" | sha256sum | cut -d' ' -f1)"

    # Separate state directory per node to avoid conflicts during setup
    NODE_STATE_DIR=".contender_state/node_$i"

    # We pass --duration slightly longer than WORKLOAD_DURATION to ensure it doesn't exit early on its own
    # although we will kill it anyway.
    # We also redirect stdin from /dev/null because run_workload.sh uses docker run -it which needs a TTY or will fail without stdin
    bash "$SCRIPT_DIR/run_workload.sh" \
        --pk "$PRIV_KEY" \
        --seed "$SEED" \
        --duration $((WORKLOAD_DURATION)) \
        --network "$NETWORK_NAME" \
        --scenario "$WORKLOAD_TYPE" \
        --tps "$TPS" \
        --accounts "$SPAMMER_ACCOUNTS" \
        --run-id "$RUN_ID" \
        --min-balance "100eth" \
        --state-dir "$NODE_STATE_DIR" \
        --rpc "$RPC_URL" < /dev/null &
    WORKLOAD_PIDS+=($!)
done
echo "Workloads started with PIDs: ${WORKLOAD_PIDS[*]}"

# 5. Wait for the specified duration
echo "Step 5: Running workload for $WORKLOAD_DURATION seconds... + $WAIT_DURATION additional seconds for workload to be processed"
sleep "$WORKLOAD_DURATION"
sleep "$WAIT_DURATION"

# 6. Stop run_workload
echo "Step 6: Stopping workloads..."
for PID in "${WORKLOAD_PIDS[@]}"; do
    if kill -0 "$PID" 2>/dev/null; then
        # Use pkill to kill the whole process tree of run_workload.sh if possible
        pkill -P "$PID" 2>/dev/null || true
        kill "$PID" 2>/dev/null || true
    fi
done

# Only stop contender containers started for this experiment.
CONTENDER_CONTAINERS=$(docker ps --filter "label=wasix-evm.experiment=$RUN_ID" --format "{{.ID}}")
if [ -n "$CONTENDER_CONTAINERS" ]; then
    echo "Stopping contender containers..."
    docker stop $CONTENDER_CONTAINERS || true
fi
echo "Workloads stopped."

# 7. Wait another 60 seconds
echo "Step 7: Waiting additional 60 seconds for the workload to be processed..."
sleep 60

# 8. Stop manage_logs
echo "Step 8: Stopping log processor..."
bash "$SCRIPT_DIR/manage_logs.sh" stop
sync || true
sleep 5

# 9. Collect results with sudo
echo "Step 9: Collecting experiment results..."
TARGETS_CSV=$(IFS=,; echo "${TARGET_INDICES[*]}")
python3 "$SCRIPT_DIR/write_experiment_metadata.py" \
    --chain-id "$CHAIN_ID" --linux "$c_linux" --geth "$c_geth" --wasix "$c_wasix" \
    --scenario "$WORKLOAD_TYPE" --duration "$WORKLOAD_DURATION" --wait-duration "$WAIT_DURATION" \
    --tps "$TPS" --accounts "$SPAMMER_ACCOUNTS" --percent "$WORKLOAD_PERCENT" \
    --targets "$TARGETS_CSV" --output data_v2/experiment_metadata.json
COLLECT_OUTPUT=$(sudo bash "$SCRIPT_DIR/collect_results.sh")
echo "$COLLECT_OUTPUT"
EXP_DIR_PATH=$(echo "$COLLECT_OUTPUT" | grep "RESULT_DIR=" | cut -d'=' -f2)

## 10. Analyze experiment results
#if [ -n "$EXP_DIR_PATH" ] && [ -d "$EXP_DIR_PATH" ]; then
#    echo "Step 10: Analyzing experiment results..."
#    bash "$SCRIPT_DIR/analyze_experiment.sh" "$EXP_DIR_PATH" "$CHAIN_ID"
#else
#    echo "Warning: Could not determine experiment directory for analysis."
#fi

# 11. Start cleanup
echo "Step 11: Cleaning up..."
bash "$SCRIPT_DIR/cleanup.sh"

echo "=== Experiment Workflow Completed ==="
