#!/bin/bash
set -e

# Local Docker Network Orchestrator for v2 Deployment

SCRIPT_DIR="${EXPERIMENTS_NETWORK_SUPPORT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)}"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
COMPOSE_FILE="$REPO_ROOT/docker-compose.v2.yml"
cd "$REPO_ROOT"
source <(tr -d '\r' < "$SCRIPT_DIR/common.sh")

LINUX_NODES=2
WASIX_NODES=2
GETH_NODES=0
RETH_NODES=0
CHAIN_ID=12345
CLEANUP=false
REGISTRY=""
TAG="latest"
GETH_IMAGE="ethereum/client-go@sha256:798b7eb1bcef6d4be7576232beea63bf291450f48b025dc8fb5c6e37840e4364"
RETH_IMAGE="ghcr.io/paradigmxyz/reth:latest"
METRICS_IMAGE_OVERRIDE=""

usage() {
    echo "Usage: $0 [options]"
    echo "Options:"
    echo "  --linux-nodes N   Number of Linux nodes (default: 2)"
    echo "  --wasix-nodes N   Number of Wasix nodes (default: 2)"
    echo "  --geth-nodes N    Number of Geth nodes (default: 0)"
    echo "  --reth-nodes N    Number of Reth nodes (default: 0)"
    echo "  --chain-id ID     Unique chain ID (default: 12345)"
    echo "  --cleanup         Remove existing data before starting"
    echo "  --registry URL    Docker registry to pull images from (optional)"
    echo "  --tag TAG         Image tag to use (default: latest)"
    echo "  --metrics-image IMAGE  Use an existing metrics image instead of building locally"
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi

while [[ "$#" -gt 0 ]]; do
    case $1 in
        --linux-nodes) LINUX_NODES="$2"; shift ;;
        --wasix-nodes) WASIX_NODES="$2"; shift ;;
        --geth-nodes) GETH_NODES="$2"; shift ;;
        --reth-nodes) RETH_NODES="$2"; shift ;;
        --chain-id) CHAIN_ID="$2"; shift ;;
        --cleanup) CLEANUP=true ;;
        --registry) REGISTRY="$2"; shift ;;
        --tag) TAG="$2"; shift ;;
        --metrics-image) METRICS_IMAGE_OVERRIDE="$2"; shift ;;
        *) usage >&2; exit 2 ;;
    esac
    shift
done

for count_name in LINUX_NODES WASIX_NODES GETH_NODES RETH_NODES; do
    count_value=${!count_name}
    if ! [[ "$count_value" =~ ^[0-9]+$ ]]; then
        echo "Error: $count_name must be a nonnegative integer (got '$count_value')." >&2
        exit 1
    fi
    printf -v "$count_name" '%d' "$((10#$count_value))"
done

if ! [[ "$CHAIN_ID" =~ ^[0-9]+$ ]] || [ "$CHAIN_ID" -eq 0 ]; then
    echo "Error: --chain-id must be a positive integer (got '$CHAIN_ID')." >&2
    exit 1
