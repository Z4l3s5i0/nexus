#!/bin/bash
set -e

# Experiment Automation Script
# Orchestrates the full lifecycle of an experiment run.

CHAIN_ID=10
WORKLOAD_DURATION=300 # Default workload duration in seconds
WAIT_DURATION=1800 # Default wait duration after workload
NODES=10
RATIO=0
WORKLOAD_PERCENT=""

usage() {
    echo "Usage: $0 [options]"
    echo "Options:"
    echo "  --duration N     Duration to run the workload in seconds (default: $WORKLOAD_DURATION)"
    echo "  --wait-duration N Wait duration after workload in seconds (default: $WAIT_DURATION)"
    echo "  --chain-id ID    Chain ID to use (default: $CHAIN_ID)"
    echo "  --nodes N        Number of nodes to start (default: $NODES)"
    echo "  --ratio N        Percent of rustcompiled nodes (default: $RATIO)"
    echo "  --workload-percent N Percent of nodes to receive workload (default: 100)"
    exit 1
}

while [[ "$#" -gt 0 ]]; do
    case $1 in
        --duration) WORKLOAD_DURATION="$2"; shift ;;
        --wait-duration) WAIT_DURATION="$2"; shift ;;
        --chain-id) CHAIN_ID="$2"; shift ;;
        --nodes) NODES="$2"; shift ;;
        --ratio) RATIO="$2"; shift ;;
        --workload-percent) WORKLOAD_PERCENT="$2"; shift ;;
        *) usage ;;
    esac
    shift
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RATIO2=$((100 - RATIO))

# If WORKLOAD_PERCENT is not set, default to 100
if [ -z "$WORKLOAD_PERCENT" ]; then
    WORKLOAD_PERCENT=100
fi

# Calculate absolute number of nodes for workload
WORKLOAD_NODES=$((NODES * WORKLOAD_PERCENT / 100))
# Ensure at least 1 node if percent > 0
if [ "$WORKLOAD_NODES" -eq 0 ] && [ "$WORKLOAD_PERCENT" -gt 0 ]; then
    WORKLOAD_NODES=1
fi

# Ensure WORKLOAD_NODES does not exceed NODES
if [ "$WORKLOAD_NODES" -gt "$NODES" ]; then
    echo "Warning: workload-nodes ($WORKLOAD_NODES) cannot be greater than nodes ($NODES). Setting workload-nodes to $NODES."
    WORKLOAD_NODES=$NODES
fi

# Calculate how many nodes of each type should get the workload
# In start_local.sh, the first c_linux nodes are linux, and the rest are wasix.
source "$SCRIPT_DIR/common.sh"
read c_linux c_wasix <<< $(calculate_counts "$NODES" "$RATIO" "$RATIO2")

# We want to distribute WORKLOAD_NODES according to the same ratio
read w_linux w_wasix <<< $(calculate_counts "$WORKLOAD_NODES" "$RATIO" "$RATIO2")

# Adjust in case we don't have enough nodes of a certain type (though calculate_counts should handle it)
# but we must ensure we don't exceed the actual available nodes of each type.
# w_linux = min(w_linux, c_linux)
if [ "$w_linux" -gt "$c_linux" ]; then w_linux=$c_linux; fi
# w_wasix = min(w_wasix, c_wasix)
if [ "$w_wasix" -gt "$c_wasix" ]; then w_wasix=$c_wasix; fi

# Final WORKLOAD_NODES might be slightly different due to rounding if we were strict,
# but calculate_counts already ensures w_linux + w_wasix = WORKLOAD_NODES.

echo "=== Starting Experiment Workflow ==="
echo "Chain ID: $CHAIN_ID"
echo "Workload Duration: $WORKLOAD_DURATION seconds"
echo "Wait Duration: $WAIT_DURATION seconds"
echo "Nodes: $NODES"
echo "Workload Nodes: $WORKLOAD_NODES"
echo "RATIO: $RATIO Rust-compiled, $RATIO2 WASM-compiled"

