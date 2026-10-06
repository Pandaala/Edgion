//! Controller-only in-process store for generated credentials.
//!
//! Entries are keyed by LinkSys identity (`namespace/name/uid`) and never
//! attached to the LinkSys resource type or conf_sync payload.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
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

/// One active generated credential per LinkSys identity, owned by the controller.
pub struct CredentialMemoryStore {
    sources: RwLock<HashMap<String, Arc<GeneratedSecretEntry>>>,
}

impl CredentialMemoryStore {
    pub fn new() -> Self {
        Self {
            sources: RwLock::new(HashMap::new()),
        }
    }

    /// Atomically replace the active publication. Existing Arc holders remain
    /// valid; the store itself retains no previous generation.
    pub fn insert_oauth_publication(&self, source_identity: &str, entry: GeneratedSecretEntry) {
        self.sources
            .write()
            .insert(source_identity.to_string(), Arc::new(entry));
    }

    pub fn active(&self, source_identity: &str) -> Option<Arc<GeneratedSecretEntry>> {
        self.sources.read().get(source_identity).cloned()
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

    /// Active entries only, one per stored source identity. External Arc holders
    /// are not counted.
    pub fn key_count(&self) -> usize {
        self.sources.read().len()
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
