use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use edgion_resources::resources::common::outbound_request::ListFields;

#[test]
fn from_spec_rejects_malformed_explicit_header_name() {
    use edgion_resources::resources::common::outbound_request::{CustomField, CustomSource};

    let marker = "private malformed header";
    let spec = OutboundRequestSpec {
        headers: ListFields {
            custom: vec![CustomField {
                name: marker.to_string(),
                source: CustomSource::Value("value".to_string()),
            }],
            ..Default::default()
        },
        ..Default::default()
    };

    let error = CompiledRequest::from_spec(&spec).expect_err("malformed header name must fail compilation");
    assert_eq!(error, "request.headers.custom[0].name must be a valid HTTP header name");
    assert!(!error.contains(marker));
}

#[test]
fn presence_policies_cover_missing_and_present_fields() {
    use edgion_resources::resources::common::outbound_request::{MethodSpec, OriginField, PathSpec};
    for kind in [FieldKind::Header, FieldKind::Query, FieldKind::Cookie] {
        let forward_all_values: &[bool] = if matches!(kind, FieldKind::Header) {
            &[false, true]
        } else {
            &[false]
        };
        for &forward_all in forward_all_values {
            for presence in [
                PresencePolicy::SkipIfMissing,
                PresencePolicy::WarnIfMissing,
                PresencePolicy::Required,
            ] {
                for present in [false, true] {
                    let mut spec = OutboundRequestSpec {
                        method: Some(MethodSpec { template: "GET".into() }),
                        path: Some(PathSpec {
                            template: "/verify".into(),
                            allow_override: false,
                        }),
                        ..Default::default()
                    };
                    let fields = match kind {
                        FieldKind::Header => &mut spec.headers,
                        FieldKind::Query => &mut spec.args,
                        FieldKind::Cookie => &mut spec.cookies,
                    };
                    fields.forward_all = forward_all;
                    fields.origin.push(OriginField {
                        name: "x-source".into(),
                        presence,
                    });
                    let compiled = CompiledRequest::from_spec(&spec).unwrap();
                    let mut session = MockPluginSession::new();
                    setup_header_mocks(
                        &mut session,
                        if present { &[("x-source", "private-value")] } else { &[] },
                    );
                    session
                        .expect_get_query_param()
                        .returning(move |_| present.then(|| "private-value".into()));
                    session
                        .expect_get_cookie()
                        .returning(move |_| present.then(|| "private-value".into()));
                    let mut diagnostics = OutboundRequestDiagnostics::default();
                    let result = compiled.build_request(&session, None, "http://example.test", None, &mut diagnostics);
                    assert_eq!(
                        result.is_err(),
                        !present && matches!(presence, PresencePolicy::Required)
                    );
                    if let Ok(request) = result {
                        let forwarded = match kind {
                            FieldKind::Header => request
                                .headers()
                                .get("x-source")
                                .map(|v| v.to_str().unwrap().to_owned()),
                            FieldKind::Query => request
                                .url()
                                .query_pairs()
                                .find(|(k, _)| k == "x-source")
                                .map(|(_, v)| v.into_owned()),
                            FieldKind::Cookie => {
                                request.headers().get("cookie").map(|v| v.to_str().unwrap().to_owned())
                            }
                        };
                        assert_eq!(forwarded.is_some(), present);
                        if let Some(value) = forwarded {
                            assert!(value.contains("private-value"));
                        }
                    }
                    let mut log = PluginLog::new("test");
                    diagnostics.emit(&mut log);
                    assert_eq!(
                        log.render_for_test(),
                        if !present && matches!(presence, PresencePolicy::WarnIfMissing) {
                            "warn outbound field missing"
                        } else {
                            ""
                        }
                    );
                }
            }
        }
    }
}

#[test]
fn missing_warning_is_bounded_and_survives_later_required_failure() {
    let mut compiled = test_presence_failure_request(PresencePolicy::WarnIfMissing);
    compiled.headers.origin = vec![compiled.headers.origin[0].clone(); 1024];
    compiled.args = compiled.headers.clone();
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[]);
    session.expect_get_query_param().return_const(None::<String>);
    session.expect_get_cookie().return_const(None::<String>);
    let mut diagnostics = OutboundRequestDiagnostics::default();
    assert!(compiled
        .build_request(&session, None, "http://example.test", None, &mut diagnostics)
        .is_err());
    let mut log = PluginLog::new("test");
    diagnostics.emit(&mut log);
    assert_eq!(log.render_for_test(), "warn outbound field missing");
    assert_eq!(std::mem::size_of::<OutboundRequestDiagnostics>(), 1);
}

