#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RUN_ID="${EXPERIMENT_RUN_ID:-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
OUTPUT_ROOT="${EXPERIMENTS_OUTPUT_ROOT:-$REPO_ROOT/experiment-results}/compliance"
CLIENTS="wasix-r-eth,wasix-w-eth"
HIVE_IMAGE="hive-custom:latest"
PARALLELISM=20
TIMEOUT=3600
SKIP_BASE_BUILD=false
SKIP_HIVE_BUILD=false
OUTPUT_DIR=""

usage() {
    cat <<EOF
Usage: bash experiments/compliance/run.sh [options]

Options:
  --clients LIST       Comma-separated custom Hive clients (default: $CLIENTS)
  --hive-image IMAGE   Hive runner image (default: $HIVE_IMAGE)
  --parallelism N      Hive simulation parallelism (default: $PARALLELISM)
  --timeout SECONDS   Per-client suite timeout (default: $TIMEOUT)
  --skip-base-build    Use existing custom-client base images
  --skip-hive-build    Use an existing Hive runner image
  --help               Show this help
EOF
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --clients) CLIENTS="$2"; shift 2 ;;
        --hive-image) HIVE_IMAGE="$2"; shift 2 ;;
        --parallelism) PARALLELISM="$2"; shift 2 ;;
        --timeout) TIMEOUT="$2"; shift 2 ;;
        --skip-base-build) SKIP_BASE_BUILD=true; shift ;;
        --skip-hive-build) SKIP_HIVE_BUILD=true; shift ;;
        --help|-h) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

[[ "$PARALLELISM" =~ ^[1-9][0-9]*$ && "$TIMEOUT" =~ ^[1-9][0-9]*$ ]] || {
    echo "Error: parallelism and timeout must be positive integers." >&2
    exit 2
}
if [[ ! "$RUN_ID" =~ ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$ ]]; then
    echo "Error: EXPERIMENT_RUN_ID contains unsupported characters." >&2
    exit 2
fi
IFS=',' read -r -a SELECTED_CLIENTS <<< "$CLIENTS"
if (( ${#SELECTED_CLIENTS[@]} == 0 )); then
    echo "Error: at least one Hive client must be selected." >&2
    exit 2
fi
for client in "${SELECTED_CLIENTS[@]}"; do
    case "$client" in
        wasix-r-eth|wasix-w-eth) ;;
        *) echo "Error: unsupported custom Hive client '$client'." >&2; exit 2 ;;
    esac
done

mkdir -p "$OUTPUT_ROOT"
OUTPUT_DIR="$OUTPUT_ROOT/$RUN_ID"
if ! mkdir "$OUTPUT_DIR" 2>/dev/null; then
    echo "Error: output directory already exists: $OUTPUT_DIR" >&2
    exit 1
fi
mkdir -p "$OUTPUT_DIR/logs" "$OUTPUT_DIR/workspace"

PREPARE_ARGS=(--output-dir "$OUTPUT_DIR" --clients "$CLIENTS" --hive-image "$HIVE_IMAGE")
if [[ "$SKIP_BASE_BUILD" == true ]]; then PREPARE_ARGS+=(--skip-base-build); fi
if [[ "$SKIP_HIVE_BUILD" == true ]]; then PREPARE_ARGS+=(--skip-hive-build); fi
bash "$SCRIPT_DIR/prepare.sh" "${PREPARE_ARGS[@]}"

python3 "$SCRIPT_DIR/capture_images.py" \
    --output "$OUTPUT_DIR/metadata.json" --clients "$CLIENTS" --hive-image "$HIVE_IMAGE"

RUN_STATUS=0
python3 "$SCRIPT_DIR/run_suite.py" \
    --clients "$CLIENTS" --image "$HIVE_IMAGE" --run-id "$RUN_ID" \
    --workspace-root "$OUTPUT_DIR/workspace" --logs-dir "$OUTPUT_DIR/logs" \
    --results "$OUTPUT_DIR/results.jsonl" --parallelism "$PARALLELISM" --timeout "$TIMEOUT" \
    || RUN_STATUS=$?
python3 "$SCRIPT_DIR/report.py" --output-dir "$OUTPUT_DIR" --run-id "$RUN_ID"
echo "Compliance results: $OUTPUT_DIR"
if (( RUN_STATUS != 0 )); then
    exit "$RUN_STATUS"
fi