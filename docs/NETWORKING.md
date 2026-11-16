# P2P Networking Guide

This document describes the P2P networking stack in NEXUS, covering node discovery (Discv4), encrypted transport (RLPx), peer management, gossip message handling, and the sync service.

## Overview

The P2P stack is implemented in the `wasix_eth_p2p` crate and consists of three main subsystems:

1. **Discovery** — Discv4 protocol for finding peers on the network
2. **Transport** — RLPx protocol for encrypted peer-to-peer communication
3. **Peer Management** — Registry, dialer, and discovery manager for connection lifecycle

```
Discovery (Discv4, UDP)
  ↓ finds peers
Peer Manager (Registry + Dialer)
  ↓ initiates connections
RLPx Transport (TCP + ECIES handshake + AES-256-CTR frames)
  ↓ encrypted sessions
Sync Service (Gossip handler, block/transaction requests)
```

## Discv4 Discovery Protocol

### Implementation

The Discv4 implementation is in `discovery/v4_service.rs` and related modules. It provides UDP-based node discovery using the Ethereum Discovery Protocol v4.

**Key components:**

- **`DiscoveryV4Service`** — Main service that binds a UDP socket and manages the discovery lifecycle
- **`RoutingTable`** — k-bucket based routing table for peer management (`discovery/kbuckets.rs`)
- **Packet types** — Ping, Pong, FindNode, Neighbors, EnrRequest, EnrResponse (`discovery/v4/packet.rs`)

### Service Initialization

The service is created with:

```rust
DiscoveryV4Service::new(
    local_identity,  // Node's secp256k1 keypair
    addr,            // Bind address
    udp_port,        // UDP discovery port
    tcp_port,        // TCP RLPx port
    bootnodes,       // Bootstrap node enode URLs
    ext_ip,          // Optional external IP
)
```

On startup, the service:

1. Generates a local ENR (Ethereum Node Record) containing the node's identity, IP, and ports
2. Parses and registers bootnodes in the routing table
3. Sends initial Ping messages to bootnodes
4. Enters a UDP packet receive loop

### Message Flow

The discovery protocol uses a request-response pattern:

- **Ping/Pong** — Periodic keepalive; carries ENR sequence numbers and endpoints
- **FindNode/Neighbors** — Queries for nodes at a given logarithmic distance; returns peer lists
- **EnrRequest/EnrResponse** — Fetches the full ENR of a remote node

### Bonded Peers

Only bonded peers (those that responded to a Ping) are trusted for further communication. The service maintains a `bonded_peers` map with timestamps, and bootnodes are pre-bonded on startup.

### Routing Table (k-buckets)

The routing table (`discovery/kbuckets.rs`) implements k-buckets for efficient peer lookup by node ID distance. Each bucket holds a fixed number of entries, and the table is organized by binary distance from the local node ID.

## RLPx Transport Protocol

### Overview

RLPx (`rlpx/` module) provides encrypted, framed TCP communication between peers. It consists of three layers:

1. **ECIES Handshake** — Key agreement and session secret derivation
2. **Frame encryption** — AES-256-CTR with MAC verification
3. **Protocol messages** — RLP-encoded application-level messages

### ECIES Handshake

The handshake (`rlpx/handshake/ecies.rs`) implements the Ethereum Wire Protocol V4 key exchange:

**Initiator flow:**
1. Generate ephemeral secp256k1 keypair and random nonce
2. Compute static shared secret via ECDH (local static key × remote static key)
3. Sign `XOR(static_shared_secret, initiator_nonce)` with the ephemeral key
4. Construct `AuthMsgV4` containing signature, static pubkey, nonce, and version
5. Encrypt the auth message body using ECIES with the remote static pubkey
6. Send the encrypted auth packet (length-prefixed)
7. Receive and decrypt the ack packet
8. Derive session secrets (AES key, MAC key, init data)

**Recipient flow:**
1. Receive and decrypt the auth packet using the local static key
2. Recover the initiator's ephemeral public key from the signature
3. Generate own ephemeral keypair and nonce
4. Construct `AuthAckV4` with ephemeral pubkey and nonce
5. Encrypt and send the ack packet
6. Derive session secrets

**Session secrets derived:**
- `aes_secret` — AES-256-CTR encryption key
- `mac_secret` — MAC computation key
- `egress_mac_init_data` / `ingress_mac_init_data` — MAC initialization data

### Frame Format

Each RLPx frame (`rlpx/frame.rs`) has the structure:

```
[16 bytes header ciphertext][16 bytes header MAC][N bytes body ciphertext][16 bytes body MAC]
```

**Header (16 bytes, encrypted):**
- Bytes 0-2: Frame payload length (big-endian, 3 bytes)
- Bytes 3-15: RLP-encoded capability and context IDs (e.g., `[0xc2, 0x80, 0x80]` for p2p/eth)

**Body (variable length, encrypted):**
- RLP-encoded message ID followed by message payload
- Padded to 16-byte alignment

**MAC computation:**
- Uses Keccak-256 hash with AES-256 encryption of the running digest
- Header MAC: encrypt the current digest, XOR with header ciphertext, hash the result
- Body MAC: hash the body ciphertext, then encrypt the previous digest, XOR with it, hash again

