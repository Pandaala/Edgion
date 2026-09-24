use super::*;
use crate::plugins::runtime::PluginRuntime;
use edgion_resources::resources::edgion_plugins::{AiModelHeaderMatch, AiModelRouteCandidate, AiModelRouteMatch};
use edgion_resources::resources::http_route::HTTPBackendRef;
use std::sync::Arc;

/// Build a `HeaderMap` from `(name, value)` pairs for the routing tests.
/// Repeating a name appends a second field line.
fn headers(pairs: &[(&str, &str)]) -> http::HeaderMap {
    let mut map = http::HeaderMap::new();
    for (name, value) in pairs {
        map.append(
            http::HeaderName::from_bytes(name.as_bytes()).expect("test header name must be valid"),
            http::HeaderValue::from_str(value).expect("test header value must be valid"),
        );
    }
    map
}

fn backend(name: &str, slot: usize, weight: i32) -> CompiledBackend {
    CompiledBackend::compile(
        HTTPBackendRef {
            name: name.to_string(),
            namespace: Some("ai".to_string()),
            port: None,
            weight: Some(weight),
            group: Some("edgion.io".to_string()),
            kind: Some("EdgionBackend".to_string()),
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        },
        "ai",
        slot,
        Arc::new(PluginRuntime::new()),
    )
}

fn selector(name: &str) -> AiBackendSlotSelector {
    AiBackendSlotSelector {
        group: "edgion.io".to_string(),
        kind: "EdgionBackend".to_string(),
        namespace: "ai".to_string(),
        name: name.to_string(),
        section: None,
        occurrence: 0,
    }
}

#[test]
fn exact_and_headers_beat_wildcard_and_apply_one_transform() {
    let routes = vec![
        AiModelRoute {
            match_: AiModelRouteMatch {
                model: "gpt*".to_string(),
                headers: Vec::new(),
            },
            action: AiModelRouteAction {
                rewrite: Some("broad".to_string()),
                candidates: None,
            },
        },
        AiModelRoute {
            match_: AiModelRouteMatch {
                model: "gpt-4".to_string(),
                headers: vec![AiModelHeaderMatch {
                    name: "X-Tier".to_string(),
                    value: "paid".to_string(),
                }],
            },
            action: AiModelRouteAction {
                rewrite: Some("exact".to_string()),
                candidates: Some(vec![AiModelRouteCandidate {
                    backend: selector("b"),
                    weight: Some(9),
                }]),
            },
        },
    ];
    let table = ModelRouteTable::compile(&routes).unwrap();
    let decision = table
        .decide(
            "gpt-4",
            &headers(&[("x-tier", "paid")]),
            &[backend("a", 0, 1), backend("b", 1, 3)],
        )
        .unwrap();
    assert_eq!(decision.identity_model, "exact");
    assert_eq!(decision.weights, vec![0, 9]);
    assert!(decision.rewrite_requires_canonical);
}

#[test]
fn wildcard_family_listing_detects_partial_shadowing() {
    let table = ModelRouteTable::compile(&[AiModelRoute {
        match_: AiModelRouteMatch {
            model: "gpt-pro*".to_string(),
            headers: Vec::new(),
        },
        action: AiModelRouteAction {
            rewrite: Some("pro".to_string()),
            candidates: None,
        },
    }])
    .unwrap();
    assert!(table.wildcard_is_partially_shadowed("gpt*", &headers(&[])));
    assert!(!table.wildcard_is_partially_shadowed("other*", &headers(&[])));
    assert!(!table.wildcard_is_partially_shadowed("gpt-pro*", &headers(&[])));

    let with_broader = ModelRouteTable::compile(&[
        AiModelRoute {
            match_: AiModelRouteMatch {
                model: "gpt*".to_string(),
                headers: Vec::new(),
            },
            action: AiModelRouteAction {
                rewrite: Some("broad".to_string()),
                candidates: None,
            },
        },
        AiModelRoute {
            match_: AiModelRouteMatch {
                model: "gpt-pro*".to_string(),
                headers: Vec::new(),
            },
            action: AiModelRouteAction {
                rewrite: Some("pro".to_string()),
                candidates: None,
            },
        },
    ])
    .unwrap();
    assert!(!with_broader.wildcard_is_partially_shadowed("gpt-pro*", &headers(&[])));
    assert!(with_broader.wildcard_is_partially_shadowed("gpt*", &headers(&[])));
}

