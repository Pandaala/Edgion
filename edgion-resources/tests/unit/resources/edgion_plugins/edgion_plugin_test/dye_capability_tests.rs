use super::*;
use crate::resources::edgion_plugins::dye::DyeOutcome;

const STAGES: [DyeStage; 4] = [
    DyeStage::Request,
    DyeStage::UpstreamResponseFilter,
    DyeStage::UpstreamResponseBodyFilter,
    DyeStage::UpstreamResponse,
];

#[test]
fn dependency_dye_capabilities_follow_effective_policy_and_stage() {
    use super::super::plugin_configs::OnRedisFailure;
    for allow in [false, true] {
        let expected = [if allow {
            DyeOutcome::FailOpen
        } else {
            DyeOutcome::FailClose
        }];
        let wasm = EdgionPlugin::Wasm(WasmConfig {
            fail_open: allow,
            ..Default::default()
        });
        let ext = EdgionPlugin::ExtProc(ExtProcConfig {
            failure_mode_allow: allow,
            ..Default::default()
        });
        let redis = EdgionPlugin::RateLimitRedis(RateLimitRedisConfig {
            on_redis_failure: if allow {
                OnRedisFailure::Allow
            } else {
                OnRedisFailure::Deny
            },
            ..Default::default()
        });
        for stage in STAGES {
            assert_eq!(
                wasm.dye_capabilities(stage),
                if matches!(stage, DyeStage::Request | DyeStage::UpstreamResponseFilter) {
                    &expected[..]
                } else {
                    &[]
                }
            );
            for plugin in [&ext, &redis] {
                assert_eq!(
                    plugin.dye_capabilities(stage),
                    if stage == DyeStage::Request { &expected[..] } else { &[] }
                );
            }
        }
    }
}

#[test]
fn selection_dye_excludes_inactive_configuration_branches() {
    use super::super::UpstreamFallback::{Fallback, Reject};
    for missing in [Fallback, Reject] {
        for invalid in [Fallback, Reject] {
            for no_match in [Fallback, Reject] {
                let mut internal = DynamicInternalUpstreamConfig {
                    on_missing: missing,
                    on_invalid: invalid,
                    on_no_match: no_match,
                    ..Default::default()
                };
                let plugin = EdgionPlugin::DynamicInternalUpstream(internal.clone());
                assert_eq!(
                    plugin
                        .dye_capabilities(DyeStage::Request)
                        .contains(&DyeOutcome::Fallback),
                    missing == Fallback || invalid == Fallback
                );
                assert_eq!(
                    plugin.dye_capabilities(DyeStage::Request).contains(&DyeOutcome::Reject),
                    missing == Reject || invalid == Reject
                );
                internal.rules = Some(
                    serde_json::from_value(serde_json::json!([
                        {"value":"key","backendRef":{"name":"app"}}
                    ]))
                    .unwrap(),
                );
                // A rules table enables on-no-match; direct mode does not.
                assert!(EdgionPlugin::DynamicInternalUpstream(internal)
                    .dye_capabilities(DyeStage::Request)
                    .contains(&if no_match == Fallback {
                        DyeOutcome::Fallback
                    } else {
                        DyeOutcome::Reject
                    }));
            }
            let direct = EdgionPlugin::DirectEndpoint(DirectEndpointConfig {
                on_missing: missing,
                on_invalid: invalid,
                ..Default::default()
            });
            assert_eq!(
                direct
                    .dye_capabilities(DyeStage::Request)
                    .contains(&DyeOutcome::Fallback),
                missing == Fallback || invalid == Fallback
            );
            assert_eq!(
                direct.dye_capabilities(DyeStage::Request).contains(&DyeOutcome::Reject),
                missing == Reject || invalid == Reject
            );
            for stage in &STAGES[1..] {
                assert!(direct.dye_capabilities(*stage).is_empty());
            }
        }
    }
}

