import re
import shlex
import shutil
import subprocess
import time


TX_HASH = re.compile(r"0x[0-9a-fA-F]{64}")
CONTRACT_CALL_GAS_LIMIT = 250_000
CONTRACT_DEPLOYMENT_GAS_LIMIT = 600_000_000
MAX_BURST_TIMEOUT_SECONDS = 300


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
    submission_clock = {"monotonic": time.monotonic(), "epoch": time.time()} if capture_timestamp else None
    submitted_at = None
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
                f"output=$({cast_command})\n"
                "submitted_at=$(date +%s.%N)\n"
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
    if capture_timestamp and cast_bin:
        submitted_at = {"monotonic": time.monotonic(), "epoch": time.time()}
    elif capture_timestamp:
        marker = next((line for line in output.splitlines() if line.startswith("SUBMITTED_AT=")), None)
        if marker is None:
            raise RuntimeError("cast did not return a submission timestamp")
        epoch = float(marker.partition("=")[2])
        submitted_at = {
            "monotonic": submission_clock["monotonic"] + epoch - submission_clock["epoch"],
            "epoch": epoch,
        }
        output = "\n".join(line for line in output.splitlines() if line != marker).strip()
    if capture_timestamp:
        return output, submitted_at
    return output


def run_cast_batch(cast_bin, cast_network, cast_image, argument_batches, timeout):
    if not argument_batches:
        return []
    if cast_bin:
        return [
            run_cast(cast_bin, arguments, timeout, cast_network, cast_image)
            for arguments in argument_batches
        ]
    if not shutil.which("docker"):
        raise RuntimeError("cast is unavailable and Docker is not installed")
    if not cast_network:
        raise RuntimeError("a Docker network is required to run the bundled cast client")

    markers = [
        (f"__CAST_RESULT_{index}_START__", f"__CAST_RESULT_{index}_END__")
        for index in range(len(argument_batches))
    ]
    script_lines = ["set -e"]
    for arguments, (start_marker, end_marker) in zip(argument_batches, markers):
        script_lines.extend(
            [
                f"printf '%s\\n' {shlex.quote(start_marker)}",
                shlex.join(["cast", *arguments]),
                f"printf '%s\\n' {shlex.quote(end_marker)}",
            ]
        )
    result = subprocess.run(
        docker_shell_command(cast_network, cast_image),
        input="\n".join(script_lines),
        text=True,
        capture_output=True,
        timeout=timeout,
        check=False,
    )
    if result.returncode:
        message = result.stderr.strip() or result.stdout.strip() or "unknown cast error"
        for arguments in argument_batches:
            for index, value in enumerate(arguments[:-1]):
                if value in ("--private-key", "--pk"):
                    message = message.replace(arguments[index + 1], "[redacted]")
            if len(arguments) > 2 and arguments[:2] == ["wallet", "private-key"]:
                message = message.replace(arguments[2], "[redacted]")
        raise RuntimeError(f"cast batch failed: {message}")

    outputs = []
    output_lines = result.stdout.splitlines()
    for start_marker, end_marker in markers:
        try:
            start = output_lines.index(start_marker) + 1
            end = output_lines.index(end_marker, start)
        except ValueError as exc:
            raise RuntimeError("cast batch returned incomplete command output") from exc
        outputs.append("\n".join(output_lines[start:end]).strip())
    return outputs


def cast_send_arguments(transaction, rpc_url):
    arguments = ["send", transaction["to_address"]]
    signature = transaction.get("function_signature")
    if signature:
        arguments.extend([signature, "--gas-limit", str(transaction["gas_limit"])])
    else:
        arguments.extend(
            ["--value", str(transaction.get("value", "1")), "--gas-limit", str(transaction.get("gas_limit", 21000))]
        )
    arguments.extend(
        [
            "--legacy",
            "--gas-price",
            str(transaction["gas_price"]),
            "--nonce",
            str(transaction["nonce"]),
            "--async",
            "--rpc-url",
            rpc_url,
            "--private-key",
            transaction["private_key"],
        ]
    )
    return arguments


def submit_transactions(cast_bin, cast_network, cast_image, rpc_url, transactions, timeout):
    if not transactions:
        return []
    outputs = run_cast_batch(
        cast_bin,
        cast_network,
        cast_image,
        [cast_send_arguments(transaction, rpc_url) for transaction in transactions],
        timeout,
    )
    if len(outputs) != len(transactions):
        raise RuntimeError(f"cast returned {len(outputs)} result(s), expected {len(transactions)}")
    return [transaction_hash(output) for output in outputs]


def submit_transaction_schedule(cast_network, cast_image, rpc_url, transactions, tps, duration, timeout):
    if not transactions:
        return []
    expected_count = tps * duration
    if tps < 1 or duration < 1 or len(transactions) != expected_count:
        raise ValueError("transaction schedule must contain exactly TPS multiplied by duration transactions")
    if not shutil.which("docker"):
        raise RuntimeError("cast is unavailable and Docker is not installed")
    if not cast_network:
        raise RuntimeError("a Docker network is required to run the bundled cast client")

    script_lines = ["set -e", "start_ns=$(date +%s%N)"]
    for index, transaction in enumerate(transactions, start=1):
        script_lines.append(shlex.join(["cast", *cast_send_arguments(transaction, rpc_url)]))
        if index % tps == 0:
            batch_number = index // tps
            script_lines.extend(
                [
                    f"target_ns=$((start_ns + {batch_number}000000000))",
                    "now_ns=$(date +%s%N)",
                    "remaining_ns=$((target_ns - now_ns))",
                    'if [ "$remaining_ns" -gt 0 ]; then '
                    'sleep "$(printf \'%d.%09d\' "$((remaining_ns / 1000000000))" '
                    '"$((remaining_ns % 1000000000))")"; fi',
                ]
            )

    script_lines.extend(
        [
            f"target_ns=$((start_ns + {duration}000000000))",
            "now_ns=$(date +%s%N)",
            "remaining_ns=$((target_ns - now_ns))",
            'if [ "$remaining_ns" -gt 0 ]; then '
            'sleep "$(printf \'%d.%09d\' "$((remaining_ns / 1000000000))" '
            '"$((remaining_ns % 1000000000))")"; fi',
        ]
    )
    result = subprocess.run(
        docker_shell_command(cast_network, cast_image),
        input="\n".join(script_lines),
        text=True,
        capture_output=True,
        timeout=max(timeout, duration + 30),
        check=False,
    )
    if result.returncode:
        message = result.stderr.strip() or result.stdout.strip() or "unknown cast error"
        for transaction in transactions:
            message = message.replace(transaction["private_key"], "[redacted]")
        raise RuntimeError(f"cast workload failed: {message}")
    hashes = TX_HASH.findall(result.stdout)
    if len(hashes) != expected_count:
        raise RuntimeError(f"cast returned {len(hashes)} transaction hash(es), expected {expected_count}")
    return hashes


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
            command.extend(["--value", "1", "--gas-limit", "21000"])
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

    deadline = time.monotonic() + min(max(timeout, 30) * max(count, 1), MAX_BURST_TIMEOUT_SECONDS)
    if cast_bin:
        submitted = []
        for offset in range(count):
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError(
                    f"transaction burst exceeded the {MAX_BURST_TIMEOUT_SECONDS}-second submission timeout"
                )
            result = run_cast(
                cast_bin,
                arguments(first_nonce + offset),
                min(timeout, remaining),
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
                f"output=$({command})\nsubmitted_at=$(date +%s.%N)\n"
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
        timeout=max(0.1, deadline - time.monotonic()),
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