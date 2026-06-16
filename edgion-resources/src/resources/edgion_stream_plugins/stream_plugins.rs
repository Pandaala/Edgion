//! Stream plugin types for EdgionStreamPlugins

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_data::{EdgionDataRef, GlobalConnectionIpRestrictionData};
use crate::resources::edgion_stream_plugins::connection_ip_restriction_rules::ConnectionIpRestrictionRules;
use crate::resources::edgion_stream_plugins::connection_rate_limit_config::ConnectionRateLimitConfig;

/// Stream plugin enum for all supported stream plugin types.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config")]
pub enum EdgionStreamPlugin {
    /// Inline IP restriction (L4-specific, no HTTP fields).
    IpRestriction(ConnectionIpRestrictionRules),

    /// L4 connection-phase IP restriction with overlay support.
    ///
    /// The `base` holds the complete, safe-default config (always present).
    /// The optional `overrideRef` points to a `GlobalConnIpRestrictionOverride`
    /// EdgionData that can hot-swap `enable`, `activeProfile`, or `profiles`.
    /// On any overlay error (absent / invalid / type-mismatch), the gateway
    /// falls back to `base` (fail-to-base) and emits a metric signal.
    GlobalConnectionIpRestriction(GlobalConnectionIpRestrictionEntry),

    /// L4 connection-rate limiter (local gate + optional Redis).
    ConnectionRateLimit(ConnectionRateLimitConfig),
}

/// Entry for the `GlobalConnectionIpRestriction` stream plugin.
///
/// The `base` field is the complete, safe-default configuration used directly
/// when no overlay is set, and as the fallback on any overlay error.
///
/// # Security invariant
/// `base` MUST be authored as the conservative/safe value — e.g.
/// `defaultAction: Deny` plus an explicit allowlist. Because fail-to-base
/// returns `base` on overlay errors, a permissive base (e.g.
/// `defaultAction: Allow` with no deny list) creates a wide-open fallback.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GlobalConnectionIpRestrictionEntry {
    /// Complete, safe-default IP restriction configuration.
    /// Used directly when `overrideRef` is absent, and as the fallback when
    /// overlay resolution fails (fail-to-base).
    pub base: GlobalConnectionIpRestrictionData,

    /// Optional reference to a `GlobalConnIpRestrictionOverride` EdgionData.
    /// Namespace defaults to the parent `EdgionStreamPlugins` namespace if omitted.
    /// When set, the effective config at connection time is `base ⊕ overlay`;
    /// overlay errors degrade to `base`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub override_ref: Option<EdgionDataRef>,
}

impl EdgionStreamPlugin {
    pub fn type_name(&self) -> &'static str {
        match self {
            EdgionStreamPlugin::IpRestriction(_) => "IpRestriction",
            EdgionStreamPlugin::GlobalConnectionIpRestriction(_) => "GlobalConnectionIpRestriction",
            EdgionStreamPlugin::ConnectionRateLimit(_) => "ConnectionRateLimit",
        }
    }
}

#[cfg(test)]
mod variant_tests {
    use super::*;

    #[test]
    fn connection_rate_limit_deserializes_with_tag_content_shape() {
        let yaml = r#"
type: ConnectionRateLimit
config:
  redisRef: "default/redis"
  perSourceIp:
    rate: 50
    interval: "1s"
"#;
        let p: EdgionStreamPlugin = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.type_name(), "ConnectionRateLimit");
    }

    #[test]
    fn gcir_entry_deserializes_base_without_override() {
        let yaml = r#"
type: GlobalConnectionIpRestriction
config:
  base:
    enable: true
    activeProfile: main
    profiles:
      main:
        defaultAction: deny
        allow:
          - name: office
            cidrs:
              - "10.0.0.0/8"
"#;
        let p: EdgionStreamPlugin = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.type_name(), "GlobalConnectionIpRestriction");
        if let EdgionStreamPlugin::GlobalConnectionIpRestriction(entry) = p {
            assert!(entry.base.enable);
            assert_eq!(entry.base.active_profile, "main");
            assert!(entry.override_ref.is_none());
        } else {
            panic!("expected GlobalConnectionIpRestriction variant");
        }
    }

    #[test]
    fn gcir_entry_deserializes_base_with_override_ref() {
        let yaml = r#"
type: GlobalConnectionIpRestriction
config:
  base:
    enable: true
    activeProfile: default
    profiles:
      default:
        defaultAction: deny
        allow:
          - name: loopback
            cidrs:
              - "127.0.0.0/8"
  overrideRef:
    name: gcir-override
    namespace: edgion-test
"#;
        let p: EdgionStreamPlugin = serde_yaml::from_str(yaml).unwrap();
        if let EdgionStreamPlugin::GlobalConnectionIpRestriction(entry) = p {
            assert!(entry.override_ref.is_some());
            let r = entry.override_ref.unwrap();
            assert_eq!(r.name, "gcir-override");
            assert_eq!(r.namespace.as_deref(), Some("edgion-test"));
        } else {
            panic!("expected GlobalConnectionIpRestriction variant");
        }
    }
}
