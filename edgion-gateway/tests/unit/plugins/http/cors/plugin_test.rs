use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;

fn create_cors_config() -> CorsConfig {
    CorsConfig {
        allow_origins: "https://example.com".to_string(),
        allow_origins_by_regex: None,
        allow_methods: "GET,POST,PUT,DELETE".to_string(),
        allow_headers: "Content-Type,Authorization".to_string(),
        expose_headers: "".to_string(),
        max_age: Some(3600),
        allow_credentials: true,
        allow_private_network: false,
        preflight_continue: false,
        timing_allow_origins: None,
        timing_allow_origins_by_regex: None,
        origins_cache: None,
        compiled_origins_regex: None,
        compiled_timing_regex: None,
    }
}

#[test]
fn plugin_name_is_static_across_configs() {
    let first = Cors::new(&create_cors_config());
    let mut second_config = create_cors_config();
    second_config.allow_origins = "https://other.example.com".to_string();
    let second = Cors::new(&second_config);

    let first_name = RequestFilter::name(&first);
    let second_name = RequestFilter::name(&second);
    assert_eq!(first_name, "Cors");
    assert_eq!(second_name, "Cors");
    assert!(std::ptr::eq(first_name, second_name));
}

#[test]
fn maximum_header_set_stays_inline_and_preserves_order() {
    let mut config = create_cors_config();
    config.expose_headers = "X-Trace-Id,X-RateLimit-Remaining".to_string();
    config.allow_private_network = true;
    config.timing_allow_origins = Some("https://example.com".to_string());
    let cors = Cors::new(&config);
    let mut session = MockPluginSession::new();
    setup_header_mocks(
        &mut session,
        &[
            ("origin", "https://example.com"),
            ("access-control-request-headers", "X-Requested"),
            ("access-control-request-private-network", "true"),
        ],
    );

    let headers = cors.build_cors_header_pairs(&mut session, "https://example.com", true);

    assert_eq!(headers.len(), 8);
    assert!(!headers.spilled());
    let emitted: Vec<(&str, &str)> = headers.iter().map(|(name, value)| (*name, &**value)).collect();
    assert_eq!(
        emitted.as_slice(),
        [
            ("Access-Control-Allow-Origin", "https://example.com"),
            ("Access-Control-Allow-Methods", "GET,POST,PUT,DELETE"),
            ("Access-Control-Allow-Headers", "Content-Type,Authorization"),
            ("Access-Control-Expose-Headers", "X-Trace-Id,X-RateLimit-Remaining"),
            ("Access-Control-Allow-Credentials", "true"),
            ("Access-Control-Max-Age", "3600"),
            ("Timing-Allow-Origin", "https://example.com"),
            ("Access-Control-Allow-Private-Network", "true"),
        ]
        .as_slice()
    );
}

#[tokio::test]
async fn test_origin_not_allowed() {
    let config = create_cors_config();
    let cors = Cors::new(&config);
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("Cors");

    mock_session.expect_get_method().return_const("GET".to_string());
    let mut headers = http::HeaderMap::new();
    headers.insert("origin", http::HeaderValue::from_static("https://evil.com"));
    mock_session.expect_request_header_map().times(1).return_const(headers);

    let result = cors.run_request(&mut mock_session, &mut plugin_log).await;

    assert_eq!(result, PluginRunningResult::GoodNext);
    assert!(plugin_log.contains("origin match:deny"));
}

#[test]
fn test_select_acao_value_standard_wildcard_returns_star() {
    let mut config = create_cors_config();
    config.allow_origins = "*".to_string();
    let cors = Cors::new(&config);

    assert!(matches!(
        cors.select_acao_value("https://any.example.com"),
        Cow::Borrowed("*")
    ));
}

#[test]
fn test_select_acao_value_force_wildcard_echoes_origin() {
    let mut config = create_cors_config();
    config.allow_origins = "**".to_string();
    let cors = Cors::new(&config);

    assert_eq!(
        cors.select_acao_value("https://any.example.com"),
        "https://any.example.com"
    );
}

#[test]
fn test_select_acao_value_specific_origin_echoes_origin() {
    let config = create_cors_config();
    let cors = Cors::new(&config);

    assert_eq!(cors.select_acao_value("https://example.com"), "https://example.com");
}

