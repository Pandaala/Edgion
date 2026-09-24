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
#[path = "../../../../tests/unit/conf_mgr/conf_center/file_system/path_registry_test.rs"]
mod tests;
