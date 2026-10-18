use super::*;
use crate::ctx::EdgionHttpContext;
use crate::plugins::runtime::traits::session::MockPluginSession;
use crate::plugins::runtime::traits::test_helpers::setup_header_mocks;
use edgion_resources::resources::{HTTPPathModifier, HTTPPathModifierType};

fn filter(scheme: Option<&str>, port: Option<i32>) -> RequestRedirectFilter {
    RequestRedirectFilter::try_new(&HTTPRequestRedirectFilter {
        scheme: scheme.map(|s| s.to_string()),
        hostname: None,
        path: None,
        port,
        status_code: None,
    })
    .unwrap()
}

#[test]
fn build_location_preserves_query_string() {
    let f = filter(Some("https"), None);
    assert_eq!(
        f.build_location(
            "http",
            Some("example.com"),
            "/login",
            Some("returnTo=/dashboard"),
            None,
            80
        ),
        Some("https://example.com/login?returnTo=/dashboard".to_string())
    );
}

#[test]
fn build_location_without_query_has_no_question_mark() {
    let f = filter(Some("https"), None);
    // None query and empty query must both omit the '?'.
    assert_eq!(
        f.build_location("http", Some("example.com"), "/login", None, None, 80),
        Some("https://example.com/login".to_string())
    );
    assert_eq!(
        f.build_location("http", Some("example.com"), "/login", Some(""), None, 80),
        Some("https://example.com/login".to_string())
    );
}

#[test]
fn build_location_appends_query_after_non_default_port() {
    let f = filter(None, Some(8443));
    // Query is appended after the port segment, producing a valid authority + path + query.
    assert_eq!(
        f.build_location("https", Some("example.com"), "/search", Some("q=hello"), None, 443),
        Some("https://example.com:8443/search?q=hello".to_string())
    );
}

#[test]
fn build_location_replaces_parameterized_prefix_by_path_elements() {
    let f = RequestRedirectFilter::try_new(&HTTPRequestRedirectFilter {
        scheme: Some("https".to_string()),
        hostname: None,
        path: Some(HTTPPathModifier {
            modifier_type: HTTPPathModifierType::ReplacePrefixMatch,
            replace_full_path: None,
            replace_prefix_match: Some("/v2".to_string()),
        }),
        port: None,
        status_code: None,
    })
    .unwrap();

    assert_eq!(
        f.build_location(
            "http",
            Some("example.com"),
            "/users/alice/orders",
            Some("view=full"),
            Some("/users/:id"),
            80,
        ),
        Some("https://example.com/v2/orders?view=full".to_string())
    );
}

async fn assert_emitted_redirect(
    downstream_is_tls: Option<bool>,
    configured_scheme: Option<&str>,
    listener_port: u16,
    status_code: Option<i32>,
    expected_status: u16,
    expected_location: &str,
) {
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("host", "example.com")]);
    session.expect_downstream_is_tls().return_const(downstream_is_tls);
    session.expect_get_path().return_const("/search".to_string());
    session
        .expect_request_query()
        .return_const(Some("q=hello%20world&next=%2Fhome"));

    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.listener_port = listener_port;
    session.expect_ctx().return_const(ctx);

    let expected_location = expected_location.to_string();
    session
        .expect_write_response_header()
        .times(1)
        .returning(move |response, end_stream| {
            assert_eq!(response.status.as_u16(), expected_status);
            assert!(end_stream);
            assert_eq!(
                response.headers.get("Location").and_then(|value| value.to_str().ok()),
                Some(expected_location.as_str())
            );
            assert_eq!(
                response
                    .headers
                    .get("Content-Length")
                    .and_then(|value| value.to_str().ok()),
                Some("0")
            );
            Ok(())
        });

    let f = RequestRedirectFilter::try_new(&HTTPRequestRedirectFilter {
        scheme: configured_scheme.map(str::to_string),
        hostname: None,
        path: None,
        port: None,
        status_code,
    })
    .unwrap();
    let mut log = PluginLog::new("test");
    assert_eq!(
        f.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrTerminateRequest
    );
}

#[tokio::test]
async fn prefix_redirect_without_matched_prefix_fails_closed() {
    let mut session = MockPluginSession::new();
    setup_header_mocks(&mut session, &[("host", "example.com")]);
    session.expect_downstream_is_tls().return_const(Some(true));
    session.expect_get_path().return_const("/api/orders".to_string());
    session.expect_request_query().return_const(None);
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.listener_port = 443;
    session.expect_ctx().return_const(ctx);
    session.expect_write_response_header().times(0);

    let filter = RequestRedirectFilter::try_new(&HTTPRequestRedirectFilter {
        scheme: None,
        hostname: None,
        path: Some(HTTPPathModifier {
            modifier_type: HTTPPathModifierType::ReplacePrefixMatch,
            replace_full_path: None,
            replace_prefix_match: Some("/v2".to_string()),
        }),
        port: None,
        status_code: None,
    })
    .unwrap();
    let mut log = PluginLog::new("test");
    assert!(matches!(
        filter.run_request(&mut session, &mut log).await,
        PluginRunningResult::ErrResponse {
            status: 500,
            body: None
        }
    ));
}

#[tokio::test]
async fn run_request_emits_exact_location_status_and_end_stream() {
    assert_emitted_redirect(
        Some(true),
        None,
        443,
        Some(308),
        308,
        "https://example.com/search?q=hello%20world&next=%2Fhome",
    )
    .await;
}

