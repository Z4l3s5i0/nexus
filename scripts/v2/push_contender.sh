#!/bin/bash
set -e

# Script to build and push contender to a Docker registry

REGISTRY=""
TAG="latest"

usage() {
    echo "Usage: $0 --registry <registry_url> [--tag <tag>]"
    echo "Example: $0 --registry myuser"
    exit 1
}

while [[ "$#" -gt 0 ]]; do
    case $1 in
        --registry) REGISTRY="$2"; shift ;;
        --tag) TAG="$2"; shift ;;
        *) usage ;;
    esac
    shift
done

if [ -z "$REGISTRY" ]; then
    echo "Error: --registry is required."
    usage
fi

# Determine the contender repository directory
# Assuming it's in the same parent directory as wasm-based-evm
# based on previous issue context (D:/MA/contender)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTENDER_DIR="$(cd "$SCRIPT_DIR/../../../contender" && pwd)"

if [ ! -d "$CONTENDER_DIR" ]; then
    echo "Error: Contender directory not found at $CONTENDER_DIR"
    exit 1
fi

echo "Building and pushing contender..."
CONTENDER_IMAGE="${REGISTRY}/contender:${TAG}"

docker build -t "$CONTENDER_IMAGE" -f "$CONTENDER_DIR/Dockerfile" "$CONTENDER_DIR"
docker push "$CONTENDER_IMAGE"

echo "Process completed for contender at ${REGISTRY} with tag ${TAG}"
