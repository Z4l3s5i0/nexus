#!/usr/bin/env python3
import os
import re
import sys
import argparse
from collections import defaultdict

def extract_times(exp_dir):
    logs_dir = os.path.join(exp_dir, 'aggregated_logs')
    data_dir = os.path.join(exp_dir, 'data_v2')
    
    if not os.path.exists(logs_dir):
        print(f"Error: {logs_dir} does not exist.")
        return None

    # --- CL Proposer Extraction (from analyze_slots.py logic) ---
    scheduled_proposers = {} # slot -> node_id
    node_proposals = defaultdict(set) # node_id -> set of slots
    all_slots = set()
    
    if os.path.exists(data_dir):
        node_dir_re = re.compile(r'node-(\d+)')
        proposal_re = re.compile(r'INFO  Signed block published to network via HTTP API  slot: (\d+)')
        duty_re = re.compile(r'INFO  Prepared beacon proposer.*prepare_slot: (\d+), validator: (\d+)')
        valid_block_re = re.compile(r'INFO  Valid block from .* proposer_index: (\d+), slot: (\d+)')
        empty_slot_re = re.compile(r'INFO  Synced .* block: ".*empty", slot: (\d+)')

        for node_dirname in os.listdir(data_dir):
            node_match = node_dir_re.match(node_dirname)
            if not node_match: continue
            
            node_id = int(node_match.group(1))
            beacon_log_path = os.path.join(data_dir, node_dirname, 'cl', 'beacon', 'logs', 'beacon.log')
            
            if os.path.exists(beacon_log_path):
                with open(beacon_log_path, 'r', errors='ignore') as f:
                    for line in f:
                        prop_match = proposal_re.search(line)
                        if prop_match:
                            slot = int(prop_match.group(1))
                            node_proposals[node_id].add(slot)
                            scheduled_proposers[slot] = node_id
                            all_slots.add(slot)
                        
                        duty_match = duty_re.search(line)
                        if duty_match:
                            slot = int(duty_match.group(1))
                            scheduled_proposers[slot] = int(duty_match.group(2))
                            all_slots.add(slot)

                        valid_match = valid_block_re.search(line)
                        if valid_match:
                            slot = int(valid_match.group(2))
                            scheduled_proposers[slot] = int(valid_match.group(1))
                            all_slots.add(slot)

                        empty_match = empty_slot_re.search(line)
                        if empty_match:
                            all_slots.add(int(empty_match.group(1)))

    # Map Slot to Block Number and CL Proposer
    slot_to_block = {}
    block_to_slot = {}
    block_to_cl_proposer = {}
    current_block = 0
    
    sorted_slots = sorted(list(all_slots))
    for slot in sorted_slots:
        is_used = any(slot in node_proposals[nid] for nid in node_proposals)
        if is_used:
            current_block += 1
            slot_to_block[slot] = current_block
            block_to_slot[current_block] = slot
            if slot in scheduled_proposers:
                block_to_cl_proposer[current_block] = scheduled_proposers[slot]
        else:
            slot_to_block[slot] = None

    # -----------------------------------------------------------

    # Regex to match the log filename and extract node ID
    # experiments-el-node-0-1_2000_aggregated.log
    file_re = re.compile(r'experiments-el-node-(\d+)-.*_aggregated\.log')
    
    # Regex to extract block number and execution time
    # INFO  [PayloadProcessor] Block 1 executed in 10.231758ms
    # INFO  [PayloadProcessor] Block 22 executed in 1.270181706s
    line_re = re.compile(r'Block (\d+) executed in ([\d\.]+)(m?s)')
    
    # Regex to identify if this node proposed the block
    # INFO  [GossipBridge] Broadcasting local block 1 (hash: 0x...)
    proposer_re = re.compile(r'(\w{3}\s+\d+\s+\d{2}:\d{2}:\d{2}\.\d{3})\s+INFO\s+\[GossipBridge\] Broadcasting local block (\d+) \(hash: (0x[0-9a-f]+)\)')

    data = defaultdict(dict)
    proposer_candidates = defaultdict(list)
    nodes = set()
    blocks = set()

    for filename in os.listdir(logs_dir):
        match = file_re.match(filename)
        if match:
            node_id = int(match.group(1))
            nodes.add(node_id)
            filepath = os.path.join(logs_dir, filename)
            
            with open(filepath, 'r') as f:
                for line in f:
                    line_match = line_re.search(line)
                    if line_match:
                        block_num = int(line_match.group(1))
                        value = float(line_match.group(2))
                        unit = line_match.group(3)
                        
                        # Convert to ms if in seconds
                        if unit == 's':
                            value *= 1000
                            
                        # Use the last one seen for a block (sometimes blocks are re-executed)
                        data[block_num][node_id] = value
                        blocks.add(block_num)
                    
                    prop_match = proposer_re.search(line)
                    if prop_match:
                        timestamp = prop_match.group(1)
                        block_num = int(prop_match.group(2))
                        block_hash = prop_match.group(3)
                        proposer_candidates[block_num].append((timestamp, node_id, block_hash))

    # Determine the actual proposer (first one to broadcast local block)
    # We also track if there were multiple hashes for the same block number (reorgs/conflicts)
    el_winners = {}
    block_notes = {}
    el_reorgs = set()
    cl_reorgs = set()

    # CL Reorg detection: Look for "New canonical head" that jumps back or switches branches
    if os.path.exists(data_dir):
        reorg_re = re.compile(r'INFO  New canonical head.*slot: (\d+), root: (0x[0-9a-f]+)')
        slot_to_roots = defaultdict(set)
        for node_dirname in os.listdir(data_dir):
            beacon_log_path = os.path.join(data_dir, node_dirname, 'cl', 'beacon', 'logs', 'beacon.log')
            if os.path.exists(beacon_log_path):
                with open(beacon_log_path, 'r', errors='ignore') as f:
                    for line in f:
                        reorg_match = reorg_re.search(line)
                        if reorg_match:
                            s = int(reorg_match.group(1))
                            r = reorg_match.group(2)
                            slot_to_roots[s].add(r)
        
        # Map slot reorgs to blocks
        curr_b = 0
        for s in sorted(list(all_slots)):
            if any(s in node_proposals[nid] for nid in node_proposals):
                curr_b += 1
                if len(slot_to_roots[s]) > 1:
                    cl_reorgs.add(curr_b)

    for block_num, candidates in proposer_candidates.items():
        # Sort by timestamp
        candidates.sort()
        el_winner = candidates[0][1]
        el_winners[block_num] = el_winner
        
        cl_proposer = block_to_cl_proposer.get(block_num)
        
        # Check for multiple unique hashes
        unique_hashes = set(c[2] for c in candidates)
        if len(unique_hashes) > 1:
            el_reorgs.add(block_num)
            block_notes[block_num] = f"Conflict: {len(unique_hashes)} hashes"
        
        # Compare EL winner with CL proposer
        unique_nodes = set(c[1] for c in candidates)
        if cl_proposer is not None:
            if el_winner != cl_proposer:
                block_notes[block_num] = f"EL race: Node {el_winner} faster than Node {cl_proposer}"
            elif len(unique_nodes) > 1 and block_num not in block_notes:
                block_notes[block_num] = f"EL Race: {len(unique_nodes)} nodes"
        elif len(unique_nodes) > 1 and block_num not in block_notes:
            block_notes[block_num] = f"EL Race: {len(unique_nodes)} nodes"

    return data, el_winners, block_to_cl_proposer, block_to_slot, slot_to_block, scheduled_proposers, sorted_slots, block_notes, sorted(list(nodes)), sorted(list(blocks)), cl_reorgs, el_reorgs

