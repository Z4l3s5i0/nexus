import importlib.util
import io
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from unittest.mock import MagicMock, patch


COMPLIANCE = pathlib.Path(__file__).resolve().parent / "compliance"
sys.path.insert(0, str(COMPLIANCE))

import capture_images
import run_suite


def load_report_module():
    spec = importlib.util.spec_from_file_location("compliance_report", COMPLIANCE / "report.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


compliance_report = load_report_module()


class ComplianceExperimentTests(unittest.TestCase):
    def test_pinned_upstream_client_selectors_match_other_experiment_versions(self):
        self.assertEqual(run_suite.DEFAULT_CLIENTS, ("go-ethereum_v1.15.11", "reth_v2.5.2"))
        self.assertEqual(run_suite.CLIENTS[2:], ("wasix-r-eth", "wasix-w-eth"))

        for client in run_suite.DEFAULT_CLIENTS:
            command = run_suite.build_hive_command(
                client, "hive-custom:latest", "/tmp/workspace", 5, "hive-run-1"
            )
            self.assertEqual(command[command.index("--client") + 1], client)

    def test_hive_command_matches_documented_suite_and_client_flags(self):
        command = run_suite.build_hive_command(
            "wasix-r-eth", "hive-custom:latest", "/tmp/workspace", 20, "hive-run-1"
        )

        self.assertIn("/var/run/docker.sock:/var/run/docker.sock", command)
        self.assertIn("--sim", command)
        self.assertIn("ethereum/engine", command)
        self.assertIn("--client", command)
        self.assertIn("wasix-r-eth", command)
        self.assertIn("--sim.parallelism", command)
        self.assertIn("--docker.pull=false", command)

    def test_clients_run_sequentially_and_keep_individual_status(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            processes = [
                MagicMock(stdout=io.StringIO(), wait=MagicMock(return_value=0)),
                MagicMock(stdout=io.StringIO(), wait=MagicMock(return_value=1)),
            ]
            with patch.object(run_suite.subprocess, "Popen", side_effect=processes) as docker_run:
                console = io.StringIO()
                with redirect_stdout(console):
                    results = run_suite.run_clients(
                        ["wasix-r-eth", "wasix-w-eth"],
                        "hive-custom:latest",
                        root / "workspace",
                        root / "logs",
                        root / "results.jsonl",
                        "run-1",
                        5,
                        3600,
                    )
            saved = [json.loads(line) for line in (root / "results.jsonl").read_text(encoding="utf-8").splitlines()]

        self.assertEqual(docker_run.call_count, 2)
        self.assertEqual([result["status"] for result in results], ["passed", "failed"])
        self.assertEqual([result["client"] for result in saved], ["wasix-r-eth", "wasix-w-eth"])
        self.assertEqual(results[0]["simulation"], "ethereum/engine")
        self.assertIn("Compliance progress: Starting Hive ethereum/engine for wasix-r-eth (client 1 of 2).", console.getvalue())
        self.assertIn("Compliance progress: Hive ethereum/engine for wasix-w-eth finished with a failure (client 2 of 2).", console.getvalue())

    def test_hive_output_is_visible_in_console_and_client_log(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            console = io.StringIO()
            with patch.object(
                run_suite.subprocess,
                "Popen",
                return_value=MagicMock(stdout=io.StringIO("Hive progress\n"), wait=MagicMock(return_value=0)),
            ):
                with redirect_stdout(console):
                    run_suite.run_clients(
                        ["wasix-r-eth"],
                        "hive-custom:latest",
                        root / "workspace",
                        root / "logs",
                        root / "results.jsonl",
                        "run-1",
                        5,
                        3600,
                    )

            self.assertIn("Hive progress", console.getvalue())
            self.assertIn("Hive progress", (root / "logs" / "wasix-r-eth.log").read_text(encoding="utf-8"))

    def test_report_counts_client_passes_and_failures(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            (root / "metadata.json").write_text(
                json.dumps({"clients": ["wasix-r-eth", "wasix-w-eth"], "images": {"hive": {"image_id": "sha256:test"}}}),
                encoding="utf-8",
            )
            (root / "results.jsonl").write_text(
                json.dumps({"client": "wasix-r-eth", "exit_code": 0, "status": "passed", "log_path": str(root / "logs/r.log")})
                + "\n"
                + json.dumps({"client": "wasix-w-eth", "exit_code": 1, "status": "failed", "log_path": str(root / "logs/w.log")})
                + "\n",
                encoding="utf-8",
            )

            report = compliance_report.build_report(root, "run-1")

        self.assertEqual(report["passed"], 1)
        self.assertEqual(report["failed"], 1)
        self.assertEqual(report["simulation"], "ethereum/engine")

    def test_image_metadata_records_missing_images_as_unavailable(self):
        with patch.object(
            capture_images.subprocess,
            "run",
            return_value=subprocess.CompletedProcess([], 1, stdout="", stderr="missing"),
        ):
            image = capture_images.image_info("missing:image")

        self.assertFalse(image["available"])
        self.assertIsNone(image["image_id"])

    def test_image_metadata_includes_pinned_upstream_client_images(self):
        with tempfile.TemporaryDirectory() as temp:
            output_path = pathlib.Path(temp) / "metadata.json"
            with patch.object(
                sys,
                "argv",
                ["capture_images.py", "--output", str(output_path), "--clients", "go-ethereum_v1.15.11,reth_v2.5.2"],
            ):
                with patch.object(
                    capture_images.subprocess,
                    "run",
                    return_value=subprocess.CompletedProcess([], 0, stdout="sha256:test\n", stderr=""),
                ) as inspect_image:
                    capture_images.main()
            metadata = json.loads(output_path.read_text(encoding="utf-8"))

        references = [call.args[0][-1] for call in inspect_image.call_args_list]
        self.assertIn("ethereum/client-go:v1.15.11", references)
        self.assertIn("ghcr.io/paradigmxyz/reth:v2.5.2", references)
        self.assertIn("go-ethereum_v1.15.11-base", metadata["images"])
        self.assertIn("reth_v2.5.2-base", metadata["images"])


if __name__ == "__main__":
    unittest.main()