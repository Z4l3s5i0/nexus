import json
import os
import pathlib
import subprocess
import sys
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

    def test_prometheus_scrapes_host_and_labeled_node_container_metrics(self):
        compose = (NETWORK_SUPPORT / "templates" / "docker-compose.yml.template").read_text(encoding="utf-8")
        prometheus = (NETWORK_SUPPORT / "monitoring" / "prometheus.yml").read_text(encoding="utf-8")
        launcher = (NETWORK_SUPPORT / "start.sh").read_text(encoding="utf-8")

        self.assertIn("gcr.io/cadvisor/cadvisor:v0.61.0", compose)
        self.assertIn("--store_container_labels=false", compose)
        self.assertIn("--whitelisted_container_labels=experiment_node_index,experiment_client", compose)
        self.assertIn("node-exporter-targets.json:/etc/prometheus/node-exporter-targets.json:ro", compose)
        self.assertIn("execution-node-targets.json:/etc/prometheus/execution-node-targets.json:ro", compose)
        self.assertIn("experiment_node_index:", launcher)
        self.assertIn("generate_prometheus_targets.py", launcher)
        self.assertIn("--execution-metrics-output", launcher)
        self.assertIn("up -d metrics-server prometheus grafana node-exporter cadvisor", launcher)
        self.assertIn("job_name: 'node-exporter-host'", prometheus)
        self.assertIn("scope: host", prometheus)
        self.assertIn("job_name: 'node-exporter-containers'", prometheus)
        self.assertIn("job_name: 'execution-node-metrics'", prometheus)
        self.assertIn("job_name: 'cadvisor'", prometheus)
        self.assertIn("target_label: node_index", prometheus)
        self.assertIn("target_label: client", prometheus)
        self.assertIn("replacement: container", prometheus)

    def test_target_generator_labels_per_node_exporter_and_execution_metrics(self):
        with tempfile.TemporaryDirectory(prefix="node-exporter-targets-test-") as temp:
            node_exporter_output = pathlib.Path(temp) / "node-exporter-targets.json"
            execution_metrics_output = pathlib.Path(temp) / "execution-node-targets.json"
            result = subprocess.run(
                [
                    sys.executable,
                    str(NETWORK_SUPPORT / "generate_prometheus_targets.py"),
                    "--node-exporter-output",
                    str(node_exporter_output),
                    "--execution-metrics-output",
                    str(execution_metrics_output),
                    "--node-types",
                    "linux",
                    "geth",
                    "reth",
                    "wasix",
                ],
                text=True,
                capture_output=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            exporter_targets = json.loads(node_exporter_output.read_text(encoding="utf-8"))
            execution_targets = json.loads(execution_metrics_output.read_text(encoding="utf-8"))

        self.assertEqual(
            [item["targets"][0] for item in exporter_targets],
            ["el-node-0:9100", "el-node-3:9100"],
        )
        self.assertEqual(exporter_targets[0]["labels"]["client"], "wasix-eth")
        self.assertEqual(exporter_targets[0]["labels"]["runtime_variant"], "native")
        self.assertEqual(exporter_targets[0]["labels"]["node_index"], "0")
        self.assertEqual(exporter_targets[0]["labels"]["scope"], "node_container_exporter")
        self.assertEqual(exporter_targets[1]["labels"]["runtime_variant"], "wasix")
        self.assertEqual(exporter_targets[1]["labels"]["node_service"], "el-node-3")
        self.assertEqual(
            [item["targets"][0] for item in execution_targets],
            ["el-node-0:9050", "el-node-1:9051", "el-node-2:9052", "el-node-3:9053"],
        )
        self.assertEqual(execution_targets[1]["labels"]["client"], "geth")
        self.assertEqual(execution_targets[2]["labels"]["client"], "reth")
        self.assertEqual(execution_targets[3]["labels"]["scope"], "execution_node")

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