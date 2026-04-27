//! SNCM Asset Schema Validation.
//!
//! This module implements **Phase 3** of the GlassChain plan: a deterministic
//! schema-compliance checker for [`TraceableAsset`] records against the
//! Brazilian Anvisa **SNCM** requirements (RDC 157/2017).
//!
//! ## Design
//! Validation runs entirely in pure Rust without external regex crates.
//! The [`SNCM_SCHEMA`] constant describes each expected field; calling
//! [`validate_asset`] returns a [`SchemaValidationReport`] that downstream
//! tooling (indexers, AI agents, gas metering) can act upon.
//!
//! ## Gas Incentive
//! Compliant assets receive a **30 % gas discount** (`gas_fee_multiplier = 0.7`).
//! Non-compliant assets pay full price (`gas_fee_multiplier = 1.0`).

use crate::asset::is_valid_iso8601_date;
use crate::TraceableAsset;
use serde::{Deserialize, Serialize};

// ── Schema descriptor ────────────────────────────────────────────────────────

/// Describes a single field requirement in the SNCM schema.
///
/// `regex_hint` is documentation-only and is **not** enforced at runtime;
/// external validators (indexers, linters) may use it for additional checks.
///
/// The struct is [`Copy`] so that it can be used freely in `const` contexts
/// and array initializers without lifetime or ownership complexity.
#[derive(Debug, Clone, Copy)]
pub struct SncmField {
    /// Field name, matching the corresponding [`TraceableAsset`] field.
    pub name: &'static str,
    /// Human-readable description for tooling and documentation.
    pub description: &'static str,
    /// Whether absence of this field makes the asset non-compliant.
    pub is_mandatory: bool,
    /// Optional regex pattern hint for external validators (informational only).
    pub regex_hint: Option<&'static str>,
}

/// The canonical SNCM schema: **4 mandatory + 2 recommended** fields.
///
/// This constant drives all compliance checks performed by [`validate_asset`].
/// Indices map directly to the six SNCM / GS1 EPCIS fields required for
/// Anvisa traceability under RDC 157/2017.
///
/// | Index | Field                  | Mandatory |
/// |------:|:-----------------------|:---------:|
/// |     0 | `gtin`                 | ✓         |
/// |     1 | `batch_number`         | ✓         |
/// |     2 | `expiry_date`          | ✓         |
/// |     3 | `serial_number`        | ✓         |
/// |     4 | `anvisa_registration`  |           |
/// |     5 | `manufacturer_id`      |           |
pub const SNCM_SCHEMA: [SncmField; 6] = [
    SncmField {
        name: "gtin",
        description: "Global Trade Item Number (GTIN-14 or EAN-13), 13-14 numeric digits",
        is_mandatory: true,
        regex_hint: Some(r"^\d{13,14}$"),
    },
    SncmField {
        name: "batch_number",
        description: "Production batch/lot number (alfanumérico)",
        is_mandatory: true,
        regex_hint: None,
    },
    SncmField {
        name: "expiry_date",
        description: "Expiry date in ISO-8601 format YYYY-MM-DD",
        is_mandatory: true,
        regex_hint: Some(r"^\d{4}-\d{2}-\d{2}$"),
    },
    SncmField {
        name: "serial_number",
        description: "Unique serialization number per unit",
        is_mandatory: true,
        regex_hint: None,
    },
    SncmField {
        name: "anvisa_registration",
        description: "Anvisa MS registration code (e.g., MS 1.0000.0001.001-1)",
        is_mandatory: false,
        regex_hint: None,
    },
    SncmField {
        name: "manufacturer_id",
        description: "CNPJ or legal entity ID of the manufacturer",
        is_mandatory: false,
        regex_hint: None,
    },
];

// ── Violation types ──────────────────────────────────────────────────────────

/// Severity level of a schema violation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViolationSeverity {
    /// Missing mandatory field — asset is flagged as non-compliant.
    Critical,
    /// Missing recommended field — reduces trust score only.
    Warning,
}

/// A single violation found during schema validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaViolation {
    /// Name of the [`TraceableAsset`] field that triggered this violation.
    pub field: String,
    /// Human-readable explanation of what is missing or incorrect.
    pub message: String,
    /// Whether this violation prevents compliance certification.
    pub severity: ViolationSeverity,
}

// ── Validation report ────────────────────────────────────────────────────────

/// Outcome of validating a [`TraceableAsset`] against [`SNCM_SCHEMA`].
///
/// Use [`SchemaValidationReport::critical_count`] and
/// [`SchemaValidationReport::warning_count`] to inspect violations, and
/// apply `gas_fee_multiplier` to any gas estimate before submitting the
/// transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaValidationReport {
    /// `true` only when there are zero [`ViolationSeverity::Critical`] violations.
    pub is_compliant: bool,
    /// All violations found during this validation run.
    pub violations: Vec<SchemaViolation>,
    /// Number of SNCM schema fields that are populated in the asset.
    pub field_count_present: usize,
    /// Total number of SNCM fields in the schema (always 6).
    pub field_count_total: usize,
    /// Gas fee multiplier to apply: `0.7` for compliant assets (30 % discount),
    /// `1.0` for non-compliant assets.
    pub gas_fee_multiplier: f64,
}