/// Run a simple (non-preflight) GET through `Cors` with the given origin and
/// return whether the plugin logged an allow decision. Mocks the response-header
/// sinks so the allowed path can complete. Mirrors the deserialization shape:
/// the caller must leave `compiled_origins_regex` / `origins_cache` as `None` so
/// the test exercises `Cors::new`'s `validate_and_init` call, not a pre-built config.
async fn origin_allowed_via_new(config: &CorsConfig, origin: &'static str) -> bool {
    let cors = Cors::new(config);
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("Cors");

    mock_session.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock_session, &[("origin", origin)]);
    mock_session.expect_set_response_header().returning(|_, _| Ok(()));
    mock_session.expect_append_response_header().returning(|_, _| Ok(()));

    let _ = cors.run_request(&mut mock_session, &mut plugin_log).await;
    plugin_log.contains("origin match:allow")
}

// Regression: `Cors::new` must call `validate_and_init` so the `#[serde(skip)]`
// matchers are compiled. Each config below is built as deserialization yields it
// (compiled fields `None`); using `CorsConfig::new` would pre-build the matchers
// and mask the bug.

#[tokio::test]
async fn test_new_compiles_regex_origin_allowed() {
    let mut config = create_cors_config();
    config.allow_credentials = false; // regex + credentials is rejected by validate_and_init
    config.allow_origins = "https://exact.example.com".to_string();
    config.allow_origins_by_regex = Some(vec![r"https://.*\.regex\.example\.com".to_string()]);

    assert!(
        origin_allowed_via_new(&config, "https://api.regex.example.com").await,
        "regex-matched origin should be allowed after Cors::new compiles allow_origins_by_regex"
    );
}

#[tokio::test]
async fn test_new_compiles_comma_separated_list() {
    let mut config = create_cors_config();
    config.allow_credentials = false;
    config.allow_origins = "https://a.example.com,https://b.example.com".to_string();

    assert!(
        origin_allowed_via_new(&config, "https://b.example.com").await,
        "second origin in a comma-separated list should be allowed via origins_cache"
    );
}

#[tokio::test]
async fn test_new_compiles_subdomain_wildcard() {
    let mut config = create_cors_config();
    config.allow_credentials = false;
    config.allow_origins = "*.example.com".to_string();

    assert!(
        origin_allowed_via_new(&config, "https://tenant.example.com").await,
        "subdomain-wildcard origin should be allowed via the compiled wildcard regex"
    );
}

/// Run a simple (non-preflight) GET with an allowed origin and return the
/// response headers the plugin queued through `set_response_header`.
async fn simple_request_queued_headers(config: &CorsConfig) -> Vec<(String, String)> {
    let queued: Arc<parking_lot::Mutex<Vec<(String, String)>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let cors = Cors::new(config);
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("Cors");

    mock_session.expect_get_method().return_const("GET".to_string());
    setup_header_mocks(&mut mock_session, &[("origin", "https://example.com")]);
    let sink = queued.clone();
    mock_session.expect_set_response_header().returning(move |name, value| {
        sink.lock().push((name.to_ascii_lowercase(), value.to_string()));
        Ok(())
    });
    mock_session.expect_append_response_header().returning(|_, _| Ok(()));

    let result = cors.run_request(&mut mock_session, &mut plugin_log).await;
    assert_eq!(result, PluginRunningResult::GoodNext);
    let headers = queued.lock().clone();
    headers
}

// Regression (gw-plugin-acl-rl-03): `Access-Control-Allow-Methods` /
// `-Allow-Headers` are preflight-response-only per WHATWG Fetch 3.2.2/3.2.3.
// The actual-request response must carry only -Allow-Origin / -Allow-Credentials
// / -Expose-Headers.

#[tokio::test]
async fn test_actual_response_omits_preflight_only_headers() {
    let headers = simple_request_queued_headers(&create_cors_config()).await;
    let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();

    assert!(
        names.contains(&"access-control-allow-origin"),
        "actual response must still carry Access-Control-Allow-Origin, got {names:?}"
    );
    assert!(
        !names.contains(&"access-control-allow-methods"),
        "Access-Control-Allow-Methods is preflight-only, got {names:?}"
    );
    assert!(
        !names.contains(&"access-control-allow-headers"),
        "Access-Control-Allow-Headers is preflight-only, got {names:?}"
    );
}

