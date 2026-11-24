# **Go-Ethereum (Geth) vs. Reth vs. Nexus: An In-Depth Architectural and Implementation Analysis**

Technical report Â· Ethereum execution clients Â· Architecture, implementation, protocol compliance, storage, execution, synchronization and design philosophy

---

## **22. Nexus: A Third Independent Execution Client**

Nexus is a third independent Ethereum execution client written in Rust. It shares the same Rust ecosystem as Reth â€” including the `evm` crate for EVM execution and the `alloy` suite for Ethereum types â€” but makes substantially different architectural choices in nearly every layer. Where Reth was designed by Paradigm with explicit emphasis on modularity, performance, and component reuse, and where Geth represents decades of evolutionary engineering in Go, Nexus represents an independent redesign driven by its own set of engineering priorities.

The defining characteristic of Nexus is its dual target: it runs natively and as a Wasix module. This is unique among the three implementations. Neither Geth nor Reth targets Wasix. This design goal influences nearly every architectural decision, from the choice of database engine to the structure of the synchronization pipeline.

This section provides a direct, source-code-level comparison of Nexus against both Geth and Reth, with particular attention to the areas where Nexus's architecture diverges most significantly: reorganization handling, payload building, and execution logic.

### **22.1 Project Structure and Organization**

Nexus is organized as a Cargo workspace with 10 crates. The decomposition is coarser than Reth's 50+ crates but finer than Geth's monolithic Go binary. The workspace is divided into `nexus_core` (chain management, Engine API, mempool, sync, gossip, and consensus), `nexus_execution` (EVM integration, transaction execution, state application, and block processing), `nexus_storage` (database layer using redb, read and write providers, and trie implementation), `nexus_types` (Ethereum type definitions re-exported from alloy and the `evm` crate), `nexus_p2p` (a complete P2P stack including discovery v4 and RLPx transport), `nexus_rpc` (JSON-RPC server covering eth, engine, admin, debug, net, and web3 namespaces), `nexus_app` (node assembly, CLI, JWT authentication, and component orchestration), and `nexus_utils` (mappers, logging, metrics, and identity utilities).

A notable structural choice is that Nexus consolidates the Engine API, mempool, synchronization, and gossip into a single `nexus_core` crate. Reth distributes these responsibilities across separate crates for engine processing, transaction pooling, staged synchronization, and networking, making each independently reusable. Geth distributes them across Go packages for transaction pooling, block downloading, consensus client communication, and peer-to-peer networking. Nexus's coarser granularity reduces compilation complexity for Wasix targets and simplifies the dependency graph, but it comes at the cost of independent component reuse.

Node assembly in Nexus uses a payload-based pattern. Four payload types â€” `StoragePayload`, `NetworkPayload`, `ExecutionPayload`, and `SyncPayload` â€” are assembled into a `Node` struct with explicit `Arc`-wrapped components. This is simpler than Reth's builder pattern with its trait-based extensibility, and less integrated than Geth's service-based node container. The tradeoff is clarity and simplicity over extensibility.

### **22.2 EVM Implementation**

At the library level, Nexus and Reth share the same EVM engine. Both use the `evm` crate, which provides the SputnikVM interpreter, the `GasometerEtable` for gas accounting, the `ExecutionEtable` for opcode execution, and the `StandardPrecompileSet` for precompiled contracts. This means the EVM opcode execution, gas accounting, and precompile logic are identical at the library level between Nexus and Reth. Geth, by contrast, implements its own EVM natively in Go, with a separate interpreter, gasometer, and precompile set.

The integration layer differs. Nexus implements `SputnikBackend`, which implements `RuntimeBaseBackend` from the `evm` crate, wrapping its own `BatchWriter` storage layer. The backend handles account balance, code, nonce, storage, delegation checks (EIP-7702), and transient storage. An `OverlayedBackend` wraps the `SputnikBackend` for in-memory state changes during execution. This is structurally similar to Reth's approach but with a simpler interface: Nexus's backend directly wraps its `BatchWriter` rather than using a separate database abstraction layer.

