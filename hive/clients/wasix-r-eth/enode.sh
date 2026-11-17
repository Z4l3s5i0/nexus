#!/bin/bash

# Script to retrieve the enode
# 
# This is copied into the validator container by Hive
# and used to provide a client-specific enode id retriever
#

# Immediately abort the script on any error encountered
set -e

# Retry loop to wait for the node to start
ip=$(ip addr show eth0 | grep 'inet ' | awk '{print $2}' | cut -d/ -f1)
for i in {1..120}; do
    TARGET_RESPONSE=$(curl -s -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"admin_nodeInfo","params":[],"id":1}' "$ip:8545" || true)
    
    if [ -n "$TARGET_RESPONSE" ]; then
        TARGET_ENODE=$(echo "${TARGET_RESPONSE}" | jq -r '.result.enode' 2>/dev/null || true)
        if [ -n "$TARGET_ENODE" ] && [ "$TARGET_ENODE" != "null" ]; then
            echo "$TARGET_ENODE"
            exit 0
        fi
        echo "Response received but no enode found: $TARGET_RESPONSE" >&2
    else
        echo "No response from node yet (attempt $i)..." >&2
    fi
    sleep 1
done

echo "Failed to retrieve enode" >&2
exit 1