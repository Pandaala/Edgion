//! Compiled entry-level dye plan.
//!
//! `PluginEntry::dye` (schema-level `DyeSuite`) is compiled once, at
//! `PluginRuntime` build time, into a `CompiledDyePlan` carried by the entry's
//! conditional wrapper. Validation (`DyeSuite::validation_error`) already ran
//! on ingest and guarantees every template parses, every name is a valid
//! non-denied header token, and no Dye value template references `secretRef`
//! (conditions retain their normal reference support) — so a
//! `CompiledTemplate::compile` failure here is a bug, not an operator error.
//! It is still surfaced through the normal compile-error vec (fail-closed,
//! matching every other `PluginRuntime` compile error), never a panic.
//!
//! See `skills/01-architecture/02-gateway/entry-level-dye.md`.

use std::sync::Arc;

use edgion_resources::resources::edgion_plugins::{CompiledTemplate, DyeOutcome, DyeRule, DyeSuite};

use super::conditions::CompiledPluginConditions;

/// One compiled dye rule: which outcomes render (`on`), the pre-compiled
/// operator value template (`None` => render the plugin-recorded outcome
/// value), and an optional render gate (never gates the anti-spoof strip).
pub struct CompiledDyeRule {
    pub name: String,
    pub on: Vec<DyeOutcome>,
    /// Compiled operator value template; None => recorded value.
    pub value: Option<CompiledTemplate>,
    pub(crate) conditions: Option<CompiledPluginConditions>,
}

impl CompiledDyeRule {
    fn compile(rule: &DyeRule) -> Result<Self, String> {
        let value = match &rule.value {
            Some(v) => Some(
                CompiledTemplate::compile(v)
                    .map_err(|e| format!("dye rule '{}': value template error: {}", rule.name, e))?,
            ),
            None => None,
        };
        Ok(Self {
            name: rule.name.clone(),
            on: rule.on.clone(),
            value,
            conditions: rule.conditions.clone().map(CompiledPluginConditions::new),
        })
    }
}

/// Compiled entry-level dye plan, split by direction so the request-stage
/// anti-spoof strip only walks `request`; outcome recording renders each
/// direction through the canonical session adapter.
pub struct CompiledDyePlan {
    /// Rules rendered onto the upstream request (success outcomes only).
    pub request: Vec<CompiledDyeRule>,
    /// Rules rendered onto the downstream response.
    pub response: Vec<CompiledDyeRule>,
    /// Stable snake_case value prefix for this entry's plugin type
    /// (`EdgionPlugin::dye_type_prefix`). Validation only permits a dye
    /// suite on a participating (non-empty-prefix) plugin type, so this is
    /// never the empty string in a plan that actually reaches the loops.
    pub type_prefix: &'static str,
}

