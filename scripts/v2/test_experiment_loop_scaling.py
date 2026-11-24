import os
import shlex
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


HERE = Path(__file__).resolve().parent


class ExperimentLoopScalingTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which("bash"), "bash is required")
    def test_network_sizes_scale_each_default_ratio_to_requested_total(self):
        with tempfile.TemporaryDirectory(dir=HERE) as directory:
            root = Path(directory)
            loop = root / "run_experiments_loop.sh"
            shutil.copy(HERE / "run_experiments_loop.sh", loop)
            fake_runner = root / "run_experiment.sh"
            fake_runner.write_text(
                '#!/bin/bash\nprintf "%s\\n" "$*" >> "$TEST_RUN_LOG"\n',
                encoding="utf-8",
            )
            fake_runner.chmod(0o755)
            calls_log = root / "calls.log"
            git = shutil.which("git")
            git_bash = Path(git).parent.parent / "bin" / "bash.exe" if git and os.name == "nt" else None
            bash = str(git_bash) if git_bash and git_bash.exists() else shutil.which("bash")
            env = os.environ.copy()
            env["TEST_RUN_LOG"] = calls_log.as_posix()
            env["LOG_FILE"] = (root / "loop.log").as_posix()
            result = subprocess.run([bash, str(loop), "--network-sizes", "4"], cwd=root,
                                    text=True, capture_output=True, env=env, timeout=30)

            self.assertEqual(result.returncode, 0, result.stderr)
            calls = [shlex.split(line) for line in calls_log.read_text(encoding="utf-8").splitlines()]
            self.assertEqual(len(calls), 3 * 7)
            observed = set()
            for args in calls:
                counts = [int(args[args.index(option) + 1]) for option in
                          ("--linux-nodes", "--wasix-nodes", "--geth-nodes", "--reth-nodes")]
                self.assertEqual(sum(counts), 4)
                observed.add(tuple(counts))
            self.assertEqual(observed, {
                (4, 0, 0, 0), (0, 4, 0, 0), (0, 0, 4, 0), (2, 2, 0, 0),
                (0, 2, 2, 0), (2, 0, 2, 0), (1, 1, 2, 0),
            })

            result = subprocess.run(
                [bash, str(loop), "--ratios", "3 4 3 2", "--network-sizes", "4",
                 "--workloads", "exp/fifa 10 1 17.415"],
                cwd=root, text=True, capture_output=True, env=env, timeout=30,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            calls = [shlex.split(line) for line in calls_log.read_text(encoding="utf-8").splitlines()]
            self.assertEqual(len(calls), 3 * 7 + 1)
            fifa_args = calls[-1]
            self.assertEqual(fifa_args[fifa_args.index("--workload-type") + 1], "exp/fifa")
            fifa_counts = [int(fifa_args[fifa_args.index(option) + 1]) for option in
                           ("--linux-nodes", "--wasix-nodes", "--geth-nodes", "--reth-nodes")]
            self.assertEqual(fifa_counts, [1, 1, 1, 1])


if __name__ == "__main__":
    unittest.main()