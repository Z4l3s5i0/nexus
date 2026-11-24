#!/bin/bash

# Log Processor for Background Continuous Log Dumping
# Usage: ./log_processor.sh <chainId> <log_dump_dir>

CHAIN_ID=$1
LOG_DUMP_DIR=$2
COMPOSE_FILE="docker-compose.v2.yml"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../../experiments/shared/network/common.sh"
COMPOSE_CMD=$(get_compose_cmd)

mkdir -p "$LOG_DUMP_DIR"

# Function to process logs for a single container
process_container_logs() {
    local container_id=$1
    local container_name=$2
    local chain_id=$3
    local dump_dir=$4
    
    echo "Starting log processor for $container_name..."
    
    local line_count=0
    local file_number=0
    
    # We need to find the next available file number if the process was restarted
    while [ -f "${dump_dir}/${container_name}_${chain_id}_${file_number}.log" ]; do
        file_number=$((file_number + 1))
    done
    
    local current_out_file="${dump_dir}/${container_name}_${chain_id}_${file_number}.log"
    
    # Use docker logs --follow to get a stream of logs
    # Note: --tail all to get history since start, or 0 for only new logs
    docker logs -f --tail all "$container_id" 2>&1 | while IFS= read -r line || [ -n "$line" ]; do
        echo "$line" >> "$current_out_file"
        line_count=$((line_count + 1))
        
        if [ "$line_count" -ge 10000 ]; then
            line_count=0
            file_number=$((file_number + 1))
            current_out_file="${dump_dir}/${container_name}_${chain_id}_${file_number}.log"
        fi
    done
}

# Main loop to discover and track containers
declare -A tracked_pids

cleanup() {
    echo "Stopping log processor..."
    for pid in "${tracked_pids[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    exit 0
}

trap cleanup SIGTERM SIGINT

while true; do
    # Get all service names
    SERVICES=$($COMPOSE_CMD -f "$COMPOSE_FILE" config --services 2>/dev/null || echo "")
    
    for SERVICE in $SERVICES; do
        CONTAINER_ID=$($COMPOSE_CMD -f "$COMPOSE_FILE" ps -q "$SERVICE" 2>/dev/null || echo "")
        if [ -z "$CONTAINER_ID" ]; then
            continue
        fi
        
        # If not already tracked
        if [ -z "${tracked_pids[$CONTAINER_ID]}" ]; then
            CONTAINER_NAME=$(docker inspect --format '{{.Name}}' "$CONTAINER_ID" | sed 's/\///')
            
            # Start background processing for this container
            process_container_logs "$CONTAINER_ID" "$CONTAINER_NAME" "$CHAIN_ID" "$LOG_DUMP_DIR" &
            tracked_pids[$CONTAINER_ID]=$!
            echo "Started tracking $CONTAINER_NAME (PID: ${tracked_pids[$CONTAINER_ID]})"
        fi
    done
    
    # Check if any tracked processes have died
    for cid in "${!tracked_pids[@]}"; do
        if ! kill -0 "${tracked_pids[$cid]}" 2>/dev/null; then
            echo "Process for $cid stopped. Removing from tracking."
            unset tracked_pids[$cid]
        fi
    done
    
    sleep 10
done
