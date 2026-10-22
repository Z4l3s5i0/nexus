#!/bin/bash
set -e

# Experiment Loop Script
# Runs the experiment with different combinations of parameters.
VARIANTS=(
    "exp/uber 120 4 4"
    "exp/counter 180 17 17"
    "exp/dota 280 66 66"
)
WAIT_DURATION=480
RATIO_VARIANTS=(
    "10 0"
    "5 5"
    "0 10"
)
WORKLOAD_NODE_PERCENT=100

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUN_EXPERIMENT="$SCRIPT_DIR/run_experiment.sh"

TOTAL_COMBINATIONS=$(( ${#VARIANTS[@]} * ${#RATIO_VARIANTS[@]} ))
COUNTER=0
FAILED_PLOTS=()

echo "=== Starting Experiment Loop ==="
echo "Total combinations to run: $TOTAL_COMBINATIONS"
echo ""

for variant in "${VARIANTS[@]}"; do
    read wl_type duration spammer_accounts tps <<< "$variant"
    for ratio_var in "${RATIO_VARIANTS[@]}"; do
        read linux_nodes wasix_nodes <<< "$ratio_var"
        NODE_COUNT=$((linux_nodes + wasix_nodes))
        COUNTER=$((COUNTER + 1))
        
        echo "--- Running Experiment $COUNTER/$TOTAL_COMBINATIONS ---"
        echo "Workload Type: $wl_type"
        echo "Nodes: $NODE_COUNT (Linux: $linux_nodes, WASM: $wasix_nodes)"
        echo "Workload Percent: $WORKLOAD_NODE_PERCENT%"
        echo "Workload Duration: $duration s"
        echo "Wait Duration: $WAIT_DURATION s"
        echo "Spammer Accounts: $spammer_accounts"
        echo "TPS: $tps"
        echo "---------------------------------------"
        
        # Run the experiment
        # We use a unique chain-id for each run to avoid any state overlap if cleanup fails
        CHAIN_ID=$((1000 + COUNTER))
        
        bash "$RUN_EXPERIMENT" \
            --linux-nodes "$linux_nodes" \
            --wasix-nodes "$wasix_nodes" \
            --workload-percent "$WORKLOAD_NODE_PERCENT" \
            --duration "$duration" \
            --wait-duration "$WAIT_DURATION" \
            --workload-type "$wl_type" \
            --spammer-accounts "$spammer_accounts" \
            --tps "$tps" \
            --chain-id "$CHAIN_ID"
                    
        # Check if plotting was skipped
        # We need to find the experiment directory. run_experiment.sh doesn't output it easily,
        # but we know it's in experiments/exp_YYYYMMDD_HHMMSS/analysis/PLOTS_SKIPPED
        # Let's find the latest directory in experiments/
        LATEST_EXP=$(ls -td experiments/exp_* 2>/dev/null | head -1)
        if [ -f "$LATEST_EXP/analysis/PLOTS_SKIPPED" ]; then
            PARAM_INFO="Type: $wl_type, Nodes: $NODE_COUNT (L:$linux_nodes, W:$wasix_nodes), WL%: $WORKLOAD_NODE_PERCENT, Dur: $duration, Wait: $WAIT_DURATION, Spammers: $spammer_accounts, TPS: $tps"
            FAILED_PLOTS+=("$LATEST_EXP ($PARAM_INFO)")
        fi

        echo "--- Finished Experiment $COUNTER/$TOTAL_COMBINATIONS ---"
        echo ""
    done
done

echo "=== All Experiments Completed ==="

if [ ${#FAILED_PLOTS[@]} -ne 0 ]; then
    echo ""
    echo "=== Experiments with skipped plotting (invalid/empty metrics) ==="
    for failed in "${FAILED_PLOTS[@]}"; do
        echo "- $failed"
    done
    echo "================================================================"
fi