Nexus supports all hardforks from Frontier through Prague, including EIP-7623 (floor data gas) and EIP-7702 (delegation). The fork configuration maps directly to the `evm` crate's `Config` variants: Prague to `Config::prague()`, Cancun to `Config::cancun()`, and so on through Frontier.

### **22.3 Transaction Execution**

Nexus's transaction execution follows a clear flow. A `TransactionExecutor` validates the transaction against state. A `SputnikBackend` is created wrapping the `BatchWriter`. An `OverlayedBackend` wraps the `SputnikBackend` for in-memory state changes. EIP-4844 and EIP-7702 pre-execution logic is applied. The `evm::transact()` function executes the transaction. The resulting `OverlayedChangeSet` is extracted and applied via `StateApplier`, which applies changes sequentially: balances, nonces, codes, storage resets, storages, and finally account cleanup with explicit EIP-161 empty account removal. State changes are then persisted through `BatchWriter`.

The state application logic works like this:

```rust
fn apply_changeset(&mut self, changeset: OverlayedChangeSet) -> Result<()> {
    // 1. Apply balance changes
    for (address, balance) in changeset.balances {
        self.set_balance(address, balance)?;
    }
    // 2. Apply nonce changes
    for (address, nonce) in changeset.nonces {
        self.set_nonce(address, nonce)?;
    }
    // 3. Apply code changes
    for (address, code) in changeset.codes {
        self.set_code(address, code)?;
    }
    // 4. Apply storage resets (cleared slots)
    for (address, slots) in changeset.storage_resets {
        self.reset_storage(address, slots)?;
    }
    // 5. Apply storage changes
    for (address, storage) in changeset.storages {
        self.set_storage(address, storage)?;
    }
    // 6. Clean up empty accounts (EIP-161)
    for address in changeset.accounts_to_remove {
        self.remove_account_if_empty(address)?;
    }
    Ok(())
}
```

This is a straightforward application model compared to Reth's more complex state provider architecture and Geth's journal-based approach. Nexus's `StateApplier` is a thin layer that applies the changeset produced by the EVM's `OverlayedBackend` directly to the database batch. There is no intermediate journaling or state provider indirection.

A distinctive feature of Nexus's execution layer is its support for interruption tokens. Every transaction execution can carry an optional `CancellationToken` that allows the executor to stop mid-block during payload building. This is critical for Nexus's budget-aware payload building strategy, which is described in detail below. Geth does not have equivalent mid-block interruption support. Reth has cancellation tokens but uses them differently, primarily for aborting entire pipeline stages rather than interrupting individual transaction execution within a block build.

### **22.4 Storage Architecture**

Nexus uses `redb`, a pure Rust embedded B+tree database, as its primary storage engine. This is fundamentally different from both Reth's MDBX (a C library with Rust bindings) and Geth's Pebble or LevelDB (LSM-tree databases). The choice of redb is directly motivated by Nexus's Wasix target: redb has no C dependencies, making it portable to Wasix environments where linking C libraries is impractical.

redb provides B+tree storage with MVCC (Multi-Version Concurrency Control), transactional semantics with savepoints, and a single-writer, multi-reader model. The API surface is simpler than MDBX's, which trades some advanced features for portability and ease of use.

Nexus organizes its data across 21 named redb tables. Chain data is stored in `Headers`, `HeaderTD`, `BlockBodies`, `Transactions`, `Receipts`, and `ReceiptsMeta`. Chain indexes include `CanonicalHeads`, `HeaderNumbers`, and `TransactionLookup`. State is stored in `Accounts`, `Storages`, and `Bytecodes`. State changes are tracked in `AccountChangeSets` and `StorageChangeSets`. State indexes use `TrieNodes` and `TrieNodeRefs`. Metadata, engine payloads, forkchoice state, state branches, and active peers each have their own tables.

