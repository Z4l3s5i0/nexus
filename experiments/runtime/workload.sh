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
WORKLOAD_WINDOW_FILE=""
FOUNDRY_IMAGE="ghcr.io/foundry-rs/foundry:v1.7.1"

usage() {
    echo "Usage: bash workload.sh --node-index N --chain-id ID --network NAME --run-id ID --window-file FILE [options]" >&2
    exit 2
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --node-index) NODE_INDEX="$2"; shift 2 ;;
        --chain-id) CHAIN_ID="$2"; shift 2 ;;
        --network) NETWORK="$2"; shift 2 ;;
        --run-id) RUN_ID="$2"; shift 2 ;;
        --window-file) WORKLOAD_WINDOW_FILE="$2"; shift 2 ;;
        --scenario) SCENARIO="$2"; shift 2 ;;
        --tps) TPS="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --accounts) ACCOUNTS="$2"; shift 2 ;;
        *) usage ;;
    esac
done

[[ "$NODE_INDEX" =~ ^[0-9]+$ && "$CHAIN_ID" =~ ^[1-9][0-9]*$ ]] || usage
[[ "$TPS" =~ ^[1-9][0-9]*$ && "$DURATION" =~ ^[1-9][0-9]*$ && "$ACCOUNTS" =~ ^[1-9][0-9]*$ ]] || usage
[[ -n "$NETWORK" && -n "$RUN_ID" && -n "$WORKLOAD_WINDOW_FILE" ]] || usage

MNEMONIC="sleep moment list remain like wall lake industry canvas wonder ecology elite duck salad naive syrup frame brass utility club odor country obey pudding"

PRIVATE_KEY="$(docker run --rm "$FOUNDRY_IMAGE" "cast wallet private-key \"$MNEMONIC\" $NODE_INDEX" | sed -n 's/.*0x\([0-9a-fA-F]\{64\}\).*/\1/p')"
if [[ ! "$PRIVATE_KEY" =~ ^[0-9a-fA-F]{64}$ ]]; then
    echo "Error: could not derive the development key for node $NODE_INDEX." >&2
    exit 1
fi
export EXPERIMENT_WALLET_MNEMONIC="$MNEMONIC"
python3 "$SCRIPT_DIR/cast_workload.py" \
    --node-index "$NODE_INDEX" --chain-id "$CHAIN_ID" --network "$NETWORK" \
    --run-id "$RUN_ID" --window-file "$WORKLOAD_WINDOW_FILE" \
    --scenario "$SCENARIO" --tps "$TPS" --duration "$DURATION" \
    --accounts "$ACCOUNTS" --private-key "$PRIVATE_KEY"