#!/usr/bin/env bash
set -euo pipefail

NODE_INDEX=""
CHAIN_ID=""
SCENARIO="exp/uber"
TPS=4
DURATION=300
ACCOUNTS=4
NETWORK=""
RUN_ID=""
STATE_DIR=""
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
[[ -n "$NETWORK" && -n "$RUN_ID" && -n "$STATE_DIR" ]] || usage

MNEMONIC="sleep moment list remain like wall lake industry canvas wonder ecology elite duck salad naive syrup frame brass utility club odor country obey pudding"
RPC_URL="http://el-node-$NODE_INDEX:$((8545 + NODE_INDEX))"
SCENARIO_REF="/scenarios/$SCENARIO.toml"
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

docker run -i "${CONTAINER_OPTIONS[@]}" "$CONTENDER_IMAGE" setup "$SCENARIO_REF" \
    -r "$RPC_URL" -p "$PRIVATE_KEY" -a "$ACCOUNTS" --seed "$SEED" \
    --env GAS_LIMIT=1eth --min-balance 100eth
docker run -i "${CONTAINER_OPTIONS[@]}" "$CONTENDER_IMAGE" spam "$SCENARIO_REF" \
    -r "$RPC_URL" --tps "$TPS" -d "$DURATION" -p "$PRIVATE_KEY" -a "$ACCOUNTS" \
    --seed "$SEED" --env GAS_LIMIT=1eth --min-balance 100eth --pending-timeout 36