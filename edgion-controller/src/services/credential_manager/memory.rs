//! Controller-only in-process store for generated credentials.
//!
//! Entries are keyed by LinkSys identity (`namespace/name/uid`) and never
//! attached to the LinkSys resource type or conf_sync payload.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use indexmap::IndexMap;
use parking_lot::RwLock;
use thiserror::Error;
use zeroize::Zeroizing;

/// One generated credential. `Debug` never prints `value`.
pub struct GeneratedSecretEntry {
    pub key: String,
    pub value: Zeroizing<Vec<u8>>,
    pub issued_at: String,
    pub valid_until: String,
    pub valid_until_mono: Instant,
}

impl fmt::Debug for GeneratedSecretEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GeneratedSecretEntry")
            .field("key", &self.key)
            .field("value", &"[redacted]")
            .field("issued_at", &self.issued_at)
            .field("valid_until", &self.valid_until)
            .field("valid_until_mono", &self.valid_until_mono)
            .finish()
    }
}

/// Fail-closed insert when the operator cap cannot be met without dropping
/// the live active key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MemoryInsertError {
    #[error(
        "credential memory cap reached for this LinkSys source; the active key cannot be evicted. Increase publication.memoryMaxKeys or wait for rotation"
    )]
    CapReached,
}

#[derive(Clone)]
struct SourceMemorySlot {
    entries: IndexMap<String, Arc<GeneratedSecretEntry>>,
    active_key: String,
    max_keys: u32,
}

/// Per-LinkSys generated-secret map owned by the controller process.
pub struct CredentialMemoryStore {
    sources: RwLock<HashMap<String, SourceMemorySlot>>,
}

impl CredentialMemoryStore {
    pub fn new() -> Self {
        Self {
            sources: RwLock::new(HashMap::new()),
        }
    }

    /// Insert an OAuth publication: the new key becomes active, at most the
    /// previous active is retained, older OAuth ids are dropped, then the
    /// operator `max_keys` cap is applied without evicting the new active.
    pub fn insert_oauth_publication(
        &self,
        source_identity: &str,
        entry: GeneratedSecretEntry,
        max_keys: u32,
    ) -> Result<(), MemoryInsertError> {
        let mut sources = self.sources.write();
        let previous = sources.get(source_identity).cloned();
        let mut next = previous.unwrap_or_else(|| SourceMemorySlot {
            entries: IndexMap::new(),
            active_key: String::new(),
            max_keys,
        });

        let new_key = entry.key.clone();
        let previous_active = next.active_key.clone();

        let mut entries = IndexMap::new();
        if !previous_active.is_empty() && previous_active != new_key {
            if let Some(retained) = next.entries.get(&previous_active) {
                entries.insert(previous_active, Arc::clone(retained));
            }
        }
        entries.insert(new_key.clone(), Arc::new(entry));

        while entries.len() > max_keys as usize {
            let victim = entries.keys().find(|key| *key != &new_key).cloned();
            match victim {
                Some(key) => {
                    entries.shift_remove(&key);
                }
                None => return Err(MemoryInsertError::CapReached),
            }
        }

        next.entries = entries;
        next.active_key = new_key;
        next.max_keys = max_keys;
        sources.insert(source_identity.to_string(), next);
        Ok(())
    }

    pub fn active(&self, source_identity: &str) -> Option<Arc<GeneratedSecretEntry>> {
        let sources = self.sources.read();
        let slot = sources.get(source_identity)?;
        slot.entries.get(&slot.active_key).cloned()
    }

    pub fn clear_source(&self, source_identity: &str) {
        self.sources.write().remove(source_identity);
    }

    pub fn clear(&self) {
        self.sources.write().clear();
    }

    pub fn source_count(&self) -> usize {
        self.sources.read().len()
    }

    pub fn key_count(&self) -> usize {
        self.sources.read().values().map(|slot| slot.entries.len()).sum()
    }

    /// LinkSys identities only — never secret keys or values.
    pub fn source_keys(&self) -> Vec<String> {
        self.sources.read().keys().cloned().collect()
    }
}

impl Default for CredentialMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/services/credential_manager/memory_test.rs"]
mod tests;
