# RPC API Reference

This document lists all JSON-RPC endpoints exposed by NEXUS. The RPC server is built on `jsonrpsee` and exposes two HTTP endpoints:

- **Eth JSON-RPC** — Standard Ethereum API (default port `8545`)
- **Auth Engine JSON-RPC** — Engine API for consensus client integration (default port `8551`, JWT-authenticated)

> **Note:** Not all methods in every namespace are fully implemented. Some methods may return partial results or error responses.

## Server Configuration

| Parameter | CLI Flag | Default |
|---|---|---|
| Eth RPC port | `--eth-rpc-port` | 8545 |
| Auth RPC port | `--auth-rpc-port` | 8551 |
| JWT secret path | `--auth-rpc-jwt-path` | Auto-generated in data dir |

## Engine API JWT Authentication

The Auth Engine JSON-RPC endpoint requires JWT authentication. The server:

1. Loads the JWT secret from `--auth-rpc-jwt-path` if provided
2. Otherwise, looks for `jwt_<chain_id>.hex` in the data directory
3. If no file exists, generates a new 32-byte secret and saves it

Clients must include the `Authorization: Bearer <token>` header with a valid JWT signed using HS256 with the shared secret.

---

## `eth_*` Namespace

Standard Ethereum JSON-RPC methods.

### Accounts

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `eth_accounts` | — | `Array<Address>` | List of available accounts |

### Gas & Transactions

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `eth_gasPrice` | — | `U256` | Current gas price |
| `eth_sendTransaction` | `TransactionRequest` | `B256` | Send a new transaction |
| `eth_sendRawTransaction` | `String` (RLP) | `B256` | Send raw signed transaction |
| `eth_signTransaction` | `TransactionRequest` | `Bytes` | Sign a transaction |
| `eth_sign` | `Address`, `String` | `Bytes` | Sign a message |
| `eth_estimateGas` | `TransactionRequest`, `BlockId` | `U256` | Estimate gas for a call |
| `eth_call` | `TransactionRequest`, `BlockId` | `Bytes` | Execute a call (dry-run) |

### Blocks

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `eth_blockNumber` | — | `U64` | Current block number |
| `eth_getBlockByNumber` | `BlockNumber`, `bool` (full tx) | `RpcBlock` | Get block by number |
| `eth_getBlockByHash` | `B256`, `bool` (full tx) | `RpcBlock` | Get block by hash |
| `eth_getBlockTransactionCountByNumber` | `BlockNumber` | `U64` | Transaction count in block |
| `eth_getBlockTransactionCountByHash` | `B256` | `U64` | Transaction count in block |
| `eth_getUncleCountByBlockNumber` | `BlockNumber` | `U64` | Uncle count in block |
| `eth_getUncleCountByBlockHash` | `B256` | `U64` | Uncle count in block |

### Transactions

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `eth_getTransactionByHash` | `B256` | `RpcTransaction` | Get transaction by hash |
| `eth_getTransactionByBlockHashAndIndex` | `B256`, `U64` | `RpcTransaction` | Get transaction by block + index |
| `eth_getTransactionByBlockNumberAndIndex` | `BlockNumber`, `U64` | `RpcTransaction` | Get transaction by number + index |
| `eth_getTransactionReceipt` | `B256` | `RpcTransactionReceipt` | Get transaction receipt |

### State & Storage

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `eth_getBalance` | `Address`, `BlockId` | `U256` | Get account balance |
| `eth_getStorageAt` | `Address`, `U256`, `BlockId` | `B256` | Get storage value at slot |
| `eth_getCode` | `Address`, `BlockId` | `Bytes` | Get contract code |
| `eth_getProof` | `Address`, `Array<U256>`, `BlockId` | `AccountProof` | Get account proof |

### Logs & Filters

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `eth_getLogs` | `Filter` | `Array<Log>` | Get logs matching filter |
| `eth_newFilter` | `Filter` | `U64` | Create log filter |
| `eth_getFilterChanges` | `U64` | `Array<Log>` | Poll filter changes |
| `eth_getFilterLogs` | `U64` | `Array<Log>` | Get all filter logs |
| `eth_uninstallFilter` | `U64` | `bool` | Uninstall a filter |

### Sync & Node

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `eth_syncing` | — | `SyncStatus` | Node sync status |
| `eth_mining` | — | `bool` | Whether node is mining (always `false`) |
| `eth_chainId` | — | `U64` | Current chain ID |

---

## `engine_*` Namespace

Engine API for consensus client (e.g., Lighthouse) integration. Requires JWT authentication.

