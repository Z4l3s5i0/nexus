#!/usr/bin/env python3
"""Launch one isolated client and measure time to its first successful RPC response."""

import argparse
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from urllib.error import URLError
from urllib.request import Request, urlopen


CLIENT_NAMES = {
    "geth": "geth",
    "reth": "reth",
    "nexus-linux": "wasix-eth",
    "nexus-wasix-docker": "wasix-eth",
    "nexus-wasmer": "wasix-eth",
}


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def safe_container_name(run_id, config, iteration):
    value = re.sub(r"[^a-zA-Z0-9_.-]", "-", f"startup-{run_id}-{config}-{iteration}")
    return value[:120]


def rpc_ready(port, timeout=2):
    body = json.dumps({"jsonrpc": "2.0", "method": "eth_chainId", "params": [], "id": 1}).encode()
    request = Request(
        f"http://127.0.0.1:{port}", data=body, headers={"Content-Type": "application/json"}
    )
    try:
        with urlopen(request, timeout=timeout) as response:
            payload = json.loads(response.read().decode("utf-8"))
        return isinstance(payload.get("result"), str) and payload["result"].startswith("0x")
    except (OSError, ValueError, URLError):
        return False


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def run_checked(command, log_file):
    with Path(log_file).open("ab") as output:
        result = subprocess.run(command, stdout=output, stderr=subprocess.STDOUT, check=False)
    if result.returncode:
        raise RuntimeError(f"client setup command failed with exit status {result.returncode}")


def ensure_image(image, log_file):
    result = subprocess.run(
        ["docker", "image", "inspect", image], capture_output=True, check=False
    )
    if result.returncode:
        run_checked(["docker", "pull", image], log_file)


def prepare_config(args, work_dir, log_file):
    work_dir = Path(work_dir).resolve()
    (work_dir / "data").mkdir(parents=True, exist_ok=True)
    (work_dir / "jwt.hex").write_text(os.urandom(32).hex() + "\n", encoding="ascii")
    if args.config == "geth":
        ensure_image(args.geth_image, log_file)
        run_checked(
            [
                "docker", "run", "--rm", "-v", f"{work_dir}:/data", args.geth_image,
                "--datadir=/data/data/geth", "init", "/data/genesis.json",
            ],
            log_file,
        )
    elif args.config == "reth":
        ensure_image(args.reth_image, log_file)
        run_checked(
            [
                "docker", "run", "--rm", "-v", f"{work_dir}:/data", args.reth_image,
                "init", "--chain", "/data/genesis.json", "--datadir", "/data/data/reth",
            ],
            log_file,
        )
    elif args.config == "nexus-linux":
        ensure_image(args.nexus_linux_image, log_file)
    elif args.config == "nexus-wasix-docker":
        if not args.nexus_wasix_image:
            raise ValueError("--nexus-wasix-image is required for nexus-wasix-docker")
        ensure_image(args.nexus_wasix_image, log_file)
    elif args.config == "nexus-wasmer":
        wasmer = shutil.which(args.wasmer_bin)
        if not args.nexus_wasix_wasm:
            raise ValueError("--nexus-wasix-wasm is required for nexus-wasmer")
        if not wasmer:
            raise ValueError(f"Wasmer executable not found: {args.wasmer_bin}")
        run_checked(
            [
                wasmer, "run", args.nexus_wasix_wasm, "--enable-async-threads", "--net",
                "--volume", f"{work_dir / 'data' / 'nexus'}:/app/data",
                "--volume", f"{work_dir}:/app/startup", "--", "init", "--data-dir", "/app/data",
                "--genesis-path", "/app/startup/genesis.json", "--eth-rpc-port", str(args.rpc_port),
                "--auth-rpc-port", str(args.rpc_port + 1), "--auth-rpc-jwt-path", "/app/startup/jwt.hex",
                "--verbose", "0",
            ],
            log_file,
        )


def launch_docker(args, work_dir, image, internal_port, docker_args, container_name):
    work_dir = str(Path(work_dir).resolve())
    command = [
        "docker", "run", "--rm", "-d", "--name", container_name,
        "-v", f"{work_dir}:/data", "-p", f"{args.rpc_port}:{internal_port}",
        image, *docker_args,
    ]
    result = subprocess.run(command, text=True, capture_output=True, check=False)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or f"docker run failed for {image}")
    inspect = subprocess.run(
        ["docker", "inspect", "--format", "{{.State.Pid}} {{.Image}}", container_name],
        text=True,
        capture_output=True,
        check=False,
    )
    values = inspect.stdout.strip().split() if inspect.returncode == 0 else []
    pid = int(values[0]) if values and values[0].isdigit() else None
    image_id = values[1] if len(values) > 1 else None
    return {"container_name": container_name, "pid": pid, "image": image, "image_id": image_id}


