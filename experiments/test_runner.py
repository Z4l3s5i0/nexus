import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
RUNNER = ROOT / "experiments" / "run.sh"


@unittest.skipUnless(shutil.which("bash"), "bash is required")
class ExperimentRunnerTests(unittest.TestCase):
    def test_shell_scripts_parse(self):
        scripts = sorted((ROOT / "experiments").rglob("*.sh"))
        for script in scripts:
            with self.subTest(script=script.relative_to(ROOT)):
                contents = script.read_text(encoding="utf-8").replace("\r\n", "\n")
                result = subprocess.run(
                    ["bash", "-n"], input=contents.encode("utf-8"), capture_output=True, check=False
                )
                self.assertEqual(
                    result.returncode,
                    0,
                    result.stderr.decode("utf-8", errors="replace"),
                )

    def run_with_stubs(self, selection, fail_module=None):
        with tempfile.TemporaryDirectory(prefix="experiment-runner-test-", dir=ROOT) as temp:
            temp_root = pathlib.Path(temp)
            module_root = temp_root / "modules"
            module_root.mkdir()
            log_path = temp_root / "calls.log"

            setup_script = temp_root / "setup.sh"
            setup_script.write_text(
                'printf "setup:%s\\n" "$EXPERIMENTS_SELECTED" >> "$TEST_LOG"\n', encoding="utf-8"
            )
            for name in ("runtime", "empty-block", "startup", "compliance", "size"):
                module_dir = module_root / name
                module_dir.mkdir()
                exit_status = 1 if name == fail_module else 0
                (module_dir / "run.sh").write_text(
                    f'printf "{name}\\n" >> "$TEST_LOG"\nexit {exit_status}\n',
                    encoding="utf-8",
                )

            env = os.environ.copy()
            env.update(
                {
                    "EXPERIMENTS_MODULE_ROOT": str(module_root),
                    "EXPERIMENTS_SETUP_SCRIPT": str(setup_script),
                    "EXPERIMENTS_OUTPUT_ROOT": str(temp_root / "results"),
                    "EXPERIMENT_RUN_ID": "test-run",
                    "TEST_LOG": str(log_path),
                }
            )
            result = subprocess.run(
                ["bash", str(RUNNER)],
                input=selection if selection.endswith("\n") else f"{selection}\n",
                text=True,
                capture_output=True,
                env=env,
                check=False,
            )
            calls = log_path.read_text(encoding="utf-8").splitlines()
            return result, calls

    def test_selected_experiments_run_in_entered_order_after_one_setup(self):
        result, calls = self.run_with_stubs("3,1,5")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["setup:startup,runtime,size", "startup", "runtime", "size"])

    def test_all_selection_uses_standard_order_after_one_setup(self):
        result, calls = self.run_with_stubs("6")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            calls,
            ["setup:runtime,empty-block,startup,compliance,size", "runtime", "empty-block", "startup", "compliance", "size"],
        )

    def test_invalid_and_duplicate_selections_are_rejected_then_reprompted(self):
        result, calls = self.run_with_stubs("0\n1,1\n2\n")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["setup:empty-block", "empty-block"])
        self.assertIn("Invalid selection", result.stderr)
        self.assertIn("selected more than once", result.stderr)

    def test_failure_stops_later_experiments(self):
        result, calls = self.run_with_stubs("1,2,3", fail_module="empty-block")

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls, ["setup:runtime,empty-block,startup", "runtime", "empty-block"])
        self.assertIn("later experiments were not started", result.stderr)


if __name__ == "__main__":
    unittest.main()