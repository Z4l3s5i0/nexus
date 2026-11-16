import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import plot_metrics

HERE = Path(__file__).resolve().parent
EVALUATOR = HERE / 'evaluate_logs.py'


class AnalysisTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=HERE)
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.logs = self.root / 'logs_dump'
        self.logs.mkdir()
        self.metadata = self.root / 'metadata.json'
        self.metadata.write_text(json.dumps({'nodes': [
            {'index': 0, 'type': 'native', 'client': 'wasix-eth', 'rpc_port': 8545},
            {'index': 1, 'type': 'geth', 'client': 'geth', 'rpc_port': 8546},
            {'index': 2, 'type': 'wasix', 'client': 'wasix-eth', 'rpc_port': 8547},
        ]}), encoding='utf-8')

    def run_evaluator(self, *args):
        return subprocess.run([sys.executable, str(EVALUATOR), *map(str, args)],
                              cwd=self.root, text=True, capture_output=True)

    def test_metadata_filters_geth_and_marks_missing_data(self):
        event = '[2026-07-09T21:16:00.000Z] [EXP] BLOCK_EXEC_END hash=0x1 elapsed_ms=4\n'
        (self.logs / 'el-node-0_123_0.log').write_text(event)
        (self.logs / 'el-node-1_123_aggregated.log').write_text(event * 3)
        (self.logs / 'el-node-9_123_0.log').write_text(event * 4)
        (self.logs / 'el-node-0_456_0.log').write_text(event * 5)
        result = self.run_evaluator(self.logs, '123', self.metadata)
        self.assertEqual(result.returncode, 0, result.stderr)
        metrics = json.loads((self.root / 'experiment_results.json').read_text())
        self.assertEqual(metrics['total_stats']['blocks_executed'], 1)
        self.assertIsNone(metrics['inclusion_latency']['avg'])
        self.assertIsNone(metrics['execution_times']['avg_tx_exec_ms'])
        self.assertIn('el-node-1', result.stdout)
        self.assertIn('unavailable', result.stdout.lower())
        self.assertIn('el-node-9', result.stdout)
        self.assertIn('partial', result.stdout.lower())

    def test_metadata_absent_keeps_legacy_output(self):
        self.metadata.unlink()
        (self.logs / 'el-node-1_123_0.log').write_text('[EXP] BLOCK_EXEC_END hash=0x1 elapsed_ms=4\n')
        result = self.run_evaluator(self.logs, '123')
        self.assertEqual(result.returncode, 0, result.stderr)
        metrics = json.loads((self.root / 'experiment_results.json').read_text())
        self.assertEqual(metrics['total_stats']['blocks_executed'], 1)
        self.assertEqual(metrics['inclusion_latency']['avg'], 0)

    def test_invalid_metadata_rejected(self):
        self.metadata.write_text('{"nodes": [{"index": 0, "type": "unknown"}]}')
        result = self.run_evaluator(self.logs, '', self.metadata)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('metadata', (result.stdout + result.stderr).lower())

    def test_empty_logs_are_unavailable_not_zero(self):
        result = self.run_evaluator(self.logs)
        self.assertEqual(result.returncode, 0, result.stderr)
        metrics = json.loads((self.root / 'experiment_results.json').read_text())
        self.assertIsNone(metrics['total_stats']['blocks_executed'])
        self.assertIsNone(metrics['execution_times']['avg_block_exec_ms'])
        self.assertIn('Total Blocks Executed: unavailable', result.stdout)

    def test_storage_node_types(self):
        result = self.run_evaluator('--storage-node-types', self.metadata)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), ['0 native', '1 geth', '2 wasix'])

    def test_analysis_entry_point_skips_geth_storage(self):
        script_dir = self.root / 'scripts'
        script_dir.mkdir()
        shutil.copy(HERE / 'analyze_experiment.sh', script_dir)
        shutil.copy(EVALUATOR, script_dir)
        bin_dir = self.root / 'bin'
        bin_dir.mkdir()
        exporter = script_dir / 'db_exporter' / 'target' / 'release' / 'db_exporter'
        exporter.parent.mkdir(parents=True)
        exporter.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$TEST_EXPORT_LOG"\nprintf "[]\\n"\n')
        sudo = bin_dir / 'sudo'
        sudo.write_text('#!/bin/sh\nexec "$@"\n')
        python = bin_dir / 'python3'
        python.write_text('#!/bin/sh\nexec "' + Path(sys.executable).as_posix() + '" "$@"\n')
        for path in (exporter, sudo, python):
            path.chmod(0o755)
        for index in (0, 1, 9):
            el = self.root / 'data_v2' / f'node-{index}' / 'el'
            el.mkdir(parents=True)
            (el / 'eth.redb').touch()
        (self.logs / 'el-node-0_123_0.log').write_text('[EXP] BLOCK_EXEC_END hash=0x1 elapsed_ms=4\n')
        (self.logs / 'el-node-1_123_0.log').write_text('[EXP] BLOCK_EXEC_END hash=0x1 elapsed_ms=4\n')
        git = shutil.which('git')
        git_bash = Path(git).parent.parent / 'bin' / 'bash.exe' if git and os.name == 'nt' else None
        bash = str(git_bash) if git_bash and git_bash.exists() else shutil.which('bash')
        env = os.environ.copy()
        env['PATH'] = str(bin_dir) + os.pathsep + env['PATH']
        env['TEST_EXPORT_LOG'] = (self.root / 'export.log').as_posix()
        result = subprocess.run([bash, str(script_dir / 'analyze_experiment.sh'), str(self.root), '123'],
                                cwd=self.root, text=True, capture_output=True, env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = (self.root / 'export.log').read_text()
        self.assertIn('node-0', calls)
        self.assertNotIn('node-1', calls)
        self.assertNotIn('node-9', calls)
        self.assertIn('Geth storage', result.stdout)
        self.assertIn('node type unavailable', result.stdout)
        evaluation = (self.root / 'analysis' / 'log_evaluation.txt').read_text()
        self.assertIn('Total Blocks Executed: 1', evaluation)
        self.assertIn('unavailable: Geth', evaluation)

    def test_plot_series_use_rpc_and_unambiguous_registration(self):
        nodes = {node['index']: node for node in json.loads(self.metadata.read_text())['nodes']}
        registrations = [
            {'id': 'native-id', 'rpc_url': 'http://host.docker.internal:8545', 'chain_id': 123},
            {'id': 'geth-id', 'rpc_url': 'http://el-node-1:8545', 'chain_id': 123},
            {'id': 'wrong-chain', 'rpc_url': 'http://host:8547', 'chain_id': 456},
        ]
        samples = [
            {'node_id': 'native-id', 'name': 'blocks_imported_total', 'value': 12, 'timestamp_ms': 1000},
            {'node_id': 'geth-id', 'name': 'transactions_committed_total', 'value': 99, 'timestamp_ms': 1000, 'source': 'PrometheusScrape'},
            {'node_id': 'geth-id', 'name': 'ethereum_peer_count', 'value': 0, 'timestamp_ms': 1000, 'source': 'JsonRpc'},
            {'node_id': 'mystery', 'name': 'mempool_size', 'value': 1, 'timestamp_ms': 1000},
        ]
        observations = [
            {'node_id': 'geth-id', 'method': 'eth_blockNumber', 'value_json': '"0x2a"', 'timestamp_ms': 2000, 'success': True},
            {'node_id': 'geth-id', 'method': 'eth_syncing', 'value_json': 'false', 'timestamp_ms': 2100, 'success': True},
            {'node_id': 'geth-id', 'method': 'net_peerCount', 'value_json': None, 'timestamp_ms': 2200, 'success': False},
        ]
        mapping, grouped, unknown = plot_metrics.prepare_series(nodes, registrations, samples, observations, 123)
        self.assertEqual(mapping, {0: 'native-id', 1: 'geth-id'})
        self.assertEqual(grouped[1, 'ethereum_latest_block'], [(2000, 42)])
        self.assertEqual(grouped[1, 'ethereum_syncing'], [(2100, 0)])
        self.assertEqual(grouped[1, 'ethereum_peer_count'], [(1000, 0)])
        self.assertNotIn((1, 'transactions_committed_total'), grouped)
        self.assertEqual(unknown, ['mystery'])
        registrations.append({'id': 'other', 'rpc_url': 'http://host:8545', 'chain_id': 123})
        self.assertNotIn(0, plot_metrics.map_registered_nodes(nodes, registrations, 123))

    def test_single_node_plot_and_empty_plot(self):
        analysis = self.root / 'analysis'
        plots = self.root / 'plots'
        analysis.mkdir()
        env = os.environ.copy()
        env['MPLBACKEND'] = 'Agg'
        env['MPLCONFIGDIR'] = str(self.root / 'mplconfig')
        node = {'index': 0, 'type': 'geth', 'client': 'geth', 'rpc_port': 8545}
        self.metadata.write_text(json.dumps({'nodes': [node]}))
        (analysis / 'metrics_nodes.json').write_text(json.dumps([{'value': {'id': 'id', 'rpc_url': 'http://host:8545'}}]))
        (analysis / 'metrics_metric_samples.json').write_text(json.dumps([{'value': {
            'node_id': 'id', 'name': 'ethereum_latest_block', 'value': 42,
            'timestamp_ms': 1000, 'source': 'JsonRpc'}}]))
        result = subprocess.run([sys.executable, str(HERE / 'plot_metrics.py'), str(analysis), str(plots)],
                                text=True, capture_output=True, env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = (plots / 'metrics_coverage.txt').read_text()
        self.assertIn('ethereum_latest_block', report)
        self.assertIn('unavailable', report)
        self.assertTrue((plots / 'blocks_per_node.png').exists())
        (analysis / 'metrics_metric_samples.json').write_text('[]')
        result = subprocess.run([sys.executable, str(HERE / 'plot_metrics.py'), str(analysis), str(plots)],
                                text=True, capture_output=True, env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn(': available (ethereum_latest_block', (plots / 'metrics_coverage.txt').read_text())

    def test_legacy_single_node_plot(self):
        self.metadata.unlink()
        analysis = self.root / 'analysis'
        plots = self.root / 'plots'
        analysis.mkdir()
        (analysis / 'metrics_metric_samples.json').write_text(json.dumps([{'value': {
            'node_id': 'native-id', 'name': 'transactions_committed_total', 'value': 1,
            'timestamp_ms': 1000}}]))
        env = os.environ.copy()
        env['MPLBACKEND'] = 'Agg'
        env['MPLCONFIGDIR'] = str(self.root / 'mplconfig')
        result = subprocess.run([sys.executable, str(HERE / 'plot_metrics.py'), str(analysis), str(plots)],
                                text=True, capture_output=True, env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((plots / 'node_metrics_stacked_faceted.png').exists())

    def test_mixed_plot_labels_imported_blocks_as_approximate(self):
        analysis = self.root / 'analysis'
        plots = self.root / 'plots'
        analysis.mkdir()
        registrations = [{'value': {'id': 'native', 'rpc_url': 'http://host:8545'}},
                         {'value': {'id': 'geth', 'rpc_url': 'http://host:8546'}}]
        samples = [{'value': {'node_id': 'native', 'name': 'blocks_imported_total', 'value': 2, 'timestamp_ms': 1000}},
                   {'value': {'node_id': 'geth', 'name': 'ethereum_latest_block', 'value': 10, 'timestamp_ms': 2000, 'source': 'JsonRpc'}}]
        (analysis / 'metrics_nodes.json').write_text(json.dumps(registrations))
        (analysis / 'metrics_metric_samples.json').write_text(json.dumps(samples))
        env = os.environ.copy()
        env['MPLBACKEND'] = 'Agg'
        env['MPLCONFIGDIR'] = str(self.root / 'mplconfig')
        result = subprocess.run([sys.executable, str(HERE / 'plot_metrics.py'), str(analysis), str(plots)],
                                text=True, capture_output=True, env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = (plots / 'metrics_coverage.txt').read_text()
        self.assertIn('el-node-0 (native): available (blocks_imported_total; approximate', report)
        self.assertIn('el-node-1 (geth): available (ethereum_latest_block', report)
        self.assertIn('el-node-2 (wasix): unavailable', report)
        self.assertFalse((plots / 'transactions_committed_stacked_bar.png').exists())


if __name__ == '__main__':
    unittest.main()