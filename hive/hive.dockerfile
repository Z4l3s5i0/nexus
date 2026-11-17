# ==========================================
# Stage 1: Build the binaries
# ==========================================
FROM golang:1.24-alpine AS builder

# Install git since we need to clone the repo
RUN apk add --no-cache git

WORKDIR /app

# Clone the official ethereum/hive repository
RUN git clone https://github.com/ethereum/hive.git .

# Build the main hive binary
RUN go build -o hive .

# Build the hiveview binary
RUN go build -o hiveview ./cmd/hiveview

# ==========================================
# Stage 2: Final lightweight runtime image
# ==========================================
FROM alpine:latest

# Install basic dependencies (like bash or certificates if needed by tests)
RUN apk add --no-cache libc6-compat bash

WORKDIR /hive

# Copy the compiled binaries from the builder stage
COPY --from=builder /app/hive /hive/hive
COPY --from=builder /app/hiveview /hive/hiveview

# Copy the simulators directory from the official hive repo
COPY --from=builder /app/simulators /hive/simulators

# Copy the official clients directory from the ethereum/hive repo
COPY --from=builder /app/clients /hive/clients

# Add our custom clients alongside the official ones
COPY hive/clients /hive/clients

# Expose the default hiveview port (adjust if hiveview uses a different port)
EXPOSE 8080

# Set hive as the default entrypoint
ENTRYPOINT ["./hive"]