def generate_latex_table(data, el_winners, cl_proposers, block_to_slot, slot_to_block, scheduled_proposers, sorted_slots, block_notes, nodes, blocks, cl_reorgs, el_reorgs):
    if not nodes:
        return "No data found."

    # LaTeX tabular header
    # Slot, Block, CL Prop, Node 0, ..., Node N
    col_spec = "ccc" + "c" * len(nodes)
    header = ["Slot", "Block", "CL Prop"] + [f"Node {n}" for n in nodes]
    
    latex = []
    latex.append(r"\begin{tabular}{" + col_spec + "}")
    latex.append(r"\hline")
    latex.append(" & ".join(header) + r" \\")
    latex.append(r"\hline")
    
    for slot in sorted_slots:
        block = slot_to_block.get(slot)
        
        row = [str(slot), str(block) if block is not None else "-"]
        
        cl_proposer = None
        if block is not None:
            cl_proposer = cl_proposers.get(block)
        else:
            cl_proposer = scheduled_proposers.get(slot)
            
        row.append(f"Node {cl_proposer}" if cl_proposer is not None else "-")
        
        el_winner = el_winners.get(block) if block is not None else None
        
        for node in nodes:
            time = None
            if block is not None:
                time = data[block].get(node)
            
            if time is not None:
                time_str = f"{time:.2f}"
                
                # Apply formatting
                is_el_winner = (node == el_winner)
                
                cell = time_str
                if is_el_winner:
                    cell = r"\textbf{" + cell + "}"
                
                # EL Reorg: Cross out
                if block in el_reorgs:
                    cell = r"\sout{" + cell + "}"
                
                # CL Reorg: Color
                if block in cl_reorgs:
                    cell = r"\textcolor{red}{" + cell + "}"
                
                row.append(cell)
            else:
                row.append("-")
        
        latex.append(" & ".join(row) + r" \\")
    
    latex.append(r"\hline")
    latex.append(r"\end{tabular}")
    
    return "\n".join(latex)

def main():
    parser = argparse.ArgumentParser(description='Extract block execution times and output LaTeX table.')
    parser.add_argument('dir', help='Experiment directory path')
    args = parser.parse_args()

    result = extract_times(args.dir)
    if result:
        data, el_winners, cl_proposers, block_to_slot, slot_to_block, scheduled_proposers, sorted_slots, block_notes, nodes, blocks, cl_reorgs, el_reorgs = result
        latex_table = generate_latex_table(data, el_winners, cl_proposers, block_to_slot, slot_to_block, scheduled_proposers, sorted_slots, block_notes, nodes, blocks, cl_reorgs, el_reorgs)
        print(latex_table)

if __name__ == "__main__":
    main()