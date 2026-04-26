//! Traceable asset model for regulatory compliance and supply-chain transparency.
//!
//! This module implements the **Phase 3 "Traceability-First" data model**,
//! introducing [`TraceableAsset`] — a rich, field-complete representation of a
//! pharmaceutical or serialised product — and the [`MetadataTrustScore`] engine
//! that incentivises participants to provide complete regulatory data without
//! hard failures ("nudge" approach).
//!
//! ## Regulatory Alignment
//! The core fields map directly to the Brazilian Anvisa **SNCM** requirements
//! (RDC 157/2017) as well as the global **GS1 EPCIS** standard:
//!
//! | SNCM/GS1 field        | Rust field              |
//! |:----------------------|:------------------------|
//! | GTIN-14 / EAN-13      | `gtin`                  |
//! | Número de lote        | `batch_number`          |
//! | Data de validade      | `expiry_date`           |
//! | Número de série       | `serial_number`         |
//! | Código ANVISA         | `anvisa_registration`   |
//! | CNPJ fabricante       | `manufacturer_id`       |
//!
//! ## The "Nudge" Strategy
//! Instead of rejecting incomplete transactions, the ledger assigns a
//! [`MetadataTrustScore`] (0–100).  External tools (indexers, AI agents, and
//! reporting dashboards) surface low-score assets in a "Low Trust" bucket,
//! creating economic and reputational pressure on suppliers to improve data
//! quality.
//!
//! Fee scaling (50 % discount for standard data) is enforced at the node layer.

use serde::{Deserialize, Serialize};

/// A fully traceable pharmaceutical or serialised product asset.
///
/// This struct is the central data model for GlassChain's regulatory
/// compliance layer.  Participants SHOULD populate all fields;
/// [`MetadataTrustScore::compute`] will flag missing core fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceableAsset {
    // ── Core identity (GS1 / Anvisa mandatory) ──────────────────────────────

    /// Global Trade Item Number (GTIN-14 or EAN-13).
    ///
    /// Required for Anvisa SNCM compliance (RDC 157/2017, Art. 3).
    pub gtin: Option<String>,

    /// Production batch / lot number.
    ///
    /// Required for Anvisa SNCM compliance.
    pub batch_number: Option<String>,

    /// Product expiry date in ISO-8601 format (`YYYY-MM-DD`).
    ///
    /// Required for Anvisa SNCM compliance.
    pub expiry_date: Option<String>,

    /// Unique serialisation number (GS1 Serial Shipping Container Code, SSCC,
    /// or a manufacturer-assigned serial).
    ///
    /// Required for Anvisa SNCM compliance (individual-unit traceability).
    pub serial_number: Option<String>,

    // ── Regulatory metadata ─────────────────────────────────────────────────

    /// Anvisa product registration number (`MS xxxxxx.xxxxxx`).
    pub anvisa_registration: Option<String>,

    /// CNPJ or legal entity identifier of the manufacturer.
    pub manufacturer_id: Option<String>,

    // ── Supply-chain context ────────────────────────────────────────────────

    /// Human-readable product name.
    pub product_name: String,

    /// Current custodian's node/participant identifier.
    pub custodian_id: String,

    /// ISO-3166-1 alpha-2 country code of origin.
    pub country_of_origin: Option<String>,

    /// Storage temperature range in degrees Celsius, e.g. `"2-8"` for
    /// cold-chain products.
    pub storage_temp_celsius: Option<String>,

    /// Quantity of units in this asset record.
    pub quantity: u64,
}

/// The result of a trust-score computation for a [`TraceableAsset`].
///
/// Scores range from **0** (no regulatory metadata) to **100** (all core
/// fields present).  The breakdown shows which specific fields contributed
/// to or reduced the score.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetadataTrustScore {
    /// Overall score in the range \[0, 100\].
    pub score: u8,
    /// `true` when `score >= TRUST_SCORE_STANDARD_THRESHOLD`.
    pub is_standard: bool,
    /// Missing core fields that reduced the score (field names).
    pub missing_core_fields: Vec<String>,
    /// Present optional fields that boosted the score (field names).
    pub bonus_fields_present: Vec<String>,
}

/// Assets with `score >= TRUST_SCORE_STANDARD_THRESHOLD` qualify for the
/// 50 % fee discount and are placed in the "High Trust" indexer bucket.
pub const TRUST_SCORE_STANDARD_THRESHOLD: u8 = 80;

/// Return `true` if `s` is a well-formed ISO-8601 date (`YYYY-MM-DD`).
///
/// This is a lightweight structural check: it verifies the length, separators,
/// and that the numeric fields are in plausible ranges.  It does not perform a
/// full calendar validation (e.g., it accepts February 30).
fn is_valid_iso8601_date(s: &str) -> bool {
    // Expected format: "YYYY-MM-DD"
    if s.len() != 10 {
        return false;
    }
    let bytes = s.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let year: u16 = match s[0..4].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let month: u8 = match s[5..7].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let day: u8 = match s[8..10].parse() {
        Ok(v) => v,
        Err(_) => return false,
    };
    year >= 1900 && month >= 1 && month <= 12 && day >= 1 && day <= 31
}