A key architectural difference is that Nexus does not have a separate static file layer for historical data. Reth's NippyJar component provides compressed append-oriented storage for historical blockchain data, separating hot execution state from cold historical data. Geth uses ancient/freezer storage for the same purpose. In Nexus, all data lives in the same redb database. This means historical queries compete with current execution for database access, which can affect performance under mixed workloads. However, it simplifies the storage architecture and eliminates the need for a separate historical data management layer.

### **22.5 Trie Implementation**

Nexus implements its own Merkle Patricia Trie with custom node types: `MyLeafNode`, `MyExtensionNode`, and `MyBranchNode`. The implementation uses `alloy_trie` for nibble operations and `alloy_rlp` for encoding, but the trie construction logic is entirely custom. Unlike Reth, which implements parallel-capable trie computation through separate account hashing and storage hashing stages, Nexus calculates the state root sequentially. This is a significant performance difference for blocks with many transactions, where Reth's parallel hashing can reduce state root computation time.

Nexus does not explicitly implement sparse trie support or proof generation, both of which are available in Geth and Reth. This means Nexus is less suited for applications that require historical Merkle proofs, such as light client verification or state proof RPC endpoints.

### **22.6 Synchronization Architecture**

Nexus's synchronization architecture is simpler than both Geth and Reth. It uses an event-driven orchestrator model rather than Geth's snap sync or Reth's explicit staged pipeline. The `SyncOrchestrator` is a background loop that listens for sync targets via `SyncRegistry` and triggers the `SyncProvider`. The `Downloader` selects peers by total difficulty or height and downloads headers, bodies, and pooled transactions via the ETH protocol. A `Processor` processes downloaded blocks, a `Registry` manages sync targets, and a `Controller` controls sync flow.

Nexus does not implement the Ethereum snap protocol for state synchronization. It does not have a staged pipeline with explicit unwind capability. It does not support ERA1 historical data import. It does not have an explicit state healing mechanism. This means Nexus's initial synchronization relies entirely on header and body download followed by sequential execution, which is slower than Geth's snap sync and less structured than Reth's staged pipeline. However, the simpler architecture is more portable to Wasix environments and easier to reason about.

### **22.7 Transaction Pool (Mempool)**

Nexus's mempool is built around immutable snapshots. A `MempoolSnapshot` provides a read-optimized, immutable view for block building, ensuring that concurrent modifications to the mempool do not affect an in-progress payload build. Transaction selection uses a binary heap (priority queue) with O(M log N) complexity. Sender-indexed queues maintain nonce ordering, and blob transactions are tracked in a separate blob map.

A unique feature of Nexus's mempool is its support for optimistic transactions. The `OPTIMISTIC_TX_LIMIT` constant defines a threshold for transactions that were skipped during standard selection but may be included as candidates. During payload building, after the standard transaction selection fills the block up to the gas limit, Nexus attempts to include optimistic transactions using checkpoint-based rollback: each optimistic transaction is executed after creating a checkpoint, and if it exceeds the gas limit or fails, the batch is reverted to the checkpoint. This allows Nexus to pack more transactions into a block without risking invalid state. Neither Geth nor Reth implements this pattern.

The mempool implementation is substantial, with comprehensive nonce management, replacement handling, and eviction logic built around the snapshot pattern.

### **22.8 Networking and P2P**

Nexus implements a complete P2P stack organized into three areas. Discovery includes k-buckets, the v4 protocol with handler, message, packet, and worker components. Peer management includes a dialer, discovery manager, peer manager, peer registry, and sync service. RLPx transport includes custom crypto, frame handling, ECIES handshake, message encoding, server, session management, and stream handling.

The implementation is self-contained and does not rely on external networking libraries like Reth's `discv4` or Geth's `p2p` package. The RLPx transport includes custom ECIES handshake, AES-CTR frame encryption, and session management. This self-containment is important for Wasix compatibility, where external networking libraries may not be available.

### **22.9 Reorganization Handling**

Nexus's approach to chain reorganizations is one of its most architecturally distinctive features. The reorg handling is split across three components: `ReorgHandler`, `Engine`, and `ReorgManager`, with coordination through `ImportGate` and `StateTip`.

