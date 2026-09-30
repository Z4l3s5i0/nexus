#!/bin/bash

# Default values
REGISTRY="docker.io/z4l3s5i0"
TAG="latest"
INTERVAL=60 # 1 minutes
ONCE=false

usage() {
    echo "Usage: $0 --registry <registry_url> [--tag <tag>] [--interval <seconds>] [--once]"
    echo "Example: $0 --registry docker.io/myuser"
    exit 1
}

while [[ "$#" -gt 0 ]]; do
    case $1 in
        --registry) REGISTRY="$2"; shift ;;
        --tag) TAG="$2"; shift ;;
        --interval) INTERVAL="$2"; shift ;;
        --once) ONCE=true ;;
        *) usage ;;
    esac
    shift
done

if [ -z "$REGISTRY" ]; then
    echo "Error: --registry is required."
    usage
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

LINUX_IMAGE="${REGISTRY}/wasix-eth-linux:${TAG}"
WASIX_IMAGE="${REGISTRY}/wasix-eth-wasix:${TAG}"

# Store last seen digests
LAST_LINUX_DIGEST=""
LAST_WASIX_DIGEST=""

# Get current image digests
get_digest() {
    local image=$1
    # We use '|| true' to avoid script exit on error if image not found
    docker inspect --format='{{.RepoDigests}}' "$image" 2>/dev/null || true
}

# Pull and check for updates
check_and_pull() {
    local image=$1
    local last_digest_var=$2
    echo "Checking for updates for $image..."
    
    local old_digest=$(get_digest "$image")
    
    # Try to pull the latest image and capture output
    local pull_output
    pull_output=$(docker pull "$image" 2>&1)
    local pull_exit=$?
    
    echo "$pull_output"
    
    if [ $pull_exit -ne 0 ]; then
        echo "Failed to pull $image."
        return 1
    fi
    
    local new_digest=$(get_digest "$image")
    local remote_digest=$(echo "$pull_output" | grep "Digest: " | cut -d' ' -f2 | tr -d '\r')

    # Update the global LAST_..._DIGEST variable
    local updated=false
    local last_seen_digest="${!last_digest_var}"
    
    # Check if we downloaded something new OR if RepoDigests changed
    if echo "$pull_output" | grep -q "Downloaded newer image" || [ "$old_digest" != "$new_digest" ]; then
        echo "Image $image updated!"
        updated=true
    # Check if the registry digest changed since we last checked
    elif [ -n "$last_seen_digest" ] && [ -n "$remote_digest" ] && [ "$remote_digest" != "$last_seen_digest" ]; then
        echo "Image $image updated (Registry digest changed: $last_seen_digest -> $remote_digest)!"
        updated=true
    fi

    # Store the remote digest for next time
    if [ -n "$remote_digest" ]; then
        eval "$last_digest_var=\"$remote_digest\""
    fi

    if [ "$updated" = true ]; then
        return 0 # Updated
    fi
    
    echo "Image $image is up to date."
    return 2 # Not updated
}

ensure_hiveview_running() {
    # 1. Build hive-custom if it doesn't exist
    if [[ "$(docker images -q hive-custom:latest 2> /dev/null)" == "" ]]; then
        echo "Building hive-custom image..."
        docker build -f "$ROOT_DIR/hive/hive.dockerfile" -t hive-custom "$ROOT_DIR"
    fi

    # Ensure workspace directory exists
    mkdir -p "$ROOT_DIR/workspace"

    # Start hiveview in the background if not already running
    if [[ "$(docker ps -q -f name=hiveview 2> /dev/null)" == "" ]]; then
        echo "Starting Hiveview server in the background..."
        docker run -d --rm \
          --name hiveview \
          -p 8080:8080 \
          -v "$ROOT_DIR/workspace:/hive/workspace" \
          --entrypoint ./hiveview \
          hive-custom \
          --serve --logdir /hive/workspace/logs
    fi
}

run_hive_tests() {
    local client=$1
    echo "Starting HIVE tests for client: $client..."

    docker run --rm \
      -v /var/run/docker.sock:/var/run/docker.sock \
      -v "$ROOT_DIR/workspace:/hive/workspace" \
      hive-custom \
      --sim ethereum/engine \
      --client "$client" \
      --sim.parallelism 20
}

perform_check() {
    check_and_pull "$LINUX_IMAGE" "LAST_LINUX_DIGEST"
    if [ $? -eq 0 ]; then
        echo "Image $LINUX_IMAGE updated. Running HIVE tests for wasix-r-eth."
        run_hive_tests "wasix-r-eth"
    else
        echo "No updates found for $LINUX_IMAGE."
    fi
    
    check_and_pull "$WASIX_IMAGE" "LAST_WASIX_DIGEST"
    if [ $? -eq 0 ]; then
        echo "Image $WASIX_IMAGE updated. Running HIVE tests for wasix-w-eth."
        run_hive_tests "wasix-w-eth"
    else
        echo "No updates found for $WASIX_IMAGE."
    fi
}

ensure_hiveview_running

if [ "$ONCE" = true ]; then
    perform_check
else
    echo "Starting check loop every $INTERVAL seconds..."
    while true; do
        perform_check
        echo "Waiting for $INTERVAL seconds before next check..."
        sleep "$INTERVAL"
    done
fi
