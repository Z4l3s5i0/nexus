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

    def run_with_stubs(self, selection, fail_module=None, config_answers=None):
        with tempfile.TemporaryDirectory(prefix="experiment-runner-test-", dir=ROOT) as temp:
            temp_root = pathlib.Path(temp)
            module_root = temp_root / "modules"
            module_root.mkdir()
            log_path = temp_root / "calls.log"

            setup_script = temp_root / "setup.sh"
            setup_script.write_text(
                'printf "setup:%s\\n" "$EXPERIMENTS_SELECTED" >> "$TEST_LOG"\n'
                'if [[ -n "${EB_PRIVATE_KEY:-}" ]]; then printf "setup-key-leaked\\n" >> "$TEST_LOG"; fi\n',
                encoding="utf-8",
            )
            for name in ("runtime", "empty-block", "startup", "compliance", "size"):
                module_dir = module_root / name
                module_dir.mkdir()
                exit_status = 1 if name == fail_module else 0
                (module_dir / "run.sh").write_text(
                    f'printf "{name}\\n" >> "$TEST_LOG"\n'
                    f'if [[ "$#" -gt 0 ]]; then printf "args:{name}:%s\\n" "$*" >> "$TEST_LOG"; fi\n'
                    'if [[ "${EXPERIMENT_RUN_ID:-}" == *-runtime-* ]]; then printf "run-id:%s\\n" "$EXPERIMENT_RUN_ID" >> "$TEST_LOG"; fi\n'
                    f'if [[ "{name}" == empty-block && -n "${{EB_PRIVATE_KEY:-}}" ]]; then printf "private-key-present\\n" >> "$TEST_LOG"; fi\n'
                    f'exit {exit_status}\n',
                    encoding="utf-8",
                )
                (module_dir / "report.py").write_text(
                    "import os, pathlib, sys\n"
                    "output_dir = pathlib.Path(sys.argv[sys.argv.index('--output-dir') + 1])\n"
                    f"with open(os.environ['TEST_LOG'], 'a', encoding='utf-8') as log: log.write('{name}:' + output_dir.name + '\\n')\n",
                    encoding="utf-8",
                )

            env = os.environ.copy()
            output_root = temp_root / "results"
            for name in ("runtime", "empty-block", "startup", "compliance", "size"):
                run_dir = output_root / name / "saved-run"
                run_dir.mkdir(parents=True)
                required = {
                    "runtime": ("metadata.json",),
                    "empty-block": ("metadata.json",),
                    "startup": ("metadata.json",),
                    "compliance": ("metadata.json", "results.jsonl"),
                    "size": ("source_size.tsv", "binary_size.tsv"),
                }[name]
                for artifact in required:
                    (run_dir / artifact).write_text("{}\n", encoding="utf-8")
            env.update(
                {
                    "EXPERIMENTS_MODULE_ROOT": str(module_root),
                    "EXPERIMENTS_SETUP_SCRIPT": str(setup_script),
                    "EXPERIMENTS_OUTPUT_ROOT": str(output_root),
                    "EXPERIMENT_RUN_ID": "test-run",
                    "TEST_LOG": str(log_path),
                    "EB_PRIVATE_KEY": "0" * 64,
                }
            )
            input_text = selection if selection.endswith("\n") else f"{selection}\n"
            if config_answers is not None:
                input_text += config_answers
            elif not selection.startswith("7\n"):
                valid_selection = selection.strip().splitlines()[-1].strip()
                configured_count = 5 if valid_selection in ("6", "all") else len(valid_selection.split(","))
                input_text += "\n" * configured_count
            result = subprocess.run(
                ["bash", str(RUNNER)],
                input=input_text,
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
        self.assertEqual(
            calls,
            ["setup:startup,runtime,size", "startup", "runtime", "args:runtime:--workload-type exp/uber", "size"],
        )

    def test_all_selection_uses_standard_order_after_one_setup(self):
        result, calls = self.run_with_stubs("6")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            calls,
            [
                "setup:runtime,empty-block,startup,compliance,size",
                "runtime",
                "args:runtime:--workload-type exp/uber",
                "empty-block",
                "private-key-present",
                "startup",
                "compliance",
                "size",
            ],
        )

    def test_invalid_and_duplicate_selections_are_rejected_then_reprompted(self):
        result, calls = self.run_with_stubs("0\n1,1\n2\n")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["setup:empty-block", "empty-block", "private-key-present"])
        self.assertIn("Invalid selection", result.stderr)
        self.assertIn("selected more than once", result.stderr)

    def test_failure_stops_later_experiments(self):
        result, calls = self.run_with_stubs("1,2,3", fail_module="empty-block")

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(
            calls,
            [
                "setup:runtime,empty-block,startup",
                "runtime",
                "args:runtime:--workload-type exp/uber",
                "empty-block",
                "private-key-present",
            ],
        )
        self.assertIn("later experiments were not started", result.stderr)

    def test_custom_runtime_configuration_is_forwarded_and_repeats_get_unique_run_ids(self):
        answers = "y\n1\n0\n1\n0\nexp/uber,exp/dota\n3\n8\n45\n0\n99\n2\nn\n"
        result, calls = self.run_with_stubs("1", config_answers=answers)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls[0], "setup:runtime")
        runtime_args = "--linux-nodes 1 --wasix-nodes 0 --geth-nodes 1 --reth-nodes 0 --accounts 3 --tps 8 --duration 45 --wait-duration 0 --chain-id 99"
        self.assertEqual(
            [call for call in calls if call.startswith("args:runtime:")],
            [
                f"args:runtime:{runtime_args} --workload-type exp/uber",
                f"args:runtime:{runtime_args} --workload-type exp/dota",
                f"args:runtime:{runtime_args} --workload-type exp/uber",
                f"args:runtime:{runtime_args} --workload-type exp/dota",
            ],
        )
        self.assertEqual(
            [call for call in calls if call.startswith("run-id:")],
            [
                "run-id:test-run-runtime-1-1",
                "run-id:test-run-runtime-1-2",
                "run-id:test-run-runtime-2-1",
                "run-id:test-run-runtime-2-2",
            ],
        )

    def test_invalid_runtime_value_is_reprompted(self):
        answers = "y\ninvalid\n1\n0\n0\n0\nexp/uber\n4\n4\n300\n1800\n12345\n1\nn\n"
        result, calls = self.run_with_stubs("1", config_answers=answers)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Enter a nonnegative whole number", result.stderr)
        self.assertTrue(any(call.startswith("args:runtime:--linux-nodes 1") for call in calls))

    def test_custom_empty_block_configuration_forwards_options_and_keeps_key_from_setup(self):
        answers = "y\ngeth\n23456\n0,1,5\n2\n3\n30\n20\n0.5\nn\n"
        result, calls = self.run_with_stubs("2", config_answers=answers)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("setup-key-leaked", calls)
        self.assertIn("private-key-present", calls)
        self.assertTrue(any(call.startswith("args:empty-block:--client geth --chain-id 23456 --counts 0,1,5") for call in calls))

    def test_custom_startup_configuration_forwards_options(self):
        answers = "y\ngeth,nexus-linux\n2\n777\n30\n0.5\ngeth:test\nreth:test\nlinux:test\nwasix:test\n\nwasmer-test\n"
        result, calls = self.run_with_stubs("3", config_answers=answers)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(any(call.startswith("args:startup:--configs geth,nexus-linux --iterations 2") for call in calls))
        self.assertTrue(any("--wasmer-bin wasmer-test" in call for call in calls))

    def test_custom_compliance_configuration_forwards_options(self):
        answers = "y\nwasix-w-eth\nhive-test\n4\n900\nn\ny\n"
        result, calls = self.run_with_stubs("4", config_answers=answers)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(any("--clients wasix-w-eth --hive-image hive-test --parallelism 4 --timeout 900 --skip-hive-build" in call for call in calls))

    def test_custom_size_configuration_splits_source_and_binary_inputs(self):
        answers = "y\ncustom=/tmp/source tree\ncustom=/tmp/client binary\n"
        result, calls = self.run_with_stubs("5", config_answers=answers)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(any("--source custom=/tmp/source tree --binary custom=/tmp/client binary" in call for call in calls))

    def test_evaluation_regenerates_only_selected_reports_without_setup_or_runs(self):
        result, calls = self.run_with_stubs("7\n2,5")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["empty-block:saved-run", "size:saved-run"])
        self.assertIn("Evaluation mode", result.stdout)
        self.assertIn("All selected saved results were evaluated successfully", result.stdout)

    def test_evaluation_reports_missing_results_without_running_setup(self):
        with tempfile.TemporaryDirectory(prefix="experiment-runner-test-", dir=ROOT) as temp:
            temp_root = pathlib.Path(temp)
            module_root = temp_root / "modules"
            module_root.mkdir()
            setup_script = temp_root / "setup.sh"
            setup_script.write_text('exit 99\n', encoding="utf-8")
            runtime = module_root / "runtime"
            runtime.mkdir()
            (runtime / "report.py").write_text("raise SystemExit('should not run')\n", encoding="utf-8")
            env = os.environ.copy()
            env.update(
                {
                    "EXPERIMENTS_MODULE_ROOT": str(module_root),
                    "EXPERIMENTS_SETUP_SCRIPT": str(setup_script),
                    "EXPERIMENTS_OUTPUT_ROOT": str(temp_root / "results"),
                }
            )
            result = subprocess.run(
                ["bash", str(RUNNER)], input="7\n1\n", text=True, capture_output=True, env=env, check=False
            )

        self.assertEqual(result.returncode, 1)
        self.assertIn("no saved runtime results", result.stderr)
        self.assertNotIn("should not run", result.stderr)

    def test_evaluation_reports_incomplete_saved_runs(self):
        with tempfile.TemporaryDirectory(prefix="experiment-runner-test-", dir=ROOT) as temp:
            temp_root = pathlib.Path(temp)
            module_root = temp_root / "modules"
            runtime = module_root / "runtime"
            runtime.mkdir(parents=True)
            (runtime / "report.py").write_text("raise SystemExit('should not run')\n", encoding="utf-8")
            output_root = temp_root / "results"
            (output_root / "runtime" / "incomplete-run").mkdir(parents=True)
            env = os.environ.copy()
            env.update(
                {
                    "EXPERIMENTS_MODULE_ROOT": str(module_root),
                    "EXPERIMENTS_SETUP_SCRIPT": str(temp_root / "missing-setup.sh"),
                    "EXPERIMENTS_OUTPUT_ROOT": str(output_root),
                }
            )
            result = subprocess.run(
                ["bash", str(RUNNER)], input="7\n1\n", text=True, capture_output=True, env=env, check=False
            )

        self.assertEqual(result.returncode, 1)
        self.assertIn("skipping incomplete runtime results", result.stderr)
        self.assertIn("metadata.json", result.stderr)
        self.assertNotIn("should not run", result.stderr)


if __name__ == "__main__":
    unittest.main()