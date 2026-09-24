//! RequestMirror plugin configuration (EdgionPlugins path).
//!
//! The EdgionPlugins CRD preserves unknown fields, so unlike the HTTPRoute
//! filter path (where the vendored Gateway API CRD prunes undeclared fields
//! and tuning must travel via `edgion.io/mirror-*` annotations), this config
//! embeds the [`MirrorTuning`] knobs inline. Both flattened parts keep the
//! historical wire shape: standard fields and tuning fields sit at the same
//! level of the `config` object.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::PluginValidation;
use crate::resources::http_route::{HTTPRequestMirrorFilter, MirrorTuning};

/// RequestMirror plugin config: Gateway API standard mirror fields plus the
/// Edgion tuning knobs, flattened into a single object.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RequestMirrorPluginConfig {
    /// Gateway API standard fields (backendRef / fraction / percent).
    #[serde(flatten)]
    pub filter: HTTPRequestMirrorFilter,

    /// Edgion tuning knobs (timeouts, buffering, mirror log, concurrency).
    #[serde(flatten)]
    pub tuning: MirrorTuning,
}

impl PluginValidation for RequestMirrorPluginConfig {
    fn validation_error(&self) -> Option<String> {
        self.filter
            .validation_error()
            .or_else(|| self.tuning.resolve_limits().err())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/resources/edgion_plugins/plugin_configs/request_mirror_test.rs"]
mod tests;
