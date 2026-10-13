#!/bin/bash
set -e

# Local Docker Network Orchestrator for v2 Deployment

source "$(dirname "$0")/common.sh"

NODES=4
LINUX_RATIO=50
WASIX_RATIO=50
CHAIN_ID=12345
CLEANUP=false
SETUP=false
REGISTRY=""
TAG="latest"
COMPOSE_FILE="docker-compose.v2.yml"

usage() {
    echo "Usage: $0 [options]"
    echo "Options:"
    echo "  --nodes N         Total number of nodes (default: 4)"
    echo "  --linux P         Percentage of Linux nodes (default: 50)"
    echo "  --wasix P         Percentage of Wasix nodes (default: 50)"
    echo "  --chain-id ID     Unique chain ID (default: 12345)"
    echo "  --cleanup         Remove existing data before starting"
    echo "  --setup           Run setup.sh locally before starting"
    echo "  --registry URL    Docker registry to pull images from (optional)"
    echo "  --tag TAG         Image tag to use (default: latest)"
    exit 1
}

while [[ "$#" -gt 0 ]]; do
    case $1 in
        --nodes) NODES="$2"; shift ;;
        --linux) LINUX_RATIO="$2"; shift ;;
        --wasix) WASIX_RATIO="$2"; shift ;;
        --chain-id) CHAIN_ID="$2"; shift ;;
        --cleanup) CLEANUP=true ;;
        --setup) SETUP=true ;;
        --registry) REGISTRY="$2"; shift ;;
        --tag) TAG="$2"; shift ;;
        *) usage ;;
    esac
    shift
done

if [ "$SETUP" = true ]; then
    echo "Running local setup..."
    "$(dirname "$0")/setup.sh"
fi

if [ "$CLEANUP" = true ]; then
    echo "Cleaning up old data..."
  # Stop running containers first to release file locks
    COMPOSE_CMD=$(get_compose_cmd)
    if [ -f "$COMPOSE_FILE" ]; then
        $COMPOSE_CMD -f "$COMPOSE_FILE" down --volumes --remove-orphans || true
    fi

    # Use sudo to force-remove files owned by root
    sudo rm -rf startup_v2 data_v2
fi

# Ensure startup_v2 directory exists
mkdir -p startup_v2/grafana/provisioning
mkdir -p startup_v2/grafana/dashboards

# 1. Generate Configuration
echo "Generating network configuration..."
"$(dirname "$0")/config_gen.sh" --chain-id "$CHAIN_ID" --nodes "$NODES" --output "startup_v2"

# Ensure SCRIPT_DIR is set (it might have been set in common.sh or previously)
SCRIPT_DIR_LOCAL="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR_LOCAL="$(cd "$SCRIPT_DIR_LOCAL/../.." && pwd)"

# Copy monitoring configuration
echo "Setting up monitoring configuration..."
cp "$SCRIPT_DIR_LOCAL/monitoring/prometheus.yml" "startup_v2/prometheus.yml"
chmod 644 "startup_v2/prometheus.yml"
cp -r "$SCRIPT_DIR_LOCAL/monitoring/grafana/provisioning/"* "startup_v2/grafana/provisioning/"
chmod -R 755 "startup_v2/grafana/provisioning"

