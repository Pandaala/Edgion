//! Etcd event types for the ConfCenter watcher → resource controller pipeline.

/// A present etcd document, including values that cannot be decoded as text.
/// Invalid bytes are discarded at ingress; rejection policy belongs to the consumer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EtcdDocument {
    Text(String),
    InvalidUtf8,
}

impl EtcdDocument {
    pub fn from_bytes(value: &[u8]) -> Self {
        match std::str::from_utf8(value) {
            Ok(content) => Self::Text(content.to_owned()),
            Err(_) => Self::InvalidUtf8,
        }
    }
}

/// Events dispatched by `EtcdWatcher` to per-kind `EtcdResourceController`s.
///
/// Mirrors `FileSystemEvent` semantics:
/// - Init phase: `Init` → `InitApply` × N → `InitDone`
/// - Runtime phase: `Apply` / `Delete`
#[derive(Debug, Clone)]
pub enum EtcdEvent {
    /// Beginning of init phase for this kind.
    Init,

    /// One resource discovered during init (full-list) phase.
    InitApply {
        /// etcd key (e.g., "/edgion/conf/HTTPRoute/default/my-route")
        key: String,
        /// Decoded YAML or an explicit decoding failure
        content: EtcdDocument,
    },

    /// End of init phase — all existing resources for this kind have been sent.
    InitDone,

    /// A resource was created or updated (runtime watch event).
    Apply {
        /// etcd key
        key: String,
        /// Decoded YAML or an explicit decoding failure
        content: EtcdDocument,
    },

    /// A resource was deleted (runtime watch event).
    Delete {
        /// etcd key
        key: String,
    },
}
