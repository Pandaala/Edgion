use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;

#[test]
fn credentialed_expose_headers_validation_and_wire_values() {
    for credentials in [None, Some(false), Some(true)] {
        for names in [vec![], vec!["*"], vec!["X-Request-Id"], vec!["*", "X-Request-Id"]] {
            let config = HTTPCORSFilter {
                allow_origins: Some(vec!["https://app.example".into()]),
                allow_credentials: credentials,
                allow_methods: None,
                allow_headers: None,
                expose_headers: Some(names.iter().map(|name| name.to_string()).collect()),
                max_age: 5,
            };
            let result = GapiCorsFilter::try_new(config);
            if credentials == Some(true) && names.contains(&"*") {
                assert!(result.unwrap_err().contains("exposeHeaders"));
                continue;
            }
            let filter = result.unwrap();
            for preflight in [false, true] {
                let session = MockPluginSession::new();
                let headers = filter.build_cors_headers(&session, "https://app.example", preflight);
                let exposed = headers
                    .iter()
                    .find(|(name, _)| *name == "Access-Control-Expose-Headers");
                assert_eq!(
                    exposed.map(|(_, value)| value.as_ref()),
                    (!names.is_empty()).then(|| names.join(", ")).as_deref()
                );
            }
        }
    }
}

#[test]
fn max_age_default_is_emitted_and_explicit_zero_is_rejected() {
    let config: HTTPCORSFilter = serde_json::from_value(serde_json::json!({
        "allowOrigins": ["https://app.example"]
    }))
    .expect("default maxAge CORS fixture must deserialize");
    let filter = GapiCorsFilter::try_new(config).expect("default maxAge must be valid");
    let session = MockPluginSession::new();
    let headers = filter.build_cors_headers(&session, "https://app.example", true);
    assert!(headers
        .iter()
        .any(|(name, value)| *name == "Access-Control-Max-Age" && value == "5"));

    let zero: HTTPCORSFilter =
        serde_json::from_value(serde_json::json!({"maxAge": 0})).expect("zero maxAge CORS fixture must deserialize");
    assert!(GapiCorsFilter::try_new(zero)
        .unwrap_err()
        .contains("maxAge must be between 1"));
}

#[test]
fn test_origin_match_exact() {
    let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
        allow_origins: Some(vec!["https://app.example".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    })
    .expect("test CORS configuration must be valid");
    assert!(filter.is_origin_allowed("https://app.example"));
    assert!(!filter.is_origin_allowed("https://evil.com"));
    assert!(!filter.is_origin_allowed("http://app.example"));
}

#[test]
fn test_origin_match_wildcard() {
    let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
        allow_origins: Some(vec!["*".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    })
    .expect("test CORS configuration must be valid");
    assert!(filter.is_origin_allowed("https://anything.com"));
    assert!(filter.is_origin_allowed("http://localhost:3000"));
}

#[test]
fn test_origin_match_subdomain_wildcard() {
    let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
        allow_origins: Some(vec!["https://*.example.com".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    })
    .expect("test CORS configuration must be valid");
    assert!(filter.is_origin_allowed("https://app.example.com"));
    assert!(filter.is_origin_allowed("https://foo.bar.example.com"));
    assert!(!filter.is_origin_allowed("https://example.com"));
    assert!(!filter.is_origin_allowed("https://evil.com"));
}

#[test]
fn test_no_origins_rejects_all() {
    let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
        allow_origins: None,
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    })
    .expect("test CORS configuration must be valid");
    assert!(!filter.is_origin_allowed("https://anything.com"));
}

