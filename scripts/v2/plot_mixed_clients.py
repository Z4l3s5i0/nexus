"""
Mixed-client metrics analysis and plotting script.

Handles both wasix-eth and Geth clients natively by mapping each client's
Prometheus metric names to a common set of metric categories. Unlike the
metadata-aware path in plot_metrics.py (which uses only JsonRpc-sourced
metrics for Geth), this script uses Geth Prometheus-scraped metrics directly,
providing full coverage for all metric categories.

Usage:
    python plot_mixed_clients.py <experiment_analysis_dir> [output_dir]

Example:
    python scripts/v2/plot_mixed_clients.py exp/experiments/exp_20261117_170125/analysis plots_mixed
"""

import json
import math
import os
import re
import sys
from collections import defaultdict
from urllib.parse import urlparse

from evaluate_logs import load_metadata


# ---------------------------------------------------------------------------
# Metric name mapping: common name -> list of (client, source, metric_name)
# ---------------------------------------------------------------------------
# Each entry lists the possible metric names for a common metric, ordered by
# preference. The first match for a given (client, source) pair is used.
#
# client: "wasix-eth" | "geth"
# source: "PrometheusScrape" | "JsonRpc"
# ---------------------------------------------------------------------------

METRIC_ALIASES = {
    # --- Block height ---
    'block_height': [
        ('wasix-eth', 'PrometheusScrape', 'current_head_block'),
        ('wasix-eth', 'PrometheusScrape', 'blocks_imported_total'),
        ('geth', 'PrometheusScrape', 'chain_head_block'),
        ('*', 'JsonRpc', 'ethereum_latest_block'),
    ],
    # --- Mempool size ---
    'mempool_size': [
        ('wasix-eth', 'PrometheusScrape', 'mempool_size'),
        ('geth', 'PrometheusScrape', 'txpool_pending'),
    ],
    # --- Rejected transactions ---
    'rejected_transactions': [
        ('wasix-eth', 'PrometheusScrape', 'mempool_rejected_transactions_total'),
        ('geth', 'PrometheusScrape', 'txpool_invalid'),
    ],
    # --- Evicted transactions ---
    'evicted_transactions': [
        ('wasix-eth', 'PrometheusScrape', 'evicted_transactions_total'),
        ('geth', 'PrometheusScrape', 'txpool_pending_discard'),
        ('geth', 'PrometheusScrape', 'txpool_queued_eviction'),
    ],
    # --- Sync status ---
    'sync_status': [
        ('wasix-eth', 'PrometheusScrape', 'sync_status'),
        ('*', 'JsonRpc', 'ethereum_syncing'),
    ],
    # --- Connected peers ---
    'connected_peers': [
        ('wasix-eth', 'PrometheusScrape', 'connected_peers'),
        ('geth', 'PrometheusScrape', 'p2p_peers'),
        ('*', 'JsonRpc', 'ethereum_peer_count'),
    ],
    # --- Gossip messages received ---
    'gossip_messages': [
        ('wasix-eth', 'PrometheusScrape', 'gossip_messages_received_total'),
        # Geth: NewPooledTransactionHashes (0x08) packets received
        ('geth', 'PrometheusScrape', 'p2p_ingress_eth_68_0x08_packets'),
        # Fallback: Transactions (0x02) packets received
        ('geth', 'PrometheusScrape', 'p2p_ingress_eth_68_0x02_packets'),
    ],
    # --- P2P bytes sent ---
    'p2p_bytes_sent': [
        ('wasix-eth', 'PrometheusScrape', 'p2p_messages_sent_bytes_total'),
        ('geth', 'PrometheusScrape', 'p2p_egress'),
    ],
    # --- P2P bytes received ---
    'p2p_bytes_received': [
        ('geth', 'PrometheusScrape', 'p2p_ingress'),
    ],
    # --- RPC requests ---
    'rpc_requests': [
        ('wasix-eth', 'PrometheusScrape', 'rpc_requests_total'),
    ],
    # --- Transactions committed ---
    'transactions_committed': [
        ('wasix-eth', 'PrometheusScrape', 'transactions_committed_total'),
        # Geth: derived from txpool_valid - txpool_pending (see prepare_series)
    ],
}

