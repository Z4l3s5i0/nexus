#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
NODE_INDEX=""
CHAIN_ID=""
SCENARIO="exp/uber"
TPS=4
DURATION=300
ACCOUNTS=4
NETWORK=""
RUN_ID=""
STATE_DIR=""
WORKLOAD_WINDOW_FILE=""
CONTENDER_IMAGE="docker.io/z4l3s5i0/contender:latest"
FOUNDRY_IMAGE="ghcr.io/foundry-rs/foundry:v1.7.1"

usage() {
    echo "Usage: bash workload.sh --node-index N --chain-id ID --network NAME --run-id ID --state-dir DIR [options]" >&2
    exit 2
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --node-index) NODE_INDEX="$2"; shift 2 ;;
        --chain-id) CHAIN_ID="$2"; shift 2 ;;
        --network) NETWORK="$2"; shift 2 ;;
        --run-id) RUN_ID="$2"; shift 2 ;;
        --state-dir) STATE_DIR="$2"; shift 2 ;;
        --window-file) WORKLOAD_WINDOW_FILE="$2"; shift 2 ;;
        --scenario) SCENARIO="$2"; shift 2 ;;
        --tps) TPS="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --accounts) ACCOUNTS="$2"; shift 2 ;;
        --contender-image) CONTENDER_IMAGE="$2"; shift 2 ;;
        *) usage ;;
    esac
done

[[ "$NODE_INDEX" =~ ^[0-9]+$ && "$CHAIN_ID" =~ ^[1-9][0-9]*$ ]] || usage
[[ "$TPS" =~ ^[1-9][0-9]*$ && "$DURATION" =~ ^[1-9][0-9]*$ && "$ACCOUNTS" =~ ^[1-9][0-9]*$ ]] || usage
[[ -n "$NETWORK" && -n "$RUN_ID" && -n "$STATE_DIR" && -n "$WORKLOAD_WINDOW_FILE" ]] || usage

MNEMONIC="sleep moment list remain like wall lake industry canvas wonder ecology elite duck salad naive syrup frame brass utility club odor country obey pudding"
RPC_URL="http://el-node-$NODE_INDEX:$((8545 + NODE_INDEX))"
SCENARIO_REF="/scenarios/$SCENARIO.toml"
SCENARIO_VOLUME=""
if [[ "$SCENARIO" == "exp/value-transfer" ]]; then
    SCENARIO_REF="/tmp/wasix-value-transfer.toml"
    SCENARIO_VOLUME="$SCRIPT_DIR/value-transfer.toml:$SCENARIO_REF:ro"
fi
mkdir -p "$STATE_DIR"
STATE_DIR="$(cd "$STATE_DIR" && pwd)"

PRIVATE_KEY="$(docker run --rm "$FOUNDRY_IMAGE" "cast wallet private-key \"$MNEMONIC\" $NODE_INDEX" | sed -n 's/.*0x\([0-9a-fA-F]\{64\}\).*/\1/p')"
if [[ ! "$PRIVATE_KEY" =~ ^[0-9a-fA-F]{64}$ ]]; then
    echo "Error: could not derive the development key for node $NODE_INDEX." >&2
    exit 1
fi
SEED="0x$(printf 'node%s' "$NODE_INDEX" | sha256sum | cut -d' ' -f1)"
CONTAINER_OPTIONS=(
    --rm
    --label "wasix-evm.experiment=$RUN_ID"
    --network "$NETWORK"
    -v "$STATE_DIR:/root/.local/state/contender"
    -w /
)
if [[ -n "$SCENARIO_VOLUME" ]]; then
    CONTAINER_OPTIONS+=( -v "$SCENARIO_VOLUME" )
fi

docker run -i "${CONTAINER_OPTIONS[@]}" "$CONTENDER_IMAGE" setup "$SCENARIO_REF" \
    -r "$RPC_URL" -p "$PRIVATE_KEY" -a "$ACCOUNTS" --seed "$SEED" \
    --env GAS_LIMIT=1eth --min-balance 100eth
WORKLOAD_START_MS="$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')"
SPAM_STATUS=0
if docker run -i "${CONTAINER_OPTIONS[@]}" "$CONTENDER_IMAGE" spam "$SCENARIO_REF" \
    -r "$RPC_URL" --tps "$TPS" -d "$DURATION" -p "$PRIVATE_KEY" -a "$ACCOUNTS" \
    --seed "$SEED" --env GAS_LIMIT=1eth --min-balance 100eth --pending-timeout 36; then
    :
else
    SPAM_STATUS=$?
fi
WORKLOAD_END_MS="$(python3 -c 'import time; print(time.time_ns() // 1_000_000)')"
python3 - "$WORKLOAD_WINDOW_FILE" "$RUN_ID" "$NODE_INDEX" "$WORKLOAD_START_MS" "$WORKLOAD_END_MS" "$SPAM_STATUS" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
path.parent.mkdir(parents=True, exist_ok=True)
window = {
    "experiment_id": sys.argv[2],
    "node_index": int(sys.argv[3]),
    "start_ms": int(sys.argv[4]),
    "end_ms": int(sys.argv[5]),
    "exit_code": int(sys.argv[6]),
}
path.write_text(json.dumps(window, sort_keys=True) + "\n", encoding="utf-8")
PY
exit "$SPAM_STATUS"