//! Etcd event types for the ConfCenter watcher → resource controller pipeline.

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
        /// Raw YAML content
        content: String,
    },

    /// End of init phase — all existing resources for this kind have been sent.
    InitDone,

    /// A resource was created or updated (runtime watch event).
    Apply {
        /// etcd key
        key: String,
        /// Raw YAML content
        content: String,
    },

    /// A resource was deleted (runtime watch event).
    Delete {
        /// etcd key
        key: String,
    },
}
