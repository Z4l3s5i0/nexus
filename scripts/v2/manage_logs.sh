#!/bin/bash

# CLI to manage background log processing
# Usage: ./manage_logs.sh [start|stop|status] [--chain-id ID] [--dir PATH]

CMD=$1
CHAIN_ID=12345
LOG_DUMP_DIR="./logs_dump"
PID_FILE=".log_processor.pid"

shift
while [[ "$#" -gt 0 ]]; do
    case $1 in
        --chain-id) CHAIN_ID="$2"; shift ;;
        --dir) LOG_DUMP_DIR="$2"; shift ;;
    esac
    shift
done

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROCESSOR_SCRIPT="$SCRIPT_DIR/log_processor.sh"

start() {
    if [ -f "$PID_FILE" ]; then
        PID=$(cat "$PID_FILE")
        if kill -0 "$PID" 2>/dev/null; then
            echo "Log processor is already running (PID: $PID)"
            return
        fi
        rm "$PID_FILE"
    fi

    echo "Starting background log processor (Chain ID: $CHAIN_ID, Dir: $LOG_DUMP_DIR)..."
    nohup bash "$PROCESSOR_SCRIPT" "$CHAIN_ID" "$LOG_DUMP_DIR" > log_processor.out 2>&1 &
    echo $! > "$PID_FILE"
    echo "Started (PID: $(cat "$PID_FILE"))"
}

stop() {
    if [ ! -f "$PID_FILE" ]; then
        echo "Log processor is not running."
        return
    fi

    PID=$(cat "$PID_FILE")
    echo "Stopping log processor (PID: $PID)..."
    
    # Kill the main processor script and its children
    if command -v pkill &> /dev/null; then
        pkill -P "$PID" 2>/dev/null || true
    else
        # Fallback: find children using ps and kill them
        CHILDREN=$(ps -o pid --no-headers --ppid "$PID" 2>/dev/null || echo "")
        if [ -n "$CHILDREN" ]; then
            kill $CHILDREN 2>/dev/null || true
        fi
    fi
    kill "$PID" 2>/dev/null || true
    
    # Wait for the process to actually terminate
    local timeout=30
    while kill -0 "$PID" 2>/dev/null && [ $timeout -gt 0 ]; do
        sleep 1
        timeout=$((timeout - 1))
    done

    if kill -0 "$PID" 2>/dev/null; then
        echo "Warning: Log processor (PID: $PID) did not stop in time. Force killing..."
        kill -9 "$PID" 2>/dev/null || true
    fi
    
    rm "$PID_FILE"
    echo "Stopped."
}

status() {
    if [ -f "$PID_FILE" ]; then
        PID=$(cat "$PID_FILE")
        if kill -0 "$PID" 2>/dev/null; then
            echo "Log processor is running (PID: $PID)"
            return
        fi
        echo "Log processor PID file exists but process is not running."
        return
    fi
    echo "Log processor is not running."
}

case $CMD in
    start) start ;;
    stop) stop ;;
    status) status ;;
    *) echo "Usage: $0 [start|stop|status] [--chain-id ID] [--dir PATH]" ;;
esac
