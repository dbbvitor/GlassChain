# Fabric endorsement-policy and collection-config shapes

**Learned:** 2026-08-20
**Method:** full-content Repomix pack of `hyperledger/fabric` (ignore patterns from
`reference-architectures.md`), grepped incrementally. All claims cite the file that
owns them inside `hyperledger/fabric`.
**Branch:** `research/fabric-policy-shapes` (throwaway — findings only, no source change).
**Why it matters:** resolves GitHub issue #20 and feeds two follow-ups — “What does
state-based endorsement require here?” (endorsement-policy model) and “Confirm the
privacy model (D3)” (private-data collection semantics).

---

## TL;DR — the three policy languages GlassChain could adopt

There are **three distinct policy shapes** in Fabric, not one. They compose.

1. **Signature policies** — the atomic endorsement/membership DSL. A protobuf
   `SignaturePolicyEnvelope`: a `Rule` tree of `NOutOf` / `SignedBy` nodes plus a flat
   `Identities` list of `MSPPrincipal`s (org+role). Expressible as strings:
   `OR('Org1MSP.member','Org2MSP.member')`. **This is the endorsement-policy model to
   copy for GlassChain.**
2. **Implicit-meta policies** — channel/config-level shorthand: `ANY`, `ALL`,
   `MAJORITY` of a *named sub-policy* evaluated across all member orgs. Expandable at
   validation time into a concrete signature policy. Relevant if GlassChain wants
   org-counting defaults (“any org can endorse”, “all orgs must”, “majority must”).
3. **Collection configs** — not a policy language per se, but a per-chaincode JSON
   bundle that ties a **membership signature policy** to dissemination/persistence
   knobs (`requiredPeerCount`, `maximumPeerCount`, `blockToLive`, `memberOnlyRead`,
   `memberOnlyWrite`) and an **optional per-collection endorsement policy**. This is
   the private-data semantics model for the D3 follow-up.

Per-key (state-based) endorsement reuses shape 1: it stores a serialized
`SignaturePolicyEnvelope` as **metadata on the key** and, at validation, substitutes
it for the chaincode default for writes to that key.

---

## 1. The signature-policy language

### Representation — `SignaturePolicyEnvelope`

`common/policydsl/policydsl_builder.go` builds it; `common/cauthdsl/cauthdsl.go`
evaluates it. Shape:

- **`Rule`** — a tree. Leaf nodes are `SignedBy(int)` (an index into `Identities`);
  internal nodes are `NOutOf(N, rules…)`. `And(a,b)` ≡ `NOutOf(2,[a,b])`;
  `Or(a,b)` ≡ `NOutOf(1,[a,b])`. `AcceptAllPolicy` = `NOutOf(0,[])`;
  `RejectAllPolicy` = `NOutOf(1,[])`.
- **`Identities`** — a flat list of `MSPPrincipal`s. A principal is
  `{PrincipalClassification, Principal}`; the common classifications are
  `ROLE` (`MSPRole{MspIdentifier, Role}`) and `IDENTITY`. Roles: `member`,
  `admin`, `client`, `peer`, `orderer`.
- **`Version`** — must be `0`; `NewPolicyProvider` rejects other versions
  (`common/cauthdsl/policy.go`).

### DSL grammar — `FromString`

`common/policydsl/policyparser.go` parses:

```
GATE(P[, P…])            # GATE ∈ {AND, OR, OutOf} (case-insensitive)
P :=  ORG.ROLE           # e.g. Org1MSP.member | Org1MSP.admin | …peer/orderer/client
      | nested GATE(...)
```

`OutOf(N, 'Org1MSP.member', 'Org2MSP.member')` → require N of them. The parser runs
three passes (string → normalized string → proto tree) and is the exact syntax seen in
chaincode definitions, collection membership, and state-based policies.

### Evaluation — `cauthdsl` semantics (important!)

`common/cauthdsl/cauthdsl.go` compiles the tree into a closure over the endorsement
signature set:

- `SignedBy(i)` is satisfied by **one** signature whose identity satisfies principal
  `i`; a `used[]` bit-array marks that signer as consumed.
- `NOutOf(N, rules)` succeeds if **N distinct** sub-rules each succeed **using
  distinct signers**.
- Consequence: one org’s member signature cannot double-satisfy two different
  principals. `AND('A.member','B.member')` therefore forces **different orgs** to sign.
  This is what makes “cross-org quorum” true rather than just “N signatures”.