#[test]
fn maximum_header_set_stays_inline_and_preserves_order() {
    let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
        allow_origins: Some(vec!["https://app.example".into()]),
        allow_credentials: Some(true),
        allow_methods: Some(vec!["GET".into(), "POST".into()]),
        allow_headers: Some(vec!["Content-Type".into(), "X-Trace-Id".into()]),
        expose_headers: Some(vec!["X-Request-Id".into()]),
        max_age: 3600,
    })
    .expect("maximum-header CORS test configuration must be valid");
    let session = MockPluginSession::new();

    drop(filter.build_cors_headers(&session, "https://app.example", true));
    let (headers, allocations) = crate::test_allocation_counter::count_allocations(|| {
        filter.build_cors_headers(&session, "https://app.example", true)
    });
    assert_eq!(allocations, 0);

    assert_eq!(headers.len(), 6);
    assert!(!headers.spilled());
    assert_eq!(
        headers
            .iter()
            .map(|(name, value)| (*name, value.as_ref()))
            .collect::<Vec<_>>()
            .as_slice(),
        [
            ("Access-Control-Allow-Origin", "https://app.example"),
            ("Access-Control-Allow-Credentials", "true"),
            ("Access-Control-Allow-Methods", "GET, POST"),
            ("Access-Control-Allow-Headers", "Content-Type, X-Trace-Id"),
            ("Access-Control-Expose-Headers", "X-Request-Id"),
            ("Access-Control-Max-Age", "3600"),
        ]
        .as_slice()
    );
}

#[test]
fn test_split_origin_port() {
    let (host, port) = split_origin_port("https://example.com:8443").unwrap();
    assert_eq!(host, "https://example.com");
    assert_eq!(port, ":8443");

    let (host, port) = split_origin_port("https://example.com").unwrap();
    assert_eq!(host, "https://example.com");
    assert_eq!(port, "");

    let (host, port) = split_origin_port("http://localhost:3000").unwrap();
    assert_eq!(host, "http://localhost");
    assert_eq!(port, ":3000");
}

#[test]
fn test_v15_origin_validation_accepts_valid() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec![
            "https://example.com".to_string(),
            "https://example.com:8443".to_string(),
            "http://localhost:3000".to_string(),
            "https://*.example.com".to_string(),
        ]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    assert!(GapiCorsFilter::try_new(config).is_ok());

    let wildcard: HTTPCORSFilter = serde_json::from_value(serde_json::json!({"allowOrigins": ["*"]}))
        .expect("wildcard CORS fixture must deserialize");
    assert!(GapiCorsFilter::try_new(wildcard).is_ok());
}

#[test]
fn test_v15_origin_validation_rejects_path() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec!["https://example.com/path".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    let err = GapiCorsFilter::try_new(config).unwrap_err();
    assert!(err.contains("path component"), "got: {}", err);
}

#[test]
fn test_v15_origin_validation_rejects_trailing_slash() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec!["https://example.com/".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    assert!(GapiCorsFilter::try_new(config).is_err());
}

#[test]
fn test_v15_origin_validation_rejects_query() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec!["https://example.com?q=1".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    let err = GapiCorsFilter::try_new(config).unwrap_err();
    assert!(err.contains("query string"), "got: {}", err);
}

#[test]
fn test_v15_origin_validation_rejects_fragment() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec!["https://example.com#sec".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    let err = GapiCorsFilter::try_new(config).unwrap_err();
    assert!(err.contains("fragment"), "got: {}", err);
}

#[test]
fn test_v15_origin_validation_rejects_no_scheme() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec!["example.com".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    let err = GapiCorsFilter::try_new(config).unwrap_err();
    assert!(err.contains("scheme"), "got: {}", err);
}

#[test]
fn test_v15_origin_validation_rejects_bad_wildcard() {
    let config = HTTPCORSFilter {
        allow_origins: Some(vec!["https://exam*ple.com".to_string()]),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    };
    let err = GapiCorsFilter::try_new(config).unwrap_err();
    assert!(err.contains("wildcard"), "got: {}", err);
}

