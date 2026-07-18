//! Plugin policy marker types and canonical plugin-name helpers.
//!
//! `PluginPolicyAction` is stamped onto plugin entries by the controller when
//! the configured plugin policy matches; the gateway enforces it. The field is
//! `#[schemars(skip)]` on every entry (never operator-facing, never in the CRD
//! schema) but stays serde-serialized so conf_sync carries it to the data
//! plane. See docs/superpowers/specs/2026-07-17-controller-plugin-policy-design.md.

use std::collections::BTreeSet;

use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};

/// Action stamped by the controller on a policy-restricted plugin entry.
/// The gateway enforces it; the plugin itself is never instantiated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum PluginPolicyAction {
    /// Reject requests on chains containing this entry. `status` is the HTTP
    /// response code; stream stages ignore it and close the connection.
    Block { status: u16 },
    /// Skip the plugin; requests proceed and the skip is logged.
    Bypass,
}

/// The three plugin families addressable by a policy qualified name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginFamily {
    Http,
    Stream,
    TlsRoute,
}

impl PluginFamily {
    pub fn as_str(&self) -> &'static str {
        match self {
            PluginFamily::Http => "http",
            PluginFamily::Stream => "stream",
            PluginFamily::TlsRoute => "tls-route",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "http" => Some(PluginFamily::Http),
            "stream" => Some(PluginFamily::Stream),
            "tls-route" => Some(PluginFamily::TlsRoute),
            _ => None,
        }
    }
}

/// Parse a `<family>/<Type>` qualified plugin name. Exact, case-sensitive,
/// no wildcards.
pub fn parse_qualified_name(s: &str) -> Result<(PluginFamily, String), String> {
    let (family_str, type_name) = s
        .split_once('/')
        .ok_or_else(|| format!("invalid qualified plugin name {s:?}: expected <family>/<Type>"))?;
    let family = PluginFamily::parse(family_str)
        .ok_or_else(|| format!("invalid plugin family {family_str:?} in {s:?}: expected http, stream, or tls-route"))?;
    if type_name.is_empty() || type_name.contains('/') {
        return Err(format!("invalid plugin type in qualified name {s:?}"));
    }
    Ok((family, type_name.to_string()))
}

/// Extract the serde tag values (`properties.type.const` of each `oneOf`
/// branch) from a tagged-enum JSON schema. Same technique as
/// `edgion-resources/tests/plugin_crd_type_coverage.rs`.
fn tag_values(schema: serde_json::Value) -> BTreeSet<String> {
    schema
        .get("oneOf")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|b| {
            b.get("properties")
                .and_then(|p| p.get("type"))
                .and_then(|t| t.get("const"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// Canonical set of `type:` discriminator values for one plugin family,
/// derived from the enum's own schema — no hand-maintained list, no drift.
/// Cost is one `schema_for!` walk; callers cache the result (the controller
/// compiles the policy once at startup).
pub fn canonical_type_names(family: PluginFamily) -> BTreeSet<String> {
    let schema = match family {
        PluginFamily::Http => serde_json::to_value(schema_for!(crate::resources::edgion_plugins::EdgionPlugin)),
        PluginFamily::Stream => {
            serde_json::to_value(schema_for!(crate::resources::edgion_stream_plugins::EdgionStreamPlugin))
        }
        PluginFamily::TlsRoute => serde_json::to_value(schema_for!(
            crate::resources::edgion_stream_plugins::TlsRouteStreamPlugin
        )),
    }
    .expect("plugin enum schema serializes");
    let names = tag_values(schema);
    assert!(
        !names.is_empty(),
        "extracted zero type names for family {}; schemars output shape changed",
        family.as_str()
    );
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::edgion_plugins::RequestFilterEntry;

    #[test]
    fn policy_action_serde_round_trip() {
        let block = PluginPolicyAction::Block { status: 403 };
        let json = serde_json::to_string(&block).unwrap();
        assert_eq!(json, r#"{"action":"block","status":403}"#);
        let back: PluginPolicyAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, block);

        let bypass = PluginPolicyAction::Bypass;
        let json = serde_json::to_string(&bypass).unwrap();
        assert_eq!(json, r#"{"action":"bypass"}"#);
    }

    #[test]
    fn entry_policy_action_is_skipped_in_schema_but_serialized() {
        // Not in the JSON schema (CRD invariant: controller-resolved field).
        let schema = serde_json::to_value(schemars::schema_for!(RequestFilterEntry)).unwrap();
        let props = schema.get("properties").expect("entry schema has properties");
        assert!(
            props.get("policyAction").is_none(),
            "policyAction must be #[schemars(skip)]"
        );

        // But serde-serialized when set (conf_sync carriage).
        // `Mock`'s config fields are all optional/defaulted; an empty config
        // is a real, minimal, valid `MockConfig`.
        let yaml = "type: Mock\nconfig: {}";
        let mut entry: RequestFilterEntry = serde_yaml::from_str(yaml).unwrap();
        entry.policy_action = Some(PluginPolicyAction::Bypass);
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["policyAction"]["action"], "bypass");

        // And absent from JSON when None (wire compatibility).
        entry.policy_action = None;
        let json = serde_json::to_value(&entry).unwrap();
        assert!(json.get("policyAction").is_none());
    }

    #[test]
    fn parse_qualified_name_accepts_all_families() {
        assert_eq!(
            parse_qualified_name("http/Wasm").unwrap(),
            (PluginFamily::Http, "Wasm".to_string())
        );
        assert_eq!(
            parse_qualified_name("stream/ConnectionRateLimit").unwrap(),
            (PluginFamily::Stream, "ConnectionRateLimit".to_string())
        );
        assert_eq!(
            parse_qualified_name("tls-route/IpRestriction").unwrap(),
            (PluginFamily::TlsRoute, "IpRestriction".to_string())
        );
    }

    #[test]
    fn parse_qualified_name_rejects_bad_input() {
        for bad in ["Wasm", "http/", "/Wasm", "grpc/Wasm", "http/Wasm/extra", ""] {
            assert!(parse_qualified_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn canonical_type_names_extracts_all_three_families() {
        let http = canonical_type_names(PluginFamily::Http);
        assert!(http.contains("Wasm") && http.contains("RequestHeaderModifier"));
        assert!(http.len() >= 30, "http family too small: {}", http.len());

        let stream = canonical_type_names(PluginFamily::Stream);
        assert!(stream.contains("ConnectionRateLimit"));
        assert!(!stream.is_empty());

        let tls_route = canonical_type_names(PluginFamily::TlsRoute);
        assert!(tls_route.contains("IpRestriction"));
        assert!(!tls_route.is_empty());
    }
}