The `RlpxFrameCodec` maintains separate encryptor/decryptor (AES-256-CTR) and egress/ingress MAC state for each direction.

### Protocol Handshake

After the ECIES handshake, a protocol-level handshake (`rlpx/handshake/protocol.rs`) exchanges:

- **Hello** — Node ID, supported capabilities (e.g., `eth/68`, `eth/69`), listener port, version
- **Status** — Protocol version, network ID, TD (total difficulty), block hash, genesis hash, fork IDs

### Server

The `P2pServer` (`rlpx/server.rs`) listens for inbound TCP connections:

1. Accepts a TCP stream
2. Runs the inbound handshake (ECIES + protocol)
3. Registers the peer in the registry
4. Creates a `PeerSession` and spawns its task
5. Requests the peer's head header to determine chain height

## Peer Management

### PeerManager

The `PeerManager` (`peer/peer_manager.rs`) is the top-level coordinator:

```
PeerManager
├── PeerRegistry    — Active session tracking, peer registration
├── PeerDialer      — Outbound connection management
└── DiscoveryManager — Discovery-to-dial coordination
```

**Key methods:**
- `start()` — Launches discovery and health check loops
- `dial_peer()` — Initiates connection to a specific address
- `dial_enode()` — Parses and dials an enode URL
- `set_gossip_tx()` — Registers the gossip message channel

### PeerRegistry

The `PeerRegistry` (`peer/peer_registry.rs`) manages active peer sessions:

- Maintains a map of peer ID → `RegisteredSession` (session ID + `PeerSession`)
- Tracks network ID, genesis hash, chain config, and chain manager
- Implements `PeerProvider` trait for querying active peers and sessions
- Handles session registration and disconnection events
- Exposes gossip channel for message broadcasting

### PeerDialer

The `PeerDialer` (`peer/dialer.rs`) handles outbound connections:

- **Dial limit** — Semaphore-based concurrency control (max 10 concurrent dials)
- **Deduplication** — In-flight set prevents duplicate dials to the same address
- **Retry tracking** — Records dial attempts with timestamps for backoff
- **Protocol penalties** — Tracks peers that caused protocol errors

Dial flow:
1. Check for in-flight dial to the same address
2. Acquire a semaphore permit
3. Establish TCP connection
4. Run outbound ECIES + protocol handshake
5. Register the session
6. On failure, record the attempt for backoff

### DiscoveryManager

The `DiscoveryManager` (`peer/discovery_manager.rs`) bridges discovery and dialing:

- Receives peer candidates from Discv4
- Filters and prioritizes peers for dialing
- Coordinates with the dialer to maintain target peer count

## Sync Service

### Overview

The `SyncService` (`peer/sync_service.rs`) is the bridge between P2P sessions and the core sync/consensus logic. It handles incoming gossip messages and outgoing requests.

**Constants:**
- `MAX_PEERS = 50` — Maximum peer count

### Gossip Message Handling

The service receives gossip messages via an MPSC channel and dispatches them:

| Gossip Type | Handler |
|---|---|
| `Transactions` | Validates session, forwards to sync provider's `process_gossip_transactions` |
| `NewPooledTransactionHashes` | Fetches missing transactions from the peer |
| `NewBlock` | Validates session, forwards to sync provider |
| `NewPooledTransaction` | Processes individual pooled transactions |

Each handler verifies the session ID matches the current active session before processing.

### Request Types

The sync service supports the following request-response pairs:

| Request | Response | Purpose |
|---|---|---|
| `GetBlockHeaders` | `BlockHeaders` | Fetch block headers by hash or number |
| `GetBlockBodies` | `BlockBodies` | Fetch block bodies (transactions, ommers) |
| `GetPooledTransactions` | `PooledTransactions` | Fetch mempool transactions |
| `GetReceipts` | `Receipts` | Fetch transaction receipts |
| `GetNodeData` | `NodeData` | Fetch trie node data |

### Metrics

The service tracks:
- `GOSSIP_MESSAGES_RECEIVED` — Counter for incoming gossip messages
- `CONNECTED_PEERS` — Gauge for active peer count (in PeerRegistry)

## Bootstrap Registry Integration

### Metrics Server

The `metrics_server/` directory contains a standalone Rust service that provides:

1. **Prometheus metrics collection** — Aggregates node metrics
2. **Node registration** — Nodes register their enode URLs on startup
3. **Bootstrap peer discovery** — Provides bootnode lists to new nodes

Nodes integrate with the metrics server by:
1. Registering their enode URL via HTTP on startup
2. Fetching bootnode lists from the server
3. Reporting metrics periodically

### Bootstrap Flow

1. Node starts and registers with the metrics server
2. Fetches bootnode list from the server or uses CLI-provided bootnodes
3. `DiscoveryV4Service` pings bootnodes on startup
4. Bootnodes respond with Neighbors, populating the routing table
5. DiscoveryManager selects peers from the routing table for dialing
6. Peer connections are established via the dialer

## Error Handling

The P2P stack defines error types in `error.rs`:

- `HeaderMacMismatch` — RLPx frame header MAC verification failed
- `FrameMacMismatch` — RLPx frame body MAC verification failed
- Various handshake and connection errors

Peers that cause repeated errors are tracked in `peer_errors` with counters and timestamps, enabling automatic disconnection and penalty.
