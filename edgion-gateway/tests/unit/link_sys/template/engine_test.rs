use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;

/// Compile a raw template string into a `CompiledTemplate`.
fn compile(raw: &str) -> CompiledTemplate {
    CompiledTemplate::compile(raw).expect("template should compile")
}

fn named_session(value: Option<&'static str>) -> MockPluginSession {
    let mut session = MockPluginSession::new();
    session.expect_get_query_param_ref().returning(move |_| value);
    let value = value.map(str::to_string);
    let mut headers = http::HeaderMap::new();
    if let Some(value) = &value {
        headers.insert("x", http::HeaderValue::from_str(value).unwrap());
        headers.insert("cookie", http::HeaderValue::from_str(&format!("x={value}")).unwrap());
    }
    session.expect_request_header_map().return_const(headers);
    session.expect_get_query_param().return_const(value.clone());
    session.expect_get_ctx_var().return_const(value.clone());
    session.expect_matched_route_meta().return_const(value);
    session
}

#[test]
fn named_variables_distinguish_missing_and_present_empty() {
    for variable in [
        "header:x",
        "query:x",
        "cookie:x",
        "ctx:x",
        "matchedHttpRoute:name",
        "matchedGrpcRoute:name",
    ] {
        let tpl = compile(&format!("${{{variable}}}"));
        for value in [Some("value"), Some(""), None] {
            let session = named_session(value);
            assert_eq!(evaluate_template(&tpl, &session), value.unwrap_or_default());
            assert_eq!(
                evaluate_template_bounded(&tpl, &session, 100),
                value
                    .map(str::to_string)
                    .ok_or(TemplateEvaluationError::MissingVariable)
            );
        }
    }
}

#[test]
fn intrinsic_variables_preserve_current_and_original_values() {
    let mut session = MockPluginSession::new();
    session.expect_get_path().return_const("/current".to_string());
    session.expect_get_uri().return_const("/current?a=1".to_string());
    session.expect_get_request_path().return_const("/original".to_string());
    session
        .expect_get_request_uri()
        .return_const("/original?b=2".to_string());
    session.expect_get_method().return_const("POST".to_string());
    session.expect_remote_ip().return_const("192.0.2.1".to_string());
    for (name, expected) in [
        ("path", "/current"),
        ("uri", "/current?a=1"),
        ("requestPath", "/original"),
        ("requestUri", "/original?b=2"),
        ("method", "POST"),
        ("remoteIp", "192.0.2.1"),
    ] {
        assert_eq!(
            evaluate_template_bounded(&compile(&format!("${{{name}}}")), &session, 100).unwrap(),
            expected
        );
    }
}

#[test]
fn unavailable_remote_ip_uses_consumer_missing_policy() {
    let mut session = MockPluginSession::new();
    session.expect_remote_ip().return_const(String::new());
    let tpl = compile("${remoteIp}");
    assert_eq!(evaluate_template(&tpl, &session), "");
    assert_eq!(
        evaluate_template_bounded(&tpl, &session, 63),
        Err(TemplateEvaluationError::MissingVariable)
    );
}

#[test]
fn secret_capability_provenance_and_debug_are_explicit() {
    let tpl = compile("Bearer ${secretRef:ns/token:key}");
    let session = MockPluginSession::new();
    let secrets = ResolvedSecrets(std::collections::BTreeMap::from([(
        "ns/token:key".to_string(),
        "sentinel-secret".to_string(),
    )]));
    assert_eq!(
        render_template(&tpl, &session, RenderPolicy::EMPTY, SecretCapability::Denied).unwrap_err(),
        TemplateEvaluationError::SecretNotAllowed
    );
    for map in [Some(&secrets), None] {
        let rendered = render_template(&tpl, &session, RenderPolicy::EMPTY, SecretCapability::Resolved(map)).unwrap();
        assert!(!format!("{rendered:?}").contains("sentinel-secret"));
        let (value, secret) = rendered.into_parts();
        assert!(secret);
        assert_eq!(
            value,
            if map.is_some() {
                "Bearer sentinel-secret"
            } else {
                "Bearer "
            }
        );
    }
    let strict = RenderPolicy {
        missing: MissingValuePolicy::Error,
        ..RenderPolicy::EMPTY
    };
    assert_eq!(
        render_template(&tpl, &session, strict, SecretCapability::Resolved(None)).unwrap_err(),
        TemplateEvaluationError::MissingVariable
    );
    let empty = ResolvedSecrets(std::collections::BTreeMap::from([(
        "ns/token:key".to_string(),
        String::new(),
    )]));
    assert_eq!(
        render_template(&tpl, &session, strict, SecretCapability::Resolved(Some(&empty)))
            .unwrap()
            .into_parts(),
        ("Bearer ".into(), true)
    );
    assert_eq!(
        render_template(
            &compile("plain"),
            &session,
            RenderPolicy::EMPTY,
            SecretCapability::Denied
        )
        .unwrap()
        .into_parts(),
        ("plain".into(), false)
    );
}

