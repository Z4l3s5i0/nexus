import re
import shlex
import shutil
import subprocess
import time


TX_HASH = re.compile(r"0x[0-9a-fA-F]{64}")
CONTRACT_CALL_GAS_LIMIT = 250_000
CONTRACT_DEPLOYMENT_GAS_LIMIT = 600_000_000


def docker_shell_command(cast_network, cast_image):
    return [
        "docker", "run", "-i", "--rm", "--network", cast_network,
        "--entrypoint", "/bin/sh", cast_image, "-s",
    ]


def transaction_hash(output):
    hashes = TX_HASH.findall(output)
    if not hashes:
        raise RuntimeError("cast did not return a transaction hash")
    return hashes[-1]


def run_cast(cast_bin, arguments, timeout, cast_network, cast_image, capture_timestamp=False):
    submitted_at = {"monotonic": time.monotonic(), "epoch": time.time()} if capture_timestamp else None
    docker_script = None
    if cast_bin:
        command = [cast_bin, *arguments]
    else:
        if not shutil.which("docker"):
            raise RuntimeError("cast is unavailable and Docker is not installed")
        if not cast_network:
            raise RuntimeError("a Docker network is required to run the bundled cast client")
        cast_command = shlex.join(["cast", *arguments])
        if capture_timestamp:
            script = (
                "set -e\n"
                "submitted_at=$(date +%s.%N)\n"
                f"output=$({cast_command})\n"
                "printf '%s\\n%s\\n' \"$output\" \"SUBMITTED_AT=$submitted_at\""
            )
        else:
            script = cast_command
        command = docker_shell_command(cast_network, cast_image)
        docker_script = script
    result = subprocess.run(
        command,
        input=docker_script,
        text=True,
        capture_output=True,
        timeout=timeout,
        check=False,
    )
    if result.returncode:
        message = result.stderr.strip() or result.stdout.strip() or "unknown cast error"
        for index, value in enumerate(arguments[:-1]):
            if value in ("--private-key", "--pk"):
                message = message.replace(arguments[index + 1], "[redacted]")
        raise RuntimeError(f"cast command failed: {message}")
    output = result.stdout.strip()
    if capture_timestamp and not cast_bin:
        marker = next((line for line in output.splitlines() if line.startswith("SUBMITTED_AT=")), None)
        if marker is None:
            raise RuntimeError("cast did not return a submission timestamp")
        epoch = float(marker.partition("=")[2])
        submitted_at = {
            "monotonic": submitted_at["monotonic"] + epoch - submitted_at["epoch"],
            "epoch": epoch,
        }
        output = "\n".join(line for line in output.splitlines() if line != marker).strip()
    if capture_timestamp:
        return output, submitted_at
    return output


def submit_burst(
    cast_bin,
    cast_network,
    cast_image,
    rpc_url,
    sender,
    private_key,
    first_nonce,
    count,
    timeout,
    gas_price,
    to_address=None,
    function_signature=None,
    include_timestamps=False,
    contract_gas_limit=CONTRACT_CALL_GAS_LIMIT,
):
    def arguments(nonce):
        destination = to_address or sender
        command = ["send", destination]
        if function_signature:
            command.extend([function_signature, "--gas-limit", str(contract_gas_limit)])
        else:
            command.extend(["--value", "0", "--gas-limit", "21000"])
        command.extend(
            [
                "--legacy",
                "--gas-price",
                str(gas_price),
                "--nonce",
                str(nonce),
                "--async",
                "--rpc-url",
                rpc_url,
                "--private-key",
                private_key,
            ]
        )
        return command

    if cast_bin:
        submitted = []
        for offset in range(count):
            result = run_cast(
                cast_bin,
                arguments(first_nonce + offset),
                timeout,
                cast_network,
                cast_image,
                capture_timestamp=include_timestamps,
            )
            if include_timestamps:
                output, submitted_at = result
                submitted.append({"hash": transaction_hash(output), "submitted_at": submitted_at})
            else:
                submitted.append(transaction_hash(result))
        return submitted

    if not shutil.which("docker"):
        raise RuntimeError("cast is unavailable and Docker is not installed")
    if not cast_network:
        raise RuntimeError("a Docker network is required to run the bundled cast client")
    if include_timestamps:
        lines = []
        for offset in range(count):
            command = shlex.join(["cast", *arguments(first_nonce + offset)])
            lines.append(
                f"submitted_at=$(date +%s.%N)\noutput=$({command})\n"
                "printf 'SUBMITTED_AT=%s\\n%s\\n' \"$submitted_at\" \"$output\""
            )
        script = "set -e\n" + "\n".join(lines)
        started = {"monotonic": time.monotonic(), "epoch": time.time()}
    else:
        script = "set -e\n" + "\n".join(
            shlex.join(["cast", *arguments(first_nonce + offset)]) for offset in range(count)
        )
    result = subprocess.run(
        docker_shell_command(cast_network, cast_image),
        input=script,
        text=True,
        capture_output=True,
        timeout=max(timeout, 30) * max(count, 1),
        check=False,
    )
    if result.returncode:
        message = result.stderr.strip() or result.stdout.strip() or "unknown cast error"
        message = message.replace(private_key, "[redacted]")
        raise RuntimeError(f"cast burst failed: {message}")
    if include_timestamps:
        submitted = []
        submitted_epoch = None
        output_lines = []
        for line in result.stdout.splitlines():
            if line.startswith("SUBMITTED_AT="):
                if submitted_epoch is not None:
                    epoch = submitted_epoch
                    submitted.append(
                        {
                            "hash": transaction_hash("\n".join(output_lines)),
                            "submitted_at": {
                                "monotonic": started["monotonic"] + epoch - started["epoch"],
                                "epoch": epoch,
                            },
                        }
                    )
                submitted_epoch = float(line.partition("=")[2])
                output_lines = []
                continue
            if submitted_epoch is not None:
                output_lines.append(line)
        if submitted_epoch is not None:
            epoch = submitted_epoch
            submitted.append(
                {
                    "hash": transaction_hash("\n".join(output_lines)),
                    "submitted_at": {
                        "monotonic": started["monotonic"] + epoch - started["epoch"],
                        "epoch": epoch,
                    },
                }
            )
    else:
        submitted = TX_HASH.findall(result.stdout)
    if len(submitted) != count:
        raise RuntimeError(f"cast returned {len(submitted)} transaction hash(es), expected {count}")
    return submitted


def submit_contract_deployment(
    cast_bin,
    cast_network,
    cast_image,
    rpc_url,
    private_key,
    nonce,
    bytecode,
    timeout,
    gas_price,
    gas_limit=CONTRACT_DEPLOYMENT_GAS_LIMIT,
):
    output, submitted_at = run_cast(
        cast_bin,
        [
            "send",
            "--legacy",
            "--gas-limit",
            str(gas_limit),
            "--gas-price",
            str(gas_price),
            "--nonce",
            str(nonce),
            "--async",
            "--rpc-url",
            rpc_url,
            "--private-key",
            private_key,
            "--create",
            "0x" + bytecode.removeprefix("0x"),
        ],
        timeout,
        cast_network,
        cast_image,
        capture_timestamp=True,
    )
    return transaction_hash(output), submitted_at