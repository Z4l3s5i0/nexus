#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RUN_ID="${EXPERIMENT_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
OUTPUT_ROOT="${EXPERIMENTS_OUTPUT_ROOT:-$REPO_ROOT/experiment-results}/size"
SOURCES=()
BINARIES=()

size_progress() {
    printf 'Size progress: %s\n' "$*"
}

usage() {
    cat <<EOF
Usage: bash experiments/size/run.sh [options]

Options:
  --source LABEL=DIR    Add a source tree for scc code-line measurement (repeatable)
  --binary LABEL=FILE   Add a compiled binary to measure (repeatable)
  --output-root DIR     Override the parent results directory
  --help                Show this help

By default, the module uses scc to measure the in-tree NEXUS source and looks for native
and WASIX release binaries. Missing external-client paths are unavailable, not zero.
EOF
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --source) SOURCES+=("$2"); shift 2 ;;
        --binary) BINARIES+=("$2"); shift 2 ;;
        --output-root) OUTPUT_ROOT="$2"; shift 2 ;;
        --help|-h) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if [[ ! "$RUN_ID" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]]; then
    echo "Error: EXPERIMENT_RUN_ID contains unsupported characters." >&2
    exit 2
fi
if [[ "${#SOURCES[@]}" -eq 0 ]]; then
    SOURCES+=("wasix-eth=$REPO_ROOT/wasix_eth")
    if [[ -n "${GETH_SOURCE_DIR:-}" ]]; then SOURCES+=("geth=$GETH_SOURCE_DIR"); fi
    if [[ -n "${RETH_SOURCE_DIR:-}" ]]; then SOURCES+=("reth=$RETH_SOURCE_DIR"); fi
fi
if [[ "${#BINARIES[@]}" -eq 0 ]]; then
    BINARIES+=("wasix-eth-native=${WASIX_ETH_NATIVE_BINARY:-$REPO_ROOT/wasix_eth/target/release/wasix_eth}")
    BINARIES+=("wasix-eth-wasix=${WASIX_ETH_WASIX_BINARY:-$REPO_ROOT/wasix_eth/target/wasm32-wasmer-wasi/release/wasix_eth.wasi.wasm}")
    if [[ -n "${GETH_BINARY:-}" ]]; then BINARIES+=("geth=$GETH_BINARY"); fi
    if [[ -n "${RETH_BINARY:-}" ]]; then BINARIES+=("reth=$RETH_BINARY"); fi
fi

mkdir -p "$OUTPUT_ROOT"
OUTPUT_DIR="$OUTPUT_ROOT/$RUN_ID"
if ! mkdir "$OUTPUT_DIR" 2>/dev/null; then
    echo "Error: output directory already exists: $OUTPUT_DIR" >&2
    exit 1
fi

record_run_status() {
    local status=$?
    python3 "$REPO_ROOT/experiments/record_run_status.py" \
        --output "$OUTPUT_DIR/run_status.json" --exit-code "$status" || true
    return "$status"
}
trap record_run_status EXIT

SOURCE_ARGS=()
for source in "${SOURCES[@]}"; do SOURCE_ARGS+=(--source "$source"); done
BINARY_ARGS=()
for binary in "${BINARIES[@]}"; do BINARY_ARGS+=(--binary "$binary"); done
size_progress "Source size measurements starting."
bash "$SCRIPT_DIR/measure_source_size.sh" "${SOURCE_ARGS[@]}" --output "$OUTPUT_DIR/source_size.tsv"
size_progress "Source size measurements finished."
size_progress "Compiled binary size measurements starting."
bash "$SCRIPT_DIR/measure_binary_size.sh" "${BINARY_ARGS[@]}" --output "$OUTPUT_DIR/binary_size.tsv"
size_progress "Compiled binary size measurements finished."
size_progress "Generating the size comparison report."
python3 "$SCRIPT_DIR/report.py" --output-dir "$OUTPUT_DIR" --run-id "$RUN_ID"
size_progress "Size comparison report generated."
echo "Size comparison results: $OUTPUT_DIR"