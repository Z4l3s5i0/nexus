#!/bin/bash
set -e

# Experiment Analysis Script
# Usage: ./analyze_experiment.sh <experiment_dir> [chain_id]

EXP_DIR=$1
CHAIN_ID=${2:-""}

if [ -z "$EXP_DIR" ]; then
    echo "Usage: $0 <experiment_dir> [chain_id]"
    exit 1
fi

if [ ! -d "$EXP_DIR" ]; then
    echo "Error: Directory $EXP_DIR does not exist."
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DB_EXPORTER="$SCRIPT_DIR/db_exporter/target/release/db_exporter"
METADATA="$EXP_DIR/metadata.json"
declare -A NODE_TYPES
if [ -f "$METADATA" ]; then
    if command -v python3 &>/dev/null; then
        PYTHON=python3
    elif command -v python &>/dev/null; then
        PYTHON=python
    else
        echo "Error: Python is required to read metadata." >&2
        exit 1
    fi
    NODE_TYPE_LIST=$("$PYTHON" "$SCRIPT_DIR/evaluate_logs.py" --storage-node-types "$METADATA") || exit 1
    while read -r index type; do
        NODE_TYPES[$index]=${type%$'\r'}
    done <<< "$NODE_TYPE_LIST"
fi

# 0. Ensure db_exporter is built
if [ ! -f "$DB_EXPORTER" ]; then
    echo "Building db_exporter..."
    # If the user is running from ~/experiments/scripts/v2/
    # the repo is at ~/wasm-based-evm/
    # We try to find the repo root to set it for cargo if needed, but here we just run cargo build
    (cd "$SCRIPT_DIR/db_exporter" && cargo build --release)
fi

echo "Starting analysis for experiment: $EXP_DIR"
if [ ! -d "$EXP_DIR/analysis" ]; then
    sudo mkdir -p "$EXP_DIR/analysis"
fi
sudo chown -R $(id -u):$(id -g) "$EXP_DIR/analysis" 2>/dev/null || true

# 1. Extract Metrics Server Data
METRICS_DB="$EXP_DIR/metrics_server_data/metrics.redb"
if [ -f "$METRICS_DB" ]; then
    echo "Extracting Metrics Server data..."
    METRICS_TABLES=("experiments" "nodes" "metric_samples" "rpc_observations")
    for table in "${METRICS_TABLES[@]}"; do
        echo "  Exporting table: $table"
        sudo "$DB_EXPORTER" export --db-path "$METRICS_DB" --table "$table" --db-type metrics | sudo tee "$EXP_DIR/analysis/metrics_$table.json" > /dev/null || echo "    Warning: Could not export $table"
    done
else
    echo "Warning: Metrics database not found at $METRICS_DB"
fi

# 2. Extract EL Node Data
echo "Extracting EL node data..."
EL_TABLES=("headers" "block_bodies" "transactions" "metadata")
# Find all node directories in data_v2
if [ -d "$EXP_DIR/data_v2" ]; then
    for node_dir in "$EXP_DIR/data_v2"/node-*; do
        [ -d "$node_dir" ] || continue
        node_name=$(basename "$node_dir")
        if [ -f "$METADATA" ]; then
            if [[ ! "$node_name" =~ ^node-([0-9]+)$ ]] || [ -z "${NODE_TYPES[${BASH_REMATCH[1]}]:-}" ]; then
                echo "  Skipping $node_name: node type unavailable in metadata"
                continue
            fi
            if [ "${NODE_TYPES[${BASH_REMATCH[1]}]}" = geth ]; then
                echo "  Skipping $node_name: Geth storage is not a wasix_eth redb database"
                continue
            fi
        fi
        # Path to EL database might vary, check common locations
        EL_DB=""
        if [ -f "$node_dir/el/linux-$node_name" ]; then
             EL_DB="$node_dir/el/linux-$node_name"
        elif [ -f "$node_dir/el/wasix-$node_name" ]; then
             EL_DB="$node_dir/el/wasix-$node_name"
        elif [ -f "$node_dir/el/eth.redb" ]; then
             EL_DB="$node_dir/el/eth.redb"
        fi
        
        if [ -n "$EL_DB" ]; then
            echo "  Checking EL data for $node_name at $EL_DB..."
            if [ ! -d "$EXP_DIR/analysis/$node_name" ]; then
                sudo mkdir -p "$EXP_DIR/analysis/$node_name"
            fi
            sudo chown -R $(id -u):$(id -g) "$EXP_DIR/analysis/$node_name" 2>/dev/null || true
            # List available tables first to debug
            sudo "$DB_EXPORTER" list --db-path "$EL_DB" | sudo tee "$EXP_DIR/analysis/$node_name/tables.txt" > /dev/null
            
            for table in "${EL_TABLES[@]}"; do
                sudo "$DB_EXPORTER" export --db-path "$EL_DB" --table "$table" --db-type el | sudo tee "$EXP_DIR/analysis/$node_name/el_$table.json" > /dev/null || echo "    Warning: Could not export $table for $node_name"
            done
        else
            echo "  Warning: No EL database found for $node_name"
        fi
    done
