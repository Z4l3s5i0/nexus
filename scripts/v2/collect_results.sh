#!/bin/bash
set -e

# Experiment Result Collection Script
# Moves all relevant logs, databases, and state files into a single folder for analysis.

EXP_DIR="experiments/exp_$(date +%Y%m%d_%H%M%S)"
mkdir -p "$EXP_DIR"

echo "Collecting experiment results into $EXP_DIR..."

# 1. Metrics Server Database
if [ -d "metrics_server/data" ]; then
    echo "Copying metrics_server database..."
    cp -r "metrics_server/data" "$EXP_DIR/metrics_server_data"
else
    echo "Warning: metrics_server/data not found."
fi

# 2. Logs Dump
if [ -d "logs_dump" ]; then
    echo "Copying logs_dump..."
    # Ensure logs are flushed to disk
    sync || true
    cp -a "logs_dump" "$EXP_DIR/logs_dump"
elif [ -d "dump_logs" ]; then
    echo "Copying dump_logs..."
    sync || true
    cp -a "dump_logs" "$EXP_DIR/logs_dump"
else
    echo "Warning: logs_dump/dump_logs not found."
fi

# 3. .contender_state
if [ -d ".contender_state" ]; then
    echo "Copying .contender_state..."
    cp -r ".contender_state" "$EXP_DIR/contender_state"
else
    echo "Warning: .contender_state not found."
fi

# 4. Node State and Blockchain (data_v2)
if [ -d "data_v2" ]; then
    echo "Copying node state and blockchain data (data_v2)..."
    cp -r "data_v2" "$EXP_DIR/data_v2"
else
    echo "Warning: data_v2 not found."
fi

# 5. Startup Configuration (startup_v2)
if [ -d "startup_v2" ]; then
    echo "Copying startup configuration (startup_v2)..."
    cp -r "startup_v2" "$EXP_DIR/startup_v2"
else
    echo "Warning: startup_v2 not found."
fi

# 6. Docker Compose File
if [ -f "docker-compose.v2.yml" ]; then
    echo "Copying docker-compose.v2.yml..."
    cp "docker-compose.v2.yml" "$EXP_DIR/"
fi

# 7. Prometheus Database
# Since Prometheus data is usually in a volume, we try to copy it out using a temporary container
COMPOSE_FILE="docker-compose.v2.yml"
if [ -f "$COMPOSE_FILE" ]; then
    echo "Attempting to extract Prometheus database from volume..."
    # Determine the volume name (standard is <project>_prometheus-data)
    # We can try to get it from docker-compose if possible, or assume a pattern
    PROJECT_NAME=$(basename "$(pwd)" | tr -cd '[:alnum:]' | tr '[:upper:]' '[:lower:]')
    VOLUME_NAME="${PROJECT_NAME}_prometheus-data"
    
    # Check if volume exists
    if docker volume inspect "$VOLUME_NAME" >/dev/null 2>&1; then
        echo "Extracting volume $VOLUME_NAME..."
        docker run --rm -v "$VOLUME_NAME:/from" -v "$(pwd)/$EXP_DIR:/to" alpine tar -cz -C /from . | tar -xz -C "$EXP_DIR/prometheus_data" 2>/dev/null || \
        mkdir -p "$EXP_DIR/prometheus_data" && docker run --rm -v "$VOLUME_NAME:/from" -v "$(pwd)/$EXP_DIR/prometheus_data:/to" alpine cp -a /from/. /to/
    else
        echo "Warning: Prometheus volume $VOLUME_NAME not found. Trying default names..."
        # Fallback to some common names
        for vol in "prometheus-data" "v2_prometheus-data"; do
            if docker volume inspect "$vol" >/dev/null 2>&1; then
                echo "Extracting volume $vol..."
                mkdir -p "$EXP_DIR/prometheus_data"
                docker run --rm -v "$vol:/from" -v "$(pwd)/$EXP_DIR/prometheus_data:/to" alpine cp -a /from/. /to/
                break
            fi
        done
    fi
fi

# 8. Aggregate Logs
if [ -d "$EXP_DIR/logs_dump" ]; then
    echo "Aggregating logs..."
    bash "$(dirname "$0")/aggregate_logs.sh" "$EXP_DIR/logs_dump" "$EXP_DIR/aggregated_logs"
fi

# 9. Reuse Instructions
cat <<EOF > "$EXP_DIR/README_ANALYSIS.md"
# Experiment Analysis Guide

This folder contains all data collected from the experiment run.

## Data Structure
- \`metrics_server_data/\`: Raw redb database from metrics-server.
- \`logs_dump/\`: Split log files.
- \`aggregated_logs/\`: Consolidated log files per service.
- \`data_v2/\`: Node-specific execution and consensus data.
- \`prometheus_data/\`: Metrics database.

## How to reuse databases for analysis

### 1. Logs Analysis
Consolidated logs are in \`aggregated_logs/\`. You can use standard tools like \`grep\`, \`awk\`, or more advanced log analyzers.

### 2. Metrics Server Data (redb)
The metrics server stores experimental results in a \`redb\` database at \`metrics_server_data/metrics.redb\`. 
You can use the provided \`db_exporter\` tool to export this data to JSON:
\`\`\`bash
# Build the exporter (one-time)
cd ../../scripts/v2/db_exporter && cargo build --release && cd -
# Export nodes table
../../scripts/v2/db_exporter/target/release/db_exporter metrics --db-path metrics_server_data/metrics.redb --table nodes > nodes.json
\`\`\`
Tables included: \`experiments\`, \`nodes\`, \`metric_samples\`, \`rpc_observations\`.

### 3. Node Execution Data (redb)
Each node's execution data is in \`data_v2/node-i/el/eth.redb\`. 
It follows the schema defined in \`wasix_eth_storage/src/tables.rs\`.
You can use the \`wasix_eth_storage\` crate as a library to read this data programmatically.

### 4. Prometheus Data
The Prometheus database can be reused by starting a local Prometheus instance pointing to the \`prometheus_data\` folder:
\`\`\`bash
docker run -d --name prom-analysis -p 9091:9090 -v \$(pwd)/prometheus_data:/prometheus prom/prometheus --storage.tsdb.path=/prometheus
\`\`\`
Then access http://localhost:9091 to query metrics.
EOF

echo ""
EXP_DIR_ABS=$(cd "$EXP_DIR" && pwd)
echo "Experiment results collected in: $EXP_DIR_ABS"
echo "RESULT_DIR=$EXP_DIR_ABS"
echo "Analysis guide created: $EXP_DIR_ABS/README_ANALYSIS.md"
echo "Structure:"
ls -F "$EXP_DIR_ABS"
