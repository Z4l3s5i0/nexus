#!/bin/bash
set -e

# Script to build and push wasix_eth wasix node to a Docker registry

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

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

build_push_node() {
    TYPE="wasix"
    echo "Checking for $TYPE node binary..."
    NODE_BIN="$ROOT_DIR/wasix_eth/target/wasm32-wasmer-wasi/release/wasix_eth.wasi.wasm"
    if [ ! -f "$NODE_BIN" ]; then
        echo -e "\e[31mERROR: $TYPE node binary not found at $NODE_BIN.\e[0m"
        echo "Please build it first: cd wasix_eth && cargo wasix build --release"
        return
    fi

    echo "Building and pushing wasix-eth-${TYPE} image..."
    IMAGE="${REGISTRY}/wasix-eth-${TYPE}:${TAG}"
    if ! docker build -t "$IMAGE" -f "$ROOT_DIR/wasix_eth/Dockerfile.${TYPE}" "$ROOT_DIR/wasix_eth"; then
        echo -e "\e[31mERROR: Failed to build $IMAGE. Skipping push.\e[0m"
        return
    fi
    if ! docker push "$IMAGE"; then
        echo -e "\e[31mERROR: Failed to push $IMAGE. Ensure you are logged in (docker login) and have permission.\e[0m"

        # Diagnostic help
        if [[ "$REGISTRY" != *.* ]] && [[ "$REGISTRY" != */* ]]; then
            echo -e "\e[31mERROR: Registry '$REGISTRY' seems to be missing a hostname.\e[0m"
            echo -e "\e[33mTIP: If you are using Docker Hub, use 'docker.io/$REGISTRY' as your registry.\e[0m"
        fi

        if [[ "$REGISTRY" == ghcr.io* ]]; then
            echo -e "\e[33mTIP: For GitHub Container Registry (ghcr.io), ensure your Personal Access Token (PAT) has the 'write:packages' scope.\e[0m"
        elif [[ "$REGISTRY" == *azurecr.io* ]]; then
            echo -e "\e[33mTIP: For Azure Container Registry, ensure you have the 'AcrPush' role or equivalent permissions.\e[0m"
        fi
    fi
}

build_push_node

echo "Process completed for ${REGISTRY} with tag ${TAG}"
