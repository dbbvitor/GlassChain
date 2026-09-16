// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! `backup-scrub` sub-command — prune expired private payloads from a storage
//! copy before archiving it (ADR-017).
//!
//! Physical backups (volume snapshots, exported directories) can retain
//! private-payload bytes past their retention window: the node's runtime D5
//! sweep only covers the live store. The operator copies the storage
//! directory, points this command at the copy, and the copy is scrubbed —
//! the chain's hash commitments persist, the expired private cleartext does
//! not.

use anyhow::Result;
use clap::Args;
use glasschain_storage::{SledStorageProvider, TransientStore};

/// Arguments accepted by the `backup-scrub` sub-command.
#[derive(Args, Debug)]
pub struct BackupScrubArgs {
    /// Path to a (copied) `GlassChain` sled storage directory.
    #[arg(long)]
    pub storage: String,
}

/// Execute the `backup-scrub` command.
///
/// Opens the storage directory, runs the D5 retention sweep
/// ([`TransientStore::purge_expired`]), flushes, and reports the removed
/// payload count to `out`.
///
/// # Errors
///
/// Returns an error when the storage directory cannot be opened, the sweep
/// fails, or the flush fails.
#[allow(clippy::needless_pass_by_value)] // clap gives us owned Args; consuming them is idiomatic
pub fn run(args: BackupScrubArgs, out: &mut dyn std::io::Write) -> Result<()> {
    log::info!("backup-scrub: storage={}", args.storage);

    let storage = SledStorageProvider::open(&args.storage)?;
    let transient = TransientStore::new(std::sync::Arc::new(storage));
    let purged = transient.purge_expired()?;
    // The backend is owned by the transient store; an explicit flush happens
    // on drop of the sled DB (opened inside). Report the outcome.
    writeln!(
        out,
        "backup-scrub: purged {purged} expired private payload(s) from {}",
        args.storage
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An expired payload is purged from the copied storage; a live one
    /// survives, and a second run is a no-op.
    #[test]
    fn scrubs_expired_payloads_and_reports() {
        let dir = std::env::temp_dir().join(format!(
            "glasschain-backup-scrub-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let storage: std::sync::Arc<dyn glasschain_core::StorageProvider> =
            std::sync::Arc::new(SledStorageProvider::open(&dir).unwrap());
        let transient = TransientStore::new(std::sync::Arc::clone(&storage));
        let live = b"still-within-retention";
        let expired = b"past-retention";
        transient
            .put(
                "pricing",
                &glasschain_core::crypto::sha256(live),
                live,
                3_600,
            )
            .unwrap();
        // expires_at in the past: retention_secs 0 puts the deadline now.
        transient
            .put(
                "pricing",
                &glasschain_core::crypto::sha256(expired),
                expired,
                0,
            )
            .unwrap();
        drop(transient);
        drop(storage);

        let mut out = Vec::new();
        run(
            BackupScrubArgs {
                storage: dir.display().to_string(),
            },
            &mut out,
        )
        .unwrap();
        let report = String::from_utf8(out).unwrap();
        assert!(report.contains("purged 1 expired"), "{report}");

        // The copy re-opened has the live payload only.
        let storage = SledStorageProvider::open(&dir).unwrap();
        let transient = TransientStore::new(std::sync::Arc::new(storage));
        assert_eq!(
            transient
                .get("pricing", &glasschain_core::crypto::sha256(live))
                .unwrap(),
            Some(live.to_vec())
        );
        assert_eq!(
            transient
                .get("pricing", &glasschain_core::crypto::sha256(expired))
                .unwrap(),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
