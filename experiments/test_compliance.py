import importlib.util
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


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
            outcomes = [subprocess.CompletedProcess([], 0), subprocess.CompletedProcess([], 1)]
            with patch.object(run_suite.subprocess, "run", side_effect=outcomes) as docker_run:
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


if __name__ == "__main__":
    unittest.main()