#!/bin/bash
set -e

# Combined Experiment Analysis Script
# Combines the functionality of:
#   - analyze_all_experiments.sh  (bulk loop over experiment directories)
#   - analyze_experiment.sh       (DB export, log evaluation)
#   - plot_metrics.py             (metric plots: faceted, stacked bars, etc.)
#   - plot_stacked_faceted.py     (stacked faceted grid plot, PDF)
#   - plot_non_stacked_faceted.py (individual per-node per-metric plots, PNG)
#   - analyze_slots.py            (CL slot analysis, LaTeX table)
#   - extract_block_times.py      (EL block execution times, LaTeX table)
#
# Usage:
#   ./run_full_analysis.sh <experiments_base_dir> [options]
#   ./run_full_analysis.sh <experiments_base_dir> [chain_id] [steps]  (legacy positional)
#
# Positional arguments:
#   experiments_base_dir  Root directory containing experiment subdirectories
#
# Options:
#   --chain-id <id>       Chain ID to filter registrations
#   --steps <s1,s2,...>   Comma-separated list of steps to run.
#                         Default: all steps (1-5).
#                         1 = export_data
#                         2 = evaluate_logs
#                         3 = plot_metrics
#                         4 = plot_faceted
#                         5 = analyze_slots_and_blocks
#
# Examples:
#   ./run_full_analysis.sh /path/to/experiments                          (all steps)
#   ./run_full_analysis.sh /path/to/experiments --steps 3,4,5           (steps 3-5, no chain_id)
#   ./run_full_analysis.sh /path/to/experiments --chain-id 1-100        (all steps, with chain_id)
#   ./run_full_analysis.sh /path/to/experiments --chain-id 1-100 --steps 3,4
#   ./run_full_analysis.sh /path/to/experiments 1-100 "3,4"             (legacy positional)

BASE_DIR="$1"
CHAIN_ID=""
STEPS_ARG=""

# Parse remaining arguments: support both named flags and legacy positional
shift 2>/dev/null || true
while [ $# -gt 0 ]; do
    case "$1" in
        --chain-id)
            CHAIN_ID="$2"
            shift 2
            ;;
        --steps)
            STEPS_ARG="$2"
            shift 2
            ;;
        *)
            # Legacy positional: first unrecognised arg is chain_id, second is steps
            if [ -z "$CHAIN_ID" ]; then
                CHAIN_ID="$1"
            elif [ -z "$STEPS_ARG" ]; then
                STEPS_ARG="$1"
            fi
            shift
            ;;
    esac
done

if [ -z "$BASE_DIR" ]; then
    echo "Usage: $0 <experiments_base_dir> [options]"
    echo ""
    echo "Options:"
    echo "  --chain-id <id>       Chain ID to filter registrations"
    echo "  --steps <s1,s2,...>   Comma-separated list of steps to run"
    echo ""
    echo "Steps:"
    echo "  1 = export_data        (export metrics DB and EL node databases)"
    echo "  2 = evaluate_logs      (evaluate CL/EL logs)"
    echo "  3 = plot_metrics       (generate metric plots)"
    echo "  4 = plot_faceted       (generate faceted plots)"
    echo "  5 = analyze_slots_and_blocks (slot analysis + block times)"
    echo ""
    echo "Examples:"
    echo "  $0 /path/to/experiments                          (all steps)"
    echo "  $0 /path/to/experiments --steps 3,4,5           (steps 3-5 only)"
    echo "  $0 /path/to/experiments --chain-id 1-100        (all steps with chain filter)"
    exit 1
fi

if [ ! -d "$BASE_DIR" ]; then
    echo "Error: Directory $BASE_DIR does not exist."
    exit 1
fi

# Resolve script directories
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
V2_DIR="$SCRIPT_DIR/v2"

# Verify v2 directory exists
if [ ! -d "$V2_DIR" ]; then
    echo "Error: v2 directory not found at $V2_DIR"
    exit 1
fi

# Locate Python interpreter
if command -v python3 &>/dev/null; then
    PYTHON=python3
elif command -v python &>/dev/null; then
    PYTHON=python
else
    echo "Error: Python is required." >&2
    exit 1
fi

# Parse steps
if [ -n "$STEPS_ARG" ]; then
    IFS=',' read -ra STEPS <<< "$STEPS_ARG"
else
    STEPS=(1 2 3 4 5)
fi

should_run_step() {
    local step=$1
    for s in "${STEPS[@]}"; do
        if [ "$s" = "$step" ]; then
            return 0
        fi
    done
    return 1
}

# Ensure db_exporter is built (needed for step 1)
DB_EXPORTER="$V2_DIR/db_exporter/target/release/db_exporter"
build_db_exporter() {
    if [ ! -f "$DB_EXPORTER" ]; then
        echo "Building db_exporter..."
        (cd "$V2_DIR/db_exporter" && cargo build --release)
    fi
}