#[test]
fn prepared_values_preserve_static_and_reflected_output() {
    for credentials in [None, Some(false), Some(true)] {
        for values in [
            None,
            Some(vec![]),
            Some(vec!["*"]),
            Some(vec!["GET", "POST"]),
            Some(vec!["*", "GET"]),
        ] {
            for requested in [None, Some(""), Some("PATCH")] {
                for preflight in [false, true] {
                    let reflects =
                        credentials == Some(true) && values.as_ref().is_some_and(|values| values.contains(&"*"));
                    let config = HTTPCORSFilter {
                        allow_origins: Some(vec!["https://app.example".into()]),
                        allow_credentials: credentials,
                        allow_methods: values
                            .as_ref()
                            .map(|values| values.iter().map(|value| (*value).into()).collect()),
                        allow_headers: values
                            .as_ref()
                            .map(|values| values.iter().map(|value| (*value).into()).collect()),
                        expose_headers: None,
                        max_age: 17,
                    };
                    let result = GapiCorsFilter::try_new(config);
                    if values
                        .as_ref()
                        .is_some_and(|values| values.len() > 1 && values.contains(&"*"))
                    {
                        assert!(result.is_err());
                        continue;
                    }
                    let filter = result.unwrap();
                    let mut session = MockPluginSession::new();
                    session
                        .expect_header_value()
                        .times(if reflects { 4 } else { 0 })
                        .returning(move |name| {
                            assert!(matches!(
                                name,
                                "access-control-request-method" | "access-control-request-headers"
                            ));
                            requested.map(str::to_owned)
                        });
                    drop(filter.build_cors_headers(&session, "https://app.example", preflight));
                    let (headers, allocations) = crate::test_allocation_counter::count_allocations(|| {
                        filter.build_cors_headers(&session, "https://app.example", preflight)
                    });
                    assert_eq!(
                        allocations,
                        if reflects && requested.is_some_and(|value| !value.is_empty()) {
                            2
                        } else {
                            0
                        }
                    );
                    assert!(!headers.spilled());
                    let expected = if reflects {
                        requested.map(str::to_owned)
                    } else {
                        values
                            .as_ref()
                            .filter(|values| !values.is_empty())
                            .map(|values| values.join(", "))
                    };
                    for name in ["Access-Control-Allow-Methods", "Access-Control-Allow-Headers"] {
                        let actual = headers
                            .iter()
                            .find(|(key, _)| *key == name)
                            .map(|(_, value)| value.as_ref());
                        assert_eq!(actual, expected.as_deref());
                    }
                    assert_eq!(
                        headers
                            .iter()
                            .find(|(key, _)| *key == "Access-Control-Max-Age")
                            .map(|(_, value)| value.as_ref()),
                        preflight.then_some("17")
                    );
                }
            }
        }
    }
}

#[test]
fn wildcard_origin_uses_live_credential_presence() {
    for credentials in [None, Some(false), Some(true)] {
        for request_credential in [None, Some("cookie"), Some("authorization")] {
            let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
                allow_origins: Some(vec!["*".into()]),
                allow_credentials: credentials,
                allow_methods: None,
                allow_headers: None,
                expose_headers: None,
                max_age: 5,
            })
            .unwrap();
            let mut map = http::HeaderMap::new();
            if let Some(name) = request_credential {
                map.insert(name, http::HeaderValue::from_static(""));
            }
            let mut session = MockPluginSession::new();
            session
                .expect_request_header_map()
                .times(if credentials == Some(true) { 0..=0 } else { 2..=4 })
                .return_const(map);
            for preflight in [false, true] {
                let headers = filter.build_cors_headers(&session, "https://app.example", preflight);
                assert_eq!(
                    headers[0].1,
                    if credentials == Some(true) || request_credential.is_some() {
                        "https://app.example"
                    } else {
                        "*"
                    }
                );
                assert!(headers.iter().all(|(_, value)| matches!(value, Cow::Borrowed(_))));
            }
        }
    }
}

