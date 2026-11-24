import re
import shlex
import shutil
import subprocess


TX_HASH = re.compile(r"0x[0-9a-fA-F]{64}")


def transaction_hash(output):
    hashes = TX_HASH.findall(output)
    if not hashes:
        raise RuntimeError("cast did not return a transaction hash")
    return hashes[-1]


def run_cast(cast_bin, arguments, timeout, cast_network, cast_image):
    if cast_bin:
        command = [cast_bin, *arguments]
    else:
        if not shutil.which("docker"):
            raise RuntimeError("cast is unavailable and Docker is not installed")
        if not cast_network:
            raise RuntimeError("a Docker network is required to run the bundled cast client")
        command = [
            "docker",
            "run",
            "--rm",
            "--network",
            cast_network,
            cast_image,
            shlex.join(["cast", *arguments]),
        ]
    result = subprocess.run(command, text=True, capture_output=True, timeout=timeout, check=False)
    if result.returncode:
        message = result.stderr.strip() or result.stdout.strip() or "unknown cast error"
        for index, value in enumerate(arguments[:-1]):
            if value in ("--private-key", "--pk"):
                message = message.replace(arguments[index + 1], "[redacted]")
        raise RuntimeError(f"cast command failed: {message}")
    return result.stdout.strip()


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
):
    def arguments(nonce):
        return [
                "send",
                sender,
                "--value",
                "0",
                "--gas-limit",
                "21000",
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

    if cast_bin:
        return [
            transaction_hash(
                run_cast(cast_bin, arguments(first_nonce + offset), timeout, cast_network, cast_image)
            )
            for offset in range(count)
        ]

    if not shutil.which("docker"):
        raise RuntimeError("cast is unavailable and Docker is not installed")
    if not cast_network:
        raise RuntimeError("a Docker network is required to run the bundled cast client")
    script = "set -e\n" + "\n".join(shlex.join(["cast", *arguments(first_nonce + offset)]) for offset in range(count))
    result = subprocess.run(
        ["docker", "run", "--rm", "--network", cast_network, cast_image, script],
        text=True,
        capture_output=True,
        timeout=max(timeout, 30) * max(count, 1),
        check=False,
    )
    if result.returncode:
        message = result.stderr.strip() or result.stdout.strip() or "unknown cast error"
        message = message.replace(private_key, "[redacted]")
        raise RuntimeError(f"cast burst failed: {message}")
    hashes = TX_HASH.findall(result.stdout)
    if len(hashes) != count:
        raise RuntimeError(f"cast returned {len(hashes)} transaction hash(es), expected {count}")
    return hashes