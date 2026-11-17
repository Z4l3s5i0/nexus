# Use a base image with required libraries
FROM --platform=linux/amd64 z4l3s5i0/wasix-eth-linux:latest

# Install basic dependencies
RUN apt-get update && apt-get install -y \
    bash \
    jq \
    curl \
    iproute2 \
    && rm -rf /var/lib/apt/lists/*


# Generate a dummy version.txt (or handle it more properly if the binary supports --version)
RUN echo "wasix-eth-0.1.0" > /version.txt

# Inject the startup script.
ADD wasix-eth.sh /wasix-eth.sh
ADD mapper.jq /mapper.jq
RUN chmod +x /wasix-eth.sh

# Inject the enode id retriever script.
RUN mkdir /hive-bin
ADD enode.sh /hive-bin/enode.sh
RUN chmod +x /hive-bin/enode.sh

# Add a default genesis file.
ADD genesis.json /genesis.json

# Export the usual networking ports
EXPOSE 8545 8551 30303 30304 3000 9050

ENTRYPOINT ["/wasix-eth.sh"]