#[tokio::test]
async fn test_actual_response_omits_wildcard_shorthand_headers() {
    // The `**` shorthands are the worst case: without the gate an actual
    // response gets a content-free `Access-Control-Allow-Headers: *` plus the
    // fully expanded 9-verb method list. `validate_and_init` rejects a `*`
    // shorthand together with credentials, so this config must disable them to
    // stay operator-admissible.
    let mut config = create_cors_config();
    config.allow_credentials = false;
    config.allow_methods = "**".to_string();
    config.allow_headers = "**".to_string();

    let headers = simple_request_queued_headers(&config).await;
    let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();

    // Positive control: the header vec really was built for this request, so the
    // absence assertions below cannot pass vacuously.
    assert!(
        names.contains(&"access-control-allow-origin"),
        "actual response must still carry Access-Control-Allow-Origin, got {headers:?}"
    );
    assert!(
        !names.contains(&"access-control-allow-methods"),
        "`**` allow_methods must not expand onto an actual response, got {headers:?}"
    );
    assert!(
        !names.contains(&"access-control-allow-headers"),
        "`**` allow_headers must not fall back to `*` on an actual response, got {headers:?}"
    );
}

#[tokio::test]
async fn test_preflight_response_keeps_methods_and_headers() {
    let config = create_cors_config();
    let cors = Cors::new(&config);
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("Cors");

    mock_session.expect_get_method().return_const("OPTIONS".to_string());
    setup_header_mocks(
        &mut mock_session,
        &[
            ("origin", "https://example.com"),
            ("access-control-request-method", "POST"),
        ],
    );

    let written: Arc<parking_lot::Mutex<Vec<(String, String)>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let sink = written.clone();
    mock_session.expect_write_response_header().returning(move |resp, _| {
        for (name, value) in resp.headers.iter() {
            sink.lock().push((
                name.as_str().to_ascii_lowercase(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            ));
        }
        Ok(())
    });
    mock_session.expect_write_response_body().returning(|_, _| Ok(()));

    let result = cors.run_request(&mut mock_session, &mut plugin_log).await;
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);

    let headers = written.lock().clone();
    let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        names.contains(&"access-control-allow-methods"),
        "preflight response must carry Access-Control-Allow-Methods, got {headers:?}"
    );
    assert!(
        names.contains(&"access-control-allow-headers"),
        "preflight response must carry Access-Control-Allow-Headers, got {headers:?}"
    );
    assert!(
        names.contains(&"access-control-max-age"),
        "preflight response must still carry Access-Control-Max-Age, got {headers:?}"
    );
}

#[tokio::test]
async fn test_no_origin_header_skip() {
    let config = create_cors_config();
    let cors = Cors::new(&config);
    let mut mock_session = MockPluginSession::new();
    let mut plugin_log = PluginLog::new("Cors");

    setup_header_mocks(&mut mock_session, &[]);

    let result = cors.run_request(&mut mock_session, &mut plugin_log).await;

    assert_eq!(result, PluginRunningResult::GoodNext);
    // No origin header, should skip CORS processing (no logs)
}

#[test]
fn origin_selection_copies_only_the_selected_accepted_value() {
    let cases = &[
        ("https://example.com", Some(b"https://denied.example".as_slice()), None),
        (
            "https://example.com",
            Some(b"https://example.com"),
            Some("https://example.com"),
        ),
        ("*", Some(b"https://example.com"), Some("*")),
        ("**", Some(b"https://example.com"), Some("https://example.com")),
        ("*", Some(b"null"), None),
        ("*", Some(b"\xff"), None),
        ("*", None, None),
    ];
    for &(policy, origin, expected) in cases {
        let mut config = create_cors_config();
        config.allow_origins = policy.to_string();
        config.allow_credentials = false;
        let cors = Cors::new(&config);
        let mut map = http::HeaderMap::new();
        if let Some(origin) = origin {
            map.insert("origin", http::HeaderValue::from_bytes(origin).unwrap());
        }
        let mut session = MockPluginSession::new();
        session.expect_request_header_map().return_const(map);
        session.expect_header_value().never();
        // Keep mock initialization outside the allocation measurement.
        assert_eq!(
            session
                .header_str("origin")
                .and_then(|origin| cors.get_validated_origin(origin))
                .as_deref(),
            expected
        );
        let (actual, calls) = crate::test_allocation_counter::count_allocations(|| {
            session
                .header_str("origin")
                .and_then(|origin| cors.get_validated_origin(origin))
        });
        assert_eq!(actual.as_deref(), expected);
        assert_eq!(calls, usize::from(expected.is_some_and(|value| value != "*")));
    }
}

