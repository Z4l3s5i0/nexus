#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MODULE_ROOT="${EXPERIMENTS_MODULE_ROOT:-$SCRIPT_DIR}"
SETUP_SCRIPT="${EXPERIMENTS_SETUP_SCRIPT:-$SCRIPT_DIR/setup.sh}"
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
WASMER_DIR="${WASMER_DIR:-$HOME/.wasmer}"
export PATH="$CARGO_HOME/bin:$WASMER_DIR/bin:$PATH"

usage() {
    cat <<EOF
Usage: bash experiments/run.sh

Choose experiments by entering their numbers separated by commas, in the order
they should run. Enter 6 to run all five in the standard order.

  1) Runtime
  2) Empty block
  3) Startup
  4) Compliance (Hive ethereum/engine)
  5) Size comparison
  6) All experiments
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi
if [[ "$#" -gt 0 ]]; then
    echo "Error: this runner is interactive and does not accept arguments." >&2
    usage >&2
    exit 2
fi

declare -a labels=(runtime empty-block startup compliance size)
declare -a selected=()

usage
while true; do
    if ! IFS= read -r -p 'Selection: ' selection; then
        echo "Error: no experiment selection was provided." >&2
        exit 2
    fi
    selection="${selection//[[:space:]]/}"
    if [[ -z "$selection" ]]; then
        echo "Please select at least one experiment." >&2
        continue
    fi
    if [[ "$selection" == "6" || "${selection,,}" == "all" ]]; then
        selected=("${labels[@]}")
        break
    fi

    IFS=',' read -r -a choices <<< "$selection"
    declare -A seen=()
    valid=true
    selected=()
    for choice in "${choices[@]}"; do
        if [[ ! "$choice" =~ ^[1-5]$ ]]; then
            echo "Invalid selection '$choice'; use numbers 1-5, a comma-separated list, or 6 for all." >&2
            valid=false
            break
        fi
        if [[ -n "${seen[$choice]:-}" ]]; then
            echo "Experiment $choice was selected more than once." >&2
            valid=false
            break
        fi
        seen[$choice]=1
        selected+=("${labels[$((choice - 1))]}")
    done
    if [[ "$valid" == true ]]; then
        break
    fi
done

if [[ ! -f "$SETUP_SCRIPT" ]]; then
    echo "Error: setup script not found: $SETUP_SCRIPT" >&2
    exit 1
fi

export EXPERIMENTS_OUTPUT_ROOT="${EXPERIMENTS_OUTPUT_ROOT:-$REPO_ROOT/experiment-results}"
export EXPERIMENT_RUN_ID="${EXPERIMENT_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
EXPERIMENTS_SELECTED="$(IFS=,; printf '%s' "${selected[*]}")"
export EXPERIMENTS_SELECTED

echo "Run ID: $EXPERIMENT_RUN_ID"
echo "Output root: $EXPERIMENTS_OUTPUT_ROOT"
echo "Running shared setup once..."
bash "$SETUP_SCRIPT"

for experiment in "${selected[@]}"; do
    module_script="$MODULE_ROOT/$experiment/run.sh"
    if [[ ! -f "$module_script" ]]; then
        echo "Error: entry point not found for '$experiment': $module_script" >&2
        exit 1
    fi
    echo "=== Starting $experiment experiment ==="
    if bash "$module_script"; then
        :
    else
        status=$?
        echo "Error: $experiment experiment failed; later experiments were not started." >&2
        exit "$status"
    fi
    echo "=== Finished $experiment experiment ==="
done

echo "All selected experiments completed successfully."