#[test]
fn budget_checks_literals_utf8_aggregate_and_overflow() {
    let session = named_session(Some("é"));
    for (raw, cap, expected) in [
        ("", 0, Some("")),
        ("x", 0, None),
        ("é", 2, Some("é")),
        ("é", 1, None),
        ("${ctx:x}${ctx:x}", 4, Some("éé")),
        ("${ctx:x}${ctx:x}", 3, None),
    ] {
        let result = render_template(
            &compile(raw),
            &session,
            RenderPolicy {
                max_bytes: Some(cap),
                ..RenderPolicy::EMPTY
            },
            SecretCapability::Denied,
        )
        .map(RenderedTemplate::into_string);
        assert_eq!(
            result,
            expected.map(str::to_string).ok_or(TemplateEvaluationError::TooLarge)
        );
    }
    assert_eq!(
        checked_output_len(usize::MAX, 1, None),
        Err(TemplateEvaluationError::TooLarge)
    );
    let mut out = Output {
        value: "abc".to_string(),
        max_bytes: Some(4),
    };
    assert_eq!(out.append("de"), Err(TemplateEvaluationError::TooLarge));
    assert_eq!(out.value, "abc");
}

#[test]
fn bounded_output_keeps_amortized_capacity_growth() {
    let mut out = Output {
        value: String::with_capacity(32_000),
        max_bytes: Some(128_000),
    };
    let segment = "a".repeat(128);
    let mut growths = 0;
    for _ in 0..1_000 {
        let before = out.value.capacity();
        out.append(&segment).unwrap();
        growths += usize::from(out.value.capacity() != before);
    }
    assert_eq!(out.value, "a".repeat(128_000));
    assert!(growths <= 2, "unexpected capacity growths: {growths}");
    assert!(out.value.capacity() <= 128_000);
    assert_eq!(out.append("x"), Err(TemplateEvaluationError::TooLarge));

    let mut out = Output {
        value: String::with_capacity(1),
        max_bytes: Some(1_000),
    };
    let mut growths = 0;
    for _ in 0..1_000 {
        let before = out.value.capacity();
        out.append_json("a").unwrap();
        growths += usize::from(out.value.capacity() != before);
    }
    assert!(growths <= 10, "unexpected JSON capacity growths: {growths}");
    assert_eq!(out.value, "a".repeat(1_000));
    assert!(out.value.capacity() <= 1_000);
}

#[test]
fn header_sensitivity_preserves_first_value_and_utf8_filtering() {
    let mut session = MockPluginSession::new();
    let mut headers = http::HeaderMap::new();
    let mut private = http::HeaderValue::from_static("private");
    private.set_sensitive(true);
    headers.append("x-private", private);
    headers.append("x-private", http::HeaderValue::from_static("ignored-second"));
    headers.insert("x-visible", http::HeaderValue::from_static("visible"));
    headers.insert("x-empty", http::HeaderValue::from_static(""));
    headers.insert("x-raw", http::HeaderValue::from_bytes(&[0xff]).unwrap());
    let mut opaque_sensitive = http::HeaderValue::from_bytes(&[0xff]).unwrap();
    opaque_sensitive.set_sensitive(true);
    headers.insert("x-private-raw", opaque_sensitive);
    session.expect_request_header_map().return_const(headers);
    for (name, expected, sensitive) in [
        ("x-private", "private", true),
        ("x-visible", "visible", false),
        ("x-empty", "", false),
        ("x-raw", "", false),
        ("x-private-raw", "", true),
        ("x-absent", "", false),
    ] {
        let tpl = compile(&format!("prefix-${{header:{name}}}"));
        let rendered = render_template(&tpl, &session, RenderPolicy::EMPTY, SecretCapability::Denied).unwrap();
        assert_eq!(rendered.into_parts(), (format!("prefix-{expected}"), sensitive));
    }
    assert_eq!(
        evaluate_template_bounded(&compile("${header:x-raw}"), &session, 100),
        Err(TemplateEvaluationError::MissingVariable)
    );
}

