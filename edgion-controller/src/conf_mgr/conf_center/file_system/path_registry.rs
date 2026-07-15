//! PathRegistry: reverse index from (kind, key) to file system paths.
//!
//! The watcher writes; the status handler and storage CRUD read; the
//! diagnostics endpoint snapshots conflicts. See
//! `docs/superpowers/specs/2026-05-10-multi-conf-dir-design.md` §4.1.

use std::collections::{HashMap, HashSet};
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

#[derive(Clone, Default)]
struct RegistryInner {
    by_key: HashMap<ResourceKey, Vec<PathBuf>>,
    by_path: HashMap<PathBuf, ResourceKey>,
    // Paths removed by the storage DELETE API before the fs_watcher has fired
    // the corresponding Remove event.  Entries here are invisible to winner_of()
    // so create_one() does not see a stale AlreadyExists, but they remain in
    // by_path / by_key so that forget() still returns the correct ForgetOutcome
    // (Removed / Promoted) and the watcher can dispatch the Delete event normally.
    pending_api_delete: HashSet<PathBuf>,
}

#[derive(Default)]
pub struct PathRegistry {
    inner: RwLock<RegistryInner>,
}

impl PathRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Atomically replace the index with a fully built scan generation.
    ///
    /// The watcher builds `next` privately, then swaps it in after scanning so
    /// readers keep seeing the complete previous generation instead of a
    /// partially rebuilt registry during reload.
    pub fn replace_from(&self, next: &Self) {
        *self.inner.write() = next.inner.read().clone();
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

    /// Mark a path as deleted by the storage API before the fs_watcher has
    /// confirmed the removal.  After this call winner_of() will skip this path,
    /// but by_path / by_key are unchanged so that forget() still delivers the
    /// correct ForgetOutcome when the watcher fires the Remove event.
    ///
    /// Only has effect if the path is currently tracked; silently ignored otherwise.
    pub fn mark_api_deleted(&self, path: &Path) {
        let mut inner = self.inner.write();
        if inner.by_path.contains_key(path) {
            inner.pending_api_delete.insert(path.to_path_buf());
        }
    }

    pub fn forget(&self, path: &Path) -> ForgetOutcome {
        let mut inner = self.inner.write();
        // Clear the pending-delete marker unconditionally; forget() is the
        // authoritative removal driven by the fs_watcher.
        inner.pending_api_delete.remove(path);
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
        inner.by_key.get(key).and_then(|v| {
            // Skip paths that have been API-deleted but not yet confirmed by the
            // watcher; they are still live in by_path / by_key so forget() works
            // correctly, but they must not block create_one() with AlreadyExists.
            v.iter().find(|p| !inner.pending_api_delete.contains(*p)).cloned()
        })
    }

    /// True iff `path` is a recorded non-winner (conflict loser) for its key.
    ///
    /// Unknown paths (not yet recorded by the watcher) return `false` — they are
    /// kept and treated as winners, so in the cold-registry window the list is a
    /// strict no-op (identical to the current unfiltered scan). It only positively
    /// drops a path the registry KNOWS is a non-winner.
    ///
    /// The "winner" here is the SAME pending-aware value `winner_of` computes
    /// (first path of the key not in `pending_api_delete`), so list and `get_one`
    /// agree, and the wipe drain loop's pass-2 promoted loser is not wrongly dropped.
    pub fn is_recorded_loser(&self, path: &Path) -> bool {
        let inner = self.inner.read();
        let Some(key) = inner.by_path.get(path) else {
            return false; // unknown path -> keep
        };
        let live_winner = inner
            .by_key
            .get(key)
            .and_then(|v| v.iter().find(|p| !inner.pending_api_delete.contains(*p)));
        match live_winner {
            Some(w) => w.as_path() != path, // loser iff it is not the live winner
            None => false,                  // no live winner left -> keep
        }
    }

    pub fn snapshot_conflicts(&self) -> Vec<ConflictReport> {
        let inner = self.inner.read();
        let mut out = Vec::new();
        for (k, v) in inner.by_key.iter() {
            let live: Vec<&PathBuf> = v.iter().filter(|p| !inner.pending_api_delete.contains(*p)).collect();
            if live.len() > 1 {
                out.push(ConflictReport {
                    key: k.clone(),
                    winner: live[0].clone(),
                    losers: live[1..].iter().map(|p| (*p).clone()).collect(),
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
            .filter_map(|(k, v)| {
                v.iter()
                    .find(|p| !inner.pending_api_delete.contains(*p))
                    .map(|p| (k.clone(), p.clone()))
            })
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
    fn replace_from_atomically_installs_a_new_scan_generation() {
        let current = PathRegistry::new();
        let key = k("Service", "default/x");
        current.record(key.clone(), p("/old/service.yaml"));
        let next = PathRegistry::new();
        let new_path = p("/new/service.yaml");
        next.record(key.clone(), new_path.clone());

        current.replace_from(&next);

        assert_eq!(current.winner_of(&key), Some(new_path));
        assert!(current.snapshot_conflicts().is_empty());
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
    fn mark_api_deleted_hides_winner_from_winner_of() {
        let r = PathRegistry::new();
        r.record(k("Gateway", "ns/gw"), p("/conf/gw.yaml"));
        r.mark_api_deleted(&p("/conf/gw.yaml"));
        assert_eq!(r.winner_of(&k("Gateway", "ns/gw")), None);
    }

    #[test]
    fn forget_after_mark_api_deleted_returns_removed_and_clears_marker() {
        let r = PathRegistry::new();
        r.record(k("Gateway", "ns/gw"), p("/conf/gw.yaml"));
        r.mark_api_deleted(&p("/conf/gw.yaml"));
        // watcher fires Remove — must still get Removed so Delete event is dispatched
        assert_eq!(
            r.forget(&p("/conf/gw.yaml")),
            ForgetOutcome::Removed {
                key: k("Gateway", "ns/gw")
            }
        );
        // pending marker gone, winner_of is also gone
        assert_eq!(r.winner_of(&k("Gateway", "ns/gw")), None);
    }

    #[test]
    fn mark_api_deleted_winner_with_conflict_hides_winner_but_exposes_conflict() {
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/a.yaml")); // winner
        r.record(k("S", "ns/x"), p("/b.yaml")); // conflict
        r.mark_api_deleted(&p("/a.yaml"));
        // conflict file is still live — winner_of must surface it
        assert_eq!(r.winner_of(&k("S", "ns/x")), Some(p("/b.yaml")));
    }

    #[test]
    fn forget_after_mark_api_deleted_winner_with_conflict_promotes() {
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/a.yaml"));
        r.record(k("S", "ns/x"), p("/b.yaml"));
        r.mark_api_deleted(&p("/a.yaml"));
        // watcher fires Remove for /a.yaml — must still return Promoted
        assert_eq!(
            r.forget(&p("/a.yaml")),
            ForgetOutcome::Promoted {
                new_winner: p("/b.yaml"),
                remaining_conflicts: vec![]
            }
        );
    }

    #[test]
    fn mark_api_deleted_unknown_path_is_noop() {
        let r = PathRegistry::new();
        r.mark_api_deleted(&p("/nonexistent.yaml")); // must not panic
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

    // --- snapshot_conflicts pending-aware tests ---

    #[test]
    fn snapshot_conflicts_pending_winner_with_one_live_conflict_not_reported() {
        // winner pending + 1 live conflict → live.len() == 1 → not a conflict
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/a.yaml")); // winner
        r.record(k("S", "ns/x"), p("/b.yaml")); // conflict
        r.mark_api_deleted(&p("/a.yaml"));
        let snap = r.snapshot_conflicts();
        assert!(snap.is_empty(), "one live path remaining is not a conflict");
    }

    #[test]
    fn snapshot_conflicts_pending_winner_with_two_live_conflicts_reports_correctly() {
        // winner pending + 2 live conflicts → live winner = first live conflict
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/a.yaml")); // winner
        r.record(k("S", "ns/x"), p("/b.yaml")); // conflict 1
        r.record(k("S", "ns/x"), p("/c.yaml")); // conflict 2
        r.mark_api_deleted(&p("/a.yaml"));
        let snap = r.snapshot_conflicts();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].winner, p("/b.yaml"));
        assert_eq!(snap[0].losers, vec![p("/c.yaml")]);
    }

    // --- snapshot_winners pending-aware tests ---

    #[test]
    fn snapshot_winners_excludes_key_when_all_paths_pending() {
        let r = PathRegistry::new();
        r.record(k("Gateway", "ns/gw"), p("/gw.yaml"));
        r.record(k("S", "ns/x"), p("/x.yaml"));
        r.mark_api_deleted(&p("/gw.yaml")); // only path for ns/gw is pending
        let snap = r.snapshot_winners();
        // ns/gw has no live winner — must not appear
        assert!(
            snap.iter().all(|(rk, _)| rk != &k("Gateway", "ns/gw")),
            "pending-only key must be excluded from snapshot_winners"
        );
        // ns/x is untouched — must still appear
        assert!(snap.iter().any(|(rk, _)| rk == &k("S", "ns/x")));
    }

    #[test]
    fn snapshot_winners_consistent_with_winner_of_under_pending() {
        // For every key, snapshot_winners and winner_of must agree
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/a.yaml")); // winner
        r.record(k("S", "ns/x"), p("/b.yaml")); // conflict
        r.record(k("S", "ns/y"), p("/y.yaml"));
        r.mark_api_deleted(&p("/a.yaml")); // ns/x winner is pending

        let snap: std::collections::HashMap<_, _> = r.snapshot_winners().into_iter().collect();

        // ns/x: snapshot_winners and winner_of must both return /b.yaml
        assert_eq!(snap.get(&k("S", "ns/x")), Some(&p("/b.yaml")));
        assert_eq!(r.winner_of(&k("S", "ns/x")), Some(p("/b.yaml")));

        // ns/y: unaffected — both return /y.yaml
        assert_eq!(snap.get(&k("S", "ns/y")), Some(&p("/y.yaml")));
        assert_eq!(r.winner_of(&k("S", "ns/y")), Some(p("/y.yaml")));
    }

    // --- is_recorded_loser tests ---

    #[test]
    fn is_recorded_loser_unknown_path_is_false() {
        let r = PathRegistry::new();
        assert!(!r.is_recorded_loser(&p("/conf/x.yaml")));
    }

    #[test]
    fn is_recorded_loser_winner_false_loser_true() {
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/conf/a.yaml")); // winner (first)
        r.record(k("S", "ns/x"), p("/conf/b.yaml")); // loser
        assert!(!r.is_recorded_loser(&p("/conf/a.yaml")), "winner is not a loser");
        assert!(r.is_recorded_loser(&p("/conf/b.yaml")), "second path is a loser");
    }

    #[test]
    fn is_recorded_loser_pending_winner_promotes_loser() {
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/conf/a.yaml"));
        r.record(k("S", "ns/x"), p("/conf/b.yaml"));
        r.mark_api_deleted(&p("/conf/a.yaml")); // a pending -> b is the live winner
        assert!(
            !r.is_recorded_loser(&p("/conf/b.yaml")),
            "promoted loser must no longer be flagged a loser (pending-aware)"
        );
    }

    #[test]
    fn is_recorded_loser_all_pending_is_false() {
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/conf/a.yaml"));
        r.mark_api_deleted(&p("/conf/a.yaml")); // no live winner left
        assert!(!r.is_recorded_loser(&p("/conf/a.yaml")), "no live winner -> keep");
    }

    #[test]
    fn is_recorded_loser_all_pending_two_paths_is_false() {
        // winner + loser BOTH pending -> no live winner -> neither path is a loser
        // (exercises the `None => false` branch with more than one recorded path).
        let r = PathRegistry::new();
        r.record(k("S", "ns/x"), p("/conf/a.yaml")); // winner
        r.record(k("S", "ns/x"), p("/conf/b.yaml")); // loser
        r.mark_api_deleted(&p("/conf/a.yaml"));
        r.mark_api_deleted(&p("/conf/b.yaml"));
        assert!(
            !r.is_recorded_loser(&p("/conf/a.yaml")),
            "all paths pending -> keep (no live winner)"
        );
        assert!(
            !r.is_recorded_loser(&p("/conf/b.yaml")),
            "all paths pending -> keep (no live winner)"
        );
    }
}
