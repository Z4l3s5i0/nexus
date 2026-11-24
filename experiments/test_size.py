import csv
import importlib.util
import json
import os
import pathlib
import subprocess
import tempfile
import unittest


SIZE = pathlib.Path(__file__).resolve().parent / "size"


def load_report_module():
    spec = importlib.util.spec_from_file_location("size_report", SIZE / "report.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


size_report = load_report_module()


class SizeExperimentTests(unittest.TestCase):
    def test_source_measurement_reports_missing_sources_as_unavailable(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            source_dir = root / "source"
            source_dir.mkdir()
            bin_dir = root / "bin"
            bin_dir.mkdir()
            cloc = bin_dir / "cloc"
            cloc.write_text(
                '#!/bin/sh\nprintf \'{"SUM":{"code":42}}\'\n', encoding="utf-8"
            )
            cloc.chmod(0o755)
            output = root / "source.tsv"
            env = os.environ.copy()
            env["PATH"] = str(bin_dir) + os.pathsep + env["PATH"]

            result = subprocess.run(
                [
                    "bash", str(SIZE / "measure_source_size.sh"),
                    "--source", f"nexus={source_dir}",
                    "--source", f"missing={root / 'missing'}",
                    "--output", str(output),
                ],
                text=True,
                capture_output=True,
                env=env,
                check=False,
            )
            with output.open(newline="", encoding="utf-8") as source:
                rows = list(csv.DictReader(source, delimiter="\t"))

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(rows[0]["value"], "42")
        self.assertEqual(rows[0]["status"], "measured")
        self.assertEqual(rows[1]["value"], "")
        self.assertEqual(rows[1]["status"], "unavailable")

    def test_binary_measurement_keeps_bytes_and_missing_binary_distinct(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            binary = root / "client.bin"
            binary.write_bytes(b"abcde")
            output = root / "binary.tsv"
            result = subprocess.run(
                [
                    "bash", str(SIZE / "measure_binary_size.sh"),
                    "--binary", f"present={binary}",
                    "--binary", f"missing={root / 'missing.bin'}",
                    "--output", str(output),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            with output.open(newline="", encoding="utf-8") as source:
                rows = list(csv.DictReader(source, delimiter="\t"))

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(rows[0]["value"], "5")
        self.assertEqual(rows[0]["status"], "measured")
        self.assertEqual(rows[1]["status"], "unavailable")

    def test_report_preserves_source_and_binary_availability(self):
        with tempfile.TemporaryDirectory() as temp:
            output_dir = pathlib.Path(temp)
            (output_dir / "source_size.tsv").write_text(
                "measurement\tlabel\tvalue\tunit\tsource\tstatus\nsource_code\tnexus\t42\tlines\twasix_eth\tmeasured\n",
                encoding="utf-8",
            )
            (output_dir / "binary_size.tsv").write_text(
                "measurement\tlabel\tvalue\tunit\tsource\tstatus\ncompiled_binary\twasm\t\tbytes\tmissing.wasm\tunavailable\n",
                encoding="utf-8",
            )

            report = size_report.build_report(output_dir, "run-1")

        self.assertEqual(report["source_code"][0]["value"], "42")
        self.assertEqual(report["compiled_binaries"][0]["status"], "unavailable")


if __name__ == "__main__":
    unittest.main()