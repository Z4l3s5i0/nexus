import os
import re
import sys
import json
from collections import defaultdict
from datetime import datetime

def load_metadata(path):
    try:
        with open(path, encoding='utf-8') as f:
            metadata = json.load(f)
        nodes = metadata['nodes']
        if not isinstance(nodes, list) or not nodes:
            raise ValueError('nodes must be a non-empty array')
        result = {}
        for node in nodes:
            index = node['index']
            if (type(index) is not int or index < 0 or index in result or
                    node['type'] not in ('native', 'geth', 'wasix')):
                raise ValueError('invalid or duplicate node index/type')
            result[index] = node
        return result
    except (OSError, ValueError, TypeError, KeyError) as exc:
        raise ValueError(f'Invalid metadata {path}: {exc}') from exc

def parse_log_line(line):
    # Regex to match: [EXP] EVENT_NAME key1=value1 key2=value2 ...
    # Note: Some values might be in quotes like hash="0x..."
    # Improved pattern to handle [EXP] preceded by timestamp/metadata and more robust kv parsing
    match = re.search(r'\[EXP\]\s+(\w+)\s+(.*)', line)
    if not match:
        return None
    
    event_name = match.group(1)
    kv_str = match.group(2)
    
    # Extract key-value pairs
    kv_pairs = {}
    # Handles key=value and key="value"
    # Improved pattern to handle various value formats: hex, numbers, strings in quotes, etc.
    pattern = r'(\w+)=({[^}]+}|"[^"]*"|0x[a-fA-F0-0]+|\S+)'
    for k, v in re.findall(pattern, kv_str):
        # Remove quotes if present
        if v.startswith('"') and v.endswith('"'):
            v = v[1:-1]
        kv_pairs[k] = v
        
    return event_name, kv_pairs