`common/cauthdsl/policy.go` wraps the closure as `policies.Policy` with
`EvaluateSignedData`/`EvaluateIdentities`; `common/policies/policy.go` defines the
`Policy`/`PolicyManager` interface and the path-addressed channel policy tree
(`Channel/Application/Orderer` groups with named policies like `Readers`, `Writers`,
`Admins`, `BlockValidation`).

### Implicit-meta policies — org-counting shorthand

`common/policies/implicitmeta.go`, `implicitmetaparser.go`:

- `ImplicitMetaPolicy{Rule, SubPolicy}` with `Rule ∈ {ANY, ALL, MAJORITY}`
  → threshold of sub-policies that must pass (ANY=1, ALL=len, MAJORITY=len/2+1).
- Evaluated by applying the same signature set against each member org’s sub-policy
  and counting.
- `common/policies/convert.go` — `Convert()` expands an implicit-meta policy into a
  concrete `SignaturePolicyEnvelope` by merging sub-policy envelopes (with principal
  dedup and `SignedBy` remapping). This is the bridge that lets config-time
  defaults become real signature policies at validation/translation time.

### Option trade-offs for GlassChain’s endorsement model

| Option | What you get | Cost / risk |
|---|---|---|
| **A. SignaturePolicyEnvelope only** | One self-contained value: tree + principal list, serialized, hashable, stored in the chaincode/contract definition. Full expressiveness (n-of-m, cross-org AND, role-aware). | Steeper authoring surface than “pick N orgs”; need a parser/builder (string DSL or API). |
| **B. Implicit-meta only** (ANY/ALL/MAJORITY of a named sub-policy) | Terse defaults; auto-adapts as orgs join/leave the channel (evaluated dynamically against current org set). | Not granular; cannot express n-of-m or per-role exceptions without escaping to A. |
| **C. Hybrid (Fabric’s actual design)** | Default via B (config `Readers/Writers/Admins`), override per-chaincode/per-collection/per-key via A. | Two concepts to implement + a conversion step. |

Fabric’s own *endorsement* is always A (a concrete signature envelope on the
chaincode/collection/key); B is reserved for channel **config** policies. A natural
GlassChain model: contract-level EP = signature policy (A); if org-counting defaults
are wanted, provide a thin B-like convenience that compiles to A.

---

## 2. State-based (key-level) endorsement — SBEP

### Storage: a key’s metadata, not its value

Chaincode API (`SetStateValidationParameter(key, epBytes)` /
`GetStateValidationParameter(key)`, see `integration/chaincode/keylevelep/chaincode.go`)
translates into a **metadata write** on that KVS key under the reserved metadata key
`VALIDATION_PARAMETER` (`peer.MetaDataKeys_VALIDATION_PARAMETER`; confirmed in a ledger
test helper, line ~83947: `simulator.SetStateMetadata(cc, key, map[string][]byte{vpMetadataKey: policy})`).

- SBEP is therefore part of the **world-state metadata**, written in-band by the
  transaction that sets it (so changing a key’s EP is itself an endorsed, MVCC-tracked
  write). `nil` clears it back to “use defaults”.
- In v1.3 the stored bytes are a raw `SignaturePolicyEnvelope`; from v2.0 the stored
  `SignaturePolicyEnvelope` is wrapped as an `ApplicationPolicy{SignaturePolicy}`
  because the v2.0 evaluator consumes `ApplicationPolicy`
  (`toApplicationPolicyTranslator`, `core/handlers/validation/builtin/v20/validation_logic.go`).

### Evaluation at validation time

`core/common/validation/statebased/validator_keylevel.go` — `KeyLevelValidator.Evaluate`
walks every write **and metadata-write** in the tx RW-set (public + per-collection,
hashed) and, for each key, calls `checkSBAndCCEP`:

1. Look up `GetValidationParameterForKey(cc, coll, key, blockNum, txNum)`.
2. **If a key-level VP exists** → validate the endorsement signature set against **it**
   (SBEP wins for that key).
3. **If none** → fall back to `CheckCCEPIfNotChecked`: evaluate the **collection-level**
   EP (v2.0 only) if the write is into a collection that defines one, else the
   **chaincode-level** EP.
4. Ends with `CheckCCEPIfNoEPChecked` — at least **one** EP must have matched, so a bare
   key write that bypasses every override still needs the chaincode EP (FAB-9473).

