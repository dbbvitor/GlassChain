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
//! to the path given by `--output` or to `stdout`.  A human-readable summary
//! is always printed to `stdout` regardless of the output destination.

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
/// the JSON to the configured destination, and prints a human-readable summary
/// to `stdout`.
///
/// # Errors
///
/// Returns an error if:
/// - The Root CA or member certificate cannot be generated (`--org` mode).
/// - The output file cannot be written (`--output` mode).
/// - JSON serialisation fails (should be unreachable in practice).
#[allow(clippy::needless_pass_by_value)] // clap gives us owned Args; consuming them is idiomatic
pub fn run(args: IdentityGenArgs) -> Result<()> {
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
        println!("Identity JSON written to: {path}");
        log::info!("Identity persisted to '{path}'");
    } else {
        println!("{json}");
    }

    // ── Human-readable summary (always to stdout) ──────────────────────────────
    println!();
    println!("────────────────────────────────────────");
    println!("  GlassChain Identity Summary");
    println!("────────────────────────────────────────");
    println!("  Node ID      : {}", doc.node_id);
    println!("  Public Key   : {}", doc.public_key_hex);
    println!(
        "  Certificate  : {}",
        if doc.has_certificate {
            "present (X.509 / ed25519)"
        } else {
            "none (standalone key pair)"
        }
    );
    if let Some(ref org) = doc.organization {
        println!("  Organisation : {org}");
    }
    println!("────────────────────────────────────────");

    Ok(())
}