impl SchemaValidationReport {
    /// Number of [`ViolationSeverity::Critical`] violations in this report.
    #[must_use]
    pub fn critical_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity == ViolationSeverity::Critical)
            .count()
    }

    /// Number of [`ViolationSeverity::Warning`] violations in this report.
    #[must_use]
    pub fn warning_count(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity == ViolationSeverity::Warning)
            .count()
    }
}

// ── Validation entry point ───────────────────────────────────────────────────

/// Validate a [`TraceableAsset`] against the [`SNCM_SCHEMA`].
///
/// # Validation Rules
///
/// - For each of the **4 mandatory** fields: absent (`None`) or empty string
///   (`""`) → push a [`ViolationSeverity::Critical`] violation.
/// - For each of the **2 recommended** fields: absent or empty string → push a
///   [`ViolationSeverity::Warning`] violation.
///
/// # Return Value
///
/// A [`SchemaValidationReport`] containing:
/// - `is_compliant` — `true` if and only if no Critical violations were found.
/// - `violations` — the full list of detected issues.
/// - `field_count_present` / `field_count_total` — field coverage metrics.
/// - `gas_fee_multiplier` — `0.7` if compliant, `1.0` otherwise.
///
/// # Relationship to `MetadataTrustScore`
///
/// [`crate::MetadataTrustScore::compute`] and `validate_asset` both evaluate
/// the same six SNCM fields, but serve different purposes:
///
/// | Mechanism | Purpose | Effect on fees |
/// |:----------|:--------|:---------------|
/// | `MetadataTrustScore` | Quality signal for indexers and AI models | `fee_multiplier()`: 0.5× (standard) or 1.0× |
/// | `validate_asset` | SNCM regulatory compliance gate | `gas_fee_multiplier`: 0.7× (compliant) or 1.0× |
///
/// Both apply the same ISO-8601 format check to `expiry_date`.  If you tighten
/// one rule (e.g. add GTIN length validation), mirror the change in the other
/// to prevent the two mechanisms from silently diverging.
#[must_use]
pub fn validate_asset(asset: &TraceableAsset) -> SchemaValidationReport {
    let mut violations: Vec<SchemaViolation> = Vec::new();
    let mut field_count_present: usize = 0;

    // Returns `true` if the `Option<String>` is `Some` and non-empty.
    fn is_present(opt: &Option<String>) -> bool {
        opt.as_deref().map(|s| !s.is_empty()).unwrap_or(false)
    }

    // Map every schema slot to its corresponding asset field value.
    // We evaluate all `is_present` calls up-front to avoid borrowing `asset`
    // inside the mutable closure below.
    let presences: [bool; 6] = [
        is_present(&asset.gtin),
        is_present(&asset.batch_number),
        // expiry_date must be non-empty AND conform to YYYY-MM-DD (ISO-8601).
        // Mirrors the rule in `MetadataTrustScore::compute` so that both
        // mechanisms agree on what constitutes a valid expiry date.
        asset
            .expiry_date
            .as_deref()
            .is_some_and(is_valid_iso8601_date),
        is_present(&asset.serial_number),
        is_present(&asset.anvisa_registration),
        is_present(&asset.manufacturer_id),
    ];

    {
        // Inner block ensures `check`'s mutable borrows of `violations` and
        // `field_count_present` are released before we inspect them below.
        let mut check = |field: SncmField, present: bool| {
            if present {
                field_count_present += 1;
            } else {
                let (severity, prefix) = if field.is_mandatory {
                    (ViolationSeverity::Critical, "Mandatory")
                } else {
                    (ViolationSeverity::Warning, "Recommended")
                };
                violations.push(SchemaViolation {
                    field: field.name.to_owned(),
                    message: format!(
                        "{} field '{}' is missing or empty. {}",
                        prefix, field.name, field.description
                    ),
                    severity,
                });
            }
        };

        for (idx, &present) in presences.iter().enumerate() {
            check(SNCM_SCHEMA[idx], present);
        }
    }

    let is_compliant = violations
        .iter()
        .all(|v| v.severity != ViolationSeverity::Critical);

    let gas_fee_multiplier = if is_compliant { 0.7 } else { 1.0 };

    SchemaValidationReport {
        is_compliant,
        violations,
        field_count_present,
        field_count_total: SNCM_SCHEMA.len(),
        gas_fee_multiplier,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TraceableAsset;

    // ── Fixture helpers ──────────────────────────────────────────────────────

    /// All 6 SNCM fields populated — fully compliant.
    fn full_asset() -> TraceableAsset {
        TraceableAsset {
            gtin: Some("07891234567890".into()),
            batch_number: Some("LOTE-2025-001".into()),
            expiry_date: Some("2027-12-31".into()),
            serial_number: Some("SN-00000001".into()),
            anvisa_registration: Some("MS 1.0000.0001.001-1".into()),
            manufacturer_id: Some("12.345.678/0001-99".into()),
            product_name: "Dipirona Sódica 500mg".into(),
            custodian_id: "fabricante-xyz".into(),
            country_of_origin: Some("BR".into()),
            storage_temp_celsius: Some("15-30".into()),
            quantity: 1_000,
        }
    }

    /// All optional fields absent — maximally non-compliant.
    fn minimal_asset() -> TraceableAsset {
        TraceableAsset {
            gtin: None,
            batch_number: None,
            expiry_date: None,
            serial_number: None,
            anvisa_registration: None,
            manufacturer_id: None,
            product_name: "Unknown Drug".into(),
            custodian_id: "unknown".into(),
            country_of_origin: None,
            storage_temp_celsius: None,
            quantity: 1,
        }
    }

    /// Only the 4 mandatory fields are present; recommended fields are absent.
    fn mandatory_only_asset() -> TraceableAsset {
        TraceableAsset {
            gtin: Some("07891234567890".into()),
            batch_number: Some("LOTE-2025-001".into()),
            expiry_date: Some("2027-12-31".into()),
            serial_number: Some("SN-00000001".into()),
            anvisa_registration: None,
            manufacturer_id: None,
            product_name: "Test Drug".into(),
            custodian_id: "test-custodian".into(),
            country_of_origin: None,
            storage_temp_celsius: None,
            quantity: 100,
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    #[test]
    fn test_full_asset_is_compliant() {
        let report = validate_asset(&full_asset());
        assert!(report.is_compliant, "all 6 fields present → compliant");
        assert_eq!(report.violations.len(), 0, "no violations expected");
        assert_eq!(report.field_count_present, 6);
        assert_eq!(report.field_count_total, 6);
    }

    #[test]
    fn test_minimal_asset_has_critical_violations() {
        let report = validate_asset(&minimal_asset());
        assert!(!report.is_compliant, "no mandatory fields → non-compliant");
        assert_eq!(
            report.critical_count(),
            4,
            "four mandatory fields missing → four Critical violations"
        );
        assert_eq!(report.field_count_present, 0);
    }

    #[test]
    fn test_missing_gtin_critical() {
        let mut asset = full_asset();
        asset.gtin = None;
        let report = validate_asset(&asset);
        assert!(!report.is_compliant);
        assert_eq!(report.critical_count(), 1);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.field == "gtin" && v.severity == ViolationSeverity::Critical),
            "expected a Critical violation for 'gtin'"
        );
    }

    #[test]
    fn test_missing_expiry_critical() {
        let mut asset = full_asset();
        asset.expiry_date = None;
        let report = validate_asset(&asset);
        assert!(!report.is_compliant);
        assert_eq!(report.critical_count(), 1);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.field == "expiry_date" && v.severity == ViolationSeverity::Critical),
            "expected a Critical violation for 'expiry_date'"
        );
    }

    #[test]
    fn test_missing_recommended_warning() {
        let report = validate_asset(&mandatory_only_asset());
        assert!(
            report.is_compliant,
            "all mandatory fields present → compliant even with warnings"
        );
        assert_eq!(report.critical_count(), 0);
        assert_eq!(
            report.warning_count(),
            2,
            "two recommended fields absent → two Warning violations"
        );
    }

    #[test]
    fn test_gas_fee_multiplier_compliant() {
        let report = validate_asset(&full_asset());
        assert!(
            (report.gas_fee_multiplier - 0.7).abs() < f64::EPSILON,
            "compliant asset must receive 0.7× gas multiplier (30 % discount)"
        );
    }

    #[test]
    fn test_gas_fee_multiplier_non_compliant() {
        let report = validate_asset(&minimal_asset());
        assert!(
            (report.gas_fee_multiplier - 1.0).abs() < f64::EPSILON,
            "non-compliant asset must receive 1.0× gas multiplier (no discount)"
        );
    }

    #[test]
    fn test_schema_field_count() {
        assert_eq!(SNCM_SCHEMA.len(), 6, "schema must define exactly 6 fields");
        let mandatory = SNCM_SCHEMA.iter().filter(|f| f.is_mandatory).count();
        let recommended = SNCM_SCHEMA.iter().filter(|f| !f.is_mandatory).count();
        assert_eq!(mandatory, 4, "expected 4 mandatory fields");
        assert_eq!(recommended, 2, "expected 2 recommended fields");
    }

    #[test]
    fn test_empty_string_gtin_critical() {
        let mut asset = full_asset();
        asset.gtin = Some("".into()); // Some("") is treated as absent
        let report = validate_asset(&asset);
        assert!(
            !report.is_compliant,
            "empty gtin string must cause non-compliance"
        );
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.field == "gtin" && v.severity == ViolationSeverity::Critical),
            "expected a Critical violation for empty 'gtin'"
        );
    }
}
