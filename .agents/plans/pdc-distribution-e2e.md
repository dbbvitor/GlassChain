# Plan — Private payload distribution end-to-end (ticket #47)

**Ticket:** [#47 PDC dissemination end-to-end](https://github.com/dbbvitor/GlassChain/issues/47)
**Builds on:** #46 (point-to-point member-scoped delivery, transient store,
four-boundary enforcement).

## The transport decision (stated openly)

The dissemination transport is the **TLS-authenticated TCP mesh with
member-scoped targeting** built and tested in #46. Publishing cleartext
payloads to gossipsub topics would *weaken* the boundary: gossipsub has no
member admission control, so any peer could subscribe and receive. The
libp2p swarm (gossipsub + Kademlia) stays the staged substrate; per-member
payload encryption is the prerequisite for gossip-based cleartext distribution
and is out of scope here. Kademlia discovery remains available in the swarm
for the #48 benchmark substrate.

## Changes

### 1. Reconciliation (pull) — the offline catch-up path
- `Message::RequestPrivatePayload { collection, commitment }` — a member asks
  a member peer for one payload it is missing. The receiver responds with the
  existing `Message::PrivatePayload` (same receive boundary: membership +
  commitment checks) if it holds the payload; silence otherwise.
- `Node::reconcile_private_payloads(collection) -> Result<usize>`: scan the
  committed chain for PDC writes in `collection`; for every commitment the
  local member lacks in its transient store, send one request to a member
  peer. Returns the number requested.
- `PROTOCOL_VERSION` → `glasschain/4` (new wire message in the private-payload
  protocol; a `/3` peer cannot answer requests).

### 2. Retention and purge (per collection)
- `ChannelConfig.retention_secs: u64` (`#[serde(default)]` = 259_200 = 72h,
  the ADR-003 decision 4 transient window).
- `TransientStore`: entries carry an `expires_at` envelope; `purge_expired(
  now) -> usize` deletes expired entries — payloads vanish, the chain's
  commitments persist forever.
  `ponytail:` the expiry index is in-memory (a `Mutex<HashMap<key,
  expires_at>>` filled on put); a restart forgets it, so a restarted member
  cannot enumerate stale payloads until overwritten — a storage `list`
  capability is the real fix and lands with the #48 substrate work if needed.
- `Node::purge_expired_private_payloads() -> usize`.

### 3. Certificate-verified delivery (closing the bare-TOFU gap)
- `CertChainVerifier::verified_subject_cn(peer_cert_der)`: verify the chain
  and return the peer certificate's subject CN (the member identity stamped at
  issuance, `msp.rs`).
- `VerifiedPeer.org_verified: bool` — set in the Hello arm when a
  `cert_verifier` is configured and the verified subject CN equals the claimed
  org; `false` under bare TOFU.
- Payload path: when the LOCAL node has a `cert_verifier` configured, the
  sender must be org-verified — the self-asserted Hello org is no longer
  trusted for private data.
- Org drift: `verify_or_register` rejects a returning peer whose claimed org
  changed (first verified claim wins).

### 4. Node-level scenarios (AC5)
- **Member receipt**: covered in #46; extended to the reconciled path.
- **Offline catch-up**: a member joins after dissemination, reconciles, and
  receives exactly the missing payloads.
- **Purge**: short retention, purge removes the payload, the commitment stays
  verifiable on the chain (block still valid, commitment equal to
  sha256(payload) recorded pre-purge).
- **Identity-verified delivery**: two identity-backed nodes under one org Root
  CA — payload accepted with a verified org; a peer whose cert CN does not
  match its claimed org is rejected; the payload from an unverified sender is
  rejected when the local node requires verification.

## As shipped (deviations)
- `TransientStore.put` takes `retention_secs` per call (from the configured
  collection); `delete` was replaced by `purge_expired` + read-time expiry.
- The Hello now carries the organization-issued certificate (PEM) and
  certificate verification runs at the app layer (Step 2.5) — the TLS
  certificate stays a transport-only self-signed cert, because a leaf-anchored
  TLS trust store cannot validate a CA-issued leaf. Step 3's old TLS-cert
  verification was removed (it could never succeed against self-signed certs).
- The pre-existing step-3 note about `GLASSCHAIN_INSECURE_TLS` remains
  unchanged and untouched.

## Out of scope (recorded)
- gossipsub cleartext payload distribution (requires per-member encryption);
  the libp2p swarm stays staged for #48.
- A storage-level key scan for purge-after-restart (in-memory index only).