#[test]
fn json_escapes_only_variables_and_counts_expansion() {
    let value = "\"\\\n\r\t\u{0001}é${ctx:literal}";
    let mut session = MockPluginSession::new();
    session.expect_get_ctx_var().return_const(Some(value.to_string()));
    let tpl = compile(r#"{"value":"${ctx:x}"}"#);
    let escaped = serde_json::to_string(value).unwrap();
    let expected = format!("{{\"value\":{escaped}}}");
    for cap in [expected.len(), expected.len() - 1] {
        let result = render_template(
            &tpl,
            &session,
            RenderPolicy {
                escape: EscapeMode::JsonString,
                max_bytes: Some(cap),
                ..RenderPolicy::EMPTY
            },
            SecretCapability::Denied,
        )
        .map(RenderedTemplate::into_string);
        if cap == expected.len() {
            assert_eq!(result.unwrap(), expected);
        } else {
            assert_eq!(result, Err(TemplateEvaluationError::TooLarge));
        }
    }
    for (value, bytes) in [("\"", 2), ("\\", 2), ("\u{0001}", 6), ("€", 3)] {
        let mut out = Output {
            value: String::new(),
            max_bytes: Some(bytes - 1),
        };
        assert_eq!(out.append_json(value), Err(TemplateEvaluationError::TooLarge));
        assert!(out.value.is_empty());
    }
}

#[test]
fn test_evaluate_literal_template() {
    let compiled = compile("hello world");
    let session = MockPluginSession::new();
    assert_eq!(evaluate_template(&compiled, &session), "hello world");
}

#[test]
fn test_evaluate_header_variable() {
    let compiled = compile("${header:X-Tenant}");
    let mut session = MockPluginSession::new();
    session
        .expect_request_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::HeaderName::from_bytes(b"X-Tenant").unwrap(),
            http::HeaderValue::from_static("acme"),
        )]));
    assert_eq!(evaluate_template(&compiled, &session), "acme");
}

#[test]
fn test_evaluate_ctx_variable() {
    let compiled = compile("${ctx:tier}");
    let mut session = MockPluginSession::new();
    session
        .expect_get_ctx_var()
        .withf(|n| n == "tier")
        .return_const(Some("gold".to_string()));
    assert_eq!(evaluate_template(&compiled, &session), "gold");
}

#[test]
fn test_resolve_missing_variable_returns_empty() {
    let mut session = MockPluginSession::new();
    session.expect_request_header_map().return_const(http::HeaderMap::new());
    let value = resolve_var(
        &TemplateVarType::Header,
        Some("X-Absent"),
        TemplateContext::Request(&session),
        SecretCapability::Denied,
    )
    .unwrap()
    .value
    .unwrap_or_default();
    assert_eq!(value, "");
}

#[test]
fn test_resolve_present_variable() {
    let mut session = MockPluginSession::new();
    session
        .expect_get_query_param()
        .withf(|n| n == "id")
        .return_const(Some("42".to_string()));
    session
        .expect_get_query_param_ref()
        .withf(|n| n == "id")
        .returning(|_| Some("42"));
    let value = resolve_var(
        &TemplateVarType::Query,
        Some("id"),
        TemplateContext::Request(&session),
        SecretCapability::Denied,
    )
    .unwrap()
    .value
    .unwrap();
    assert_eq!(value, "42");
}