#[test]
fn timing_and_private_network_decisions_borrow_header_inputs() {
    for (policy, expected_timing, allocations) in [
        (None, None, 0),
        (Some("https://denied.example"), None, 0),
        (Some("*"), Some("*"), 0),
        (Some("**"), Some("https://example.com"), 1),
        (Some("https://example.com"), Some("https://example.com"), 1),
    ] {
        for private_network in ["true", "false", "TRUE"] {
            let mut config = create_cors_config();
            config.timing_allow_origins = policy.map(str::to_owned);
            config.allow_private_network = true;
            let cors = Cors::new(&config);
            let mut map = http::HeaderMap::new();
            map.insert("origin", http::HeaderValue::from_static("https://example.com"));
            map.insert(
                "access-control-request-private-network",
                private_network.parse().unwrap(),
            );
            let mut session = MockPluginSession::new();
            session.expect_request_header_map().return_const(map);
            session.expect_header_value().never();
            drop(cors.build_cors_header_pairs(&mut session, "https://example.com", false));
            let (headers, calls) = crate::test_allocation_counter::count_allocations(|| {
                cors.build_cors_header_pairs(&mut session, "https://example.com", false)
            });
            assert_eq!(calls, allocations);
            assert!(!headers.spilled());
            let value = |name| {
                headers
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.as_ref())
            };
            assert_eq!(value("Timing-Allow-Origin"), expected_timing);
            assert_eq!(
                value("Access-Control-Allow-Private-Network"),
                (private_network == "true").then_some("true")
            );
        }
    }
}

#[test]
fn requested_headers_are_read_only_for_preflight_reflection() {
    for configured in ["*", "X-One,X-Two", "**"] {
        for requested in [None, Some(""), Some("X-Requested,X-Other")] {
            for preflight in [false, true] {
                let config = CorsConfig {
                    allow_headers: configured.to_string(),
                    allow_credentials: false,
                    max_age: None,
                    ..create_cors_config()
                };
                let cors = Cors::new(&config);
                let reflects = preflight && configured == "**";
                let mut session = MockPluginSession::new();
                session.expect_request_header_map().return_const(http::HeaderMap::new());
                session
                    .expect_header_value()
                    .times(if reflects { 2 } else { 0 })
                    .returning(move |name| {
                        assert_eq!(name, "access-control-request-headers");
                        requested.map(str::to_owned)
                    });
                drop(cors.build_cors_header_pairs(&mut session, "https://example.com", preflight));
                let (headers, calls) = crate::test_allocation_counter::count_allocations(|| {
                    cors.build_cors_header_pairs(&mut session, "https://example.com", preflight)
                });
                let expected = preflight.then_some(if configured == "**" {
                    requested.unwrap_or("*")
                } else {
                    configured
                });
                let actual = headers
                    .iter()
                    .find(|(name, _)| *name == "Access-Control-Allow-Headers")
                    .map(|(_, value)| value.as_ref());
                assert_eq!(actual, expected);
                assert_eq!(
                    calls,
                    usize::from(reflects && requested.is_some_and(|value| !value.is_empty()))
                );
                assert!(!headers.spilled());
            }
        }
    }
}

