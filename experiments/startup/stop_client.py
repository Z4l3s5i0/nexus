#!/usr/bin/env python3
import argparse
import json
import os
import signal
import subprocess
import time
from pathlib import Path


def stop_client(state_path, timeout=5, keep_failed=False):
    state = json.loads(Path(state_path).read_text(encoding="utf-8"))
    container_name = state.get("container_name")
    if container_name:
        if keep_failed and not state.get("ready", False):
            inspected = subprocess.run(
                ["docker", "inspect", "--format", "{{.State.Status}}", container_name],
                capture_output=True,
                check=False,
            )
            if inspected.returncode == 0:
                rpc_port = state.get("rpc_port")
                print(f"Keeping failed client container {container_name} for debugging.")
                print(f"Container state: {inspected.stdout.strip()}")
                print(f"RPC endpoint: http://127.0.0.1:{rpc_port}")
                print(
                    "Probe with:\n"
                    "  curl -sS -H 'Content-Type: application/json' "
                    "--data '{\"jsonrpc\":\"2.0\",\"method\":\"eth_chainId\",\"params\":[],\"id\":1}' "
                    f"http://127.0.0.1:{rpc_port}"
                )
                print(f"Inspect logs: docker logs {container_name}")
                print(f"Remove when finished: docker rm -f {container_name}")
                return
        subprocess.run(["docker", "stop", "--time", str(timeout), container_name], capture_output=True, check=False)
        subprocess.run(["docker", "rm", "-f", container_name], capture_output=True, check=False)
        return

    pid = state.get("pid")
    if not pid:
        return
    try:
        os.kill(int(pid), signal.SIGTERM)
    except ProcessLookupError:
        return
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            os.kill(int(pid), 0)
        except ProcessLookupError:
            return
        time.sleep(0.1)
    try:
        os.kill(int(pid), signal.SIGKILL)
    except ProcessLookupError:
        pass


def main():
    parser = argparse.ArgumentParser(description="Stop a startup benchmark client by its recorded run state.")
    parser.add_argument("--state", required=True)
    parser.add_argument("--timeout", type=int, default=5)
    parser.add_argument("--keep-failed", action="store_true", help="keep a failed Docker client for debugging")
    args = parser.parse_args()
    stop_client(args.state, args.timeout, args.keep_failed)


if __name__ == "__main__":
    main()