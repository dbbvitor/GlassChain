//! `identity-gen` sub-command — generate a new node identity for `GlassChain`.
//!
//! Supports two modes:
//!
//! - **Standalone** (`--node-id <id>`): generates an ed25519 key pair with no
//!   X.509 certificate attached.
//! - **Organisation-issued** (`--node-id <id> --org <name>`): creates a Root CA
//!   for the named organisation, issues a member certificate for the node, and
//!   records both the public key and the PEM certificate in the output.
//!
//! The identity is serialised to a compact JSON document and written either
//! to the path given by `--output` or to the output writer.  A human-readable
//! summary is always written to the output writer regardless of the
//! destination.

use std::fs;

use anyhow::Result;
use clap::Args;
use glasschain_identity::{Identity, Organization};
use serde::Serialize;

// ── Argument struct ────────────────────────────────────────────────────────────

/// Arguments accepted by the `identity-gen` sub-command.
#[derive(Args, Debug)]
pub struct IdentityGenArgs {
    /// Node ID for the new identity (e.g. `"warehouse-node-1"`).
    #[arg(long)]
    pub node_id: String,

    /// Organisation name.
    ///
    /// When provided, a Root CA is created for the organisation and a member
    /// X.509 certificate is issued and embedded in the output JSON.
    #[arg(long)]
    pub org: Option<String>,

    /// Output file path for the identity JSON.
    ///
    /// When omitted the JSON is written to `stdout`.
    #[arg(long)]
    pub output: Option<String>,
}

// ── Serialisable output document ───────────────────────────────────────────────

/// A JSON-serialisable summary of a generated identity.
///
/// Only the *public* material is included; the ed25519 signing key never
/// leaves the in-process [`Identity`] object and is not serialised.
#[derive(Debug, Serialize)]
struct IdentityDocument {
    /// Human-readable node / participant identifier.
    node_id: String,
    /// 64-character hex-encoded ed25519 public key.
    public_key_hex: String,
    /// `true` when an X.509 certificate signed by the organisation Root CA is
    /// present in this document.
    has_certificate: bool,
    /// PEM-encoded X.509 member certificate, if the identity was issued by an
    /// [`Organization`].
    #[serde(skip_serializing_if = "Option::is_none")]
    certificate_pem: Option<String>,
    /// Name of the issuing organisation, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    organization: Option<String>,
}

// ── Command entry point ────────────────────────────────────────────────────────

