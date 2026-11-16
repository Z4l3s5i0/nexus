#!/bin/bash
set -e

# Experiment Result Collection Script
# Moves all relevant logs, databases, and state files into a single folder for analysis.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/common.sh"
COMPOSE_FILE="docker-compose.v2.yml"
if [ ! -f "$COMPOSE_FILE" ]; then
    echo "Error: $COMPOSE_FILE is required to stop the metrics writer safely." >&2
    exit 1
fi
COMPOSE_CMD=$(get_compose_cmd)
EXP_DIR="experiments/exp_$(date +%Y%m%d_%H%M%S)"
mkdir -p "$EXP_DIR"

echo "Collecting experiment results into $EXP_DIR..."

# Capture registry and the exact Geth exposition while the services are running.
if [ -f data_v2/experiment_metadata.json ]; then
    cp data_v2/experiment_metadata.json "$EXP_DIR/metadata.json"
    while read -r index port; do
        [ -n "$index" ] || continue
        if ! $COMPOSE_CMD -f "$COMPOSE_FILE" exec -T metrics-server \
            curl -fsS --max-time 10 "http://el-node-$index:$port/debug/metrics/prometheus" \
            > "$EXP_DIR/geth-metrics-node-$index.prom"; then
            rm -f "$EXP_DIR/geth-metrics-node-$index.prom"
            echo "Warning: Geth node $index metric inventory is unavailable." >&2
        fi
    done < <(python3 -c 'import json; d=json.load(open("data_v2/experiment_metadata.json")); [print(n["index"], 9050+n["index"]) for n in d["nodes"] if n["type"] == "geth"]')
fi
curl -fsS --max-time 5 http://127.0.0.1:9100/api/nodes > "$EXP_DIR/registry-nodes.json" || rm -f "$EXP_DIR/registry-nodes.json"
cp "$SCRIPT_DIR/GETH_METRICS.md" "$EXP_DIR/GETH_METRICS.md"

# A stopped writer is required for a consistent redb copy. Stop Prometheus as
# well before copying its volume; resolve the volume from this exact container.
PROM_CONTAINER=$($COMPOSE_CMD -f "$COMPOSE_FILE" ps -q prometheus)
METRICS_CONTAINER=$($COMPOSE_CMD -f "$COMPOSE_FILE" ps -q metrics-server)
if [ -z "$METRICS_CONTAINER" ]; then
    echo "Error: metrics-server container is not running; cannot confirm a clean snapshot." >&2
    exit 1
fi
PROM_VOLUME=""
if [ -n "$PROM_CONTAINER" ]; then
    PROM_VOLUME=$(docker inspect --format '{{range .Mounts}}{{if eq .Destination "/prometheus"}}{{.Name}}{{end}}{{end}}' "$PROM_CONTAINER")
fi
$COMPOSE_CMD -f "$COMPOSE_FILE" stop metrics-server prometheus
METRICS_EXIT=$(docker inspect --format '{{.State.ExitCode}}' "$METRICS_CONTAINER")
if [ "$METRICS_EXIT" != 0 ]; then
    echo "Error: metrics-server exited with code $METRICS_EXIT; refusing to archive an unconfirmed database snapshot." >&2
    exit 1
fi

# 1. Metrics Server Database
if [ -f "data_v2/metrics-server/metrics.redb" ]; then
    echo "Copying metrics_server database..."
    cp -a "data_v2/metrics-server" "$EXP_DIR/metrics_server_data"
else
    echo "Error: metrics database data_v2/metrics-server/metrics.redb not found after stopping the writer." >&2
    exit 1
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
if [ -n "$PROM_VOLUME" ]; then
    mkdir -p "$EXP_DIR/prometheus_data"
    docker run --rm -v "$PROM_VOLUME:/from:ro" -v "$(pwd)/$EXP_DIR/prometheus_data:/to" alpine cp -a /from/. /to/
else
    echo "Warning: no Prometheus volume found for this Compose project."
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
- \`metadata.json\`: Runtime/client mapping, node IDs, images and workload parameters.
- \`geth-metrics-node-*.prom\`: Observed Geth metric exposition, where available.
- \`metrics_server_data/\`: redb database copied after stopping metrics-server.
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
For native and WASIX nodes, execution data may be in \`data_v2/node-i/el/eth.redb\`.
Geth uses a different storage layout; do not apply the wasix_eth schema to it.
Node state is copied while execution clients are running and is not a guaranteed consistent snapshot.
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
