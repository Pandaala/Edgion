use crate::resources::edgion_plugins::dye::{DyeOutcome, DyeRule, DyeSuite};
use crate::resources::edgion_plugins::{
    EdgionPlugin, EdgionPlugins, EdgionPluginsSpec, RequestFilterEntry, UpstreamResponseEntry,
};

fn with_request_plugins(entries: Vec<RequestFilterEntry>) -> EdgionPlugins {
    EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(entries),
            ..Default::default()
        },
        status: None,
    }
}

fn header_filter() -> crate::resources::http_route::HTTPHeaderFilter {
    crate::resources::http_route::HTTPHeaderFilter {
        set: None,
        add: None,
        remove: None,
    }
}

fn dye_response(name: &str, on: &[DyeOutcome]) -> DyeSuite {
    DyeSuite {
        request: vec![],
        response: vec![DyeRule {
            name: name.to_string(),
            on: on.to_vec(),
            value: None,
            conditions: None,
        }],
    }
}

#[test]
fn invalid_dye_is_reported_and_does_not_skip_stage_check() {
    // RequestHeaderModifier declares no dye capabilities, so a default
    // `[success]` dye rule is rejected — mirrors the alias parity test
    // above, with no spurious stage error alongside it.
    let mut entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(header_filter()));
    entry.dye = Some(dye_response("X-Dye", &[DyeOutcome::Success]));
    let ep = with_request_plugins(vec![entry]);
    let errs = ep.validate_plugins();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(
        errs[0].contains("cannot render in this stage and configuration"),
        "{errs:?}"
    );
}

#[test]
fn no_dye_produces_no_error() {
    let entry = RequestFilterEntry::new(EdgionPlugin::RequestHeaderModifier(header_filter()));
    let ep = with_request_plugins(vec![entry]);
    assert!(ep.validate_plugins().is_empty());
}

#[test]
fn set_clobber_collision_within_one_array_detected_by_validate_plugins() {
    let success_entry = valid_selection_dye_entry("X-Dye", DyeOutcome::Success);
    let fallback_entry = valid_selection_dye_entry("x-dye", DyeOutcome::Fallback);

    let ep = with_request_plugins(vec![success_entry, fallback_entry]);
    let errs = ep.validate_plugins();
    assert_eq!(errs.len(), 1, "only the collision should fail: {errs:?}");
    assert!(
        errs.iter().any(|e| e.contains("dye: response header")),
        "expected a set-clobber error, got: {errs:?}"
    );
}

#[test]
fn set_clobber_collision_across_stage_arrays_detected_by_validate_plugins() {
    // Success rule lives in requestPlugins, the colliding failure rule in
    // upstreamResponsePlugins — the clobber check runs once per
    // resource over all four stage arrays chained, not per-array.
    let success_entry = valid_selection_dye_entry("X-Dye", DyeOutcome::Success);

    let mut fallback_entry = UpstreamResponseEntry::new(
        serde_json::from_value(serde_json::json!({
            "type": "Dsl",
            "config": {"name": "response-dye", "source": "return next()", "errorPolicy": "ignore"}
        }))
        .unwrap(),
    );
    fallback_entry.dye = Some(dye_response("x-dye", &[DyeOutcome::FailOpen]));
    assert_eq!(fallback_entry.validate_dye(), None);
    let standalone = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            upstream_response_plugins: Some(vec![fallback_entry.clone()]),
            ..Default::default()
        },
        status: None,
    };
    assert!(standalone.validate_plugins().is_empty());

    let ep = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![success_entry]),
            upstream_response_plugins: Some(vec![fallback_entry]),
            ..Default::default()
        },
        status: None,
    };
    let errs = ep.validate_plugins();
    assert_eq!(errs.len(), 1, "only the cross-stage collision should fail: {errs:?}");
    assert!(
        errs.iter().any(|e| e.contains("dye: response header")),
        "expected a cross-array set-clobber error, got: {errs:?}"
    );
}

#[test]
fn set_clobber_ignores_disabled_entries() {
    // A disabled entry's dye is dormant — must not trip the resource-level
    // clobber check, matching the disabled-entry-is-inert rule used
    // throughout `validate_plugins`.
    let success_entry = valid_selection_dye_entry("X-Dye", DyeOutcome::Success);

    let mut fallback_entry = valid_selection_dye_entry("x-dye", DyeOutcome::Fallback);
    fallback_entry.enable = false;

    let ep = with_request_plugins(vec![success_entry, fallback_entry]);
    let errs = ep.validate_plugins();
    assert!(
        errs.is_empty(),
        "disabled entry's dye must not trigger a clobber error, got: {errs:?}"
    );
}

fn valid_selection_dye_entry(name: &str, outcome: DyeOutcome) -> RequestFilterEntry {
    let mut entry = RequestFilterEntry::new(
        serde_json::from_value(serde_json::json!({
            "type": "Canary",
            "config": {
                "enable": true,
                "activeProfile": "stable",
                "profiles": {"stable": {"name": "app"}},
                "onInvalid": "fallback"
            }
        }))
        .unwrap(),
    );
    entry.dye = Some(dye_response(name, &[outcome]));
    assert_eq!(entry.validate_dye(), None);
    let errors = with_request_plugins(vec![entry.clone()]).validate_plugins();
    assert!(errors.is_empty(), "collision inputs must be valid alone: {errors:?}");
    entry
}

#[test]
fn graphql_root_quota_header_is_protected_from_request_dye_stripping() {
    let config=serde_json::from_value(serde_json::json!({
        "schema":"type Query {ok:Int}","redisRef":"default/redis",
        "policies":[{"rate":100,"interval":"1m"}],"onRedisFailure":"Allow",
        "rootQuotas":[{"name":"root","field":"Query.ok","key":[{"type":"header","name":"X-Root"}],"policies":[{"rate":10,"interval":"1m"}]}]
    })).unwrap();
    let mut entry = RequestFilterEntry::new(EdgionPlugin::GraphqlRateLimit(config));
    let headers = entry.plugin.key_header_names();
    assert_eq!(headers, vec!["X-Root".to_string()]);
    // Verify the canonical collision mechanism with its supported request outcome.
    // GraphQL itself currently supports failure dye only, so request dye is
    // independently rejected at the earlier capability gate.
    let mut dye = DyeSuite {
        request: vec![DyeRule {
            name: "x-root".into(),
            on: vec![DyeOutcome::Success],
            value: None,
            conditions: None,
        }],
        response: vec![],
    };
    let refs: Vec<_> = headers.iter().map(String::as_str).collect();
    assert!(dye
        .validation_error(true, &[DyeOutcome::Success], &refs)
        .unwrap()
        .contains("collid"));
    dye.request[0].name = "X-Outcome".into();
    assert!(dye.validation_error(true, &[DyeOutcome::Success], &refs).is_none());
    entry.dye = Some(dye);
    assert!(!with_request_plugins(vec![entry.clone()]).validate_plugins().is_empty());
    entry.dye = Some(dye_response("X-Outcome", &[DyeOutcome::FailOpen]));
    assert!(with_request_plugins(vec![entry]).validate_plugins().is_empty());
}