# -------------------------------------------------------
# Step 1: Export Data (from analyze_experiment.sh)
# -------------------------------------------------------
export_data() {
    local exp_dir="$1"
    local chain_id="$2"
    local analysis_dir="$exp_dir/analysis"

    echo "  [1/5] Exporting databases..."

    if [ ! -d "$analysis_dir" ]; then
        mkdir -p "$analysis_dir"
    fi

    # 1a. Metrics Server Data
    local metrics_db="$exp_dir/metrics_server_data/metrics.redb"
    if [ -f "$metrics_db" ]; then
        local METRICS_TABLES=("experiments" "nodes" "metric_samples" "rpc_observations")
        for table in "${METRICS_TABLES[@]}"; do
            echo "    Exporting metrics table: $table"
            "$DB_EXPORTER" export --db-path "$metrics_db" --table "$table" --db-type metrics \
                > "$analysis_dir/metrics_$table.json" 2>/dev/null \
                || echo "    Warning: Could not export $table"
        done
    else
        echo "    Warning: Metrics database not found at $metrics_db"
    fi

    # 1b. EL Node Data
    local el_tables=("headers" "block_bodies" "transactions" "metadata")
    if [ -d "$exp_dir/data_v2" ]; then
        for node_dir in "$exp_dir/data_v2"/node-*; do
            [ -d "$node_dir" ] || continue
            local node_name
            node_name=$(basename "$node_dir")

            # Find EL database
            local el_db=""
            if [ -f "$node_dir/el/linux-$node_name" ]; then
                el_db="$node_dir/el/linux-$node_name"
            elif [ -f "$node_dir/el/wasix-$node_name" ]; then
                el_db="$node_dir/el/wasix-$node_name"
            elif [ -f "$node_dir/el/eth.redb" ]; then
                el_db="$node_dir/el/eth.redb"
            fi

            if [ -n "$el_db" ]; then
                echo "    Exporting EL data for $node_name"
                if [ ! -d "$analysis_dir/$node_name" ]; then
                    mkdir -p "$analysis_dir/$node_name"
                fi
                "$DB_EXPORTER" list --db-path "$el_db" > "$analysis_dir/$node_name/tables.txt" 2>/dev/null || true
                for table in "${el_tables[@]}"; do
                    "$DB_EXPORTER" export --db-path "$el_db" --table "$table" --db-type el \
                        > "$analysis_dir/$node_name/el_$table.json" 2>/dev/null \
                        || echo "    Warning: Could not export $table for $node_name"
                done
            fi
        done
    else
        echo "    Warning: data_v2 directory not found."
    fi
}

# -------------------------------------------------------
# Step 2: Evaluate Logs (from analyze_experiment.sh)
# -------------------------------------------------------
evaluate_logs() {
    local exp_dir="$1"
    local chain_id="$2"
    local analysis_dir="$exp_dir/analysis"
    local metadata="$exp_dir/metadata.json"

    echo "  [2/5] Evaluating logs..."

    local logs_dir="$exp_dir/logs_dump"
    if [ ! -d "$logs_dir" ]; then
        logs_dir="$exp_dir/aggregated_logs"
    fi

    if [ -d "$logs_dir" ]; then
        if [ -f "$metadata" ]; then
            "$PYTHON" "$V2_DIR/evaluate_logs.py" "$logs_dir" "$chain_id" "$metadata" \
                > "$analysis_dir/log_evaluation.txt" 2>&1
        else
            bash "$V2_DIR/evaluate.sh" "$logs_dir" "$chain_id" \
                > "$analysis_dir/log_evaluation.txt" 2>&1
        fi
        echo "    Log evaluation saved to $analysis_dir/log_evaluation.txt"
    else
        echo "    Warning: No logs found for evaluation."
    fi
}

# -------------------------------------------------------
# Step 3: Plot Metrics (from plot_metrics.py)
# -------------------------------------------------------
plot_metrics() {
    local exp_dir="$1"
    local analysis_dir="$exp_dir/analysis"
    local plots_dir="$exp_dir/plots"

    echo "  [3/5] Generating metric plots..."

    if [ ! -d "$analysis_dir" ]; then
        echo "    Error: Analysis directory not found. Run step 1 first."
        return 1
    fi

    if [ ! -d "$plots_dir" ]; then
        mkdir -p "$plots_dir"
    fi

    local plot_exit_code=0
    "$PYTHON" "$V2_DIR/plot_metrics.py" "$analysis_dir" "$plots_dir" || plot_exit_code=$?

    if [ $plot_exit_code -eq 2 ]; then
        echo "    Warning: Plotting skipped due to empty or invalid metrics data."
        touch "$analysis_dir/PLOTS_SKIPPED"
    elif [ $plot_exit_code -ne 0 ]; then
        echo "    Error: Plotting failed with exit code $plot_exit_code"
        return $plot_exit_code
    fi
}