### Payload Management

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `engine_newPayloadV1` | `ExecutionPayloadV1` | `PayloadStatus` | Process new payload (pre-shanghai) |
| `engine_newPayloadV2` | `ExecutionPayloadV2` | `PayloadStatus` | Process new payload (shanghai+) |
| `engine_newPayloadV3` | `ExecutionPayloadV3`, `Vec<B256>`, `B256` | `PayloadStatus` | Process new payload (dencun+) |
| `engine_newPayloadV4` | `ExecutionPayloadV4`, `Vec<B256>`, `B256`, `Vec<Bytes>` | `PayloadStatus` | Process new payload (electra+) |

### Forkchoice

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `engine_forkchoiceUpdatedV1` | `ForkchoiceState`, `PayloadAttributes?` | `ForkchoiceUpdated` | Update forkchoice (pre-shanghai) |
| `engine_forkchoiceUpdatedV2` | `ForkchoiceState`, `PayloadAttributes?` | `ForkchoiceUpdated` | Update forkchoice (shanghai+) |
| `engine_forkchoiceUpdatedV3` | `ForkchoiceState`, `PayloadAttributes?` | `ForkchoiceUpdated` | Update forkchoice (dencun+) |
| `engine_forkchoiceUpdatedV4` | `ForkchoiceState`, `PayloadAttributes?` | `ForkchoiceUpdated` | Update forkchoice (electra+) |

### Payload Retrieval

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `engine_getPayloadV1` | `PayloadId` | `ExecutionPayloadV1` | Get payload (pre-shanghai) |
| `engine_getPayloadV2` | `PayloadId` | `ExecutionPayloadEnvelopeV2` | Get payload (shanghai+) |
| `engine_getPayloadV3` | `PayloadId` | `ExecutionPayloadEnvelopeV3` | Get payload (dencun+) |
| `engine_getPayloadV4` | `PayloadId` | `ExecutionPayloadEnvelopeV4` | Get payload (electra+) |

### Payload Bodies

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `engine_getPayloadBodiesByHashV1` | `Array<B256>` | `Array<Option<ExecutionPayloadBodyV1>>` | Get payload bodies by hash |
| `engine_getPayloadBodiesByRangeV1` | `start`, `count` | `Array<Option<ExecutionPayloadBodyV1>>` | Get payload bodies by range |

### Blobs

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `engine_getBlobsV1` | `Array<B256>` | `Array<Option<BlobAndProofV1>>` | Get blobs by versioned hash |
| `engine_getBlobsV2` | `Array<B256>` | `Option<Array<BlobAndProofV2>>` | Get blobs (v2) |
| `engine_getBlobsV3` | `Array<B256>` | `Option<Array<Option<BlobAndProofV2>>>` | Get blobs (v3) |
| `engine_getBlobsV4` | `Array<B256>`, `B128` | `Option<Value>` | Get blobs (v4) |

### Configuration

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `engine_exchangeTransitionConfigurationV1` | `TransitionConfiguration` | `TransitionConfiguration` | Exchange transition config |
| `engine_exchangeCapabilities` | `Array<String>` | `Array<String>` | Exchange supported capabilities |

---

## `admin_*` Namespace

Node administration methods.

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `admin_nodeInfo` | — | `NodeInfo` | Get node information (enode, ports, etc.) |
| `admin_addPeer` | `String` (enode URL) | `bool` | Manually add a peer |
| `admin_peers` | — | `Array<PeerInfo>` | List connected peers with details |

---

## `net_*` Namespace

Network information methods.

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `net_peerCount` | — | `U256` | Number of connected peers |

---

## `web3_*` Namespace

Web3 utility methods.

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `web3_clientVersion` | — | `String` | Client version string |

---

## `debug_*` Namespace

Debugging and diagnostic methods.

| Method | Parameters | Returns | Description |
|---|---|---|---|
| `debug_getMempool` | — | `Array<RpcTransaction>` | List all transactions in the mempool |
| `debug_getRawBlock` | `BlockId` | `Bytes` | Get raw RLP-encoded block |
| `debug_getRawHeader` | `BlockId` | `Bytes` | Get raw RLP-encoded header |
| `debug_getRawReceipts` | `BlockId` | `Array<Bytes>` | Get raw RLP-encoded receipts |
| `debug_getRawTransaction` | `B256` | `Bytes` | Get raw RLP-encoded transaction |

---

## Metrics Endpoint

The node exposes Prometheus metrics at `http://localhost:<metrics_port>/metrics` (default port `9050`).

Key metrics:
- `RPC_REQUESTS_TOTAL` — Total RPC requests received
- `GOSSIP_MESSAGES_RECEIVED` — Total gossip messages received
- `CONNECTED_PEERS` — Current number of connected peers

---

## Error Codes

The RPC server uses standard JSON-RPC error codes:

| Code | Meaning |
|---|---|
| `-32700` | Parse error |
| `-32600` | Invalid request |
| `-32601` | Method not found |
| `-32602` | Invalid params |
| `-32603` | Internal error |

Custom errors from `RpcError`:
- Invalid block hash
- Transaction not found
- Block not found
- Insufficient funds
- And more
