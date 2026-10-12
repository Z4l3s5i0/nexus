#!/bin/bash
set -e

# System Setup Script for v2 Deployment
# Installs Docker, Docker Compose, and other necessary tools.

echo "Starting system setup..."

# 1. Update system and install basic tools
echo "Updating package list..."
sudo apt-get update -y

echo "Installing essential packages..."
sudo apt-get install -y \
    curl \
    wget \
    git \
    jq \
    ca-certificates \
    gnupg \
    lsb-release \
    openssl

# 2. Install Docker if not present
if ! command -v docker &> /dev/null; then
    echo "Installing Docker..."
    sudo mkdir -p /etc/apt/keyrings
    curl -fsSL https://download.docker.com/linux/ubuntu/gpg | sudo gpg --dearmor -o /etc/apt/keyrings/docker.gpg

    echo \
      "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu \
      $(lsb_release -cs) stable" | sudo tee /etc/apt/sources.list.d/docker.list > /dev/null

    sudo apt-get update -y
    sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-compose-plugin
    
    # Add current user to docker group
    sudo usermod -aG docker $USER
    echo "Docker installed. Note: You might need to log out and back in for group changes to take effect."
else
    echo "Docker is already installed."
fi

# 3. Install Docker Compose (V2 is usually included in docker-ce as a plugin)
# But we can check for the docker-compose command as well.
if ! docker compose version &> /dev/null; then
    echo "Installing Docker Compose standalone..."
    sudo apt-get install -y docker-compose
else
    echo "Docker Compose (V2) is available."
fi

# Function to retry a command
retry_command() {
    local n=1
    local max=5
    local delay=5
    while true; do
        "$@" && break || {
            if [[ $n -lt $max ]]; then
                ((n++))
                echo "Command failed. Attempt $n/$max:"
                sleep $delay;
            else
                echo "The command has failed after $n attempts."
                return 1
            fi
        }
    done
}

# 4. Pull necessary common images to speed up first start
echo "Pre-pulling common Docker images..."
retry_command docker pull ethpandaops/ethereum-genesis-generator:6.1.3
retry_command docker pull sigp/lighthouse:v8.2.1
retry_command docker pull ghcr.io/ethstaker/ethstaker-deposit-cli:v1.3.0
retry_command docker pull prom/node-exporter:v1.8.1
retry_command docker pull grafana/grafana:11.1.0
retry_command docker pull prom/prometheus:v2.53.1
retry_command docker pull docker.io/z4l3s5i0/contender:latest
retry_command docker pull ghcr.io/foundry-rs/foundry:v1.7.1
echo "Setup complete!"