#[tokio::test]
async fn request_origin_is_borrowed_until_local_response_is_built() {
    for origins in [
        vec![],
        vec![b"".as_slice()],
        vec![b"https://app.example".as_slice()],
        vec![b"https://denied.example".as_slice()],
        vec![b"\xff".as_slice()],
        vec![b"\xff".as_slice(), b"https://app.example".as_slice()],
        vec![b"https://denied.example".as_slice(), b"https://app.example".as_slice()],
        vec![b"https://app.example".as_slice(), b"https://denied.example".as_slice()],
    ] {
        for method in ["GET", "OPTIONS"] {
            for requested_method in [
                None,
                Some(b"\xff".as_slice()),
                Some(b"".as_slice()),
                Some(b"POST".as_slice()),
            ] {
                let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
                    allow_origins: Some(vec!["https://app.example".into()]),
                    allow_credentials: Some(true),
                    allow_methods: Some(vec!["POST".into()]),
                    allow_headers: None,
                    expose_headers: None,
                    max_age: 5,
                })
                .unwrap();
                let mut map = http::HeaderMap::new();
                for origin in &origins {
                    map.append("origin", http::HeaderValue::from_bytes(origin).unwrap());
                }
                if let Some(value) = requested_method {
                    map.insert(
                        "access-control-request-method",
                        http::HeaderValue::from_bytes(value).unwrap(),
                    );
                }
                let first = map.get("origin").and_then(|value| value.to_str().ok());
                let present = first.is_some();
                let allowed = first == Some("https://app.example");
                let preflight = present
                    && method == "OPTIONS"
                    && map
                        .get("access-control-request-method")
                        .is_some_and(|value| value.to_str().is_ok());
                let mut session = MockPluginSession::new();
                session.expect_request_header_map().return_const(map);
                session.expect_header_value().never();
                session
                    .expect_get_method()
                    .times(usize::from(present))
                    .return_const(method.to_owned());
                let mut sequence = mockall::Sequence::new();
                session
                    .expect_write_response_header()
                    .times(usize::from(preflight))
                    .in_sequence(&mut sequence)
                    .returning(move |response, end| {
                        assert_eq!(response.status.as_u16(), 204);
                        assert!(end);
                        assert_eq!(
                            response
                                .headers
                                .get("access-control-allow-origin")
                                .map(|value| value.to_str().unwrap()),
                            allowed.then_some("https://app.example")
                        );
                        assert_eq!(
                            response.headers.get("vary").map(|value| value.to_str().unwrap()),
                            allowed.then_some("Origin")
                        );
                        Ok(())
                    });
                session
                    .expect_write_response_body()
                    .times(usize::from(preflight))
                    .in_sequence(&mut sequence)
                    .returning(|body, end| {
                        assert!(end);
                        assert_eq!(body, Some(Bytes::new()));
                        Ok(())
                    });
                let mut log = PluginLog::new("GapiCorsFilter");
                let result = filter.run_request(&mut session, &mut log).await;
                assert_eq!(log.contains("origin match:deny"), present && !allowed);
                assert_eq!(log.contains("cors preflight deny"), preflight && !allowed);
                assert_eq!(log.contains("cors actual"), present && allowed && !preflight);
                if !present {
                    assert!(!log.contains("cors"));
                }
                assert_eq!(matches!(result, PluginRunningResult::ErrTerminateRequest), preflight);
                if !preflight {
                    assert!(matches!(result, PluginRunningResult::GoodNext));
                }
            }
        }
    }
}

