import csv
import importlib.util
import json
import os
import pathlib
import subprocess
import tempfile
import unittest


SIZE = pathlib.Path(__file__).resolve().parent / "size"
LEGACY_SIZE_RUNNER = pathlib.Path(__file__).resolve().parents[1] / "scripts" / "v2" / "compare_sizes.sh"


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
            scc = bin_dir / "scc"
            scc.write_text(
                "#!/bin/sh\n"
                'printf \'%s\\n\' "$*" > "$SCC_TEST_ARGS"\n'
                "printf 'Language,Lines,Code,Comments,Blanks,Complexity,Bytes,Files,ULOC\\n"
                "Rust,40,30,4,6,0,500,2,0\\n"
                "Python,20,12,2,6,0,250,1,0\\n"
                "Total,60,42,6,12,0,750,3,0\\n'\n",
                encoding="utf-8",
            )
            scc.chmod(0o755)
            output = root / "source.tsv"
            args_path = root / "scc-args.txt"
            env = os.environ.copy()
            env["PATH"] = str(bin_dir) + os.pathsep + env["PATH"]
            env["CARGO_HOME"] = str(root / "cargo")
            env["SCC_TEST_ARGS"] = str(args_path)

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
            self.assertEqual(result.returncode, 0, result.stderr)
            with output.open(newline="", encoding="utf-8") as source:
                rows = list(csv.DictReader(source, delimiter="\t"))
            scc_args = args_path.read_text(encoding="utf-8")

        self.assertIn("Size progress: Measuring source-code size for nexus.", result.stderr)
        self.assertIn("Size progress: Source size measurement unavailable for missing", result.stderr)
        self.assertEqual(rows[0]["value"], "42")
        self.assertEqual(rows[0]["status"], "measured")
        self.assertEqual(rows[1]["value"], "")
        self.assertEqual(rows[1]["status"], "unavailable")
        self.assertIn("--format csv", scc_args)
        self.assertIn("--exclude-dir=", scc_args)
        self.assertIn("--not-match", scc_args)
        self.assertIn("--no-config", scc_args)
        self.assertIn("--no-scc-ignore", scc_args)
        self.assertIn("--no-gitignore", scc_args)
        self.assertIn("--no-ignore", scc_args)
        self.assertIn("--no-gen", scc_args)

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
            self.assertEqual(result.returncode, 0, result.stderr)
            with output.open(newline="", encoding="utf-8") as source:
                rows = list(csv.DictReader(source, delimiter="\t"))

        self.assertIn("Size progress: Measuring compiled binary size for present.", result.stderr)
        self.assertIn("Size progress: Compiled binary size measurement unavailable for missing", result.stderr)
        self.assertEqual(rows[0]["value"], "5")
        self.assertEqual(rows[0]["status"], "measured")
        self.assertEqual(rows[1]["status"], "unavailable")

    def test_legacy_size_runner_uses_scc_csv_counts(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            source_dir = root / "source"
            source_dir.mkdir()
            bin_dir = root / "bin"
            bin_dir.mkdir()
            scc = bin_dir / "scc"
            scc.write_text(
                "#!/bin/sh\n"
                "printf 'Language,Lines,Code,Comments,Blanks,Complexity,Bytes,Files,ULOC\\n"
                "Rust,40,30,4,6,0,500,2,0\\n"
                "Python,20,12,2,6,0,250,1,0\\n"
                "Total,60,42,6,12,0,750,3,0\\n'\n",
                encoding="utf-8",
            )
            scc.chmod(0o755)
            env = os.environ.copy()
            env["PATH"] = str(bin_dir) + os.pathsep + env["PATH"]
            env["CARGO_HOME"] = str(root / "cargo")
            result = subprocess.run(
                ["bash", str(LEGACY_SIZE_RUNNER), "--source", f"nexus={source_dir}"],
                text=True,
                capture_output=True,
                env=env,
                check=False,
            )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("source_code\tnexus\t42\tlines\t", result.stdout)

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