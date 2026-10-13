use super::*;
use edgion_resources::prelude_resources::BackendTLSPolicy;

#[test]
fn admin_beta_http_route_rejects_unserved_default_gateway_attachment() {
    let source = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "HTTPRoute",
        "metadata": {"name": "route", "namespace": "demo"},
        "spec": {"useDefaultGateways": "All", "rules": [{}]}
    });
    let route: edgion_resources::HTTPRoute = parse_resource_and_update_version(&source.to_string(), false).unwrap();
    assert_eq!(route.spec.use_default_gateways.as_deref(), Some("All"));
    assert_eq!(validate_http_route_transforms(&route), Err(StatusCode::BAD_REQUEST));
}

fn backend_tls_policy_body(api_version: &str) -> String {
    format!(
        r#"{{
                "apiVersion": "{api_version}",
                "kind": "BackendTLSPolicy",
                "metadata": {{"name": "backend-tls", "namespace": "default"}},
                "spec": {{
                    "targetRefs": [{{"group": "", "kind": "Service", "name": "backend"}}],
                    "validation": {{
                        "hostname": "backend.default.svc",
                        "wellKnownCACertificates": "System"
                    }}
                }}
            }}"#
    )
}

#[test]
fn admin_parser_converts_reference_grant_beta_in_both_modes() {
    let source = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1",
        "kind": "ReferenceGrant",
        "metadata": {"name": "allow", "namespace": "backend", "labels": {"team": "edge"}},
        "spec": {
            "from": [{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "namespace": "app"}],
            "to": [{"group": "", "kind": "Service", "name": "api"}]
        }
    });
    for update_version in [false, true] {
        let grant: edgion_resources::ReferenceGrant =
            parse_resource_and_update_version(&source.to_string(), update_version).unwrap();
        assert_eq!(grant.metadata.labels.as_ref().unwrap()["team"], "edge");
        assert_eq!(grant.spec.from[0].namespace, "app");
        assert_eq!(grant.spec.to[0].name.as_deref(), Some("api"));
        assert_eq!(
            serde_json::to_value(grant).unwrap()["apiVersion"],
            "gateway.networking.k8s.io/v1"
        );
    }
}

#[test]
fn admin_parser_accepts_backend_tls_policy_alpha3_in_all_modes() {
    let legacy = backend_tls_policy_body("gateway.networking.k8s.io/v1alpha3");

    for update_version in [false, true] {
        let result = parse_resource_and_update_version::<BackendTLSPolicy>(&legacy, update_version);
        assert!(result.is_ok());
    }

    let canonical = backend_tls_policy_body("gateway.networking.k8s.io/v1");
    assert!(parse_resource_and_update_version::<BackendTLSPolicy>(&canonical, false).is_ok());
    assert!(parse_resource_and_update_version::<BackendTLSPolicy>(&canonical, true).is_ok());
}

#[test]
fn admin_parser_tls_route_version_matrix() {
    use edgion_resources::TLSRoute;
    let canonical = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "TLSRoute",
        "metadata": {"name": "tls-route", "namespace": "default"},
        "spec": {"hostnames": ["example.com"]}
    });
    for version in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::json!(42)),
        Some(serde_json::json!("")),
        Some(serde_json::json!("gateway.networking.k8s.io/v1alpha3")),
        Some(serde_json::json!("gateway.networking.k8s.io/v1")),
    ] {
        let valid = matches!(
            version.as_ref().and_then(serde_json::Value::as_str),
            Some("gateway.networking.k8s.io/v1" | "gateway.networking.k8s.io/v1alpha3")
        );
        let mut document = canonical.clone();
        match version {
            Some(value) => document["apiVersion"] = value,
            None => {
                document.as_object_mut().unwrap().remove("apiVersion");
            }
        }
        for body in [
            serde_json::to_string(&document).unwrap(),
            serde_yaml::to_string(&document).unwrap(),
        ] {
            for update_version in [false, true] {
                let result = parse_resource_and_update_version::<TLSRoute>(&body, update_version);
                if valid {
                    assert!(result.is_ok());
                } else {
                    assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST));
                }
            }
        }
    }
}

#[test]
fn admin_parser_rejects_missing_or_malformed_backend_tls_policy_version() {
    let canonical: serde_json::Value =
        serde_json::from_str(&backend_tls_policy_body("gateway.networking.k8s.io/v1")).unwrap();
    for version in [
        None,
        Some(serde_json::Value::Null),
        Some(serde_json::json!(1)),
        Some(serde_json::json!("")),
        Some(serde_json::json!("gateway.networking.k8s.io/v1beta1")),
    ] {
        let mut document = canonical.clone();
        match version {
            Some(value) => document["apiVersion"] = value,
            None => {
                document.as_object_mut().unwrap().remove("apiVersion");
            }
        }
        for body in [
            serde_json::to_string(&document).unwrap(),
            serde_yaml::to_string(&document).unwrap(),
        ] {
            for update_version in [false, true] {
                let result = parse_resource_and_update_version::<BackendTLSPolicy>(&body, update_version);
                assert_eq!(result.err(), Some(StatusCode::BAD_REQUEST), "input: {body}");
            }
        }
    }
}

#[test]
fn not_supported_maps_to_501() {
    let status = map_writer_error(ConfWriterError::NotSupported("x".to_string()));
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
}

#[test]
fn stale_pagination_maps_to_410() {
    let status = map_writer_error(ConfWriterError::StalePagination("expired".to_string()));
    assert_eq!(status, StatusCode::GONE);
}

