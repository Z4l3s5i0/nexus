import os
import pathlib
import re
import shlex
import shutil
import subprocess
import tempfile
import time
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
RUNNER = ROOT / "experiments" / "run.sh"


def bash_compatible_path(path):
    path = pathlib.Path(path)
    if os.name == "nt":
        return f"/mnt/{path.drive[0].lower()}/{path.as_posix()[3:]}"
    return str(path)


def forward_wsl_environment(env, names):
    if os.name == "nt":
        forwarded = [entry for entry in env.get("WSLENV", "").split(":") if entry]
        forwarded_names = {entry.split("/", 1)[0] for entry in forwarded}
        forwarded.extend(f"{name}/u" for name in names if name not in forwarded_names)
        env["WSLENV"] = ":".join(forwarded)


def bash_has_command(command):
    if shutil.which(command):
        return True
    if os.name == "nt" and shutil.which("bash"):
        return subprocess.run(
            ["bash", "-c", f"command -v {shlex.quote(command)} >/dev/null 2>&1"],
            check=False,
        ).returncode == 0
    return False


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

    def run_with_stubs(
        self, selection, fail_module=None, config_answers=None, fail_setup_status=None,
        runner_args=None, private_key="0" * 64,
    ):
        with tempfile.TemporaryDirectory(prefix="experiment-runner-test-", dir=ROOT) as temp:
            temp_root = pathlib.Path(temp)
            module_root = temp_root / "modules"
            module_root.mkdir()
            log_path = temp_root / "calls.log"

            setup_script = temp_root / "setup.sh"
            setup_script.write_bytes((
                'printf "setup-output\\n"\n'
                'printf "setup:%s\\n" "$EXPERIMENTS_SELECTED" >> "$TEST_LOG"\n'
                'if [[ -n "${EB_PRIVATE_KEY:-}" ]]; then printf "setup-key-leaked\\n" >> "$TEST_LOG"; fi\n'
                + (f"exit {fail_setup_status}\n" if fail_setup_status is not None else "")
            ).encode("utf-8"))
            for name in ("runtime", "empty-block", "startup", "compliance", "size"):
                module_dir = module_root / name
                module_dir.mkdir()
                exit_status = 1 if name == fail_module else 0
                (module_dir / "run.sh").write_bytes((
                    f'printf "{name}-output\\n"\n'
                    f'printf "{name}\\n" >> "$TEST_LOG"\n'
                    f'if [[ "$#" -gt 0 ]]; then printf "args:{name}:%s\\n" "$*" >> "$TEST_LOG"; fi\n'
                    'if [[ "${EXPERIMENT_RUN_ID:-}" == *-runtime-* ]]; then printf "run-id:%s\\n" "$EXPERIMENT_RUN_ID" >> "$TEST_LOG"; fi\n'
                    f'if [[ "{name}" == empty-block && -n "${{EB_PRIVATE_KEY:-}}" ]]; then printf "private-key-present\\n" >> "$TEST_LOG"; fi\n'
                    f'if [[ "{name}" == empty-block && "${{EB_PRIVATE_KEY:-}}" == "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80" ]]; then printf "private-key-default\\n" >> "$TEST_LOG"; fi\n'
                    f'exit {exit_status}\n'
                ).encode("utf-8"))
                (module_dir / "report.py").write_text(
                    "import os, pathlib, sys\n"
                    "output_dir = pathlib.Path(sys.argv[sys.argv.index('--output-dir') + 1])\n"
                    f"print('{name}-report-output')\n"
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
                    "EXPERIMENTS_MODULE_ROOT": bash_compatible_path(module_root),
                    "EXPERIMENTS_SETUP_SCRIPT": bash_compatible_path(setup_script),
                    "EXPERIMENTS_OUTPUT_ROOT": bash_compatible_path(output_root),
                    "EXPERIMENT_RUN_ID": "test-run",
                    "TEST_LOG": bash_compatible_path(log_path),
                    "EB_PRIVATE_KEY": private_key,
                }
            )
            forward_wsl_environment(
                env,
                (
                    "EXPERIMENTS_MODULE_ROOT",
                    "EXPERIMENTS_SETUP_SCRIPT",
                    "EXPERIMENTS_OUTPUT_ROOT",
                    "EXPERIMENT_RUN_ID",
                    "TEST_LOG",
                    "EB_PRIVATE_KEY",
                ),
            )
            input_text = selection if selection.endswith("\n") else f"{selection}\n"
            if config_answers is not None:
                input_text += config_answers
            elif not selection.startswith("7\n"):
                valid_selection = selection.strip().splitlines()[-1].strip()
                configured_count = 5 if valid_selection in ("6", "all") else len(valid_selection.split(","))
                input_text += "n\n" * configured_count
            runner_copy = temp_root / "run.sh"
            runner_copy.write_bytes(RUNNER.read_text(encoding="utf-8").replace("\r\n", "\n").encode("utf-8"))
            input_path = temp_root / "runner-input.txt"
            input_path.write_bytes(input_text.encode("utf-8"))
            runner_arguments = [runner_copy.relative_to(ROOT).as_posix(), *(runner_args or [])]
            command = "exec bash " + " ".join(shlex.quote(argument) for argument in runner_arguments)
            command += " < " + shlex.quote(bash_compatible_path(input_path))
            result = subprocess.run(
                ["bash", "-c", command],
                text=True,
                capture_output=True,
                env=env,
                cwd=ROOT,
                check=False,
            )
            if "--detach" in (runner_args or []):
                sessions = list((output_root / "logs").iterdir())
                if sessions:
                    status_file = sessions[0] / "runner.status"
                    deadline = time.monotonic() + 10
                    while not status_file.exists() and time.monotonic() < deadline:
                        time.sleep(0.05)
            calls = log_path.read_text(encoding="utf-8").splitlines() if log_path.exists() else []
            logs_root = output_root / "logs"
            log_sessions = list(logs_root.iterdir()) if logs_root.exists() else []
            self.last_logs = {}
            if log_sessions:
                session_dir = log_sessions[0]
                self.last_logs = {
                    path.relative_to(session_dir).as_posix(): path.read_text(encoding="utf-8")
                    for path in session_dir.rglob("*")
                    if path.is_file()
                }
            return result, calls

    def test_selected_experiments_run_in_entered_order_after_one_setup(self):
        result, calls = self.run_with_stubs("3,1,5")

        self.assertEqual(result.returncode, 0, f"{result.stderr}\n{result.stdout}")
        self.assertEqual(
            calls,
            ["setup:startup,runtime,size", "startup", "runtime", "args:runtime:--workload-type exp/uber", "size"],
        )
        self.assertIn("runner.log", self.last_logs, self.last_logs)
        self.assertIn("Choose experiments by entering their numbers", self.last_logs["runner.log"])
        self.assertIn("setup-output", self.last_logs["setup.log"])
        self.assertIn("startup-output", self.last_logs["startup/run-test-run.log"], self.last_logs)
        self.assertIn("runtime-output", self.last_logs["runtime/run-test-run.log"])
        self.assertIn("size-output", self.last_logs["size/run-test-run.log"])
        self.assertEqual(
            [path for path in self.last_logs if pathlib.PurePosixPath(path).name == "setup.log"],
            ["setup.log"],
        )

    def test_setup_failure_is_logged_and_preserves_exit_status(self):
        result, calls = self.run_with_stubs("1", fail_setup_status=7)

        self.assertEqual(result.returncode, 7)
        self.assertEqual(calls, ["setup:runtime"])
        self.assertIn("Exit status: 7", self.last_logs["setup.log"])
        self.assertNotIn("runtime/run-test-run.log", self.last_logs)

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

    def test_reset_network_option_is_forwarded_to_runtime_defaults(self):
        result, calls = self.run_with_stubs("1", runner_args=["--reset-network"])

        self.assertEqual(result.returncode, 0, f"{result.stderr}\n{result.stdout}")
        self.assertIn("args:runtime:--reset-network --workload-type exp/uber", calls)

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
        self.assertIn("empty-block-output", self.last_logs["empty-block/run-test-run.log"])
        self.assertIn("Exit status: 1", self.last_logs["empty-block/run-test-run.log"])

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

    def test_empty_block_defaults_to_prefunded_anvil_key_without_prompting(self):
        result, calls = self.run_with_stubs("2", config_answers="n\n", private_key="")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("private-key-default", calls)
        self.assertNotIn("Funded test account private key", result.stdout + result.stderr)
        self.assertNotIn("0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80", "".join(self.last_logs.values()))

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

    @unittest.skipUnless(
        os.name != "nt" and bash_has_command("setsid") and bash_has_command("nohup"),
        "detached child processes cannot be validated from a Windows WSL host",
    )
    def test_detached_run_finishes_and_writes_reattachable_console_log(self):
        result, calls = self.run_with_stubs("5", config_answers="\n", runner_args=["--detach"])

        self.assertEqual(result.returncode, 0, f"{result.stderr}\n{result.stdout}")
        self.assertIn("size", calls, {"stdout": result.stdout, "logs": self.last_logs})
        self.assertIn("--attach", result.stdout)
        self.assertIn("--stop", result.stdout)
        self.assertIn("runner.status", self.last_logs)
        self.assertIn("size-output", self.last_logs["runner.log"])

    @unittest.skipUnless(
        os.name != "nt" and bash_has_command("setsid") and bash_has_command("nohup"),
        "detached child processes cannot be validated from a Windows WSL host",
    )
    def test_stop_terminates_a_running_detached_session(self):
        with tempfile.TemporaryDirectory(prefix="experiment-runner-stop-", dir=ROOT) as temp:
            temp_root = pathlib.Path(temp)
            module_root = temp_root / "modules"
            size_module = module_root / "size"
            size_module.mkdir(parents=True)
            (size_module / "run.sh").write_bytes(b"printf 'size-started\\n'\nsleep 30\n")
            setup_script = temp_root / "setup.sh"
            setup_script.write_bytes(b"printf 'setup-started\\n'\n")
            output_root = temp_root / "results"
            runner_copy = temp_root / "run.sh"
            runner_copy.write_bytes(RUNNER.read_text(encoding="utf-8").replace("\r\n", "\n").encode("utf-8"))
            input_path = temp_root / "runner-input.txt"
            input_path.write_bytes(b"5\nn\n")
            env = os.environ.copy()
            env.update(
                {
                    "EXPERIMENTS_MODULE_ROOT": bash_compatible_path(module_root),
                    "EXPERIMENTS_SETUP_SCRIPT": bash_compatible_path(setup_script),
                    "EXPERIMENTS_OUTPUT_ROOT": bash_compatible_path(output_root),
                }
            )
            forward_wsl_environment(
                env,
                ("EXPERIMENTS_MODULE_ROOT", "EXPERIMENTS_SETUP_SCRIPT", "EXPERIMENTS_OUTPUT_ROOT"),
            )
            runner_argument = runner_copy.relative_to(ROOT).as_posix()
            launch_command = f"exec bash {shlex.quote(runner_argument)} --detach"
            launch_command += " < " + shlex.quote(bash_compatible_path(input_path))
            session_id = None
            try:
                launched = subprocess.run(
                    ["bash", "-c", launch_command],
                    text=True,
                    capture_output=True,
                    env=env,
                    cwd=ROOT,
                    check=False,
                )
                self.assertEqual(launched.returncode, 0, launched.stderr)
                match = re.search(r"Detached experiment session: ([A-Za-z0-9._-]+)", launched.stdout)
                self.assertIsNotNone(match, launched.stdout)
                session_id = match.group(1)
                session_dir = output_root / "logs" / session_id
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    log_path = session_dir / "runner.log"
                    if log_path.is_file() and "size-started" in log_path.read_text(encoding="utf-8"):
                        break
                    time.sleep(0.05)
                self.assertTrue(log_path.is_file() and "size-started" in log_path.read_text(encoding="utf-8"))

                stop_command = f"exec bash {shlex.quote(runner_argument)} --stop {shlex.quote(session_id)}"
                stopped = subprocess.run(
                    ["bash", "-c", stop_command],
                    text=True,
                    capture_output=True,
                    env=env,
                    cwd=ROOT,
                    check=False,
                )
                self.assertEqual(stopped.returncode, 0, stopped.stderr)
                deadline = time.monotonic() + 10
                while not (session_dir / "runner.status").exists() and time.monotonic() < deadline:
                    time.sleep(0.05)
                self.assertTrue((session_dir / "runner.status").is_file())
            finally:
                if session_id is not None:
                    session_dir = output_root / "logs" / session_id
                    if (session_dir / "runner.pid").is_file() and not (session_dir / "runner.status").exists():
                        stop_command = f"exec bash {shlex.quote(runner_argument)} --stop {shlex.quote(session_id)}"
                        subprocess.run(
                            ["bash", "-c", stop_command],
                            text=True,
                            capture_output=True,
                            env=env,
                            cwd=ROOT,
                            check=False,
                        )

    def test_attach_replays_an_existing_session_log(self):
        with tempfile.TemporaryDirectory(prefix="experiment-runner-attach-") as temp:
            temp_root = pathlib.Path(temp)
            output_root = temp_root / "results"
            session_dir = output_root / "logs" / "test-session"
            session_dir.mkdir(parents=True)
            (session_dir / "runner.log").write_text("saved console output\n", encoding="utf-8")
            (session_dir / "runner.status").write_text("0\n", encoding="utf-8")
            runner_copy = temp_root / "run.sh"
            runner_copy.write_bytes(RUNNER.read_text(encoding="utf-8").replace("\r\n", "\n").encode("utf-8"))
            env = os.environ.copy()
            env["EXPERIMENTS_OUTPUT_ROOT"] = bash_compatible_path(output_root)
            forward_wsl_environment(env, ("EXPERIMENTS_OUTPUT_ROOT",))

            command = f"exec bash {shlex.quote(bash_compatible_path(runner_copy))} --attach test-session"
            result = subprocess.run(
                ["bash", "-c", command],
                text=True,
                capture_output=True,
                env=env,
                cwd=ROOT,
                check=False,
            )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("saved console output", result.stdout)

    def test_stop_reports_a_completed_session_without_signaling_its_pid(self):
        with tempfile.TemporaryDirectory(prefix="experiment-runner-stop-", dir=ROOT) as temp:
            temp_root = pathlib.Path(temp)
            output_root = temp_root / "results"
            session_dir = output_root / "logs" / "completed-session"
            session_dir.mkdir(parents=True)
            (session_dir / "runner.pid").write_text("1\n", encoding="utf-8")
            (session_dir / "runner.status").write_text("0\n", encoding="utf-8")
            runner_copy = temp_root / "run.sh"
            runner_copy.write_bytes(RUNNER.read_text(encoding="utf-8").replace("\r\n", "\n").encode("utf-8"))
            env = os.environ.copy()
            env["EXPERIMENTS_OUTPUT_ROOT"] = bash_compatible_path(output_root)
            forward_wsl_environment(env, ("EXPERIMENTS_OUTPUT_ROOT",))

            result = subprocess.run(
                ["bash", runner_copy.relative_to(ROOT).as_posix(), "--stop", "completed-session"],
                text=True,
                capture_output=True,
                env=env,
                cwd=ROOT,
                check=False,
            )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("has already finished", result.stdout)

    def test_evaluation_regenerates_only_selected_reports_without_setup_or_runs(self):
        result, calls = self.run_with_stubs("7\n2,5")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["empty-block:saved-run", "size:saved-run"])
        self.assertIn("Evaluation mode", result.stdout)
        self.assertIn("All selected saved results were evaluated successfully", self.last_logs["runner.log"])
        self.assertIn("empty-block-report-output", self.last_logs["empty-block/evaluation-1.log"])
        self.assertIn("size-report-output", self.last_logs["size/evaluation-1.log"])
        self.assertNotIn("setup.log", self.last_logs)

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
                    "EXPERIMENTS_MODULE_ROOT": bash_compatible_path(module_root),
                    "EXPERIMENTS_SETUP_SCRIPT": bash_compatible_path(setup_script),
                    "EXPERIMENTS_OUTPUT_ROOT": bash_compatible_path(temp_root / "results"),
                }
            )
            forward_wsl_environment(
                env,
                ("EXPERIMENTS_MODULE_ROOT", "EXPERIMENTS_SETUP_SCRIPT", "EXPERIMENTS_OUTPUT_ROOT"),
            )
            runner_copy = temp_root / "run.sh"
            runner_copy.write_bytes(RUNNER.read_text(encoding="utf-8").replace("\r\n", "\n").encode("utf-8"))
            input_path = temp_root / "runner-input.txt"
            input_path.write_bytes(b"7\n1\n")
            command = "exec bash " + shlex.quote(runner_copy.relative_to(ROOT).as_posix())
            command += " < " + shlex.quote(bash_compatible_path(input_path))
            result = subprocess.run(
                ["bash", "-c", command], text=True, capture_output=True, env=env, cwd=ROOT, check=False
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
                    "EXPERIMENTS_MODULE_ROOT": bash_compatible_path(module_root),
                    "EXPERIMENTS_SETUP_SCRIPT": bash_compatible_path(temp_root / "missing-setup.sh"),
                    "EXPERIMENTS_OUTPUT_ROOT": bash_compatible_path(output_root),
                }
            )
            forward_wsl_environment(
                env,
                ("EXPERIMENTS_MODULE_ROOT", "EXPERIMENTS_SETUP_SCRIPT", "EXPERIMENTS_OUTPUT_ROOT"),
            )
            runner_copy = temp_root / "run.sh"
            runner_copy.write_bytes(RUNNER.read_text(encoding="utf-8").replace("\r\n", "\n").encode("utf-8"))
            input_path = temp_root / "runner-input.txt"
            input_path.write_bytes(b"7\n1\n")
            command = "exec bash " + shlex.quote(runner_copy.relative_to(ROOT).as_posix())
            command += " < " + shlex.quote(bash_compatible_path(input_path))
            result = subprocess.run(
                ["bash", "-c", command], text=True, capture_output=True, env=env, cwd=ROOT, check=False
            )

        self.assertEqual(result.returncode, 1)
        self.assertIn("skipping incomplete runtime results", result.stderr)
        self.assertIn("metadata.json", result.stderr)
        self.assertNotIn("should not run", result.stderr)


if __name__ == "__main__":
    unittest.main()