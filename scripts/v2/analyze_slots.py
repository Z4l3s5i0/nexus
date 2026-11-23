#!/usr/bin/env python3
import os
import re
import sys
import argparse
from collections import defaultdict

def analyze_slots(exp_dir):
    data_dir = os.path.join(exp_dir, 'data_v2')
    logs_dir = os.path.join(exp_dir, 'aggregated_logs')
    
    if not os.path.exists(data_dir):
        print(f"Error: {data_dir} does not exist.")
        return None

    # Regex for node directories
    node_dir_re = re.compile(r'node-(\d+)')
    
    # Regex to identify a successful local proposal (CL)
    proposal_re = re.compile(r'INFO  Signed block published to network via HTTP API  slot: (\d+)')
    
    # Regex to identify an empty slot (skipped)
    empty_slot_re = re.compile(r'INFO  Synced .* block: ".*empty", slot: (\d+)')

    # Regex to identify scheduled proposer duty
    duty_re = re.compile(r'INFO  Prepared beacon proposer.*prepare_slot: (\d+), validator: (\d+)')

    # Regex to identify proposer for a block imported via gossip/HTTP
    valid_block_re = re.compile(r'INFO  Valid block from .* proposer_index: (\d+), slot: (\d+)')

    # Regex for reorgs (though rare in these logs, we look for head changes)
    # INFO  New canonical head slot: 21, root: 0xfdb...
    reorg_re = re.compile(r'INFO  New canonical head.*slot: (\d+), root: (0x[0-9a-f]+)')

    # --- EL Proposer Detection (to align with extract_block_times.py) ---
    el_file_re = re.compile(r'experiments-el-node-(\d+)-.*_aggregated\.log')
    el_proposer_re = re.compile(r'(\w{3}\s+\d+\s+\d{2}:\d{2}:\d{2}\.\d{3})\s+INFO\s+\[GossipBridge\] Broadcasting local block (\d+) \(hash: (0x[0-9a-f]+)\)')
    
    el_proposer_candidates = defaultdict(list) # block_num -> [(timestamp, node_id, hash)]
    
    if os.path.exists(logs_dir):
        for filename in os.listdir(logs_dir):
            match = el_file_re.match(filename)
            if match:
                node_id = int(match.group(1))
                filepath = os.path.join(logs_dir, filename)
                with open(filepath, 'r') as f:
                    for line in f:
                        prop_match = el_proposer_re.search(line)
                        if prop_match:
                            timestamp = prop_match.group(1)
                            block_num = int(prop_match.group(2))
                            block_hash = prop_match.group(3)
                            el_proposer_candidates[block_num].append({
                                'timestamp': timestamp,
                                'node_id': node_id,
                                'hash': block_hash
                            })
    
    el_first_broadcaster = {}
    el_all_broadcasters = defaultdict(list)
    for block_num, candidates in el_proposer_candidates.items():
        candidates.sort(key=lambda x: x['timestamp'])
        el_first_broadcaster[block_num] = candidates[0]['node_id']
        el_all_broadcasters[block_num] = candidates
    # -------------------------------------------------------------------

    node_proposals = defaultdict(set) # node_id -> set of slots
    scheduled_proposers = {} # slot -> node_id
    slot_to_root = {} # slot -> canonical root
    reorged_slots = set()
    all_slots = set()

    for node_dirname in os.listdir(data_dir):
        node_match = node_dir_re.match(node_dirname)
        if not node_match:
            continue
        
        node_id = int(node_match.group(1))
        beacon_log_path = os.path.join(data_dir, node_dirname, 'cl', 'beacon', 'logs', 'beacon.log')
        
        if not os.path.exists(beacon_log_path):
            continue
            
        with open(beacon_log_path, 'r', errors='ignore') as f:
            for line in f:
                prop_match = proposal_re.search(line)
                if prop_match:
                    slot = int(prop_match.group(1))
                    node_proposals[node_id].add(slot)
                    scheduled_proposers[slot] = node_id
                    all_slots.add(slot)
                
                empty_match = empty_slot_re.search(line)
                if empty_match:
                    slot = int(empty_match.group(1))
                    all_slots.add(slot)

                duty_match = duty_re.search(line)
                if duty_match:
                    slot = int(duty_match.group(1))
                    val_index = int(duty_match.group(2))
                    scheduled_proposers[slot] = val_index
                    all_slots.add(slot)

                valid_match = valid_block_re.search(line)
                if valid_match:
                    val_index = int(valid_match.group(1))
                    slot = int(valid_match.group(2))
                    scheduled_proposers[slot] = val_index
                    all_slots.add(slot)
                
                reorg_match = reorg_re.search(line)
                if reorg_match:
                    slot = int(reorg_match.group(1))
                    # We could track if the root changed for a slot, but usually 
                    # "New canonical head" for a previously finalized/accepted slot indicates reorg.
                    pass

    final_data = []
    current_block = 0
    for slot in sorted(list(all_slots)):
        is_used = any(slot in node_proposals[nid] for nid in node_proposals)
        
        if is_used:
            current_block += 1
            block_num = current_block
            status = 'Used'
            
            # Scheduled Proposer (from CL)
            scheduled_id = None
            if slot in scheduled_proposers:
                scheduled_id = scheduled_proposers[slot]
            
            # EL First Broadcaster (what extract_block_times.py bolds)
            first_broadcaster = el_first_broadcaster.get(block_num)
            
            # Conflict analysis
            broadcasters = el_all_broadcasters.get(block_num, [])
            unique_nodes = set(b['node_id'] for b in broadcasters)
            
            note = ""
            if scheduled_id is not None and first_broadcaster is not None and scheduled_id != first_broadcaster:
                note = f"EL race: Node {first_broadcaster} faster than Node {scheduled_id}"
            elif len(unique_nodes) > 1:
                note = f"Multiple broadcasters: {len(unique_nodes)}"
            
            # For the table, we show the CL scheduled proposer as the "Proposer" 
            # but note the EL difference.
            proposer_str = f"Node {scheduled_id}" if scheduled_id is not None else "-"
            block_str = str(block_num)
        else:
            status = 'Skipped'
            block_str = "-"
            proposer_id = scheduled_proposers.get(slot)
            proposer_str = f"Node {proposer_id}" if proposer_id is not None else "-"
            note = ""
            
        final_data.append({
            'slot': slot, 
            'status': status, 
            'proposer': proposer_str, 
            'block': block_str,
            'note': note
        })
            
    return final_data

def generate_latex_table(data):
    if not data:
        return "No data found."

    latex = []
    latex.append(r"\begin{tabular}{cccc}")
    latex.append(r"\hline")
    latex.append(r"Slot & Block & Status & Proposer \\")
    latex.append(r"\hline")
    
    for entry in data:
        row = [str(entry['slot']), entry['block'], entry['status'], entry['proposer']]
        latex.append(" & ".join(row) + r" \\")
    
    latex.append(r"\hline")
    latex.append(r"\end{tabular}")
    
    return "\n".join(latex)

def main():
    parser = argparse.ArgumentParser(description='Analyze skipped and used slots from Lighthouse logs.')
    parser.add_argument('dir', help='Experiment directory path')
    args = parser.parse_args()

    data = analyze_slots(args.dir)
    if data:
        latex_table = generate_latex_table(data)
        print(latex_table)

if __name__ == "__main__":
    main()
