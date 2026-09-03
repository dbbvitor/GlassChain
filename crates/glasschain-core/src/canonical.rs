//! Canonical schema v1 (ADR-006): the immutable network-wide record registry and
//! strict deterministic validation.
//!
//! This module replaces the compile-time `SNCM_SCHEMA` constant as the model for
//! *new* v1 records: 13 record families keyed by `(schema_id, schema_version,
//! schema_hash)`, a common record envelope, registered extension namespaces,
//! anchored commitments for lots, certification, audit, and state-commitment
//! records, and an explicit legacy-input boundary.
//!
//! Validation is pure data-driven logic — it never depends on partner extension
//! semantics, so every peer reaches the same accept/reject result. Signature
//! *verification* (as opposed to presence) is the endorsement layer's job
//! (ADR-008; tickets #37/#45), and capability-controlled schema activation is
//! ticket #36.

use crate::asset::is_valid_iso8601_date;
use crate::crypto::sha256;
use crate::error::CoreError;
use crate::TraceableAsset;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::LazyLock;
use uuid::Uuid;

/// The v1 schema version shared by every family in the registry.
pub const SCHEMA_VERSION_V1: u32 = 1;

/// A signature attached to a canonical record.
///
/// Core treats signatures as opaque presence data; cryptographic verification
/// happens in `glasschain-identity` (matching the existing `SignedTransaction`
/// path). See ADR-008 for the identity-neutral boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordSignature {
    /// Human-readable signer identity (MSP member / node identifier).
    pub signer: String,
    /// Raw signature bytes over the record's [`CanonicalRecord::canonical_form`].
    pub signature_bytes: Vec<u8>,
}

/// One registered namespace extension attached to a record.
///
/// The canonical serialized value participates in the record commitment, so
/// extensions are anchored exactly like core fields (ADR-006 decision 5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionValue {
    /// Version of the namespace descriptor this value validates against.
    pub schema_version: u32,
    /// The extension's field map (may carry only a commitment when the
    /// namespace is private).
    pub value: BTreeMap<String, Value>,
}

/// A canonical v1 record: the ADR-006 common envelope plus a family payload.
///
/// `record_id` and `schema_version` are part of the signed canonical form
/// ([`Self::canonical_form`]); records are append-only once anchored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CanonicalRecord {
    /// Globally unique record identifier.
    pub record_id: String,
    /// The record family (one of the 13 registry `schema_id`s).
    pub schema_id: String,
    /// Schema version of the family (must be [`SCHEMA_VERSION_V1`] in v1).
    pub schema_version: u32,
    /// The canonical record hash where an anchor is required: must equal
    /// [`Self::commitment`] for anchored families, and must be absent otherwise.
    pub commitment: Option<String>,
    /// Unix timestamp (seconds) of when the record was created.
    pub occurred_at: u64,
    /// Originating/issuing MSP identity of this record.
    pub issuer: String,
    /// The required signature set; every v1 record carries at least one.
    pub signatures: Vec<RecordSignature>,
    /// Optional channel/PDC reference (required for private extension values).
    pub pdc_ref: Option<String>,
    /// Registered namespaced extensions, keyed by namespace.
    pub extensions: BTreeMap<String, ExtensionValue>,
    /// Family-specific payload, validated against the family's descriptor.
    pub payload: BTreeMap<String, Value>,
}

impl CanonicalRecord {
    /// Create a new v1 record with a fresh `record_id`, no signatures, no
    /// extensions, and no anchor. Callers set the remaining fields before
    /// submission; anchored families must set [`Self::commitment`] to
    /// [`Self::commitment`]-computed value.
    #[must_use]
    pub fn new(
        occurred_at: u64,
        schema_id: impl Into<String>,
        payload: BTreeMap<String, Value>,
        issuer: impl Into<String>,
    ) -> Self {
        Self {
            record_id: Uuid::new_v4().to_string(),
            schema_id: schema_id.into(),
            schema_version: SCHEMA_VERSION_V1,
            commitment: None,
            occurred_at,
            issuer: issuer.into(),
            signatures: Vec::new(),
            pdc_ref: None,
            extensions: BTreeMap::new(),
            payload,
        }
    }

    /// The deterministic canonical form: envelope identity fields plus payload
    /// and extensions, serialized as a JSON tuple. This is what signatures
    /// cover and what the anchor commitment hashes.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Serialization`] if the record cannot be serialized;
    /// this cannot occur for records built from JSON values.
    pub fn canonical_form(&self) -> Result<String, CoreError> {
        Ok(serde_json::to_string(&(
            &self.record_id,
            &self.schema_id,
            self.schema_version,
            self.occurred_at,
            &self.issuer,
            &self.pdc_ref,
            &self.payload,
            &self.extensions,
        ))?)
    }