# 1. Start the local network
echo "Step 1: Starting local network..."
bash "$SCRIPT_DIR/start_local.sh" --nodes "$NODES" --chain-id "$CHAIN_ID" --linux "$RATIO" --wasix "$RATIO2" --setup --cleanup --registry docker.io/z4l3s5i0

# 2. Start manage_logs
echo "Step 2: Starting background log processor..."
bash "$SCRIPT_DIR/manage_logs.sh" start --chain-id "$CHAIN_ID"

# 3. Wait for 300 seconds
echo "Step 3: Waiting 300 seconds for network stabilization..."
sleep 300

# 4. Start run_workload for each node in background
echo "Step 4: Starting workload for $WORKLOAD_NODES node(s) in background ($w_linux Linux, $w_wasix WASM)..."
MNEMONIC="sleep moment list remain like wall lake industry canvas wonder ecology elite duck salad naive syrup frame brass utility club odor country obey pudding"
WORKLOAD_PIDS=()
BASE_PORT=8545

# Target indices
TARGET_INDICES=()
# Linux nodes are 0 to c_linux-1
for i in $(seq 0 $((w_linux - 1))); do
    if [ $i -lt $c_linux ]; then
        TARGET_INDICES+=($i)
    fi
done
# WASM nodes are c_linux to NODES-1
for i in $(seq 0 $((w_wasix - 1))); do
    idx=$((c_linux + i))
    if [ $idx -lt $NODES ]; then
        TARGET_INDICES+=($idx)
    fi
done

for i in "${TARGET_INDICES[@]}"; do
    RPC_PORT=$((BASE_PORT + i))
    RPC_URL="http://experiments-el-node-$i-1:$RPC_PORT"
    
    # Deriving private key for the account index i
    echo "Deriving private key for node $i..."
    PRIV_KEY=$(docker run --rm ghcr.io/foundry-rs/foundry:latest "cast wallet private-key \"$MNEMONIC\" $i" | sed -n 's/.*0x\([0-9a-fA-F]\{64\}\).*/\1/p')

    # Validate PRIV_KEY length (should be 64 for raw hex)
    if [ ${#PRIV_KEY} -ne 64 ]; then
        echo "Error: Invalid private key derived for node $i (length ${#PRIV_KEY}, expected 64): '$PRIV_KEY'"
        exit 1
    fi

    echo "Launching workload for node $i at $RPC_URL with seed node$i"
    # We derive a deterministic seed based on the node index
    # to avoid collisions when using the same state directory.
    SEED="0x$(printf "node$i" | sha256sum | cut -d' ' -f1)"

    # We pass --duration slightly longer than WORKLOAD_DURATION to ensure it doesn't exit early on its own
    # although we will kill it anyway.
    # We also redirect stdin from /dev/null because run_workload.sh uses docker run -it which needs a TTY or will fail without stdin
    bash "$SCRIPT_DIR/run_workload.sh" \
        --pk "$PRIV_KEY" \
        --seed "$SEED" \
        --duration $((WORKLOAD_DURATION)) \
        --network experiments_blockchain-net \
        --scenario "exp/uber" \
        --tps 4 \
        --accounts 4 \
        --min-balance "100eth" \
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

# Also stop all contender containers
CONTENDER_CONTAINERS=$(docker ps --filter "ancestor=z4l3s5i0/contender" --format "{{.ID}}")
if [ -n "$CONTENDER_CONTAINERS" ]; then
    echo "Stopping contender containers..."
    docker stop $CONTENDER_CONTAINERS || true
fi
echo "Workloads stopped."

# 7. Wait another 300 seconds
echo "Step 7: Waiting additional 300 seconds for the workload to be processed..."
sleep 300

# 8. Stop manage_logs
echo "Step 8: Stopping log processor..."
bash "$SCRIPT_DIR/manage_logs.sh" stop

# 9. Collect results with sudo
echo "Step 9: Collecting experiment results..."
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
