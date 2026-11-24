#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
WASMER_DIR="${WASMER_DIR:-$HOME/.wasmer}"
export CARGO_HOME WASMER_DIR
export PATH="$CARGO_HOME/bin:$WASMER_DIR/bin:$PATH"
required_packages=(curl wget git jq ca-certificates gnupg lsb-release openssl python3 cloc)
required_commands=(curl wget git jq openssl python3 cloc)

usage() {
    cat <<EOF
Usage: bash experiments/setup.sh

Installs shared Linux tools and the dependencies for the selected stages in
EXPERIMENTS_SELECTED. When invoked directly without that variable, prepares all
five experiment stages.
EOF
}

fail() {
    echo "Setup error: $*" >&2
    exit 1
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi
if [[ "$#" -gt 0 ]]; then
    echo "Setup does not accept arguments." >&2
    usage >&2
    exit 2
fi

as_root() {
    if [[ "$EUID" -eq 0 ]]; then
        "$@"
    elif command -v sudo >/dev/null 2>&1; then
        sudo "$@"
    else
        fail "root privileges or sudo are required to install prerequisites."
    fi
}

if [[ "$(uname -s)" != "Linux" ]]; then
    fail "automatic setup supports Ubuntu/Debian Linux; install Docker with Compose, Python 3, cloc, curl, wget, git, and jq manually on this system."
fi
if [[ ! -r /etc/os-release ]]; then
    fail "cannot identify the Linux distribution; install prerequisites manually."
fi
# shellcheck disable=SC1091
source /etc/os-release
if [[ "${ID:-}" != "ubuntu" && "${ID:-}" != "debian" ]]; then
    fail "automatic package installation supports Ubuntu/Debian, not '${ID:-unknown}'."
fi
command -v apt-get >/dev/null 2>&1 || fail "apt-get is required for automatic prerequisite installation."

selection="${EXPERIMENTS_SELECTED:-runtime,empty-block,startup,compliance,size}"
selection="${selection//,/ }"
read -r -a selected_experiments <<< "$selection"
needs_docker=false
needs_chain_images=false
needs_metrics_image=false
needs_client_binaries=false
for experiment in "${selected_experiments[@]}"; do
    case "$experiment" in
        runtime|empty-block|startup)
            needs_docker=true
            needs_chain_images=true
            needs_metrics_image=true
            needs_client_binaries=true
            ;;
        compliance)
            needs_docker=true
            needs_client_binaries=true
            ;;
        size)
            needs_client_binaries=true
            ;;
        *) fail "unknown experiment '$experiment' in EXPERIMENTS_SELECTED." ;;
    esac
done

echo "Installing shared experiment tools..."
as_root apt-get update -y
as_root apt-get install -y "${required_packages[@]}"

for command_name in "${required_commands[@]}"; do
    command -v "$command_name" >/dev/null 2>&1 || fail "required command '$command_name' is still unavailable."
done

if [[ "$needs_client_binaries" == true ]]; then
    if ! command -v rustup >/dev/null 2>&1; then
        echo "Installing the Rust toolchain..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
        export PATH="$CARGO_HOME/bin:$WASMER_DIR/bin:$PATH"
    fi
    command -v rustup >/dev/null 2>&1 || fail "rustup is required to install the stable Rust toolchain."
    command -v cargo >/dev/null 2>&1 || fail "Cargo is required to install cargo-wasix."
    rustup toolchain install stable --profile minimal || fail "could not install the stable Rust toolchain."
    if ! command -v cargo-wasix >/dev/null 2>&1; then
        echo "Installing cargo-wasix with stable Rust..."
        cargo +stable install cargo-wasix || fail "cargo-wasix installation failed."
    fi
    command -v cargo-wasix >/dev/null 2>&1 || fail "cargo-wasix is still unavailable after installation."

    if ! command -v wasmer >/dev/null 2>&1 || [[ "$(wasmer --version 2>/dev/null || true)" != *"7.4.2"* ]]; then
        echo "Installing Wasmer 7.4.2..."
        curl -fsSL https://get.wasmer.io | sh -s "v7.4.2"
        export PATH="$CARGO_HOME/bin:$WASMER_DIR/bin:$PATH"
    fi
    command -v wasmer >/dev/null 2>&1 || fail "Wasmer is still unavailable after installation."
    [[ "$(wasmer --version 2>/dev/null || true)" == *"7.4.2"* ]] || fail "Wasmer 7.4.2 is required, but the installed version is different."

    wasix_eth_root="${EXPERIMENTS_WASIX_ETH_ROOT:-$REPO_ROOT/wasix_eth}"
    native_binary="$wasix_eth_root/target/release/wasix_eth"
    wasix_binary="$wasix_eth_root/target/wasm32-wasmer-wasi/release/wasix_eth.wasi.wasm"
    if [[ ! -s "$native_binary" ]]; then
        command -v cargo >/dev/null 2>&1 || fail "native NEXUS binary is missing and Cargo is unavailable; install Rust or build $native_binary manually."
        echo "Building the native NEXUS binary once..."
        (cd "$wasix_eth_root" && cargo build --release) || fail "native NEXUS build failed."
    fi
    if [[ ! -s "$wasix_binary" ]]; then
        command -v cargo >/dev/null 2>&1 || fail "WASIX NEXUS binary is missing and Cargo is unavailable; install the WASIX toolchain and build $wasix_binary manually."
        echo "Building the WASIX NEXUS binary once..."
        (cd "$wasix_eth_root" && cargo +stable wasix build --release) || fail "WASIX NEXUS build failed with the stable cargo-wasix toolchain."
    fi
    [[ -s "$native_binary" ]] || fail "native NEXUS binary was not produced at $native_binary."
    [[ -s "$wasix_binary" ]] || fail "WASIX NEXUS binary was not produced at $wasix_binary."
