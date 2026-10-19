use super::*;
use edgion_resources::resources::http_route::CookieConfig;
use tokio::io::AsyncWriteExt;

async fn session_with_request(request: &[u8]) -> Session {
    let (mut client, server) = tokio::io::duplex(4096);
    client.write_all(request).await.unwrap();
    drop(client);
    let mut session = Session::new_h1(Box::new(server));
    assert!(session.read_request().await.unwrap());
    session
}

#[tokio::test]
async fn extracts_session_cookie_from_later_field() {
    let session = session_with_request(
        b"GET / HTTP/1.1\r\nHost: test\r\nCookie: unrelated=first\r\nCookie: affinity=abc123.1060\r\n\r\n",
    )
    .await;
    let sp = SessionPersistence {
        session_name: Some("affinity".to_string()),
        absolute_timeout: None,
        persistence_type: None,
        cookie_config: None,
        strict: None,
    };
    let mut state = SessionPersistenceState::InjectNew;
    assert_eq!(sp_extract_token(&session, &mut state, Some(&sp), 1000), Some("abc123"));
    assert!(matches!(state, SessionPersistenceState::Inactive));
}

#[tokio::test]
async fn session_cookie_uses_first_match_within_and_across_fields() {
    let session = session_with_request(
            b"GET / HTTP/1.1\r\nHost: test\r\nCookie: unrelated=first; affinity=first; affinity=second\r\nCookie: affinity=third\r\n\r\n",
        )
        .await;
    assert_eq!(extract_token_from_cookie(&session, "affinity"), Some("first"));
}

#[tokio::test]
async fn session_token_extraction_borrows_headers_without_allocating() {
    for persistence_type in [SessionPersistenceType::Cookie, SessionPersistenceType::Header] {
        for (wire, expected) in [
            (None, None),
            (Some(""), Some("")),
            (Some("abc123"), Some("abc123")),
            (Some("abc123.1060"), Some("abc123")),
            (Some("abc123.1000"), None),
            (Some("abc123.999"), None),
            (Some("abc123.invalid"), Some("abc123.invalid")),
        ] {
            let header = wire.map_or_else(String::new, |wire| match persistence_type {
                SessionPersistenceType::Cookie => format!("Cookie: other=first; affinity={wire}\r\n"),
                SessionPersistenceType::Header => format!("affinity: {wire}\r\n"),
            });
            let request = format!("GET / HTTP/1.1\r\nHost: test\r\n{header}\r\n");
            let session = session_with_request(request.as_bytes()).await;
            let sp = SessionPersistence {
                session_name: Some("affinity".into()),
                persistence_type: Some(persistence_type.clone()),
                absolute_timeout: None,
                cookie_config: None,
                strict: None,
            };
            let original = extract_session_token(&session, &sp);
            let mut state = SessionPersistenceState::Inactive;
            let (token, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
                sp_extract_token(&session, &mut state, Some(&sp), 1000)
            });
            assert_eq!(token, expected);
            assert_eq!(stats.calls, 0);
            assert_eq!(stats.requested_bytes, 0);
            if let Some(token) = token {
                assert_eq!(token.as_ptr(), original.unwrap().as_ptr());
                assert!(matches!(state, SessionPersistenceState::Inactive));
            } else {
                assert!(matches!(state, SessionPersistenceState::InjectNew));
            }
        }
    }
}

#[test]
fn test_token_generation_determinism() {
    let addr = "10.0.0.1:8080";
    let t1 = generate_session_token(addr);
    let t2 = generate_session_token(addr);
    assert_eq!(t1, t2, "token generation must be deterministic");
    assert_eq!(t1.len(), 16, "token must be 16 hex chars (64-bit identity)");
    assert!(
        t1.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "token must be lowercase hex"
    );
}

#[test]
fn test_token_generation_different_addrs() {
    let t1 = generate_session_token("10.0.0.1:8080");
    let t2 = generate_session_token("10.0.0.2:8080");
    assert_ne!(t1, t2, "different addresses should produce different tokens");
}

#[test]
fn scoped_token_binds_direct_and_discovery_identity() {
    let addr = "10.0.0.1:8080";
    let first = generate_scoped_session_token(Some("edgion.io/EdgionBackend/ns/first\0core/Service/ns/shared"), addr);
    let other_direct =
        generate_scoped_session_token(Some("edgion.io/EdgionBackend/ns/second\0core/Service/ns/shared"), addr);
    let other_discovery =
        generate_scoped_session_token(Some("edgion.io/EdgionBackend/ns/first\0core/Service/ns/other"), addr);

    assert_ne!(first, other_direct);
    assert_ne!(first, other_discovery);
    assert_eq!(generate_scoped_session_token(None, addr), generate_session_token(addr));
}