#[test]
fn oidc_nonce_callback_keeps_jwks_dye_independent_of_bearer_mode() {
    use super::super::VerificationMode;
    for mode in [
        VerificationMode::Auto,
        VerificationMode::JwksOnly,
        VerificationMode::IntrospectionOnly,
    ] {
        for use_jwks in [false, true] {
            for use_nonce in [false, true] {
                let expected =
                    use_nonce || mode == VerificationMode::JwksOnly || (mode == VerificationMode::Auto && use_jwks);
                for bearer_only in [false, true] {
                    let plugin = EdgionPlugin::OpenidConnect(Box::new(OpenidConnectConfig {
                        verification_mode: mode,
                        use_jwks,
                        use_nonce,
                        bearer_only,
                        renew_access_token_on_expiry: false,
                        ..Default::default()
                    }));
                    assert_eq!(
                        plugin.dye_capabilities(DyeStage::Request),
                        if expected { DYE_FAIL_BOTH } else { &[] }
                    );
                    for stage in &STAGES[1..] {
                        assert!(plugin.dye_capabilities(*stage).is_empty());
                    }
                }
            }
        }
    }
}

#[test]
fn oidc_session_refresh_dye_requires_session_mode_and_renewal() {
    use super::super::{RequestFilterEntry, VerificationMode};
    for mode in [VerificationMode::IntrospectionOnly, VerificationMode::Auto] {
        for (bearer_only, renew, supported) in [
            (false, true, true),
            (false, false, false),
            (true, true, false),
            (true, false, false),
        ] {
            let plugin = EdgionPlugin::OpenidConnect(Box::new(OpenidConnectConfig {
                verification_mode: mode,
                use_jwks: false,
                use_nonce: false,
                bearer_only,
                renew_access_token_on_expiry: renew,
                ..Default::default()
            }));
            let mut entry = RequestFilterEntry::new(plugin);
            entry.dye = Some(
                serde_json::from_value(serde_json::json!({
                    "response": [{"name": "X-OIDC-Degraded", "on": ["fail-open", "fail-close"]}]
                }))
                .unwrap(),
            );
            assert_eq!(
                entry.validate_dye().is_none(),
                supported,
                "{mode:?}/{bearer_only}/{renew}"
            );
            assert_eq!(
                entry.plugin.dye_capabilities(DyeStage::Request),
                if supported { DYE_FAIL_BOTH } else { &[] }
            );
            for stage in &STAGES[1..] {
                assert!(entry.plugin.dye_capabilities(*stage).is_empty());
            }
        }
    }
}

#[test]
fn dsl_ignore_retains_hard_error_capability_in_request_and_async_response_stages() {
    use super::super::plugin_configs::DslErrorPolicy;
    for policy in [
        None,
        Some(DslErrorPolicy::Ignore),
        Some(DslErrorPolicy::Deny),
        Some(DslErrorPolicy::DenyWith {
            status: 503,
            body: None,
        }),
    ] {
        let expected = if matches!(policy, None | Some(DslErrorPolicy::Ignore)) {
            DYE_FAIL_BOTH
        } else {
            DYE_FAIL_CLOSE_ONLY
        };
        let plugin = EdgionPlugin::Dsl(DslConfig {
            error_policy: policy,
            ..Default::default()
        });
        for stage in STAGES {
            assert_eq!(
                plugin.dye_capabilities(stage),
                if matches!(stage, DyeStage::Request | DyeStage::UpstreamResponse) {
                    expected
                } else {
                    &[]
                }
            );
        }
    }
}