The core of Nexus's reorg logic is the `classify_switch` function, which determines whether a fork switch runs synchronously or in the background. The classification is based purely on state work: the number of blocks reverted from the state tip plus the number of blocks re-executed on top of the common ancestor. Marking blocks canonical is considered cheap and never forces the background path. The threshold is 64 state operations: if the total number of reverted blocks plus blocks to execute is at most 64, the switch runs synchronously; otherwise it runs in the background.

The classification logic works like this:

```rust
fn classify_switch(
    state_tip: U64,
    new_tip: U64,
    old_canonical: U64,
) -> ForkSwitchMode {
    // How many blocks need to be reverted from the state tip?
    let revert_count = if state_tip > old_canonical {
        state_tip - old_canonical
    } else {
        U64::ZERO
    };
    // How many blocks need to be re-executed?
    let execute_count = new_tip - old_canonical;
    // Total state work (marking canonical is cheap, doesn't count)
    let total_state_ops = revert_count + execute_count;
    
    if total_state_ops <= 64 {
        ForkSwitchMode::Synchronous
    } else {
        ForkSwitchMode::Background
    }
}
```

This is a nuanced design decision. Geth handles fork switches synchronously without an explicit classification mechanism. Reth uses a similar synchronous/async distinction but with different thresholds and implementation details. Nexus's approach is particularly well-suited to its single-writer database model: by classifying the switch before committing to it, Nexus avoids blocking the Engine API for deep reorgs while still handling common shallow reorgs efficiently.

When a fork switch is classified as synchronous, Nexus executes `apply_fork_switch` directly within the `forkchoiceUpdated` call. When classified as background, it spawns a separate async task that runs the fork switch while the Engine API returns `SYNCING` immediately. The background task uses `scopeguard` to ensure that reorg-in-progress flags are cleared even if the fork switch fails. During a background fork switch, subsequent `forkchoiceUpdated` calls return `SYNCING` until the background task completes, preventing concurrent reorgs.

The `ReorgHandler` provides three key operations. `resolve_reorg` walks back from the new head through parent hashes, collecting blocks and checking whether each block is already marked canonical, until it finds the common ancestor with the old head. This walk-back checks both main storage and the payload table, ensuring that blocks validated but not yet canonical are still accessible. `mark_branch_canonical` promotes blocks from the payload table to main storage, updating canonical heads, header numbers, block hashes, total difficulty, and transaction lookups. `revert_to_height` rolls back state changes from the applied-state tip down to a target height by iterating through change sets in reverse order, restoring account state and storage to their previous values.

The change-set-based revert is a key architectural choice. Nexus stores `AccountChangeSets` and `StorageChangeSets` for each block during execution. When reverting, it reads these change sets and applies them in reverse, restoring the flat state tables to their pre-block values:

```rust
fn revert_to_height(&mut self, from: u64, to: u64) -> Result<()> {
    // Iterate through blocks in reverse order
    for block_number in (to..=from).rev() {
        // Read account changesets for this block
        let account_changes = self.read_account_changesets(block_number)?;
        for change in account_changes {
            // Restore the previous account state
            self.set_account(change.address, change.previous_state)?;
        }
        // Read storage changesets for this block
        let storage_changes = self.read_storage_changesets(block_number)?;
        for change in storage_changes {
            // Restore the previous storage value
            self.set_storage(change.address, change.slot, change.previous_value)?;
        }
    }
    Ok(())
}
```

This is simpler than Geth's path-based state revert, which maintains reverse diffs, and more direct than Reth's stage-based unwind, which reverses each pipeline stage. However, it requires that change sets are retained for all blocks that might need to be reverted, which increases storage overhead.

The `ImportGate` is a concurrency control mechanism that prevents concurrent block imports. It is used by both the payload builder and the payload processor to ensure that only one block import is in progress at a time. This is critical for Nexus's single-writer database model: concurrent writes to redb would cause conflicts. The `StateTip` tracks the current state tip hash and number, allowing Nexus to efficiently determine whether a fork switch requires state revert or is simply a canonicality update.

