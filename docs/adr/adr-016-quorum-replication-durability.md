# ADR-016 — "Committed" is quorum-replicated; local disk flush is not the durability promise

**Status:** Accepted
**Date:** 2026-09-14
**Decision owner:** project owner
**Relates to:**
[ADR-002](adr-002-consensus-finality.md) (consensus finality) ·
[ADR-007](adr-007-vm-state-semantics.md) (ledger = authoritative chain, storage = projection) ·
performance plan §4 / residual 4

## Context

A block is committed when the round leader's precommit quorum exists and every
consumer receives the `CommitNotification`. On each node, persistence happens
off the broadcast path: `after_block_commit` applies the block in one atomic
redb write transaction (`RedbStorageProvider::apply_block`), and redb's default
durability makes each commit crash-safe (fsync on commit). Local loss therefore
requires disk failure, corruption, or operator error rather than a crash
window; a quorum-wide failure can still lose acknowledged blocks everywhere.

The performance plan required a decision before any persistent pilot: what does
"committed" promise across process crash, power loss and quorum-wide failure,
and does the periodic flush window break that promise?

## Decision

1. **"Committed" means quorum replication, not local fsync.** Once a block
   carries a quorum certificate, at least `floor(2n/3)+1` validators hold it
   (each committed to its own local store). A single node that loses its local
   copy recovers from its peers: on restart the chain is restored from storage
   (ADR-007), and any prefix absent from local storage is repaired from peers.
   Quorum-wide loss is not within scope of this promise and is treated as a
   disaster-recovery scenario, not a durability mode.

2. **Local stable-storage acknowledgement stays a separate metric.** The
   performance plan already keeps logical finality, replication and local
   stable-storage acknowledgement as three different numbers. redb commits are
   crash-safe by default, so per-node durable ack is already the default;
   `RedbStorageProvider::flush` remains as a parity no-op, and any future
   relaxed-durability mode would be an explicit opt-in measured against the
   finality number, never merged silently into the commit path.

3. **No extra write-ahead log.** redb's commits are atomic and crash-safe, and
   block + write-set replay rebuilds every derived projection (capability
   history, policies, world state, watcher state). A third log adds nothing to
   tail or recovery.

4. **Bounded group commit is deferred.** It exists only for a measured
   throughput-vs-loss-bound trade-off nobody has asked for yet; if a pilot
   needs it, it arrives with the explicit flush mode (item 2), not before.

## Consequences

- The commit path is unchanged: persistence happens after the commit
  notification, so no fsync sits in the finality latency budget, and finality
  at 300 validators keeps its current shape.
- Local disk loss (hardware failure, corruption, operator error) can lose
  acknowledged blocks *on one node*; the promise is honored by replication.
  Replay-from-chain at startup plus repair-from-peers must stay working — that
  is the load-bearing mechanism for ADR-007 and this decision.
- Regression coverage: `glasschain-storage`'s reopen test pins that accepted
  commits survive a process-lifetime crash (redb's crash-safe commits), and node
  integration restart tests pin chain restoration. Peer-replication repair of
  missing local prefixes is exercised by the existing distribution tests.

## Rejected alternatives

- **Flush-at-every-commit as default**: pays a disk flywheel latency inside
  every round to promise something the quorum already promises.
- **A WAL**: a third log for state that is already reconstructible from the
  chain.