else
    echo "Warning: data_v2 directory not found."
fi

# 3. Evaluate Logs
echo "Evaluating logs..."
LOGS_DIR="$EXP_DIR/logs_dump"
if [ ! -d "$LOGS_DIR" ]; then
    # Try aggregated logs if logs_dump is missing
    LOGS_DIR="$EXP_DIR/aggregated_logs"
fi

if [ -d "$LOGS_DIR" ]; then
    if [ -f "$METADATA" ]; then
        set -o pipefail
        sudo "$PYTHON" "$SCRIPT_DIR/evaluate_logs.py" "$LOGS_DIR" "$CHAIN_ID" "$METADATA" | sudo tee "$EXP_DIR/analysis/log_evaluation.txt" > /dev/null
        set +o pipefail
    else
        sudo bash "$SCRIPT_DIR/evaluate.sh" "$LOGS_DIR" "$CHAIN_ID" | sudo tee "$EXP_DIR/analysis/log_evaluation.txt" > /dev/null
    fi
    echo "  Log evaluation saved to $EXP_DIR/analysis/log_evaluation.txt"
else
    echo "Warning: No logs found for evaluation."
fi

# 4. Generate Plots
echo "Generating plots..."
mkdir -p "$EXP_DIR/plots"
chown -R "$(id -u):$(id -g)" "$EXP_DIR/plots" 2>/dev/null || true
if command -v python3 &>/dev/null; then
    PYTHON=python3
elif command -v python &>/dev/null; then
    PYTHON=python
else
    echo "Error: Python is required to generate plots." >&2
    exit 1
fi

rm -f "$EXP_DIR/analysis/PLOTS_SKIPPED"
PLOT_EXIT_CODE=0
if [ -f "$EXP_DIR/analysis/metrics_metric_samples.json" ]; then
    "$PYTHON" "$SCRIPT_DIR/plot_metrics.py" "$EXP_DIR/analysis" "$EXP_DIR/plots" || PLOT_EXIT_CODE=$?
else
    PLOT_EXIT_CODE=2
fi
if [ "$PLOT_EXIT_CODE" -eq 2 ]; then
    echo "Warning: Plotting skipped due to empty or invalid metrics data."
    touch "$EXP_DIR/analysis/PLOTS_SKIPPED"
elif [ "$PLOT_EXIT_CODE" -ne 0 ]; then
    echo "Error: Plotting failed with exit code $PLOT_EXIT_CODE" >&2
    exit "$PLOT_EXIT_CODE"
fi

# 5. Generate Summary
echo "Analysis complete. Results are in $EXP_DIR/analysis and $EXP_DIR/plots"
chown -R "$(id -u):$(id -g)" "$EXP_DIR/analysis" "$EXP_DIR/plots" 2>/dev/null || true
cat > "$EXP_DIR/analysis/SUMMARY.md" <<EOF
# Experiment Analysis Summary

- Experiment: $(basename "$EXP_DIR")
- Date: $(date)
- Metrics and node database exports are stored in this directory.
- Log analysis: [log_evaluation.txt](./log_evaluation.txt)
- Plots: [plots directory](../plots)
$(if [ -f "$EXP_DIR/analysis/PLOTS_SKIPPED" ]; then printf '%s\n' '- Plot generation was skipped because metrics data was empty or invalid.'; fi)
EOF
