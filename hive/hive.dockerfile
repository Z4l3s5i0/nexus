# ==========================================
# Stage 1: Build the binaries
# ==========================================
FROM golang:1.24-alpine AS builder

# Install git since we need to clone the repo
RUN apk add --no-cache git

WORKDIR /app

# Clone the repository and switch to the custom branch
RUN git clone https://github.com/Z4l3s5i0/hive.git . && \
    git checkout custom && \
    git pull

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

# Copy the config and simulator directories required for HIVE
COPY --from=builder /app/clients /hive/clients
COPY --from=builder /app/simulators /hive/simulators

# Expose the default hiveview port (adjust if hiveview uses a different port)
EXPOSE 8080

# Set hive as the default entrypoint
ENTRYPOINT ["./hive"]