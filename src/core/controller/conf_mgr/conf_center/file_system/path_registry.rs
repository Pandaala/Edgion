//! PathRegistry: reverse index from (kind, key) to file system paths.
//!
//! The watcher writes; the status handler and storage CRUD read; the
//! diagnostics endpoint snapshots conflicts. See
//! `docs/superpowers/specs/2026-05-10-multi-conf-dir-design.md` §4.1.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::RwLock;

/// Identity of a logical resource. `key` is "namespace/name" for
/// namespace-scoped resources and just "name" for cluster-scoped.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct ResourceKey {
    pub kind: String,
    pub key: String,
}

impl ResourceKey {
    pub fn new(kind: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            key: key.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOutcome {
    /// First time this key is seen. `path` is the new winner.
    NewWinner { path: PathBuf },
    /// Key already has a winner. New path joined the conflict list.
    ConflictAdded { winner: PathBuf, conflicts: Vec<PathBuf> },
    /// `path` was already known under this key.
    AlreadyKnown { is_winner: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgetOutcome {
    /// Path was not in the index.
    Unknown,
    /// Path was a non-winner conflict; winner unchanged.
    ConflictRemoved {
        winner: PathBuf,
        remaining_conflicts: Vec<PathBuf>,
    },
    /// Path was the winner; first-inserted conflict was promoted.
    Promoted {
        new_winner: PathBuf,
        remaining_conflicts: Vec<PathBuf>,
    },
    /// Path was the winner with no conflicts; key fully removed.
    Removed { key: ResourceKey },
}

#[derive(Debug, Clone)]
pub struct ConflictReport {
    pub key: ResourceKey,
    pub winner: PathBuf,
    pub losers: Vec<PathBuf>,
}

#[derive(Default)]
struct RegistryInner {
    by_key: HashMap<ResourceKey, Vec<PathBuf>>,
    by_path: HashMap<PathBuf, ResourceKey>,
}

#[derive(Default)]
pub struct PathRegistry {
    inner: RwLock<RegistryInner>,
}

impl PathRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&self, key: ResourceKey, path: PathBuf) -> RecordOutcome {
        let mut inner = self.inner.write();
        if let Some(existing_key) = inner.by_path.get(&path) {
            let is_winner = inner
                .by_key
                .get(existing_key)
                .and_then(|v| v.first())
                .map(|p| p == &path)
                .unwrap_or(false);
            return RecordOutcome::AlreadyKnown { is_winner };
        }
        let was_empty = inner.by_key.get(&key).map(|v| v.is_empty()).unwrap_or(true);
        inner.by_key.entry(key.clone()).or_default().push(path.clone());
        inner.by_path.insert(path.clone(), key.clone());
        if was_empty {
            RecordOutcome::NewWinner { path }
        } else {
            let entry = &inner.by_key[&key];
            let winner = entry[0].clone();
            let conflicts = entry[1..].to_vec();
            RecordOutcome::ConflictAdded { winner, conflicts }
        }
    }

    pub fn forget(&self, path: &Path) -> ForgetOutcome {
        let mut inner = self.inner.write();
        let Some(key) = inner.by_path.remove(path) else {
            return ForgetOutcome::Unknown;
        };
        let Some(entry) = inner.by_key.get_mut(&key) else {
            return ForgetOutcome::Unknown;
        };
        let pos = entry.iter().position(|p| p == path);
        let was_winner = pos == Some(0);
        if let Some(idx) = pos {
            entry.remove(idx);
        }
        if entry.is_empty() {
            inner.by_key.remove(&key);
            return ForgetOutcome::Removed { key };
        }
        let winner = entry[0].clone();
        let remaining_conflicts = entry[1..].to_vec();
        if was_winner {
            ForgetOutcome::Promoted {
                new_winner: winner,
                remaining_conflicts,
            }
        } else {
            ForgetOutcome::ConflictRemoved {
                winner,
                remaining_conflicts,
            }
        }
    }

    pub fn winner_of(&self, key: &ResourceKey) -> Option<PathBuf> {
        let inner = self.inner.read();
        inner.by_key.get(key).and_then(|v| v.first().cloned())
    }

    pub fn snapshot_conflicts(&self) -> Vec<ConflictReport> {
        let inner = self.inner.read();
        let mut out = Vec::new();
        for (k, v) in inner.by_key.iter() {
            if v.len() > 1 {
                out.push(ConflictReport {
                    key: k.clone(),
                    winner: v[0].clone(),
                    losers: v[1..].to_vec(),
                });
            }
        }
        out
    }

    pub fn snapshot_winners(&self) -> Vec<(ResourceKey, PathBuf)> {
        let inner = self.inner.read();
        inner
            .by_key
            .iter()
            .filter_map(|(k, v)| v.first().map(|p| (k.clone(), p.clone())))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(kind: &str, key: &str) -> ResourceKey {
        ResourceKey::new(kind, key)
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn record_new_key_returns_new_winner() {
        let r = PathRegistry::new();
        let outcome = r.record(k("Service", "default/x"), p("/conf/a.yaml"));
        assert_eq!(
            outcome,
            RecordOutcome::NewWinner {
                path: p("/conf/a.yaml")
            }
        );
        assert_eq!(r.winner_of(&k("Service", "default/x")), Some(p("/conf/a.yaml")));
    }

    #[test]
    fn record_second_path_returns_conflict_winner_unchanged_regardless_of_lex_order() {
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/conf/b.yaml"));
        let outcome = r.record(k("S", "ns/x"), p("/conf/a.yaml"));
        assert_eq!(
            outcome,
            RecordOutcome::ConflictAdded {
                winner: p("/conf/b.yaml"),
                conflicts: vec![p("/conf/a.yaml")]
            }
        );
        assert_eq!(r.winner_of(&k("S", "ns/x")), Some(p("/conf/b.yaml")));
    }

    #[test]
    fn record_known_path_returns_already_known_with_correct_is_winner() {
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/conf/a.yaml"));
        r.record(k("S", "ns/x"), p("/conf/b.yaml"));
        assert_eq!(
            r.record(k("S", "ns/x"), p("/conf/a.yaml")),
            RecordOutcome::AlreadyKnown { is_winner: true }
        );
        assert_eq!(
            r.record(k("S", "ns/x"), p("/conf/b.yaml")),
            RecordOutcome::AlreadyKnown { is_winner: false }
        );
    }

    #[test]
    fn forget_winner_with_conflicts_promotes_first_inserted() {
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/conf/a.yaml"));
        r.record(k("S", "ns/x"), p("/conf/c.yaml"));
        r.record(k("S", "ns/x"), p("/conf/b.yaml"));
        let outcome = r.forget(&p("/conf/a.yaml"));
        assert_eq!(
            outcome,
            ForgetOutcome::Promoted {
                new_winner: p("/conf/c.yaml"),
                remaining_conflicts: vec![p("/conf/b.yaml")]
            }
        );
    }

    #[test]
    fn forget_non_winner_returns_conflict_removed() {
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/conf/a.yaml"));
        r.record(k("S", "ns/x"), p("/conf/b.yaml"));
        assert_eq!(
            r.forget(&p("/conf/b.yaml")),
            ForgetOutcome::ConflictRemoved {
                winner: p("/conf/a.yaml"),
                remaining_conflicts: vec![]
            }
        );
    }

    #[test]
    fn forget_unknown_path_returns_unknown() {
        let r = PathRegistry::new();
        assert_eq!(r.forget(&p("/nope")), ForgetOutcome::Unknown);
    }

    #[test]
    fn forget_only_path_returns_removed() {
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/conf/a.yaml"));
        assert_eq!(
            r.forget(&p("/conf/a.yaml")),
            ForgetOutcome::Removed { key: k("S", "ns/x") }
        );
        assert_eq!(r.winner_of(&k("S", "ns/x")), None);
    }

    #[test]
    fn snapshot_conflicts_only_lists_keys_with_more_than_one_path() {
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/conf/a.yaml"));
        r.record(k("S", "ns/x"), p("/conf/b.yaml"));
        r.record(k("S", "ns/y"), p("/conf/y.yaml"));
        let snap = r.snapshot_conflicts();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].key, k("S", "ns/x"));
        assert_eq!(snap[0].winner, p("/conf/a.yaml"));
        assert_eq!(snap[0].losers, vec![p("/conf/b.yaml")]);
    }

    #[test]
    fn snapshot_winners_returns_all_keys_with_their_winner() {
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/conf/a.yaml"));
        r.record(k("S", "ns/x"), p("/conf/b.yaml"));
        r.record(k("HTTPRoute", "ns/y"), p("/conf/y.yaml"));
        let mut snap = r.snapshot_winners();
        snap.sort_by(|a, b| a.0.key.cmp(&b.0.key));
        assert_eq!(snap.len(), 2);
    }
}
