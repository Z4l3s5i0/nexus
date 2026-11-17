#!/bin/bash

# Startup script to initialize and boot a wasix_eth instance.

# Immediately abort the script on any error encountered
set -e

binary=/app/wasix_eth
FLAGS="--verbose 1"

# Bootnodes
if [ "$HIVE_BOOTNODE" != "" ]; then
    FLAGS="$FLAGS --bootnodes $HIVE_BOOTNODE"
fi

# Configure the chain.
echo "Configuring chain with jq..."
mv /genesis.json /genesis-input.json
# Using -c to keep output on one line and avoid any weirdness with large outputs in logs
if jq -e -f /mapper.jq /genesis-input.json > /genesis.json; then
    echo "jq success"
else
    echo "jq failed or unsupported fork requested"
    if [ "$HIVE_PRAGUE_TIMESTAMP" != "" ]; then
        echo "ERROR: Prague fork is not supported by wasix-eth"
        exit 1
    fi
    cat /genesis-input.json > /genesis.json
fi

# Dump genesis.
if [ "$HIVE_LOGLEVEL" != "" ] && [ "$HIVE_LOGLEVEL" -lt 4 ]; then
    echo "Supplied genesis state (trimmed, use --sim.loglevel 4 or 5 for full output):"
    jq 'del(.alloc[] | select(.balance == "0x123450000000000000000"))' /genesis.json
else
    echo "Supplied genesis state:"
    cat /genesis.json
fi

# Genesis path
FLAGS="$FLAGS --genesis-path /genesis.json"

# Network ID
if [ "$HIVE_NETWORK_ID" != "" ]; then
    FLAGS="$FLAGS --chain $HIVE_NETWORK_ID"
fi

# Hive block import
# These flags trigger block import during node startup.
# We do this in a single process run to keep it simple, 
# as the wasix-eth implementation handles imports before starting RPC.
IMPORT_FLAGS=""
if [ -f /chain.rlp ]; then
    echo "Found /chain.rlp, adding --import-chain flag"
    IMPORT_FLAGS="$IMPORT_FLAGS --import-chain /chain.rlp"
fi
if [ -d /blocks ]; then
    echo "Found /blocks directory, adding --import-blocks flag"
    IMPORT_FLAGS="$IMPORT_FLAGS --import-blocks /blocks"
fi

# RPC Ports
FLAGS="$FLAGS --eth-rpc-port 8545"
FLAGS="$FLAGS --auth-rpc-port 8551"

# RPC Ports
FLAGS="$FLAGS --discovery-port 30303"
FLAGS="$FLAGS --p2p-port 30304"

# JWT Secret
if [ "$HIVE_TERMINAL_TOTAL_DIFFICULTY" != "" ]; then
    echo "0x7365637265747365637265747365637265747365637265747365637265747365" > /jwtsecret
    FLAGS="$FLAGS --auth-rpc-jwt-path /jwtsecret"
fi

## Dev mode (mining)
#if [ "$HIVE_MINER" != "" ]; then
#    PERIOD=${HIVE_CLIQUE_PERIOD:-12}
#    FLAGS="$FLAGS --dev $PERIOD"
#fi

# External IP
ip=$(ip addr show eth0 | grep 'inet ' | awk '{print $2}' | cut -d/ -f1)
FLAGS="$FLAGS --ext-ip $ip"

FLAGS="$FLAGS --data-dir /data"

# Run the implementation with the requested flags.
#echo "Initializing genesis..."
#$binary init $FLAGS $IMPORT_FLAGS
if [ "$IMPORT_FLAGS" != "" ]; then
    echo "Importing blocks..."
    $binary import $FLAGS $IMPORT_FLAGS
fi

echo "Running wasix-eth with flags $FLAGS"
$binary run $FLAGS