#[test]
fn specificity_is_exact_then_longest_prefix_then_header_count() {
    let route = |model: &str, headers: Vec<AiModelHeaderMatch>, rewrite: &str| AiModelRoute {
        match_: AiModelRouteMatch {
            model: model.to_string(),
            headers,
        },
        action: AiModelRouteAction {
            rewrite: Some(rewrite.to_string()),
            candidates: None,
        },
    };
    let header = |name: &str, value: &str| AiModelHeaderMatch {
        name: name.to_string(),
        value: value.to_string(),
    };
    let table = ModelRouteTable::compile(&[
        route("g*", Vec::new(), "broad"),
        route("gpt*", Vec::new(), "long"),
        route("gpt*", vec![header("x-tier", "paid")], "headed"),
        route("gpt-4", Vec::new(), "exact"),
    ])
    .unwrap();
    let backends = [backend("a", 0, 1)];
    assert_eq!(
        table.decide("gpt-3", &headers(&[]), &backends).unwrap().identity_model,
        "long"
    );
    assert_eq!(
        table
            .decide("gpt-3", &headers(&[("X-Tier", "free"), ("x-tier", "paid")]), &backends,)
            .unwrap()
            .identity_model,
        "headed"
    );
    assert_eq!(
        table.decide("gpt-4", &headers(&[]), &backends).unwrap().identity_model,
        "exact"
    );
    assert!(!table.decide("GPT-4", &headers(&[]), &backends).unwrap().matched);
    assert!(!table.decide("other", &headers(&[]), &backends).unwrap().matched);
}

#[test]
fn typed_occurrence_and_weight_overrides_preserve_unselected_invalid_share() {
    let mut first = backend("same", 0, 7);
    let mut second = backend("same", 1, 0);
    second.identity.occurrence = 1;
    first.schema.ref_denied = Some(edgion_resources::resources::common::RefDenied {
        target_namespace: "ai".to_string(),
        target_name: "same".to_string(),
        reason: Some("denied".to_string()),
    });
    let mut second_selector = selector("same");
    second_selector.occurrence = 1;
    let table = ModelRouteTable::compile(&[AiModelRoute {
        match_: AiModelRouteMatch {
            model: "virtual".to_string(),
            headers: Vec::new(),
        },
        action: AiModelRouteAction {
            rewrite: None,
            candidates: Some(vec![AiModelRouteCandidate {
                backend: second_selector,
                weight: Some(5),
            }]),
        },
    }])
    .unwrap();
    let decision = table.decide("virtual", &headers(&[]), &[first, second]).unwrap();
    assert_eq!(decision.weights, vec![7, 5]);

    let missing = ModelRouteTable::compile(&[AiModelRoute {
        match_: AiModelRouteMatch {
            model: "virtual".to_string(),
            headers: Vec::new(),
        },
        action: AiModelRouteAction {
            rewrite: None,
            candidates: Some(vec![AiModelRouteCandidate {
                backend: selector("missing"),
                weight: None,
            }]),
        },
    }])
    .unwrap();
    assert_eq!(
        missing.decide("virtual", &headers(&[]), &[backend("same", 0, 1)]),
        Err(ModelRouteDecisionError::InvalidSelector)
    );

    let all_zero = ModelRouteTable::compile(&[AiModelRoute {
        match_: AiModelRouteMatch {
            model: "virtual".to_string(),
            headers: Vec::new(),
        },
        action: AiModelRouteAction {
            rewrite: None,
            candidates: Some(vec![AiModelRouteCandidate {
                backend: selector("same"),
                weight: Some(0),
            }]),
        },
    }])
    .unwrap();
    assert_eq!(
        all_zero
            .decide(
                "virtual",
                &headers(&[]),
                &[backend("same", 0, 3), backend("other", 1, 5)]
            )
            .unwrap()
            .weights,
        vec![0, 0]
    );
}
