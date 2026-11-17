# HIVE (Custom Client Test Suite) — Docker Guide

This guide explains how to build and run the customized HIVE test suite using Docker. This eliminates the need to manually install Go or manage local dependencies.

## Prerequisites
* [Docker](https://docs.docker.com/get-docker/) installed and running.

---

## Client Docker Images

Hive requires Docker images for each client. The custom clients (`wasix-r-eth` and `wasix-w-eth`) are defined in `hive/clients/`. Their Dockerfiles reference pre-built base images that must be available locally before running Hive.

### Build the Native Client Image

```bash
docker build -t z4l3s5i0/wasix-eth-linux:latest -f wasix_eth/Dockerfile.linux wasix_eth
```

### Build the WASIX Client Image

```bash
docker build -t z4l3s5i0/wasix-eth-wasix:latest -f wasix_eth/Dockerfile.wasix wasix_eth
```

---

## 1. Build the Hive Docker Image

Run the following command from the project root to clone the official [ethereum/hive](https://github.com/ethereum/hive) repository, compile both `hive` and `hiveview`, and include the custom client definitions:

```bash
docker build -f ./hive/hive.dockerfile -t hive-custom .
```

The `hive.dockerfile` copies the custom clients from `hive/clients/` into the image, so Hive can discover and build `wasix-r-eth` and `wasix-w-eth` at runtime.

## 2. Running the test suite

Run the Wasix Wasm-Ethereum Client (WASIX):
```bash
docker run --rm \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v $(pwd)/workspace:/hive/workspace \
  hive-custom \
  --sim ethereum/engine \
  --client wasix-w-eth \
  --sim.parallelism 5 \
  --docker.pull=false
```

Run the Wasix Rust-Ethereum Client (Native):
```bash
docker run --rm \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v $(pwd)/workspace:/hive/workspace \
  hive-custom \
  --sim ethereum/engine \
  --client wasix-r-eth \
  --sim.parallelism 20 \
  --docker.pull=false
```

Run the Hiveview Server:
```bash
docker run --rm -it -p 8080:8080 -v $(pwd)/workspace:/hive/workspace --entrypoint ./hiveview hive-custom --serve --logdir /hive/workspace/logs
```