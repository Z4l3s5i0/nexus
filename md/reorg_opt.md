### Report: Feasibility and Design of State Snapshotting in `wasix_eth`

The proposal to implement a snapshotting system (e.g., every 10 blocks, keeping the last 5) to handle complex reorganizations or node freezes is a robust approach to ensuring chain integrity. Below is a detailed analysis of what this would imply, its feasibility in a WASM/Wasix environment, and the systems concerned.

#### 1. Current State vs. Snapshots
Currently, `wasix_eth` relies on **Change Sets** (`AccountChangeSets` and `StorageChangeSets`) to perform reverts. This is a "diff-based" approach where the node undoes specific changes block-by-block.

*   **Implication of Snapshots**: A snapshot system would move towards a "point-in-time" recovery. Instead of undoing 100 blocks one-by-one, the node could simply "jump" back to the state of block `N-100`.

#### 2. Affected Systems
*   **Storage Layer (`wasix_eth_storage`)**:
    *   **New Table**: A `Snapshots` table would be needed to store either full database backups or references to specific root hashes.
    *   **Database Backend (`redb`)**: `redb` (the current DB) supports transactions. A snapshot could technically be implemented as a separate file copy or a internal "savepoint" if the DB supports it.
*   **Reorg Manager (`ReorgHandler`)**:
    *   Needs to be aware of snapshots. When a reorg depth exceeds a certain threshold (e.g., more than 10 blocks), it could choose to restore a snapshot instead of iterating through change sets.
*   **Engine (`Engine`)**:
    *   Would need a "Recovery Mode" where, upon detecting a freeze or a "reorg-during-reorg" failure, it triggers a rollback to the latest valid snapshot and re-syncs.

#### 3. Feasibility in WASM/Wasix
*   **Storage Overhead (High Risk)**: In a WASM environment, disk I/O and space are often constrained.
    *   If a "snapshot" is a full copy of the `redb` file, disk usage would quintuple (5 snapshots = 5x size).
    *   If a "snapshot" is logical (e.g., just keeping old trie nodes), it's more efficient but harder to "rollback" because you must ensure old nodes aren't pruned.
*   **Performance**: Creating a snapshot every 10 blocks adds a write spike. In WASM, this might cause a noticeable "hiccup" in block processing times.
*   **Rollback Mechanism**: Copying a large snapshot file back to the active data file in WASM is relatively slow but technically feasible.

#### 4. Pros and Cons

| Feature | Change-Set Revert (Current) | Snapshot Rollback (Proposed) |
| :--- | :--- | :--- |
| **Complexity** | High (must track every change) | Medium (file management) |
| **Recovery Speed** | Linear (slow for deep reorgs) | Constant (fast regardless of depth) |
| **Reliability** | Susceptible to "broken diffs" | Highly resilient to state corruption |
| **Disk Space** | Minimal (only diffs) | Significant (multi-state storage) |

#### 5. Proposed Hybrid Implementation Strategy
Instead of full file copies, a more "ambitious but feasible" approach for a WASM node would be:

1.  **Tagging Snapshots**: Every 10 blocks, record the `StateRoot` and `BlockId` in a `Snapshots` index.
2.  **Deferred Pruning**: Ensure that the `TrieNodes` table does not delete any nodes that are part of the last 5 snapshots.
3.  **Logical Rollback**: To "rollback," simply update the `CanonicalHead` to the snapshot's block height and set the current `StateRoot` to the snapshot's root. The "old" state is immediately available because the trie nodes were never deleted.
4.  **Physical Rollback (Emergency)**: Only if the database itself is corrupted (e.g., `redb` header error), fallback to a filesystem-level backup (if available).

#### 6. Sidechains for Easier Reorganizations
Building "Sidechains" or ephemeral state branches for non-canonical blocks could significantly simplify reorgs. Instead of instantly overwriting or reverting the main state, the node maintains multiple "Leaves" of the state trie.

*   **Mechanism**: Each new block, if it's not a direct descendant of the current head, starts a new `StateBranch`.
*   **Feasibility**: 
    *   **Trie-based Advantage**: Since Ethereum uses a Merkle Patricia Trie, multiple "States" can coexist in the same database. They share the same underlying nodes for unchanged accounts, only differing at the root and changed branches.
    *   **Transition to Canonical**: When a reorg occurs, "switching" to a sidechain is as simple as updating the `CanonicalHead` pointer and the `StateRoot`. No block-by-block reverts are needed.
*   **WASM Consideration**: This is highly efficient in terms of I/O because it leverages the natural persistence of Trie nodes. The main cost is the metadata tracking for these branches and the delayed pruning of nodes belonging to non-canonical (but still viable) branches.

#### 7. Parallelizing EVM Execution
Parallelization is a critical optimization for scaling transaction throughput, particularly in resource-constrained WASM environments.

*   **Proposed Strategies**:
    1.  **Optimistic Execution (STM)**: Execute transactions in parallel, assuming they won't touch the same storage slots. If a conflict is detected (Read/Write collision), the conflicting transaction is re-executed sequentially.
    2.  **Access List Pre-declaration (EIP-648/EIP-2930 style)**: Use access lists to pre-determine which transactions can run in parallel. Transactions with disjoint access lists are safe to run concurrently.
*   **Feasibility in `wasix_eth`**:
    *   **Current Bottleneck**: The `InMemoryEnvironment` and the `OverlayedBackend` are currently designed for single-threaded access.
    *   **Implementation**: Would require a "Shared State" layer with fine-grained locking or versioning. In a WASM environment, `wasix` supports threads, but memory synchronization primitives (atomic operations) are necessary.
*   **Complexity**: High. Parallel execution requires a deterministic way to resolve conflicts to ensure the final `StateRoot` remains consistent across all nodes. A hybrid approach—parallelizing signature verification and pre-fetching state while keeping execution sequential—might be a more realistic first step for `wasix_eth`.

#### Conclusion
**Is it feasible? Yes.** It is a standard practice in many production clients (like Geth's snapshot layer or Erigon's staged sync). 
**Is it high ambition? Yes.** It requires careful management of the "Trie Node Lifetime" to prevent the database from growing indefinitely while still keeping the last 50-100 blocks of state reachable.

For the moment, the priority should be ensuring the **Change Set** logic is 100% bug-free. Snapshotting, sidechain tracking, and parallel execution optimizations should be considered the next evolution of the storage and execution engines.
