import csv
import io
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

    def test_runtime_value_transfer_uses_cast_workload(self):
        workload_script = (ROOT / "experiments" / "runtime" / "workload.sh").read_text(encoding="utf-8")
        cast_workload = (ROOT / "experiments" / "runtime" / "cast_workload.py").read_text(encoding="utf-8")

        self.assertIn('python3 "$SCRIPT_DIR/cast_workload.py"', workload_script)
        self.assertIn('export EXPERIMENT_WALLET_MNEMONIC="$MNEMONIC"', workload_script)
        self.assertIn('if scenario == "value-transfer":', cast_workload)
        self.assertIn('destination = VALUE_TRANSFER_TO', cast_workload)
        self.assertIn('value = "1"', cast_workload)
        self.assertIn('gas_limit = 21_000', cast_workload)

    def test_runtime_cast_setup_waits_before_spam(self):
        workload_script = (ROOT / "experiments" / "runtime" / "workload.sh").read_text(encoding="utf-8")
        cast_workload = (ROOT / "experiments" / "runtime" / "cast_workload.py").read_text(encoding="utf-8")
        setup = cast_workload.index("accounts, contract_address = prepare_accounts(")
        delay = cast_workload.index("time.sleep(args.start_delay)", setup)
        workload_start = cast_workload.index("start_ms = time.time_ns()", delay)
        spam = cast_workload.index("submit_transaction_schedule(", workload_start)
        window = cast_workload.index("write_workload_window(args, start_ms, end_ms, exit_code)", spam)

        self.assertIn('cast wallet private-key', workload_script)
        self.assertLess(setup, delay)
        self.assertLess(delay, workload_start)
        self.assertLess(workload_start, spam)
        self.assertLess(spam, window)
        self.assertIn("wait_for_receipts(rpc_url, funding_hashes", cast_workload)
        self.assertIn("submit_contract_deployment(", cast_workload)

    def run_with_stubs(
        self, selection, fail_module=None, config_answers=None, fail_setup_status=None,
        runner_args=None, private_key="0" * 64, fail_empty_block_count=None,
        missing_run_status_for=None,
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
                    f'case "{name}" in runtime) printf "Runtime progress: Workload exp/uber spam starting.\\n" ;; startup) printf "Startup progress: Starting geth iteration 1 of 1.\\n" ;; compliance) printf "Compliance progress: Starting Hive ethereum/engine for wasix-r-eth.\\n" ;; size) printf "Size progress: Source size measurements starting.\\n" ;; esac\n'
                    f'if [[ "{name}" == empty-block ]]; then printf "Empty-block progress: step 1 of 7 - transaction spam (10 transactions)\\n"; fi\n'
                    f'printf "{name}\\n" >> "$TEST_LOG"\n'
                    f'if [[ "$#" -gt 0 ]]; then printf "args:{name}:%s\\n" "$*" >> "$TEST_LOG"; fi\n'
                    'if [[ "${EXPERIMENT_RUN_ID:-}" == *-runtime-* ]]; then printf "run-id:%s\\n" "$EXPERIMENT_RUN_ID" >> "$TEST_LOG"; fi\n'
                    f'if [[ "{name}" == empty-block && -n "${{EB_PRIVATE_KEY:-}}" ]]; then printf "private-key-present\\n" >> "$TEST_LOG"; fi\n'
                    f'if [[ "{name}" == empty-block && "${{EB_PRIVATE_KEY:-}}" == "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80" ]]; then printf "private-key-default\\n" >> "$TEST_LOG"; fi\n'
                    f'if [[ {exit_status} -ne 0 ]]; then printf "Error: stubbed failure of {name}\\n" >&2; exit {exit_status}; fi\n'
                    f'if [[ "{name}" == empty-block && -n "${{FAIL_EMPTY_BLOCK_COUNT:-}}" && " $* " == *" --counts ${{FAIL_EMPTY_BLOCK_COUNT}} "* ]]; then printf "Error: simulated failure for burst count\\n" >&2; exit 1; fi\n'
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
                if missing_run_status_for != name:
                    (run_dir / "run_status.json").write_text(
                        '{"status":"passed","exit_code":0}\n', encoding="utf-8"
                    )
                if name == "empty-block":
                    (run_dir / "metadata.json").write_text(
                        '{"experiment_id":"saved-run","chain_id":1,"sweep":{"transaction_counts":[0],"blocks_per_count":1}}\n',
                        encoding="utf-8",
                    )
                    csv_headers = {
                        "block_samples.csv": "target_tx_count,block_number,actual_tx_count,matched\n0,1,0,true\n",
                        "samples.csv": "target_tx_count,block_number,actual_tx_count,matched\n0,1,0,true\n",
                        "transaction_inclusion_samples.csv": (
                            "transaction_type,tx_hash,submitted_at_utc,included_at_utc,"
                            "inclusion_latency_ms,latency_basis,block_number,status\n"
                        ),
                        "transaction_burst_samples.csv": (
                            "target_tx_count,attempt,submitted_tx_count,included_tx_count,completed,blocks_spanned,latency_basis\n"
                        ),
                    }
                    for filename, contents in csv_headers.items():
                        (run_dir / filename).write_text(contents, encoding="utf-8")
                    for filename in (
                        "system_metric_samples.jsonl", "events.jsonl", "log_metric_samples.jsonl",
                        "derived_metric_samples.jsonl",
                    ):
                        (run_dir / filename).write_text("", encoding="utf-8")
                    for filename in ("block_metric_coverage.json", "log_metrics_coverage.json", "node_metric_coverage.json"):
                        (run_dir / filename).write_text("{}\n", encoding="utf-8")
                    metrics_dir = run_dir / "metrics_server_data"
                    metrics_dir.mkdir()
                    (metrics_dir / "metrics.redb").write_bytes(b"saved-metrics")
                elif name == "size":
                    (run_dir / "source_size.tsv").write_text(
                        "measurement\tlabel\tvalue\tunit\tsource\tstatus\nsource_code\tclient\t1\tlines\tsource\tmeasured\n",
                        encoding="utf-8",
                    )
                    (run_dir / "binary_size.tsv").write_text(
                        "measurement\tlabel\tvalue\tunit\tsource\tstatus\ncompiled_binary\tclient\t1\tbytes\tbinary\tmeasured\n",
                        encoding="utf-8",
                    )
            env.update(
                {
                    "EXPERIMENTS_MODULE_ROOT": bash_compatible_path(module_root),
                    "EXPERIMENTS_SETUP_SCRIPT": bash_compatible_path(setup_script),
                    "EXPERIMENTS_OUTPUT_ROOT": bash_compatible_path(output_root),
                    "EXPERIMENT_RUN_ID": "test-run",
                    "TEST_LOG": bash_compatible_path(log_path),
                    "EB_PRIVATE_KEY": private_key,
                    "FAIL_EMPTY_BLOCK_COUNT": fail_empty_block_count or "",
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
                    "FAIL_EMPTY_BLOCK_COUNT",
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
        answers = "n\ny\n2\n2\n0\n0\nexp/uber,exp/uber_v2\n4\n4\n300\n1800\n12345\n1\nn\nn\n"
        result, calls = self.run_with_stubs("3,1,5", config_answers=answers)

        self.assertEqual(result.returncode, 0, f"{result.stderr}\n{result.stdout}")
        self.assertEqual(
            calls,
            [
                "setup:startup,runtime,size", "startup",
                "args:startup:--configs geth,reth,nexus-linux,nexus-wasix-docker,nexus-wasmer --iterations 5",
                "runtime",
                "args:runtime:--linux-nodes 2 --wasix-nodes 2 --geth-nodes 0 --reth-nodes 0 --accounts 4 --tps 4 --duration 300 --wait-duration 1800 --chain-id 12345 --workload-type exp/uber",
                "run-id:test-run-runtime-1-1", "runtime",
                "args:runtime:--linux-nodes 2 --wasix-nodes 2 --geth-nodes 0 --reth-nodes 0 --accounts 4 --tps 4 --duration 300 --wait-duration 1800 --chain-id 12345 --workload-type exp/uber_v2",
                "run-id:test-run-runtime-1-2", "size",
            ],
        )
        self.assertIn("runner.log", self.last_logs, self.last_logs)
        self.assertIn("Choose experiments by entering their numbers", self.last_logs["runner.log"])
        self.assertIn("setup-output", self.last_logs["setup.log"])
        self.assertIn("startup-output", self.last_logs["startup/run-test-run.log"], self.last_logs)
        self.assertIn("runtime-output", self.last_logs["runtime/run-test-run-runtime-1-1.log"])
        self.assertIn("runtime-output", self.last_logs["runtime/run-test-run-runtime-1-2.log"])
        self.assertIn("size-output", self.last_logs["size/run-test-run.log"])
        self.assertEqual(
            [path for path in self.last_logs if pathlib.PurePosixPath(path).name == "setup.log"],
            ["setup.log"],
        )

    def test_info_verbosity_keeps_child_output_in_logs_only(self):
        runtime_answers = "y\n2\n2\n0\n0\nexp/uber\n4\n4\n300\n1800\n12345\n1\nn\n"
        result, calls = self.run_with_stubs("1", config_answers=runtime_answers)

        self.assertEqual(result.returncode, 0, f"{result.stderr}\n{result.stdout}")
        console = result.stdout + result.stderr
        self.assertIn("Running shared setup once", console)
        self.assertIn("=== Starting runtime experiment ===", console)
        self.assertNotIn("setup-output", console)
        self.assertNotIn("runtime-output", console)
        self.assertIn("setup-output", self.last_logs["setup.log"])
        self.assertIn("runtime-output", self.last_logs["runtime/run-test-run-runtime-1-1.log"])
        self.assertIn("=== Starting runtime experiment ===", self.last_logs["runner.log"])

    def test_progress_is_visible_without_child_diagnostics_at_info_verbosity(self):
        answers = "y\n2\n2\n0\n0\nexp/uber\n4\n4\n300\n1800\n12345\n1\nn\nn\nn\nn\n"
        result, calls = self.run_with_stubs("1,3,4,5", config_answers=answers)

        self.assertEqual(result.returncode, 0, f"{result.stderr}\n{result.stdout}")
        console = result.stdout + result.stderr
        for progress in (
            "Runtime progress: Workload exp/uber spam starting.",
            "Startup progress: Starting geth iteration 1 of 1.",
            "Compliance progress: Starting Hive ethereum/engine for wasix-r-eth.",
            "Size progress: Source size measurements starting.",
        ):
            self.assertIn(progress, console)
        for diagnostic in ("runtime-output", "startup-output", "compliance-output", "size-output"):
            self.assertNotIn(diagnostic, console)

    def test_progress_filter_preserves_child_failure_status(self):
        runtime_answers = "y\n2\n2\n0\n0\nexp/uber\n4\n4\n300\n1800\n12345\n1\nn\n"
        result, calls = self.run_with_stubs("1", fail_module="runtime", config_answers=runtime_answers)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Runtime progress: Workload exp/uber spam starting.", result.stdout)
        rows = list(csv.DictReader(io.StringIO(self.last_logs["overview.csv"])))
        self.assertEqual(rows[-1]["status"], "failed")

    def test_debug_verbosity_streams_child_output_and_keeps_logs(self):
        runtime_answers = "y\n2\n2\n0\n0\nexp/uber\n4\n4\n300\n1800\n12345\n1\nn\n"
        result, calls = self.run_with_stubs(
            "1", config_answers=runtime_answers, runner_args=["--verbosity", "debug"]
        )

        self.assertEqual(result.returncode, 0, f"{result.stderr}\n{result.stdout}")
        console = result.stdout + result.stderr
        self.assertIn("setup-output", console)
        self.assertIn("runtime-output", console)
        self.assertIn("setup-output", self.last_logs["setup.log"])
        self.assertIn("runtime-output", self.last_logs["runtime/run-test-run-runtime-1-1.log"])

    def test_setup_failure_is_logged_and_preserves_exit_status(self):
        result, calls = self.run_with_stubs("1", fail_setup_status=7)

        self.assertEqual(result.returncode, 7)
        self.assertEqual(calls, ["setup:runtime"])
        self.assertIn("Exit status: 7", self.last_logs["setup.log"])
        self.assertNotIn("runtime/run-test-run.log", self.last_logs)
        rows = list(csv.DictReader(io.StringIO(self.last_logs["overview.csv"])))
        self.assertEqual([row["status"] for row in rows], ["failed", "skipped"])
        self.assertIn("shared setup failed", rows[1]["problem"])

    def test_all_selection_uses_standard_order_after_one_setup(self):
        result, calls = self.run_with_stubs("6")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls[0], "setup:runtime,empty-block,startup,compliance,size")
        self.assertEqual(calls.count("runtime"), 250)
        self.assertEqual(calls.count("empty-block"), 720)
        runtime_args = [call for call in calls if call.startswith("args:runtime:")]
        self.assertEqual(len(runtime_args), 250)
        self.assertTrue(all("--reset-network" in call for call in runtime_args))
        runtime_workloads = {
            "counter": "--accounts 17 --tps 17 --duration 180 --wait-duration 900 --chain-id 12345",
            "dota": "--accounts 66 --tps 66 --duration 280 --wait-duration 900 --chain-id 12345",
            "uber": "--accounts 4 --tps 4 --duration 120 --wait-duration 900 --chain-id 12345",
            "uber_v2": "--accounts 4 --tps 4 --duration 120 --wait-duration 900 --chain-id 12345",
            "value-transfer": "--accounts 10 --tps 200 --duration 10 --wait-duration 900 --chain-id 12345",
        }
        for workload, args in runtime_workloads.items():
            matching_runs = [
                call for call in runtime_args
                if call.endswith(f"--workload-type exp/{workload}") and args in call
            ]
            self.assertEqual(len(matching_runs), 50, workload)
        runtime_run_ids = [call for call in calls if call.startswith("run-id:")]
        self.assertEqual(len(runtime_run_ids), 250)
        for total in (4, 8, 16, 32, 64):
            for mix in (
                "native-only", "wasix-only", "geth-only", "reth-only",
                "wasix-native-half", "wasix-reth-half", "wasix-geth-half",
                "native-reth-half", "native-geth-half", "all-quarter",
            ):
                profile = f"{total}-{mix}"
                self.assertEqual(
                    sum(run_id.startswith(f"run-id:test-run-runtime-{profile}-") for run_id in runtime_run_ids),
                    5,
                    profile,
                )
        self.assertTrue(any(
            "--linux-nodes 8 --wasix-nodes 8 --geth-nodes 8 --reth-nodes 8" in call
            for call in runtime_args
        ))
        empty_args = [call for call in calls if call.startswith("args:empty-block:")]
        self.assertEqual(len(empty_args), 720)
        self.assertEqual(
            {re.search(r"--counts (\d+)", call).group(1) for call in empty_args},
            {"0", "1", "5", "10", "20", "50", "100", "200", "500", "1000", "5000", "10000"},
        )
        submitted_counts = [re.search(r"--counts (\d+)", call).group(1) for call in empty_args]
        self.assertTrue(all(submitted_counts.count(count) == 60 for count in (
            "0", "1", "5", "10", "20", "50", "100", "200", "500", "1000", "5000", "10000",
        )))
        self.assertTrue(all("--receipt-timeout 300 --block-timeout 120" in call for call in empty_args))
        self.assertEqual(sum("--workload " not in call for call in empty_args), 144)
        self.assertTrue(any("--parallelism 5" in call for call in calls if call.startswith("args:compliance:")))
        self.assertIn(
            "args:startup:--configs geth,reth,nexus-linux,nexus-wasix-docker,nexus-wasmer --iterations 5",
            calls,
        )
        self.assertIn("5 iterations", result.stdout)
        self.assertIn("step 4 of 12: transaction spam (10 transactions)", result.stdout)
        rows = list(csv.DictReader(io.StringIO(self.last_logs["overview.csv"])))
        self.assertEqual(len(rows), 974)
        runtime_rows = [row for row in rows if row["experiment"].startswith("runtime/")]
        self.assertEqual(len(runtime_rows), 250)
        empty_rows = [row for row in rows if row["experiment"].startswith("empty-block/")]
        self.assertEqual(len(empty_rows), 720)
        for count in ("0", "1", "5", "10", "20", "50", "100", "200", "500", "1000", "5000", "10000"):
            for repeat in range(1, 4):
                suffix = f"-count-{count}-repeat-{repeat}"
                self.assertEqual(sum(row["run_id"].endswith(suffix) for row in empty_rows), 20)
        self.assertTrue(all(row["status"] == "passed" for row in rows))
        self.assertLess(calls.index("runtime"), calls.index("empty-block"))
        self.assertLess(calls.index("empty-block"), calls.index("startup"))
        self.assertIn("startup", calls)
        self.assertIn("compliance", calls)
        self.assertIn("size", calls)

    def test_test_preset_runs_each_experiment_variant_once_with_short_defaults(self):
        for selection in ("test", "8"):
            with self.subTest(selection=selection):
                result, calls = self.run_with_stubs(selection)

                self.assertEqual(result.returncode, 0, f"{result.stderr}\n{result.stdout}")
                self.assertEqual(calls[0], "setup:runtime,empty-block,startup,compliance,size")
                runtime_args = [call for call in calls if call.startswith("args:runtime:")]
                self.assertEqual(len(runtime_args), 5)
                runtime_workloads = {
                    "counter": "--accounts 17 --tps 1 --duration 10 --wait-duration 5 --chain-id 12345",
                    "dota": "--accounts 66 --tps 1 --duration 10 --wait-duration 5 --chain-id 12345",
                    "uber": "--accounts 4 --tps 1 --duration 10 --wait-duration 5 --chain-id 12345",
                    "uber_v2": "--accounts 4 --tps 1 --duration 10 --wait-duration 5 --chain-id 12345",
                    "value-transfer": "--accounts 10 --tps 1 --duration 10 --wait-duration 5 --chain-id 12345",
                }
                for call in runtime_args:
                    workload = call.rsplit("--workload-type exp/", 1)[1]
                    self.assertIn(runtime_workloads[workload], call)
                    self.assertIn(
                        "--reset-network --linux-nodes 1 --wasix-nodes 1 --geth-nodes 1 --reth-nodes 1",
                        call,
                    )
                self.assertEqual(
                    {call.rsplit("--workload-type exp/", 1)[1] for call in runtime_args},
                    set(runtime_workloads),
                )

                empty_args = [call for call in calls if call.startswith("args:empty-block:")]
                self.assertEqual(len(empty_args), 20)
                self.assertEqual(
                    {re.search(r"--client ([^ ]+)", call).group(1) for call in empty_args},
                    {"native", "wasix", "geth", "reth"},
                )
                self.assertEqual(
                    {re.search(r"--workload ([^ ]+)", call).group(1) for call in empty_args if "--workload " in call},
                    {"counter", "dota", "uber", "uber_v2"},
                )
                self.assertTrue(all("--counts 1" in call for call in empty_args))
                self.assertTrue(all("--blocks-per-count 1 --max-attempts 1 --receipt-timeout 60 --block-timeout 30" in call for call in empty_args))
                self.assertEqual(sum("--workload " not in call for call in empty_args), 4)

                self.assertIn(
                    "args:startup:--configs geth,reth,nexus-linux,nexus-wasix-docker,nexus-wasmer --iterations 1 --timeout 60",
                    calls,
                )
                self.assertIn(
                    "args:compliance:--clients wasix-w-eth --parallelism 5 --timeout 1800",
                    calls,
                )
                rows = list(csv.DictReader(io.StringIO(self.last_logs["overview.csv"])))
                self.assertEqual(len(rows), 29)
                self.assertTrue(all(row["status"] == "passed" for row in rows))

    def test_single_experiment_runtime_defaults_match_all_experiment_defaults(self):
        runtime_result, runtime_calls = self.run_with_stubs("1", config_answers="n\n")

        self.assertEqual(runtime_result.returncode, 0, runtime_result.stderr)
        runtime_args = [call for call in runtime_calls if call.startswith("args:runtime:")]
        self.assertEqual(len(runtime_args), 250)
        self.assertEqual(
            {call.rsplit("--workload-type exp/", 1)[1] for call in runtime_args},
            {"counter", "dota", "uber", "uber_v2", "value-transfer"},
        )
        self.assertEqual(sum(call.startswith("run-id:test-run-runtime-") for call in runtime_calls), 250)
        self.assertTrue(all("--reset-network" in call for call in runtime_args))

    def test_empty_block_defaults_run_every_client_with_distinct_run_ids(self):
        result, calls = self.run_with_stubs("2", config_answers="n\n")

        self.assertEqual(result.returncode, 0, result.stderr)
        empty_args = [call for call in calls if call.startswith("args:empty-block:")]
        self.assertEqual(len(empty_args), 720)
        self.assertEqual(
            {re.search(r"--counts (\d+)", call).group(1) for call in empty_args},
            {"0", "1", "5", "10", "20", "50", "100", "200", "500", "1000", "5000", "10000"},
        )
        self.assertTrue(all("--blocks-per-count 1 --max-attempts 2 --receipt-timeout 300" in call for call in empty_args))
        self.assertEqual(sum("--workload " not in call for call in empty_args), 144)
        self.assertIn(
            "empty-block/run-test-run-empty-block-native-value-transfer-count-0-repeat-1.log",
            self.last_logs,
        )
        self.assertIn(
            "Empty-block run (client: native; workload: value-transfer; step 1 of 12: empty-block baseline (0 transactions)) (repeat 1 of 3; client 1 of 4; workload 1 of 5) ---",
            result.stdout,
        )
        self.assertIn(
            "Empty-block progress: step 1 of 7 - transaction spam (10 transactions)",
            result.stdout,
        )
        self.assertNotIn("empty-block-output", result.stdout)

    def test_custom_empty_block_configuration_can_run_multiple_clients(self):
        answers = "y\ngeth,reth\n23456\n0,1,5\n2\n3\n30\n20\n0.5\nn\nnone\n"
        result, calls = self.run_with_stubs("2", config_answers=answers)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            [call for call in calls if call.startswith("args:empty-block:")],
            [
                "args:empty-block:--client geth --chain-id 23456 --counts 0,1,5 --blocks-per-count 2 --max-attempts 3 --receipt-timeout 30 --block-timeout 20 --poll-interval 0.5",
                "args:empty-block:--client reth --chain-id 23456 --counts 0,1,5 --blocks-per-count 2 --max-attempts 3 --receipt-timeout 30 --block-timeout 20 --poll-interval 0.5",
            ],
        )
        self.assertIn("empty-block/run-test-run-empty-block-geth.log", self.last_logs)
        self.assertIn("empty-block/run-test-run-empty-block-reth.log", self.last_logs)

    def test_custom_empty_block_configuration_forwards_contract_workload(self):
        answers = "y\ngeth\n23456\n1,5\n1\n2\n30\n20\n0.5\nn\nuber_v2\n"
        result, calls = self.run_with_stubs("2", config_answers=answers)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "args:empty-block:--client geth --chain-id 23456 --counts 1,5 --blocks-per-count 1 "
            "--max-attempts 2 --receipt-timeout 30 --block-timeout 20 --poll-interval 0.5 --workload uber_v2",
            calls,
        )

    def test_reset_network_option_is_forwarded_to_runtime_defaults(self):
        answers = "y\n2\n2\n0\n0\nexp/uber,exp/uber_v2\n4\n4\n300\n1800\n12345\n1\n"
        result, calls = self.run_with_stubs("1", config_answers=answers, runner_args=["--reset-network"])

        self.assertEqual(result.returncode, 0, f"{result.stderr}\n{result.stdout}")
        self.assertIn("--reset-network --workload-type exp/uber", "\n".join(calls))
        self.assertIn("--reset-network --workload-type exp/uber_v2", "\n".join(calls))

    def test_reset_network_option_is_forwarded_to_empty_block_defaults(self):
        answers = "y\nnative\n23456\n1\n1\n2\n30\n20\n1\nnone\n"
        result, calls = self.run_with_stubs("2", config_answers=answers, runner_args=["--reset-network"])

        self.assertEqual(result.returncode, 0, f"{result.stderr}\n{result.stdout}")
        self.assertEqual(
            [call for call in calls if call.startswith("args:empty-block:")],
            [
                "args:empty-block:--client native --chain-id 23456 --counts 1 --blocks-per-count 1 --max-attempts 2 --receipt-timeout 30 --block-timeout 20 --poll-interval 1 --reset-network",
            ],
        )

    def test_reset_network_option_is_forwarded_to_startup_defaults(self):
        result, calls = self.run_with_stubs("3", runner_args=["--reset-network"])

        self.assertEqual(result.returncode, 0, f"{result.stderr}\n{result.stdout}")
        self.assertTrue(any(call.startswith("args:startup:") and "--reset-network" in call for call in calls))

    def test_invalid_and_duplicate_selections_are_rejected_then_reprompted(self):
        answers = "y\nnative\n23456\n0\n1\n1\n30\n20\n1\nn\nnone\n"
        result, calls = self.run_with_stubs("0\n1,1\n2\n", config_answers=answers)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls[0], "setup:empty-block")
        self.assertEqual(calls.count("empty-block"), 1)
        self.assertEqual(
            [call for call in calls if call.startswith("args:empty-block:")],
            ["args:empty-block:--client native --chain-id 23456 --counts 0 --blocks-per-count 1 --max-attempts 1 --receipt-timeout 30 --block-timeout 20 --poll-interval 1"],
        )
        self.assertIn("Invalid selection", result.stderr)
        self.assertIn("selected more than once", result.stderr)

    def test_module_failures_are_recorded_and_later_experiments_continue(self):
        answers = "y\n2\n2\n0\n0\nexp/uber\n4\n4\n300\n1800\n12345\n1\nn\ny\nnative\n23456\n1\n1\n1\n30\n20\n1\nn\nnone\nn\n"
        result, calls = self.run_with_stubs("1,2,3", fail_module="empty-block", config_answers=answers)

        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls[0], "setup:runtime,empty-block,startup")
        self.assertEqual(calls.count("empty-block"), 1)
        self.assertIn("startup", calls)
        self.assertIn("continuing with the remaining experiments", result.stderr)
        empty_block_log = self.last_logs["empty-block/run-test-run-empty-block-native.log"]
        self.assertIn("empty-block-output", empty_block_log)
        self.assertIn("Exit status: 1", empty_block_log)
        rows = list(csv.DictReader(io.StringIO(self.last_logs["overview.csv"])))
        failed = [row for row in rows if row["status"] == "failed"]
        self.assertEqual(len(failed), 1)
        self.assertTrue(all("stubbed failure of empty-block" in row["problem"] for row in failed))
        self.assertTrue(any(row["experiment"] == "startup" and row["status"] == "passed" for row in rows))

    def test_full_suite_continues_after_an_empty_block_count_fails(self):
        result, calls = self.run_with_stubs("6", fail_empty_block_count="5")

        self.assertNotEqual(result.returncode, 0)
        empty_block_args = [call for call in calls if call.startswith("args:empty-block:")]
        self.assertEqual(len(empty_block_args), 720)
        self.assertIn("--counts 10000", empty_block_args[-1])
        self.assertIn("startup", calls)
        self.assertIn("compliance", calls)
        rows = list(csv.DictReader(io.StringIO(self.last_logs["overview.csv"])))
        failed = [row for row in rows if row["status"] == "failed"]
        self.assertEqual(len(failed), 60)
        self.assertTrue(all("/count-5/repeat-" in row["experiment"] for row in failed))
        self.assertTrue(any("/count-10000/repeat-" in row["experiment"] for row in rows))

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
        answers = "y\ngeth\n23456\n0,1,5\n2\n3\n30\n20\n0.5\nn\nnone\n"
        result, calls = self.run_with_stubs("2", config_answers=answers)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("setup-key-leaked", calls)
        self.assertIn("private-key-present", calls)
        self.assertTrue(any(call.startswith("args:empty-block:--client geth --chain-id 23456 --counts 0,1,5") for call in calls))

    def test_empty_block_defaults_to_prefunded_anvil_key_without_prompting(self):
        answers = "y\nnative\n23456\n1\n1\n2\n30\n20\n1\nn\nnone\n"
        result, calls = self.run_with_stubs("2", config_answers=answers, private_key="")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("private-key-default", calls)
        self.assertNotIn("Funded test account private key", result.stdout + result.stderr)
        self.assertNotIn("0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80", "".join(self.last_logs.values()))

    def test_custom_startup_configuration_forwards_options(self):
        answers = "y\ngeth,nexus-linux,nexus-wasmer\n2\n777\n30\n0.5\ngeth:test\nreth:test\nlinux:test\nwasix:test\n\nwasmer-test\n"
        result, calls = self.run_with_stubs("3", config_answers=answers)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(any(call.startswith("args:startup:--configs geth,nexus-linux,nexus-wasmer --iterations 2") for call in calls))
        self.assertTrue(any("--nexus-wasix-wasm" in call and "wasm32-wasmer-wasi/release/wasix_eth.wasi.wasm" in call for call in calls))
        self.assertTrue(any("--wasmer-bin wasmer-test" in call for call in calls))

    def test_startup_defaults_include_geth_and_host_wasix(self):
        result, calls = self.run_with_stubs("3", config_answers="n\n")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "Default startup configuration: geth,reth,nexus-linux,nexus-wasix-docker,nexus-wasmer;",
            result.stdout,
        )
        self.assertIn(
            "args:startup:--configs geth,reth,nexus-linux,nexus-wasix-docker,nexus-wasmer --iterations 5",
            calls,
        )

    def test_single_compliance_defaults_match_all_experiment_defaults(self):
        result, calls = self.run_with_stubs("4", config_answers="n\n")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(
            "args:compliance:--clients wasix-w-eth,wasix-r-eth,go-ethereum_v1.15.11,reth_v2.5.2 --parallelism 5 --timeout 3600",
            calls,
        )

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

    def test_evaluation_rejects_saved_results_without_a_success_status(self):
        result, calls = self.run_with_stubs(
            "7\n2", missing_run_status_for="empty-block"
        )

        self.assertEqual(result.returncode, 1)
        self.assertEqual(calls, [])
        self.assertIn("run_status.json", self.last_logs["empty-block/validation-1.log"])
        self.assertNotIn("empty-block-report-output", "".join(self.last_logs.values()))


if __name__ == "__main__":
    unittest.main()