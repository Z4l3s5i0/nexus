### Geth metric availability

The execution image is pinned by digest in
`experiments/shared/network/start.sh`. The metrics server stores raw Geth Prometheus names, labels,
and the registered node's `geth` client identity; these names are not renamed
to `wasix_eth` metrics. The Geth endpoint is `/debug/metrics/prometheus`.

The actual inventory and semantics of this pinned image have **not yet been
verified in a running network**. Capture the exposition from a running Geth
node (replace the index and port as needed) and save it with experiment results:

```sh
docker compose -f docker-compose.v2.yml exec -T metrics-server \
  curl -fsS http://el-node-0:9050/debug/metrics/prometheus > geth-metrics.txt
```

Compare the inventory and metric descriptions with this report before asserting
equivalence. Geth changes its metrics between versions; see
https://geth.ethereum.org/docs/monitoring/metrics.

| Existing measurement | Status / provenance | Gap or proposed instrumentation |
|---|---|---|
| Chain ID, canonical head height, peer count, gas price | Common JSON-RPC methods, contingent on successful responses; gas price is a client estimate. | Compare the raw RPC observations and their timestamps. |
| Sync state | Common `eth_syncing`: `ethereum_syncing` is 1 while syncing, 0 when not syncing. This is not the `wasix_eth` `sync_status` encoding and cannot identify stalled nodes. | Use explicit state provenance; add common stall detection if required. |
| RPC success and latency | Collector-side observations per method and node; latency includes network and collector overhead. | Instrument server-side requests separately for execution-only latency. |
| Imported blocks, committed transactions | No verified common import/event counter. Canonical head height is not an import count. | Derive canonical blocks and transaction counts via RPC, handling reorganizations; instrument import events for exact counts. |
| Mempool size, rejections, evictions | No verified matching Geth mapping; pending and queued pools have different scopes. | Verify pool metrics or RPC scope; add reason-specific rejection and eviction instrumentation. |
| Gossip messages, P2P bytes, RPC request totals, sync target | No verified matching counters or accounting boundaries. | Inventory raw Geth metrics; add aligned protocol/RPC instrumentation and label approximations. |
| CPU, memory, disk, network | Not per-client measurements in this metrics database. | Collect per-container or per-process samples through the same server; host exporters are not substitutes. |
| Log-derived timing and events | `evaluate_logs.py` patterns are specific to `wasix_eth`; Geth is unavailable. | Add structured common client events or derive timings from chain/RPC observations. |

Missing data is unavailable, not zero. No Geth-to-`wasix_eth` Prometheus name
mapping should be promoted as equivalent until its semantics are verified.