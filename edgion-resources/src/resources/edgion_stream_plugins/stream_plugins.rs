//! Stream plugin types for EdgionStreamPlugins

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::plugin_configs::GeoIpLocationConfig;
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

    /// L4 connection-phase geo resolution/restriction. Same config shape as
    /// the HTTP `GeoIpLocation` (`edgion_plugins::plugin_configs::GeoIpLocationConfig`);
    /// only `DirectPeerIp`-sourced rules are legal here (no XFF at L4).
    GeoIpLocation(GeoIpLocationConfig),
    /// Non-executable marker; source validation rejects even disabled entries.
    #[serde(
        other,
        deserialize_with = "crate::resources::common::discard_unsupported_plugin_config"
    )]
    #[schemars(skip)]
    Unsupported,
}

impl EdgionStreamPlugin {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Unsupported => "Unsupported",
            EdgionStreamPlugin::IpRestriction(_) => "IpRestriction",
            EdgionStreamPlugin::GlobalConnectionIpRestriction(_) => "GlobalConnectionIpRestriction",
            EdgionStreamPlugin::ConnectionRateLimit(_) => "ConnectionRateLimit",
            EdgionStreamPlugin::GeoIpLocation(_) => "GeoIpLocation",
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_stream_plugins/stream_plugins_test/variant_tests.rs"]
mod variant_tests;