# Geth-derived metrics: components needed for computed metrics.
# Each key is a common metric key; value is a list of (client, source, component_name)
# that must be collected before derivation.
GETH_DERIVED_COMPONENTS = {
    'transactions_committed': [
        ('geth', 'PrometheusScrape', 'txpool_valid'),
        ('geth', 'PrometheusScrape', 'txpool_pending'),
    ],
}


# The list of metrics to plot, in order.
# Each tuple: (display_title, output_filename, metric_key)
METRICS_TO_PLOT = [
    ('Block height', 'block_height_per_node.png', 'block_height'),
    ('Mempool size', 'mempool_size_per_node.png', 'mempool_size'),
    ('Rejected transactions', 'rejected_transactions_per_node.png', 'rejected_transactions'),
    ('Evicted transactions', 'evicted_transactions_per_node.png', 'evicted_transactions'),
    ('Sync status (1=syncing, 0=synced)', 'sync_status_per_node.png', 'sync_status'),
    ('Connected peers', 'connected_peers_per_node.png', 'connected_peers'),
    ('Gossip messages received', 'gossip_messages_per_node.png', 'gossip_messages'),
    ('P2P bytes sent', 'p2p_bytes_sent_per_node.png', 'p2p_bytes_sent'),
    ('P2P bytes received', 'p2p_bytes_received_per_node.png', 'p2p_bytes_received'),
    ('RPC requests', 'rpc_requests_per_node.png', 'rpc_requests'),
    ('Transactions committed', 'transactions_committed_per_node.png', 'transactions_committed'),
]


def load_export(path):
    """Load a JSON export file and return the list of value objects."""
    if not os.path.exists(path):
        return []
    with open(path, encoding='utf-8') as f:
        entries = json.load(f)
    if not isinstance(entries, list):
        raise ValueError(f'Invalid export: {path}')
    return [entry['value'] for entry in entries if isinstance(entry, dict) and isinstance(entry.get('value'), dict)]


def map_registered_nodes(nodes, registrations, chain_id=None):
    """Map node registrations to node indices."""
    candidates = {}
    for registration in registrations:
        if chain_id is not None and registration.get('chain_id') != chain_id:
            continue
        try:
            url = urlparse(registration.get('rpc_url') or '')
            host_match = re.fullmatch(r'el-node-(\d+)', url.hostname or '')
            if host_match:
                index = int(host_match.group(1))
                if index not in nodes:
                    continue
            else:
                matches = [i for i, node in nodes.items() if node.get('rpc_port') == url.port]
                if len(matches) != 1:
                    continue
                index = matches[0]
            node_id = registration.get('id')
            if isinstance(node_id, str) and node_id:
                candidates.setdefault(index, set()).add(node_id)
        except ValueError:
            continue
    counts = {}
    for ids in candidates.values():
        for node_id in ids:
            counts[node_id] = counts.get(node_id, 0) + 1
    return {index: next(iter(ids)) for index, ids in candidates.items()
            if len(ids) == 1 and counts[next(iter(ids))] == 1}


def resolve_metric_name(client, source, aliases):
    """
    Given a client type and data source, find the matching metric name from
    the alias list. Returns the first matching metric name or None.
    """
    for alias_client, alias_source, metric_name in aliases:
        if (alias_client == '*' or alias_client == client) and alias_source == source:
            return metric_name
    return None