#[test]
fn selection_plugins_declare_success() {
    use crate::resources::plugin_configs::{CanaryConfig, DynUpstreamTarget, UpstreamFallback};
    use std::collections::HashMap;

    // CanaryConfig has no `Default` impl; construct directly (mirrors the
    // existing `valid_config()` helper in canary.rs's own tests).
    let cfg = CanaryConfig {
        enable: true,
        active_profile: "stable".to_string(),
        profiles: HashMap::from([(
            "stable".to_string(),
            DynUpstreamTarget {
                name: "app-v1".to_string(),
                namespace: None,
            },
        )]),
        active_profile_ref: None,
        on_invalid: UpstreamFallback::Reject,
    };
    let p = EdgionPlugin::Canary(cfg);
    assert!(p.dye_capabilities(DyeStage::Request).contains(&DyeOutcome::Success));
    assert!(!p.dye_capabilities(DyeStage::Request).contains(&DyeOutcome::Fallback));
    assert!(p.dye_capabilities(DyeStage::Request).contains(&DyeOutcome::Reject));
    assert_eq!(p.dye_type_prefix(), "canary");
}

#[test]
fn unwired_plugin_declares_nothing() {
    use crate::resources::plugin_configs::CorsConfig;
    let p = EdgionPlugin::Cors(CorsConfig::default());
    assert!(p.dye_capabilities(DyeStage::Request).is_empty());
}

#[test]
fn type_prefix_is_snake_case_of_type_name() {
    use crate::resources::plugin_configs::RateLimitRedisConfig;
    let p = EdgionPlugin::RateLimitRedis(RateLimitRedisConfig::default());
    assert_eq!(p.dye_type_prefix(), "rate_limit_redis");
    assert!(p.dye_capabilities(DyeStage::Request).contains(&DyeOutcome::FailOpen));
}

#[test]
fn key_header_names_mirrors_single_from_field_plugins() {
    use crate::resources::plugin_configs::{
        DirectEndpointConfig, DynamicExternalUpstreamConfig, DynamicInternalUpstreamConfig,
    };

    // `KeyGet` tags are `#[serde(tag = "type", rename_all = "camelCase")]`,
    // so the `Header` variant renders as lowercase `header`.
    let direct: DirectEndpointConfig =
        yaml_serde::from_str("from:\n  type: header\n  name: X-Backend-Target\n").unwrap();
    assert_eq!(
        EdgionPlugin::DirectEndpoint(direct).key_header_names(),
        vec!["X-Backend-Target".to_string()]
    );

    let dyn_internal: DynamicInternalUpstreamConfig =
        yaml_serde::from_str("from:\n  type: header\n  name: X-Upstream\n").unwrap();
    assert_eq!(
        EdgionPlugin::DynamicInternalUpstream(dyn_internal).key_header_names(),
        vec!["X-Upstream".to_string()]
    );

    let dyn_external: DynamicExternalUpstreamConfig =
        yaml_serde::from_str("from:\n  type: header\n  name: X-External\ndomainMap: {}\n").unwrap();
    assert_eq!(
        EdgionPlugin::DynamicExternalUpstream(dyn_external).key_header_names(),
        vec!["X-External".to_string()]
    );

    // Non-header key source contributes nothing.
    let direct_ip: DirectEndpointConfig = yaml_serde::from_str("from:\n  type: remoteIp\n").unwrap();
    assert!(EdgionPlugin::DirectEndpoint(direct_ip).key_header_names().is_empty());
}

#[test]
fn key_header_names_collects_all_region_route_sources() {
    use crate::resources::plugin_configs::RegionRouteConfig;

    let cfg: RegionRouteConfig = yaml_serde::from_str(
        "clusterRegionRef: {name: local}\n\
             keyGet:\n  - type: header\n    name: X-Key\n\
             hashKeyGet:\n  - type: header\n    name: X-Hash\n\
             routeRules:\n  - type: RouteByDirectRegion\n    keyFrom:\n      - type: header\n        name: X-Region\n\
             ",
    )
    .unwrap();
    let mut names = EdgionPlugin::RegionRoute(cfg).key_header_names();
    names.sort();
    assert_eq!(
        names,
        vec!["X-Hash".to_string(), "X-Key".to_string(), "X-Region".to_string()]
    );
    assert_eq!(
        EdgionPlugin::RegionRoute(RegionRouteConfig::default()).dye_capabilities(DyeStage::Request),
        &[DyeOutcome::Fallback]
    );
}