def process_logs(log_dir, target_chain_id=None, nodes=None):
    tx_recv_times = defaultdict(dict)  # tx_hash -> {peer_id: timestamp}
    tx_exec_times = {}                 # tx_hash -> timestamp
    block_recv_times = defaultdict(dict) # block_hash -> {peer_id: timestamp}
    block_exec_times = {}                # block_hash -> timestamp
    block_txs = {}                       # block_hash -> [tx_hashes]
    
    node_stats = defaultdict(lambda: {
        'tx_received': 0,
        'tx_sent': 0,
        'blocks_received': 0,
        'blocks_sent': 0,
        'blocks_executed': 0,
        'tx_executed': 0,
        'total_gas': 0,
        'total_tx_exec_ms': 0,
        'total_block_exec_ms': 0
    })
    measurements = defaultdict(lambda: defaultdict(int))
    unknown_nodes = set()

    # Sort files to process them in chronological order per node
    all_files = sorted([f for f in os.listdir(log_dir) if f.endswith('.log')])
    files_to_process = []
    
    for filename in all_files:
        # Assuming filename format: {container_name}_{chain_id}_{file_number}.log
        parts = filename.split('_')
        if len(parts) < 2:
            continue
            
        node_id = parts[0]
        file_chain_id = parts[1]
        
        if target_chain_id and file_chain_id != target_chain_id:
            continue

        if nodes is not None:
            match = re.fullmatch(r'el-node-(\d+)', node_id)
            if not match:
                continue
            index = int(match.group(1))
            if index not in nodes:
                unknown_nodes.add(node_id)
                continue
            if nodes[index]['type'] == 'geth':
                continue
            
        files_to_process.append((filename, node_id))
    
    for filename, node_id in files_to_process:
        filepath = os.path.join(log_dir, filename)
        with open(filepath, 'r', errors='ignore') as f:
            for line in f:
                # Basic timestamp extraction if present in the log line
                # Standard log format often has timestamps at the beginning
    # [2026-07-09 21:16:00,123] or [2026-07-09T21:16:00.123Z] or Jul 10 08:16:07.859
                ts_match = re.search(r'(?:\[(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}[.,]\d+Z?)\]|^([A-Z][a-z]{2}\s+\d+\s+\d{2}:\d{2}:\d{2}\.\d{3}))', line)
                if ts_match:
                    timestamp = ts_match.group(1) or ts_match.group(2)
                else:
                    timestamp = None
                
                parsed = parse_log_line(line)
                if not parsed:
                    continue
                
                event, data = parsed
                if nodes is not None:
                    event_fields = {
                        'P2P_RECV_TX': 'tx_received', 'TX_RPC_RECV': 'tx_received',
                        'P2P_SEND_TX': 'tx_sent', 'P2P_RECV_BLOCK': 'blocks_received',
                        'P2P_SEND_BLOCK': 'blocks_sent', 'BLOCK_EXEC_END': 'blocks_executed',
                        'TX_EXEC_START': 'tx_executed',
                    }
                    if event in event_fields:
                        measurements[node_id][event_fields[event]] += 1
                    for field, event_name, key in (
                        ('total_gas', 'TX_EXEC_END', 'gas_used'),
                        ('total_tx_exec_ms', 'TX_EXEC_END', 'elapsed_ms'),
                        ('total_block_exec_ms', 'BLOCK_EXEC_END', 'elapsed_ms'),
                    ):
                        if event == event_name and key in data:
                            measurements[node_id][field] += 1
                
                if event in ['P2P_RECV_TX', 'TX_RPC_RECV']:
                    tx_hash = data.get('hash')
                    node_stats[node_id]['tx_received'] += 1
                    if tx_hash and timestamp:
                        if tx_hash not in tx_recv_times:
                            tx_recv_times[tx_hash]['first_seen'] = timestamp
                        tx_recv_times[tx_hash][node_id] = timestamp
                        
                elif event == 'P2P_SEND_TX':
                    node_stats[node_id]['tx_sent'] += 1
                    
                elif event == 'TX_EXEC_START':
                    tx_hash = data.get('hash')
                    node_stats[node_id]['tx_executed'] += 1
                    if tx_hash and timestamp:
                        if tx_hash not in tx_exec_times:
                            tx_exec_times[tx_hash] = timestamp
                        # If we never saw this TX via P2P (e.g. it came in a block or was local),
                        # use its execution time as a fallback for 'first_seen'
                        if tx_hash not in tx_recv_times:
                            tx_recv_times[tx_hash]['first_seen'] = timestamp
                            
                elif event == 'TX_EXEC_END':
                    node_stats[node_id]['total_gas'] += int(data.get('gas_used', 0))
                    node_stats[node_id]['total_tx_exec_ms'] += int(data.get('elapsed_ms', 0))

                elif event in ['P2P_RECV_BLOCK', 'BLOCK_RPC_RECV']:
                    block_hash = data.get('hash')
                    if event == 'P2P_RECV_BLOCK':
                        node_stats[node_id]['blocks_received'] += 1
                    if block_hash and timestamp:
                        if block_hash not in block_recv_times:
                            block_recv_times[block_hash]['first_seen'] = timestamp
                        block_recv_times[block_hash][node_id] = timestamp

                elif event == 'P2P_SEND_BLOCK':
                    node_stats[node_id]['blocks_sent'] += 1

                elif event == 'BLOCK_EXEC_START':
                    block_hash = data.get('hash')
                    if block_hash and timestamp:
                        # Fallback for block first_seen if we didn't get P2P_RECV_BLOCK
                        if block_hash not in block_recv_times:
                            block_recv_times[block_hash]['first_seen'] = timestamp

                elif event == 'BLOCK_EXEC_END':
                    block_hash = data.get('hash')
                    node_stats[node_id]['blocks_executed'] += 1
                    node_stats[node_id]['total_block_exec_ms'] += int(data.get('elapsed_ms', 0))
                    if block_hash and timestamp:
                        block_exec_times[block_hash] = timestamp

    return {
        'node_stats': node_stats,
        'tx_recv_times': tx_recv_times,
        'tx_exec_times': tx_exec_times,
        'block_recv_times': block_recv_times,
        'block_exec_times': block_exec_times,
        'measurements': measurements,
        'unknown_nodes': sorted(unknown_nodes),
        'metadata_nodes': nodes,
    }

