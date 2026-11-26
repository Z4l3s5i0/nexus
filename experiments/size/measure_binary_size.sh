#!/usr/bin/env bash
set -euo pipefail

BINARIES=()
OUTPUT=""

usage() {
    echo "Usage: bash measure_binary_size.sh --binary LABEL=FILE... [--output TSV]" >&2
}

parse_entry() {
    local value="$1"
    ENTRY_LABEL="${value%%=*}"
    ENTRY_PATH="${value#*=}"
    [[ "$ENTRY_LABEL" != "$value" && -n "$ENTRY_LABEL" && -n "$ENTRY_PATH" ]] || {
        echo "Error: expected LABEL=FILE, got '$value'." >&2
        exit 2
    }
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --binary)
            [[ "$#" -ge 2 ]] || { usage; exit 2; }
            parse_entry "$2"
            BINARIES+=("$ENTRY_LABEL=$ENTRY_PATH")
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

[[ "${#BINARIES[@]}" -gt 0 ]] || { usage; exit 2; }
if [[ -n "$OUTPUT" ]]; then
    [[ ! -e "$OUTPUT" ]] || { echo "Error: output already exists: $OUTPUT" >&2; exit 1; }
    mkdir -p "$(dirname "$OUTPUT")"
    exec > "$OUTPUT"
fi

printf 'measurement\tlabel\tvalue\tunit\tsource\tstatus\n'
for entry in "${BINARIES[@]}"; do
    label="${entry%%=*}"
    path="${entry#*=}"
    if [[ ! -f "$path" ]]; then
        printf 'Size progress: Compiled binary size measurement unavailable for %s; file was not found.\n' "$label" >&2
        printf 'compiled_binary\t%s\t\tbytes\t%s\tunavailable\n' "$label" "$path"
        continue
    fi
    printf 'Size progress: Measuring compiled binary size for %s.\n' "$label" >&2
    bytes="$(wc -c < "$path" | tr -d '[:space:]')"
    printf 'compiled_binary\t%s\t%s\tbytes\t%s\tmeasured\n' "$label" "$bytes" "$path"
    printf 'Size progress: Compiled binary size measurement finished for %s.\n' "$label" >&2
done