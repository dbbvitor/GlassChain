# Plan — Committed write sets with atomic block-plus-state apply (ticket #41)

**Ticket:** [#41 Committed write sets with atomic block-plus-state apply](https://github.com/dbbvitor/GlassChain/issues/41)
**Spec:** spec decision 2 (VM state semantics) · ADR-007

## Scope

1. **Block carries the write set** (`glasschain-core`) — `Block.write_set:
   Vec<PersistentWrite>` joins the hash-covered canonical content; the existing
   `Block::new` keeps its signature (empty write set) and a new
   `Block::with_write_set` carries the accepted writes, so the ~48 existing
   call sites are untouched. `PersistentWrite::block_form()` redacts
   PDC-scoped values to their SHA-256 commitment — the block never holds a
   private value, only the collection name and commitment — and
   `PersistentWrite::state_key()` defines the world-state key
   (`ws:<channel>:<contract>:<key>`).
2. **Atomic boundary on the seam** — `StorageProvider::apply_block(&Block)`:
   persists the block **and** applies its write set, with a stale-tip check
   (`previous_hash`/index must match the stored tip) inside the same atomic
   section, so a stale candidate is rejected whole — block and write set
   together. Default impl = sequential fallback (documented non-atomic);
   `InMemoryStorageProvider` and `SledStorageProvider` override with real
   atomic sections (block+state locks; sled multi-tree transaction).
3. **Node wiring** — `NodeState` gains the derived world-state cache
   (`world_state: HashMap<String, Vec<u8>>`) and an executor handle. At
   mining, the node executes each `ContractExecution` in the candidate block
   against the committed snapshot (via the registered `ExecutionProvider`),
   canonicalizes the writes, redacts PDC values, and builds the block with the
   write set. A failed execution accepts no writes (deterministic, so every
   node computes the identical set). `after_block_commit` calls
   `storage.apply_block` and updates the cache from the block's write set; on
   rejection the chain stays authoritative and the next rebuild heals the
   storage divergence. `start` persists any chain blocks missing from storage
   (a fresh node's genesis) through the same atomic boundary so later blocks
   can chain to it.
4. **Replay without re-execution** — `rebuild_world_state(storage)` reads
   committed blocks in order and applies their write sets to the cache and
   storage state (healing a partial apply). Wired into
   `rebuild_runtime_state_from_chain`; no path executes WASM to rebuild state.
5. **Docs** — PLUGIN_KIT.md gains `apply_block` in the `StorageProvider`
   contract; README notes the block now carries the canonical write set.
6. **Tests** — hash covers the write set; PDC redaction; stale-tip rejection
   (in-memory + sled, one `InvalidBlock` shape across backends); node-level
   persistence + restart rebuild without re-execution (in-memory and
   sled-backed); failed execution accepts no writes; PDC commitment shape in
   the committed block; failure-injection recovery (block durable, state not
   → rebuild heals).

## Out of scope

- PDC dissemination/reconciliation and transient store (#46/#47): the private
  value travels the ADR-003 path; only its commitment enters the block.
- Endorsement authorization of writes (#45).
- Re-execution-based state verification: none — replay consumes write sets.