/// Rule-scoped resolution walks backendRefs in declaration order and finds
/// the token in a LATER backendRef's Service — the core GEP-1619 fix: a
/// session bound to Service B must resolve even though weighted selection
/// might have picked Service A.
#[test]
fn test_sp_lookup_rule_scoped_across_services() {
    use crate::backends::discovery::endpoint_slice::get_roundrobin_store;
    use crate::lb::runtime_state::sp_affinity_get_or_build_key;
    use edgion_resources::constants::labels::k8s::SERVICE_NAME;
    use k8s_openapi::api::discovery::v1::{Endpoint, EndpointConditions, EndpointPort, EndpointSlice};
    use kube::api::ObjectMeta;
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::net::SocketAddr as StdSocketAddr;

    let slice = |service: &str, name: &str, addr: StdSocketAddr| EndpointSlice {
        metadata: ObjectMeta {
            namespace: Some("rule-scope-test".to_string()),
            name: Some(name.to_string()),
            labels: Some(BTreeMap::from([(SERVICE_NAME.to_string(), service.to_string())])),
            ..Default::default()
        },
        address_type: "IPv4".to_string(),
        endpoints: vec![Endpoint {
            addresses: vec![addr.ip().to_string()],
            conditions: Some(EndpointConditions {
                ready: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        }],
        ports: Some(vec![EndpointPort {
            port: Some(i32::from(addr.port())),
            ..Default::default()
        }]),
    };
    let a_addr: StdSocketAddr = "10.1.0.1:8080".parse().unwrap();
    let b_addr: StdSocketAddr = "10.2.0.1:9090".parse().unwrap();
    let svc_a = "rule-scope-test/svc-a";
    let svc_b = "rule-scope-test/svc-b";
    get_roundrobin_store().update_data_only(
        HashMap::from([
            (
                "rule-scope-test/svc-a-sp".to_string(),
                slice("svc-a", "svc-a-sp", a_addr),
            ),
            (
                "rule-scope-test/svc-b-sp".to_string(),
                slice("svc-b", "svc-b-sp", b_addr),
            ),
        ]),
        HashMap::new(),
        &HashSet::new(),
    );
    let section_a = crate::backends::service_section_key_from_port(Some(i32::from(a_addr.port()))).unwrap();
    let section_b = crate::backends::service_section_key_from_port(Some(i32::from(b_addr.port()))).unwrap();
    sp_affinity_get_or_build_key(svc_a, &section_a, || unreachable!());
    sp_affinity_get_or_build_key(svc_b, &section_b, || unreachable!());

    let refs = [
        ("svc-a", None::<&str>, Some(i32::from(a_addr.port()))),
        ("svc-b", None, Some(i32::from(b_addr.port()))),
    ];
    let token_b = generate_session_token(&b_addr.to_string());
    let hit = sp_lookup_rule_scoped(refs.iter().copied(), "rule-scope-test", &token_b);
    assert_eq!(hit, Some((1, b_addr)), "token bound to svc-b must resolve to index 1");
    let mut state = SessionPersistenceState::Inactive;
    let malformed_wire = format!("{token_b}.invalid");
    let malformed = sp_apply_extracted_token(Some(&malformed_wire), &mut state, 1000).unwrap();
    assert_eq!(
        sp_lookup_rule_scoped(refs.iter().copied(), "rule-scope-test", malformed),
        None,
        "a malformed expiry must not adopt a known live endpoint"
    );
    let indexed_refs = [
        (0, "svc-a", None::<&str>, Some(i32::from(a_addr.port()))),
        (2, "svc-b", None, Some(i32::from(b_addr.port()))),
    ];
    let indexed_hit = sp_lookup_rule_scoped_indexed(indexed_refs.into_iter(), "rule-scope-test", &token_b);
    assert_eq!(
        indexed_hit,
        Some((2, b_addr)),
        "typed filtering must retain the raw backendRef slot"
    );

    let token_a = generate_session_token(&a_addr.to_string());
    let hit = sp_lookup_rule_scoped(refs.iter().copied(), "rule-scope-test", &token_a);
    assert_eq!(hit, Some((0, a_addr)), "token bound to svc-a must resolve to index 0");

    // A token minted for an endpoint outside this rule's backendRefs must
    // miss (rule scoping / stale-endpoint fallback).
    let foreign = generate_session_token("10.9.9.9:7070");
    let miss = sp_lookup_rule_scoped(refs.iter().copied(), "rule-scope-test", &foreign);
    assert_eq!(miss, None, "foreign token must not resolve within this rule");

    get_roundrobin_store().update_data_only(
        HashMap::new(),
        HashMap::new(),
        &HashSet::from([
            "rule-scope-test/svc-a-sp".to_string(),
            "rule-scope-test/svc-b-sp".to_string(),
        ]),
    );
}

#[test]
fn test_build_wire_token_optional_expiry() {
    // No ttl -> bare addrhash (unchanged behavior).
    assert_eq!(
        build_wire_token("1a2b3c4d5e6f7a8b".into(), 1000, None),
        "1a2b3c4d5e6f7a8b"
    );
    // ttl -> addrhash.<now+ttl>
    assert_eq!(
        build_wire_token("1a2b3c4d5e6f7a8b".into(), 1000, Some(60)),
        "1a2b3c4d5e6f7a8b.1060"
    );
    assert_eq!(build_wire_token("hash".into(), 1000, Some(0)), "hash.1000");
    assert_eq!(
        build_wire_token("hash".into(), u64::MAX - 1, Some(60)),
        "hash.18446744073709551615"
    );
}

#[test]
fn bare_wire_token_reuses_generated_hash_without_allocating() {
    let hash = generate_session_token("10.0.0.1:8080");
    let original = hash.as_ptr();
    let expected = hash.clone();
    let (token, stats) = crate::test_allocation_counter::count_allocation_stats(|| build_wire_token(hash, 1000, None));
    assert_eq!(token, expected);
    assert_eq!(token.as_ptr(), original);
    assert_eq!(stats.calls, 0);
    assert_eq!(stats.requested_bytes, 0);
}

#[test]
fn expiring_wire_token_preserves_bytes_with_exact_capacity() {
    let mut times = vec![(0_u64, 0_u64), (u64::MAX - 1, 60), (u64::MAX, u64::MAX)];
    for power in 0..=19 {
        let value = 10_u64.pow(power);
        times.extend([(value - 1, 0), (value, 0), (value + 1, 0)]);
    }
    let mut measurements = Vec::new();
    for hash in [String::new(), "0123456789abcdef".to_owned(), "a".repeat(256)] {
        for &(now, ttl) in &times {
            let expected = format!("{hash}.{}", now.saturating_add(ttl));
            let input = hash.clone();
            let (actual, construction) =
                crate::test_allocation_counter::count_allocation_stats(|| build_wire_token(input, now, Some(ttl)));
            assert_eq!(actual, expected);
            let length = actual.len();
            let capacity = actual.capacity();
            let original = actual.as_ptr();
            let (header, conversion) =
                crate::test_allocation_counter::count_allocation_stats(|| http::HeaderValue::try_from(actual).unwrap());
            assert_eq!(header.as_bytes(), expected.as_bytes());
            assert_eq!(header.as_bytes().as_ptr(), original);
            measurements.push((length, capacity, construction, conversion));
        }
    }
    for (length, capacity, construction, conversion) in measurements {
        assert_eq!(capacity, length);
        assert_eq!(construction.calls, 1);
        assert_eq!(construction.requested_bytes, length);
        assert_eq!(conversion.calls, 0);
        assert_eq!(conversion.requested_bytes, 0);
    }
}

#[test]
fn generated_bare_wire_token_moves_into_header_without_allocating() {
    let token = build_wire_token(generate_session_token("10.0.0.1:8080"), 1000, None);
    let expected = token.clone();
    let original = token.as_ptr();
    let (header, stats) =
        crate::test_allocation_counter::count_allocation_stats(|| http::HeaderValue::try_from(token).unwrap());
    assert_eq!(header.as_bytes(), expected.as_bytes());
    assert_eq!(header.as_bytes().as_ptr(), original);
    assert_eq!(stats.calls, 0);
    assert_eq!(stats.requested_bytes, 0);
}

#[test]
fn test_parse_wire_token() {
    assert_eq!(parse_wire_token("abc123"), ("abc123", None));
    assert_eq!(parse_wire_token("abc123.1060"), ("abc123", Some(1060)));
    assert_eq!(parse_wire_token("abc123.xyz"), ("abc123.xyz", None));
}

#[test]
fn malformed_expiry_preserves_whole_lookup_token() {
    for token in [
        "abc123.xyz",
        "abc123.",
        "abc123.18446744073709551616",
        "abc123.1000.2000",
    ] {
        let mut state = SessionPersistenceState::Inactive;
        assert_eq!(parse_wire_token(token), (token, None));
        assert_eq!(sp_apply_extracted_token(Some(token), &mut state, 1000), Some(token));
    }
}

#[test]
fn live_token_borrows_original_storage() {
    for wire in ["abc123", "abc123.1060"] {
        let token = wire.to_string();
        let buffer = token.as_ptr();
        let mut state = SessionPersistenceState::Inactive;
        let extracted = sp_apply_extracted_token(Some(&token), &mut state, 1000).unwrap();
        assert_eq!(extracted, "abc123");
        assert_eq!(extracted.as_ptr(), buffer);
    }
}

#[test]
fn test_extract_token_expiry() {
    // Past expiry: now (1000) >= expiry (500) -> expired -> InjectNew, None.
    let mut st = SessionPersistenceState::Inactive;
    let got = sp_apply_extracted_token(Some("aaaabbbbccccdddd.500"), &mut st, 1000);
    assert_eq!(got, None);
    assert!(matches!(st, SessionPersistenceState::InjectNew));

    let mut boundary_state = SessionPersistenceState::Inactive;
    assert_eq!(
        sp_apply_extracted_token(Some("aaaabbbbccccdddd.1000"), &mut boundary_state, 1000),
        None
    );
    assert!(matches!(boundary_state, SessionPersistenceState::InjectNew));

    // Future expiry: live -> Inactive, returns bare addrhash (expiry stripped).
    let mut st2 = SessionPersistenceState::Inactive;
    let got2 = sp_apply_extracted_token(Some("aaaabbbbccccdddd.5000"), &mut st2, 1000);
    assert_eq!(got2, Some("aaaabbbbccccdddd"));
    assert!(matches!(st2, SessionPersistenceState::Inactive));

    // No expiry component: live forever -> Inactive, bare hash.
    let mut st3 = SessionPersistenceState::Inactive;
    let got3 = sp_apply_extracted_token(Some("aaaabbbbccccdddd"), &mut st3, 1000);
    assert_eq!(got3, Some("aaaabbbbccccdddd"));
    assert!(matches!(st3, SessionPersistenceState::Inactive));

    // Absent token -> InjectNew, None.
    let mut st4 = SessionPersistenceState::Inactive;
    let got4 = sp_apply_extracted_token(None, &mut st4, 1000);
    assert_eq!(got4, None);
    assert!(matches!(st4, SessionPersistenceState::InjectNew));
}

#[test]
fn test_default_session_name() {
    let sp = SessionPersistence {
        session_name: None,
        absolute_timeout: None,
        persistence_type: None,
        cookie_config: None,
        strict: None,
    };
    assert_eq!(get_session_name(&sp), "edgion-session-affinity");
}

#[test]
fn test_custom_session_name() {
    let sp = SessionPersistence {
        session_name: Some("my-session".to_string()),
        absolute_timeout: None,
        persistence_type: None,
        cookie_config: None,
        strict: None,
    };
    assert_eq!(get_session_name(&sp), "my-session");
}

#[test]
fn test_default_persistence_type() {
    let sp = SessionPersistence {
        session_name: None,
        absolute_timeout: None,
        persistence_type: None,
        cookie_config: None,
        strict: None,
    };
    assert_eq!(get_persistence_type(&sp), SessionPersistenceType::Cookie);
}

#[test]
fn test_header_persistence_type() {
    let sp = SessionPersistence {
        session_name: None,
        absolute_timeout: None,
        persistence_type: Some(SessionPersistenceType::Header),
        cookie_config: None,
        strict: None,
    };
    assert_eq!(get_persistence_type(&sp), SessionPersistenceType::Header);
}

#[test]
fn test_build_set_cookie_session_lifetime() {
    let sp = SessionPersistence {
        session_name: None,
        absolute_timeout: Some("1h".to_string()),
        persistence_type: None,
        cookie_config: Some(CookieConfig {
            lifetime_type: Some(CookieLifetimeType::Session),
        }),
        strict: None,
    };
    // Session lifetime: no Max-Age even though a ttl is supplied.
    let cookie = build_set_cookie_value(&sp, "abcd1234", false, Some(3600));
    assert_eq!(
        cookie,
        "edgion-session-affinity=abcd1234; Path=/; HttpOnly; SameSite=Lax"
    );
    assert!(!cookie.contains("Max-Age"), "Session lifetime should not have Max-Age");
}

#[test]
fn test_build_set_cookie_permanent_lifetime() {
    let sp = SessionPersistence {
        session_name: Some("my-sess".to_string()),
        absolute_timeout: Some("24h".to_string()),
        persistence_type: None,
        cookie_config: Some(CookieConfig {
            lifetime_type: Some(CookieLifetimeType::Permanent),
        }),
        strict: None,
    };
    let cookie = build_set_cookie_value(&sp, "abcd1234", false, Some(86400));
    assert_eq!(
        cookie,
        "my-sess=abcd1234; Path=/; Max-Age=86400; HttpOnly; SameSite=Lax"
    );
    for (ttl, expected) in [
        (
            Some(0),
            "my-sess=abcd1234; Path=/; Max-Age=0; HttpOnly; SameSite=Lax; Secure",
        ),
        (
            Some(u64::MAX),
            "my-sess=abcd1234; Path=/; Max-Age=18446744073709551615; HttpOnly; SameSite=Lax; Secure",
        ),
        (None, "my-sess=abcd1234; Path=/; HttpOnly; SameSite=Lax; Secure"),
    ] {
        assert_eq!(build_set_cookie_value(&sp, "abcd1234", true, ttl), expected);
    }
}

#[test]
fn test_build_set_cookie_no_cookie_config() {
    let sp = SessionPersistence {
        session_name: None,
        absolute_timeout: Some("1h".to_string()),
        persistence_type: None,
        cookie_config: None,
        strict: None,
    };
    let cookie = build_set_cookie_value(&sp, "token123", false, Some(3600));
    // No cookie_config means not permanent, so no Max-Age
    assert!(!cookie.contains("Max-Age"));
}

#[test]
fn test_build_set_cookie_secure_flag_when_tls() {
    let sp = SessionPersistence {
        session_name: None,
        absolute_timeout: None,
        persistence_type: None,
        cookie_config: None,
        strict: None,
    };
    let cookie = build_set_cookie_value(&sp, "abcd1234", true, None);
    assert_eq!(
        cookie,
        "edgion-session-affinity=abcd1234; Path=/; HttpOnly; SameSite=Lax; Secure"
    );
}

#[test]
fn typed_sticky_lookup_skips_invalid_denied_slots_without_allocating() {
    let refs = serde_json::json!([
        {"name":"wrong","group":"core","port":80},
        {"name":"denied","port":80,"refDenied":{"targetNamespace":"default","targetName":"denied"}},
        {"name":"api","port":80,"weight":0},
        {"name":"api","port":443,"weight":3},
        {"name":"missing-port"}
    ]);
    let http: edgion_resources::HTTPRoute = serde_json::from_value(
        serde_json::json!({"metadata":{"name":"owner"},"spec":{"rules":[{"backendRefs":refs}]}}),
    )
    .unwrap();
    let grpc: edgion_resources::GRPCRoute = serde_json::from_value(
        serde_json::json!({"metadata":{"name":"owner"},"spec":{"rules":[{"backendRefs":refs}]}}),
    )
    .unwrap();
    let http = crate::routes::http::match_unit::HttpRouteRuleUnit::compile_http_rule(&http, 0).unwrap();
    let grpc = crate::routes::grpc::GrpcRouteRuleUnit::compile_grpc_rule(&grpc, 0).unwrap();
    let addr = "10.0.0.1:8443".parse().unwrap();
    for candidates in [
        http.backends
            .iter()
            .enumerate()
            .map(|(slot, b)| (slot, b.permitted_service()))
            .collect::<Vec<_>>(),
        grpc.backends
            .iter()
            .enumerate()
            .map(|(slot, b)| (slot, b.permitted_service()))
            .collect::<Vec<_>>(),
    ] {
        let mut slots = [usize::MAX; 2];
        let mut reads = 0;
        let (result, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            sp_lookup_typed_candidates(
                candidates.iter().filter_map(|(slot, target)| {
                    let target = target.as_ref()?;
                    Some(StickyCandidate {
                        slot: *slot,
                        service_key: target.key(),
                        section: target.section(),
                        scope: None,
                    })
                }),
                "token",
                |candidate, _| {
                    assert_eq!(candidate.service_key, "default/api");
                    slots[reads] = candidate.slot;
                    reads += 1;
                    (candidate.slot == 3).then_some(addr)
                },
            )
        });
        assert_eq!(reads, 2);
        assert_eq!(slots, [2, 3]);
        assert_eq!(result, Some((3, addr, None)));
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }
}

#[test]
fn cookie_builder_preserves_bytes_with_one_exact_allocation() {
    fn previous(sp: &SessionPersistence, token: &str, tls: bool, ttl: Option<u64>) -> String {
        let mut cookie = format!("{}={}; Path=/", get_session_name(sp), token);
        if sp.cookie_config.as_ref().and_then(|c| c.lifetime_type.as_ref()) == Some(&CookieLifetimeType::Permanent) {
            if let Some(secs) = ttl {
                let _ = write!(cookie, "; Max-Age={secs}");
            }
        }
        cookie.push_str("; HttpOnly; SameSite=Lax");
        if tls {
            cookie.push_str("; Secure");
        }
        cookie
    }
    let mut ttls = vec![None, Some(0), Some(u64::MAX)];
    for power in 0..=19 {
        let value = 10_u64.pow(power);
        ttls.extend([Some(value - 1), Some(value), Some(value + 1)]);
    }
    let names = [
        None,
        Some(String::new()),
        Some("affinity".to_owned()),
        Some("n".repeat(256)),
    ];
    let tokens = [String::new(), "abcd1234.1060".to_owned(), "t".repeat(256)];
    let mut allocations = Vec::new();
    for name in names {
        for lifetime in [
            None,
            Some(None),
            Some(Some(CookieLifetimeType::Session)),
            Some(Some(CookieLifetimeType::Permanent)),
        ] {
            let sp = SessionPersistence {
                session_name: name.clone(),
                absolute_timeout: None,
                persistence_type: None,
                cookie_config: lifetime.map(|lifetime_type| CookieConfig { lifetime_type }),
                strict: None,
            };
            for token in &tokens {
                for tls in [false, true] {
                    for ttl in &ttls {
                        let expected = previous(&sp, token, tls, *ttl);
                        let (actual, stats) = crate::test_allocation_counter::count_allocation_stats(|| {
                            build_set_cookie_value(&sp, token, tls, *ttl)
                        });
                        assert_eq!(actual, expected);
                        allocations.push((stats.calls, stats.requested_bytes, actual.len()));
                        assert_eq!(actual.capacity(), actual.len());
                        let original = actual.as_ptr();
                        let (header, conversion) = crate::test_allocation_counter::count_allocation_stats(|| {
                            http::HeaderValue::try_from(actual).unwrap()
                        });
                        assert_eq!(header.as_bytes(), expected.as_bytes());
                        assert_eq!(header.as_bytes().as_ptr(), original);
                        assert_eq!(conversion.calls, 0);
                        assert_eq!(conversion.requested_bytes, 0);
                    }
                }
            }
        }
    }
    for (calls, bytes, length) in allocations {
        assert_eq!(calls, 1, "cookie construction must not grow its buffer");
        assert_eq!(bytes, length, "reserve only the final cookie byte length");
    }
}

#[test]
fn cookie_header_owned_conversion_preserves_validation() {
    let sp = SessionPersistence {
        session_name: None,
        absolute_timeout: None,
        persistence_type: None,
        cookie_config: None,
        strict: None,
    };
    for token in [
        "plain",
        "with\ttab",
        "café",
        "bad\rvalue",
        "bad\nvalue",
        "bad\0value",
        "bad\u{7f}value",
    ] {
        let cookie = build_set_cookie_value(&sp, token, false, None);
        let borrowed = http::HeaderValue::try_from(&cookie);
        let owned = http::HeaderValue::try_from(cookie);
        match (borrowed, owned) {
            (Ok(borrowed), Ok(owned)) => assert_eq!(borrowed.as_bytes(), owned.as_bytes()),
            (Err(_), Err(_)) => assert!(token.contains(['\r', '\n', '\0', '\u{7f}'])),
            _ => panic!("owned and borrowed cookie conversion must agree"),
        }
    }
}