    /// Compute the canonical record commitment (ADR-006 decision 2): the
    /// SHA-256 of [`Self::canonical_form`].
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::Serialization`] if the canonical form cannot be
    /// serialized; this cannot occur for records built from JSON values.
    pub fn commitment(&self) -> Result<String, CoreError> {
        Ok(sha256(self.canonical_form()?.as_bytes()))
    }
}

/// Field type in a namespace descriptor — a JSON-Schema-compatible subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionFieldType {
    String,
    Integer,
    Number,
    Boolean,
    Object,
    Array,
}

/// Immutable descriptor for one registered extension namespace
/// (ADR-006 decision 5). The registry rejects unknown namespaces, and private
/// namespaces may only anchor a commitment behind a PDC reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamespaceDescriptor {
    /// Namespace identifier (e.g. `"anvisa"`).
    pub namespace: &'static str,
    /// Immutable descriptor version.
    pub version: u32,
    /// `true`: values must be a single `commitment` and the record must carry a
    /// `pdc_ref`; the payload itself stays out of global replication.
    pub private: bool,
    /// Fields that must be present in the extension value.
    pub required: &'static [&'static str],
    /// Field-name → type pairs for type checking.
    pub properties: &'static [(&'static str, ExtensionFieldType)],
}

/// One immutable schema version of a record family: the field catalog for
/// strict v1 validation (ADR-006 decisions 1 and 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaDescriptor {
    /// The record family identifier.
    pub schema_id: &'static str,
    /// Immutable schema version.
    pub version: u32,
    /// Payload keys that must be present and non-empty.
    pub required: &'static [&'static str],
    /// The only additional payload keys allowed; anything else is rejected
    /// (consensus-boundary whitelist, ADR-010 decision 1).
    pub optional: &'static [&'static str],
    /// `true`: the family must carry `commitment` equal to the record's
    /// canonical hash.
    pub anchored: bool,
    /// Closed status vocabulary for the `status` payload key, when applicable.
    pub status_values: Option<&'static [&'static str]>,
}

/// The 13 canonical v1 record families (ADR-006 decision 1).
///
/// `required` lists the payload keys that must be present and non-empty;
/// `optional` lists the only additional keys a payload may carry — anything
/// else is rejected so private data cannot be smuggled past the consensus
/// boundary (ADR-010 decision 1); `anchored` families must carry
/// `commitment == sha256(canonical_form)`; `status_values` is the closed status
/// vocabulary when the family has a `status` payload key.
pub const SCHEMA_V1: &[SchemaDescriptor] = &[
    SchemaDescriptor {
        schema_id: "party_identity",
        version: 1,
        required: &["org_id", "legal_name"],
        optional: &[],
        anchored: false,
        status_values: None,
    },
    SchemaDescriptor {
        schema_id: "product",
        version: 1,
        required: &["product_id", "gtin", "product_name"],
        optional: &[],
        anchored: false,
        status_values: None,
    },
    SchemaDescriptor {
        schema_id: "lot",
        version: 1,
        required: &["lot_id", "product_id", "batch_number"],
        optional: &["expiry_date"],
        anchored: true,
        status_values: None,
    },
    SchemaDescriptor {
        schema_id: "inventory_threshold",
        version: 1,
        required: &["trigger_id", "product_id", "owner_id", "reorder_threshold"],
        optional: &[],
        anchored: false,
        status_values: None,
    },
    SchemaDescriptor {
        schema_id: "purchase_order",
        version: 1,
        required: &[
            "product_id",
            "buyer_id",
            "seller_id",
            "quantity",
            "currency",
        ],
        optional: &[],
        anchored: false,
        status_values: None,
    },
    SchemaDescriptor {
        schema_id: "shipment",
        version: 1,
        required: &["lot_ref", "from_org", "to_org"],
        optional: &[],
        anchored: false,
        status_values: None,
    },
    SchemaDescriptor {
        schema_id: "transit_event",
        version: 1,
        required: &["shipment_ref", "event_type", "location"],
        optional: &[],
        anchored: false,
        status_values: None,
    },
    SchemaDescriptor {
        schema_id: "delivery_receipt",
        version: 1,
        required: &["shipment_ref", "receiver_id", "received_at"],
        optional: &[],
        anchored: false,
        status_values: None,
    },
    SchemaDescriptor {
        schema_id: "inventory_transformation",
        version: 1,
        required: &["lot_ref", "transformation_type"],
        optional: &[],
        anchored: false,
        status_values: None,
    },
    SchemaDescriptor {
        schema_id: "recall",
        version: 1,
        required: &["lot_ref", "reason", "status", "issued_by"],
        optional: &[],
        anchored: false,
        status_values: Some(&["issued", "active", "completed"]),
    },
    SchemaDescriptor {
        schema_id: "quality_certification",
        version: 1,
        required: &[
            "lot_ref",
            "issuer",
            "scope",
            "valid_from",
            "valid_to",
            "status",
            "evidence_manifest",
        ],
        optional: &[],
        anchored: true,
        status_values: Some(&["valid", "suspended", "revoked"]),
    },
    SchemaDescriptor {
        schema_id: "audit_attestation",
        version: 1,
        required: &[
            "lot_ref",
            "issuer",
            "scope",
            "valid_from",
            "valid_to",
            "status",
            "evidence_manifest",
        ],
        optional: &[],
        anchored: true,
        status_values: Some(&["valid", "suspended", "revoked"]),
    },
    SchemaDescriptor {
        schema_id: "state_commitment",
        version: 1,
        required: &["merkle_root", "counterparties"],
        optional: &["aggregation_ratio"],
        anchored: true,
        status_values: None,
    },
];

/// The v1 registered extension namespaces (ADR-006 decision 5).
///
/// `anvisa` is the open regulator namespace: partner/regulator-specific fields
/// validate against its descriptor, and the field catalog is filled by the
/// Stage-5 SEFAZ adapter.
pub const NAMESPACE_V1: &[NamespaceDescriptor] = &[NamespaceDescriptor {
    namespace: "anvisa",
    version: 1,
    private: false,
    required: &[],
    properties: &[],
}];

/// Core field names (envelope plus every v1 family payload key). Extension
/// namespaces may never override, shadow, or redefine these
/// (ADR-006 decision 5).
pub const CORE_FIELD_NAMES: &[&str] = &[
    "record_id",
    "schema_id",
    "schema_version",
    "commitment",
    "occurred_at",
    "issuer",
    "signatures",
    "pdc_ref",
    "extensions",
    "payload",
    "org_id",
    "legal_name",
    "product_id",
    "gtin",
    "product_name",
    "lot_id",
    "batch_number",
    "expiry_date",
    "trigger_id",
    "owner_id",
    "reorder_threshold",
    "buyer_id",
    "seller_id",
    "quantity",
    "currency",
    "lot_ref",
    "from_org",
    "to_org",
    "shipment_ref",
    "event_type",
    "location",
    "receiver_id",
    "received_at",
    "transformation_type",
    "reason",
    "status",
    "issued_by",
    "scope",
    "valid_from",
    "valid_to",
    "evidence_manifest",
    "manifest_commitment",
    "merkle_root",
    "counterparties",
    "aggregation_ratio",
];

/// Legacy `TraceableAsset`-shaped payload keys; three or more of these mark a
/// payload as a smuggled legacy asset rather than a v1 record.
const LEGACY_ASSET_KEYS: [&str; 6] = [
    "gtin",
    "batch_number",
    "expiry_date",
    "serial_number",
    "anvisa_registration",
    "manufacturer_id",
];

/// One immutable schema version in the registry: the descriptor plus its
/// derived `schema_hash` (ADR-006 decision 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaEntry {
    /// The immutable descriptor.
    pub descriptor: &'static SchemaDescriptor,
    /// SHA-256 of the descriptor's canonical form — the registry key's third
    /// component.
    pub schema_hash: &'static str,
}

/// An immutable, network-wide schema registry keyed by
/// `(schema_id, schema_version, schema_hash)`.
///
/// [`Registry::v1`] is the network's registry; validating members never mutate
/// it. Historical versions stay available for validating old blocks
/// (capability-controlled activation of *new* versions is ticket #36).
#[derive(Debug, Clone)]
pub struct Registry {
    schemas: BTreeMap<(&'static str, u32), SchemaEntry>,
    namespaces: BTreeMap<&'static str, NamespaceDescriptor>,
}

fn descriptor_hash(descriptor: &SchemaDescriptor) -> String {
    // Deterministic, infallible canonical text: no serializer, so the registry
    // hash can never panic (AGENTS.md allows no expect in library code).
    let canonical = format!(
        "{}|v{}|required={}|optional={}|anchored={}|status={:?}",
        descriptor.schema_id,
        descriptor.version,
        descriptor.required.join(","),
        descriptor.optional.join(","),
        descriptor.anchored,
        descriptor.status_values,
    );
    sha256(canonical.as_bytes())
}

static SCHEMA_HASHES: LazyLock<BTreeMap<(&'static str, u32), String>> = LazyLock::new(|| {
    SCHEMA_V1
        .iter()
        .map(|d| ((d.schema_id, d.version), descriptor_hash(d)))
        .collect()
});

impl Registry {
    /// The network-wide v1 registry: 13 families and the registered namespaces,
    /// immutable and shared by every node.
    #[must_use]
    pub fn v1() -> &'static Self {
        static V1: LazyLock<Registry> = LazyLock::new(Registry::from_static);
        &V1
    }

    /// Build a registry seeded with the static v1 tables plus one additional
    /// namespace. Used by tests and by future capability-activation code that
    /// constructs per-height registries.
    #[must_use]
    pub fn with_namespace(mut self, namespace: NamespaceDescriptor) -> Self {
        self.namespaces.insert(namespace.namespace, namespace);
        self
    }

    /// Build a registry seeded with the static v1 tables plus one additional
    /// schema version. This is how historical versions become *available for
    /// validation* (ADR-006 decision 6); capability activation decides which
    /// version is *accepted for new blocks* (ticket #36).
    #[must_use]
    pub fn with_schema(mut self, descriptor: &'static SchemaDescriptor) -> Self {
        let entry = SchemaEntry {
            descriptor,
            schema_hash: descriptor_hash(descriptor).leak(),
        };
        self.schemas
            .insert((descriptor.schema_id, descriptor.version), entry);
        self
    }

    fn from_static() -> Self {
        let schemas = SCHEMA_V1
            .iter()
            .map(|d| {
                let entry = SchemaEntry {
                    descriptor: d,
                    schema_hash: SCHEMA_HASHES[&(d.schema_id, d.version)].as_str(),
                };
                ((d.schema_id, d.version), entry)
            })
            .collect();
        let namespaces = NAMESPACE_V1.iter().map(|n| (n.namespace, *n)).collect();
        Self {
            schemas,
            namespaces,
        }
    }

    /// Look up a schema version by `(schema_id, schema_version)`. The returned
    /// entry's `schema_hash` is the immutable third registry-key component.
    #[must_use]
    pub fn lookup_schema(&self, schema_id: &str, version: u32) -> Option<SchemaEntry> {
        self.schemas
            .iter()
            .find(|((id, ver), _)| *id == schema_id && *ver == version)
            .map(|(_, entry)| *entry)
    }

    /// Look up a registered extension namespace by name.
    #[must_use]
    pub fn lookup_namespace(&self, namespace: &str) -> Option<NamespaceDescriptor> {
        self.namespaces.get(namespace).copied()
    }
}

fn err(schema_id: &str, message: impl Into<String>) -> CoreError {
    CoreError::InvalidTransaction(format!("canonical record {schema_id}: {}", message.into()))
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

const fn is_present(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

fn matches_type(value: &Value, ty: ExtensionFieldType) -> bool {
    match ty {
        ExtensionFieldType::String => value.is_string(),
        ExtensionFieldType::Integer => value.is_i64() || value.is_u64(),
        ExtensionFieldType::Number => value.is_number(),
        ExtensionFieldType::Boolean => value.is_boolean(),
        ExtensionFieldType::Object => value.is_object(),
        ExtensionFieldType::Array => value.is_array(),
    }
}

/// True when the payload carries three or more legacy `TraceableAsset` keys
/// and is therefore a smuggled legacy asset rather than a v1 record.
fn looks_like_legacy_asset(payload: &BTreeMap<String, Value>) -> bool {
    LEGACY_ASSET_KEYS
        .iter()
        .filter(|key| payload.contains_key(**key))
        .count()
        >= 3
}

/// Validate a canonical record against the network-wide v1 registry.
///
/// # Errors
///
/// Returns [`CoreError::InvalidTransaction`] for the first rule violation:
/// envelope problems, unknown schema/version, missing or invalid required
/// fields, commitment mismatches, unregistered or shadowing extensions,
/// private-extension leakage, legacy-asset-shaped payloads, or unknown
/// namespaces.
pub fn validate_record(record: &CanonicalRecord) -> Result<(), CoreError> {
    validate_record_with(Registry::v1(), record)
}

/// Validate a canonical record against an explicit registry (used by the
/// default path and by capability-activation code that selects per-height
/// registries).
///
/// # Errors
///
/// Same rules as [`validate_record`].
#[allow(clippy::too_many_lines)] // one linear validator; split when a caller needs a piece
pub fn validate_record_with(
    registry: &Registry,
    record: &CanonicalRecord,
) -> Result<(), CoreError> {
    // ── Common envelope ──────────────────────────────────────────────────────
    if record.record_id.is_empty() {
        return Err(err(
            record.schema_id.as_str(),
            "record_id must not be empty",
        ));
    }
    if record.issuer.is_empty() {
        return Err(err(record.schema_id.as_str(), "issuer must not be empty"));
    }
    if record.signatures.is_empty() {
        return Err(err(
            record.schema_id.as_str(),
            "at least one signature is required",
        ));
    }
    if record.schema_version == 0 {
        return Err(err(
            record.schema_id.as_str(),
            "schema version 0 is not a valid version",
        ));
    }

    // ── Family lookup and required fields ────────────────────────────────────
    // The registry is the version gate: the network-wide v1 registry accepts
    // only v1, while registries extended with historical versions (ADR-006
    // decision 6) validate records under the version effective at their height.
    let entry = registry
        .lookup_schema(&record.schema_id, record.schema_version)
        .ok_or_else(|| {
            let known = registry
                .schemas
                .keys()
                .any(|(id, _)| *id == record.schema_id.as_str());
            if known {
                err(
                    record.schema_id.as_str(),
                    format!("unsupported schema version {}", record.schema_version),
                )
            } else {
                err(
                    record.schema_id.as_str(),
                    format!(
                        "unknown schema {}/v{}",
                        record.schema_id, record.schema_version
                    ),
                )
            }
        })?;
    let descriptor = entry.descriptor;

    if looks_like_legacy_asset(&record.payload) {
        return Err(err(
            descriptor.schema_id,
            "payload is TraceableAsset-shaped: legacy assets are not valid v1 records; \
             migrate with `migrate_legacy_asset` and re-register as product + lot records",
        ));
    }

    for key in descriptor.required {
        if !is_present(record.payload.get(*key)) {
            return Err(err(
                descriptor.schema_id,
                format!("missing required field '{key}'"),
            ));
        }
    }

    // Consensus-boundary whitelist: a payload key outside the family's
    // required/optional catalog rejects the record, so private quantities,
    // pricing, raw evidence, and telemetry cannot ride along public records
    // (ADR-006 decision 4, ADR-010 decision 1). Partner-specific fields belong
    // in registered extension namespaces.
    let allowed: Vec<&str> = descriptor
        .required
        .iter()
        .chain(descriptor.optional.iter())
        .copied()
        .collect();
    if let Some(unknown) = record
        .payload
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        return Err(err(
            descriptor.schema_id,
            format!("unknown payload field '{unknown}' (use a registered extension namespace)"),
        ));
    }

    // ── Family-specific strictness ───────────────────────────────────────────
    match descriptor.schema_id {
        "product" => {
            let gtin = record
                .payload
                .get("gtin")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if gtin.len() != 13 && gtin.len() != 14 || !gtin.bytes().all(|b| b.is_ascii_digit()) {
                return Err(err("product", "gtin must be 13 or 14 numeric digits"));
            }
        }
        "delivery_receipt" => {
            let received_at = record
                .payload
                .get("received_at")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !is_valid_iso8601_date(received_at) {
                return Err(err(
                    "delivery_receipt",
                    "received_at must be an ISO-8601 date (YYYY-MM-DD)",
                ));
            }
        }
        "quality_certification" | "audit_attestation" => {
            let valid_from = record
                .payload
                .get("valid_from")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let valid_to = record
                .payload
                .get("valid_to")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !is_valid_iso8601_date(valid_from) || !is_valid_iso8601_date(valid_to) {
                return Err(err(
                    descriptor.schema_id,
                    "valid_from and valid_to must be ISO-8601 dates (YYYY-MM-DD)",
                ));
            }
            if valid_to < valid_from {
                return Err(err(
                    descriptor.schema_id,
                    "valid_to must not precede valid_from",
                ));
            }
            // EvidenceManifest is embedded, not a standalone entity (ADR-005).
            let manifest = record.payload.get("evidence_manifest");
            let commitment = manifest
                .and_then(Value::as_object)
                .and_then(|m| m.get("manifest_commitment"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !is_hex64(commitment) {
                return Err(err(
                    descriptor.schema_id,
                    "evidence_manifest.manifest_commitment must be a 64-hex commitment",
                ));
            }
        }
        "state_commitment" => {
            let merkle_root = record
                .payload
                .get("merkle_root")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !is_hex64(merkle_root) {
                return Err(err(
                    "state_commitment",
                    "merkle_root must be a 64-hex commitment",
                ));
            }
            let counterparties = record.payload.get("counterparties");
            let Some(counterparties) = counterparties.and_then(Value::as_array) else {
                return Err(err("state_commitment", "counterparties must be an array"));
            };
            if counterparties.is_empty()
                || !counterparties
                    .iter()
                    .all(|c| c.as_str().is_some_and(|s| !s.is_empty()))
            {
                return Err(err(
                    "state_commitment",
                    "counterparties must name at least one non-empty organization",
                ));
            }
            // Counterparty MSP signatures: the issuer's opaque signature set
            // must carry at least one entry per named counterparty. This is a
            // structural schema check only — the bytes are advisory metadata.
            // Authorization is the endorsement layer's job (ADR-008/ADR-012):
            // when the `endorsement` capability is active, the operation
            // default requires the issuer and every named counterparty as
            // verified endorsement carriers.
            if record.signatures.len() < counterparties.len() {
                return Err(err(
                    "state_commitment",
                    "signature set must carry at least one signature per counterparty",
                ));
            }
            // Aggregation ratio is intentionally left configurable, not assumed
            // (ADR-004 open question 1); when present it must be sane.
            if let Some(Value::Number(ratio)) = record.payload.get("aggregation_ratio") {
                if ratio.as_u64().unwrap_or_default() < 1 {
                    return Err(err(
                        "state_commitment",
                        "aggregation_ratio must be at least 1 when present",
                    ));
                }
            }
        }
        _ => {}
    }

    if let Some(values) = descriptor.status_values {
        let status = record
            .payload
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !values.contains(&status) {
            return Err(err(
                descriptor.schema_id,
                format!("status '{status}' is not in the v1 vocabulary {values:?}"),
            ));
        }
    }

    // ── Anchor commitment (ADR-005/006) ──────────────────────────────────────
    if descriptor.anchored {
        let Some(commitment) = record.commitment.as_deref() else {
            return Err(err(
                descriptor.schema_id,
                "anchored family requires a commitment",
            ));
        };
        let expected = record.commitment()?;
        if commitment != expected {
            return Err(err(
                descriptor.schema_id,
                "commitment does not match the canonical record form",
            ));
        }
    } else if record.commitment.is_some() {
        return Err(err(
            descriptor.schema_id,
            "commitment is only allowed on anchored families",
        ));
    }

    // ── Registered extension namespaces ──────────────────────────────────────
    for (namespace, extension) in &record.extensions {
        let Some(descriptor) = registry.lookup_namespace(namespace) else {
            return Err(err(
                record.schema_id.as_str(),
                format!("unknown extension namespace '{namespace}'"),
            ));
        };
        if extension.schema_version != descriptor.version {
            return Err(err(
                record.schema_id.as_str(),
                format!(
                    "extension namespace '{namespace}' uses unsupported version {}",
                    extension.schema_version
                ),
            ));
        }
        for key in extension.value.keys() {
            // `commitment` is the sanctioned private-anchor shape (ADR-006
            // decision 5); every other key may not touch the core vocabulary.
            if key != "commitment" && CORE_FIELD_NAMES.contains(&key.as_str()) {
                return Err(err(
                    record.schema_id.as_str(),
                    format!("extension namespace '{namespace}' shadows core field '{key}'"),
                ));
            }
        }
        for key in descriptor.required {
            if !is_present(extension.value.get(*key)) {
                return Err(err(
                    record.schema_id.as_str(),
                    format!("extension namespace '{namespace}' missing required field '{key}'"),
                ));
            }
        }
        for (name, ty) in descriptor.properties {
            if let Some(value) = extension.value.get(*name) {
                if !matches_type(value, *ty) {
                    return Err(err(
                        record.schema_id.as_str(),
                        format!(
                            "extension namespace '{namespace}' field '{name}' has the wrong type"
                        ),
                    ));
                }
            }
        }
        if descriptor.private {
            if record.pdc_ref.as_deref().is_none_or(str::is_empty) {
                return Err(err(
                    record.schema_id.as_str(),
                    format!(
                        "private extension namespace '{namespace}' requires a pdc_ref on the record"
                    ),
                ));
            }
            if extension.value.len() != 1 {
                return Err(err(
                    record.schema_id.as_str(),
                    format!(
                        "private extension namespace '{namespace}' must carry exactly a 'commitment'"
                    ),
                ));
            }
            let commitment = extension
                .value
                .get("commitment")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !is_hex64(commitment) {
                return Err(err(
                    record.schema_id.as_str(),
                    format!(
                        "private extension namespace '{namespace}' commitment must be 64 hex characters"
                    ),
                ));
            }
        }
    }

    Ok(())
}

/// Explicit legacy compatibility path (ADR-006 decision 7): convert a
/// [`TraceableAsset`] into the `product` and `lot` v1 records it represents,
/// with the lot record carrying its anchor commitment.
///
/// The caller attaches signatures before submission.
///
/// # Errors
///
/// Returns [`CoreError::Serialization`] if the lot record's canonical form
/// cannot be serialized; this cannot occur for assets built from JSON values.
pub fn migrate_legacy_asset(
    asset: &TraceableAsset,
    issuer: &str,
    occurred_at: u64,
) -> Result<(CanonicalRecord, CanonicalRecord), CoreError> {
    let gtin = asset.gtin.clone().unwrap_or_default();
    let batch = asset.batch_number.clone().unwrap_or_default();
    let product = CanonicalRecord::new(
        occurred_at,
        "product",
        BTreeMap::from([
            ("product_id".to_owned(), Value::String(gtin.clone())),
            ("gtin".to_owned(), Value::String(gtin.clone())),
            (
                "product_name".to_owned(),
                Value::String(asset.product_name.clone()),
            ),
        ]),
        issuer,
    );
    let mut lot_payload = BTreeMap::from([
        (
            "lot_id".to_owned(),
            Value::String(format!("{gtin}-{batch}")),
        ),
        ("product_id".to_owned(), Value::String(gtin)),
        ("batch_number".to_owned(), Value::String(batch)),
    ]);
    if let Some(expiry) = &asset.expiry_date {
        lot_payload.insert("expiry_date".to_owned(), Value::String(expiry.clone()));
    }
    let mut lot = CanonicalRecord::new(occurred_at, "lot", lot_payload, issuer);
    lot.commitment = Some(lot.commitment()?);
    Ok((product, lot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HEX64: &str = "abababababababababababababababababababababababababababababababab";

    fn payload(schema_id: &str) -> Value {
        match schema_id {
            "party_identity" => json!({"org_id": "cooperative-x", "legal_name": "Cooperative X"}),
            "product" => {
                json!({"product_id": "SKU-1", "gtin": "07891234100016", "product_name": "Drug A"})
            }
            "lot" => json!({"lot_id": "lot-1", "product_id": "SKU-1", "batch_number": "BATCH-001"}),
            "inventory_threshold" => {
                json!({"trigger_id": "trig-1", "product_id": "SKU-1", "owner_id": "buyer-1", "reorder_threshold": 100})
            }
            "purchase_order" => {
                json!({"product_id": "SKU-1", "buyer_id": "buyer-1", "seller_id": "seller-1", "quantity": 50, "currency": "USD"})
            }
            "shipment" => json!({"lot_ref": "lot-hash", "from_org": "maker-1", "to_org": "dist-1"}),
            "transit_event" => {
                json!({"shipment_ref": "ship-1", "event_type": "departure", "location": "BR-SP"})
            }
            "delivery_receipt" => {
                json!({"shipment_ref": "ship-1", "receiver_id": "pharmacy-1", "received_at": "2026-01-15"})
            }
            "inventory_transformation" => {
                json!({"lot_ref": "lot-hash", "transformation_type": "split"})
            }
            "recall" => {
                json!({"lot_ref": "lot-hash", "reason": "contamination", "status": "issued", "issued_by": "maker-1"})
            }
            "quality_certification" => {
                json!({"lot_ref": "lot-hash", "issuer": "certifier-1", "scope": "GMP",
                "valid_from": "2026-01-01", "valid_to": "2027-01-01", "status": "valid",
                "evidence_manifest": {"manifest_commitment": HEX64}})
            }
            "audit_attestation" => {
                json!({"lot_ref": "lot-hash", "issuer": "auditor-1", "scope": "annual inspection",
                "valid_from": "2026-01-01", "valid_to": "2026-12-31", "status": "valid",
                "evidence_manifest": {"manifest_commitment": HEX64}})
            }
            "state_commitment" => {
                json!({"merkle_root": HEX64, "counterparties": ["org-a", "org-b"]})
            }
            other => panic!("unknown family {other}"),
        }
    }

    fn sign(record: &mut CanonicalRecord) {
        record.signatures.push(RecordSignature {
            signer: record.issuer.clone(),
            signature_bytes: vec![0x42; 8],
        });
    }

    fn anchor(record: &mut CanonicalRecord) {
        let registry = Registry::v1();
        let anchored = registry
            .lookup_schema(&record.schema_id, record.schema_version)
            .is_some_and(|entry| entry.descriptor.anchored);
        if anchored {
            record.commitment = record.commitment().ok();
        }
    }

    /// A validation-passing record for `schema_id`, signed and (where required)
    /// anchored.
    fn valid_record(schema_id: &str) -> CanonicalRecord {
        let json_payload = payload(schema_id);
        let payload: BTreeMap<String, Value> = serde_json::from_value(json_payload).unwrap();
        let mut record = CanonicalRecord::new(0, schema_id, payload, "org-issuer");
        sign(&mut record);
        anchor(&mut record);
        if schema_id == "state_commitment" {
            // v1 requires one opaque signature per named counterparty.
            let counterparties = record
                .payload
                .get("counterparties")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            while record.signatures.len() < counterparties {
                sign(&mut record);
            }
            anchor(&mut record);
        }
        match validate_record(&record) {
            Ok(()) => record,
            Err(e) => panic!("fixture {schema_id} must validate: {e}"),
        }
    }

    #[test]
    fn test_registry_defines_thirteen_families() {
        assert_eq!(SCHEMA_V1.len(), 13, "v1 must define exactly 13 families");
        let mut ids: Vec<&str> = SCHEMA_V1.iter().map(|d| d.schema_id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SCHEMA_V1.len(), "schema_ids must be unique");
        for descriptor in SCHEMA_V1 {
            assert!(
                !descriptor.required.is_empty(),
                "{} must have a catalog",
                descriptor.schema_id
            );
        }
    }

    #[test]
    fn test_all_thirteen_families_validate() {
        for descriptor in SCHEMA_V1 {
            valid_record(descriptor.schema_id);
        }
    }

    #[test]
    fn test_registry_lookup_returns_stable_hash() {
        let registry = Registry::v1();
        let first = registry.lookup_schema("lot", 1).expect("lot v1 registered");
        let second = registry.lookup_schema("lot", 1).expect("lot v1 registered");
        assert_eq!(first.schema_hash, second.schema_hash);
        assert_eq!(first.schema_hash.len(), 64);
    }

    #[test]
    fn test_missing_required_field_rejected_for_every_family() {
        for descriptor in SCHEMA_V1 {
            let mut record = valid_record(descriptor.schema_id);
            let missing = descriptor.required[0];
            record.payload.remove(missing);
            let error = validate_record(&record).expect_err("must be rejected");
            assert!(
                error.to_string().contains(missing),
                "error for {} must name '{missing}': {error}",
                descriptor.schema_id
            );
        }
    }

    #[test]
    fn test_unknown_schema_id_rejected() {
        let mut record = valid_record("lot");
        record.schema_id = "warehouse_manifest".into();
        let error = validate_record(&record).expect_err("must be rejected");
        assert!(error.to_string().contains("unknown schema"), "{error}");
    }

    #[test]
    fn test_unknown_schema_version_rejected() {
        let mut record = valid_record("lot");
        record.schema_version = 2;
        let error = validate_record(&record).expect_err("must be rejected");
        assert!(error.to_string().contains("version"), "{error}");
    }

    #[test]
    fn test_envelope_rules() {
        let mut record = valid_record("party_identity");
        record.signatures.clear();
        assert!(
            validate_record(&record).is_err(),
            "unsigned record must fail"
        );

        let mut record = valid_record("party_identity");
        record.issuer.clear();
        assert!(validate_record(&record).is_err(), "empty issuer must fail");

        let mut record = valid_record("party_identity");
        record.record_id.clear();
        assert!(
            validate_record(&record).is_err(),
            "empty record_id must fail"
        );
    }

    #[test]
    fn test_anchor_commitment_enforced() {
        let mut record = valid_record("lot");
        record.commitment = Some("deadbeef".into());
        let error = validate_record(&record).expect_err("mismatched commitment must fail");
        assert!(error.to_string().contains("commitment"), "{error}");

        let mut record = valid_record("lot");
        record.commitment = None;
        assert!(
            validate_record(&record).is_err(),
            "missing commitment must fail"
        );

        let mut non_anchored = valid_record("party_identity");
        non_anchored.commitment = Some(HEX64.into());
        assert!(
            validate_record(&non_anchored).is_err(),
            "commitment on a non-anchored family must fail"
        );
    }

    #[test]
    fn test_status_vocabulary_enforced() {
        let mut record = valid_record("recall");
        record.payload.insert("status".into(), json!("banana"));
        let error = validate_record(&record).expect_err("unknown status must fail");
        assert!(error.to_string().contains("status"), "{error}");

        let mut record = valid_record("quality_certification");
        record.payload.insert("status".into(), json!("issued"));
        assert!(
            validate_record(&record).is_err(),
            "cert statuses are closed"
        );
    }

    #[test]
    fn test_certification_validity_and_evidence_rules() {
        let mut record = valid_record("quality_certification");
        record
            .payload
            .insert("valid_from".into(), json!("2027-01-01"));
        record
            .payload
            .insert("valid_to".into(), json!("2026-01-01"));
        assert!(
            validate_record(&record).is_err(),
            "valid_to before valid_from must fail"
        );

        let mut record = valid_record("quality_certification");
        record
            .payload
            .insert("valid_from".into(), json!("01/01/2026"));
        assert!(validate_record(&record).is_err(), "non-ISO dates must fail");

        let mut record = valid_record("quality_certification");
        record.payload.insert(
            "evidence_manifest".into(),
            json!({"manifest_commitment": "not-a-hash"}),
        );
        assert!(validate_record(&record).is_err(), "bad manifest must fail");

        let mut record = valid_record("quality_certification");
        record.payload.insert(
            "evidence_manifest".into(),
            json!({"uri": "https://evidence.example/lot-1"}),
        );
        assert!(
            validate_record(&record).is_err(),
            "manifest without commitment must fail"
        );
    }

    #[test]
    fn test_state_commitment_rules() {
        let mut record = valid_record("state_commitment");
        record.payload.insert("merkle_root".into(), json!("xyz"));
        assert!(
            validate_record(&record).is_err(),
            "bad merkle root must fail"
        );

        let mut record = valid_record("state_commitment");
        record.payload.insert("counterparties".into(), json!([]));
        assert!(
            validate_record(&record).is_err(),
            "empty counterparties must fail"
        );

        let mut record = valid_record("state_commitment");
        record.payload.insert("aggregation_ratio".into(), json!(0));
        assert!(validate_record(&record).is_err(), "zero ratio must fail");

        // Left configurable, not assumed: a sane ratio is accepted.
        let mut record = valid_record("state_commitment");
        record.payload.insert("aggregation_ratio".into(), json!(17));
        anchor(&mut record);
        assert!(validate_record(&record).is_ok());
    }

    #[test]
    fn test_product_gtin_format() {
        let mut record = valid_record("product");
        record.payload.insert("gtin".into(), json!("123"));
        assert!(validate_record(&record).is_err(), "short gtin must fail");

        let mut record = valid_record("product");
        record
            .payload
            .insert("gtin".into(), json!("0789123410001X"));
        assert!(
            validate_record(&record).is_err(),
            "non-numeric gtin must fail"
        );

        let mut record = valid_record("product");
        record
            .payload
            .insert("gtin".into(), json!("07891234100016"));
        assert!(validate_record(&record).is_ok());
    }

    #[test]
    fn test_unknown_namespace_rejected() {
        let mut record = valid_record("party_identity");
        record.extensions.insert(
            "urn:partner:unknown".into(),
            ExtensionValue {
                schema_version: 1,
                value: BTreeMap::from([("partner_key".into(), json!("value"))]),
            },
        );
        let error = validate_record(&record).expect_err("unknown namespace must fail");
        assert!(error.to_string().contains("unknown extension"), "{error}");
    }

    #[test]
    fn test_extension_cannot_shadow_core_fields() {
        let mut record = valid_record("party_identity");
        record.extensions.insert(
            "anvisa".into(),
            ExtensionValue {
                schema_version: 1,
                value: BTreeMap::from([("legal_name".into(), json!("shadowed"))]),
            },
        );
        let error = validate_record(&record).expect_err("shadowing must fail");
        assert!(error.to_string().contains("shadows core field"), "{error}");
    }

    #[test]
    fn test_unknown_payload_fields_rejected_any_family() {
        // Raw pricing smuggled onto a public shipment record.
        let mut record = valid_record("shipment");
        record.payload.insert("unit_price".into(), json!(2990));
        let error = validate_record(&record).expect_err("raw price must be rejected");
        assert!(
            error.to_string().contains("unknown payload field"),
            "{error}"
        );

        // Raw high-frequency telemetry on a state commitment.
        let mut record = valid_record("state_commitment");
        record
            .payload
            .insert("raw_telemetry".into(), json!([1, 2, 3]));
        assert!(validate_record(&record).is_err());
    }

    #[test]
    fn test_registered_extensions_validate_descriptor() {
        let registry = Registry::from_static().with_namespace(NamespaceDescriptor {
            namespace: "test.partner",
            version: 1,
            private: false,
            required: &["sku_code"],
            properties: &[("sku_code", ExtensionFieldType::String)],
        });
        let mut record = valid_record("party_identity");
        record.extensions.insert(
            "test.partner".into(),
            ExtensionValue {
                schema_version: 1,
                value: BTreeMap::from([("sku_code".into(), json!("P-42"))]),
            },
        );
        assert!(validate_record_with(&registry, &record).is_ok());

        // Missing required extension field.
        let mut broken = record.clone();
        broken
            .extensions
            .get_mut("test.partner")
            .expect("inserted")
            .value
            .remove("sku_code");
        assert!(validate_record_with(&registry, &broken).is_err());

        // Wrong field type.
        let mut typed = record.clone();
        typed
            .extensions
            .get_mut("test.partner")
            .expect("inserted")
            .value
            .insert("sku_code".into(), json!(42));
        assert!(validate_record_with(&registry, &typed).is_err());

        // The network v1 registry rejects unregistered namespaces.
        assert!(validate_record(&record).is_err());
    }

    #[test]
    fn test_private_extension_requires_pdc_ref_and_commitment() {
        let registry = Registry::from_static().with_namespace(NamespaceDescriptor {
            namespace: "test.pricing",
            version: 1,
            private: true,
            required: &[],
            properties: &[],
        });

        // Raw private value without a PDC reference leaks into the record.
        let mut record = valid_record("purchase_order");
        record.extensions.insert(
            "test.pricing".into(),
            ExtensionValue {
                schema_version: 1,
                value: BTreeMap::from([("unit_price".into(), json!(2990))]),
            },
        );
        assert!(
            validate_record_with(&registry, &record).is_err(),
            "private raw value must be rejected at the boundary"
        );

        // Commitment-only behind a pdc_ref is the accepted shape.
        let mut record = valid_record("purchase_order");
        record.pdc_ref = Some("pdc:pricing".into());
        record.extensions.insert(
            "test.pricing".into(),
            ExtensionValue {
                schema_version: 1,
                value: BTreeMap::from([("commitment".into(), json!(HEX64))]),
            },
        );
        assert!(validate_record_with(&registry, &record).is_ok());
    }

    #[test]
    fn test_legacy_asset_shaped_payload_rejected() {
        let mut record = valid_record("inventory_threshold");
        record
            .payload
            .insert("gtin".into(), json!("07891234100016"));
        record
            .payload
            .insert("batch_number".into(), json!("BATCH-001"));
        record
            .payload
            .insert("expiry_date".into(), json!("2027-12-31"));
        let error = validate_record(&record).expect_err("legacy payload must fail");
        assert!(error.to_string().contains("legacy"), "{error}");
    }

    #[test]
    fn test_migrate_legacy_asset_produces_valid_v1_records() {
        let asset = TraceableAsset {
            gtin: Some("07891234100016".into()),
            batch_number: Some("BATCH-001".into()),
            expiry_date: Some("2027-12-31".into()),
            serial_number: Some("SN-001".into()),
            anvisa_registration: None,
            manufacturer_id: None,
            product_name: "Drug A".into(),
            custodian_id: "maker-1".into(),
            country_of_origin: Some("BR".into()),
            storage_temp_celsius: None,
            quantity: 100,
        };
        let (mut product, mut lot) =
            migrate_legacy_asset(&asset, "org-issuer", 0).expect("migration builds records");
        sign(&mut product);
        sign(&mut lot);
        assert!(validate_record(&product).is_ok(), "product must validate");
        assert!(validate_record(&lot).is_ok(), "lot must validate");
        assert_eq!(
            lot.commitment.as_deref(),
            Some(lot.commitment().expect("recompute").as_str()),
            "lot carries its anchor commitment"
        );
        assert_eq!(
            lot.payload.get("lot_id").and_then(Value::as_str),
            Some("07891234100016-BATCH-001")
        );
    }

    #[test]
    fn test_canonical_form_is_deterministic_and_versioned() {
        let a = valid_record("party_identity");
        let mut b = a.clone();
        assert_eq!(
            a.canonical_form().expect("form"),
            b.canonical_form().expect("form")
        );

        b.schema_version = 2;
        assert_ne!(
            a.canonical_form().expect("form"),
            b.canonical_form().expect("form"),
            "schema version must be part of the signed canonical form"
        );

        let first = a.commitment().expect("commitment");
        let second = a.commitment().expect("commitment");
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn test_state_commitment_requires_signature_per_counterparty() {
        let mut insufficient = valid_record("state_commitment");
        while insufficient.signatures.len() > 1 {
            insufficient.signatures.pop();
        }
        let error = validate_record(&insufficient).expect_err("fewer sigs than counterparties");
        assert!(error.to_string().contains("signature"), "{error}");

        let sufficient = valid_record("state_commitment");
        assert!(validate_record(&sufficient).is_ok());
    }

    #[test]
    fn test_historical_schema_versions_remain_available() {
        // A v2 descriptor is registered the way a future schema release would
        // be; historical records validate against it while the network v1
        // registry still rejects the unknown version.
        static PARTY_V2: SchemaDescriptor = SchemaDescriptor {
            schema_id: "party_identity",
            version: 2,
            required: &["org_id"],
            optional: &[],
            anchored: false,
            status_values: None,
        };
        let registry = Registry::from_static().with_schema(&PARTY_V2);

        let payload: BTreeMap<String, Value> =
            serde_json::from_value(json!({ "org_id": "cooperative-x" })).unwrap();
        let mut historical = CanonicalRecord::new(0, "party_identity", payload, "org-issuer");
        historical.schema_version = 2;
        sign(&mut historical);

        assert!(validate_record_with(&registry, &historical).is_ok());
        let error = validate_record(&historical).expect_err("v1 registry rejects v2");
        assert!(error.to_string().contains("version"), "{error}");
    }

    #[test]
    fn test_pdc_ref_is_part_of_the_canonical_form() {
        let mut a = valid_record("purchase_order");
        a.pdc_ref = Some("pdc:pricing-a".into());
        let mut b = a.clone();
        b.pdc_ref = Some("pdc:pricing-b".into());
        assert_ne!(
            a.canonical_form().expect("form"),
            b.canonical_form().expect("form"),
            "swapping the PDC reference must invalidate the anchor"
        );
    }

    #[test]
    fn test_anchor_commitment_covers_extensions() {
        let mut record = valid_record("lot");
        record.extensions.insert(
            "anvisa".into(),
            ExtensionValue {
                schema_version: 1,
                value: BTreeMap::from([("reg_key".into(), json!("reg-value"))]),
            },
        );
        // Anchored families must recompute the commitment over the extensions.
        record.commitment = None;
        assert!(
            validate_record(&record).is_err(),
            "stale commitment must fail"
        );
        anchor(&mut record);
        assert!(validate_record(&record).is_ok());
    }
}
