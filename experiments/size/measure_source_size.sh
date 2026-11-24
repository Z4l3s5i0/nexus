#!/usr/bin/env bash
set -euo pipefail

SOURCES=()
OUTPUT=""
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export PATH="$CARGO_HOME/bin:$PATH"

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
if ! command -v scc >/dev/null 2>&1 || ! command -v python3 >/dev/null 2>&1; then
    echo "Error: scc and Python 3 are required for source size measurements; run experiments/setup.sh with the size stage selected." >&2
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
    code_lines="$(scc --format csv --no-cocomo --no-complexity --no-size \
        --no-config --no-scc-ignore --no-gitignore --no-ignore --no-gen \
        --exclude-dir=.git,node_modules,target,build,dist,coverage,generated,vendor,.venv,venv \
        --not-match '(^|/)(test|tests|__tests__)(/|$)|(^|/)(test_[^/]*|[^/]*_test|[^/]*\.(test|spec))[^/]*$' \
        "$path" | python3 -c '
import csv
import sys

reader = csv.DictReader(sys.stdin)
fields = {name.lower(): name for name in (reader.fieldnames or [])}
if "code" not in fields:
    raise SystemExit("scc CSV output is missing the Code column")
code_column = fields["code"]
language_column = fields.get("language")
total = sum(
    int(row[code_column] or 0)
    for row in reader
    if not language_column or row.get(language_column, "").lower() not in {"total", "sum"}
)
print(total)
')"
    printf 'source_code\t%s\t%s\tlines\t%s\tmeasured\n' "$label" "$code_lines" "$path"
done