import json
import pandas as pd
import matplotlib.pyplot as plt
import matplotlib.ticker as ticker
import sys
import os

def main():
    if len(sys.argv) < 2:
        print("Usage: python plot_stacked_faceted.py <experiment_analysis_dir> [output_dir] [exp_dir] [output_filename]")
        sys.exit(1)

    analysis_dir = sys.argv[1]
    output_dir = sys.argv[2] if len(sys.argv) > 2 else analysis_dir
    exp_dir = sys.argv[3] if len(sys.argv) > 3 else None
    output_filename = sys.argv[4] if len(sys.argv) > 4 else "node_metrics_stacked_faceted.pdf"
    
    if not os.path.exists(output_dir):
        os.makedirs(output_dir)

    samples_file = os.path.join(analysis_dir, "metrics_metric_samples.json")
    nodes_info_file = os.path.join(analysis_dir, "metrics_nodes.json")

    if not os.path.exists(samples_file):
        print(f"File not found: {samples_file}")
        sys.exit(1)

    print(f"Loading data from {samples_file}...")
    try:
        with open(samples_file, 'r') as f:
            data = json.load(f)
    except Exception as e:
        print(f"Error loading {samples_file}: {e}")
        sys.exit(1)

    # Convert to flat list of values
    rows = [item['value'] for item in data]
    df = pd.DataFrame(rows)

    if df.empty:
        print("No data found in samples file.")
        sys.exit(0)

    # Load node info to map node_id to index/name
    # Font sizes
    TITLE_FONT_SIZE = 18
    AXIS_LABEL_FONT_SIZE = 16
    TICK_FONT_SIZE = 14
    LEGEND_FONT_SIZE = 13
    node_id_to_info = {}
    if os.path.exists(nodes_info_file):
        try:
            with open(nodes_info_file, 'r') as f:
                nodes_info_data = json.load(f)
            for item in nodes_info_data:
                info = item['value']
                n_id = info['id']
                m_url = info.get('metrics_url', '')
                # Extract port from e.g. "http://172.28.30.134:9050"
                port = None
                if ':' in m_url.replace('http://', ''):
                    port_str = m_url.split(':')[-1]
                    if port_str.isdigit():
                        port = int(port_str)
                
                # In our setup:
                # node-0: 9050
                # node-1: 9051
                # ...
                # node-N: 9050 + N
                if port is not None and 9050 <= port <= 9100:
                    idx = port - 9050
                    node_id_to_info[n_id] = {
                        'index': idx,
                        'client': info.get('client', 'rust')
                    }
        except Exception as e:
            print(f"Warning: Error loading {nodes_info_file}: {e}")

    # Normalize timestamps: start from 0 and convert to seconds
    min_ts = df['timestamp_ms'].min()
    df['time_sec'] = (df['timestamp_ms'] - min_ts) / 1000.0

    # Compute node mapping
    unique_nodes = df['node_id'].unique()
    
    # Sort nodes based on our mapping if available, else lexicographically
    def get_sort_key(n_id):
        if n_id in node_id_to_info:
            return node_id_to_info[n_id]['index']
        return n_id

    nodes = sorted(unique_nodes, key=get_sort_key)
    node_to_idx = {node_id: i for i, node_id in enumerate(nodes)}
    num_nodes = len(nodes)

    if num_nodes == 0:
        print("No nodes found in data.")
        sys.exit(0)

    # Define metrics to plot
    metrics_config = [
        ('transactions_committed_total', 'Transactions Committed'),
        ('mempool_size', 'Mempool Size'),
        ('current_head_block', 'Current Head Block'),
        ('connected_peers', 'Connected Peers'),
        ('rpc_requests_total', 'Total RPC Requests')
    ]
    
    # Fallback for blocks metric name
    if df[df['name'] == 'current_head_block'].empty:
        metrics_config[2] = ('blocks_imported_total', 'Blocks')

    # Determine when spamming starts (first time any node has mempool > 0)
    spam_start_time = None
    mempool_df = df[df['name'] == 'mempool_size']
    if not mempool_df.empty:
        spamming = mempool_df[mempool_df['value'] > 0].sort_values('time_sec')
        if not spamming.empty:
            spam_start_time = spamming['time_sec'].iloc[0]

    # Determine when the first transaction was committed
    first_commit_time = None
    tx_df = df[df['name'] == 'transactions_committed_total']
    if not tx_df.empty:
        commits = tx_df[tx_df['value'] > 0].sort_values('time_sec')
        if not commits.empty:
            first_commit_time = commits['time_sec'].iloc[0]

    # Reference time for "first transaction happens"
    # We'll use the earlier of spam start (mempool) or first commit
    first_tx_happens = None
    if spam_start_time is not None and first_commit_time is not None:
        first_tx_happens = min(spam_start_time, first_commit_time)
    elif spam_start_time is not None:
        first_tx_happens = spam_start_time
    elif first_commit_time is not None:
        first_tx_happens = first_commit_time

    # Determine marker time based on experiment type in path
    marker_time = None
    if first_tx_happens is not None:
        duration = 120 # Default
        path_lower = analysis_dir.lower()
        if 'fifa' in path_lower:
            duration = 180
        elif 'dota' in path_lower:
            duration = 280
        elif 'uber' in path_lower:
            duration = 120
        
        marker_time = first_tx_happens + duration

    # Determine when the last transaction was committed (still useful for x_max calculation)
    last_commit_time = None
    if not tx_df.empty:
        last_increases = []
        for node in nodes:
            node_tx = tx_df[tx_df['node_id'] == node].sort_values('time_sec')
            if not node_tx.empty:
                # Find where value changes
                diff = node_tx['value'].diff()
                commits = node_tx[diff > 0]
                if not commits.empty:
                    last_increases.append(commits['time_sec'].iloc[-1])
                elif node_tx['value'].iloc[0] > 0:
                    # If it starts above 0, the first sample is the last known increase if no others exist
                    last_increases.append(node_tx['time_sec'].iloc[0])
        if last_increases:
            last_commit_time = max(last_increases)

    # Calculate X axis limits
    x_min = df['time_sec'].min()
    x_max = df['time_sec'].max()

    if spam_start_time is not None:
        x_min = max(df['time_sec'].min(), spam_start_time - 60)
    
    # We still use last_commit_time + 120 for the view window, 
    # or the marker_time + 60, whichever is larger to ensure we see the line.
    if last_commit_time is not None:
        x_max = min(df['time_sec'].max(), marker_time + 120)
    
    if marker_time is not None:
        x_max = max(x_max, min(df['time_sec'].max(), marker_time + 60))

    # Filter data for Y axis scaling based on the time frame
    df_filtered = df[(df['time_sec'] >= x_min) & (df['time_sec'] <= x_max)]

    color_map = plt.colormaps['tab20']
    
    fig, axes = plt.subplots(num_nodes, len(metrics_config), figsize=(25, 3 * num_nodes), sharex=True, sharey='col')
    
    # Handle the case of a single node (axes will be 1D)
    if num_nodes == 1:
        axes = axes.reshape(1, -1)

    # Pre-calculate Y limits for each metric based on the filtered time frame
    metric_y_limits = {}
    for j, (metric_name, title) in enumerate(metrics_config):
        if metric_name == 'transactions_committed_total':
            # For integrated graph, consider both committed and rejected
            committed_data = df_filtered[df_filtered['name'] == 'transactions_committed_total']
            rejected_data = df_filtered[df_filtered['name'] == 'mempool_rejected_transactions_total']
            
            m_min = 0
            m_max = 0
            if not committed_data.empty:
                m_min = min(m_min, committed_data['value'].min())
                m_max = max(m_max, committed_data['value'].max())
            if not rejected_data.empty:
                m_min = min(m_min, rejected_data['value'].min())
                m_max = max(m_max, rejected_data['value'].max())
            
            padding = (m_max - m_min) * 0.05 if m_max > m_min else 0.05
            metric_y_limits[metric_name] = (m_min - padding, m_max + padding)
        elif metric_name == 'connected_peers':
            padding = max(0.75, num_nodes * 0.05)
            metric_y_limits[metric_name] = (
                -padding,
                (num_nodes - 1) + padding
            )
        else:
            metric_data = df_filtered[df_filtered['name'] == metric_name]
            if not metric_data.empty:
                m_min = metric_data['value'].min()
                m_max = metric_data['value'].max()
                
                padding = (m_max - m_min) * 0.05 if m_max > m_min else 0.05
                metric_y_limits[metric_name] = (m_min - padding, m_max + padding)
            else:
                metric_y_limits[metric_name] = (-0.05, 1.05)

    def get_node_label(node_id, index, exp_dir, node_info_map):
        # Default label if mapping fails
        base_label = f"node-{index}"
        
        prefix = "node"
        if node_id in node_info_map:
            mapped_idx = node_info_map[node_id]['index']
            client = node_info_map[node_id]['client']
            # If the client is reported as "wasix-eth", it might be because the registry 
            # defaults to it or the node reports it regardless of being rust/wasix.
            # We try to refine this using exp_dir if available.
            if client == "wasix-eth":
                prefix = "wasix"
            else:
                prefix = "rust"
            
            # Refine prefix using folder structure if exp_dir is available
            if exp_dir:
                folder = f"node-{mapped_idx}"
                el_dir = os.path.join(exp_dir, "data_v2", folder, "el")
                if os.path.exists(el_dir):
                    files = os.listdir(el_dir)
                    if any(f.startswith("linux-node-") for f in files):
                        prefix = "rust"
                    elif any(f.startswith("wasix-node-") for f in files):
                        prefix = "wasix"
            
            return f"{prefix}-{mapped_idx}"
        
        if not exp_dir:
            return node_id[:8]
        
        # Fallback to old folder-based detection if mapping not available
        candidate_folders = [f"node-{node_id}", f"node-{index}"]
        
        for folder in candidate_folders:
            el_dir = os.path.join(exp_dir, "data_v2", folder, "el")
            if os.path.exists(el_dir):
                files = os.listdir(el_dir)
                patterns = [f"-node-{node_id}", f"-node-{index}"]
                for p in patterns:
                    for f in files:
                        if f.endswith(p):
                            prefix = f.replace(p, "")
                            if prefix == "linux":
                                prefix = "rust"
                            return f"{prefix}-{index}"
        
        return base_label

    for i, node in enumerate(nodes):
        idx = node_to_idx[node]
        label = get_node_label(node, idx, exp_dir, node_id_to_info)
        c = color_map(i % 20)
        
        for j, (metric_name, title) in enumerate(metrics_config):
            ax = axes[i, j]
            ax.tick_params(axis='both', labelsize=TICK_FONT_SIZE)
            if metric_name == 'transactions_committed_total':
                # Plot Transactions Committed in node color
                node_data = df[(df['node_id'] == node) & (df['name'] == 'transactions_committed_total')].sort_values('time_sec')
                if not node_data.empty:
                    ax.plot(node_data['time_sec'], node_data['value'], color=c, label='Committed')
                
                # Plot Rejected Transactions (in red)
                rejected_data = df[(df['node_id'] == node) & (df['name'] == 'mempool_rejected_transactions_total')].sort_values('time_sec')
                if not rejected_data.empty:
                    ax.plot(rejected_data['time_sec'], rejected_data['value'], color='red', linestyle='--', alpha=0.7, label='Rejected')
                
                # Add legend to top left
                ax.legend(loc='upper left', fontsize=LEGEND_FONT_SIZE)
            else:
                node_data = df[(df['node_id'] == node) & (df['name'] == metric_name)].sort_values('time_sec')
                if not node_data.empty:
                    ax.plot(node_data['time_sec'], node_data['value'], color=c)
            
            # Apply pre-calculated Y limits
            if metric_name in metric_y_limits:
                ax.set_ylim(metric_y_limits[metric_name])
            
            if j == 0:
                ax.set_ylabel(label, fontsize=AXIS_LABEL_FONT_SIZE, fontweight='bold')
            if i == 0:
                ax.set_title(title, fontsize=TITLE_FONT_SIZE, fontweight='bold')
            ax.grid(True, linestyle=':', alpha=0.5)
            if spam_start_time is not None:
                ax.axvline(x=spam_start_time, color='black', linestyle=':', alpha=0.5)
            if marker_time is not None:
                ax.axvline(x=marker_time, color='red', linestyle=':', alpha=0.5)
            
            # Apply X axis limits
            ax.set_xlim(x_min, x_max)

            # X axis ticks: 20s interval without labels, 100s interval with labels
            ax.xaxis.set_major_locator(ticker.MultipleLocator(20))
            def x_formatter(x, pos):
                if x % 100 == 0:
                    return f"{int(x)}"
                return ""
            ax.xaxis.set_major_formatter(ticker.FuncFormatter(x_formatter))
            
            # Use integer ticks for Y axis
            if metric_name == 'connected_peers':
                ax.yaxis.set_major_locator(ticker.MultipleLocator(1))
                ax.yaxis.set_major_formatter(ticker.FormatStrFormatter('%d'))
            else:
                ax.yaxis.set_major_locator(ticker.MaxNLocator(integer=True))

    plt.tight_layout()
    output_path = os.path.join(output_dir, output_filename)
    plt.savefig(
        output_path,
        format="pdf",
        bbox_inches="tight"
    )

    plt.close()
    print(f"Saved stacked faceted plot to {output_path}")

if __name__ == "__main__":
    main()