#[tokio::test]
async fn omitted_scheme_uses_transport_tls_state_and_https_safe_unknown() {
    assert_emitted_redirect(
        Some(false),
        None,
        80,
        None,
        302,
        "http://example.com/search?q=hello%20world&next=%2Fhome",
    )
    .await;
    // The default context has no SNI, proving TLS does not depend on SNI presence.
    assert_emitted_redirect(
        Some(true),
        None,
        443,
        None,
        302,
        "https://example.com/search?q=hello%20world&next=%2Fhome",
    )
    .await;
    assert_emitted_redirect(
        None,
        None,
        443,
        None,
        302,
        "https://example.com/search?q=hello%20world&next=%2Fhome",
    )
    .await;
}

#[tokio::test]
async fn explicit_scheme_takes_precedence_over_transport_tls_state() {
    assert_emitted_redirect(
        Some(true),
        Some("http"),
        443,
        None,
        302,
        "http://example.com/search?q=hello%20world&next=%2Fhome",
    )
    .await;
}

#[tokio::test]
async fn redirect_borrows_raw_query_before_writing_owned_location() {
    for (query, expected) in [
        (None, "https://example.com/search"),
        (Some(""), "https://example.com/search"),
        (
            Some("q=a+b&x=a%26b&x=&flag&next=%2f%3F"),
            "https://example.com/search?q=a+b&x=a%26b&x=&flag&next=%2f%3F",
        ),
    ] {
        let mut session = MockPluginSession::new();
        setup_header_mocks(&mut session, &[("host", "example.com")]);
        session.expect_downstream_is_tls().return_const(Some(true));
        session.expect_get_path().return_const("/search".to_owned());
        session.expect_get_query().never();
        session.expect_request_query().times(1).return_const(query);
        let mut ctx = EdgionHttpContext::new();
        ctx.request_info.listener_port = 443;
        session.expect_ctx().return_const(ctx);
        session
            .expect_write_response_header()
            .times(1)
            .returning(move |response, end| {
                assert_eq!(response.status.as_u16(), 302);
                assert!(end);
                assert_eq!(response.headers.get("Location").unwrap(), expected);
                Ok(())
            });
        let mut log = PluginLog::new("test");
        assert_eq!(
            filter(None, None).run_request(&mut session, &mut log).await,
            PluginRunningResult::ErrTerminateRequest
        );
    }
}

#[test]
fn location_builder_uses_one_output_allocation_for_all_path_modes() {
    let paths = [
        (None, "/users/alice/orders", 1),
        (
            Some(HTTPPathModifier {
                modifier_type: HTTPPathModifierType::ReplaceFullPath,
                replace_full_path: Some("/new".into()),
                replace_prefix_match: None,
            }),
            "/new",
            1,
        ),
        (
            Some(HTTPPathModifier {
                modifier_type: HTTPPathModifierType::ReplacePrefixMatch,
                replace_full_path: None,
                replace_prefix_match: Some("/v2/".into()),
            }),
            "/v2/orders",
            1,
        ),
        (
            Some(HTTPPathModifier {
                modifier_type: HTTPPathModifierType::ReplacePrefixMatch,
                replace_full_path: None,
                replace_prefix_match: Some("".into()),
            }),
            "/orders",
            1,
        ),
    ];
    for (path, expected_path, expected_allocations) in paths {
        let filter = RequestRedirectFilter::try_new(&HTTPRequestRedirectFilter {
            scheme: None,
            hostname: None,
            path,
            port: None,
            status_code: None,
        })
        .unwrap();
        for (host, expected_host) in [
            (None, "localhost"),
            (Some("example.com:8080"), "example.com"),
            (Some("[2001:db8::1]:8080"), "[2001:db8::1]"),
        ] {
            for (port, port_suffix) in [(443, ""), (0, ":0"), (9, ":9"), (10, ":10"), (65535, ":65535")] {
                for query in [None, Some(""), Some("x=a%26b&x=+&flag")] {
                    let query_suffix = query
                        .filter(|query| !query.is_empty())
                        .map(|query| format!("?{query}"))
                        .unwrap_or_default();
                    let expected = format!("https://{expected_host}{port_suffix}{expected_path}{query_suffix}");
                    let build =
                        || filter.build_location("https", host, "/users/alice/orders", query, Some("/users/:id"), port);
                    drop(build());
                    let (actual, allocations) = crate::test_allocation_counter::count_allocations(build);
                    assert_eq!(actual.as_deref(), Some(expected.as_str()));
                    assert_eq!(allocations, expected_allocations);
                }
            }
        }
    }
}

#[test]
fn missing_prefix_context_does_not_allocate_location() {
    let filter = RequestRedirectFilter::try_new(&HTTPRequestRedirectFilter {
        scheme: None,
        hostname: None,
        path: Some(HTTPPathModifier {
            modifier_type: HTTPPathModifierType::ReplacePrefixMatch,
            replace_full_path: None,
            replace_prefix_match: Some("/v2".into()),
        }),
        port: None,
        status_code: None,
    })
    .unwrap();
    let (location, allocations) = crate::test_allocation_counter::count_allocations(|| {
        filter.build_location("https", Some("example.com"), "/users/alice", Some("x=1"), None, 8443)
    });
    assert_eq!(location, None);
    assert_eq!(allocations, 0);
}