So the resolution precedence is **key-level → collection-level → chaincode-level**,
with SBEP *refining* (not weakening) the chaincode default — a key override only ever
adds constraints, because the fallback chain still requires the cc EP when the key has
no override.

### Within-block consistency (the subtle part)

`core/common/validation/statebased/vpmanager.go` + `vpmanagerimpl.go`:

- Validation parameters are retrieved per `(cc, coll, key)` at a specific
  block/tx height. A `KeyLevelValidationParameterManager` tracks, per block, which
  transactions write which keys’ metadata (`ExtractValidationParameterDependency`) and
  waits / short-circuits (`ValidationParameterUpdatedError`) if an earlier tx in the
  same block changed the param, so a tx can’t endorse against a param that a later
  sibling also changes. `PostValidate` records results so dependents unblock.
- Errors are triaged: deterministic ledger errors (collection not defined) are logged
  and skipped; anything unexpected halts channel processing (no fork risk).

### Proof of behavior — `integration/sbe/sbe_test.go`

Two-org etcdraft network, chaincode EP `OR('Org1MSP.member','Org2MSP.member')`,
V2_0 application capabilities:

- Org1 alone sets key value and adds itself to the key SBEP ⇒ afterwards **Org2 alone
  is rejected** (`ENDORSEMENT_POLICY_FAILURE`), Org1 alone succeeds.
- After Org1 adds Org2 ⇒ a write endorsed by **both** orgs succeeds; either org alone is
  rejected (the SBEP composed as AND-of-orgs).
- Updating/removing an org from the key SBEP requires satisfying the *current* EP
  (Org2 alone can’t delete Org1 from an AND(Org1,Org2) EP; both must).
- Key-level EP works identically for public keys and for private-data collection keys
  (`SetPrivateDataValidationParameter`).

### SBEP trade-offs for GlassChain

| Aspect | Note |
|---|---|
| Expressiveness | Same signature-policy language as cc EP — no new DSL needed. |
| Storage | Requires per-key **metadata** distinct from the value + world-state reads of that metadata at validation. GlassChain would add a metadata channel alongside contract state. |
| Determinism | Must be stored/read deterministically (in-band write, MVCC-tracked), not as a side table the endorser mutates out-of-band. |
| Complexity | The within-block dependency ordering (`vpmanagerimpl`) is the hardest part — needed only if a single block can both change a key EP and write that key. |
| Capability gating | SBEP is a v1.3+ feature; GlassChain (pre-1.0, no compat burden) can adopt it directly. |

---

## 3. Collection-config syntax and semantics

### The JSON shape (authoring form)

Parsed by `GetCollectionConfigFromFile` →
`core/chaincode/... ` (struct `collectionConfigJson`, `internal/...`/`GetCollectionConfigFromFile`,
lines ~305020–305135 in the pack, under `internal/peer/lifecycle`/`collection`):

```json
[
  {
    "name": "collectionMarblePrivateDetails",
    "policy": "OR('Org1MSP.member','Org2MSP.member')",   // MEMBERSHIP signature policy
    "requiredPeerCount": 0,          // default 0
    "maxPeerCount": 2,               // default 1
    "blockToLive": 100,              // default 0 == no expiry (MaxUint64)
    "memberOnlyRead": false,         // default false
    "memberOnlyWrite": false,        // default false
    "endorsementPolicy": {           // optional per-collection EP (new lifecycle)
      "signaturePolicy": "OR('Org1MSP.member','Org2MSP.member')"
      // OR "channelConfigPolicy": "Channel/Application/...named policy..."
    }
  }
]
```

- `policy` is parsed via `policydsl.FromString` into a `CollectionPolicyConfig`
  carrying a `SignaturePolicy`. The **member orgs** are the MSP IDs that appear as
  principals in that envelope (`getMemberOrgs`), so membership == “whose identities are
  named in the policy”.
- Defaults when omitted: `requiredPeerCount=0`, `maxPeerCount=1`.

### Semantics of each knob

`core/common/privdata/collection.go`, `simplecollection.go`, `membershipinfo.go`:

- **`RequiredPeerCount`** — minimum peers private data must be disseminated to at
  endorsement; **endorsement fails** if that number isn’t reached
  (`CollectionAccessPolicy.RequiredPeerCount()`).
- **`MaximumPeerCount`** — ceiling on dissemination targets; must be ≥
  `RequiredPeerCount`.
