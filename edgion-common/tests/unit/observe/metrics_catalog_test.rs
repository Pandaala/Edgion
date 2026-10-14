use super::*;
#[test]
fn catalog_has_exact_classes_and_no_open_fixed_domains() {
    assert_eq!(DEFINITIONS.len(), 102);
    let mut names = std::collections::HashSet::new();
    for def in DEFINITIONS {
        assert!(names.insert(def.name), "duplicate metric family");
        assert!(
            std::ptr::eq(def, definition(def.name).unwrap()),
            "lookup must resolve its exact definition"
        );
        let mut labels = std::collections::HashSet::new();
        for label in def.labels {
            assert!(labels.insert(label.key), "duplicate label key");
            if let LabelDomain::Closed(values) = label.domain {
                assert!(!values.is_empty());
                assert_eq!(
                    values.iter().collect::<std::collections::HashSet<_>>().len(),
                    values.len()
                );
            }
        }
    }
    assert_eq!(
        DEFINITIONS.iter().filter(|def| def.class == MetricClass::Fixed).count(),
        77
    );
    assert_eq!(
        DEFINITIONS
            .iter()
            .filter(|def| def.class == MetricClass::Dynamic)
            .count(),
        23
    );
    assert_eq!(
        DEFINITIONS
            .iter()
            .filter(|def| def.class == MetricClass::Snapshot)
            .count(),
        2
    );
    for def in DEFINITIONS.iter().filter(|def| def.class == MetricClass::Fixed) {
        assert!(def
            .labels
            .iter()
            .all(|label| !matches!(label.domain, LabelDomain::Open)));
    }
}
#[test]
fn rejects_identity_labels_wrong_class_kind_and_open_kind() {
    let key = Key::from_parts(
        "edgion_ctx_created_total",
        vec![metrics::Label::new("route_name", "route-123")],
    );
    assert!(!validate(&key, MetricType::Counter, MetricClass::Fixed));
    let key = Key::from_static_name("edgion_ctx_created_total");
    assert!(!validate(&key, MetricType::Gauge, MetricClass::Fixed));
    assert!(!validate(&key, MetricType::Counter, MetricClass::Dynamic));
    let key = Key::from_parts(
        "edgion_config_event_parse_errors_total",
        vec![metrics::Label::new("kind", "resource-123")],
    );
    assert!(!validate(&key, MetricType::Counter, MetricClass::Fixed));
}
#[test]
fn rejects_duplicate_undeclared_and_oversized_labels() {
    let key = Key::from_parts(
        "edgion_lb_degrade_selections_total",
        vec![metrics::Label::new("service", "a"), metrics::Label::new("service", "b")],
    );
    assert!(!validate(&key, MetricType::Counter, MetricClass::Dynamic));
    let key = Key::from_parts(
        "edgion_lb_degrade_selections_total",
        vec![metrics::Label::new("service", "x".repeat(MAX_LABEL_BYTES + 1))],
    );
    assert!(!validate(&key, MetricType::Counter, MetricClass::Dynamic));
    let key = Key::from_parts(
        "edgion_ldap_auth_errors_total",
        vec![metrics::Label::new("kind", "timestamp-123")],
    );
    assert!(!validate(&key, MetricType::Counter, MetricClass::Fixed));
}

#[test]
fn every_canonical_kind_is_valid_but_cli_aliases_are_not() {
    for info in edgion_resources::resource::ALL_RESOURCE_INFOS {
        let key = Key::from_parts(
            "edgion_config_event_parse_errors_total",
            vec![metrics::Label::new("kind", info.kind.as_str())],
        );
        assert!(validate(&key, MetricType::Counter, MetricClass::Fixed));
        if info.kind_name != info.kind.as_str() {
            let alias = Key::from_parts(
                "edgion_config_event_parse_errors_total",
                vec![metrics::Label::new("kind", info.kind_name)],
            );
            assert!(!validate(&alias, MetricType::Counter, MetricClass::Fixed));
        }
    }
}

#[test]
fn grpc_web_fallback_keeps_its_protocol_label() {
    for (name, kind) in [
        ("edgion_backend_requests_total", MetricType::Counter),
        ("edgion_gateway_request_duration_ms", MetricType::Histogram),
    ] {
        let key = Key::from_parts(name, vec![metrics::Label::new("protocol", "grpc-web")]);
        assert!(validate(&key, kind, MetricClass::Dynamic));
    }
}
