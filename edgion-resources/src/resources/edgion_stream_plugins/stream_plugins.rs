//! Stream plugin types for EdgionStreamPlugins

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_stream_plugins::connection_ip_restriction_rules::ConnectionIpRestrictionRules;
use crate::resources::edgion_stream_plugins::connection_rate_limit_config::ConnectionRateLimitConfig;
use crate::resources::edgion_stream_plugins::global_connection_ip_restriction::GlobalConnectionIpRestrictionConfig;

/// Stream plugin enum for all supported stream plugin types.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "config")]
pub enum EdgionStreamPlugin {
    /// Inline IP restriction (L4-specific, no HTTP fields).
    IpRestriction(ConnectionIpRestrictionRules),

    /// L4 connection-phase IP restriction.
    ///
    /// Carries the complete, git-owned IP config inline (`enable` + named `profiles`). The
    /// only dynamic input is *which profile is active*, supplied via an optional
    /// `activeProfileRef` pointing at a `Selector` EdgionConfigData. On any selector error
    /// (absent / type-mismatch / unknown profile / envelope-disabled), the gateway falls
    /// back to `activeProfile` (fail-to-base) and emits a metric signal.
    GlobalConnectionIpRestriction(GlobalConnectionIpRestrictionConfig),

    /// L4 connection-rate limiter (local gate + optional Redis).
    ConnectionRateLimit(ConnectionRateLimitConfig),
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
    fn gcir_deserializes_flat_config_without_active_profile_ref() {
        let yaml = r#"
type: GlobalConnectionIpRestriction
config:
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
        if let EdgionStreamPlugin::GlobalConnectionIpRestriction(cfg) = p {
            assert!(cfg.enable);
            assert_eq!(cfg.active_profile, "main");
            assert!(cfg.active_profile_ref.is_none());
        } else {
            panic!("expected GlobalConnectionIpRestriction variant");
        }
    }

    #[test]
    fn gcir_deserializes_flat_config_with_active_profile_ref() {
        let yaml = r#"
type: GlobalConnectionIpRestriction
config:
  enable: true
  activeProfile: default
  profiles:
    default:
      defaultAction: deny
      allow:
        - name: loopback
          cidrs:
            - "127.0.0.0/8"
  activeProfileRef:
    name: gcir-active
    namespace: edgion-test
"#;
        let p: EdgionStreamPlugin = serde_yaml::from_str(yaml).unwrap();
        if let EdgionStreamPlugin::GlobalConnectionIpRestriction(cfg) = p {
            assert!(cfg.active_profile_ref.is_some());
            let r = cfg.active_profile_ref.unwrap();
            assert_eq!(r.name, "gcir-active");
            assert_eq!(r.namespace.as_deref(), Some("edgion-test"));
        } else {
            panic!("expected GlobalConnectionIpRestriction variant");
        }
    }
}
