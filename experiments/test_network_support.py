import os
import pathlib
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
NETWORK_SUPPORT = ROOT / "experiments" / "shared" / "network"


class NetworkSupportTests(unittest.TestCase):
    def test_experiment_network_modules_use_relocated_launcher(self):
        for module in ("runtime", "empty-block"):
            script = (ROOT / "experiments" / module / "network.sh").read_text(encoding="utf-8")
            with self.subTest(module=module):
                self.assertIn("experiments/shared/network/launch.sh", script)
                self.assertNotIn("scripts/v2", script)

    def test_network_launcher_help_resolves_assets_outside_repo_cwd(self):
        with tempfile.TemporaryDirectory(prefix="network-launcher-test-") as temp:
            result = subprocess.run(
                ["bash", str(NETWORK_SUPPORT / "launch.sh"), "--help"],
                cwd=temp,
                text=True,
                capture_output=True,
                check=False,
            )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Usage:", result.stdout)

    def test_config_generator_mounts_canonical_output_directory(self):
        script = (NETWORK_SUPPORT / "config_gen.sh").read_text(encoding="utf-8")

        self.assertIn('OUTPUT_DIR="$(cd "$OUTPUT_DIR" && pwd)"', script)
        self.assertIn('-v "$OUTPUT_DIR/values.env:/config/values.env"', script)
        self.assertIn('-v "$OUTPUT_DIR:/data"', script)
        self.assertNotIn('$(pwd)/$OUTPUT_DIR', script)

    def test_config_generator_writes_and_mounts_output_outside_repository_cwd(self):
        with tempfile.TemporaryDirectory(prefix="network-config-test-") as temp:
            temp_root = pathlib.Path(temp)
            bin_dir = temp_root / "bin"
            bin_dir.mkdir()
            output_dir = temp_root / "generated-config"
            docker_log = temp_root / "docker.log"
            docker = bin_dir / "docker"
            docker.write_text(
                "#!/usr/bin/env bash\n"
                'printf "%s\\n" "$*" >> "$NETWORK_TEST_DOCKER_LOG"\n'
                'if [[ "$*" == *"ethstaker-deposit-cli"* ]]; then\n'
                '  mount=""\n'
                '  while [[ "$#" -gt 0 ]]; do\n'
                '    if [[ "$1" == "-v" ]]; then mount="${2%:/data}"; shift 2; else shift; fi\n'
                '  done\n'
                '  mkdir -p "$mount/validator_keys"\n'
                '  printf "{}\\n" > "$mount/validator_keys/keystore-m_12381_3600_0-0.json"\n'
                'fi\n',
                encoding="utf-8",
            )
            docker.chmod(0o755)
            openssl = bin_dir / "openssl"
            openssl.write_text("#!/bin/sh\nprintf '%064d' 0\n", encoding="utf-8")
            openssl.chmod(0o755)

            env = os.environ.copy()
            env.update(
                {
                    "EXPERIMENTS_NETWORK_SUPPORT_DIR": str(NETWORK_SUPPORT),
                    "NETWORK_TEST_DOCKER_LOG": str(docker_log),
                    "OUTPUT_DIR": str(output_dir),
                    "PATH": str(bin_dir) + os.pathsep + "/usr/bin:/bin",
                }
            )
            runner = (
                'bash <(tr -d "\\r" < "$EXPERIMENTS_NETWORK_SUPPORT_DIR/config_gen.sh") '
                '--chain-id 9876 --nodes 1 --output "$OUTPUT_DIR"'
            )
            result = subprocess.run(
                ["bash", "-c", runner],
                cwd=temp_root,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("CHAIN_ID=9876", (output_dir / "values.env").read_text(encoding="utf-8"))
            self.assertTrue((output_dir / "validators" / "node_0" / "keystore-m_12381_3600_0-0.json").is_file())
            docker_calls = docker_log.read_text(encoding="utf-8")
            self.assertIn(f"{output_dir}/values.env:/config/values.env", docker_calls)
            self.assertIn(f"{output_dir}:/data", docker_calls)


if __name__ == "__main__":
    unittest.main()