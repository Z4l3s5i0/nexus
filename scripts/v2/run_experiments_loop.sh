#!/bin/bash
set -e

# Experiment Loop Script
# Runs the experiment with different combinations of parameters.

# Log file: all console output goes here as well
LOG_FILE="${LOG_FILE:-experiments.log}"
exec > >(tee -a "$LOG_FILE") 2>&1

VARIANTS=(
    "exp/uber 120 4 4"
    "exp/counter 180 17 17"
    "exp/dota 280 66 66"
)
WAIT_DURATION=900
RATIO_VARIANTS=(
    "10 0 0"
    "0 10 0"
    "0 0 10"
    "5 5 0"
    "0 5 5"
    "5 0 5"
    "3 3 4"
)
NETWORK_SIZES_ARG=""
WORKLOADS_ARG=""
WORKLOAD_NODE_PERCENT=100

usage() {
    cat <<EOF
Usage: $0 [--ratios 'LINUX WASIX [GETH [RETH]]; ...'] [--network-sizes 4,8,16,32,64]
          [--workloads 'SCENARIO DURATION SPAMMER_ACCOUNTS TPS; ...'] [--help]

  --ratios  Semicolon-separated list of whitespace-separated node counts.
            Each entry must have two to four nonnegative integers in Linux,
            WASIX, Geth, Reth order. Geth and Reth default to zero.
            Example: --ratios '3 4 3 2; 0 5 5 0'
  --network-sizes  Scale each ratio mix proportionally to these total node counts.
                  May be combined with --ratios to scale custom mixes.
                  Example: --network-sizes 4,8,16,32,64
  --workloads  Optional semicolon-separated workload variants; defaults to Uber,
               Counter, and Dota. Each entry has scenario, duration, accounts, TPS.
               Example: --workloads 'exp/uber 120 4 4; exp/fifa 180 17 17.415; exp/dota 280 66 66'
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
        --network-sizes)
            if [ "$#" -lt 2 ] || [ -z "$2" ]; then
                echo "Error: --network-sizes requires a comma-separated list." >&2
                exit 1
            fi
            NETWORK_SIZES_ARG=$2
            shift 2
            ;;
        --workloads)
            if [ "$#" -lt 2 ] || [ -z "$2" ]; then
                echo "Error: --workloads requires a semicolon-separated list." >&2
                exit 1
            fi
            WORKLOADS_ARG=$2
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

if [ -n "$WORKLOADS_ARG" ]; then
    IFS=';' read -ra WORKLOAD_VARIANTS <<< "$WORKLOADS_ARG"
    VARIANTS=()
    for variant in "${WORKLOAD_VARIANTS[@]}"; do
        read -r -a fields <<< "$variant"
        if [ "${#fields[@]}" -ne 4 ] || [[ ! "${fields[0]}" =~ ^[^[:space:]]+$ ]] || \
           [[ ! "${fields[1]}" =~ ^[0-9]+$ ]] || [[ ! "${fields[2]}" =~ ^[0-9]+$ ]] || \
           [[ ! "${fields[3]}" =~ ^[0-9]+([.][0-9]+)?$ ]] || [ "${fields[1]}" -eq 0 ] || \
           [ "${fields[2]}" -eq 0 ]; then
            echo "Error: invalid workload '$variant'; expected SCENARIO DURATION SPAMMER_ACCOUNTS TPS." >&2
            exit 1
        fi
        VARIANTS+=("${fields[*]}")
    done
fi

ratio_pattern='^[[:space:]]*([0-9]+)[[:space:]]+([0-9]+)([[:space:]]+([0-9]+))?([[:space:]]+([0-9]+))?[[:space:]]*$'
max_count=9223372036854775807
NORMALIZED_RATIOS=()
for ratio_var in "${RATIO_VARIANTS[@]}"; do
    if [[ ! "$ratio_var" =~ $ratio_pattern ]]; then
        echo "Error: invalid ratio '$ratio_var': expected two to four nonnegative integer counts." >&2
        exit 1
    fi
    counts=("${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" "${BASH_REMATCH[4]:-0}" "${BASH_REMATCH[6]:-0}")
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

if [ -n "$NETWORK_SIZES_ARG" ]; then
    IFS=',' read -ra NETWORK_SIZES <<< "$NETWORK_SIZES_ARG"
    declare -A SEEN_SIZES=()
    NORMALIZED_SIZES=()
    for size in "${NETWORK_SIZES[@]}"; do
        if [[ ! "$size" =~ ^[0-9]+$ ]]; then
            echo "Error: invalid network size '$size': expected a positive integer." >&2
            exit 1
        fi
        size=${size#"${size%%[!0]*}"}
        size=${size:-0}
        if [ "${#size}" -gt 6 ] || { [ "${#size}" -eq 6 ] && [ "$size" -gt 100000 ]; } || [ "$size" -eq 0 ]; then
            echo "Error: network sizes must be between 1 and 100000." >&2
            exit 1
        fi
        if [ -n "${SEEN_SIZES[$size]:-}" ]; then
            echo "Error: duplicate network size '$size'." >&2
            exit 1
        fi
        SEEN_SIZES[$size]=1
        NORMALIZED_SIZES+=("$size")
    done

    BASE_RATIOS=("${RATIO_VARIANTS[@]}")
    SCALED_RATIOS=()
    for size in "${NORMALIZED_SIZES[@]}"; do
        for ratio_var in "${BASE_RATIOS[@]}"; do
            read -r -a counts <<< "$ratio_var"
            base_total=0
            for count in "${counts[@]}"; do
                base_total=$((base_total + count))
            done
            scaled_counts=()
            remainders=()
            scaled_total=0
            for count in "${counts[@]}"; do
                if (( count > max_count / size )); then
                    echo "Error: ratio '$ratio_var' is too large to scale safely." >&2
                    exit 1
                fi
                product=$((count * size))
                scaled=$((product / base_total))
                scaled_counts+=("$scaled")
                remainders+=("$((product % base_total))")
                scaled_total=$((scaled_total + scaled))
            done
            remaining=$((size - scaled_total))
            for ((allocation = 0; allocation < remaining; allocation++)); do
                best_index=0
                for ((index = 1; index < ${#remainders[@]}; index++)); do
                    if (( remainders[index] > remainders[best_index] )); then
                        best_index=$index
                    fi
                done
                scaled_counts[$best_index]=$((scaled_counts[best_index] + 1))
                remainders[$best_index]=-1
            done
            SCALED_RATIOS+=("${scaled_counts[*]}")
        done
    done
    RATIO_VARIANTS=("${SCALED_RATIOS[@]}")
fi

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
        read linux_nodes wasix_nodes geth_nodes reth_nodes <<< "$ratio_var"
        reth_nodes=${reth_nodes:-0}
        NODE_COUNT=$((linux_nodes + wasix_nodes + geth_nodes + reth_nodes))
        COUNTER=$((COUNTER + 1))
        
        echo "--- Running Experiment $COUNTER/$TOTAL_COMBINATIONS ---"
        echo "Workload Type: $wl_type"
        echo "Nodes: $NODE_COUNT (Linux: $linux_nodes, WASM: $wasix_nodes, Geth: $geth_nodes, Reth: $reth_nodes)"
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
            --reth-nodes "$reth_nodes" \
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
            PARAM_INFO="Type: $wl_type, Nodes: $NODE_COUNT (L:$linux_nodes, W:$wasix_nodes, G:$geth_nodes, R:$reth_nodes), WL%: $WORKLOAD_NODE_PERCENT, Dur: $duration, Wait: $WAIT_DURATION, Spammers: $spammer_accounts, TPS: $tps"
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
