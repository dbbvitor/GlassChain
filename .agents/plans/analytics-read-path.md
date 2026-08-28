# Plan — Analytics read path: provenance, lineage, bounded event bus (ticket #39)

**Ticket:** [#39 Analytics read path](https://github.com/dbbvitor/GlassChain/issues/39)
**Spec:** spec decision 7 · integration-completion Phase 4

## Scope

1. **Node owns the analytics projections** — `ProvenanceIndex` +
   `AnalyticalFlattener` fields on `Node`, ingested on every block commit
   (`after_block_commit`) and rebuilt from committed blocks on start and chain
   replacement (`rebuild_runtime_state_from_chain`), with `Arc` accessors for
   the RPC layer.
2. **RPC wiring** — `ServerState` holds provenance + flattener;
   `QueryAssetHistory` answers from the provenance index (GTIN/SN matching on
   canonical asset ids; `payload_json` carries the custody event — documented
   semantic change); `GetVerifiableLineage` uses
   `VerifiableLineage::build` (custody chain + flat records + trust average),
   dropping the chain scan. `asset_id_for` gains the `SN:<serial>` canonical
   form so serial-only assets stay addressable.
3. **Bounded event bus** — the in-memory event log becomes a capacity-capped
   ring (drop-oldest); the explicit backpressure policy (bounded channel,
   slow consumers receive `Lagged`; publisher never blocks) is documented;
   buffer-fill test proves a slow consumer cannot grow memory.
4. **Tests** — node-level scenarios through the existing server streams:
   provenance-backed `QueryAssetHistory` filtering, GTIN/lot/batch lineage
   with trust averages, live block events; event-bus fill test.
5. No in-tree warehouse writer (out-of-workspace adapter unchanged). README
   documents the query semantic change.

## Out of scope

Bidirectional gRPC streaming (backlog), PostgreSQL/ClickHouse adapter (recipe
already published), integration-test rehoming (scheduled cross-cutting item).
