# Forkchoice Analysis: Contention and Freezes in `wasix_eth`

This document analyzes the forkchoice engine API path for contention, locks, freezes, and other potential problems, especially during high contention or chain reorganizations.

## 1. Critical Contention Points

### 1.1 Database Write Lock (`redb`) - [PARTIALLY MITIGATED]
`wasix_eth` uses `redb` as its underlying storage. `redb` enforces a strict **single-writer** policy. 
- **Location**: `wasix_eth_storage/src/write.rs`
- **Current Status**: Large reorg batches have been broken down into per-block transactions with `tokio::task::yield_now()` calls in between. Ongoing executions are now interruptible and release the write lock promptly upon reorganization.
- **Remaining Risk**: High frequency of `new_payload` calls can still serialize at the writer, but "head-of-line" blocking by long-running reorgs is significantly reduced.

### 1.2 `PayloadBuilder` Global Lock - [RESOLVED]
- **Location**: `wasix_eth_core/src/engine/payload_builder.rs`, `build_locks: Arc<DashMap<PayloadId, Arc<tokio::sync::Mutex<()>>>>`.
- **Change**: Replaced the global `build_lock` with per-PayloadID locks using a `DashMap`.
- **Result**: Concurrent `build_new_payload` requests for different payload IDs no longer serialize during initialization, reducing latency when multiple ELs or payload attributes are involved.

### 1.3 `CanonicalState` RwLock - [RESOLVED]
- **Location**: `wasix_eth_core/src/engine/canonicality_tracker.rs`.
- **Change**: Refactored to update internal memory state immediately and offload DB persistence to a background `tokio::spawn` task.
- **Result**: The `RwLock` is now held only for the duration of the in-memory pointer swap, preventing readers (like `get_head`) from being blocked by database I/O.

---

## 2. Potential Freezes and Latency Spikes

### 2.1 Synchronous Reorgs in `forkchoice_updated` - [RESOLVED]
- **Location**: `wasix_eth_core/src/engine/engine.rs`.
- **Change**: `mempool.revalidate` has been moved to a background task using `tokio::spawn`. Reorg handling now returns to the CL as soon as the database is updated.

### 2.2 Re-execution of "Promoted" Blocks - [RESOLVED]
- **Location**: `wasix_eth_core/src/engine/engine.rs`.
- **Change**: Blocks promoted from the `Payloads` table to main storage are now re-executed and marked canonical in a background task.
- **Result**: `forkchoiceUpdated` returns to the Consensus Client immediately after reorg logic (revert/fast-switch) and forkchoice state updates, without waiting for block re-execution. Mempool revalidation is also deferred until after promotion completes.

### 2.3 The "Fast Switch" Half-Measure - [MITIGATION IMPLEMENTED]
- **Location**: `wasix_eth_core/src/engine/reorg_manager.rs`.
- **Status**: A background "Healing" process for flat state tables has been implemented to reconcile Accounts/Storage tables with the MPT root. (Currently disabled but ready for deployment).

---

## 3. Deadlock and Cancellation Risks

### 3.1 Reorg vs. Payload Building - [MITIGATED]
- **Status**: `abort_all_executions()` is now async and waits for aborted tasks to release their DB handles. This prevents the reorg from starting while a payload build still holds the `redb` write lock.

### 3.2 Merkle Patricia Trie (MPT) Interruptibility - [RESOLVED]
- **Status**: State root calculations are now interruptible. The `BatchWriter` checks for cancellation during trie traversal, ensuring that even heavy hashing operations can be stopped immediately to favor a new reorganization.

---

## 4. Engine API Timeout and Retry Risks

The Consensus Client (CL), such as Lighthouse, imposes strict timeouts (defaulting to ~8 seconds) on Engine API calls. If the Execution Layer (EL) does not respond within this window, the CL may retry the request or treat the node as unresponsive.

### 4.1 Retry Idempotency - [RESOLVED]
- **Issue**: Concurrent retries of `new_payload` or `forkchoice_updated` (fCU) for the same block/head could lead to redundant executions and database lock contention.
- **Solution**: Implemented idempotency channels (`in_flight_results`) in `PayloadProcessor`. Retried requests now "join" the existing execution task and await its result instead of starting a new one.

### 4.2 Lighthouse Configuration Recommendation
In environments with high execution latency (e.g., WASIX/WASM or high-TPS workloads), the default 8s timeout may be insufficient. 
- **Recommendation**: Increase the Lighthouse timeout using the flag:
  `--execution-timeout-multiplier 2` (or higher)
  This provides a 16s+ window, reducing the frequency of retries and allowing the EL to finish heavy MPT updates or block re-executions without being interrupted by the CL's retry logic.

---

## 5. Remaining Recommendations

3.  **Enable Flat-State Healing**: Once verified, enable the `heal_flat_state` background task to ensure high-performance flat state reads remain consistent after deep reorgs.