impl MetadataTrustScore {
    /// Compute the trust score for a [`TraceableAsset`].
    ///
    /// Scoring rules:
    /// - Each of the 4 **core** fields (GTIN, batch, expiry, serial) is worth
    ///   20 points → maximum 80 points from core.
    /// - Each of the 2 **bonus** fields (Anvisa registration, manufacturer ID)
    ///   is worth 10 points → maximum 20 points from bonus.
    /// - Total maximum score: **100**.
    pub fn compute(asset: &TraceableAsset) -> Self {
        let mut score: u8 = 0;
        let mut missing = Vec::new();
        let mut bonus = Vec::new();

        // Core fields — 20 pts each.
        if asset.gtin.as_deref().map(|s| !s.is_empty()).unwrap_or(false) {
            score += 20;
        } else {
            missing.push("gtin".to_owned());
        }
        if asset
            .batch_number
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            score += 20;
        } else {
            missing.push("batch_number".to_owned());
        }
        // expiry_date must be non-empty AND conform to YYYY-MM-DD (ISO-8601)
        // to earn points.  A malformed date does not score.
        if asset
            .expiry_date
            .as_deref()
            .map(is_valid_iso8601_date)
            .unwrap_or(false)
        {
            score += 20;
        } else {
            missing.push("expiry_date".to_owned());
        }
        if asset
            .serial_number
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            score += 20;
        } else {
            missing.push("serial_number".to_owned());
        }

        // Bonus fields — 10 pts each.
        if asset
            .anvisa_registration
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            score += 10;
            bonus.push("anvisa_registration".to_owned());
        }
        if asset
            .manufacturer_id
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            score += 10;
            bonus.push("manufacturer_id".to_owned());
        }

        let is_standard = score >= TRUST_SCORE_STANDARD_THRESHOLD;

        MetadataTrustScore {
            score,
            is_standard,
            missing_core_fields: missing,
            bonus_fields_present: bonus,
        }
    }

    /// Return the fee multiplier for transactions carrying this asset.
    ///
    /// Standard-compliant assets (`is_standard == true`) pay 50 % less gas,
    /// incentivising participants to supply complete regulatory data.
    pub fn fee_multiplier(&self) -> f64 {
        if self.is_standard {
            0.5
        } else {
            1.0
        }
    }
}

impl std::fmt::Display for MetadataTrustScore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "TrustScore({}/100, standard={}, missing={:?})",
            self.score, self.is_standard, self.missing_core_fields
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            quantity: 1000,
        }
    }

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

    fn partial_asset() -> TraceableAsset {
        TraceableAsset {
            gtin: Some("07891234567890".into()),
            batch_number: Some("LOTE-A".into()),
            expiry_date: None, // missing
            serial_number: None, // missing
            ..full_asset()
        }
    }

    #[test]
    fn test_full_asset_scores_100() {
        let score = MetadataTrustScore::compute(&full_asset());
        assert_eq!(score.score, 100);
        assert!(score.is_standard);
        assert!(score.missing_core_fields.is_empty());
        assert_eq!(score.bonus_fields_present.len(), 2);
    }

    #[test]
    fn test_minimal_asset_scores_0() {
        let score = MetadataTrustScore::compute(&minimal_asset());
        assert_eq!(score.score, 0);
        assert!(!score.is_standard);
        assert_eq!(score.missing_core_fields.len(), 4);
        assert!(score.bonus_fields_present.is_empty());
    }

    #[test]
    fn test_partial_asset_scores_correctly() {
        let score = MetadataTrustScore::compute(&partial_asset());
        // gtin (20) + batch_number (20) + anvisa (10) + manufacturer (10) = 60
        assert_eq!(score.score, 60);
        assert!(!score.is_standard);
        assert!(score.missing_core_fields.iter().any(|f| f == "expiry_date"));
        assert!(score.missing_core_fields.iter().any(|f| f == "serial_number"));
    }

    #[test]
    fn test_standard_threshold_boundary() {
        // 4 core fields = 80 → exactly at threshold
        let mut asset = full_asset();
        asset.anvisa_registration = None;
        asset.manufacturer_id = None;
        let score = MetadataTrustScore::compute(&asset);
        assert_eq!(score.score, 80);
        assert!(score.is_standard);
    }

    #[test]
    fn test_fee_multiplier_standard() {
        let score = MetadataTrustScore::compute(&full_asset());
        assert!((score.fee_multiplier() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_fee_multiplier_non_standard() {
        let score = MetadataTrustScore::compute(&minimal_asset());
        assert!((score.fee_multiplier() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_display_format() {
        let score = MetadataTrustScore::compute(&full_asset());
        let s = score.to_string();
        assert!(s.contains("100/100"));
        assert!(s.contains("standard=true"));
    }

    #[test]
    fn test_malformed_expiry_date_does_not_score() {
        let mut asset = full_asset();
        asset.expiry_date = Some("31-12-2027".into()); // DD-MM-YYYY is not ISO-8601
        let score = MetadataTrustScore::compute(&asset);
        assert!(score.missing_core_fields.iter().any(|f| f == "expiry_date"));
        // gtin(20) + batch(20) + serial(20) + anvisa(10) + manufacturer(10) = 80
        assert_eq!(score.score, 80);
    }

    #[test]
    fn test_valid_iso8601_date_scores() {
        let mut asset = full_asset();
        asset.expiry_date = Some("2027-12-31".into()); // correct ISO-8601
        let score = MetadataTrustScore::compute(&asset);
        assert!(!score.missing_core_fields.iter().any(|f| f == "expiry_date"));
    }

    #[test]
    fn test_is_valid_iso8601_date_helper() {
        assert!(super::is_valid_iso8601_date("2027-06-30"));
        assert!(super::is_valid_iso8601_date("1900-01-01"));
        assert!(!super::is_valid_iso8601_date("30-06-2027")); // wrong order
        assert!(!super::is_valid_iso8601_date("2027/06/30")); // wrong separator
        assert!(!super::is_valid_iso8601_date("2027-13-01")); // month > 12
        assert!(!super::is_valid_iso8601_date("2027-06-32")); // day > 31
        assert!(!super::is_valid_iso8601_date(""));
        assert!(!super::is_valid_iso8601_date("not-a-date"));
    }
}