def prepare_series(nodes, registrations, samples, observations, chain_id=None):
    """
    Group metric samples by (node_index, common_metric_key) using client-aware
    name resolution. Returns (mapping, grouped, unknown_ids, coverage_info).
    
    coverage_info: dict of (node_index, metric_key) -> (resolved_name, sample_count)
    """
    mapping = map_registered_nodes(nodes, registrations, chain_id)
    by_id = {node_id: index for index, node_id in mapping.items()}
    
    grouped = defaultdict(list)
    unknown_ids = set()
    coverage_info = {}
    
    # Build a lookup: (client, source) -> metric_name for each common key
    alias_lookup = {}
    for metric_key, aliases in METRIC_ALIASES.items():
        for client, source, name in aliases:
            alias_lookup.setdefault((metric_key, client, source), []).append(name)
    
    for sample in samples:
        node_id = sample.get('node_id')
        if node_id not in by_id:
            unknown_ids.add(str(node_id))
            continue
        
        index = by_id[node_id]
        node_type = nodes[index]['type']  # 'native', 'geth', 'wasix'
        raw_name = sample.get('name')
        source = sample.get('source', '')
        timestamp = sample.get('timestamp_ms')
        value = sample.get('value')
        
        # Validate and convert timestamp and value
        if type(timestamp) not in (int, float):
            try:
                timestamp = float(timestamp)
            except (ValueError, TypeError):
                continue
        if type(value) not in (int, float):
            try:
                value = float(value)
            except (ValueError, TypeError):
                continue
        if not math.isfinite(timestamp) or not math.isfinite(value):
            continue
        
        # Determine client type for alias resolution
        # 'native' and 'wasix' both map to 'wasix-eth' client
        if node_type in ('native', 'wasix'):
            client = 'wasix-eth'
        else:
            client = node_type
        
        # Try to match this sample's raw_name to a common metric key
        matched_key = None
        for metric_key, aliases in METRIC_ALIASES.items():
            for alias_client, alias_source, alias_name in aliases:
                if alias_name == raw_name:
                    if (alias_client == '*' or alias_client == client) and alias_source == source:
                        matched_key = metric_key
                        break
            if matched_key:
                break
        
        if matched_key is None:
            continue
        
        key = (index, matched_key)
        grouped[key].append((timestamp, value))
        
        # Track coverage
        if key not in coverage_info:
            coverage_info[key] = (raw_name, 0)
        coverage_info[key] = (raw_name, coverage_info[key][1] + 1)
    
    # --- Derive Geth metrics from component gauges ---
    # For transactions_committed: txpool_valid - txpool_pending
    # Collect per-node component series, then compute the derived metric.
    for metric_key, components in GETH_DERIVED_COMPONENTS.items():
        # Build a per-node lookup: node_index -> {component_name: [(ts, val)]}
        node_components = defaultdict(dict)
        for comp_client, comp_source, comp_name in components:
            for sample in samples:
                node_id = sample.get('node_id')
                if node_id not in by_id:
                    continue
                index = by_id[node_id]
                node_type = nodes[index]['type']
                if node_type != comp_client:
                    continue
                if sample.get('name') != comp_name or sample.get('source', '') != comp_source:
                    continue
                timestamp = sample.get('timestamp_ms')
                value = sample.get('value')
                if type(timestamp) not in (int, float):
                    try:
                        timestamp = float(timestamp)
                    except (ValueError, TypeError):
                        continue
                if type(value) not in (int, float):
                    try:
                        value = float(value)
                    except (ValueError, TypeError):
                        continue
                if not math.isfinite(timestamp) or not math.isfinite(value):
                    continue
                if comp_name not in node_components[index]:
                    node_components[index][comp_name] = []
                node_components[index][comp_name].append((timestamp, value))

        # Compute derived values for each node that has all components
        for index, comps in node_components.items():
            if len(comps) != len(components):
                continue
            # Build timestamp-indexed values for each component
            comp_by_ts = {}
            for comp_client, comp_source, comp_name in components:
                comp_by_ts[comp_name] = {ts: val for ts, val in comps.get(comp_name, [])}
            # Find all timestamps where all components are available
            all_ts = set()
            for comp_name in comp_by_ts:
                all_ts.update(comp_by_ts[comp_name].keys())
            points = []
            for ts in all_ts:
                vals = []
                complete = True
                for comp_client, comp_source, comp_name in components:
                    if ts not in comp_by_ts[comp_name]:
                        complete = False
                        break
                    vals.append(comp_by_ts[comp_name][ts])
                if not complete:
                    continue
                # transactions_committed = txpool_valid - txpool_pending
                derived_val = vals[0] - vals[1]
                points.append((ts, derived_val))
            if points:
                points.sort()
                key = (index, metric_key)
                grouped[key].extend(points)
                # Track coverage
                if key not in coverage_info:
                    coverage_info[key] = ('txpool_valid - txpool_pending', 0)
                coverage_info[key] = (coverage_info[key][0], coverage_info[key][1] + len(points))

    # Re-sort after adding derived points
    for key in grouped:
        grouped[key].sort()

    # Also incorporate JsonRpc observations for shared metrics
    methods = {
        'eth_blockNumber': 'block_height',
        'eth_syncing': 'sync_status',
        'net_peerCount': 'connected_peers',
    }
    for obs in observations:
        if obs.get('node_id') not in by_id:
            unknown_ids.add(str(obs.get('node_id')))
            continue
        if not obs.get('success') or obs.get('method') not in methods:
            continue
        
        index = by_id[obs['node_id']]
        metric_key = methods[obs['method']]
        timestamp = obs.get('timestamp_ms')
        
        try:
            value_json = json.loads(obs['value_json'])
            if metric_key == 'sync_status':
                if isinstance(value_json, bool):
                    value = 1.0 if value_json else 0.0
                elif isinstance(value_json, dict):
                    value = 1.0
                else:
                    continue
            elif metric_key == 'block_height':
                if isinstance(value_json, str):
                    value = float(int(value_json, 16))
                elif isinstance(value_json, (int, float)) and value_json >= 0:
                    value = float(value_json)
                else:
                    continue
            elif metric_key == 'connected_peers':
                if isinstance(value_json, str):
                    value = float(int(value_json, 16))
                elif isinstance(value_json, (int, float)) and value_json >= 0:
                    value = float(value_json)
                else:
                    continue
            else:
                continue
            
            if type(timestamp) not in (int, float) or not math.isfinite(timestamp):
                continue
        except (ValueError, TypeError, KeyError):
            continue
        
        key = (index, metric_key)
        grouped[key].append((timestamp, value))
        
        # Track coverage for JsonRpc observations
        if key not in coverage_info:
            coverage_info[key] = ('JsonRpc', 0)
        coverage_info[key] = (coverage_info[key][0], coverage_info[key][1] + 1)
    
    # Sort all series by timestamp
    for key in grouped:
        grouped[key].sort()
    
    return mapping, dict(grouped), sorted(unknown_ids), coverage_info


