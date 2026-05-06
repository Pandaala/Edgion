//! Stateless plugin validation entry point.
//!
//! This trait provides the canonical, side-effect-free way to surface
//! plugin configuration errors. Implementations MUST be safe to call
//! repeatedly without mutating any state.
//!
//! Used by the controller preparse path (via `EdgionPlugin::validation_error`)
//! to report invalid plugin configs through K8s status conditions before any
//! gateway-side construction occurs.

/// Stateless validation entry point for plugin configs.
pub trait PluginValidation {
    /// Returns `Some(error)` if the config is invalid; `None` otherwise.
    ///
    /// Errors should be human-readable and suitable for K8s status conditions.
    /// Implementations MUST NOT mutate `self` or perform any I/O.
    fn validation_error(&self) -> Option<String>;
}