- **`BlockToLive`** — blocks after which the collection data is purged (a key last
  modified at block N is purged at N + BTL + 1); `0` == `MaxUint64` (no expiry). Enforced
  by `core/ledger/kvledger/txmgmt/pvtstatepurgemgmt/`.
- **`memberOnlyRead` / `memberOnlyWrite`** — whether non-members may read/write private
  data; membership is decided by `AccessFilter` (evaluate signed identity against the
  membership policy).
- **Membership vs. endorsement are separate concepts.** Membership = who may
  read/write/disseminate the data. Endorsement = whose signatures are required for a
  write to commit. A per-collection `endorsementPolicy` (v2.0) can be **stricter than**
  membership (e.g. members are Org1+Org2 but both must endorse every write).

### Endorsement-policy interaction (v2.0)

`core/common/validation/statebased/v20.go` — `policyCheckerV20.CheckCCEPIfNotChecked`:
when a tx writes into collection `c`, look up that collection’s EP
(`CollectionResources.CollectionValidationInfo`); if one is defined, evaluate it, else
fall through to the chaincode EP. Same key-level refinement applies inside the
collection. So the precedence is uniform: **key → collection → chaincode**.

### Implicit per-org collections (v2.0)

`core/chaincode/implicitcollection/name.go` — every org gets an automatic collection
named `_implicit_org_<mspID>` (member = that one org), no explicit config needed. Used
for org-private state (e.g. chaincode-definition approvals). Enables per-org secrets
without defining a collection.

### Composition & storage

- `CollectionConfigPackage{Config: []*CollectionConfig}` is a list; each `CollectionConfig`
  is one of `CollectionConfig_StaticCollectionConfig` (the common case).
- The package is stored as the value of a reserved key built from the chaincode name
  (`BuildCollectionKVSKey(cc) = cc + "~collection"`) in the lifecycle/`lscc` namespace
  (`core/common/privdata/collection.go`) — i.e. collections are part of the **chaincode
  definition**, committed via lifecycle, versioned and read deterministically.

### Collection trade-offs for the D3 privacy follow-up

| Decision | Fabric model to steal | GlassChain note |
|---|---|---|
| Membership | Signature policy over org+role principals; “owning orgs” derived from it | Reuse the same policy type as endorsement → one evaluator. |
| Dissemination | `requiredPeerCount`/`maximumPeerCount` (gossip/distributor) | GlassChain can map to libp2p fan-out; decide required-vs-max per collection. |
| Retention | `blockToLive` purge | Deterministic purge is the integration point with the ledger/state store. |
| Read/write | `memberOnlyRead`/`memberOnlyWrite` flags | Cheap, orthogonal access control. |
| Isolation | Separate implicit per-org collections | Good default for per-org private state before real multi-org collections. |
| Config lifecycle | Part of the chaincode/lifecycle definition, committed + versioned | Mirrors GlassChain’s existing tx/contract definition flow. |

---

## 4. Capability framework and the upgrade path

### Mechanics — `common/capabilities/`

- `capabilities.go`: a `registry` couples a `provider` (which knows the capability
  strings) to the channel’s required `map[string]*cb.Capability`. `Supported()` iterates
  required capabilities and fails the peer if `HasCapability` is false — a peer that
  can’t honor a raised capability **refuses to participate** rather than silently
  mis-validating.
- `application.go` / `channel.go` / `orderer.go`: one provider per config group, each a
  struct of booleans (`v11…v25`) set from which capability strings are present, exposing
  named feature predicates:
  - `V1_2Validation()` — v1.2+ (collection access / stricter lscc validation)
  - `V1_3Validation()` — v1.3+ (adds **key-level endorsement**, SBEP)
  - `V2_0Validation()` — v2.0+ (new lifecycle, implicit per-org collections)
  - `KeyLevelEndorsement()` — v1.3+ (SBEP)
  - `PrivateChannelData()` — collections (v1.2+, exp. before)
  - `PurgePvtData()`, `StorePvtDataOfInvalidTx()`, `CollectionUpgrade()`, …
- Predicates are **inclusive or / stair-step**: a higher level implies all lower
  features (e.g. `V2_0Validation()` true also means `V1_3Validation()`, `V1_2Validation()`
  true).

### How capability selects validation logic

`core/handlers/validation/builtin/default_validation.go` — `DefaultValidation.Validate`
is a switch over the capability provider of the channel:

```go
switch {
case v.Capabilities.V2_0Validation():  // v20 validator
case v.Capabilities.V1_3Validation():  // v13 validator (adds SBEP)
case v.Capabilities.V1_2Validation():
default:                                // v12 validator (baseline)
}
```

Parallel directories hold the actual logic; each wires its own evaluator:

- `v12/validation_logic.go` — plain single policy evaluation against the supplied
  policy, plus LSCC-specific checks; no key-level, no collection-level EP.
- `v13/validation_logic.go` (+ `validator.go`) — builds
  `KeyLevelValidationParameterManagerImpl{PolicyTranslator: noop}` +
  `statebased.NewV13Evaluator` + `KeyLevelValidator`; checks cc EP **and** SBEP.
- `v20/validation_logic.go` (+ `validator.go`) — same + `NewV20Evaluator` (adds
  collection-level EP via `CollectionResources`) and
  `{PolicyTranslator: toApplicationPolicyTranslator}` so a v1.3-style stored signature
  envelope is re-wrapped as an `ApplicationPolicy` for the v2.0 evaluator.

### The upgrade path

Raising a channel’s Application capability level (a config update, itself endorsed and
committed) flips the predicates and **immediately selects the newer validator for
blocks validated from that point on**. Key properties to preserve:

1. **Old rules are kept, not retroactively applied** — the versioned dirs coexist; a
   channel at v1.2 keeps v1.2 semantics for the ledger it already has. Raising the level
   is forward-looking: new validation rules apply to subsequently validated transactions.
2. **Raising is monotone and safe only if every peer running the new logic agrees** —
   `registry.Supported()` is the guard: a peer still on old binaries refuses to join a
   channel that requires capabilities it lacks. Hence upgrades are coordinated binary +
   config-level changes.
3. **Protocol-level behavioral changes need a fork point** — because ledger rules can’t
   change retroactively, any change to what makes a tx valid must be gated (new dir +
   new capability), never mutated in place.

### Capability trade-offs / relevance for GlassChain

GlassChain currently has a bare `PROTOCOL_VERSION` constant and no capability concept
(see `reference-architectures.md` finding #8). ADR-003 requires a wire-protocol change,
so this becomes relevant soon:

| Approach | Trade-off |
|---|---|
| Single `PROTOCOL_VERSION` constant | Simple, but no per-channel granularity; a version bump is all-or-nothing network-wide and can’t let one channel opt in. |
| Per-config-group capability map (Fabric-style) | Lets each channel raise validation independently and keeps old rules for un-migrated history; adds a config subsection + a versioned-validator dispatch. |
| Feature-bit predicates over a record of capabilities | Middle ground: keep one versioned record but expose per-feature booleans so logic selects minimally. |

For a pre-1.0 chain with no external compat burden, the pragmatic path is a single
versioned validation record per channel exposed as named predicates, with validation
logic versioned in parallel dirs so a future rule change forks rather than mutates.

---

## Sources

All inside `hyperledger/fabric` (packed, ignore patterns per `reference-architectures.md`):

- **Policy language:** `common/policydsl/policydsl_builder.go`,
  `common/policydsl/policyparser.go`, `common/cauthdsl/cauthdsl.go`,
  `common/cauthdsl/policy.go`, `common/policies/policy.go`, `common/policies/implicitmeta.go`,
  `common/policies/implicitmetaparser.go`, `common/policies/convert.go`.
- **SBEP:** `core/common/validation/statebased/validator_keylevel.go`, `v13.go`, `v20.go`,
  `vpmanager.go`, `vpmanagerimpl.go`; `integration/chaincode/keylevelep/chaincode.go`;
  `integration/sbe/sbe_test.go`; key-metadata write in ledger test (`putSBEP`, ~line 83947).
- **Collections:** `core/common/privdata/collection.go`, `simplecollection.go`,
  `membershipinfo.go`; collection-config JSON parse (`collectionConfigJson`, ~lines 305040–305135);
  `core/chaincode/implicitcollection/name.go`.
- **Capabilities/versioned validation:** `common/capabilities/capabilities.go`,
  `application.go`, `channel.go`, `orderer.go`;
  `core/handlers/validation/builtin/default_validation.go`;
  `core/handlers/validation/builtin/v12|v13|v20/validation_logic.go`, `validator.go`.

Cross-reference: this study is the implementation-detail companion to the structure-level
findings already in [`reference-architectures.md`](./reference-architectures.md)
(consensus, capabilities gating, private-data decomposition).