def plot_with_metadata(analysis_dir, output_dir, metadata_path):
    """Main plotting function that handles mixed-client experiments."""
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    import matplotlib.colors as mcolors

    nodes = load_metadata(metadata_path)
    with open(metadata_path, encoding='utf-8') as f:
        metadata = json.load(f)
    
    registrations = load_export(os.path.join(analysis_dir, 'metrics_nodes.json'))
    samples = load_export(os.path.join(analysis_dir, 'metrics_metric_samples.json'))
    observations = load_export(os.path.join(analysis_dir, 'metrics_rpc_observations.json'))
    
    mapping, grouped, unknown_ids, coverage_info = prepare_series(
        nodes, registrations, samples, observations, metadata.get('chain_id')
    )
    
    # Compute origin timestamp
    all_timestamps = [ts for points in grouped.values() for ts, _ in points]
    origin = min(all_timestamps) if all_timestamps else 0
    
    # Build report
    report = []
    report.append('Mixed-client metrics analysis')
    report.append('=' * 60)
    report.append(f'Nodes: {len(nodes)}')
    for idx, node in sorted(nodes.items()):
        report.append(f'  el-node-{idx}: {node["type"]}')
    report.append('')
    report.append('Note: unavailable series are not zero; no stacked totals are calculated.')
    report.append('Note: Geth transactions_committed is derived as txpool_valid - txpool_pending.')
    report.append('Note: Geth gossip messages uses p2p_ingress_eth_68_0x08_packets (NewPooledTransactionHashes).')
    report.append('Note: Lines connect recorded samples; gaps between observations are not measurements.')
    report.append('')
    if unknown_ids:
        report.append(f'Unmapped metric node IDs: {", ".join(unknown_ids)}')
    report.append('')
    
    # Color and style helpers for distinguishing nodes
    cmap = matplotlib.colormaps['tab20']
    line_styles = ['-', '--', ':', '-.']
    
    # Track per-node coverage for the summary
    node_coverage = defaultdict(lambda: defaultdict(int))
    
    for title, filename, metric_key in METRICS_TO_PLOT:
        fig, ax = plt.subplots(figsize=(12, 6))
        report.append(title + ':')
        
        has_data = False
        for index, node in sorted(nodes.items()):
            label = f"el-node-{index} ({node['type']})"
            key = (index, metric_key)
            
            if key in grouped:
                points = grouped[key]
                resolved_name, sample_count = coverage_info.get(key, ('unknown', 0))
                
                detail = f'{resolved_name} ({sample_count} samples)'
                if metric_key == 'transactions_committed' and node['type'] == 'geth':
                    detail += '; derived (txpool_valid - txpool_pending)'
                elif metric_key == 'block_height' and resolved_name == 'blocks_imported_total':
                    detail += '; approximate, not head height'
                
                c = cmap(index % 20)
                ls = line_styles[index % len(line_styles)]
                ax.plot(
                    [(ts - origin) / 1000 for ts, value in points],
                    [value for ts, value in points],
                    marker='.', markersize=3, linewidth=1.5,
                    color=c, linestyle=ls,
                    label=f'{label}: {detail}'
                )
                has_data = True
                node_coverage[index][metric_key] = sample_count
                report.append(f'  {label}: available ({detail})')
            else:
                # Check if node is registered but has no data
                if index in mapping:
                    reason = 'no compatible metric observations'
                else:
                    reason = 'node registration unavailable'
                ax.plot([], [], label=f'{label}: unavailable')
                report.append(f'  {label}: unavailable ({reason})')
        
        if not has_data:
            report.append('  (no data available for any node)')
        
        ax.set_title(title)
        ax.set_xlabel('Time since first observation (seconds)')
        ax.set_ylabel(title.split(' (')[0].split(' ')[0])  # Short y-axis label
        ax.grid(True, alpha=0.3)
        ax.legend(loc='upper left', bbox_to_anchor=(1.05, 1), fontsize=8)
        fig.tight_layout()
        fig.savefig(os.path.join(output_dir, filename), dpi=150)
        plt.close(fig)
        report.append('')
    
    # Write coverage report
    report.append('Coverage summary:')
    report.append('-' * 40)
    for index, node in sorted(nodes.items()):
        node_key = f'el-node-{index} ({node["type"]})'
        metrics_available = node_coverage[index]
        total_metrics = len(METRICS_TO_PLOT)
        report.append(f'  {node_key}: {len(metrics_available)}/{total_metrics} metrics with data')
        for metric_key, count in sorted(metrics_available.items()):
            report.append(f'    - {metric_key}: {count} samples')
    
    report_path = os.path.join(output_dir, 'metrics_coverage.txt')
    with open(report_path, 'w', encoding='utf-8') as f:
        f.write('\n'.join(report) + '\n')
    
    print('\n'.join(report))
    print(f'\nCoverage report saved to {report_path}')
    print(f'Plots saved to {output_dir}/')