def launch_config(args, work_dir, log_file, container_name):
    work_dir = Path(work_dir).resolve()
    data = work_dir / "data"
    rpc_port = args.rpc_port
    auth_port = rpc_port + 1

    if args.config == "geth":
        return launch_docker(
            args, work_dir, args.geth_image, 8545,
            [
                "--datadir=/data/data/geth", f"--networkid={args.chain_id}", "--http",
                "--http.addr=0.0.0.0", "--http.port=8545", "--http.api=eth,net,web3",
                "--http.vhosts=*", "--authrpc.addr=0.0.0.0", "--authrpc.port=8551",
                "--authrpc.vhosts=*", "--authrpc.jwtsecret=/data/jwt.hex", "--maxpeers=0", "--nodiscover",
            ],
            container_name,
        )

    if args.config == "reth":
        return launch_docker(
            args, work_dir, args.reth_image, 8545,
            [
                "node", "--datadir", "/data/data/reth", "--http", "--http.addr", "0.0.0.0",
                "--http.port", "8545", "--authrpc.addr", "0.0.0.0", "--authrpc.port", "8551",
                "--authrpc.jwtsecret", "/data/jwt.hex", "--max-outbound-peers", "0", "--max-inbound-peers", "0",
            ],
            container_name,
        )

    if args.config == "nexus-linux":
        return launch_docker(
            args, work_dir, args.nexus_linux_image, rpc_port,
            [
                "--data-dir", "/data/data/nexus", "--genesis-path", "/data/genesis.json",
                "--eth-rpc-port", str(rpc_port), "--auth-rpc-port", str(auth_port),
                "--auth-rpc-jwt-path", "/data/jwt.hex", "--verbose", "0",
            ],
            container_name,
        )

    if args.config == "nexus-wasix-docker":
        if not args.nexus_wasix_image:
            raise ValueError("--nexus-wasix-image is required for nexus-wasix-docker")
        command = (
            "wasmer run /app/wasix_eth.wasm --enable-async-threads --net "
            "--volume /data/data/nexus:/app/data --volume /data:/app/startup -- "
            f"init --data-dir /app/data --genesis-path /app/startup/genesis.json --eth-rpc-port {rpc_port} "
            f"--auth-rpc-port {auth_port} --auth-rpc-jwt-path /app/startup/jwt.hex --verbose 0 && "
            "wasmer run /app/wasix_eth.wasm --enable-async-threads --net "
            "--volume /data/data/nexus:/app/data --volume /data:/app/startup -- "
            f"run --data-dir /app/data --genesis-path /app/startup/genesis.json --eth-rpc-port {rpc_port} "
            f"--auth-rpc-port {auth_port} --auth-rpc-jwt-path /app/startup/jwt.hex --verbose 0"
        )
        return launch_docker(args, work_dir, args.nexus_wasix_image, rpc_port, ["sh", "-c", command], container_name)

    if args.config == "nexus-wasmer":
        wasmer = shutil.which(args.wasmer_bin)
        if not wasmer:
            raise ValueError(f"Wasmer executable not found: {args.wasmer_bin}")
        command = [
            wasmer, "run", args.nexus_wasix_wasm, "--enable-async-threads", "--net",
            "--volume", f"{data}:/app/data", "--volume", f"{Path(work_dir).resolve()}:/app/startup",
            "--", "init", "--data-dir", "/app/data", "--genesis-path", "/app/startup/genesis.json",
            "--eth-rpc-port", str(rpc_port), "--auth-rpc-port", str(auth_port),
            "--auth-rpc-jwt-path", "/app/startup/jwt.hex", "--verbose", "0",
        ]
        run_command = [
            wasmer, "run", args.nexus_wasix_wasm, "--enable-async-threads", "--net",
            "--volume", f"{data}:/app/data", "--volume", f"{Path(work_dir).resolve()}:/app/startup",
            "--", "run", "--data-dir", "/app/data", "--genesis-path", "/app/startup/genesis.json",
            "--eth-rpc-port", str(rpc_port), "--auth-rpc-port", str(auth_port),
            "--auth-rpc-jwt-path", "/app/startup/jwt.hex", "--verbose", "0",
        ]
        with Path(log_file).open("ab") as output:
            process = subprocess.Popen(run_command, stdout=output, stderr=subprocess.STDOUT)
        digest = hashlib.sha256(Path(args.nexus_wasix_wasm).read_bytes()).hexdigest()
        return {
            "container_name": None,
            "pid": process.pid,
            "image": args.nexus_wasix_wasm,
            "image_id": f"sha256:{digest}",
        }

    raise ValueError(f"unknown startup configuration: {args.config}")