### **22.10 Payload Building**

Nexus's payload builder is its most sophisticated component and one of its core strengths. The `PayloadBuilder` implements a continuous background building strategy with budget-aware transaction selection, cost model-based timing, and interruption support.

When a `forkchoiceUpdated` call includes payload attributes, Nexus creates an initial empty payload and spawns a background task that continuously rebuilds the payload as new transactions arrive. The build loop runs for up to 80% of the effective slot duration (derived from the timestamp difference between parent and attributes, clamped to the configured slot length). It responds to new transaction events with debouncing and minimum rebuild intervals, ensuring that it does not waste resources on rapid-fire rebuilds.

The budget-aware transaction selection is the most distinctive feature. Nexus uses a `BuildCostModel` that tracks the cost of finalization (state root calculation plus persistence) using an Exponentially Weighted Moving Average (EWMA). The model maintains two parameters: microseconds per transaction and fixed microseconds for state root calculation. After each build, the model records the actual finalization time and updates the EWMA. During the next build, it uses the model to estimate how many transactions can be included while still leaving enough time to finalize before the slot starts.

The cost model updates its estimates after each build:

```rust
struct BuildCostModel {
    ewma_per_tx: f64,       // microseconds per transaction
    ewma_fixed: f64,        // fixed microseconds for state root + persistence
    alpha: f64,             // EWMA smoothing factor (e.g. 0.1)
}

impl BuildCostModel {
    fn record_finalization(&mut self, tx_count: usize, duration: Duration) {
        let per_tx = duration.as_micros() as f64 / tx_count as f64;
        let fixed = duration.as_micros() as f64 - per_tx * tx_count as f64;
        
        self.ewma_per_tx = self.alpha * per_tx + (1.0 - self.alpha) * self.ewma_per_tx;
        self.ewma_fixed = self.alpha * fixed + (1.0 - self.alpha) * self.ewma_fixed;
    }
    
    fn estimate_finalization(&self, tx_count: usize) -> Duration {
        let micros = self.ewma_fixed + self.ewma_per_tx * tx_count as f64;
        Duration::from_micros(micros as u64)
    }
}
```

The budget calculation is sophisticated. The first build uses only 25% of the remaining budget to ensure a non-empty payload is stored early. Subsequent rebuilds reserve half of the budget for execution and half for finalization. The builder computes a deadline that is the minimum of a slot-relative cap (35% of the effective slot) and a budget-relative cap (remaining time minus estimated finalization cost). If the deadline is reached during transaction execution, the builder stops including transactions and finalizes the block with whatever it has.

This approach is fundamentally different from both Geth and Reth. Geth's payload builder selects transactions based on gas price and includes as many as fit within the block gas limit, without explicit timing budgets. Reth's payload builder also uses transaction selection based on profitability but does not have the same level of budget-aware timing control. Nexus's approach ensures that the payload is always ready before the slot starts, even under high load or slow hardware.

The interruption mechanism is equally distinctive. Nexus uses two types of cancellation tokens: cancel tokens for the build loop and rebuild interruption tokens for individual rebuilds. When `getPayload` is called, the builder first cancels the build loop to stop starting new builds, then cancels only the transaction loop of any in-flight rebuild so that its state root calculation and persistence can still complete. This ensures that `getPayload` always returns a valid, persisted payload rather than a partially built one. The builder uses a `BuildStatus` enum to track whether the initial build is in progress or completed, and `getPayload` waits for the initial build to complete before returning.

The optimistic transaction mechanism, described in the mempool section, is tightly integrated with payload building. After the standard transaction selection fills the block, Nexus attempts to include optimistic transactions using checkpoint-based rollback. Each optimistic transaction is executed after creating a checkpoint in the `BatchWriter`. If the transaction succeeds and fits within the gas limit, the checkpoint is cleared. If it fails or exceeds the gas limit, the batch is reverted to the checkpoint and the next optimistic transaction is tried. This allows Nexus to maximize block utilization without risking invalid state.

