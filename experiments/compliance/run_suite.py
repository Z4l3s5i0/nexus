#!/usr/bin/env python3
import argparse
import json
import re
import subprocess
import sys
import threading
from datetime import datetime, timezone
from pathlib import Path


CLIENTS = ("wasix-r-eth", "wasix-w-eth")


def container_name(run_id, client):
    value = re.sub(r"[^a-zA-Z0-9_.-]", "-", f"hive-{run_id}-{client}")
    return value[:120]


def build_hive_command(client, image, workspace, parallelism, name):
    return [
        "docker", "run", "--rm", "--name", name,
        "-v", "/var/run/docker.sock:/var/run/docker.sock",
        "-v", f"{workspace}:/hive/workspace",
        image,
        "--sim", "ethereum/engine",
        "--client", client,
        "--sim.parallelism", str(parallelism),
        "--docker.pull=false",
    ]


def forward_output(stream, log_file):
    for line in stream:
        log_file.write(line)
        log_file.flush()
        sys.stdout.write(line)
        sys.stdout.flush()


def run_clients(clients, image, workspace_root, logs_dir, results_path, run_id, parallelism, timeout):
    results = []
    for client in clients:
        workspace = Path(workspace_root) / client
        log_path = Path(logs_dir) / f"{client}.log"
        command_path = Path(logs_dir) / f"{client}.command.json"
        workspace.mkdir(parents=True, exist_ok=True)
        log_path.parent.mkdir(parents=True, exist_ok=True)
        name = container_name(run_id, client)
        command = build_hive_command(client, image, str(workspace), parallelism, name)
        command_path.write_text(json.dumps(command, indent=2) + "\n", encoding="utf-8")
        started = datetime.now(timezone.utc).isoformat()
        status = 1
        error = None
        print(f"Starting Hive ethereum/engine for {client}; live output is also saved to {log_path}.", flush=True)
        try:
            with log_path.open("w", encoding="utf-8") as output:
                process = subprocess.Popen(
                    command,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    bufsize=1,
                )
                output_thread = threading.Thread(target=forward_output, args=(process.stdout, output))
                output_thread.start()
                try:
                    status = process.wait(timeout=timeout)
                except subprocess.TimeoutExpired:
                    error = f"Hive suite exceeded timeout of {timeout} seconds"
                    subprocess.run(["docker", "stop", "--time", "20", name], capture_output=True, check=False)
                    subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
                    if process.poll() is None:
                        try:
                            process.kill()
                        except ProcessLookupError:
                            pass
                    process.wait()
                    status = 124
                output_thread.join()
        except OSError as exc:
            error = str(exc)
        print(f"Hive {client} finished with exit code {status}; log: {log_path}", flush=True)
        result = {
            "client": client,
            "simulation": "ethereum/engine",
            "parallelism": parallelism,
            "exit_code": status,
            "status": "passed" if status == 0 else "failed",
            "error": error,
            "started_at": started,
            "finished_at": datetime.now(timezone.utc).isoformat(),
            "log_path": str(log_path),
            "workspace": str(workspace),
            "command": command,
        }
        results.append(result)
        with Path(results_path).open("a", encoding="utf-8") as results_file:
            results_file.write(json.dumps(result, sort_keys=True) + "\n")
    return results


def main():
    parser = argparse.ArgumentParser(description="Run the custom clients against Hive ethereum/engine.")
    parser.add_argument("--clients", default=",".join(CLIENTS))
    parser.add_argument("--image", default="hive-custom:latest")
    parser.add_argument("--workspace-root", required=True)
    parser.add_argument("--logs-dir", required=True)
    parser.add_argument("--results", required=True)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--parallelism", type=int, default=20)
    parser.add_argument("--timeout", type=int, default=3600)
    args = parser.parse_args()
    clients = [client for client in args.clients.split(",") if client]
    if not clients or any(client not in CLIENTS for client in clients):
        parser.error(f"--clients must contain one or more of: {', '.join(CLIENTS)}")
    if len(set(clients)) != len(clients):
        parser.error("--clients must not contain duplicates")
    if args.parallelism < 1 or args.timeout < 1:
        parser.error("parallelism and timeout must be positive")
    results = run_clients(
        clients, args.image, args.workspace_root, args.logs_dir, args.results,
        args.run_id, args.parallelism, args.timeout,
    )
    return 0 if all(result["exit_code"] == 0 for result in results) else 1


if __name__ == "__main__":
    sys.exit(main())