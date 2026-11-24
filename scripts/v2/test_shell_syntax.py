import pathlib
import shutil
import subprocess
import unittest


class ShellSyntaxTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which("bash"), "bash is required")
    def test_scripts_parse(self):
        directory = pathlib.Path(__file__).parent
        for name in ("run_experiment.sh", "analyze_experiment.sh",
                     "run_experiments_loop.sh", "run_workload.sh", "collect_results.sh",
                     "cleanup.sh", "compare_sizes.sh"):
            with self.subTest(script=name):
                script = (directory / name).read_text(encoding="utf-8").replace("\r\n", "\n")
                result = subprocess.run(["bash", "-n"], input=script.encode("utf-8"), capture_output=True)
                self.assertEqual(result.returncode, 0, result.stderr.decode("utf-8", errors="replace"))


if __name__ == "__main__":
    unittest.main()