#[test]
fn test_multi_variable_template() {
    let compiled = compile("${header:X-User}@${ctx:domain}");
    let mut session = MockPluginSession::new();
    session
        .expect_request_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::HeaderName::from_bytes(b"X-User").unwrap(),
            http::HeaderValue::from_static("alice"),
        )]));
    session
        .expect_get_ctx_var()
        .withf(|n| n == "domain")
        .return_const(Some("example.com".to_string()));
    assert_eq!(evaluate_template(&compiled, &session), "alice@example.com");
}

#[test]
fn test_resolve_matched_http_route() {
    use crate::plugins::runtime::traits::session::RouteProto;
    let mut session = MockPluginSession::new();
    session
        .expect_matched_route_meta()
        .withf(|p, s| *p == RouteProto::Http && s == "name")
        .return_const(Some("route1".to_string()));
    let v = resolve_var(
        &TemplateVarType::MatchedHttpRoute,
        Some("name"),
        TemplateContext::Request(&session),
        SecretCapability::Denied,
    )
    .unwrap()
    .value
    .unwrap();
    assert_eq!(v, "route1");
}

#[test]
fn test_secret_ref_is_denied_in_session_facade() {
    let compiled = compile("x=${secretRef:ns/n:k}");
    let session = MockPluginSession::new();
    // Invalid artifacts cannot resolve Secrets or emit partial template output.
    assert_eq!(evaluate_template(&compiled, &session), "");
}

#[test]
fn test_template_injection_prevention() {
    // A resolved value that itself contains `${…}` must be emitted verbatim,
    // never re-parsed as a nested directive.
    let compiled = compile("${header:X-Inject}");
    let mut session = MockPluginSession::new();
    session
        .expect_request_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::HeaderName::from_bytes(b"X-Inject").unwrap(),
            http::HeaderValue::from_static("${header:Authorization}"),
        )]));
    assert_eq!(evaluate_template(&compiled, &session), "${header:Authorization}");
}

#[test]
fn bounded_template_rejects_missing_variable() {
    let compiled = compile("${header:X-Absent}");
    let mut session = MockPluginSession::new();
    session.expect_request_header_map().return_const(http::HeaderMap::new());
    assert_eq!(
        evaluate_template_bounded(&compiled, &session, 64),
        Err(TemplateEvaluationError::MissingVariable)
    );
}

#[test]
fn bounded_template_checks_before_append() {
    let compiled = compile("prefix-${header:X-Value}");
    let mut session = MockPluginSession::new();
    session
        .expect_request_header_map()
        .return_const(http::HeaderMap::from_iter([(
            http::HeaderName::from_bytes(b"X-Value").unwrap(),
            http::HeaderValue::from_static("value"),
        )]));
    assert_eq!(
        evaluate_template_bounded(&compiled, &session, 11),
        Err(TemplateEvaluationError::TooLarge)
    );
    assert_eq!(
        evaluate_template_bounded(&compiled, &session, 12).unwrap(),
        "prefix-value"
    );
}

struct TrustedValues;

impl TrustedTemplateVariables for TrustedValues {
    fn trusted_ctx_var(&self, key: &str) -> Option<String> {
        (key == "tenant").then(|| "tenant-a".to_string())
    }

    fn trusted_remote_ip(&self) -> &str {
        "192.0.2.9"
    }

    fn trusted_matched_route_meta(&self, _proto: RouteProto, selector: &str) -> Option<String> {
        (selector == "name").then(|| "route-a".to_string())
    }
}

#[test]
fn trusted_context_reads_trusted_values_without_request_capabilities() {
    let template = compile("${ctx:tenant}/${remoteIp}/${matchedHttpRoute:name}/${matchedGrpcRoute:name}");
    let rendered = render_template_in(
        &template,
        TemplateContext::Trusted(&TrustedValues),
        RenderPolicy::EMPTY,
        SecretCapability::Denied,
    )
    .unwrap();
    assert_eq!(
        rendered.into_parts(),
        ("tenant-a/192.0.2.9/route-a/route-a".into(), false)
    );
}