fi

if [[ "$needs_docker" == true ]]; then
    if ! command -v docker >/dev/null 2>&1; then
        architecture="$(dpkg --print-architecture)"
        codename="${VERSION_CODENAME:-}"
        [[ -n "$codename" ]] || fail "could not determine the distribution codename for Docker installation."
        as_root install -m 0755 -d /etc/apt/keyrings
        curl -fsSL "https://download.docker.com/linux/$ID/gpg" | as_root gpg --dearmor --yes -o /etc/apt/keyrings/docker.gpg
        as_root chmod a+r /etc/apt/keyrings/docker.gpg
        printf 'deb [arch=%s signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/%s %s stable\n' \
            "$architecture" "$ID" "$codename" | as_root tee /etc/apt/sources.list.d/docker.list >/dev/null
        as_root apt-get update -y
        as_root apt-get install -y docker-ce docker-ce-cli containerd.io docker-compose-plugin
    fi
    if ! docker compose version >/dev/null 2>&1; then
        as_root apt-get install -y docker-compose-plugin
    fi
    command -v docker >/dev/null 2>&1 || fail "Docker is still unavailable."
    docker compose version >/dev/null 2>&1 || fail "Docker Compose v2 is unavailable."
    docker info >/dev/null 2>&1 || fail "Docker is installed but the daemon is not reachable by the current user. Start Docker and grant this user access before running experiments."
else
    echo "Skipping Docker installation and image preparation; selected stages do not use Docker."
fi

retry_pull() {
    local image="$1"
    local attempt
    for attempt in 1 2 3 4 5; do
        if docker pull "$image"; then
            return 0
        fi
        if [[ "$attempt" -lt 5 ]]; then
            echo "Pull failed for $image (attempt $attempt/5); retrying in 5 seconds..." >&2
            sleep 5
        fi
    done
    fail "could not pull shared image '$image'."
}

if [[ "$needs_chain_images" == true ]]; then
    echo "Preparing shared execution and monitoring images..."
    for image in \
        ethpandaops/ethereum-genesis-generator:6.1.3 \
        sigp/lighthouse:v8.2.1 \
        ghcr.io/ethstaker/ethstaker-deposit-cli:v1.3.0 \
        prom/node-exporter:v1.8.1 \
        grafana/grafana:11.1.0 \
        prom/prometheus:v2.53.1 \
        docker.io/z4l3s5i0/contender:latest \
        ghcr.io/foundry-rs/foundry:v1.7.1 \
        ethereum/client-go@sha256:798b7eb1bcef6d4be7576232beea63bf291450f48b025dc8fb5c6e37840e4364 \
        ghcr.io/paradigmxyz/reth:latest; do
        retry_pull "$image"
    done
fi

if [[ "$needs_chain_images" == true ]]; then
    for experiment in "${selected_experiments[@]}"; do
        if [[ "$experiment" == startup ]]; then
            retry_pull docker.io/z4l3s5i0/wasix-eth-linux:clean
            retry_pull docker.io/z4l3s5i0/wasix-eth-wasix:clean
            break
        fi
    done
fi

if [[ "$needs_metrics_image" == true ]]; then
    echo "Building the shared metrics-server image once..."
    docker build -t wasix-eth-metrics:latest -f "$REPO_ROOT/metrics_server/Dockerfile" "$REPO_ROOT/metrics_server"
fi

echo "Shared experiment setup complete."