#!/usr/bin/env bash
set -euo pipefail

SOURCES=()
BINARIES=()
IMAGES=()
OUTPUT=""

usage() {
    cat <<EOF
Usage: $0 [--source LABEL=DIR]... [--binary LABEL=FILE]... [--image LABEL=IMAGE]... [--output FILE]

Compare source-code lines, compiled binary byte sizes, and Docker image byte sizes.
Source counts use cloc (which excludes blank/comment lines) and omit common generated,
vendor, build, and test directories/files.

Examples:
  $0 --source nexus=../wasix_eth --source geth=../go-ethereum
  $0 --binary nexus=../target/release/nexus --image nexus=nexus:latest
EOF
}

parse_entry() {
    local value="$1"
    local label="${value%%=*}"
    local path="${value#*=}"
    if [ "$label" = "$value" ] || [ -z "$label" ] || [ -z "$path" ]; then
        echo "Error: expected LABEL=PATH, got '$value'." >&2
        return 1
    fi
    ENTRY_LABEL="$label"
    ENTRY_PATH="$path"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --source|--binary|--image|--output)
            if [ "$#" -lt 2 ]; then
                echo "Error: $1 requires a value." >&2
                exit 1
            fi
            option="$1"
            value="$2"
            if [ "$option" = "--source" ]; then
                parse_entry "$value"
                SOURCES+=("$ENTRY_LABEL=$ENTRY_PATH")
            elif [ "$option" = "--binary" ]; then
                parse_entry "$value"
                BINARIES+=("$ENTRY_LABEL=$ENTRY_PATH")
            elif [ "$option" = "--image" ]; then
                parse_entry "$value"
                IMAGES+=("$ENTRY_LABEL=$ENTRY_PATH")
            else
                OUTPUT="$value"
            fi
            shift 2
            ;;
        --help|-h)
            usage
            exit 0
            ;;
        *)
            echo "Error: unknown option '$1'." >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [ "${#SOURCES[@]}" -eq 0 ] && [ "${#BINARIES[@]}" -eq 0 ] && [ "${#IMAGES[@]}" -eq 0 ]; then
    usage >&2
    exit 1
fi

for entry in "${SOURCES[@]}"; do
    path="${entry#*=}"
    if [ ! -d "$path" ]; then
        echo "Error: source directory not found: $path" >&2
        exit 1
    fi
done
for entry in "${BINARIES[@]}"; do
    path="${entry#*=}"
    if [ ! -f "$path" ]; then
        echo "Error: binary file not found: $path" >&2
        exit 1
    fi
done
if [ "${#SOURCES[@]}" -gt 0 ] && ! command -v cloc >/dev/null 2>&1; then
    echo "Error: cloc is required for source-code line counts." >&2
    exit 1
fi
if [ "${#SOURCES[@]}" -gt 0 ]; then
    if command -v python3 >/dev/null 2>&1; then
        PYTHON=python3
    elif command -v python >/dev/null 2>&1; then
        PYTHON=python
    else
        echo "Error: Python is required to parse cloc output." >&2
        exit 1
    fi
fi
if [ "${#IMAGES[@]}" -gt 0 ] && ! command -v docker >/dev/null 2>&1; then
    echo "Error: Docker is required to inspect image sizes." >&2
    exit 1
fi
if [ -n "$OUTPUT" ]; then
    if [ -e "$OUTPUT" ]; then
        echo "Error: output file already exists: $OUTPUT" >&2
        exit 1
    fi
    mkdir -p "$(dirname "$OUTPUT")"
    exec > "$OUTPUT"
fi

printf 'measurement\tlabel\tvalue\tunit\tsource\n'

for entry in "${SOURCES[@]}"; do
    label="${entry%%=*}"
    path="${entry#*=}"
    cloc_json=$(cloc --quiet --json --fullpath \
        --exclude-dir=.git,node_modules,target,build,dist,coverage,generated,vendor,.venv,venv \
        --not-match-d='(^|/)(test|tests|__tests__)(/|$)' \
        --not-match-f='(^|/)(test_|.*_test|.*\.test|.*\.spec)[^/]*$' "$path")
    code_lines=$(printf '%s' "$cloc_json" | "$PYTHON" -c 'import json,sys; print(json.load(sys.stdin)["SUM"]["code"])')
    printf 'source_code\t%s\t%s\tlines\t%s\n' "$label" "$code_lines" "$path"
done

for entry in "${BINARIES[@]}"; do
    label="${entry%%=*}"
    path="${entry#*=}"
    bytes=$(wc -c < "$path" | tr -d '[:space:]')
    printf 'compiled_binary\t%s\t%s\tbytes\t%s\n' "$label" "$bytes" "$path"
done

for entry in "${IMAGES[@]}"; do
    label="${entry%%=*}"
    image="${entry#*=}"
    bytes=$(docker image inspect --format '{{.Size}}' "$image")
    printf 'docker_image\t%s\t%s\tbytes\t%s\n' "$label" "$bytes" "$image"
done

if [ -n "$OUTPUT" ]; then
    echo "Size comparison written to $OUTPUT" >&2
fi