# -------------------------------------------------------
# Step 4: Faceted Plots (from plot_stacked_faceted.py + plot_non_stacked_faceted.py)
# -------------------------------------------------------
plot_faceted() {
    local exp_dir="$1"
    local analysis_dir="$exp_dir/analysis"
    local plots_dir="$exp_dir/plots"

    echo "  [4/5] Generating faceted plots..."

    if [ ! -d "$analysis_dir" ]; then
        echo "    Error: Analysis directory not found. Run step 1 first."
        return 1
    fi

    if [ ! -d "$plots_dir" ]; then
        mkdir -p "$plots_dir"
    fi

    # Stacked faceted plot (single PDF with all nodes x metrics)
    echo "    Generating stacked faceted plot..."
    "$PYTHON" "$V2_DIR/plot_stacked_faceted.py" "$analysis_dir" "$plots_dir" "$exp_dir" "node_metrics_stacked_faceted.pdf" \
        || echo "    Warning: Stacked faceted plot failed"

    # Non-stacked faceted plots (individual PNGs per node per metric)
    echo "    Generating non-stacked faceted plots..."
    "$PYTHON" "$V2_DIR/plot_non_stacked_faceted.py" "$analysis_dir" "$plots_dir" "$exp_dir" "node_metrics_non_stacked_faceted.png" \
        || echo "    Warning: Non-stacked faceted plot failed"
}

# -------------------------------------------------------
# Step 5: Slot Analysis & Block Times (from analyze_slots.py + extract_block_times.py)
# -------------------------------------------------------
analyze_slots_and_blocks() {
    local exp_dir="$1"
    local analysis_dir="$exp_dir/analysis"

    echo "  [5/5] Analyzing slots and block times..."

    if [ ! -d "$analysis_dir" ]; then
        mkdir -p "$analysis_dir"
    fi

    # Slot analysis
    echo "    Analyzing CL slots..."
    "$PYTHON" "$V2_DIR/analyze_slots.py" "$exp_dir" > "$analysis_dir/slots_analysis.tex" 2>&1 \
        || echo "    Warning: Slot analysis failed"

    # Block execution times
    echo "    Extracting block execution times..."
    "$PYTHON" "$V2_DIR/extract_block_times.py" "$exp_dir" > "$analysis_dir/block_times.tex" 2>&1 \
        || echo "    Warning: Block time extraction failed"
}

# -------------------------------------------------------
# Analyze a single experiment directory
# -------------------------------------------------------
analyze_single_experiment() {
    local exp_dir="$1"
    local chain_id="$2"
    local exp_name
    exp_name=$(basename "$exp_dir")

    echo ""
    echo "=== Analyzing: $exp_name ==="

    if should_run_step 1; then
        export_data "$exp_dir" "$chain_id"
    fi

    if should_run_step 2; then
        evaluate_logs "$exp_dir" "$chain_id"
    fi

    if should_run_step 3; then
        plot_metrics "$exp_dir"
    fi

    if should_run_step 4; then
        plot_faceted "$exp_dir"
    fi

    if should_run_step 5; then
        analyze_slots_and_blocks "$exp_dir"
    fi

    echo "=== Done: $exp_name ==="
}

# -------------------------------------------------------
# Main: Bulk analysis loop (from analyze_all_experiments.sh)
# -------------------------------------------------------
echo "=== Starting Combined Analysis in $BASE_DIR ==="
echo "Steps to run: ${STEPS[*]}"
if [ -n "$CHAIN_ID" ]; then
    echo "Chain ID filter: $CHAIN_ID"
fi
echo ""

# Build db_exporter if step 1 is needed
if should_run_step 1; then
    build_db_exporter
fi

# Get absolute path
ABS_BASE_DIR=$(cd "$BASE_DIR" && pwd)

# Count directories
TOTAL_DIRS=$(find "$ABS_BASE_DIR" -maxdepth 1 -mindepth 1 -type d | wc -l)
COUNTER=0

echo "Found $TOTAL_DIRS directories to process."
echo ""

for exp_dir in "$ABS_BASE_DIR"/*; do
    if [ -d "$exp_dir" ]; then
        COUNTER=$((COUNTER + 1))
        exp_name=$(basename "$exp_dir")

        echo "--- ($COUNTER/$TOTAL_DIRS): $exp_name ---"

        # Check if it looks like an experiment directory
        if [ -d "$exp_dir/data_v2" ] || [ -d "$exp_dir/metrics_server_data" ] || \
           [ -d "$exp_dir/logs_dump" ] || [ -d "$exp_dir/aggregated_logs" ]; then
            analyze_single_experiment "$exp_dir" "$CHAIN_ID"
        else
            echo "Skipping $exp_name: Not a recognized experiment directory."
        fi

        echo "---------------------------------------------------"
    fi
done

echo ""
echo "=== Combined Analysis Complete ==="
echo "Results are in <experiment_dir>/analysis/ and <experiment_dir>/plots/"
