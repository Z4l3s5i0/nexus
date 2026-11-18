#!/bin/bash
set -e

# Experiment Loop Script
# Runs the experiment with different combinations of parameters.
VARIANTS=(
    "exp/uber 120 4 4"
#    "exp/counter 180 17 17"
#    "exp/dota 280 66 66"
)
WAIT_DURATION=900
RATIO_VARIANTS=(
#    "10 0 0"
#    "0 10 0"
    "0 0 10"
#    "5 5 0"
#    "0 5 5"
#    "5 0 5"
#    "3 3 4"
)
WORKLOAD_NODE_PERCENT=100

usage() {
    cat <<EOF
Usage: $0 [--ratios 'LINUX WASIX [GETH]; ...'] [--help]

  --ratios  Semicolon-separated list of whitespace-separated node counts.
            Each entry must have two or three nonnegative integers in Linux,
            WASIX, Geth order, with a positive total. Geth defaults to zero.
            Example: --ratios '3 4 3; 0 5 5'
  --help    Show this help message.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --ratios)
            if [ "$#" -lt 2 ]; then
                echo "Error: --ratios requires a list of count pairs or triples." >&2
                exit 1
            fi
            ratio_list=$2
            RATIO_VARIANTS=()
            while :; do
                RATIO_VARIANTS+=("${ratio_list%%;*}")
                if [[ "$ratio_list" != *';'* ]]; then
                    break
                fi
                ratio_list=${ratio_list#*;}
            done
            shift 2
            ;;
        --help)
            usage
            exit 0
            ;;
        *)
            echo "Error: unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

ratio_pattern='^[[:space:]]*([0-9]+)[[:space:]]+([0-9]+)([[:space:]]+([0-9]+))?[[:space:]]*$'
max_count=9223372036854775807
NORMALIZED_RATIOS=()
for ratio_var in "${RATIO_VARIANTS[@]}"; do
    if [[ ! "$ratio_var" =~ $ratio_pattern ]]; then
        echo "Error: invalid ratio '$ratio_var': expected two or three nonnegative integer counts." >&2
        exit 1
    fi
    counts=("${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" "${BASH_REMATCH[4]:-0}")
    normalized_counts=()
    node_count=0
    for count in "${counts[@]}"; do
        count=${count#"${count%%[!0]*}"}
        count=${count:-0}
        if [ "${#count}" -gt 19 ] || { [ "${#count}" -eq 19 ] && [[ "$count" > "$max_count" ]]; }; then
            echo "Error: count in ratio '$ratio_var' is too large." >&2
            exit 1
        fi
        if (( node_count > max_count - count )); then
            echo "Error: total in ratio '$ratio_var' is too large." >&2
            exit 1
        fi
        node_count=$((node_count + count))
        normalized_counts+=("$count")
    done
    if [ "$node_count" -eq 0 ]; then
        echo "Error: ratio '$ratio_var' must have a positive total." >&2
        exit 1
    fi
    NORMALIZED_RATIOS+=("${normalized_counts[*]}")
done
RATIO_VARIANTS=("${NORMALIZED_RATIOS[@]}")

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
        read linux_nodes wasix_nodes geth_nodes <<< "$ratio_var"
        NODE_COUNT=$((linux_nodes + wasix_nodes + geth_nodes))
        COUNTER=$((COUNTER + 1))
        
        echo "--- Running Experiment $COUNTER/$TOTAL_COMBINATIONS ---"
        echo "Workload Type: $wl_type"
        echo "Nodes: $NODE_COUNT (Linux: $linux_nodes, WASM: $wasix_nodes, Geth: $geth_nodes)"
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
            --geth-nodes "$geth_nodes" \
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
            PARAM_INFO="Type: $wl_type, Nodes: $NODE_COUNT (L:$linux_nodes, W:$wasix_nodes, G:$geth_nodes), WL%: $WORKLOAD_NODE_PERCENT, Dur: $duration, Wait: $WAIT_DURATION, Spammers: $spammer_accounts, TPS: $tps"
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
