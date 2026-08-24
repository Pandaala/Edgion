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
mod defaults_tests {
    use super::*;
    use crate::resources::edgion_plugins::plugin_configs::{
        DirectEndpointConfig, DynamicExternalUpstreamConfig, DynamicInternalUpstreamConfig,
    };

    const DIRECT_ENDPOINT_MINIMAL: &str = r#"
from:
  type: header
  name: X-Target-Endpoint
"#;

    const DYN_UPSTREAM_MINIMAL: &str = r#"
from:
  type: header
  name: X-Backend-Target
"#;

    const EXT_UPSTREAM_MINIMAL: &str = r#"
from:
  type: header
  name: X-Target-Region
domainMap:
  us:
    domain: example.com
"#;

    #[test]
    fn direct_endpoint_on_missing_defaults_to_fallback() {
        let cfg: DirectEndpointConfig = serde_yaml::from_str(DIRECT_ENDPOINT_MINIMAL).unwrap();
        assert_eq!(cfg.on_missing, UpstreamFallback::Fallback);
    }

    #[test]
    fn direct_endpoint_on_invalid_defaults_to_reject() {
        let cfg: DirectEndpointConfig = serde_yaml::from_str(DIRECT_ENDPOINT_MINIMAL).unwrap();
        assert_eq!(cfg.on_invalid, UpstreamFallback::Reject);
    }

    #[test]
    fn dyn_upstream_on_missing_defaults_to_fallback() {
        let cfg: DynamicInternalUpstreamConfig = serde_yaml::from_str(DYN_UPSTREAM_MINIMAL).unwrap();
        assert_eq!(cfg.on_missing, UpstreamFallback::Fallback);
    }

    #[test]
    fn dyn_upstream_on_no_match_defaults_to_fallback() {
        let cfg: DynamicInternalUpstreamConfig = serde_yaml::from_str(DYN_UPSTREAM_MINIMAL).unwrap();
        assert_eq!(cfg.on_no_match, UpstreamFallback::Fallback);
    }

    #[test]
    fn dyn_upstream_on_invalid_defaults_to_reject() {
        let cfg: DynamicInternalUpstreamConfig = serde_yaml::from_str(DYN_UPSTREAM_MINIMAL).unwrap();
        assert_eq!(cfg.on_invalid, UpstreamFallback::Reject);
    }

    #[test]
    fn ext_upstream_on_missing_defaults_to_fallback() {
        let cfg: DynamicExternalUpstreamConfig = serde_yaml::from_str(EXT_UPSTREAM_MINIMAL).unwrap();
        assert_eq!(cfg.on_missing, UpstreamFallback::Fallback);
    }

    #[test]
    fn ext_upstream_on_no_match_defaults_to_fallback() {
        let cfg: DynamicExternalUpstreamConfig = serde_yaml::from_str(EXT_UPSTREAM_MINIMAL).unwrap();
        assert_eq!(cfg.on_no_match, UpstreamFallback::Fallback);
    }

    #[test]
    fn ext_upstream_skip_token_is_rejected() {
        let yaml = format!("{EXT_UPSTREAM_MINIMAL}onMissing: skip\n");
        let result: Result<DynamicExternalUpstreamConfig, _> = serde_yaml::from_str(&yaml);
        assert!(result.is_err(), "onMissing: skip must fail deserialize (no alias)");
    }
}
