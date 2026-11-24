import io
import os
import pathlib
import shutil
import subprocess
import tarfile
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
    "sha256sum",
    "docker",
    "lld",
    "sudo",
    "rustup",
    "cargo",
)


@unittest.skipUnless(shutil.which("bash"), "bash is required")
class ExperimentSetupTests(unittest.TestCase):
    def run_setup(
        self,
        selection,
        create_wasix_binary=True,
        preinstall_wasixcc=False,
        missing_images=(),
    ):
        with tempfile.TemporaryDirectory(prefix="experiment-setup-test-") as temp:
            temp_root = pathlib.Path(temp)
            bin_dir = temp_root / "bin"
            bin_dir.mkdir()
            log_path = temp_root / "commands.log"
            source_root = temp_root / "wasix_eth"
            cargo_home = temp_root / "cargo"
            wasmer_dir = temp_root / "wasmer"
            scc_archive = temp_root / "scc-release.tar.gz"
            scc_script = b"#!/bin/sh\nprintf 'scc version 4.1.0\\n'\n"
            with tarfile.open(scc_archive, "w:gz") as archive:
                binary = tarfile.TarInfo("scc")
                binary.mode = 0o755
                binary.size = len(scc_script)
                archive.addfile(binary, io.BytesIO(scc_script))
            native_binary = source_root / "target" / "release" / "wasix_eth"
            wasix_binary = source_root / "target" / "wasm32-wasmer-wasi" / "release" / "wasix_eth.wasi.wasm"
            native_binary.parent.mkdir(parents=True)
            wasix_binary.parent.mkdir(parents=True)
            native_binary.write_bytes(b"native-binary")
            if create_wasix_binary:
                wasix_binary.write_bytes(b"wasix-binary")
            if preinstall_wasixcc:
                wasixcc = bin_dir / "wasixcc"
                wasixcc.write_text("#!/bin/sh\nprintf 'wasixcc 0.4.7\\n'\n", encoding="utf-8")
                wasixcc.chmod(0o755)
                wasixccenv = bin_dir / "wasixccenv"
                wasixccenv.write_text(
                    '#!/bin/sh\nprintf "%s %s\\n" "$(basename "$0")" "$*" >> "$SETUP_TEST_LOG"\n',
                    encoding="utf-8",
                )
                wasixccenv.chmod(0o755)
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
                        'if [ "$1" = "+stable" ] && [ "$2" = "install" ] && [ "$3" = "wasixcc" ]; then\n'
                        '  mkdir -p "$CARGO_HOME/bin"\n'
                        '  printf "%s\\n" "#!/bin/sh" "printf \\"wasixcc 0.4.7\\\\n\\"" > "$CARGO_HOME/bin/wasixcc"\n'
                        '  printf "%s\\n" "#!/bin/sh" "printf \\"wasixccenv %s\\\\n\\" \\"\\$*\\" >> \\"\\$SETUP_TEST_LOG\\"" > "$CARGO_HOME/bin/wasixccenv"\n'
                        '  chmod +x "$CARGO_HOME/bin/wasixcc" "$CARGO_HOME/bin/wasixccenv"\n'
                        'fi\n'
                        'if [ "$1" = "+stable" ] && [ "$2" = "wasix" ] && [ "$3" = "build" ]; then\n'
                        '  mkdir -p "$EXPERIMENTS_WASIX_ETH_ROOT/target/wasm32-wasmer-wasi/release"\n'
                        '  printf "wasix-binary" > "$EXPERIMENTS_WASIX_ETH_ROOT/target/wasm32-wasmer-wasi/release/wasix_eth.wasi.wasm"\n'
                        'fi\n'
                    )
                elif name == "curl":
                    stub += (
                        'if [ "$2" = "https://get.wasmer.io" ]; then\n'
                        '  mkdir -p "$WASMER_DIR/bin"\n'
                        '  printf "#!/bin/sh\\nprintf \\\"wasmer 7.4.2\\\\n\\\"\\n" > "$WASMER_DIR/bin/wasmer"\n'
                        '  chmod +x "$WASMER_DIR/bin/wasmer"\n'
                        'fi\n'
                        'if [ "$6" = "https://github.com/boyter/scc/releases/download/v4.1.0/scc_Linux_x86_64.tar.gz" ] || [ "$6" = "https://github.com/boyter/scc/releases/download/v4.1.0/scc_Linux_arm64.tar.gz" ]; then\n'
                        '  cp "$SCC_TEST_ARCHIVE" "$5"\n'
                        'fi\n'
                    )
                elif name == "sha256sum":
                    stub += 'exit 0\n'
                elif name == "docker":
                    stub += (
                        'if [ "$1" = "image" ] && [ "$2" = "inspect" ]; then\n'
                        '  case ",$SETUP_TEST_MISSING_IMAGES," in *,"$3",*) exit 1 ;; esac\n'
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
                    "SCC_TEST_ARCHIVE": str(scc_archive),
                    "EXPERIMENTS_WASIX_ETH_ROOT": str(source_root),
                    "SETUP_TEST_MISSING_IMAGES": ",".join(missing_images),
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
        self.assertTrue(any("boyter/scc/releases/download/v4.1.0/scc_Linux_" in call for call in calls))
        self.assertIn("sha256sum --check --status checksum", calls)

    def test_wasixcc_and_lld_are_ready_before_wasix_build(self):
        result, calls = self.run_setup("size", create_wasix_binary=False)

        self.assertEqual(result.returncode, 0, result.stderr)
        lld_install = next(
            call for call in calls if "apt-get install -y" in call and call.split()[-1] == "lld"
        )
        wasixcc_install = "cargo +stable install wasixcc --version 0.4.7 --locked --force"
        download_all = "wasixccenv download-all"
        wasix_build = "cargo +stable wasix build --release"

        self.assertIn(wasixcc_install, calls)
        self.assertTrue(any(call.startswith("wasixccenv install-executables ") for call in calls))
        self.assertIn(download_all, calls)
        self.assertLess(calls.index(lld_install), calls.index(wasix_build))
        self.assertLess(calls.index(wasixcc_install), calls.index(wasix_build))
        self.assertLess(calls.index(download_all), calls.index(wasix_build))

    def test_existing_wasixcc_version_is_verified_without_reinstalling(self):
        result, calls = self.run_setup("size", preinstall_wasixcc=True)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("cargo +stable install wasixcc --version 0.4.7 --locked --force", calls)
        self.assertIn("wasixccenv download-all", calls)

    def test_runtime_setup_checks_docker_and_prepares_images(self):
        result, calls = self.run_setup("runtime")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("docker compose version", calls)
        self.assertIn("docker info", calls)
        self.assertFalse(any("boyter/scc/releases" in call for call in calls))
        pulls = [call for call in calls if call.startswith("docker pull ")]
        self.assertIn(
            "docker pull ethereum/client-go@sha256:798b7eb1bcef6d4be7576232beea63bf291450f48b025dc8fb5c6e37840e4364",
            pulls,
        )
        self.assertIn("docker pull ghcr.io/paradigmxyz/reth:latest", pulls)
        self.assertIn("docker pull ghcr.io/google/cadvisor:0.61.0", pulls)

    def test_startup_setup_builds_missing_nexus_images_locally(self):
        result, calls = self.run_setup(
            "startup",
            create_wasix_binary=False,
            missing_images=("wasix-eth-linux:latest", "wasix-eth-wasix:latest"),
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(any(call.startswith("docker build -t wasix-eth-metrics:latest") for call in calls))
        native_build = next(call for call in calls if call.startswith("docker build -t wasix-eth-linux:latest"))
        wasix_build = next(call for call in calls if call.startswith("docker build -t wasix-eth-wasix:latest"))
        self.assertIn("-f ", native_build)
        self.assertIn("Dockerfile.linux", native_build)
        self.assertIn("Dockerfile.wasix", wasix_build)
        self.assertLess(calls.index("cargo +stable wasix build --release"), calls.index(wasix_build))
        self.assertFalse(any(call.startswith("docker pull docker.io/z4l3s5i0/wasix-eth-") for call in calls))

    def test_startup_setup_reuses_existing_local_nexus_images(self):
        result, calls = self.run_setup("startup")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any(call.startswith("docker build -t wasix-eth-linux:latest") for call in calls))
        self.assertFalse(any(call.startswith("docker build -t wasix-eth-wasix:latest") for call in calls))
        self.assertFalse(any(call.startswith("docker pull docker.io/z4l3s5i0/wasix-eth-") for call in calls))


if __name__ == "__main__":
    unittest.main()