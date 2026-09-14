# Design — HotStuff-1-style speculative confirmation: rollback/prefix-fork analysis (Step 5)

**Status:** research design doc, 2026-09-14 — a prerequisite for the future
ADR-002 question. **This document is not authorization to ship a different
protocol.** No code exists behind it; the measured driver stays the dev/test
Tendermint-shaped two-phase driver (ADR-002).
**Source:** `.agents/memories/latency-candidates.md` (primary-source check),
the Step 5 fault profile, the Step 0 phase decomposition.

## The question an ADR-002 amendment would answer

Can a client-visible early confirmation be safe in this architecture, and
what must be built to make it honest? HotStuff-1 sends clients confirmations
one phase early and resolves the prefix speculation dilemma — streamlined
protocols cannot halt and roll back like stable-leader protocols, so the
speculative chain needs its own voiding rule.

## What the measured driver already gives

- **Fail-closed under leader quorum loss (measured):** 3.01 s to the phase
  deadline, no commit, no conflicting tips; 173 ms heal after repair. The
  driver never finalizes without quorum reach — any speculative layer must
  preserve this property.
- **State discipline (ADR-007):** the committed chain is authoritative; every
  projection (capability history, policies, world state, watcher state) is
  replayable from blocks alone. A rollback engine therefore has a clean
  substrate: nothing but the chain is authoritative.
- **Locking:** per-height Tendermint locking already exists
  (`BftRound.locked`); equivocation detection (#95/#96/#77) records dual
  votes live.

## What one-phase speculation would require (design constraints)

1. **Speculative results are provisional, never final.** They must not
   authorize inventory allocation, payment settlement, endorsement issuance
   or private-payload side effects. The speculative API is read-only
   visibility for clients, clearly labeled — never a commit consumer.
2. **No speculative state.** Because ADR-007 derives everything from the
   committed chain, speculative confirmation must not advance any projection.
   A client-visible speculative ack carries the prevote QC reference, not
   state.
3. **Prefix rule (to be verified against HotStuff-1's text, not invented):**
   a speculative confirmation at height h is valid only while every
   *committed* height below h agrees, and it is voided when a conflicting
   certificate wins at any height ≤ h. Voiding is a client-side fact: the
   chain itself never contains speculative content, so nothing on-chain ever
   rolls back — only client acknowledgments do.
4. **Side-effect boundary is already enforced structurally:** watcher
   automation, PDC dissemination and event-bus consumers all trigger inside
   `after_block_commit` (the committed path). A speculative layer that never
   calls those consumers cannot leak side effects. The rule to keep: *any
   new speculative path must not be plumbed into `after_block_commit`.*
5. **Fault assumptions must be stated:** the early-reply's safety relies on
   the same 2f+1 prevote-quorum intersection argument as the commit path.
   The deviation is only *when the client hears*, not *what can happen*.
   The measured fail-closed deadline bounds how long a client may see a
   speculative ack before finality contradicts it (the "never call it
   final" rule absorbs this).

## Rollback/prefix-fork benches (only after a prototype exists)

- conflicting prevote certificates at one height (equivocation via the
  receipt journal) → all speculative acks above that height voided;
- leader quorum loss while a speculative ack is outstanding (the D7
  leader-loss scenario is the ready-made harness);
- replication lag: speculative ack at height h while h−1 is still
  replicating.

## What this doc does NOT decide

Whether to adopt. That is the ADR-002 question: does GlassChain want
client-visible one-phase confirmations at all, given (a) sub-second is not
yet claimed, (b) the measured wall is receiver-side post-commit cost, not
phase count, and (c) a speculative API is new public surface with misuse
risk in a supply-chain (irreversible-action) domain.