### **22.11 Execution Logic**

Nexus's execution layer is organized around the `EthExecutionProvider`, which implements the `ExecutionProvider` trait. The trait defines multiple execution modes that cover different use cases:

```rust
trait ExecutionProvider {
    // Simple block execution (validation)
    fn execute_block(&self, block: &Block) -> Result<ExecutionOutcome>;
    
    // Execution with commit control
    fn execute_block_with_commit(&self, block: &Block, commit: bool) -> Result<ExecutionOutcome>;
    
    // Execution with state root calculation
    fn execute_block_with_state_root(&self, block: &Block) -> Result<ExecutionOutcome>;
    
    // Building mode with interruption and deadline support
    fn execute_block_with_batch_timed(
        &self, block: &Block, batch: &mut BatchWriter,
        building: bool, interrupt: Option<&CancellationToken>,
        deadline: Option<Instant>, optimistic: Vec<Transaction>,
    ) -> Result<ExecutionOutcome>;
    
    // Batch execution for reorg re-execution
    fn execute_blocks_with_batch(&self, blocks: &[Block]) -> Result<Vec<ExecutionOutcome>>;
    
    // Side-commit for sidechain blocks
    fn execute_block_side_commit(&self, block: &Block) -> Result<StateRoot>;
    
    // Simulation execution
    fn execute_simulation(&self, tx: &Transaction) -> Result<SimulationResult>;
}
```

The trait covers simple block execution, execution with commit control, execution with state root, execution with building mode and interruption support, batch execution for reorg re-execution, side-commit execution for sidechain blocks, payload-specific execution, and simulation execution.

The `execute_block_with_batch_timed` method is the core execution function. It takes a block, a mutable `BatchWriter`, an optional state root, a building flag, an optional interruption token, an optional deadline, and a list of optimistic transactions. It prepares the execution environment, applies EIP-4788 beacon root system calls for Cancun and later, and then iterates through the transactions. For each transaction, it checks the interruption token and deadline, executes the transaction, and collects the receipt. If building mode is enabled and a transaction fails, it is skipped rather than causing the entire block to fail. If building mode is disabled and a transaction fails, the entire block execution fails.

The `execute_blocks_with_batch` method is used during reorg re-execution. It takes a list of blocks and executes them all within a single database batch. For each block, it executes the transactions, collects the change sets, and persists the receipt metadata. After all blocks are executed, it commits the batch in a single transaction. This atomic multi-block execution is more efficient than executing each block separately and ensures that the state is consistent after the reorg.

The `execute_block_for_payload_timed` method is used during payload building. It constructs a block from a list of transactions, a parent header, and payload attributes. It then calls `execute_block_with_batch_timed` with building mode enabled, allowing interruption and deadline-based stopping. The method handles optimistic transactions by executing them after the standard transactions, using checkpoint-based rollback.

A key design decision is that Nexus separates execution from persistence. The `BatchWriter` accumulates state changes during execution, and the changes are only persisted when the batch is committed. This allows Nexus to execute blocks without committing them (for validation), to execute blocks and commit them (for import), and to execute blocks and store them as sidechain payloads (for fork choice). The `execute_block_side_commit` method is used for sidechain blocks: it executes the block, persists the state root in the `StateBranches` table, and commits the batch without making the block canonical.

### **22.12 Design Philosophy**

Nexus's design philosophy is shaped by three priorities: Wasix compatibility, correctness, and simplicity. The Wasix target is the primary constraint that influences architectural decisions. The choice of redb over MDBX, the coarser crate structure, the simpler synchronization architecture, and the sequential trie computation are all motivated by the need to run within a Wasix runtime.

Correctness is the second priority. Nexus shares the same EVM engine as Reth, which provides a high degree of confidence in execution correctness. The change-set-based reorg revert is simple and direct, reducing the risk of state corruption during reorganizations. The budget-aware payload building ensures that payloads are always ready before the slot starts, reducing the risk of missed proposals.