# Copy local monitoring dashboards if they exist
if [ -d "$SCRIPT_DIR_LOCAL/monitoring/grafana/dashboards" ]; then
    cp "$SCRIPT_DIR_LOCAL/monitoring/grafana/dashboards"/*.json "startup_v2/grafana/dashboards/" 2>/dev/null || true
fi

# Try to find dashboards in multiple possible locations
DASHBOARD_SOURCES=(
    "$ROOT_DIR_LOCAL/wasix_eth/grafana/dashboards"
    "$ROOT_DIR_LOCAL/grafana/dashboards"
    "$(dirname "$0")/../../wasix_eth/grafana/dashboards"
)

for src in "${DASHBOARD_SOURCES[@]}"; do
    if [ -d "$src" ]; then
        # Check if there are any .json files to copy
        if ls "$src"/*.json &>/dev/null; then
            echo "Copying dashboards from $src..."
            cp "$src"/*.json "startup_v2/grafana/dashboards/"
        fi
    fi
done

# Ensure all dashboards are readable
chmod 644 "startup_v2/grafana/dashboards"/*.json 2>/dev/null || true

# 2. Get External IP
# Find the first non-loopback IPv4 address
# Try to get it from the default route first, as it's the most reliable way to find the primary IP
EXT_IP=$(ip -4 route get 1.1.1.1 2>/dev/null | grep -oP 'src \K\S+')
if [ -z "$EXT_IP" ]; then
    # Fallback 1: hostname -I
    EXT_IP=$(hostname -I 2>/dev/null | awk '{print $1}')
fi
if [ -z "$EXT_IP" ]; then
    # Fallback 2: get the first non-loopback IPv4 address from ip addr
    EXT_IP=$(ip -4 addr show | grep 'inet ' | grep -v '127.0.0.1' | awk '{print $2}' | cut -d/ -f1 | head -n 1)
fi

if [ -z "$EXT_IP" ]; then
    echo "Error: Could not determine external IP address."
    exit 1
fi
echo "Using external IP: $EXT_IP"

# 3. Calculate node counts
read c_linux c_wasix <<< $(calculate_counts "$NODES" "$LINUX_RATIO" "$WASIX_RATIO")
echo "Node counts: Linux=$c_linux, Wasix=$c_wasix"

# 3. Generate docker-compose.yml
# Prepare variables for template replacement
if [ -n "$REGISTRY" ]; then
    METRICS_IMAGE="${REGISTRY}/wasix-eth-metrics:${TAG}"
    METRICS_BUILD_SECTION="# Registry image used"
else
    METRICS_IMAGE="wasix-eth-metrics:latest"
    METRICS_BUILD_SECTION="build:\n      context: $ROOT_DIR_LOCAL/metrics_server\n      dockerfile: Dockerfile"
fi

    # Use sed to replace placeholders
    # We use a literal newline replacement for METRICS_BUILD_SECTION
    # Use a temporary file for sed operations to avoid issues with complex replacements
    cp "$(dirname "$0")/templates/docker-compose.yml.template" "$COMPOSE_FILE"
    
    # Use a robust way to replace placeholders including multi-line ones
    # We'll use a temporary script to perform the replacement to avoid escaping hell
    # Use python3 or python depending on what's available
    PYTHON_CMD="python3"
    if ! command -v python3 &> /dev/null; then
        PYTHON_CMD="python"
    fi
    
    # Use a temporary directory for the python script to avoid cluttering the current directory
    TEMP_DIR=$(mktemp -d)
    REPLACE_PY="$TEMP_DIR/replace_placeholders.py"

    # We use a literal EOF to prevent shell variable expansion inside the python script
    # except for the ones we explicitly want to pass in.
    # Actually, it's easier to pass them as environment variables.
    export METRICS_IMAGE_ESC="$METRICS_IMAGE"
    export METRICS_BUILD_SECTION_ESC="$METRICS_BUILD_SECTION"
    export COMPOSE_FILE_ESC="$COMPOSE_FILE"

    cat <<'EOF_PY' > "$REPLACE_PY"
import os
import sys

compose_file = os.environ.get("COMPOSE_FILE_ESC")
metrics_image = os.environ.get("METRICS_IMAGE_ESC")
metrics_build = os.environ.get("METRICS_BUILD_SECTION_ESC")

with open(compose_file, "r") as f:
    content = f.read()

content = content.replace("${METRICS_IMAGE:-wasix-eth-metrics:latest}", metrics_image)
# Handle the literal \n in METRICS_BUILD_SECTION if it came from the shell
content = content.replace("${METRICS_BUILD_SECTION:-# No build section}", metrics_build.replace("\\n", "\n"))

with open(compose_file, "w") as f:
    f.write(content)
EOF_PY
    $PYTHON_CMD "$REPLACE_PY"
    rm -rf "$TEMP_DIR"

    # Clear trailing whitespace or artifacts from replacements
    sed -i 's/[[:space:]]*$//' "$COMPOSE_FILE"

# Track assigned types
node_types=()
for i in $(seq 1 $c_linux); do node_types+=("linux"); done
for i in $(seq 1 $c_wasix); do node_types+=("wasix"); done

for i in $(seq 0 $((NODES - 1))); do
    TYPE=${node_types[$i]}
    IS_VAL=$(is_validator "$i" "$NODES")
    
    ETH_PORT=$(get_eth_rpc_port "$i")
    AUTH_PORT=$(get_auth_rpc_port "$i")
    DISC_PORT=$(get_discovery_port "$i")
    P2P_PORT=$(get_p2p_port "$i")
    METRICS_PORT=$(get_metrics_port "$i")
    BN_RPC_PORT=$(get_beacon_rpc_port "$i")
    BN_P2P_PORT=$(get_beacon_p2p_port "$i")
    BN_QUIC_PORT=$((BN_P2P_PORT + 1))
    
    PEER_NAME="${TYPE}-node-${i}"
    DATA_DIR="./data_v2/node-${i}"
    mkdir -p "$DATA_DIR/el" "$DATA_DIR/cl"

    # Append Execution Client
    FLAGS="--data-dir /app/data --genesis-path /app/startup/genesis.json --verbose 2 --peer-name $PEER_NAME --eth-rpc-port $ETH_PORT --auth-rpc-port $AUTH_PORT --p2p-port $P2P_PORT --discovery-port $DISC_PORT --metrics-port $METRICS_PORT --auth-rpc-jwt-path /app/startup/jwt_$i.hex --bootstrap-registry http://metrics-server:9100 --ext-ip $EXT_IP"
    
    if [ "$TYPE" = "wasix" ]; then
        # Wasix nodes need init before run. Start node-exporter in background.
        ENTRYPOINT_STR="entrypoint: [\"sh\", \"-c\", \"prometheus-node-exporter --web.listen-address=:9100 & sleep 2 && wasmer run /app/wasix_eth.wasm --enable-async-threads --net --volume /app/data:/app/data --volume /app/startup:/app/startup -- init $FLAGS && wasmer run /app/wasix_eth.wasm --enable-async-threads --net --volume /app/data:/app/data --volume /app/startup:/app/startup -- run $FLAGS\"]"
        COMMAND_STR="# Command is handled by entrypoint for wasix"
    else
        # Linux nodes need init before run. Start node-exporter in background.
        ENTRYPOINT_STR="entrypoint: [\"sh\", \"-c\", \"prometheus-node-exporter --web.listen-address=:9100 & sleep 2 && /app/wasix_eth init $FLAGS && /app/wasix_eth run $FLAGS\"]"
        COMMAND_STR="# Command is handled by entrypoint for linux"
    fi

    DEPENDS_ON="depends_on:
      metrics-server:
        condition: service_started"

    if [ -n "$REGISTRY" ]; then
        IMAGE_STR="image: ${REGISTRY}/wasix-eth-${TYPE}:${TAG}"
        BUILD_STR="# Using registry image"
    else
        IMAGE_STR="image: wasix-eth-${TYPE}:latest"
        BUILD_STR="build:
      context: ./wasix_eth
      dockerfile: Dockerfile.$TYPE"
    fi

    cat <<EOF >> "$COMPOSE_FILE"

  el-node-$i:
    $IMAGE_STR
    $BUILD_STR
    volumes:
      - $DATA_DIR/el:/app/data
      - ./startup_v2:/app/startup
    ports:
      - "$ETH_PORT:$ETH_PORT"
      - "$AUTH_PORT:$AUTH_PORT"
      - "$P2P_PORT:$P2P_PORT"
      - "$DISC_PORT:$DISC_PORT/udp"
      - "$METRICS_PORT:$METRICS_PORT"
    networks:
      - blockchain-net
    $ENTRYPOINT_STR
    $COMMAND_STR
    $DEPENDS_ON

EOF

    # Append Consensus Client (Lighthouse Beacon Node)
    BOOT_NODES_VAR="BOOT_NODES_CONFIG_$i"
    BOOT_NODES_FLAG="\${$BOOT_NODES_VAR:-}"

    cat <<EOF >> "$COMPOSE_FILE"
  cl-node-$i:
    image: sigp/lighthouse
    volumes:
      - $DATA_DIR/cl:/root/.lighthouse
      - ./startup_v2:/app/startup
    ports:
      - "$BN_RPC_PORT:5052"
      - "$BN_P2P_PORT:9000"
      - "$BN_P2P_PORT:9000/udp"
      - "$BN_QUIC_PORT:9001/udp"
    networks:
      - blockchain-net
    depends_on:
      - el-node-$i
    command: >
      lighthouse bn
      --execution-endpoint http://el-node-$i:$AUTH_PORT
      --execution-jwt /app/startup/jwt_$i.hex
      --http
      --http-address 0.0.0.0
      --testnet-dir /app/startup
      --debug-level info
      --datadir /root/.lighthouse
      --enr-address $EXT_IP
      --enr-udp-port $BN_P2P_PORT
      --enr-tcp-port $BN_P2P_PORT
      --enr-quic-port $BN_QUIC_PORT
      --listen-address 0.0.0.0
      --port 9000
      --quic-port 9001
      --enable-private-discovery
      $BOOT_NODES_FLAG

EOF

    # Append Validator Client if applicable
    if [ "$IS_VAL" = "true" ]; then
        cat <<EOF >> "$COMPOSE_FILE"
  vc-node-$i:
    image: sigp/lighthouse
    volumes:
      - $DATA_DIR/cl:/root/.lighthouse
      - ./startup_v2:/app/startup
    networks:
      - blockchain-net
    depends_on:
      - cl-node-$i
    entrypoint:
      - sh
      - -c
      - |
        lighthouse --testnet-dir /app/startup account validator import --directory /app/startup/validators/node_$i --password-file /app/startup/validators/node_$i/password.txt --datadir /root/.lighthouse --reuse-password &&
        lighthouse vc --beacon-nodes http://cl-node-$i:5052 --testnet-dir /app/startup --datadir /root/.lighthouse --debug-level info --suggested-fee-recipient 0x0000000000000000000000000000000000000000 --init-slashing-protection
EOF
    fi
done

cat <<EOF >> "$COMPOSE_FILE"

volumes:
  prometheus-data:
  grafana-data:
EOF

echo "Generated $COMPOSE_FILE"
COMPOSE_CMD=$(get_compose_cmd)

# Start metrics server and monitoring first
echo "Starting metrics-server, prometheus, grafana, and node-exporter..."
$COMPOSE_CMD -f "$COMPOSE_FILE" up -d metrics-server prometheus grafana node-exporter

CUMULATIVE_ENRS=""

for i in $(seq 0 $((NODES - 1))); do
    IS_VAL=$(is_validator "$i" "$NODES")
    PEER_SERVICES="el-node-$i cl-node-$i"
    if [ "$IS_VAL" = "true" ]; then
        PEER_SERVICES="$PEER_SERVICES vc-node-$i"
    fi

    echo "Starting node $i ($PEER_SERVICES)..."
    
    # Set the boot nodes for THIS node based on previous nodes
    if [ -n "$CUMULATIVE_ENRS" ]; then
        export "BOOT_NODES_CONFIG_$i=--boot-nodes $CUMULATIVE_ENRS"
    fi

    $COMPOSE_CMD -f "$COMPOSE_FILE" up -d $PEER_SERVICES

    # Wait for the ENR of the newly started node
    BN_RPC_PORT=$(get_beacon_rpc_port $i)
    echo "Waiting for cl-node-$i to start on port $BN_RPC_PORT to extract ENR..."

    MAX_RETRIES=30
    RETRY_COUNT=0
    CURRENT_ENR=""

    while [ $RETRY_COUNT -lt $MAX_RETRIES ]; do
        # Use a subshell and || true to prevent set -e from exiting the script if curl or python fails
        CURRENT_ENR=$(curl -s http://localhost:$BN_RPC_PORT/eth/v1/node/identity 2>/dev/null | python3 -c "import sys, json; print(json.load(sys.stdin)['data']['enr'])" 2>/dev/null || echo "")
        if [ -n "$CURRENT_ENR" ]; then
            echo "Extracted ENR from node $i: $CURRENT_ENR"
            if [ -z "$CUMULATIVE_ENRS" ]; then
                CUMULATIVE_ENRS="$CURRENT_ENR"
            else
                CUMULATIVE_ENRS="$CUMULATIVE_ENRS,$CURRENT_ENR"
            fi
            break
        fi
        echo "Waiting for ENR... ($((RETRY_COUNT + 1))/$MAX_RETRIES)"
        sleep 2
        RETRY_COUNT=$((RETRY_COUNT + 1))
    done

    if [ -z "$CURRENT_ENR" ]; then
        echo "Warning: Failed to extract ENR from cl-node-$i within timeout. Continuing anyway..."
    fi
done

echo "Network started. Use '$COMPOSE_CMD -f $COMPOSE_FILE logs -f' to see logs."

# --- Log Management Section ---
# This section provides information about log locations and offers a CLI to manage background log dumping.

echo ""
echo "--- Log Information ---"
echo "Client node logs are stored in containers and can be viewed using: $COMPOSE_CMD -f $COMPOSE_FILE logs"
echo "Contender logs are typically output to stdout during execution of run_workload.sh."
echo "If run_workload.sh was used, you can also check the container logs: docker logs <contender_container_id>"
echo "Contender state/database is saved in: $(pwd)/.contender_state"

echo ""
echo "To start continuous background log dumping (splitting every 1000 lines):"
echo "  $(dirname "$0")/manage_logs.sh start --chain-id $CHAIN_ID"
echo "To stop it:"
echo "  $(dirname "$0")/manage_logs.sh stop"
echo "Logs will be saved in ./logs_dump"
echo "-----------------------"
