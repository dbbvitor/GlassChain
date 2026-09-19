# Demo viewer-scoped fetch: response-discard starvation

**Found:** 2026-09-18 (admin cert drawer showing `0 payload(s)` for recent lots).

## Symptom

In Admin (demo) view, opening a recent certificate/lot drawer showed
`Readable to the current viewer: 0 payload(s)` even though
`GET /api/snapshot?as=admin` held every payload for that lot. Older lots in
the same run could be fine.

## Root cause

The page refreshed the viewer-scoped snapshot on every 500 ms tick and guarded
responses with a monotonic request id:

```js
const request = ++orgRequestId;
const data = await fetch(...);
if (request !== orgRequestId) return;   // discard stale
```

The admin lens is the whole private-payload collection, so it grows with the
run (~8 rows per lot). Once the fetch took longer than the tick — large run,
slow machine, debug build — a new request started before the previous resolved,
and **every** response was discarded as stale. `state.orgView` never got set
(or kept an older view), so lot/cert drawers filtered an empty list.

Reproduced deterministically by delaying only `?as=admin` responses by 800 ms
in a DOM harness: 0 payloads vs 10 ground truth.

## Fix and rule

- One in-flight fetch at a time; responses are invalidated only by a **view
  change**, never by a newer request (`demo/static/app.js` `refreshOrg`).
- The viewer snapshot is tagged with the view it belongs to (`orgViewFor`);
  consumers refuse mismatched data.
- The admin lens is fetched **on demand** (drawer open) plus a 2 s freshness
  window; member lenses stay on the tick cadence.
- Drawers `await refreshOrg()` before filtering payloads.

Rule: never let a periodic refetcher discard responses just because it
scheduled the next request; dedupe in flight instead. If a poll can be slower
than its period, a self-scheduling loop with a "latest wins" guard starves
itself.
