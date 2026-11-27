import json
import os
import pathlib
import shlex
import subprocess
import sys
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
NETWORK_SUPPORT = ROOT / "experiments" / "shared" / "network"


def bash_compatible_path(path):
    path = pathlib.Path(path)
    if os.name == "nt":
        return f"/mnt/{path.drive[0].lower()}/{path.as_posix()[3:]}"
    return str(path)


class NetworkSupportTests(unittest.TestCase):
    @unittest.skipIf(os.name == "nt", "Docker cleanup integration requires a Linux filesystem")
    def test_network_reset_removes_stopped_generated_data_and_preserves_running_network(self):
        reset_script = NETWORK_SUPPORT / "reset.sh"
        with tempfile.TemporaryDirectory(prefix="network-reset-test-") as temp:
            temp_root = pathlib.Path(temp)
            bin_dir = temp_root / "bin"
            bin_dir.mkdir()
            fake_docker = bin_dir / "docker"
            docker_log = temp_root / "docker.log"
            fake_docker.write_text(
                "#!/usr/bin/env bash\n"
                'printf "%s\\n" "$*" >> "$NETWORK_TEST_DOCKER_LOG"\n'
                'case "$*" in\n'
                '  "compose version") exit 0 ;;\n'
                '  *" ps -q") printf "%s" "${NETWORK_TEST_RUNNING_CONTAINERS:-}" ;;\n'
                '  *" down --volumes --remove-orphans") exit 0 ;;\n'
                '  *) exit 99 ;;\n'
                'esac\n',
                encoding="utf-8",
            )
            fake_docker.chmod(0o755)
            compose_file = temp_root / "docker-compose.v2.yml"
            compose_file.write_text("services: {}\n", encoding="utf-8")
            for name in ("startup_v2", "data_v2"):
                (temp_root / name).mkdir()
                (temp_root / name / "generated.txt").write_text("generated\n", encoding="utf-8")

            def reset(active_containers=""):
                assignments = {
                    "EXPERIMENTS_NETWORK_REPO_ROOT": bash_compatible_path(temp_root),
                    "NETWORK_TEST_DOCKER_LOG": bash_compatible_path(docker_log),
                    "DOCKER_BIN": bash_compatible_path(fake_docker),
                }
                if active_containers:
                    assignments["NETWORK_TEST_RUNNING_CONTAINERS"] = active_containers
                command = " ".join(
                    f"{name}={shlex.quote(value)}" for name, value in assignments.items()
                )
                command += f" exec bash {shlex.quote(bash_compatible_path(reset_script))}"
                return subprocess.run(
                    ["bash", "-c", command], text=True, capture_output=True, check=False
                )

            result = reset()

            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse((temp_root / "startup_v2").exists())
            self.assertFalse((temp_root / "data_v2").exists())
            self.assertIn("down --volumes --remove-orphans", docker_log.read_text(encoding="utf-8"))

            for name in ("startup_v2", "data_v2"):
                (temp_root / name).mkdir()
                (temp_root / name / "generated.txt").write_text("generated\n", encoding="utf-8")
            result = reset("active-container")

            self.assertNotEqual(result.returncode, 0)
            self.assertIn("already running", result.stderr)
            self.assertTrue((temp_root / "startup_v2" / "generated.txt").is_file())
            self.assertTrue((temp_root / "data_v2" / "generated.txt").is_file())

    def test_experiment_network_modules_use_relocated_launcher(self):
        for module in ("runtime", "empty-block"):
            script = (ROOT / "experiments" / module / "network.sh").read_text(encoding="utf-8")
            with self.subTest(module=module):
                self.assertIn("experiments/shared/network/launch.sh", script)
                self.assertNotIn("scripts/v2", script)
        empty_block_script = (ROOT / "experiments" / "empty-block" / "network.sh").read_text(encoding="utf-8")
        self.assertIn('GENESIS_DELAY=0 bash "$NETWORK_START"', empty_block_script)

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

    def test_log_capture_scripts_include_consensus_and_validator_services(self):
        runtime_capture = (ROOT / "experiments" / "runtime" / "capture_logs.sh").read_text(encoding="utf-8")
        empty_block_capture = (ROOT / "experiments" / "empty-block" / "capture_logs.sh").read_text(encoding="utf-8")

        self.assertIn("cl-node-$index", runtime_capture)
        self.assertIn("vc-node-$index", runtime_capture)
        self.assertIn("cl-node-0", empty_block_capture)
        self.assertIn("vc-node-0", empty_block_capture)

    @unittest.skipIf(os.name == "nt", "Docker log capture integration requires Bash")
    def test_experiment_log_capture_includes_consensus_and_available_validator_clients(self):
        with tempfile.TemporaryDirectory(prefix="capture-client-logs-test-") as temp:
            temp_root = pathlib.Path(temp)
            bin_dir = temp_root / "bin"
            bin_dir.mkdir()
            fake_docker = bin_dir / "docker"
            fake_docker.write_text(
                "#!/usr/bin/env bash\n"
                'if [[ "$1" == "compose" && "$2" == "version" ]]; then exit 0; fi\n'
                'if [[ "$1" == "compose" && "$*" == *" ps "* ]]; then\n'
                '  [[ "$*" == *" --all --quiet "* ]] || exit 98\n'
                '  service="${!#}"\n'
                '  case "$service" in\n'
                '    el-node-0) printf "container-el-0\\n" ;;\n'
                '    el-node-1) printf "container-el-1\\n" ;;\n'
                '    cl-node-0) printf "container-cl-0\\n" ;;\n'
                '    cl-node-1) printf "container-cl-1\\n" ;;\n'
                '    vc-node-0) printf "container-vc-0\\n" ;;\n'
                '  esac\n'
                '  exit 0\n'
                'fi\n'
                'if [[ "$1" == "logs" ]]; then printf "captured:%s\\n" "${!#}"; exit 0; fi\n'
                'exit 99\n',
                encoding="utf-8",
            )
            fake_docker.chmod(0o755)

            env = os.environ.copy()
            env["PATH"] = str(bin_dir) + os.pathsep + env.get("PATH", "")
            capture_scripts = (
                ("runtime", ROOT / "experiments" / "runtime" / "capture_logs.sh", ["--output-dir", "{output}", "--nodes", "2"]),
                ("empty-block", ROOT / "experiments" / "empty-block" / "capture_logs.sh", ["{output}"]),
            )
            for module, script, arguments in capture_scripts:
                with self.subTest(module=module):
                    output_dir = temp_root / module
                    normalized_script = temp_root / f"{module}-capture_logs.sh"
                    normalized_script.write_text(script.read_text(encoding="utf-8"), encoding="utf-8")
                    command = [
                        "bash",
                        str(normalized_script),
                        *(str(output_dir) if argument == "{output}" else argument for argument in arguments),
                    ]
                    result = subprocess.run(command, env=env, text=True, capture_output=True, check=False)

                    self.assertEqual(result.returncode, 0, result.stderr)
                    expected_services = ["el-node-0", "cl-node-0", "vc-node-0"]
                    if module == "runtime":
                        expected_services.extend(["el-node-1", "cl-node-1"])
                    for service in expected_services:
                        self.assertEqual(
                            (output_dir / f"{service}.log").read_text(encoding="utf-8").strip(),
                            f"captured:container-{service.replace('-node-', '-')}",
                        )
                    self.assertFalse((output_dir / "vc-node-1.log").exists())

    def test_config_generator_mounts_canonical_output_directory(self):
        script = (NETWORK_SUPPORT / "config_gen.sh").read_text(encoding="utf-8")

        self.assertIn('OUTPUT_DIR="$(cd "$OUTPUT_DIR" && pwd)"', script)
        self.assertIn('-v "$OUTPUT_DIR/values.env:/config/values.env"', script)
        self.assertIn('-v "$OUTPUT_DIR:/data"', script)
        self.assertIn("GENESIS_GASLIMIT=650000000", script)
        self.assertNotIn('$(pwd)/$OUTPUT_DIR', script)

    def test_prometheus_scrapes_host_and_labeled_node_container_metrics(self):
        compose = (NETWORK_SUPPORT / "templates" / "docker-compose.yml.template").read_text(encoding="utf-8")
        prometheus = (NETWORK_SUPPORT / "monitoring" / "prometheus.yml").read_text(encoding="utf-8")
        launcher = (NETWORK_SUPPORT / "start.sh").read_text(encoding="utf-8")

        self.assertIn("ghcr.io/google/cadvisor:0.61.0", compose)
        self.assertIn("--store_container_labels=false", compose)
        self.assertIn(
            "--whitelisted_container_labels="
            "experiment_node_index,experiment_client,experiment_runtime_variant,experiment_node_service",
            compose,
        )
        self.assertIn("node-exporter-targets.json:/etc/prometheus/node-exporter-targets.json:ro", compose)
        self.assertIn("execution-node-targets.json:/etc/prometheus/execution-node-targets.json:ro", compose)
        for label in (
            "experiment_node_index",
            "experiment_client",
            "experiment_runtime_variant",
            "experiment_node_service",
        ):
            self.assertIn(f"{label}:", launcher)
        self.assertIn("generate_prometheus_targets.py", launcher)
        self.assertIn("--execution-metrics-output", launcher)
        self.assertIn("up -d metrics-server prometheus grafana node-exporter cadvisor", launcher)
        self.assertIn("job_name: 'node-exporter-host'", prometheus)
        self.assertIn("scope: host", prometheus)
        self.assertIn("job_name: 'node-exporter-containers'", prometheus)
        self.assertIn("job_name: 'execution-node-metrics'", prometheus)
        self.assertIn("job_name: 'cadvisor'", prometheus)
        for label in ("node_index", "client", "runtime_variant", "node_service", "container_name"):
            self.assertIn(f"target_label: {label}", prometheus)
        for label in (
            "container_label_experiment_node_index",
            "container_label_experiment_client",
            "container_label_experiment_runtime_variant",
            "container_label_experiment_node_service",
        ):
            self.assertIn(f"source_labels: [{label}]", prometheus)
        self.assertIn("scope: host", prometheus)
        self.assertIn("replacement: container", prometheus)

    def test_node_exporter_disables_systemd_collector_in_node_containers(self):
        launcher = (NETWORK_SUPPORT / "start.sh").read_text(encoding="utf-8")

        self.assertEqual(launcher.count("prometheus-node-exporter --no-collector.systemd"), 2)

    def test_reth_uses_supported_tcp_and_discovery_ports(self):
        launcher = (NETWORK_SUPPORT / "start.sh").read_text(encoding="utf-8")
        reth_config = launcher.split('elif [ "$TYPE" = "reth" ]; then', 1)[1].split(
            'elif [ "$TYPE" = "wasix" ]; then', 1
        )[0]

        self.assertIn("- --port=$P2P_PORT", reth_config)
        self.assertIn("- --discovery.port=$DISC_PORT", reth_config)
        self.assertNotIn("--discovery.quic-port", reth_config)
        self.assertIn("$P2P_PORT:$P2P_PORT/tcp", reth_config)
        self.assertIn("$DISC_PORT:$DISC_PORT/udp", reth_config)
        self.assertIn("local discovery_port=$4", launcher)
        self.assertIn("?discport=$discovery_port", launcher)
        self.assertIn(
            'register_reth_node "$i" "$ETH_PORT" "$P2P_PORT" "$DISC_PORT" "$METRICS_PORT"',
            launcher,
        )

    def test_geth_rpc_gas_and_transaction_fee_caps_are_disabled(self):
        launcher = (NETWORK_SUPPORT / "start.sh").read_text(encoding="utf-8")
        geth_config = launcher.split('if [ "$TYPE" = "geth" ]; then', 1)[1].split(
            'elif [ "$TYPE" = "reth" ]; then', 1
        )[0]

        self.assertIn("- --rpc.gascap=0", geth_config)
        self.assertIn("- --rpc.txfeecap=0", geth_config)
        self.assertIn(r'GETH_RPC_GASCAP: \"0\"', geth_config)
        self.assertIn(r'GETH_RPC_TXFEECAP: \"0\"', geth_config)

    def test_reth_registration_exposes_admin_rpc_and_reads_node_id(self):
        launcher = (NETWORK_SUPPORT / "start.sh").read_text(encoding="utf-8")
        reth_config = launcher.split('elif [ "$TYPE" = "reth" ]; then', 1)[1].split(
            'elif [ "$TYPE" = "wasix" ]; then', 1
        )[0]

        self.assertIn("- --http.api=admin,eth,net,web3", reth_config)
        self.assertIn('d.get("id"', launcher)

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
                    "GENESIS_DELAY": "0",
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
            self.assertIn("GENESIS_DELAY=0", (output_dir / "values.env").read_text(encoding="utf-8"))
            self.assertIn("GENESIS_GASLIMIT=650000000", (output_dir / "values.env").read_text(encoding="utf-8"))
            self.assertTrue((output_dir / "validators" / "node_0" / "keystore-m_12381_3600_0-0.json").is_file())
            docker_calls = docker_log.read_text(encoding="utf-8")
            self.assertIn(f"{output_dir}/values.env:/config/values.env", docker_calls)
            self.assertIn(f"{output_dir}:/data", docker_calls)


if __name__ == "__main__":
    unittest.main()