def calculate_metrics(data):
    def parse_ts(ts_str):
        # Handle format like Jul 10 08:16:07.859
        if re.match(r'^[A-Z][a-z]{2}\s+\d+\s+\d{2}:\d{2}:\d{2}\.\d{3}$', ts_str):
            # Assume current year if not provided
            current_year = 2026
            return datetime.strptime(f"{current_year} {ts_str}", "%Y %b %d %H:%M:%S.%f")

        ts_str = ts_str.replace(',', '.').replace(' ', 'T')
        if not ts_str.endswith('Z'):
            ts_str += 'Z'
        try:
            return datetime.strptime(ts_str, "%Y-%m-%dT%H:%M:%S.%fZ")
        except:
            # Try without milliseconds if needed
            return datetime.strptime(ts_str.split('.')[0] + 'Z', "%Y-%m-%dT%H:%M:%SZ")

    # 1. Inclusion Latency (First Seen -> Executed)
    inclusion_latencies = []
    inclusion_timeseries = [] # list of (timestamp, latency)
    for tx_hash, exec_ts_str in data['tx_exec_times'].items():
        if tx_hash in data['tx_recv_times'] and 'first_seen' in data['tx_recv_times'][tx_hash]:
            start = parse_ts(data['tx_recv_times'][tx_hash]['first_seen'])
            end = parse_ts(exec_ts_str)
            latency = (end - start).total_seconds()
            if latency >= 0:
                inclusion_latencies.append(latency)
                inclusion_timeseries.append((exec_ts_str, latency))

    # 2. Block Propagation Latency
    propagation_latencies = []
    propagation_timeseries = [] # list of (timestamp, latency)
    for block_hash, nodes in data['block_recv_times'].items():
        if 'first_seen' in nodes:
            first_ts_str = nodes['first_seen']
            first_ts = parse_ts(first_ts_str)
            for node_id, ts_str in nodes.items():
                if node_id == 'first_seen': continue
                lat = (parse_ts(ts_str) - first_ts).total_seconds()
                if lat >= 0:  # Changed from > 0 to >= 0
                    propagation_latencies.append(lat)
                    propagation_timeseries.append((ts_str, lat))

    # Summarize Node Stats
    total_tx_exec = sum(s['tx_executed'] for s in data['node_stats'].values())
    total_blocks = sum(s['blocks_executed'] for s in data['node_stats'].values())
    total_tx_exec_ms = sum(s['total_tx_exec_ms'] for s in data['node_stats'].values())
    total_block_exec_ms = sum(s['total_block_exec_ms'] for s in data['node_stats'].values())
    
    results = {
        'inclusion_latency': {
            'avg': sum(inclusion_latencies) / len(inclusion_latencies) if inclusion_latencies else 0,
            'min': min(inclusion_latencies) if inclusion_latencies else 0,
            'max': max(inclusion_latencies) if inclusion_latencies else 0,
            'count': len(inclusion_latencies),
            'timeseries': inclusion_timeseries
        },
        'propagation_latency': {
            'avg': sum(propagation_latencies) / len(propagation_latencies) if propagation_latencies else 0,
            'max': max(propagation_latencies) if propagation_latencies else 0,
            'count': len(propagation_latencies),
            'timeseries': propagation_timeseries
        },
        'execution_times': {
            'avg_tx_exec_ms': total_tx_exec_ms / total_tx_exec if total_tx_exec > 0 else 0,
            'avg_block_exec_ms': total_block_exec_ms / total_blocks if total_blocks > 0 else 0,
        },
        'total_stats': {
            'transactions_executed': total_tx_exec,
            'blocks_executed': total_blocks,
            'total_tx_exec_ms': total_tx_exec_ms,
            'total_block_exec_ms': total_block_exec_ms,
        },
        'node_breakdown': {node: stats for node, stats in data['node_stats'].items()}
    }

    if data['metadata_nodes'] is not None:
        measurements = data['measurements']
        for section in ('inclusion_latency', 'propagation_latency'):
            if not results[section]['count']:
                for key in ('avg', 'min', 'max'):
                    if key in results[section]:
                        results[section][key] = None
        for result_key, field in (('avg_tx_exec_ms', 'total_tx_exec_ms'),
                                  ('avg_block_exec_ms', 'total_block_exec_ms')):
            count = sum(m[field] for m in measurements.values())
            total = sum(s[field] for s in data['node_stats'].values())
            results['execution_times'][result_key] = total / count if count else None
        for key, field in (('transactions_executed', 'tx_executed'),
                           ('blocks_executed', 'blocks_executed'),
                           ('total_tx_exec_ms', 'total_tx_exec_ms'),
                           ('total_block_exec_ms', 'total_block_exec_ms')):
            if not any(m[field] for m in measurements.values()):
                results['total_stats'][key] = None
        for node, stats in results['node_breakdown'].items():
            for field in stats:
                if not measurements[node][field]:
                    stats[field] = None
        results['coverage'] = {
            'status': 'partial',
            'note': 'Only observed wasix_eth EXP events are included; counts and gas per block are partial and log latencies are approximate (file order and execution fallback).',
            'nodes': {f'el-node-{index}': {
                'type': node['type'],
                'status': ('unavailable: Geth EXP events are not supported' if node['type'] == 'geth'
                           else 'available: observed EXP events' if f'el-node-{index}' in data['node_stats']
                           else 'unavailable: no EXP events observed')
            } for index, node in data['metadata_nodes'].items()},
            'unknown_nodes': data['unknown_nodes'],
        }
    
    return results

