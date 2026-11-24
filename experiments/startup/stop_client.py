#!/usr/bin/env python3
import argparse
import json
import os
import signal
import subprocess
import time
from pathlib import Path


def stop_client(state_path, timeout=5):
    state = json.loads(Path(state_path).read_text(encoding="utf-8"))
    container_name = state.get("container_name")
    if container_name:
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
    args = parser.parse_args()
    stop_client(args.state, args.timeout)


if __name__ == "__main__":
    main()