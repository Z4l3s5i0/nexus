# Builder stage
FROM rust:1.91-slim-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Copy the source code
COPY . .

# The WASIX overlay is for cross-compilation, not this native Linux image.
RUN rm -f /app/.cargo/config.toml && cargo build --release --locked

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y libssl3 ca-certificates curl && rm -rf /var/lib/apt/lists/*

# Copy the binary from the builder stage
COPY --from=builder /app/target/release/metrics_server /app/metrics_server

# Copy the default config
COPY config /app/config
RUN chmod -R 755 /app/config

# Create data directory
RUN mkdir -p /app/data

# Expose the API port
EXPOSE 9100

# Default logging level
ENV RUST_LOG=info

# Set the entrypoint
ENTRYPOINT ["/app/metrics_server"]
CMD ["run", "--config", "config/docker.toml"]