#[test]
fn preflight_header_transfer_preserves_wire_and_errors() {
    fn session(reflected: &str) -> MockPluginSession {
        let mut session = MockPluginSession::new();
        let mut headers = http::HeaderMap::new();
        headers.insert("origin", "https://example.com".parse().unwrap());
        session.expect_request_header_map().return_const(headers);
        let reflected = reflected.to_owned();
        session.expect_header_value().returning(move |name| {
            assert_eq!(name, "access-control-request-headers");
            Some(reflected.clone())
        });
        session
    }

    // Reference the previous borrowed insertion boundary, including Pingora's
    // validation errors. Invalid reflected values exercise defensive handling
    // through a synthetic session, not an accepted HTTP request on the wire.
    fn borrowed_response(
        cors: &Cors,
        session: &mut dyn PluginSession,
        origin: &str,
    ) -> CorsResult<Box<ResponseHeader>> {
        let headers = cors.build_cors_header_pairs(session, origin, true);
        let mut response = ResponseHeader::build(204, Some(headers.len() + 1)).map_err(|e| Box::new(e) as CorsError)?;
        for (name, value) in &headers {
            response
                .insert_header(*name, value.as_ref())
                .map_err(|e| Box::new(e) as CorsError)?;
        }
        if cors.config.should_add_vary_header() {
            response
                .append_header("Vary", "Origin")
                .map_err(|e| Box::new(e) as CorsError)?;
        }
        Ok(Box::new(response))
    }

    for reflected in ["X-Requested, X-Trace", "", "café", "invalid\r\nvalue"] {
        for wildcard in [false, true] {
            for max_age in [0, u32::MAX] {
                let mut config = create_cors_config();
                config.allow_headers = "**".to_owned();
                config.max_age = Some(max_age);
                config.timing_allow_origins = Some(if wildcard { "*" } else { "https://example.com" }.to_owned());
                if wildcard {
                    config.allow_origins = "*".to_owned();
                    config.allow_credentials = false;
                }
                let cors = Cors::new(&config);
                for origin in ["https://example.com", "invalid\norigin"] {
                    let old = borrowed_response(&cors, &mut session(reflected), origin);
                    let new = cors.build_preflight_response(&mut session(reflected), origin);
                    match (old, new) {
                        (Ok(old), Ok(new)) => {
                            assert_eq!(old.status, new.status);
                            assert_eq!(old.headers, new.headers);
                            let mut old_wire = Vec::new();
                            let mut new_wire = Vec::new();
                            old.header_to_h1_wire(&mut old_wire);
                            new.header_to_h1_wire(&mut new_wire);
                            assert_eq!(old_wire, new_wire);
                            assert!(new.headers.values().all(|value| !value.is_sensitive()));
                        }
                        (Err(old), Err(new)) => {
                            assert_eq!(old.to_string(), new.to_string());
                            assert_eq!(format!("{old:?}"), format!("{new:?}"));
                        }
                        _ => panic!("preflight header transfer changed success or failure"),
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn request_classification_preserves_text_header_presence_and_continue_policy() {
    let origins: &[Option<&[u8]>] = &[
        None,
        Some(b"\xff"),
        Some(b""),
        Some(b"https://denied.example"),
        Some(b"https://example.com"),
    ];
    let request_methods: &[Option<&[u8]>] = &[None, Some(b"\xff"), Some(b""), Some(b"POST")];
    for &origin in origins {
        for method in ["GET", "OPTIONS"] {
            for &request_method in request_methods {
                for preflight_continue in [false, true] {
                    let mut config = create_cors_config();
                    config.preflight_continue = preflight_continue;
                    let cors = Cors::new(&config);
                    let mut headers = http::HeaderMap::new();
                    if let Some(origin) = origin {
                        headers.insert("origin", http::HeaderValue::from_bytes(origin).unwrap());
                    }
                    if let Some(request_method) = request_method {
                        headers.insert(
                            "access-control-request-method",
                            http::HeaderValue::from_bytes(request_method).unwrap(),
                        );
                    }
                    let textual_origin = headers.get("origin").and_then(|value| value.to_str().ok());
                    let allowed = textual_origin == Some("https://example.com");
                    let preflight = textual_origin.is_some()
                        && method == "OPTIONS"
                        && headers
                            .get("access-control-request-method")
                            .is_some_and(|value| value.to_str().is_ok());
                    let terminates = preflight && (!allowed || !preflight_continue);
                    let mut session = MockPluginSession::new();
                    session
                        .expect_get_method()
                        .times(usize::from(textual_origin.is_some()))
                        .return_const(method.to_owned());
                    session.expect_request_header_map().return_const(headers);
                    session.expect_set_response_header().returning(|_, _| Ok(()));
                    session.expect_append_response_header().returning(|_, _| Ok(()));
                    session
                        .expect_write_response_header()
                        .times(usize::from(terminates))
                        .returning(move |response, end| {
                            assert_eq!(response.status.as_u16(), 204);
                            assert!(end);
                            assert_eq!(response.headers.contains_key("access-control-allow-origin"), allowed);
                            Ok(())
                        });
                    session
                        .expect_write_response_body()
                        .times(usize::from(terminates))
                        .returning(|body, end| {
                            assert!(end);
                            assert!(body.is_some_and(|body| body.is_empty()));
                            Ok(())
                        });
                    let mut log = PluginLog::new("Cors");
                    let result = cors.run_request(&mut session, &mut log).await;
                    assert_eq!(
                        result,
                        if terminates {
                            PluginRunningResult::ErrTerminateRequest
                        } else {
                            PluginRunningResult::GoodNext
                        },
                        "origin={origin:?}, method={method}, request_method={request_method:?}, continue={preflight_continue}",
                    );
                    assert_eq!(log.contains("cors preflight"), preflight);
                    assert_eq!(log.contains("cors actual"), allowed && !preflight);
                    assert_eq!(log.contains("origin match:allow"), allowed);
                }
            }
        }
    }
}