Simplicity is the third priority. Nexus's architecture is simpler than both Geth and Reth in several ways. The synchronization pipeline is a straightforward event-driven orchestrator rather than Geth's snap sync or Reth's staged pipeline. The storage architecture is a single database rather than Reth's MDBX plus static files or Geth's LevelDB plus ancient storage. The reorg handling is change-set-based rather than Geth's path-based state or Reth's stage-based unwind. This simplicity makes Nexus easier to understand, maintain, and port to Wasix environments.

The tradeoff is that Nexus sacrifices some performance and feature completeness for simplicity and portability. It does not have parallel trie computation, hot/cold data separation, snap sync, or ERA1 import. These are deliberate choices that prioritize Wasix compatibility and architectural clarity over raw performance.

### **22.13 Comprehensive Three-Way Comparison**

The three implementations represent three different approaches to building an Ethereum execution client. Geth is the mature, battle-tested implementation written in Go, with extensive operational history and broad compatibility. Reth is the modern, modular implementation written in Rust, with explicit emphasis on performance, parallelism, and component reuse. Nexus is the Wasix-compatible implementation written in Rust, with emphasis on portability, simplicity, and budget-aware execution.

At the EVM level, Nexus and Reth share the same engine (SputnikVM via the `evm` crate), while Geth uses its own native Go implementation. At the storage level, Nexus uses redb (pure Rust B+tree), Reth uses MDBX (C library B-tree), and Geth uses Pebble or LevelDB (LSM-tree). At the synchronization level, Nexus uses a simple event-driven orchestrator, Reth uses an explicit staged pipeline, and Geth uses snap sync with state healing. At the reorg level, Nexus uses change-set-based revert with fork switch classification, Reth uses stage-based unwind, and Geth uses integrated reorg handling. At the payload building level, Nexus uses continuous background building with budget-aware transaction selection, Reth uses modular payload building, and Geth uses integrated payload construction.

### **22.14 Practical Implications**

Nexus is advantageous in environments where Wasix compatibility is required or where pure Rust dependencies are preferred over C libraries. Its budget-aware payload building is particularly well-suited for environments with variable hardware performance, where the timing of state root calculation and persistence can vary significantly. Its optimistic transaction mechanism allows it to maximize block utilization without risking invalid state. Its change-set-based reorg revert is simple and direct, reducing the risk of state corruption during reorganizations.

Nexus is less advantageous for high-throughput RPC infrastructure, where Reth's parallel trie computation and hot/cold data separation provide better performance. It is also less advantageous for initial synchronization speed, where Geth's snap sync and Reth's ERA1 import provide faster bootstrap. And it is less advantageous for historical data queries, where Reth's static file layer and Geth's ancient storage provide better separation of hot and cold data.

### **22.15 Conclusion**

Nexus represents a third independent Ethereum execution client that shares the Rust ecosystem with Reth but makes fundamentally different architectural choices. Its defining characteristic is Wasix compatibility, which influences its choice of redb over MDBX, its simpler sync architecture, and its sequential trie computation. Its core strengths lie in its budget-aware payload building with continuous background reconstruction, its change-set-based reorg handling with fork switch classification, and its optimistic transaction mechanism with checkpoint-based rollback.

While it shares the same EVM (SputnikVM) and type ecosystem (alloy) with Reth, its storage architecture, synchronization approach, reorg handling, and payload building are independently designed. It is not a fork of Reth, nor is it a Rust port of Geth. It is a distinct implementation with its own design priorities.

The three implementations together provide valuable client diversity for Ethereum. Geth offers mature, battle-tested infrastructure with extensive operational history. Reth offers modern, modular infrastructure with parallel computation and component reuse. Nexus offers Wasix-compatible infrastructure with budget-aware execution and simple, direct reorg handling. None is inherently more correct. Correctness depends on producing the state transitions and protocol behavior required by Ethereum. The architectural differences create tradeoffs in performance, portability, maintainability, and operational complexity.

---

*This comparison is based on source code analysis of the Nexus repository. The original Geth vs Reth comparison (Sections 1-21) remains unchanged and is available in the full report.*