if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == '--storage-node-types':
        try:
            for index, node in sorted(load_metadata(sys.argv[2]).items()):
                print(index, node['type'])
        except ValueError as exc:
            print(exc, file=sys.stderr)
            sys.exit(1)
        sys.exit(0)
    if len(sys.argv) < 2:
        print("Usage: python evaluate_logs.py <log_dir> [chain_id]")
        sys.exit(1)
        
    log_dir = sys.argv[1]
    if not os.path.isdir(log_dir):
        print(f"Error: {log_dir} is not a directory")
        sys.exit(1)
        
    target_chain_id = sys.argv[2] if len(sys.argv) > 2 else None
    metadata_path = sys.argv[3] if len(sys.argv) > 3 else os.path.join(os.path.dirname(os.path.abspath(log_dir)), 'metadata.json')
    try:
        nodes = load_metadata(metadata_path) if len(sys.argv) > 3 or os.path.exists(metadata_path) else None
    except ValueError as exc:
        print(exc, file=sys.stderr)
        sys.exit(1)
    
    if target_chain_id:
        print(f"Processing logs in {log_dir} for chain_id {target_chain_id}...")
    else:
        print(f"Processing all logs in {log_dir}...")
        
    data = process_logs(log_dir, target_chain_id, nodes)
    metrics = calculate_metrics(data)

    if nodes is not None:
        print('\nCoverage: partial; only observed wasix_eth EXP events. Counts and gas per block are partial; latencies are approximate.')
        for node, coverage in metrics['coverage']['nodes'].items():
            print(f"  {node} ({coverage['type']}): {coverage['status']}")
        for node in data['unknown_nodes']:
            print(f'  {node}: unavailable (not present in metadata)')

    def format_value(value, precision=3, suffix=''):
        return 'unavailable' if value is None else f'{value:.{precision}f}{suffix}'
    
    print("\n=== Experiment Metrics ===")
    def count_value(value):
        return 'unavailable' if value is None else str(value)

    print(f"Total Transactions Executed: {count_value(metrics['total_stats']['transactions_executed'])}")
    print(f"Total Blocks Executed: {count_value(metrics['total_stats']['blocks_executed'])}")
    
    ex = metrics['execution_times']
    print(f"\nAverage Execution Times:")
    print(f"  Avg Transaction Execution: {format_value(ex['avg_tx_exec_ms'], suffix='ms')}")
    print(f"  Avg Block Execution:       {format_value(ex['avg_block_exec_ms'], suffix='ms')}")
    
    il = metrics['inclusion_latency']
    print(f"\nInclusion Latency (Time from P2P recv to EVM exec):")
    print(f"  Avg: {format_value(il['avg'], suffix='s')}")
    print(f"  Min: {format_value(il['min'], suffix='s')}")
    print(f"  Max: {format_value(il['max'], suffix='s')}")
    print(f"  Count: {il['count']}")
    
    pl = metrics['propagation_latency']
    print(f"\nBlock Propagation Latency (Time from first node recv to other nodes recv):")
    print(f"  Avg: {format_value(pl['avg'], suffix='s')}")
    print(f"  Max: {format_value(pl['max'], suffix='s')}")
    
    print("\nNode Breakdown:")
    for node, stats in metrics['node_breakdown'].items():
        print(f"  {node}:")
        print(f"    TX Recv/Sent: {count_value(stats['tx_received'])}/{count_value(stats['tx_sent'])}")
        print(f"    Blocks Executed: {count_value(stats['blocks_executed'])}")
        if stats['blocks_executed'] and stats['total_gas'] is not None and stats['total_block_exec_ms'] is not None:
            avg_gas = stats['total_gas'] / stats['blocks_executed']
            block_count = data['measurements'][node]['total_block_exec_ms'] if nodes is not None else stats['blocks_executed']
            avg_block_ms = stats['total_block_exec_ms'] / block_count
            print(f"    Avg Gas/Block{' (partial)' if nodes is not None else ''}: {avg_gas:.0f}")
            print(f"    Avg Block Exec: {avg_block_ms:.2f}ms")
        if stats['tx_executed'] and stats['total_tx_exec_ms'] is not None:
            tx_count = data['measurements'][node]['total_tx_exec_ms'] if nodes is not None else stats['tx_executed']
            avg_tx_ms = stats['total_tx_exec_ms'] / tx_count
            print(f"    Avg TX Exec: {avg_tx_ms:.2f}ms")
        
    print("\n--- Time Series Data ---")
    print("Inclusion Latency Time Series (timestamp, latency_s):")
    for ts, lat in metrics['inclusion_latency']['timeseries']:
        print(f"  {ts}, {lat:.3f}")
        
    print("\nBlock Propagation Latency Time Series (timestamp, latency_s):")
    for ts, lat in metrics['propagation_latency']['timeseries']:
        print(f"  {ts}, {lat:.3f}")

    with open('experiment_results.json', 'w') as f:
        json.dump(metrics, f, indent=2)
    print("\nFull results saved to experiment_results.json")