fi
CHAIN_ID=$((10#$CHAIN_ID))

NODES=$((LINUX_NODES + GETH_NODES + RETH_NODES + WASIX_NODES))
if [ "$NODES" -le 0 ]; then
    echo "Error: at least one execution node is required." >&2
    exit 1
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
mkdir -p data_v2/metrics-server

# 1. Generate Configuration
echo "Generating network configuration..."
EXPERIMENTS_NETWORK_SUPPORT_DIR="$SCRIPT_DIR" bash <(tr -d '\r' < "$SCRIPT_DIR/config_gen.sh") \
    --chain-id "$CHAIN_ID" --nodes "$NODES" --output "$REPO_ROOT/startup_v2"

# Copy monitoring configuration
echo "Setting up monitoring configuration..."
cp "$SCRIPT_DIR/monitoring/prometheus.yml" "startup_v2/prometheus.yml"
chmod 644 "startup_v2/prometheus.yml"
cp -r "$SCRIPT_DIR/monitoring/grafana/provisioning/"* "startup_v2/grafana/provisioning/"
chmod -R 755 "startup_v2/grafana/provisioning"

# Copy local monitoring dashboards if they exist
if [ -d "$SCRIPT_DIR/monitoring/grafana/dashboards" ]; then
    cp "$SCRIPT_DIR/monitoring/grafana/dashboards"/*.json "startup_v2/grafana/dashboards/" 2>/dev/null || true
fi

# Try to find dashboards in multiple possible locations
DASHBOARD_SOURCES=(
    "$REPO_ROOT/wasix_eth/grafana/dashboards"
    "$REPO_ROOT/grafana/dashboards"
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
EXT_IP=$(ip -4 route get 1.1.1.1 2>/dev/null | grep -oP 'src \K\S+' || true)
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

# 3. Node counts and deterministic type assignment
c_linux=$LINUX_NODES
c_geth=$GETH_NODES
c_reth=$RETH_NODES
c_wasix=$WASIX_NODES
echo "Node counts: Linux=$c_linux, Geth=$c_geth, Reth=$c_reth, Wasix=$c_wasix"

# 3. Generate docker-compose.yml
# Prepare variables for template replacement
if [ -n "$METRICS_IMAGE_OVERRIDE" ]; then
    METRICS_IMAGE="$METRICS_IMAGE_OVERRIDE"
    METRICS_BUILD_SECTION="# Explicit metrics image used"
else
    METRICS_IMAGE="wasix-eth-metrics:latest"
    METRICS_BUILD_SECTION="build:\n      context: $REPO_ROOT/metrics_server\n      dockerfile: Dockerfile"
fi

    # Use sed to replace placeholders
    # We use a literal newline replacement for METRICS_BUILD_SECTION
    # Use a temporary file for sed operations to avoid issues with complex replacements
    cp "$SCRIPT_DIR/templates/docker-compose.yml.template" "$COMPOSE_FILE"
    
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
for ((i = 0; i < c_linux; i++)); do node_types+=("linux"); done
for ((i = 0; i < c_geth; i++)); do node_types+=("geth"); done
for ((i = 0; i < c_reth; i++)); do node_types+=("reth"); done
for ((i = 0; i < c_wasix; i++)); do node_types+=("wasix"); done

"$PYTHON_CMD" "$SCRIPT_DIR/generate_prometheus_targets.py" \
    --node-exporter-output "$REPO_ROOT/startup_v2/node-exporter-targets.json" \
    --execution-metrics-output "$REPO_ROOT/startup_v2/execution-node-targets.json" \
    --node-types "${node_types[@]}"

for i in $(seq 0 $((NODES - 1))); do
    TYPE=${node_types[$i]}
    IS_VAL=$(is_validator "$i" "$NODES")
    NODE_CLIENT_LABEL="$TYPE"
    NODE_RUNTIME_VARIANT="$TYPE"
    if [ "$TYPE" = "linux" ] || [ "$TYPE" = "wasix" ]; then
        NODE_CLIENT_LABEL="wasix-eth"
        [ "$TYPE" = "linux" ] && NODE_RUNTIME_VARIANT="native"
    fi
    
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
    
    if [ "$TYPE" = "geth" ]; then
        IMAGE_STR="image: $GETH_IMAGE"
        BUILD_STR="# Pinned upstream Geth image"
        ENTRYPOINT_STR="# Geth uses the pinned image entrypoint"
        COMMAND_STR="command:
      - --datadir=/app/data
      - --networkid=$CHAIN_ID
      - --syncmode=full
      - --http
      - --http.addr=0.0.0.0
      - --http.port=$ETH_PORT
      - --http.api=eth,net,web3
      - '--http.vhosts=*'
      - --authrpc.addr=0.0.0.0
      - --authrpc.port=$AUTH_PORT
      - '--authrpc.vhosts=*'
      - --authrpc.jwtsecret=/app/startup/jwt_$i.hex
      - --port=$P2P_PORT
      - --discovery.port=$DISC_PORT
      - --nat=extip:$EXT_IP
      - --maxpeers=50
      - --metrics
      - --metrics.addr=0.0.0.0
      - --metrics.port=$METRICS_PORT"
        PORTS_STR="ports:
      - \"$ETH_PORT:$ETH_PORT\"
      - \"$P2P_PORT:$P2P_PORT/tcp\"
      - \"$DISC_PORT:$DISC_PORT/udp\""
        DEPENDS_ON="depends_on:
      metrics-server:
        condition: service_started"
    elif [ "$TYPE" = "reth" ]; then
        IMAGE_STR="image: $RETH_IMAGE"
        BUILD_STR="# Pinned upstream Reth image"
        ENTRYPOINT_STR="# Reth uses the pinned image entrypoint"
        COMMAND_STR="command:
      - node
      - --datadir=/app/data
      - --chain=/app/startup/genesis.json
      - --http
      - --http.addr=0.0.0.0
      - --http.port=$ETH_PORT
      - --authrpc.addr=0.0.0.0
      - --authrpc.port=$AUTH_PORT
      - --authrpc.jwtsecret=/app/startup/jwt_$i.hex
      - --port=$P2P_PORT
      - --discovery.addr=0.0.0.0
      - --discovery.port=$DISC_PORT
      - --nat=extip:$EXT_IP
      - --max-outbound-peers=50
      - --max-inbound-peers=50
      - --metrics=$METRICS_PORT"
        PORTS_STR="ports:
      - \"$ETH_PORT:$ETH_PORT\"
      - \"$P2P_PORT:$P2P_PORT/tcp\"
      - \"$DISC_PORT:$DISC_PORT/udp\"
      - \"$METRICS_PORT:$METRICS_PORT\""
        DEPENDS_ON="depends_on:
      metrics-server:
        condition: service_started"
    elif [ "$TYPE" = "wasix" ]; then
        # Wasix nodes need init before run. Start node-exporter in background.
        ENTRYPOINT_STR="entrypoint: [\"sh\", \"-c\", \"prometheus-node-exporter --no-collector.systemd --web.listen-address=:9100 & sleep 2 && wasmer run /app/wasix_eth.wasm --enable-async-threads --net --volume /app/data:/app/data --volume /app/startup:/app/startup -- init $FLAGS && wasmer run /app/wasix_eth.wasm --enable-async-threads --net --volume /app/data:/app/data --volume /app/startup:/app/startup -- run $FLAGS\"]"
        COMMAND_STR="# Command is handled by entrypoint for wasix"
    else
        # Linux nodes need init before run. Start node-exporter in background.
        ENTRYPOINT_STR="entrypoint: [\"sh\", \"-c\", \"prometheus-node-exporter --no-collector.systemd --web.listen-address=:9100 & sleep 2 && /app/wasix_eth init $FLAGS && /app/wasix_eth run $FLAGS\"]"
        COMMAND_STR="# Command is handled by entrypoint for linux"
    fi

    if [ "$TYPE" != "geth" ] && [ "$TYPE" != "reth" ]; then
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
        PORTS_STR="ports:
      - \"$ETH_PORT:$ETH_PORT\"
      - \"$AUTH_PORT:$AUTH_PORT\"
      - \"$P2P_PORT:$P2P_PORT\"
      - \"$DISC_PORT:$DISC_PORT/udp\"
      - \"$METRICS_PORT:$METRICS_PORT\""
    fi

    cat <<EOF >> "$COMPOSE_FILE"

  el-node-$i:
    $IMAGE_STR
    $BUILD_STR
    labels:
      experiment_node_index: "$i"
      experiment_client: "$NODE_CLIENT_LABEL"
      experiment_runtime_variant: "$NODE_RUNTIME_VARIANT"
      experiment_node_service: "el-node-$i"
    volumes:
      - $DATA_DIR/el:/app/data
      - ./startup_v2:/app/startup
    $PORTS_STR
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
    image: sigp/lighthouse:v8.2.1
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
      --execution-timeout-multiplier 2
      $BOOT_NODES_FLAG

EOF

    # Append Validator Client if applicable
    if [ "$IS_VAL" = "true" ]; then
        cat <<EOF >> "$COMPOSE_FILE"
  vc-node-$i:
    image: sigp/lighthouse:v8.2.1
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

geth_attach() {
    local service=$1
    local expression=$2
    $COMPOSE_CMD -f "$COMPOSE_FILE" exec -T "$service" geth attach --exec "$expression" ipc:/app/data/geth.ipc
}

geth_peer_ids() {
    local service=$1
    local peer_output
    peer_output=$(geth_attach "$service" 'admin.peers.map(function(peer) { return peer.enode; }).join(",")' 2>/dev/null || true)
    python3 -c 'import re,sys; print("\n".join(sorted(set(x.lower() for x in re.findall(r"(?:0x)?([0-9a-f]{128})", sys.argv[1], re.I)))))' "$peer_output"
}

geth_active_bootstrap_nodes() {
    local own_id=$1
    local non_external_only=${2:-false}
    local node_data
    node_data=$(curl -fsS --max-time 5 "http://127.0.0.1:9100/api/nodes") || return 1
    python3 -c 'import json,re,sys
nodes=json.loads(sys.argv[1])
chain_id=int(sys.argv[2])
own_id=sys.argv[3].lower()
non_external_only=sys.argv[4] == "true"
for node in nodes:
    enode=node.get("enode") or ""
    match=re.fullmatch(r"enode://([0-9a-fA-F]{128})@([0-9.]+):([0-9]{1,5})(?:\?discport=[0-9]{1,5})?", enode)
    if node.get("status") != "Active" or node.get("network") != str(chain_id) or node.get("chain_id") != chain_id or node.get("id", "").lower() == own_id or (non_external_only and node.get("client") in ("geth", "reth")) or not match:
        continue
    print(match.group(1).lower()+"\t"+enode)' "$node_data" "$CHAIN_ID" "$own_id" "$non_external_only"
}

register_geth_node() {
    local index=$1
    local eth_port=$2
    local p2p_port=$3
    local discovery_port=$4
    local metrics_port=$5
    local service="el-node-$index"
    local expected_chain_id

    echo "Waiting for Geth RPC and verifying chain ID on node $index..."
    for attempt in $(seq 1 60); do
        local rpc_response
        rpc_response=$(curl -fsS --max-time 3 -H 'Content-Type: application/json' \
            --data-binary '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}' \
            "http://127.0.0.1:$eth_port" 2>/dev/null || true)
        expected_chain_id=$(python3 -c 'import json,sys; print(int(json.loads(sys.argv[1])["result"],16))' "$rpc_response" 2>/dev/null || true)
        if [ "$expected_chain_id" = "$CHAIN_ID" ]; then
            break
        fi
        if [ "$attempt" -eq 60 ]; then
            echo "Error: Geth node $index did not expose expected chain ID $CHAIN_ID at RPC port $eth_port." >&2
            $COMPOSE_CMD -f "$COMPOSE_FILE" logs --tail=100 "$service" >&2 || true
            return 1
        fi
        sleep 2
    done

    local raw_enode
    raw_enode=$(geth_attach "$service" 'admin.nodeInfo.enode' 2>/dev/null || true)
    local node_id
    node_id=$(python3 -c 'import re,sys; match=re.search(r"enode://([0-9a-fA-F]{128})@", sys.argv[1]); print(match.group(1).lower() if match else "")' "$raw_enode")
    if [ -z "$node_id" ]; then
        echo "Error: Could not obtain a valid Geth node identity from $service over its private IPC socket." >&2
        $COMPOSE_CMD -f "$COMPOSE_FILE" logs --tail=100 "$service" >&2 || true
        return 1
    fi

    local enode="enode://$node_id@$EXT_IP:$p2p_port?discport=$discovery_port"
    local register_body
    register_body=$(python3 - "$node_id" "$CHAIN_ID" "$index" "$EXT_IP" "$eth_port" "$metrics_port" "$p2p_port" "$discovery_port" "$enode" <<'PY'
import json
import sys

node_id, chain_id, index, external_ip, eth_port, metrics_port, p2p_port, discovery_port, enode = sys.argv[1:]
chain_id = int(chain_id)
print(json.dumps({
    "id": node_id,
    "network": str(chain_id),
    "chain_id": chain_id,
    "client": "geth",
    "rpc_url": f"http://el-node-{index}:{eth_port}",
    "metrics_url": f"http://el-node-{index}:{metrics_port}/debug/metrics/prometheus",
    "p2p_addr": f"{external_ip}:{p2p_port}",
    "discovery_addr": f"{external_ip}:{discovery_port}",
    "enode": enode,
}))
PY
    )
    echo "Registering Geth node $index with the metrics-server..."
    if ! curl -fsS --max-time 12 -H 'Content-Type: application/json' \
        --data-binary "$register_body" http://127.0.0.1:9100/api/nodes/register; then
        echo "Error: Geth node $index registration failed." >&2
        return 1
    fi

    local bootstrap_data=""
    if [ "$index" -gt 0 ]; then
        echo "Waiting for active execution bootstrap peers for Geth node $index..."
        for attempt in $(seq 1 30); do
            bootstrap_data=$(geth_active_bootstrap_nodes "$node_id" || true)
            if [ -n "$bootstrap_data" ]; then
                break
            fi
            sleep 2
        done
        if [ -z "$bootstrap_data" ]; then
            echo "Error: No active execution bootstrap peers were available for Geth node $index." >&2
            $COMPOSE_CMD -f "$COMPOSE_FILE" logs --tail=100 "$service" >&2 || true
            return 1
        fi
    fi

    local expected_peer_ids=""
    if [ -n "$bootstrap_data" ]; then
        while IFS=$'\t' read -r peer_id peer_enode; do
            [ -n "$peer_id" ] || continue
            expected_peer_ids+="$peer_id"$'\n'
            echo "Adding active execution peer $peer_id to Geth node $index..."
            if ! geth_attach "$service" "admin.addPeer(\"$peer_enode\")" >/dev/null; then
                echo "Warning: Geth node $index failed to dial peer $peer_id immediately; checking peer state." >&2
            fi
        done <<< "$bootstrap_data"

        local connected_peer_id=""
        for attempt in $(seq 1 30); do
            local connected_ids
            connected_ids=$(geth_peer_ids "$service")
            connected_peer_id=$(python3 -c 'import sys; expected=set(sys.argv[1].split()); connected=set(sys.argv[2].split()); print(next(iter(expected & connected), ""))' "$expected_peer_ids" "$connected_ids")
            if [ -n "$connected_peer_id" ]; then
                break
            fi
            sleep 2
        done
        if [ -z "$connected_peer_id" ]; then
            echo "Error: Geth node $index registered but has no verified execution connection to its active bootstrap peers." >&2
            $COMPOSE_CMD -f "$COMPOSE_FILE" logs --tail=100 "$service" >&2 || true
            return 1
        fi
        echo "Verified Geth execution peer $connected_peer_id on node $index."
    fi
}

register_reth_node() {
    local index=$1
    local eth_port=$2
    local p2p_port=$3
    local discovery_port=$4
    local metrics_port=$5
    local service="el-node-$index"
    local expected_chain_id

    echo "Waiting for Reth RPC and verifying chain ID on node $index..."
    for attempt in $(seq 1 60); do
        local rpc_response
        rpc_response=$(curl -fsS --max-time 3 -H 'Content-Type: application/json' \
            --data-binary '{"jsonrpc":"2.0","method":"eth_chainId","params":[],"id":1}' \
            "http://127.0.0.1:$eth_port" 2>/dev/null || true)
        expected_chain_id=$(python3 -c 'import json,sys; print(int(json.loads(sys.argv[1])["result"],16))' "$rpc_response" 2>/dev/null || true)
        if [ "$expected_chain_id" = "$CHAIN_ID" ]; then
            break
        fi
        if [ "$attempt" -eq 60 ]; then
            echo "Error: Reth node $index did not expose expected chain ID $CHAIN_ID at RPC port $eth_port." >&2
            $COMPOSE_CMD -f "$COMPOSE_FILE" logs --tail=100 "$service" >&2 || true
            return 1
        fi
        sleep 2
    done

    # Reth exposes its node record via admin_nodeInfo RPC
    local node_info
    node_info=$(curl -fsS --max-time 5 -H 'Content-Type: application/json' \
        --data-binary '{"jsonrpc":"2.0","method":"admin_nodeInfo","params":[],"id":1}' \
        "http://127.0.0.1:$eth_port" 2>/dev/null || true)
    local node_id
    local enr
    node_id=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1])["result"]; print(d.get("enr","").split("id=")[1].split(",")[0] if "id=" in d.get("enr","") else "")' "$node_info" 2>/dev/null || true)
    # If we can't extract from ENR, try admin_node record
    if [ -z "$node_id" ]; then
        node_id=$(python3 -c 'import json,sys; d=json.loads(sys.argv[1])["result"]; print(d.get("enode","").split("@")[0].split("//")[1] if "enode" in d else "")' "$node_info" 2>/dev/null || true)
    fi
    if [ -z "$node_id" ]; then
        echo "Error: Could not obtain a valid Reth node identity from $service." >&2
        $COMPOSE_CMD -f "$COMPOSE_FILE" logs --tail=100 "$service" >&2 || true
        return 1
    fi

    local enode="enode://$node_id@$EXT_IP:$p2p_port?discport=$discovery_port"
    local register_body
    register_body=$(python3 - "$node_id" "$CHAIN_ID" "$index" "$EXT_IP" "$eth_port" "$metrics_port" "$p2p_port" "$discovery_port" "$enode" <<'PY'
import json
import sys

node_id, chain_id, index, external_ip, eth_port, metrics_port, p2p_port, discovery_port, enode = sys.argv[1:]
chain_id = int(chain_id)
print(json.dumps({
    "id": node_id,
    "network": str(chain_id),
    "chain_id": chain_id,
    "client": "reth",
    "rpc_url": f"http://el-node-{index}:{eth_port}",
    "metrics_url": f"http://el-node-{index}:{metrics_port}/metrics",
    "p2p_addr": f"{external_ip}:{p2p_port}",
    "discovery_addr": f"{external_ip}:{discovery_port}",
    "enode": enode,
}))
PY
    )
    echo "Registering Reth node $index with the metrics-server..."
    if ! curl -fsS --max-time 12 -H 'Content-Type: application/json' \
        --data-binary "$register_body" http://127.0.0.1:9100/api/nodes/register; then
        echo "Error: Reth node $index registration failed." >&2
        return 1
    fi

    echo "Verified Reth node $index."
}

verify_geth_mixed_peers() {
    local external_nodes=$((GETH_NODES + RETH_NODES))
    local non_external_nodes=$((LINUX_NODES + WASIX_NODES))
    if [ "$external_nodes" -eq 0 ] || [ "$non_external_nodes" -eq 0 ]; then
        return 0
    fi

    local non_external_bootstrap_nodes=""
    for attempt in $(seq 1 30); do
        non_external_bootstrap_nodes=$(geth_active_bootstrap_nodes "" true || true)
        if [ -n "$non_external_bootstrap_nodes" ]; then
            break
        fi
        sleep 2
    done
    if [ -z "$non_external_bootstrap_nodes" ]; then
        echo "Error: No active non-external execution nodes are available to verify the mixed network." >&2
        return 1
    fi

    for ((geth_offset = 0; geth_offset < GETH_NODES; geth_offset++)); do
        local index=$((LINUX_NODES + geth_offset))
        local service="el-node-$index"
        local expected_peer_ids=""
        while IFS=$'\t' read -r peer_id peer_enode; do
            [ -n "$peer_id" ] || continue
            expected_peer_ids+="$peer_id"$'\n'
            geth_attach "$service" "admin.addPeer(\"$peer_enode\")" >/dev/null || true
        done <<< "$non_external_bootstrap_nodes"

        local connected_peer_id=""
        for attempt in $(seq 1 30); do
            local connected_ids
            connected_ids=$(geth_peer_ids "$service")
            connected_peer_id=$(python3 -c 'import sys; expected=set(sys.argv[1].split()); connected=set(sys.argv[2].split()); print(next(iter(expected & connected), ""))' "$expected_peer_ids" "$connected_ids")
            if [ -n "$connected_peer_id" ]; then
                break
            fi
            sleep 2
        done
        if [ -z "$connected_peer_id" ]; then
            echo "Error: Geth node $index has no verified execution connection to a non-Geth client." >&2
            $COMPOSE_CMD -f "$COMPOSE_FILE" logs --tail=100 "$service" >&2 || true
            return 1
        fi
        echo "Verified mixed-client execution peer $connected_peer_id on Geth node $index."
    done

    for ((reth_offset = 0; reth_offset < RETH_NODES; reth_offset++)); do
        local index=$((LINUX_NODES + GETH_NODES + reth_offset))
        local service="el-node-$index"
        local eth_port
        eth_port=$(get_eth_rpc_port "$index")
        echo "Verifying Reth node $index has peers..."
        for attempt in $(seq 1 30); do
            local peer_count
            peer_count=$(curl -fsS --max-time 5 -H 'Content-Type: application/json' \
                --data-binary '{"jsonrpc":"2.0","method":"admin_nodeInfo","params":[],"id":1}' \
                "http://127.0.0.1:$eth_port" 2>/dev/null | \
                python3 -c 'import json,sys; d=json.loads(sys.argv[1])["result"]; print(d.get("peers",0))' 2>/dev/null || echo 0)
            if [ "$peer_count" -gt 0 ] 2>/dev/null; then
                echo "Verified Reth node $index has $peer_count peer(s)."
                break
            fi
            sleep 2
        done
    done
}

# Start metrics server and monitoring first
echo "Starting metrics-server, prometheus, grafana, node-exporter, and cAdvisor..."
if [ -z "$METRICS_IMAGE_OVERRIDE" ]; then
    echo "Building local metrics-server image..."
    $COMPOSE_CMD -f "$COMPOSE_FILE" build metrics-server
fi
$COMPOSE_CMD -f "$COMPOSE_FILE" up -d metrics-server prometheus grafana node-exporter cadvisor

if [ "$GETH_NODES" -gt 0 ] || [ "$RETH_NODES" -gt 0 ]; then
    for attempt in $(seq 1 60); do
        if curl -fsS --max-time 2 http://127.0.0.1:9100/health >/dev/null; then
            break
        fi
        if [ "$attempt" -eq 60 ]; then
            echo "Error: metrics-server did not become ready for Geth/Reth registration." >&2
            $COMPOSE_CMD -f "$COMPOSE_FILE" logs --tail=100 metrics-server >&2 || true
            exit 1
        fi
        sleep 2
    done
fi

CUMULATIVE_ENRS=""

for i in $(seq 0 $((NODES - 1))); do
    TYPE=${node_types[$i]}
    IS_VAL=$(is_validator "$i" "$NODES")
    ETH_PORT=$(get_eth_rpc_port "$i")
    AUTH_PORT=$(get_auth_rpc_port "$i")
    DISC_PORT=$(get_discovery_port "$i")
    P2P_PORT=$(get_p2p_port "$i")
    METRICS_PORT=$(get_metrics_port "$i")
    PEER_SERVICES="el-node-$i cl-node-$i"
    if [ "$IS_VAL" = "true" ]; then
        PEER_SERVICES="$PEER_SERVICES vc-node-$i"
    fi

    echo "Starting node $i ($PEER_SERVICES)..."
    
    # Set the boot nodes for THIS node based on previous nodes
    if [ -n "$CUMULATIVE_ENRS" ]; then
        export "BOOT_NODES_CONFIG_$i=--boot-nodes $CUMULATIVE_ENRS"
    fi

    if [ "$TYPE" = "geth" ]; then
        echo "Initializing Geth node $i from the generated genesis..."
        $COMPOSE_CMD -f "$COMPOSE_FILE" run --rm --no-deps --entrypoint geth "el-node-$i" --datadir=/app/data init /app/startup/genesis.json
        $COMPOSE_CMD -f "$COMPOSE_FILE" up -d "el-node-$i"
        register_geth_node "$i" "$ETH_PORT" "$P2P_PORT" "$DISC_PORT" "$METRICS_PORT"
        CL_SERVICES="cl-node-$i"
        if [ "$IS_VAL" = "true" ]; then
            CL_SERVICES="$CL_SERVICES vc-node-$i"
        fi
        $COMPOSE_CMD -f "$COMPOSE_FILE" up -d $CL_SERVICES
    elif [ "$TYPE" = "reth" ]; then
        echo "Initializing Reth node $i from the generated genesis..."
        $COMPOSE_CMD -f "$COMPOSE_FILE" run --rm --no-deps "el-node-$i" db init --datadir /app/data --chain /app/startup/genesis.json 2>/dev/null || true
        $COMPOSE_CMD -f "$COMPOSE_FILE" up -d "el-node-$i"
        register_reth_node "$i" "$ETH_PORT" "$P2P_PORT" "$DISC_PORT" "$METRICS_PORT"
        CL_SERVICES="cl-node-$i"
        if [ "$IS_VAL" = "true" ]; then
            CL_SERVICES="$CL_SERVICES vc-node-$i"
        fi
        $COMPOSE_CMD -f "$COMPOSE_FILE" up -d $CL_SERVICES
    else
        $COMPOSE_CMD -f "$COMPOSE_FILE" up -d $PEER_SERVICES
    fi

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

verify_geth_mixed_peers

echo "Network started. Use '$COMPOSE_CMD -f $COMPOSE_FILE logs -f' to see logs."
