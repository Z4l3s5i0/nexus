import pathlib
import shutil
import subprocess
import unittest


class ShellSyntaxTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which("bash"), "bash is required")
    def test_scripts_parse(self):
        directory = pathlib.Path(__file__).parent
        for name in ("start_local.sh", "run_experiment.sh", "run_experiments_loop.sh",
                     "run_workload.sh", "collect_results.sh", "cleanup.sh"):
            with self.subTest(script=name):
                script = (directory / name).read_text(encoding="utf-8")
                result = subprocess.run(["bash", "-n"], input=script, text=True, capture_output=True)
                self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()