#[test]
fn preflight_owned_reflections_preserve_borrowed_insertion_behavior() {
    fn session(reflected: Option<&str>) -> MockPluginSession {
        let mut session = MockPluginSession::new();
        let reflected = reflected.map(str::to_owned);
        session.expect_header_value().returning(move |name| {
            assert!(matches!(
                name,
                "access-control-request-method" | "access-control-request-headers"
            ));
            reflected.clone()
        });
        session
    }

    // Preserve the old borrowed insertion boundary as a differential reference.
    // Invalid mock values exercise ignored insertion failures, not wire admission.
    fn borrowed_response(filter: &GapiCorsFilter, session: &dyn PluginSession, origin: &str) -> ResponseHeader {
        let headers = filter.build_cors_headers(session, origin, true);
        let mut response = ResponseHeader::build(204, Some(headers.len() + 1)).unwrap();
        for (name, value) in &headers {
            let _ = response.insert_header(*name, value.as_ref());
        }
        let _ = response.append_header("Vary", "Origin");
        response
    }

    for reflect in [false, true] {
        let filter = GapiCorsFilter::try_new(HTTPCORSFilter {
            allow_origins: Some(vec!["https://app.example".into()]),
            allow_credentials: Some(true),
            allow_methods: Some(vec![if reflect { "*" } else { "GET" }.into()]),
            allow_headers: Some(vec![if reflect { "*" } else { "X-Test" }.into()]),
            expose_headers: Some(vec!["X-Result".into()]),
            max_age: 17,
        })
        .unwrap();
        for reflected in [None, Some(""), Some("PATCH"), Some("café"), Some("invalid\r\nvalue")] {
            for origin in ["https://app.example", "invalid\norigin"] {
                let expected = borrowed_response(&filter, &session(reflected), origin);
                let actual = filter.build_preflight_response(&session(reflected), origin).unwrap();
                assert_eq!(actual.status, expected.status);
                assert_eq!(actual.headers, expected.headers);
                let mut expected_wire = Vec::new();
                let mut actual_wire = Vec::new();
                expected.header_to_h1_wire(&mut expected_wire);
                actual.header_to_h1_wire(&mut actual_wire);
                assert_eq!(actual_wire, expected_wire);
                assert!(actual.headers.values().all(|value| !value.is_sensitive()));
                assert_eq!(actual.headers["vary"], "Origin");
                assert_eq!(actual.headers["access-control-max-age"], "17");
                if reflect && reflected.is_some_and(|value| value.contains('\r')) {
                    assert!(!actual.headers.contains_key("access-control-allow-methods"));
                    assert!(!actual.headers.contains_key("access-control-allow-headers"));
                }
            }
        }
    }
}

// Frozen pre-preparation algorithm: keep raw equality and permissive live-input
// handling as a differential oracle, not a new admission policy.
fn unprepared_origin_match(origin: &str, pattern: &str) -> bool {
    if pattern == "*" || origin == pattern {
        return true;
    }
    if !pattern.contains('*') {
        return false;
    }
    let Some((p_host, p_port)) = split_origin_port(pattern) else {
        return false;
    };
    let Some((o_host, o_port)) = split_origin_port(origin) else {
        return false;
    };
    if p_port != o_port {
        return false;
    }
    if let Some(suffix) = p_host.strip_prefix('*') {
        o_host.ends_with(suffix)
    } else if let Some(pos) = p_host.find('*') {
        o_host.starts_with(&p_host[..pos]) && o_host.ends_with(&p_host[pos + 1..])
    } else {
        false
    }
}

fn native_origin_config(patterns: Vec<String>) -> HTTPCORSFilter {
    HTTPCORSFilter {
        allow_origins: Some(patterns),
        allow_credentials: None,
        allow_methods: None,
        allow_headers: None,
        expose_headers: None,
        max_age: 5,
    }
}