/// TDD test (written before implementation): `forward_all=true` forwards
/// non-hop-by-hop headers and strips both static and dynamically-listed
/// hop-by-hop headers (RFC 9110 §7.6.1).
///
/// RED: fails because `collect_headers` returns an empty map.
/// GREEN: passes after the full implementation is wired in.
#[test]
fn forward_all_includes_normal_strips_hop_by_hop() {
    use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;

    let spec = OutboundRequestSpec {
        headers: ListFields {
            forward_all: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let compiled = CompiledRequest::from_spec(&spec).unwrap();

    let mut session = MockPluginSession::new();
    // Connection header nominates "X-Custom-Hop" as an additional hop-by-hop name.
    // All request headers the session exposes, including hop-by-hop and normal ones.
    setup_header_mocks(
        &mut session,
        &[
            ("content-type", "application/json"),
            ("content-length", "9"),
            ("transfer-encoding", "chunked"),
            ("connection", "X-Custom-Hop"),
            ("keep-alive", "timeout=5"),
            ("x-custom-hop", "remove-this"),
            ("authorization", "Bearer token123"),
        ],
    );

    let result = compiled
        .collect_headers(&session, None, &mut Default::default())
        .unwrap();

    // Non-hop-by-hop headers must be present.
    assert_eq!(
        result.get("content-type").and_then(|value| value.to_str().ok()),
        Some("application/json"),
        "content-type should be forwarded"
    );
    assert_eq!(
        result.get("authorization").and_then(|value| value.to_str().ok()),
        Some("Bearer token123"),
        "authorization should be forwarded"
    );
    // Static hop-by-hop headers must be stripped.
    assert!(
        result.get("connection").is_none(),
        "connection should be stripped (static hop-by-hop)"
    );
    assert!(
        result.get("keep-alive").is_none(),
        "keep-alive should be stripped (static hop-by-hop)"
    );
    assert!(
        result.get("content-length").is_none(),
        "content-length belongs to the outbound body, not the incoming request"
    );
    assert!(
        result.get("transfer-encoding").is_none(),
        "transfer-encoding belongs to the outbound transport"
    );
    // Dynamically-listed hop-by-hop header must be stripped.
    assert!(
        result.get("x-custom-hop").is_none(),
        "x-custom-hop should be stripped (nominated by Connection header)"
    );
}

#[test]
fn forward_all_preserves_duplicate_header_values() {
    use edgion_resources::resources::common::outbound_request::{
        CustomField, CustomSource, MethodSpec, OutboundRequestSpec, PathSpec,
    };

    let spec = OutboundRequestSpec {
        path: Some(PathSpec {
            template: "/verify".to_string(),
            allow_override: false,
        }),
        method: Some(MethodSpec {
            template: "GET".to_string(),
        }),
        headers: ListFields {
            forward_all: true,
            custom: vec![CustomField {
                name: "X-Role".to_string(),
                source: CustomSource::Value("configured".to_string()),
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    let compiled = CompiledRequest::from_spec(&spec).unwrap();

    let mut session = MockPluginSession::new();
    setup_header_mocks(
        &mut session,
        &[
            ("x-forwarded-for", "192.0.2.1"),
            ("x-forwarded-for", "198.51.100.2"),
            ("x-role", "incoming-first"),
            ("x-role", "incoming-second"),
        ],
    );

    let req = compiled
        .build_request(&session, None, "http://auth.test", None, &mut Default::default())
        .unwrap();
    let values = req
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .map(|value| value.to_str().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(values, vec!["192.0.2.1", "198.51.100.2"]);
    assert_eq!(
        req.headers()
            .get_all("x-role")
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["configured"]
    );
}

#[test]
fn forward_all_preserves_sensitive_values_and_utf8_filtering() {
    let spec = OutboundRequestSpec {
        headers: ListFields {
            forward_all: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let compiled = CompiledRequest::from_spec(&spec).unwrap();
    let mut headers = http::HeaderMap::new();
    for bytes in [b"private-first".as_slice(), b"private-second".as_slice()] {
        let mut value = http::HeaderValue::from_bytes(bytes).unwrap();
        value.set_sensitive(true);
        headers.append("x-custom-credential", value);
    }
    headers.insert("x-visible", http::HeaderValue::from_static("public"));
    headers.insert("x-utf8", http::HeaderValue::from_bytes("é".as_bytes()).unwrap());
    headers.insert("x-invalid-utf8", http::HeaderValue::from_bytes(&[0xff]).unwrap());
    headers.insert("connection", http::HeaderValue::from_static("x-private-hop"));
    let mut hop = http::HeaderValue::from_static("hop-secret");
    hop.set_sensitive(true);
    headers.insert("x-private-hop", hop);
    let mut session = MockPluginSession::new();
    session.expect_request_header_map().return_const(headers);

    let result = compiled
        .collect_headers(&session, None, &mut Default::default())
        .unwrap();
    let values: Vec<_> = result.get_all("x-custom-credential").iter().collect();
    assert_eq!(values.len(), 2);
    assert_eq!(values[0].as_bytes(), b"private-first");
    assert_eq!(values[1].as_bytes(), b"private-second");
    assert!(values.iter().all(|value| value.is_sensitive()));
    assert!(!result["x-visible"].is_sensitive());
    assert_eq!(result["x-utf8"].as_bytes(), "é".as_bytes());
    assert!(!result.contains_key("x-invalid-utf8"));
    assert!(!result.contains_key("connection"));
    assert!(!result.contains_key("x-private-hop"));
    assert!(!format!("{result:?}").contains("private-first"));
}

#[test]
fn rendered_credentials_origin_headers_and_cookies_are_sensitive() {
    use edgion_resources::resources::common::outbound_request::{OriginField, PresencePolicy};
    let mut spec = body_spec(BodySpec::Raw { template: "".into() });
    spec.headers = ListFields {
        origin: vec![OriginField {
            name: "x-identity".into(),
            presence: PresencePolicy::Required,
        }],
        custom: vec![
            CustomField {
                name: "x-arbitrary-credential".into(),
                source: CustomSource::Template("Bearer ${secretRef:default/auth:token}".into()),
            },
            CustomField {
                name: "x-configured".into(),
                source: CustomSource::Value("fixed-value".into()),
            },
        ],
        ..Default::default()
    };
    spec.cookies = ListFields {
        custom: vec![CustomField {
            name: "session".into(),
            source: CustomSource::Template("${secretRef:default/auth:token}".into()),
        }],
        ..Default::default()
    };
    let compiled = CompiledRequest::from_spec(&spec).unwrap();
    let mut session = MockPluginSession::new();
    session
        .expect_header_value()
        .withf(|name| name == "x-identity")
        .return_const(Some("private-user".to_owned()));
    let resolved = ResolvedSecrets(std::collections::BTreeMap::from([(
        "default/auth:token".into(),
        "private-token".into(),
    )]));
    let request = compiled
        .build_request(
            &session,
            None,
            "http://upstream.test",
            Some(&resolved),
            &mut Default::default(),
        )
        .unwrap();
    for (name, expected) in [
        ("x-identity", "private-user"),
        ("x-arbitrary-credential", "Bearer private-token"),
        ("x-configured", "fixed-value"),
        ("cookie", "session=private-token"),
    ] {
        assert_eq!(request.headers()[name].as_bytes(), expected.as_bytes());
        assert!(request.headers()[name].is_sensitive());
    }
    assert!(!format!("{:?}", request.headers()).contains("private-token"));
}

#[test]
fn template_header_provenance_survives_header_only_and_request_clones() {
    let mut spec = body_spec(BodySpec::Raw { template: "".into() });
    spec.headers.custom = vec![
        CustomField {
            name: "x-private".into(),
            source: CustomSource::Template("prefix-${secretRef:ns/token:key}-suffix".into()),
        },
        CustomField {
            name: "x-public".into(),
            source: CustomSource::Template("prefix-${ctx:name}-suffix".into()),
        },
    ];
    let compiled = CompiledRequest::from_spec(&spec).unwrap();
    let mut session = MockPluginSession::new();
    session.expect_get_ctx_var().return_const(Some("public".to_string()));
    let secret = ResolvedSecrets(std::collections::BTreeMap::from([(
        "ns/token:key".to_string(),
        "private".to_string(),
    )]));
    for resolved in [Some(&secret), None] {
        let headers = compiled
            .collect_headers(&session, resolved, &mut Default::default())
            .unwrap();
        let expected = if resolved.is_some() {
            "prefix-private-suffix"
        } else {
            "prefix--suffix"
        };
        assert_eq!(headers["x-private"], expected);
        assert!(headers["x-private"].is_sensitive());
        assert_eq!(headers["x-public"], "prefix-public-suffix");
        assert!(!headers["x-public"].is_sensitive());
        let request = compiled
            .build_request(
                &session,
                None,
                "http://upstream.test",
                resolved,
                &mut Default::default(),
            )
            .unwrap();
        let cloned = request.try_clone().unwrap();
        assert!(cloned.headers()["x-private"].is_sensitive());
        assert!(!cloned.headers()["x-public"].is_sensitive());
        assert!(!format!("{:?}", cloned.headers()).contains("prefix-private-suffix"));
    }
}

#[test]
fn rendered_url_and_method_errors_do_not_disclose_template_values() {
    use edgion_resources::resources::common::outbound_request::MethodSpec;
    let mut spec = body_spec(BodySpec::Raw { template: "".into() });
    spec.method = Some(MethodSpec {
        template: "${secretRef:ns/token:key}".into(),
    });
    let secret = ResolvedSecrets(std::collections::BTreeMap::from([(
        "ns/token:key".to_string(),
        "sentinel private".to_string(),
    )]));
    let compiled = CompiledRequest::from_spec(&spec).unwrap();
    let session = MockPluginSession::new();
    let error = compiled
        .build_request(
            &session,
            None,
            "http://upstream.test",
            Some(&secret),
            &mut Default::default(),
        )
        .unwrap_err();
    assert_eq!(error, "invalid outbound HTTP method");
    let error = compiled
        .build_request(
            &session,
            None,
            "http://[sentinel-private",
            Some(&secret),
            &mut Default::default(),
        )
        .unwrap_err();
    assert_eq!(error, "invalid outbound URL");
}

#[test]
fn bodyless_outbound_request_does_not_inherit_incoming_content_length() {
    use edgion_resources::resources::common::outbound_request::{MethodSpec, OutboundRequestSpec, PathSpec};

    let spec = OutboundRequestSpec {
        path: Some(PathSpec {
            template: "/verify".to_string(),
            allow_override: false,
        }),
        method: Some(MethodSpec {
            template: "GET".to_string(),
        }),
        headers: ListFields {
            forward_all: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let compiled = CompiledRequest::from_spec(&spec).unwrap();

    let mut session = MockPluginSession::new();
    setup_header_mocks(
        &mut session,
        &[("authorization", "Bearer valid-token"), ("content-length", "9")],
    );

    let req = compiled
        .build_request(&session, None, "http://auth.test", None, &mut Default::default())
        .unwrap();

    assert!(req.body().is_none());
    assert!(req.headers().get(reqwest::header::CONTENT_LENGTH).is_none());
    assert_eq!(
        req.headers()
            .get(reqwest::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer valid-token")
    );
}

#[test]
fn body_framing_sync_replaces_stale_length_and_transfer_encoding() {
    let mut req = reqwest::Request::new(
        reqwest::Method::POST,
        reqwest::Url::parse("http://upstream.test/").unwrap(),
    );
    req.headers_mut().insert(
        reqwest::header::CONTENT_LENGTH,
        reqwest::header::HeaderValue::from_static("99"),
    );
    req.headers_mut().insert(
        reqwest::header::TRANSFER_ENCODING,
        reqwest::header::HeaderValue::from_static("chunked"),
    );
    *req.body_mut() = Some(bytes::Bytes::from_static(b"abc").into());

    sync_outbound_body_framing(&mut req).unwrap();

    assert_eq!(
        req.headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()),
        Some("3")
    );
    assert!(req.headers().get(reqwest::header::TRANSFER_ENCODING).is_none());
}

/// C2 regression: `forward_all=true` with a `Required` origin field whose
/// header is absent from the session must return `Err`, not silently omit it.
///
/// Before the fix, `collect_headers` used `let _ = collect_list_fields(...)`,
/// which swallowed the error and downgraded `Required` to `SkipIfMissing`.
#[test]
fn forward_all_with_required_origin_missing_returns_error() {
    use edgion_resources::resources::common::outbound_request::{OriginField, OutboundRequestSpec, PresencePolicy};

    let spec = OutboundRequestSpec {
        headers: ListFields {
            forward_all: true,
            origin: vec![OriginField {
                name: "X-Required".to_string(),
                presence: PresencePolicy::Required,
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    let compiled = CompiledRequest::from_spec(&spec).unwrap();

    let mut session = MockPluginSession::new();
    // No "connection" header — connection_listed_hop_by_hop returns empty set.
    // The session headers do not include X-Required.
    setup_header_mocks(&mut session, &[("content-type", "text/plain")]);

    let result = compiled.collect_headers(&session, None, &mut Default::default());
    assert!(result.is_err(), "Expected Err for missing Required header, got Ok");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("Required") || msg.contains("required"),
        "Error message should mention Required; got: {msg}"
    );
}

/// `Expect` is end-to-end, so hop-by-hop filtering keeps it. `forward_all`
/// must still withhold it: the outbound auth call sends no body of its own,
/// and a server enforcing the expectation answers `417 Expectation Failed`,
/// which ForwardAuth would read as a denied request.
#[test]
fn forward_all_strips_expect_continue() {
    use edgion_resources::resources::common::outbound_request::OutboundRequestSpec;

    let spec = OutboundRequestSpec {
        headers: ListFields {
            forward_all: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let compiled = CompiledRequest::from_spec(&spec).unwrap();

    let mut session = MockPluginSession::new();
    setup_header_mocks(
        &mut session,
        &[("expect", "100-continue"), ("authorization", "Bearer token123")],
    );

    let result = compiled
        .collect_headers(&session, None, &mut Default::default())
        .unwrap();

    assert!(
        !result.contains_key("expect"),
        "expect states an expectation owned by the inbound hop"
    );
    assert_eq!(
        result.get("authorization").and_then(|value| value.to_str().ok()),
        Some("Bearer token123"),
        "unrelated headers must still be forwarded"
    );
}

/// The automatic skip covers `forward_all` only. Naming `Expect` in `origin`
/// is a deliberate operator act and keeps working, matching how the other
/// non-hop-by-hop skips treat an explicit listing.
#[test]
fn explicitly_listed_origin_expect_is_still_forwarded() {
    use edgion_resources::resources::common::outbound_request::{OriginField, OutboundRequestSpec, PresencePolicy};

    let spec = OutboundRequestSpec {
        headers: ListFields {
            forward_all: true,
            origin: vec![OriginField {
                name: "Expect".to_string(),
                presence: PresencePolicy::SkipIfMissing,
            }],
            ..Default::default()
        },
        ..Default::default()
    };
    let compiled = CompiledRequest::from_spec(&spec).unwrap();

    let mut session = MockPluginSession::new();
    // Deliberately NOT `setup_header_mocks`: this test needs the two lookup
    // paths to disagree so the assertion can tell which branch produced the
    // entry. The forward_all sweep reads the borrowed map; the explicitly
    // listed origin field renders through `header_value`.
    let mut map = http::HeaderMap::new();
    map.append(
        http::HeaderName::from_static("expect"),
        http::HeaderValue::from_static("100-continue-from-sweep"),
    );
    session.expect_request_header_map().return_const(map);
    session.expect_header_value().returning(|name| {
        name.eq_ignore_ascii_case("expect")
            .then(|| "100-continue-via-origin".to_string())
    });

    let result = compiled
        .collect_headers(&session, None, &mut Default::default())
        .unwrap();

    assert_eq!(
        result.get("expect").and_then(|value| value.to_str().ok()),
        Some("100-continue-via-origin"),
        "an explicitly listed origin field overrides the forward_all skip"
    );
}

use edgion_resources::resources::common::outbound_request::{BodySpec, CustomField, CustomSource, OutboundRequestSpec};

/// Build a spec with fixed path "/wh" + method "POST" so the session mock
/// only needs expectations for the variables the body actually reads.
fn body_spec(body: BodySpec) -> OutboundRequestSpec {
    use edgion_resources::resources::common::outbound_request::{MethodSpec, PathSpec};
    OutboundRequestSpec {
        path: Some(PathSpec {
            template: "/wh".to_string(),
            allow_override: false,
        }),
        method: Some(MethodSpec {
            template: "POST".to_string(),
        }),
        body: Some(body),
        ..Default::default()
    }
}

fn req_body_string(req: &reqwest::Request) -> String {
    String::from_utf8(req.body().unwrap().as_bytes().unwrap().to_vec()).unwrap()
}

#[test]
fn json_body_escapes_variable_values_and_sets_content_type() {
    let spec = body_spec(BodySpec::Json {
        template: r#"{"user": "${ctx:user}", "source": "edgion"}"#.to_string(),
    });
    let compiled = CompiledRequest::from_spec(&spec).unwrap();

    let mut session = MockPluginSession::new();
    // Value contains a quote, a backslash, and a newline — all must be escaped.
    session
        .expect_get_ctx_var()
        .withf(|n| n == "user")
        .return_const(Some("ali\"ce\\bob\nline2".to_string()));

    let req = compiled
        .build_request(&session, None, "http://upstream.test", None, &mut Default::default())
        .unwrap();

    let body = req_body_string(&req);
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("rendered body must be valid JSON");
    assert_eq!(parsed["user"], "ali\"ce\\bob\nline2");
    assert_eq!(parsed["source"], "edgion");
    assert_eq!(
        req.headers().get("content-type").map(|v| v.to_str().unwrap()),
        Some("application/json")
    );
    assert_eq!(
        req.headers().get("content-length").map(|v| v.to_str().unwrap()),
        Some(body.len().to_string().as_str())
    );
}

#[test]
fn form_body_percent_encodes_values() {
    let spec = body_spec(BodySpec::Form {
        fields: vec![
            CustomField {
                name: "userId".to_string(),
                source: CustomSource::Template("${ctx:uid}".to_string()),
            },
            CustomField {
                name: "source".to_string(),
                source: CustomSource::Value("edgion".to_string()),
            },
        ],
    });
    let compiled = CompiledRequest::from_spec(&spec).unwrap();

    let mut session = MockPluginSession::new();
    session
        .expect_get_ctx_var()
        .withf(|n| n == "uid")
        .return_const(Some("a&b=c d".to_string()));

    let req = compiled
        .build_request(&session, None, "http://upstream.test", None, &mut Default::default())
        .unwrap();

    assert_eq!(req_body_string(&req), "userId=a%26b%3Dc+d&source=edgion");
    assert_eq!(
        req.headers().get("content-type").map(|v| v.to_str().unwrap()),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        req.headers().get("content-length").map(|v| v.to_str().unwrap()),
        Some(req_body_string(&req).len().to_string().as_str())
    );
}

#[test]
fn raw_body_is_verbatim_without_content_type() {
    let spec = body_spec(BodySpec::Raw {
        template: "plain ${ctx:x} text".to_string(),
    });
    let compiled = CompiledRequest::from_spec(&spec).unwrap();

    let mut session = MockPluginSession::new();
    session
        .expect_get_ctx_var()
        .withf(|n| n == "x")
        .return_const(Some("a\"b".to_string()));

    let req = compiled
        .build_request(&session, None, "http://upstream.test", None, &mut Default::default())
        .unwrap();

    // Raw: no escaping, no auto Content-Type.
    assert_eq!(req_body_string(&req), "plain a\"b text");
    assert!(req.headers().get("content-type").is_none());
    assert_eq!(
        req.headers().get("content-length").map(|v| v.to_str().unwrap()),
        Some("14")
    );
}

#[test]
fn json_body_content_type_overrides_forwarded_incoming() {
    use edgion_resources::resources::common::outbound_request::ListFields;
    let mut spec = body_spec(BodySpec::Json {
        template: r#"{"a": "1"}"#.to_string(),
    });
    spec.headers = ListFields {
        forward_all: true,
        ..Default::default()
    };
    let compiled = CompiledRequest::from_spec(&spec).unwrap();

    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("content-type", "text/plain")]);

    let req = compiled
        .build_request(&session, None, "http://upstream.test", None, &mut Default::default())
        .unwrap();

    // The body-derived Content-Type must win over the forwarded incoming one.
    assert_eq!(
        req.headers().get("content-type").map(|v| v.to_str().unwrap()),
        Some("application/json")
    );
    assert_eq!(
        req.headers().get("content-length").map(|v| v.to_str().unwrap()),
        Some("10")
    );
}

#[test]
fn outbound_default_method_uses_entry_while_method_template_uses_current() {
    use edgion_resources::resources::common::outbound_request::MethodSpec;
    for (entry, current) in [("GET", "POST"), ("POST", "GET"), ("GET", "GET"), ("REPORT", "PATCH")] {
        for template in [None, Some("${method}"), Some("PUT")] {
            let spec = OutboundRequestSpec {
                method: template.map(|template| MethodSpec {
                    template: template.into(),
                }),
                ..Default::default()
            };
            let compiled = CompiledRequest::from_spec(&spec).unwrap();
            let mut session = MockPluginSession::new();
            session.expect_get_request_path().return_const("/entry".to_owned());
            session.expect_get_request_method().return_const(Some(entry));
            session.expect_get_method().return_const(current.to_owned());
            let request = compiled
                .build_request(&session, None, "http://upstream.test", None, &mut Default::default())
                .unwrap();
            assert_eq!(
                request.method().as_str(),
                match template {
                    None => entry,
                    Some("${method}") => current,
                    Some(_) => "PUT",
                }
            );
            assert_eq!(request.url().path(), "/entry");
            assert!(request.body().is_none());
        }
    }
}

#[test]
fn outbound_default_path_uses_entry_while_path_template_uses_current() {
    use edgion_resources::resources::common::outbound_request::PathSpec;
    for current in ["/users/42", "/graphql"] {
        for template in [None, Some("${path}"), Some("/verify")] {
            let spec = OutboundRequestSpec {
                path: template.map(|template| PathSpec {
                    template: template.into(),
                    allow_override: false,
                }),
                ..Default::default()
            };
            let mut session = MockPluginSession::new();
            session.expect_get_request_path().return_const("/users/42".to_string());
            session.expect_get_request_method().return_const(Some("GET"));
            session.expect_get_path().return_const(current.to_string());
            let req = CompiledRequest::from_spec(&spec)
                .unwrap()
                .build_request(&session, None, "http://auth.test", None, &mut Default::default())
                .unwrap();
            assert_eq!(
                req.url().path(),
                match template {
                    None => "/users/42",
                    Some("${path}") => current,
                    Some(_) => "/verify",
                }
            );
        }
    }
}

#[test]
fn outbound_missing_entry_method_fails_without_live_fallback_but_explicit_method_works() {
    use edgion_resources::resources::common::outbound_request::MethodSpec;
    for template in [None, Some("${method}"), Some("PUT")] {
        let spec = OutboundRequestSpec {
            method: template.map(|template| MethodSpec {
                template: template.into(),
            }),
            ..Default::default()
        };
        let compiled = CompiledRequest::from_spec(&spec).unwrap();
        let mut session = MockPluginSession::new();
        session.expect_get_request_path().return_const("/entry".to_owned());
        session.expect_get_request_method().return_const(None);
        session.expect_get_method().return_const("POST".to_owned());
        let result = compiled.build_request(&session, None, "http://upstream.test", None, &mut Default::default());
        match template {
            None => assert_eq!(result.unwrap_err(), "missing incoming request method"),
            Some("${method}") => assert_eq!(result.unwrap().method().as_str(), "POST"),
            Some(_) => assert_eq!(result.unwrap().method().as_str(), "PUT"),
        }
    }
}

struct TrustedRequestValues;

impl crate::link_sys::template::TrustedTemplateVariables for TrustedRequestValues {
    fn trusted_ctx_var(&self, key: &str) -> Option<String> {
        (key == "tenant").then(|| "tenant-a".to_string())
    }

    fn trusted_remote_ip(&self) -> &str {
        "192.0.2.9"
    }

    fn trusted_matched_route_meta(
        &self,
        _proto: crate::plugins::runtime::traits::session::RouteProto,
        _selector: &str,
    ) -> Option<String> {
        None
    }
}

#[test]
fn trusted_request_build_retains_normalized_guardrail_metadata_and_secret_flags() {
    let spec: OutboundRequestSpec = serde_json::from_value(serde_json::json!({
        "path": {"template": "/decision/${ctx:tenant}"},
        "method": {"template": "POST"},
        "headers": {"custom": [{"name": "authorization", "template": "Bearer ${secretRef:ns/token:key}"}]}
    }))
    .unwrap();
    let secrets = ResolvedSecrets(std::collections::BTreeMap::from([(
        "ns/token:key".into(),
        "credential".into(),
    )]));
    let request = CompiledRequest::from_spec(&spec)
        .unwrap()
        .build_request_in(
            TemplateContext::Trusted(&TrustedRequestValues),
            None,
            "http://decision.test",
            Some(&secrets),
            &mut Default::default(),
        )
        .unwrap();
    assert_eq!(request.method(), reqwest::Method::POST);
    assert_eq!(request.url().path(), "/decision/tenant-a");
    assert_eq!(request.headers()["authorization"], "Bearer credential");
    assert!(request.headers()["authorization"].is_sensitive());
    assert!(request.try_clone().unwrap().headers()["authorization"].is_sensitive());
}

#[test]
fn trusted_request_build_rejects_every_requested_incoming_capability() {
    let valid = serde_json::json!({"path": {"template": "/decision"}, "method": {"template": "POST"}});
    let mut cases = Vec::new();
    for field in ["path", "method"] {
        let mut value = valid.clone();
        value.as_object_mut().unwrap().remove(field);
        cases.push(value);
    }
    for field in ["headers", "args", "cookies"] {
        if field == "headers" {
            let mut value = valid.clone();
            value[field] = serde_json::json!({"forwardAll": true});
            cases.push(value);
        }
        let mut value = valid.clone();
        value[field] = serde_json::json!({"origin": [{"name": "x", "presence": "skipIfMissing"}]});
        cases.push(value);
    }
    for value in cases {
        let spec: OutboundRequestSpec = serde_json::from_value(value).unwrap();
        let request = CompiledRequest::from_spec(&spec).unwrap().build_request_in(
            TemplateContext::Trusted(&TrustedRequestValues),
            None,
            "http://decision.test",
            None,
            &mut Default::default(),
        );
        assert!(matches!(request, Err(error) if error == "template request capability unavailable"));
    }
}

#[test]
fn build_request_preserves_url_concatenation_boundaries() {
    use edgion_resources::resources::common::outbound_request::{MethodSpec, PathSpec};

    for (base, path, expected) in [
        ("http://example.test///", "", "http://example.test/"),
        ("http://example.test/base/", "next", "http://example.test/base/next"),
        ("http://example.test/base/", "/next", "http://example.test/base/next"),
        (
            "http://example.test",
            "//other.test/x",
            "http://example.test//other.test/x",
        ),
        (
            "http://example.test",
            "/a%20b?x=1#part",
            "http://example.test/a%20b?x=1#part",
        ),
    ] {
        for use_override in [false, true] {
            let spec = OutboundRequestSpec {
                method: Some(MethodSpec { template: "GET".into() }),
                path: Some(PathSpec {
                    template: if use_override { "/unused" } else { path }.into(),
                    allow_override: true,
                }),
                ..Default::default()
            };
            let compiled = CompiledRequest::from_spec(&spec).unwrap();
            let session = MockPluginSession::new();
            let request = compiled
                .build_request(
                    &session,
                    use_override.then_some(path),
                    base,
                    None,
                    &mut Default::default(),
                )
                .unwrap();
            assert_eq!(request.url().as_str(), expected, "{base:?} {path:?} {use_override}");
        }
    }
}
