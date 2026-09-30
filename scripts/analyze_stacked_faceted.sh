#!/bin/bash
set -e

# Stacked Faceted Analysis Script
# Usage: ./analyze_stacked_faceted.sh <experiment_dir> [output_dir] [output_filename]

EXP_DIR=$1
OUTPUT_DIR_ARG=$2
OUTPUT_FILENAME=${3:-"node_metrics_stacked_faceted.pdf"}

if [ -z "$EXP_DIR" ]; then
    echo "Usage: $0 <experiment_dir> [output_dir] [output_filename]"
    exit 1
fi

if [ ! -d "$EXP_DIR" ]; then
    echo "Error: Directory $EXP_DIR does not exist."
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ANALYSIS_DIR="$EXP_DIR/analysis"

if [ -n "$OUTPUT_DIR_ARG" ]; then
    PLOTS_DIR="$OUTPUT_DIR_ARG"
else
    PLOTS_DIR="$EXP_DIR/plots"
fi

if [ ! -d "$ANALYSIS_DIR" ]; then
    echo "Error: Analysis directory $ANALYSIS_DIR not found. Run analyze_experiment.sh first."
    exit 1
fi

if [ ! -d "$PLOTS_DIR" ]; then
    mkdir -p "$PLOTS_DIR"
fi

echo "Generating stacked faceted plots for: $EXP_DIR"

# Try python3 then python
PLOT_EXIT_CODE=0
if command -v python3 &>/dev/null; then
    python3 "$SCRIPT_DIR/plot_stacked_faceted.py" "$ANALYSIS_DIR" "$PLOTS_DIR" "$EXP_DIR" "$OUTPUT_FILENAME" || PLOT_EXIT_CODE=$?
else
    python "$SCRIPT_DIR/plot_stacked_faceted.py" "$ANALYSIS_DIR" "$PLOTS_DIR" "$EXP_DIR" "$OUTPUT_FILENAME" || PLOT_EXIT_CODE=$?
fi

if [ $PLOT_EXIT_CODE -ne 0 ]; then
    echo "Error: Plotting failed with exit code $PLOT_EXIT_CODE"
    exit $PLOT_EXIT_CODE
fi

echo "Stacked faceted plot generated in $PLOTS_DIR/$OUTPUT_FILENAME"