#[test]
fn prepared_origins_preserve_raw_matching_and_bounded_configurations() {
    let longest = format!(
        "https://*.{}",
        ["a".repeat(60), "b".repeat(60), "c".repeat(60), "d".repeat(60)].join(".")
    );
    assert_eq!(longest.len(), 253);
    let patterns = [
        "*",
        "https://*",
        "http://*",
        "https://*.example.com",
        "https://*.example.com:443",
        "https://*.example.com:0443",
        "https://APP.example",
        "http://localhost:3000",
        longest.as_str(),
    ];
    let mut origins: Vec<String> = [
        "",
        "null",
        "https://",
        "https://:",
        "https://::",
        "https://app.example.com",
        "https://foo.bar.example.com",
        "https://example.com",
        "https://.example.com",
        "https://APP.example",
        "https://app.EXAMPLE.com",
        "HTTPS://app.example.com",
        "http://app.example.com",
        "https://app.example.com:",
        "https://app.example.com:443",
        "https://app.example.com:0443",
        "https://app.example.com:443:443",
        "https://app.example.com:abc",
        "https://app.example.com/path",
        "https://app.example.com?x",
        "https://café.example.com",
        "https://€",
        "prefix://https://app.example.com",
        "https://app.example.com\n",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    origins.extend(patterns.iter().map(|p| p.to_string()));
    origins.push(longest.replace('*', "a"));
    let mut lists: Vec<Vec<String>> = patterns.iter().map(|p| vec![p.to_string()]).collect();
    lists.push(Vec::new());
    lists.push(patterns[1..].iter().map(|p| p.to_string()).collect());
    for wildcard in [false, true] {
        let list: Vec<_> = (0..64)
            .map(|i| format!("https://{}s{i}.example", if wildcard { "*." } else { "" }))
            .collect();
        origins.extend(list.iter().map(|p| p.replace('*', "a.b")));
        lists.push(list);
    }
    for list in lists {
        let filter = GapiCorsFilter::try_new(native_origin_config(list.clone())).unwrap();
        let clone = filter.clone();
        for origin in &origins {
            let expected = list.iter().any(|p| unprepared_origin_match(origin, p));
            for owner in [&filter, &clone] {
                let (actual, allocations) =
                    crate::test_allocation_counter::count_allocations(|| owner.is_origin_allowed(origin));
                assert_eq!(actual, expected, "patterns={list:?}, origin={origin:?}");
                assert_eq!(allocations, 0);
            }
        }
    }
}

#[test]
fn prepared_origin_offsets_follow_owned_generations() {
    let source = "https://*.old.example:0443".to_owned();
    let ptr = source.as_ptr();
    let (old, cold) = crate::test_allocation_counter::count_allocation_stats(|| {
        GapiCorsFilter::try_new(native_origin_config(vec![source])).unwrap()
    });
    let PreparedCorsOrigin::Wildcard { source, .. } = &old.allow_origins[0] else {
        panic!("wildcard must be prepared")
    };
    assert_eq!(source.as_ptr(), ptr, "preparation moves the original allocation");
    eprintln!("native CORS: old entry size={}, prepared entry size={}, prepared vector capacity={}, source capacity={}, constructor allocations={}, requested bytes={}",
        std::mem::size_of::<String>(), std::mem::size_of::<PreparedCorsOrigin>(), old.allow_origins.capacity(), source.capacity(), cold.calls, cold.requested_bytes);
    let old = std::sync::Arc::new(old);
    let weak = std::sync::Arc::downgrade(&old);
    let in_flight = old.clone();
    let new = GapiCorsFilter::try_new(native_origin_config(vec!["https://*.new.example:443".into()])).unwrap();
    drop(old);
    assert!(in_flight.is_origin_allowed("https://a.old.example:0443"));
    assert!(!new.is_origin_allowed("https://a.old.example:0443"));
    assert!(new.is_origin_allowed("https://a.new.example:443"));
    drop(in_flight);
    assert!(weak.upgrade().is_none());
}

#[tokio::test]
async fn prepared_origins_recheck_live_header_in_each_phase() {
    let filter = GapiCorsFilter::try_new(native_origin_config(vec!["https://*.allowed.example".into()])).unwrap();
    for (request, response, allowed) in [
        ("https://a.allowed.example", "https://denied.example", false),
        ("https://denied.example", "https://b.allowed.example", true),
    ] {
        let mut session = MockPluginSession::new();
        let mut map = http::HeaderMap::new();
        map.insert("origin", request.parse().unwrap());
        session.expect_request_header_map().return_const(map);
        session.expect_get_method().return_const("GET".to_owned());
        session
            .expect_header_value()
            .withf(|name| name == "origin")
            .return_const(Some(response.to_owned()));
        session
            .expect_set_response_header()
            .times(usize::from(allowed))
            .returning(move |name, value| {
                assert_eq!(name, "Access-Control-Allow-Origin");
                assert_eq!(value, response);
                Ok(())
            });
        session
            .expect_append_response_header()
            .times(usize::from(allowed))
            .returning(|name, value| {
                assert_eq!((name, value), ("Vary", "Origin"));
                Ok(())
            });
        let mut log = PluginLog::new("GapiCorsFilter");
        assert_eq!(
            filter.run_request(&mut session, &mut log).await,
            PluginRunningResult::GoodNext
        );
        assert_eq!(
            filter.run_upstream_response_filter(&mut session, &mut log),
            PluginRunningResult::GoodNext
        );
    }
}