#[test]
fn exhausted_backend_occ_conflict_maps_to_409() {
    let status = map_writer_error(ConfWriterError::Conflict("object keeps changing".to_string()));
    assert_eq!(status, StatusCode::CONFLICT);
}

#[test]
fn if_match_precedes_payload_resource_version() {
    let mut headers = HeaderMap::new();
    headers.insert(IF_MATCH, r#""header-rv""#.parse().unwrap());
    let body = r#"metadata: {resourceVersion: payload-rv}"#;
    assert_eq!(
        expected_resource_version(&headers, body).unwrap().as_deref(),
        Some("header-rv")
    );
}

#[test]
fn payload_resource_version_is_compatibility_fallback() {
    let body = r#"metadata: {resourceVersion: payload-rv}"#;
    assert_eq!(
        expected_resource_version(&HeaderMap::new(), body).unwrap().as_deref(),
        Some("payload-rv")
    );
}

#[test]
fn if_match_star_overrides_payload_resource_version() {
    let mut headers = HeaderMap::new();
    headers.insert(IF_MATCH, "*".parse().unwrap());
    let body = r#"metadata: {resourceVersion: payload-rv}"#;
    assert_eq!(expected_resource_version(&headers, body).unwrap(), None);
}

#[test]
fn absent_update_precondition_remains_unconditional() {
    let body = r#"metadata: {name: route-a}"#;
    assert_eq!(expected_resource_version(&HeaderMap::new(), body).unwrap(), None);
}

#[derive(Clone, Default)]
struct ParseLogWriter(std::sync::Arc<parking_lot::Mutex<Vec<u8>>>);

impl std::io::Write for ParseLogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ParseLogWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn capture_parse_logs(run: impl FnOnce()) -> Vec<serde_json::Value> {
    let writer = ParseLogWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .without_time()
        .with_writer(writer.clone())
        .with_max_level(tracing::Level::WARN)
        .finish();
    tracing::subscriber::with_default(subscriber, run);
    let bytes = writer.0.lock().clone();
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn assert_value_free_parse_warning(events: &[serde_json::Value], stage: &str, has_location: bool) {
    assert_eq!(events.len(), 1, "expected exactly one parser warning: {events:?}");
    let event = &events[0];
    assert_eq!(event["level"], "WARN");
    let fields = &event["fields"];
    assert_eq!(fields["component"], "unified_api");
    assert_eq!(fields["stage"], stage);
    if has_location {
        assert!(fields["line"].as_u64().is_some_and(|line| line > 0));
        assert!(fields["column"].as_u64().is_some_and(|column| column > 0));
    } else {
        assert!(fields.get("line").is_none());
        assert!(fields.get("column").is_none());
    }
    let serialized = event.to_string();
    for private_fragment in ["private-marker", "private-key", "clientAuth", "verifyDepth", "spec."] {
        assert!(
            !serialized.contains(private_fragment),
            "unexpected authored data: {serialized}"
        );
    }
    assert!(fields.get("error").is_none());
}

#[test]
fn admin_parser_warns_without_authored_values_for_json_and_yaml() {
    let valid = serde_json::json!({
        "apiVersion": "edgion.io/v1", "kind": "EdgionTls",
        "metadata": {"name": "tls", "namespace": "default"},
        "spec": {"hosts": ["example.com"], "secretRef": {"name": "tls"},
                 "clientAuth": {"mode": "Terminate", "verifyDepth": 1}}
    });
    for yaml in [false, true] {
        let encode = |value: &serde_json::Value| {
            if yaml {
                serde_yaml::to_string(value).unwrap()
            } else {
                value.to_string()
            }
        };
        let events = capture_parse_logs(|| {
            let parsed = parse_resource::<edgion_resources::EdgionTls>(&encode(&valid)).unwrap();
            assert_eq!(parsed.spec.hosts, ["example.com"]);
        });
        assert!(events.is_empty());
        for field in ["mode", "verifyDepth"] {
            let mut invalid = valid.clone();
            invalid["spec"]["clientAuth"][field] = serde_json::json!("private-marker");
            let events = capture_parse_logs(|| {
                assert_eq!(
                    parse_resource::<edgion_resources::EdgionTls>(&encode(&invalid)).err(),
                    Some(StatusCode::BAD_REQUEST)
                );
            });
            assert_value_free_parse_warning(&events, "resource_parse", true);
        }
    }

    // A dynamic map key must not become a logged parser path either.
    let events = capture_parse_logs(|| {
        assert_eq!(
            parse_resource::<std::collections::BTreeMap<String, u8>>("private-key: private-marker").err(),
            Some(StatusCode::BAD_REQUEST)
        );
    });
    assert_value_free_parse_warning(&events, "resource_parse", true);
}

#[test]
fn admin_source_version_warning_omits_authored_diagnostics() {
    let malformed = "apiVersion: gateway.networking.k8s.io/v1\nprivate-key: [private-marker";
    let events = capture_parse_logs(|| {
        assert_eq!(
            parse_resource_and_update_version::<edgion_resources::HTTPRoute>(malformed, false).err(),
            Some(StatusCode::BAD_REQUEST)
        );
    });
    assert_value_free_parse_warning(&events, "source_version", true);

    let unsupported = backend_tls_policy_body("private-marker");
    let events = capture_parse_logs(|| {
        assert_eq!(
            parse_resource_and_update_version::<BackendTLSPolicy>(&unsupported, false).err(),
            Some(StatusCode::BAD_REQUEST)
        );
    });
    assert_value_free_parse_warning(&events, "source_version", false);
}
