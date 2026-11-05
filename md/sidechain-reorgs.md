# Sidechain-based Reorganizations for `wasix_eth`

This document proposes a transition from the current diff-based reorganization mechanism to a sidechain-based mechanism that leverages the inherent persistence of Merkle Patricia Trie (MPT) nodes to allow for near-instantaneous state switches during chain reorganizations.

## 1. Proposal: Ephemeral State Branches

Currently, `wasix_eth` handles reorganizations by walking back to a common ancestor and applying "Change Sets" (reverse diffs) block-by-block. While reliable, this is linear in time relative to the reorg depth and computationally expensive.

The **Sidechain-Reorg** mechanism proposes maintaining multiple "active" state roots in the database. Since the MPT is content-addressed, different blocks can share the vast majority of trie nodes. Only the modified branches and the root hash differ between blocks.

### Core Mechanism:
- **Persistent State Roots**: Instead of only keeping the canonical state root, the node stores and tracks the state roots of all "recent" sidechain blocks.
- **Trie-based Switching**: A reorganization becomes a pointer swap. To switch from branch A to branch B, the node updates its `CanonicalHead` to point to the tip of branch B and sets the active `StateRoot` to branch B's root.
- **Implicit Reversion**: No manual "revert" of account values is needed because the "old" state nodes for branch A remain in the database (until pruned). Branch B's state is already fully formed and accessible via its root.

---

## 2. Affected Systems and Modules

To implement this mechanism, the following modules must be updated:

### 2.1 `wasix_eth_storage`
- **New Table: `StateBranches`**: A table to track `(BlockId -> StateRoot)`. This allows the engine to quickly find the state root associated with any block, whether canonical or not.
- **Trie Management (`trie.rs`)**:
    - Modify the pruning logic. Currently, nodes not reachable from the canonical root might be pruned. The pruner must now respect a "Pinning" mechanism where nodes reachable from any "active" branch tip are preserved.
- **Write Path (`write.rs`)**:
    - Ensure that when a sidechain block is executed, its state changes are committed to the `TrieNodes` table even if the block is not yet canonical.

### 2.2 `wasix_eth_execution`
- **Execution Context**: The `ExecutionProvider` must be able to load any `StateRoot` from storage to start execution, not just the latest canonical one. (Note: Much of this is already present via `execute_block_with_state_root`).
- **Batching**: The `BatchWriter` should be able to commit side-state roots without affecting the `PlainState` table (which usually represents the current canonical tip).

### 2.3 `wasix_eth_core`
- **Reorg Manager (`reorg_manager.rs`)**:
    - **Logic Change**: Instead of calling `revert_to_height`, the `ReorgHandler` will identify the target `StateRoot` for the new head.
    - **Transition Logic**: It will perform the "pointer swap" by updating the `CanonicalHeads` table and the in-memory `CanonicalState`.
- **Engine (`engine.rs` / `payload_processor.rs`)**:
    - Must correctly route "side" blocks to the execution engine and ensure their resulting state roots are indexed in the `StateBranches` table.
- **Mempool**:
    - When a sidechain reorg occurs, transactions from the discarded branch must be re-inserted into the mempool.
    - The `MempoolListener` or `Engine` must identify transactions present in the discarded blocks (but not in the new canonical branch) and return them to the pending pool.
    - A full `revalidate()` of the mempool is required after the "pointer swap" to ensure nonces and balances are consistent with the new state branch.

---

## 3. Implementation Plan

### Phase 1: Infrastructure & Side-State Persistence
- **Storage**: Add `StateBranches` table to `tables.rs`.
- **Storage**: Implement `insert_state_root(block_hash, root)` in `write.rs`.
- **Execution**: Enable "Side-Commit" mode in `EthExecutionProvider` where it calculates and persists trie nodes for a block but skips updating the `PlainState` (singleton canonical state).
- **Verification**: Ensure sidechain blocks can be executed and their state roots are stored.

### Phase 2: Hybrid Reorg Manager
- **Core**: Update `ReorgHandler` to check if the `new_head` has a pre-calculated `StateRoot` in the `StateBranches` table.
- **Logic**: If the `StateRoot` exists, perform a "Fast Switch".
- **Mempool**: Ensure `Engine::handle_reorgs_for_mempool` is called during a "Fast Switch" to re-queue discarded transactions.
- **Mempool**: Trigger `mempool.revalidate()` immediately after the `CanonicalHead` pointer swap to align mempool nonces with the new branch.
- **Fallback**: Maintain the `ChangeSet` logic as a fallback for blocks where the state root was not persisted or for very deep reorgs exceeding the "Sidechain Window".
- **Consistency**: Update `PlainState` to match the new canonical state after a "Fast Switch" (this can be done via a background sync or a lazy-update mechanism).

### Phase 3: Optimized Pruning & GC
- **Ref-Counting/Pinning**: Implement a reference-counting or "Generation-based" garbage collector for `TrieNodes`.
- **Windowing**: Define a `REORG_LIMIT` (e.g., 100 blocks). Any branch tip older than this limit is unpinned, allowing the pruner to reclaim space.
- **Metadata Cleanup**: Periodically prune the `StateBranches` and `ChangeSets` tables for blocks older than the finality threshold.

---

## 4. Design Considerations

### Logic Preservation
The `ChangeSet` mechanism will remain as the "Source of Truth" for historical audits. The Sidechain mechanism acts as an acceleration layer. If a node crashes or loses sidechain metadata, it can always reconstruct the state using the canonical chain and change sets.

### WASM Efficiency
In a WASM environment, the primary bottleneck is I/O. By avoiding the multi-step "delete and insert" cycles of block-by-block reverts, we significantly reduce the number of database writes during a reorg. The increased disk usage for keeping unpruned trie nodes is mitigated by the fact that many nodes are shared between branches.
