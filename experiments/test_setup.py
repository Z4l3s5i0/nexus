import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
SETUP = ROOT / "experiments" / "setup.sh"
STUB_COMMANDS = (
    "apt-get",
    "curl",
    "wget",
    "git",
    "jq",
    "openssl",
    "python3",
    "cloc",
    "docker",
    "sudo",
    "rustup",
    "cargo",
)


@unittest.skipUnless(shutil.which("bash"), "bash is required")
class ExperimentSetupTests(unittest.TestCase):
    def run_setup(self, selection):
        with tempfile.TemporaryDirectory(prefix="experiment-setup-test-") as temp:
            temp_root = pathlib.Path(temp)
            bin_dir = temp_root / "bin"
            bin_dir.mkdir()
            log_path = temp_root / "commands.log"
            source_root = temp_root / "wasix_eth"
            cargo_home = temp_root / "cargo"
            wasmer_dir = temp_root / "wasmer"
            native_binary = source_root / "target" / "release" / "wasix_eth"
            wasix_binary = source_root / "target" / "wasm32-wasmer-wasi" / "release" / "wasix_eth.wasi.wasm"
            native_binary.parent.mkdir(parents=True)
            wasix_binary.parent.mkdir(parents=True)
            native_binary.write_bytes(b"native-binary")
            wasix_binary.write_bytes(b"wasix-binary")
            for name in STUB_COMMANDS:
                command = bin_dir / name
                stub = (
                    '#!/bin/sh\n'
                    'printf "%s %s\\n" "$(basename "$0")" "$*" >> "$SETUP_TEST_LOG"\n'
                )
                if name == "cargo":
                    stub += (
                        'if [ "$1" = "+stable" ] && [ "$2" = "install" ] && [ "$3" = "cargo-wasix" ]; then\n'
                        '  mkdir -p "$CARGO_HOME/bin"\n'
                        '  printf "#!/bin/sh\\nexit 0\\n" > "$CARGO_HOME/bin/cargo-wasix"\n'
                        '  chmod +x "$CARGO_HOME/bin/cargo-wasix"\n'
                        'fi\n'
                    )
                elif name == "curl":
                    stub += (
                        'if [ "$2" = "https://get.wasmer.io" ]; then\n'
                        '  mkdir -p "$WASMER_DIR/bin"\n'
                        '  printf "#!/bin/sh\\nprintf \\\"wasmer 7.4.2\\\\n\\\"\\n" > "$WASMER_DIR/bin/wasmer"\n'
                        '  chmod +x "$WASMER_DIR/bin/wasmer"\n'
                        'fi\n'
                    )
                stub += 'exit 0\n'
                command.write_text(stub, encoding="utf-8")
                command.chmod(0o755)

            env = os.environ.copy()
            env.update(
                {
                    "EXPERIMENTS_SELECTED": selection,
                    "PATH": str(bin_dir) + os.pathsep + "/usr/bin:/bin",
                    "SETUP_TEST_LOG": str(log_path),
                    "EXPERIMENTS_WASIX_ETH_ROOT": str(source_root),
                    "CARGO_HOME": str(cargo_home),
                    "WASMER_DIR": str(wasmer_dir),
                }
            )
            result = subprocess.run(
                ["bash", str(SETUP)], text=True, capture_output=True, env=env, check=False
            )
            calls = log_path.read_text(encoding="utf-8").splitlines()
            return result, calls

    def test_size_only_setup_skips_docker_and_image_pulls(self):
        result, calls = self.run_setup("size")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Skipping Docker installation and image preparation", result.stdout)
        self.assertFalse(any(call.startswith("docker ") for call in calls))
        self.assertIn("rustup toolchain install stable --profile minimal", calls)
        self.assertIn("cargo +stable install cargo-wasix", calls)
        self.assertIn("curl -fsSL https://get.wasmer.io", calls)

    def test_runtime_setup_checks_docker_and_prepares_images(self):
        result, calls = self.run_setup("runtime")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("docker compose version", calls)
        self.assertIn("docker info", calls)
        pulls = [call for call in calls if call.startswith("docker pull ")]
        self.assertIn(
            "docker pull ethereum/client-go@sha256:798b7eb1bcef6d4be7576232beea63bf291450f48b025dc8fb5c6e37840e4364",
            pulls,
        )
        self.assertIn("docker pull ghcr.io/paradigmxyz/reth:latest", pulls)

    def test_startup_setup_prepares_the_metric_service_and_client_images(self):
        result, calls = self.run_setup("startup")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(any(call.startswith("docker build -t wasix-eth-metrics:latest") for call in calls))
        self.assertIn("docker pull docker.io/z4l3s5i0/wasix-eth-linux:clean", calls)
        self.assertIn("docker pull docker.io/z4l3s5i0/wasix-eth-wasix:clean", calls)


if __name__ == "__main__":
    unittest.main()