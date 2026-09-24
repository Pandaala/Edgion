use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Shared continue-or-reject action for upstream-selection plugins.
///
/// Used by `onMissing` / `onNoMatch` / `onInvalid` on DirectEndpoint,
/// DynamicInternalUpstream, DynamicExternalUpstream, and Canary. The three
/// fields stay distinct — they are different trigger points that share this
/// action space. Per-field defaults stay on the fields, not on this enum:
/// `onMissing` and `onNoMatch` default to [`UpstreamFallback::Fallback`],
/// `onInvalid` defaults to [`UpstreamFallback::Reject`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum UpstreamFallback {
    /// Proceed with normal weighted / backend selection.
    Fallback,
    /// Reject the request (status is field-specific: 400 for onMissing /
    /// onNoMatch, 403 for onInvalid).
    Reject,
}

/// Default for `onMissing` / `onNoMatch`.
pub fn default_upstream_fallback() -> UpstreamFallback {
    UpstreamFallback::Fallback
}

/// Default for `onInvalid`.
pub fn default_upstream_reject() -> UpstreamFallback {
    UpstreamFallback::Reject
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/upstream_fallback_test/defaults_tests.rs"]
mod defaults_tests;