/// Execute the `identity-gen` command.
///
/// Generates an identity according to `args`, serialises it to JSON, writes
/// the JSON to the configured destination, and writes a human-readable summary
/// to `out`.
///
/// # Errors
///
/// Returns an error if:
/// - The Root CA or member certificate cannot be generated (`--org` mode).
/// - The output file cannot be written (`--output` mode).
/// - JSON serialisation fails (should be unreachable in practice).
/// - Writing to `out` fails.
#[allow(clippy::needless_pass_by_value)] // clap gives us owned Args; consuming them is idiomatic
pub fn run(args: IdentityGenArgs, out: &mut dyn std::io::Write) -> Result<()> {
    log::info!(
        "identity-gen: node_id={}, org={:?}, output={:?}",
        args.node_id,
        args.org,
        args.output,
    );

    // ── Generate identity ──────────────────────────────────────────────────────
    let (identity, org_name) = if let Some(ref org_name) = args.org {
        log::info!("Creating organisation '{org_name}' with Root CA …");
        let mut org = Organization::new(org_name)?;

        log::info!("Issuing member certificate for node '{}' …", &args.node_id);
        // `issue_identity` returns `&Identity`; clone so we own the value.
        let identity = org.issue_identity(&args.node_id)?.clone();

        (identity, Some(org_name.clone()))
    } else {
        log::info!(
            "Generating standalone identity for node '{}' …",
            &args.node_id
        );
        (Identity::generate(&args.node_id), None)
    };

    // ── Build output document ──────────────────────────────────────────────────
    let doc = IdentityDocument {
        node_id: identity.node_id.clone(),
        public_key_hex: identity.public_key_hex(),
        has_certificate: identity.certificate_pem.is_some(),
        certificate_pem: identity.certificate_pem,
        organization: org_name,
    };

    let json = serde_json::to_string_pretty(&doc)?;

    // ── Write JSON ─────────────────────────────────────────────────────────────
    if let Some(ref path) = args.output {
        fs::write(path, &json)?;
        writeln!(out, "Identity JSON written to: {path}")?;
        log::info!("Identity persisted to '{path}'");
    } else {
        writeln!(out, "{json}")?;
    }

    // ── Human-readable summary (always written to the sink) ────────────────────
    writeln!(out)?;
    writeln!(out, "────────────────────────────────────────")?;
    writeln!(out, "  GlassChain Identity Summary")?;
    writeln!(out, "────────────────────────────────────────")?;
    writeln!(out, "  Node ID      : {}", doc.node_id)?;
    writeln!(out, "  Public Key   : {}", doc.public_key_hex)?;
    writeln!(
        out,
        "  Certificate  : {}",
        if doc.has_certificate {
            "present (X.509 / ed25519)"
        } else {
            "none (standalone key pair)"
        }
    )?;
    if let Some(ref org) = doc.organization {
        writeln!(out, "  Organisation : {org}")?;
    }
    writeln!(out, "────────────────────────────────────────")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_captured(args: IdentityGenArgs) -> String {
        let mut out = Vec::new();
        run(args, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    /// Extract the pretty-printed JSON document from the top of the captured
    /// output (the human summary follows after a blank line).
    fn json_doc(text: &str) -> serde_json::Value {
        serde_json::from_str(text.split("\n\n").next().unwrap()).unwrap()
    }

    #[test]
    fn standalone_writes_json_and_summary_to_stdout() {
        let args = IdentityGenArgs {
            node_id: "standalone-node-1".into(),
            org: None,
            output: None,
        };
        let text = run_captured(args);

        // JSON goes to the sink; no file-destination branch.
        assert!(text.contains("standalone-node-1"));
        assert!(!text.contains("Identity JSON written to:"));

        // JSON serialisation: public material only, no certificate.
        let doc = json_doc(&text);
        assert_eq!(doc["node_id"], "standalone-node-1");
        assert!(!doc["has_certificate"].as_bool().unwrap());
        let public_key = doc["public_key_hex"].as_str().unwrap();
        assert_eq!(public_key.len(), 64);
        assert!(public_key.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert!(doc.get("certificate_pem").is_none());
        assert!(doc.get("organization").is_none());

        // Cert-present summary branch: standalone reports no certificate.
        assert!(text.contains("Certificate  : none (standalone key pair)"));
        assert!(text.contains("Node ID      : standalone-node-1"));
        assert!(!text.contains("Organisation"));
    }

    #[test]
    fn org_issued_writes_certificate_and_organization() {
        let args = IdentityGenArgs {
            node_id: "org-node-1".into(),
            org: Some("PharmaCorp".into()),
            output: None,
        };
        let text = run_captured(args);

        // Cert-present summary branch.
        assert!(text.contains("Certificate  : present (X.509 / ed25519)"));
        assert!(text.contains("Organisation : PharmaCorp"));

        let doc = json_doc(&text);
        assert_eq!(doc["node_id"], "org-node-1");
        assert!(doc["has_certificate"].as_bool().unwrap());
        assert_eq!(doc["organization"], "PharmaCorp");
        assert!(doc["certificate_pem"]
            .as_str()
            .unwrap()
            .contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn file_output_writes_json_to_path_not_stdout() {
        let path = std::env::temp_dir().join(format!(
            "glasschain-identity-test-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let args = IdentityGenArgs {
            node_id: "file-node-1".into(),
            org: None,
            output: Some(path.to_string_lossy().into_owned()),
        };
        let text = run_captured(args);

        // File branch reports the destination and still prints the summary.
        assert!(text.contains("Identity JSON written to:"));
        assert!(text.contains("Node ID      : file-node-1"));
        // The JSON document itself went to the file, not the sink.
        assert!(!text.contains("\"public_key_hex\""));

        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["node_id"], "file-node-1");
        assert!(!doc["has_certificate"].as_bool().unwrap());

        std::fs::remove_file(&path).unwrap();
    }
}
