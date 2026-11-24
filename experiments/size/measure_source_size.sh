#!/usr/bin/env bash
set -euo pipefail

SOURCES=()
OUTPUT=""

usage() {
    echo "Usage: bash measure_source_size.sh --source LABEL=DIR... [--output TSV]" >&2
}

parse_entry() {
    local value="$1"
    ENTRY_LABEL="${value%%=*}"
    ENTRY_PATH="${value#*=}"
    [[ "$ENTRY_LABEL" != "$value" && -n "$ENTRY_LABEL" && -n "$ENTRY_PATH" ]] || {
        echo "Error: expected LABEL=DIR, got '$value'." >&2
        exit 2
    }
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --source)
            [[ "$#" -ge 2 ]] || { usage; exit 2; }
            parse_entry "$2"
            SOURCES+=("$ENTRY_LABEL=$ENTRY_PATH")
            shift 2
            ;;
        --output)
            [[ "$#" -ge 2 ]] || { usage; exit 2; }
            OUTPUT="$2"
            shift 2
            ;;
        --help|-h) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage; exit 2 ;;
    esac
done

[[ "${#SOURCES[@]}" -gt 0 ]] || { usage; exit 2; }
if [[ -n "$OUTPUT" ]]; then
    [[ ! -e "$OUTPUT" ]] || { echo "Error: output already exists: $OUTPUT" >&2; exit 1; }
    mkdir -p "$(dirname "$OUTPUT")"
    exec > "$OUTPUT"
fi
if ! command -v cloc >/dev/null 2>&1 || ! command -v python3 >/dev/null 2>&1; then
    echo "Error: cloc and Python 3 are required for source size measurements." >&2
    exit 1
fi

printf 'measurement\tlabel\tvalue\tunit\tsource\tstatus\n'
for entry in "${SOURCES[@]}"; do
    label="${entry%%=*}"
    path="${entry#*=}"
    if [[ ! -d "$path" ]]; then
        printf 'source_code\t%s\t\tlines\t%s\tunavailable\n' "$label" "$path"
        continue
    fi
    cloc_json="$(cloc --quiet --json --fullpath \
        --exclude-dir=.git,node_modules,target,build,dist,coverage,generated,vendor,.venv,venv \
        --not-match-d='(^|/)(test|tests|__tests__)(/|$)' \
        --not-match-f='(^|/)(test_|.*_test|.*\.test|.*\.spec)[^/]*$' "$path")"
    code_lines="$(printf '%s' "$cloc_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["SUM"]["code"])')"
    printf 'source_code\t%s\t%s\tlines\t%s\tmeasured\n' "$label" "$code_lines" "$path"
done