### Refactoring Plan: Standardizing RPC I/O with Alloy Types

This plan outlines the steps required to transition the `wasix_eth` project to use standard `alloy_rpc_types` for all external communication (JSON-RPC, P2P/Gossip, Engine API) while maintaining current storage structures until a later phase.

---

### Phase 1: Core Types Refactoring (`wasix_eth_types`) ✓
The goal is to eliminate custom RPC structures in favor of standard Alloy representations and clearly separate Consensus types from RPC types.

1.  **Standardize RPC Exports in `lib.rs`:** ✓
    *   Introduce a `rpc` module to group all standard RPC types.
    *   Deprecate/Replace custom `RpcHeader` and `RpcTransaction` with `alloy_rpc_types::eth::Header` and `alloy_rpc_types::eth::Transaction`.
    *   Define aliases in `wasix_eth_types\src\lib.rs`:
        ```rust
        pub mod rpc {
            pub use alloy_rpc_types::eth::{
                Transaction as RpcTransaction,
                Block as RpcBlock,
                Header as RpcHeader,
                TransactionReceipt as RpcReceipt
            };
        }
        ```

2.  **Align Engine API Types (`engine_types.rs`):** ✓
    *   Remove custom `ExecutionPayloadV2/V3/V4` structs if they only add flattening.
    *   Use `alloy_rpc_types::engine::ExecutionPayloadV1-V4` directly in the `EthRpc` trait and `EngineController`.

3.  **Refine P2P Types (`p2p.rs`):** ✓
    *   Ensure all P2P message structures (like `NewBlock`, `PooledTransactions`) use consensus-ready types (e.g., `alloy_consensus::TxEnvelope`) but follow the wire protocol encoding requirements.

4.  **Separate Files for Storage vs API Traits:** ✓
    *   Create a `consensus` module for internal/storage types.
    *   Create an `api` module to group all JSON-RPC interface traits (`EthRpc`, `AdminApi`, etc.).
    *   Ensure `lib.rs` re-exports everything for backward compatibility.

---

### Phase 2: RPC Layer Migration (`wasix_eth_rpc`) ✓
Update the RPC controllers and services to work exclusively with the new RPC types.

1.  **Update Trait Definitions (`api/eth.rs`, `api/admin.rs`):** ✓
    *   Ensure all methods return `wasix_eth_types::rpc` variants.
    *   Verify `EthRpc` uses the updated `rpc::RpcBlock` and `rpc::RpcTransaction` aliases.

2.  **Update `EthController` & `EthService`:** ✓
    *   Adjust logic to handle the fields of standard Alloy RPC types.
    *   Migrate `map_receipt` and similar helper functions to produce `alloy_rpc_types::eth::TransactionReceipt` instead of standard types.

3.  **Update Engine Controller:** ✓
    *   Switch to `alloy_rpc_types::engine` types for all payload and forkchoice operations.

---

### Phase 3: Utility and Mapping Alignment (`wasix_eth_utils`) ✓
The mapping logic needs to handle the conversion from internal consensus/storage types to the standard RPC types.

1.  **Refactor `TransactionMapper`:** ✓
    *   Update `to_rpc_transaction` to return `alloy_rpc_types::eth::Transaction`. ✓
    *   Update `to_rpc_receipt` to return `alloy_rpc_types::eth::TransactionReceipt`. ✓
    *   Implement conversions that correctly populate RPC-only fields (e.g., `effectiveGasPrice`, `txIndex`) using data from both the transaction and the block header. ✓

2.  **Refactor `BlockMapper`:** ✓
    *   Update `to_rpc_block` to return `alloy_rpc_types::eth::Block`. ✓
    *   Ensure the conversion correctly maps `alloy_consensus::Header` to `alloy_rpc_types::eth::Header`. ✓

---

### Phase 4: P2P and Gossip Standardization (`wasix_eth_p2p`) ✓
Align the P2P layer with the Ethereum wire protocol using Alloy types.

1.  **Transaction Gossip:** ✓
    *   Ensure `process_gossip_transactions` and `process_pooled_transactions` in `SyncProvider` use `alloy_consensus::TxEnvelope` (pooled transactions). ✓
    *   Verify that RLP encoding/decoding of these envelopes matches the Ethereum `eth/68+` specifications. ✓

2.  **Block Gossip:** ✓
    *   Use `alloy_consensus::Block<alloy_consensus::TxEnvelope>` for `process_gossip_block`. ✓

---

### Phase 5: Storage Layer (Future Phase)
While storage remains as-is for now, the plan for future migration involves:
*   **Consensus-only Storage:** Replacing custom `wasix_eth_types::Receipt` with `alloy_consensus::Receipt`.
*   **RLP Consistency:** Ensuring all tables (`Headers`, `Transactions`, `Receipts`) store raw `alloy_consensus` types encoded with `alloy_rlp`.
*   **Removal of `ReceiptMeta`:** Merging metadata (like `contract_address`) into a more standard indexing system if necessary, or strictly adhering to the minimal consensus receipt structure.

### Summary of Targeted Type Mappings
| Category | Storage / Internal (Stay same for now) | RPC I/O / Gossip (New Goal) |
| :--- | :--- | :--- |
| **Transactions** | `alloy_consensus::TxEnvelope` | `alloy_rpc_types::eth::Transaction` |
| **Headers** | `alloy_consensus::Header` | `alloy_rpc_types::eth::Header` |
| **Blocks** | `alloy_consensus::Block<TxEnvelope>` | `alloy_rpc_types::eth::Block` |
| **Receipts** | `wasix_eth_types::Receipt` | `alloy_rpc_types::eth::TransactionReceipt` |
| **Engine API** | Custom `ExecutionPayload` | `alloy_rpc_types::engine::ExecutionPayload` |