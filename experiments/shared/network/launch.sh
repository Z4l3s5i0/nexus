#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export EXPERIMENTS_NETWORK_SUPPORT_DIR="$SCRIPT_DIR"
exec bash <(tr -d '\r' < "$SCRIPT_DIR/start.sh") "$@"