def run(args):
    if args.rpc_port == 0:
        args.rpc_port = free_port()
    work_dir = Path(args.work_dir).resolve()
    work_dir.mkdir(parents=True, exist_ok=True)
    log_file = Path(args.log_file)
    log_file.parent.mkdir(parents=True, exist_ok=True)
    container_name = safe_container_name(args.run_id, args.config, args.iteration)
    identity = {
        "experiment_id": args.run_id,
        "config": args.config,
        "iteration": args.iteration,
        "client": CLIENT_NAMES[args.config],
        "client_variant": args.config,
        "node_id": f"startup-{args.config}-{args.iteration}",
        "chain_id": args.chain_id,
        "rpc_port": args.rpc_port,
    }
    state = {**identity, "container_name": None, "pid": None, "ready": False}
    result = {**identity, "image": None, "image_id": None, "ready": False, "startup_time_ms": None, "error": None}
    write_json(args.state_file, state)

    try:
        if not Path(args.genesis).is_file():
            raise FileNotFoundError(f"genesis file not found: {args.genesis}")
        if args.prepare_only:
            prepare_config(args, work_dir, log_file)
            state["prepared"] = True
            write_json(args.state_file, state)
            return 0
        if not args.skip_preparation:
            prepare_config(args, work_dir, log_file)
        if args.config != "nexus-wasmer":
            state["container_name"] = container_name
            write_json(args.state_file, state)
        started = time.perf_counter_ns()
        launched = launch_config(args, work_dir, log_file, container_name)
        state.update(launched)
        state["started_at"] = datetime.now(timezone.utc).isoformat()
        write_json(args.state_file, state)

        deadline = time.monotonic() + args.timeout
        ready = False
        while time.monotonic() < deadline:
            if rpc_ready(args.rpc_port, min(2, args.poll_interval + 1)):
                ready = True
                break
            time.sleep(min(args.poll_interval, max(0, deadline - time.monotonic())))
        finished = time.perf_counter_ns()
        if ready:
            result["ready"] = True
            result["ready_at"] = datetime.now(timezone.utc).isoformat()
            result["startup_time_ms"] = (finished - started) / 1_000_000
        else:
            result["error"] = f"RPC did not become ready within {args.timeout} seconds"
            result["timeout_elapsed_ms"] = (finished - started) / 1_000_000
        result["image"] = launched["image"]
        result["image_id"] = launched.get("image_id")
        state["ready"] = ready
        state["startup_time_ms"] = result["startup_time_ms"]
        write_json(args.state_file, state)
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as exc:
        result["error"] = str(exc)
        result["finished_at"] = datetime.now(timezone.utc).isoformat()
    write_json(args.result_file, result)
    return 0 if result["ready"] else 2


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", choices=tuple(CLIENT_NAMES), required=True)
    parser.add_argument("--iteration", type=int, required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--chain-id", type=int, required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--genesis", required=True)
    parser.add_argument("--state-file", required=True)
    parser.add_argument("--result-file", required=True)
    parser.add_argument("--log-file", required=True)
    parser.add_argument("--rpc-port", type=int, default=0)
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument("--poll-interval", type=float, default=0.25)
    preparation = parser.add_mutually_exclusive_group()
    preparation.add_argument("--prepare-only", action="store_true")
    preparation.add_argument("--skip-preparation", action="store_true")
    parser.add_argument("--geth-image", default="ethereum/client-go@sha256:798b7eb1bcef6d4be7576232beea63bf291450f48b025dc8fb5c6e37840e4364")
    parser.add_argument("--reth-image", default="ghcr.io/paradigmxyz/reth:latest")
    parser.add_argument("--nexus-linux-image", default="docker.io/z4l3s5i0/wasix-eth-linux:clean")
    parser.add_argument("--nexus-wasix-image", default="docker.io/z4l3s5i0/wasix-eth-wasix:clean")
    parser.add_argument("--nexus-wasix-wasm", default="")
    parser.add_argument("--wasmer-bin", default="wasmer")
    args = parser.parse_args()
    if args.iteration < 1 or args.chain_id < 1 or args.rpc_port < 0 or args.timeout <= 0 or args.poll_interval <= 0:
        parser.error("iteration, chain ID, RPC port, timeout, and poll interval must be positive")
    return run(args)


if __name__ == "__main__":
    sys.exit(main())