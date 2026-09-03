# GlassChain Domain

GlassChain coordinates supply-chain offers, inventory automation, and purchase commitments across participating organizations.

## Language

**Supply offer**:
An offer from a seller describing available product, price, currency, quantity, and lead time for a buyer to evaluate.
_Avoid_: inventory offer

**Inventory trigger**:
A rule that watches an inventory condition and may generate a purchase order when the condition is met.
_Avoid_: reorder service

**Approval gate**:
A decision point that must approve an automated purchase before the purchase order is emitted. A denial or failed active gate prevents the automated purchase.
_Avoid_: validation hook

**Purchase order**:
The supply-chain commitment generated when an accepted supply offer or inventory trigger authorizes an automatic purchase.
_Avoid_: order, purchase transaction

**Member organization**:
A participating organization holding a verifiable MSP identity; may submit and endorse transactions.
_Avoid_: participant, peer (a peer is the network process, not the organization)

**Federation trust store**:
The operator-configured set of peer-organization Root CAs a node accepts as certificate issuers, alongside its own organization's Root CA. An organization outside the store stays connected but is not trusted on organization-gated paths (private payloads). Distribution between organizations is manual and out-of-band; trust anchors are configuration and persist across restarts.
_Avoid_: shared CA (no single root exists), on-chain registry (a considered but rejected alternative, ADR-011)

**Validator**:
A member organization that participates in block finality voting. In v1 every member organization is a validator (full participation); bounding the validator set later is configuration, not redesign — the consensus family supports per-height validator-set changes. Validating is an operational role, not a status: it confers no read access, no authority to authorize a business change, and no fee or settlement advantage. Bounding the set is a liveness requirement (a quorum needs ⅔+ of validators responsive), not an exclusion mechanism.
_Avoid_: miner (the retired proof-of-work role); tier, rank (validator is a role a member holds, not a class it belongs to)

**Zero trust**:
The network's consensus posture: no participant is trusted by default. Commercial rivals may operate validators, so finality must not depend on any single operator's honesty.

**Light client**:
A member organization that submits transactions and queries state through authenticated gRPC without operating a validator, verifying block headers against the validator set's signatures. It takes state validity on trust from the quorum and therefore cannot detect an invalid state transition. At national scale most members are light clients; provenance guarantees are unchanged because every submission is signed by an MSP identity.
_Avoid_: full node, verifier (a light client verifies headers, not state)

**Verifying member**:
A member organization that holds the full committed chain and independently re-executes and rechecks it — signatures, hashes, endorsement policies, schema, and state transitions — without voting. Verification is a unilateral local act requiring no membership in any set, so every member may be one. Only a verifying member can detect an invalid state transition; a light client cannot.
_Avoid_: light client (a different guarantee), non-voting observer (understates it — verifying members also endorse)

**Governance standing**:
A member organization's right to vote on network parameters, schema versions, and protocol rules. It attaches to membership, never to validation: one member organization, one governance vote, whether or not it operates a validator. Tying governance to validation would disenfranchise the members least able to run infrastructure and turn the validator set into a cartel.
_Avoid_: validator weight, stake (no economics model exists; voting power remains an open question in ADR-002)

**State commitment**:
An on-chain anchor — a Merkle root plus the counterparties' MSP signatures — attesting to a batch of off-chain events (telemetry, high-frequency inventory transitions). Raw events live off-chain; the commitment makes them tamper-evident and globally ordered.
_Avoid_: rollup (implies core-chain re-execution; the core does not re-execute)

**Ephemeral execution output**:
A contract result visible only to the current invocation and its caller. It can drive an approval decision but does not change committed contract state.
_Avoid_: state write (when no persistence was requested)

**Persistent state write**:
An explicit contract request to set or delete scoped contract state. It becomes part of the committed transaction only when accepted into the globally ordered chain; persistence is never implied by an invocation-local output.
_Avoid_: implicit world-state mutation

**Committed write set**:
The immutable, ordered set of accepted persistent state changes attached to a committed block. Materialized state storage is derived from it and can be rebuilt from the chain.
_Avoid_: state sidecar (as the source of truth)

**State scope**:
The explicit channel, contract, key, and visibility boundary for a persistent state write. Public values are globally verifiable; PDC values remain private while their commitments are anchored publicly.
_Avoid_: global keyspace

**Endorsement policy**:
A rule over verified MSP principals that authorizes a transaction or scoped state change. It is application authorization, separate from BFT finality and PDC membership.
_Avoid_: consensus quorum (when discussing business authorization)

**State-based endorsement policy**:
An endorsement policy attached to a persistent state scope that can add constraints beyond the channel or contract default. It is versioned with committed history and cannot weaken an applicable base policy.
_Avoid_: mutable policy side table

**Endorsement principal**:
A verified MSP organization member whose signature can satisfy a policy rule. Distinct principals count separately; duplicate signatures or multiple nodes from one organization do not create extra organizational approvals.
_Avoid_: caller-supplied organization label

**Private data collection (PDC)**:
A named set of member organizations authorized to hold a class of private payloads. Only a hash commitment is written to the global chain; payloads are disseminated point-to-point to collection members, purge after the collection's retention window, and remain pullable by late peers for the retention period. Regulator organizations are members of every collection by default.
_Avoid_: side-ledger, private channel

**Lot commitment**:
An immutable global-chain commitment identifying the committed state of a lot or batch. Certification and audit records reference it; they never modify or overwrite it.
_Avoid_: lot update, certification update

**Certification**:
A signed first-class process record asserting that a lot commitment satisfies a defined scope for a validity interval. Its evidence manifest and status are anchored publicly; raw evidence remains private/off-chain.
_Avoid_: certification flag, lot mutation

**Audit attestation**:
A signed first-class record of an audit or inspection process and its outcome/status for a referenced lot commitment. Corrections, renewal, suspension, or revocation are new signed records or status events, not edits to the original transaction.
_Avoid_: audit log entry

**Canonical record**:
A record from the network-wide schema vocabulary. In v1 it is strictly validated, signed, and append-only once anchored; partner-specific data may appear only under a registered namespace.
_Avoid_: arbitrary payload

**Schema registry**:
The immutable, versioned vocabulary of canonical record types and registered extension namespaces. Historical schema versions remain available, while capability activation controls which versions may be used for new blocks.
_Avoid_: mutable schema

**Capability**:
A named network rule or feature whose effect can change consensus-visible wire, admission, validation, schema, endorsement, replay, or consensus behavior. Capabilities are not negotiated ad hoc by peers; the active set is part of committed history.
_Avoid_: optional peer preference

**Capability activation**:
A signed, append-only network control-plane record that names an immutable capability version/hash and a future height at which it becomes active. It never changes the meaning of earlier blocks or takes effect midway through its own block.
_Avoid_: in-place protocol upgrade

**Consensus boundary**:
The data boundary of the globally ordered chain: approved public canonical records and commitments may enter it, while private commercial payloads, raw evidence, and high-frequency telemetry remain in PDC/off-chain paths.
_Avoid_: commitment-only ledger

**Wire protocol version**:
The version of the peer message encoding and handshake compatibility. It is separate from the committed ledger capability set; a compatible connection cannot activate ledger semantics by itself.
_Avoid_: consensus version

**Read-only observer**:
A peer that may inspect and validate compatible committed history but cannot propose, vote, relay active writes, or participate in consensus after it lacks an active capability.
_Avoid_: legacy validator