#[test]
fn trusted_context_rejects_request_variables_even_with_empty_missing_policy() {
    for variable in [
        "header:x",
        "query:x",
        "cookie:x",
        "path",
        "uri",
        "requestPath",
        "requestUri",
        "method",
    ] {
        for missing in [MissingValuePolicy::Empty, MissingValuePolicy::Error] {
            let result = render_template_in(
                &compile(&format!("${{{variable}}}")),
                TemplateContext::Trusted(&TrustedValues),
                RenderPolicy {
                    missing,
                    ..RenderPolicy::EMPTY
                },
                SecretCapability::Denied,
            );
            assert!(matches!(result, Err(TemplateEvaluationError::UnavailableVariable)));
        }
    }
}

#[test]
fn trusted_context_retains_explicit_secret_capability_and_sensitivity() {
    let template = compile("Bearer ${secretRef:ns/token:key}");
    let secrets = ResolvedSecrets(std::collections::BTreeMap::from([(
        "ns/token:key".into(),
        "credential".into(),
    )]));
    let rendered = render_template_in(
        &template,
        TemplateContext::Trusted(&TrustedValues),
        RenderPolicy::EMPTY,
        SecretCapability::Resolved(Some(&secrets)),
    )
    .unwrap();
    assert_eq!(
        format!("{rendered:?}"),
        "RenderedTemplate { value: \"[redacted]\", sensitive: true }"
    );
    assert_eq!(rendered.into_parts(), ("Bearer credential".into(), true));
    assert!(matches!(
        render_template_in(
            &template,
            TemplateContext::Trusted(&TrustedValues),
            RenderPolicy::EMPTY,
            SecretCapability::Denied,
        ),
        Err(TemplateEvaluationError::SecretNotAllowed)
    ));
}

#[test]
fn resolved_variables_borrow_existing_sources_until_rendered() {
    let mut session = named_session(Some("source"));
    session.expect_get_path().return_const("/live".to_owned());
    session.expect_get_request_path().return_const("/entry".to_owned());
    session.expect_get_method().return_const("POST".to_owned());
    session.expect_remote_ip().return_const("192.0.2.1".to_owned());
    let secrets = ResolvedSecrets(std::collections::BTreeMap::from([(
        "ns/key:value".into(),
        "secret-value".into(),
    )]));
    let sources = [
        (TemplateVarType::Cookie, Some("x"), session.get_cookie_ref("x").unwrap()),
        (
            TemplateVarType::Header,
            Some("x"),
            session.request_header_map()["x"].to_str().unwrap(),
        ),
        (
            TemplateVarType::Query,
            Some("x"),
            session.get_query_param_ref("x").unwrap(),
        ),
        (TemplateVarType::Path, None, session.get_path()),
        (TemplateVarType::RequestPath, None, session.get_request_path()),
        (TemplateVarType::Method, None, session.get_method()),
        (TemplateVarType::RemoteIp, None, session.remote_ip()),
        (
            TemplateVarType::SecretRef,
            Some("ns/key:value"),
            secrets.0["ns/key:value"].as_str(),
        ),
    ];
    for (kind, name, expected) in sources {
        let resolved = resolve_var(
            &kind,
            name,
            TemplateContext::Request(&session),
            SecretCapability::Resolved(Some(&secrets)),
        )
        .unwrap();
        let value = resolved.value.unwrap();
        assert!(matches!(value, Cow::Borrowed(_)));
        assert_eq!(value.as_ptr(), expected.as_ptr());
        assert_eq!(value, expected);
        assert_eq!(
            resolved.sensitive,
            matches!(kind, TemplateVarType::Cookie | TemplateVarType::SecretRef)
        );
    }
    for kind in [
        TemplateVarType::Ctx,
        TemplateVarType::MatchedHttpRoute,
        TemplateVarType::MatchedGrpcRoute,
    ] {
        let resolved = resolve_var(
            &kind,
            Some("x"),
            TemplateContext::Request(&session),
            SecretCapability::Denied,
        )
        .unwrap();
        assert!(matches!(resolved.value, Some(Cow::Owned(value)) if value == "source"));
    }
    let rendered = render_template(
        &compile("${header:x}/${secretRef:ns/key:value}"),
        &session,
        RenderPolicy::EMPTY,
        SecretCapability::Resolved(Some(&secrets)),
    )
    .unwrap();
    drop(session);
    drop(secrets);
    assert_eq!(rendered.into_parts(), ("source/secret-value".into(), true));
}