impl CompiledDyePlan {
    /// Compile an entry's suite. Validation already ran; a template that
    /// fails to compile here is a bug — surface via the compile error vec.
    pub fn compile(suite: &DyeSuite, type_prefix: &'static str) -> Result<Arc<Self>, String> {
        let request = suite
            .request
            .iter()
            .map(CompiledDyeRule::compile)
            .collect::<Result<Vec<_>, _>>()?;
        let response = suite
            .response
            .iter()
            .map(CompiledDyeRule::compile)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Arc::new(Self {
            request,
            response,
            type_prefix,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dye_producer_inventory_matches_declared_capability_union() {
        use edgion_resources::resources::edgion_plugins::{DyeStage, EdgionPlugin};
        use std::collections::BTreeSet;
        // Actual producer source is independent of the schema capability table.
        // Policy/stage cases and executable producer tests supply the branch
        // evidence; this guard catches newly orphaned/undeclared outcome wiring.
        let cases: &[(&str, &[&str], &[&str])] = &[
            (
                "Canary",
                &[
                    r#"{"enable":true,"activeProfile":"s","profiles":{"s":{"name":"app"}},"onInvalid":"reject"}"#,
                    r#"{"enable":true,"activeProfile":"s","profiles":{"s":{"name":"app"}},"onInvalid":"fallback"}"#,
                ],
                &[include_str!("../http/canary/plugin.rs")],
            ),
            (
                "DynamicInternalUpstream",
                &[r#"{"from":{"type":"header","name":"X-Key"}}"#],
                &[include_str!("../http/dynamic_internal_upstream/plugin.rs")],
            ),
            (
                "DynamicExternalUpstream",
                &[
                    r#"{"from":{"type":"header","name":"X-Key"},"domainMap":{"a":{"domain":"example.com"}},"onMissing":"reject"}"#,
                ],
                &[include_str!("../http/dynamic_external_upstream/plugin.rs")],
            ),
            (
                "DirectEndpoint",
                &[r#"{"from":{"type":"header","name":"X-Key"}}"#],
                &[include_str!("../http/direct_endpoint/plugin.rs")],
            ),
            (
                "RegionRoute",
                &[
                    r#"{"myRegion":"a","routeRules":[{"type":"RouteByDirectRegion","keyFrom":[{"type":"header","name":"X-Region"}]}],"overrideRef":{"name":"regions"}}"#,
                ],
                &[include_str!("../http/region_route/plugin.rs")],
            ),
            (
                "RateLimitRedis",
                &[
                    r#"{"redisRef":"default/redis","policies":[{"rate":10,"interval":"1s"}],"onRedisFailure":"Allow"}"#,
                    r#"{"redisRef":"default/redis","policies":[{"rate":10,"interval":"1s"}],"onRedisFailure":"Deny"}"#,
                ],
                &[include_str!("../http/rate_limit_redis/plugin.rs")],
            ),
            (
                "ExtProc",
                &[
                    r#"{"grpcService":{"target":"http://example.com:9000"},"failureModeAllow":true}"#,
                    r#"{"grpcService":{"target":"http://example.com:9000"},"failureModeAllow":false}"#,
                ],
                &[include_str!("../http/ext_proc/plugin.rs")],
            ),
            (
                "Wasm",
                &[
                    r#"{"source":{"file":"plugin.wasm"},"failOpen":true}"#,
                    r#"{"source":{"file":"plugin.wasm"},"failOpen":false}"#,
                ],
                &[include_str!("../http/wasm/plugin.rs")],
            ),
            (
                "Dsl",
                &[r#"{"name":"d","source":"return next()"}"#],
                &[include_str!("../http/dsl/lang/vm.rs")],
            ),
            (
                "OpenidConnect",
                &[r#"{"discovery":"https://example.com/discovery","clientId":"client"}"#],
                &[include_str!("../http/openid_connect/discovery_jwks.rs")],
            ),
            (
                "ForwardAuth",
                &[r#"{"target":{"name":"auth","port":80},"decision":{}}"#],
                &[include_str!("../http/forward_auth/plugin.rs")],
            ),
            (
                "LdapAuth",
                &[r#"{"ldapHost":"ldap.example.com","baseDn":"dc=example,dc=com","attribute":"uid"}"#],
                &[include_str!("../http/ldap_auth/plugin.rs")],
            ),
            ("AiProxy", &[r#"{}"#], &[include_str!("../http/ai_proxy/plugin.rs")]),
        ];
        let producer = regex::Regex::new(r"\.record_outcome\(\s*DyeOutcome::(\w+)").unwrap();
        for (kind, configs, sources) in cases {
            let mut declared = BTreeSet::new();
            for config in *configs {
                let config: serde_json::Value = serde_json::from_str(config).unwrap();
                let plugin: EdgionPlugin = serde_json::from_value(serde_json::json!({"type":kind,"config":config}))
                    .unwrap_or_else(|e| panic!("{kind}: {e}"));
                for stage in [
                    DyeStage::Request,
                    DyeStage::UpstreamResponseFilter,
                    DyeStage::UpstreamResponseBodyFilter,
                    DyeStage::UpstreamResponse,
                ] {
                    for outcome in plugin.dye_capabilities(stage) {
                        declared.insert(format!("{outcome:?}"));
                    }
                }
            }
            let recorded: BTreeSet<_> = sources
                .iter()
                .flat_map(|source| {
                    let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
                    producer
                        .captures_iter(production)
                        .map(|c| c[1].to_string())
                        .collect::<Vec<_>>()
                })
                .collect();
            assert_eq!(declared, recorded, "{kind}: declared/producer dye drift");
        }
        for source in [
            include_str!("../http/ext_proc/response_streamed.rs"),
            include_str!("../http/ext_proc/response_trailer.rs"),
        ] {
            assert!(
                !source
                    .split("#[cfg(test)]\nmod tests")
                    .next()
                    .unwrap()
                    .contains(".record_outcome("),
                "committed response paths cannot produce header dye"
            );
        }
    }

    #[test]
    fn compile_plan_splits_directions_and_compiles_templates() {
        let suite: DyeSuite = serde_yaml::from_str(
            "request:\n  - name: X-T\nresponse:\n  - name: X-O\n    on: [fail-open]\n    value: 'v ${header:X-Id}'\n",
        )
        .unwrap();
        let plan = CompiledDyePlan::compile(&suite, "canary").unwrap();
        assert_eq!(plan.request.len(), 1);
        assert_eq!(plan.response.len(), 1);
        assert!(plan.response[0].value.is_some());
        assert_eq!(plan.type_prefix, "canary");
    }

    #[test]
    fn compile_plan_leaves_omitted_value_as_recorded_value() {
        let suite: DyeSuite = serde_yaml::from_str("response:\n  - name: X-O\n").unwrap();
        let plan = CompiledDyePlan::compile(&suite, "canary").unwrap();
        assert_eq!(plan.response.len(), 1);
        assert!(plan.response[0].value.is_none());
        assert_eq!(plan.response[0].on, vec![DyeOutcome::Success]);
    }

    #[test]
    fn compile_plan_carries_rule_conditions() {
        let suite: DyeSuite = serde_yaml::from_str(
            "response:\n  - name: X-O\n    conditions:\n      run:\n        allOf:\n          - type: keyExist\n            key:\n              type: header\n              name: X-Debug\n",
        )
        .unwrap();
        let plan = CompiledDyePlan::compile(&suite, "canary").unwrap();
        assert!(plan.response[0].conditions.is_some());
    }
}