def main():
    if len(sys.argv) < 2:
        print("Usage: python plot_mixed_clients.py <experiment_analysis_dir> [output_dir]")
        print()
        print("Analyzes and plots metrics from mixed-client experiments (wasix-eth + Geth).")
        print("Uses Geth Prometheus-scraped metrics directly for full coverage.")
        sys.exit(1)

    analysis_dir = sys.argv[1]
    output_dir = sys.argv[2] if len(sys.argv) > 2 else os.path.join(analysis_dir, 'plots_mixed')

    if not os.path.exists(analysis_dir):
        print(f"Analysis directory not found: {analysis_dir}", file=sys.stderr)
        sys.exit(1)

    if not os.path.exists(output_dir):
        os.makedirs(output_dir)

    # Look for metadata.json in the parent directory of the analysis dir
    metadata_path = os.path.join(os.path.dirname(os.path.abspath(analysis_dir)), 'metadata.json')
    if not os.path.exists(metadata_path):
        print(f"Metadata file not found: {metadata_path}", file=sys.stderr)
        sys.exit(1)

    try:
        plot_with_metadata(analysis_dir, output_dir, metadata_path)
    except (ValueError, OSError, KeyError, TypeError) as exc:
        print(f'Error reading metadata or metrics: {exc}', file=sys.stderr)
        import traceback
        traceback.print_exc()
        sys.exit(2)


if __name__ == '__main__':
    main()
