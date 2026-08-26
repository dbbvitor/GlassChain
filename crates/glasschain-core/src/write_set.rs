//! Typed execution results and scoped persistent write sets (ADR-007).
//!
//! An execution produces two distinct outputs:
//!
//! - **ephemeral output** — invocation-local `(key, value)` pairs (the legacy
//!   `set_state` semantics). Approval gates read `approve` from here, and an
//!   approval evaluation never persists anything.
//! - **persistent writes** — explicit, scoped set/delete operations that a
//!   contract opts into via the separate persistence host operation.
//!
//! [`ExecutionResult::canonicalize`] is the single validation point: every
//! scope component must be non-empty and a scoped key may have at most one
//! canonical result per execution. Duplicate operations are rejected rather
//! than left to provider-specific ordering (ADR-007 decision 2).

use crate::error::CoreError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Visibility of a persistent state write (ADR-007 decision 3).
///
/// Public values are globally projected; a PDC-scoped write never places its
/// private value in the globally replicated block — only the collection
/// reference and value/tombstone commitment (dissemination and reconciliation
/// follow ADR-003, ticket #47).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteVisibility {
    Public,
    /// Named private data collection.
    Pdc(String),
}

/// A single persistent state operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteOp {
    /// Write `bytes` under the scoped key.
    Set(Vec<u8>),
    /// Remove the scoped key (a tombstone in the committed record).
    Delete,
}

/// One scoped persistent write: channel, contract, logical key, operation, and
/// visibility. There is no implicit global or cross-channel keyspace
/// (ADR-007 decision 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistentWrite {
    /// Channel scope.
    pub channel: String,
    /// Contract scope.
    pub contract: String,
    /// Logical key within the (channel, contract) scope.
    pub key: String,
    /// Set or delete.
    pub op: WriteOp,
    /// Public or a named private data collection.
    pub visibility: WriteVisibility,
}

/// The typed execution result: ephemeral output separated from the persistent
/// write set (ADR-007 decision 1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Invocation-local output (the legacy `set_state` semantics).
    pub ephemeral: Vec<(String, Vec<u8>)>,
    /// Explicit persistent set/delete operations.
    pub writes: Vec<PersistentWrite>,
}

impl From<Vec<(String, Vec<u8>)>> for ExecutionResult {
    /// Legacy providers that only produce ephemeral output.
    fn from(ephemeral: Vec<(String, Vec<u8>)>) -> Self {
        Self {
            ephemeral,
            writes: Vec::new(),
        }
    }
}

impl ExecutionResult {
    /// Validate this result and return a deterministically ordered copy.
    ///
    /// Rules (ADR-007 decisions 2–3):
    /// - `channel`, `contract`, and `key` must be non-empty;
    /// - a `Pdc` visibility must name a non-empty collection;
    /// - a scoped `(channel, contract, key)` may appear at most once — an
    ///   ambiguous duplicate is rejected rather than resolved by
    ///   provider-specific ordering.
    ///
    /// The returned copy sorts writes by scope so the committed write set has
    /// one canonical serialization regardless of guest execution order.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTransaction`] for the first violated rule.
    pub fn canonicalize(&self) -> Result<Self, CoreError> {
        let mut seen: HashSet<(&str, &str, &str)> = HashSet::new();
        for write in &self.writes {
            if write.channel.is_empty() {
                return Err(CoreError::InvalidTransaction(
                    "persistent write: channel scope must not be empty".into(),
                ));
            }
            if write.contract.is_empty() {
                return Err(CoreError::InvalidTransaction(
                    "persistent write: contract scope must not be empty".into(),
                ));
            }
            if write.key.is_empty() {
                return Err(CoreError::InvalidTransaction(
                    "persistent write: key must not be empty".into(),
                ));
            }
            if let WriteVisibility::Pdc(name) = &write.visibility {
                if name.is_empty() {
                    return Err(CoreError::InvalidTransaction(
                        "persistent write: PDC visibility must name a collection".into(),
                    ));
                }
            }
            let scoped = (
                write.channel.as_str(),
                write.contract.as_str(),
                write.key.as_str(),
            );
            if !seen.insert(scoped) {
                return Err(CoreError::InvalidTransaction(format!(
                    "persistent write: scoped key ({}, {}, {}) has more than one operation",
                    write.channel, write.contract, write.key
                )));
            }
        }
        let mut writes = self.writes.clone();
        writes.sort_by(|a, b| {
            (&a.channel, &a.contract, &a.key).cmp(&(&b.channel, &b.contract, &b.key))
        });
        Ok(Self {
            ephemeral: self.ephemeral.clone(),
            writes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(channel: &str, contract: &str, key: &str) -> PersistentWrite {
        PersistentWrite {
            channel: channel.into(),
            contract: contract.into(),
            key: key.into(),
            op: WriteOp::Set(b"v".to_vec()),
            visibility: WriteVisibility::Public,
        }
    }

    fn result(writes: Vec<PersistentWrite>) -> ExecutionResult {
        ExecutionResult {
            ephemeral: vec![("approve".into(), b"1".to_vec())],
            writes,
        }
    }

    #[test]
    fn test_ephemeral_only_from_legacy_pairs() {
        let result: ExecutionResult = vec![("approve".into(), b"1".to_vec())].into();
        assert_eq!(result.ephemeral.len(), 1);
        assert!(result.writes.is_empty());
        assert!(result.canonicalize().is_ok());
    }

    #[test]
    fn test_canonicalize_sorts_writes_deterministically() {
        let unsorted = result(vec![
            write("ch-b", "contract", "key"),
            write("ch-a", "contract", "key"),
        ]);
        let canonical = unsorted.canonicalize().expect("valid");
        assert_eq!(canonical.writes[0].channel, "ch-a");
        assert_eq!(canonical.writes[1].channel, "ch-b");
        assert_eq!(
            canonical,
            canonical.canonicalize().expect("idempotent"),
            "canonicalization must be idempotent"
        );
    }

    #[test]
    fn test_duplicate_scoped_key_rejected() {
        let ambiguous = result(vec![
            write("ch", "contract", "key"),
            PersistentWrite {
                op: WriteOp::Delete,
                ..write("ch", "contract", "key")
            },
        ]);
        let error = ambiguous.canonicalize().expect_err("duplicate must fail");
        assert!(error.to_string().contains("more than one"), "{error}");
    }

    #[test]
    fn test_same_key_in_different_scopes_is_distinct() {
        let distinct = result(vec![
            write("ch-1", "contract", "key"),
            write("ch-2", "contract", "key"),
            write("ch-1", "other-contract", "key"),
        ]);
        assert!(distinct.canonicalize().is_ok());
    }

    #[test]
    fn test_empty_scope_components_rejected() {
        for (channel, contract, key) in [("", "c", "k"), ("ch", "", "k"), ("ch", "c", "")] {
            let bad = result(vec![write(channel, contract, key)]);
            assert!(bad.canonicalize().is_err(), "empty scope must fail");
        }
    }

    #[test]
    fn test_empty_pdc_name_rejected() {
        let bad = result(vec![PersistentWrite {
            visibility: WriteVisibility::Pdc(String::new()),
            ..write("ch", "contract", "key")
        }]);
        assert!(bad.canonicalize().is_err(), "empty PDC name must fail");
    }

    #[test]
    fn test_roundtrip_serialization() {
        let result = result(vec![write("ch", "contract", "key")])
            .canonicalize()
            .expect("valid");
        let json = serde_json::to_string(&result).expect("serialize");
        let decoded: ExecutionResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, result);
    }
}
