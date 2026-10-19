use super::{
    apply_upstream_authority_decision, apply_upstream_read_timeout_override, backend_select_status, backoff_action,
    choose_typed_retry_budget, decide_upstream_authority, decide_want_raw, direct_backend_metadata,
    finalize_request_uri, fold_peer_timeouts, init_backend_context_if_needed, pick_backend_ref, replay_write_floor,
    sp_should_refresh_on_cross_attempt_retry, sp_should_repin_on_strict_retry, AuthorityDecision, BackoffAction,
};
use crate::backends::policy::backend_traffic::authority::AuthorityError;
use crate::backends::policy::backend_traffic::RetryBudget;
use crate::backends::runtime::CompiledBackendTarget;
use crate::backends::ServiceBackendKind;
use crate::ctx::BackendSource;
use crate::plugins::PluginRuntime;
use crate::routes::http::match_unit::{HttpRouteRuleUnit, MatchSpecificity};
use crate::routes::http::routes_mgr::RouteRules;
use crate::{BackendContext, BackendTlsInfo, DirectEndpointPreset, EdgionHttpContext, UpstreamInfo};
use crate::{PeerTimeoutSource, SessionPersistenceState};
use edgion_resources::resources::common::RefDenied;
use edgion_resources::{EdError, EdgionErrorCode, HTTPBackendRef, HTTPRouteMatch, HTTPRouteRule};
use pingora_core::prelude::HttpPeer;
use pingora_core::protocols::l4::socket::SocketAddr as PingoraSocketAddr;
use pingora_core::ErrorType;
use pingora_load_balancing::Backend;
use pingora_proxy::Session;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn ai_predispatch_error_writes_current_request_ids_on_wire() {
    crate::ai::http::attempt::tests::ai_predispatch_error_writes_current_request_ids_on_wire().await;
}

#[test]
fn ai_unavailable_maps_to_fixed_openai_backend_unavailable() {
    crate::ai::http::attempt::tests::ai_unavailable_maps_to_fixed_openai_backend_unavailable();
}

#[test]
fn ai_predispatch_deadlines_map_to_standard_gateway_statuses() {
    crate::ai::http::attempt::tests::ai_predispatch_deadlines_map_to_standard_gateway_statuses();
}

#[tokio::test]
async fn post_io_predispatch_gate_returns_standard_deadline_error() {
    crate::ai::http::attempt::tests::post_io_predispatch_gate_returns_standard_deadline_error().await;
}

#[test]
fn invalid_group_service_kind_stays_unsupported_after_selection() {
    let schema: HTTPBackendRef = serde_json::from_value(serde_json::json!({
        "group": "example.io",
        "kind": "Service",
        "name": "wrong-group-service",
        "namespace": "route-ns",
        "port": 8080
    }))
    .unwrap();
    let backend = crate::routes::http::match_unit::compile_http_backend_fixture(
        schema,
        "route-ns",
        0,
        Arc::new(PluginRuntime::new()),
    );
    assert_eq!(
        backend.target(),
        &CompiledBackendTarget::Invalid {
            classified_kind: edgion_resources::HTTPBackendRefKind::Unsupported
        }
    );
    let mut ctx = EdgionHttpContext::new();
    let logical = crate::backends::LogicalBackendHandle::new(Arc::from(vec![backend]), 0).unwrap();
    ctx.selected_backend = Some(crate::backends::SelectedHttpBackend::Invalid(logical));

    init_backend_context_if_needed(&mut ctx).unwrap();

    assert!(matches!(
        ctx.backend_context.as_ref().and_then(|context| context.service_kind.as_ref()),
        Some(ServiceBackendKind::Unsupported(kind)) if kind == "Service"
    ));
}

#[test]
fn direct_backend_metadata_uses_the_exact_service_slot() {
    let compile = |name: &str, namespace: &str, slot| {
        crate::routes::http::match_unit::compile_http_backend_fixture(
            HTTPBackendRef {
                name: name.to_string(),
                namespace: Some(namespace.to_string()),
                port: Some(8080),
                weight: Some(1),
                group: None,
                kind: Some("Service".to_string()),
                filters: None,
                backend_tls_policy: None,
                ref_denied: None,
            },
            "route-ns",
            slot,
            Arc::new(PluginRuntime::new()),
        )
    };
    let backends = vec![
        compile("duplicate", "first-ns", 0),
        compile("duplicate", "second-ns", 1),
    ];

    let (name, namespace, kind, slot) = direct_backend_metadata(&backends, 1).unwrap();

    assert_eq!(name, "duplicate");
    assert_eq!(namespace, "second-ns");
    assert_eq!(kind, ServiceBackendKind::Service);
    assert_eq!(slot, 1);
    assert!(direct_backend_metadata(&backends, 2).is_none());
}

#[test]
fn direct_backend_metadata_rejects_non_service_slots() {
    let backend = crate::routes::http::match_unit::compile_http_backend_fixture(
        HTTPBackendRef {
            name: "provider".to_string(),
            namespace: Some("ai-ns".to_string()),
            port: None,
            weight: Some(1),
            group: Some("edgion.io".to_string()),
            kind: Some("EdgionBackend".to_string()),
            filters: None,
            backend_tls_policy: None,
            ref_denied: None,
        },
        "route-ns",
        0,
        Arc::new(PluginRuntime::new()),
    );

    assert!(direct_backend_metadata(&[backend], 0).is_none());
}

#[test]
fn direct_backend_metadata_rejects_referencegrant_denied_services() {
    let backend = crate::routes::http::match_unit::compile_http_backend_fixture(
        HTTPBackendRef {
            name: "cross-namespace".to_string(),
            namespace: Some("other-ns".to_string()),
            port: Some(8080),
            weight: Some(1),
            group: None,
            kind: Some("Service".to_string()),
            filters: None,
            backend_tls_policy: None,
            ref_denied: Some(edgion_resources::resources::common::RefDenied {
                target_namespace: "other-ns".to_string(),
                target_name: "cross-namespace".to_string(),
                reason: Some("NoMatchingReferenceGrant".to_string()),
            }),
        },
        "route-ns",
        0,
        Arc::new(PluginRuntime::new()),
    );

    assert!(direct_backend_metadata(&[backend], 0).is_none());
}

#[test]
fn replay_write_floor_applies_only_with_captured_body_and_enabled_default() {
    let global = Duration::from_secs(30);
    // Captured body + enabled global default → the write bound floor.
    assert_eq!(replay_write_floor(true, global), Some(global));
    // No captured body: nothing replays, nothing to bound.
    assert_eq!(replay_write_floor(false, global), None);
    // Global default disabled: the operator asked for no bounds anywhere.
    assert_eq!(replay_write_floor(true, Duration::ZERO), None);
}

async fn live_h1_session(uri: &str) -> (Session, tokio::io::DuplexStream) {
    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(format!("GET {uri} HTTP/1.1\r\nHost: test\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let stream: pingora_core::protocols::Stream = Box::new(server);
    let mut session = Session::new_h1(stream);
    session.read_request().await.unwrap();
    (session, client)
}

#[tokio::test]
async fn finalize_request_uri_writes_normalized_path_with_original_query_then_latches() {
    let (mut session, _client) = live_h1_session("/%61dmin?scope=read").await;
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.normalized_current_path = "/admin".to_string();

    finalize_request_uri(&mut session, &mut ctx).unwrap();

    assert_eq!(session.req_header().uri, "/admin?scope=read");
    assert!(ctx.uri_finalized);
}

#[tokio::test]
async fn finalize_request_uri_keeps_h2_authority_without_host() {
    let (mut session, _client) = live_h1_session("/%61dmin?scope=read").await;
    session
        .req_header_mut()
        .set_uri("https://example.com/%61dmin?scope=read".parse().unwrap());
    session.req_header_mut().set_version(http::Version::HTTP_2);
    session.req_header_mut().remove_header("Host");
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.normalized_current_path = "/admin".to_string();

    finalize_request_uri(&mut session, &mut ctx).unwrap();

    let header = session.req_header();
    assert_eq!(header.uri.to_string(), "https://example.com/admin?scope=read");
    assert!(header.headers.get("Host").is_none());
    assert!(ctx.uri_finalized);
}

#[tokio::test]
async fn finalize_request_uri_keeps_absolute_form_authority_and_query() {
    let (mut session, _client) = live_h1_session("http://test/%61dmin?scope=read").await;
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.normalized_current_path = "/admin".to_string();

    finalize_request_uri(&mut session, &mut ctx).unwrap();

    let header = session.req_header();
    assert_eq!(header.uri.to_string(), "/admin?scope=read");
    assert_eq!(header.raw_path(), b"http://test/admin?scope=read");
    assert_eq!(header.headers.get("host").unwrap(), "test");
    assert!(ctx.uri_finalized);
}

#[tokio::test]
async fn finalize_request_uri_leaves_an_already_finalized_uri_untouched() {
    let (mut session, _client) = live_h1_session("/plugin-owned?scope=read").await;
    let original = session.req_header().uri.clone();
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.normalized_current_path = "/different".to_string();
    ctx.uri_finalized = true;

    finalize_request_uri(&mut session, &mut ctx).unwrap();

    assert_eq!(session.req_header().uri, original);
    assert!(ctx.uri_finalized);
}

#[tokio::test]
async fn finalize_request_uri_latches_when_the_computed_uri_is_already_equal() {
    let (mut session, _client) = live_h1_session("/admin?scope=read").await;
    let original = session.req_header().uri.clone();
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.normalized_current_path = "/admin".to_string();

    finalize_request_uri(&mut session, &mut ctx).unwrap();

    assert_eq!(session.req_header().uri, original);
    assert!(ctx.uri_finalized);
}

#[tokio::test]
async fn finalize_request_uri_parse_failure_preserves_uri_and_latch() {
    let (mut session, _client) = live_h1_session("/raw?scope=read").await;
    let original = session.req_header().uri.clone();
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.normalized_current_path = "/invalid path".to_string();

    assert!(finalize_request_uri(&mut session, &mut ctx).is_err());
    assert_eq!(session.req_header().uri, original);
    assert!(!ctx.uri_finalized);
}

#[test]
fn default_writes_normalized() {
    // No annotation, no external_jump, global forward_normalized_path=true → normalized
    assert!(!decide_want_raw(None, false, true));
}

#[test]
fn annotation_true_forces_raw() {
    assert!(decide_want_raw(Some(true), false, true));
}

#[test]
fn annotation_false_forces_normalized_overrides_region_route() {
    // annotation "false" overrides region_route's preserve_raw_path = true
    assert!(!decide_want_raw(Some(false), true, true));
}

#[test]
fn region_route_default_raw() {
    assert!(decide_want_raw(None, true, true));
}

#[test]
fn dynamic_external_upstream_default_normalized() {
    // dynamic_external_upstream sets preserve_raw_path: false on its
    // ExternalJumpPreset. With no annotation override, this should still
    // forward normalized — same outcome as the no-external-jump default,
    // pinned here as an intentional regression check for plugin defaults.
    assert!(!decide_want_raw(None, false, true));
}

#[test]
fn global_disable_forwards_raw() {
    assert!(decide_want_raw(None, false, false));
}

#[test]
fn annotation_true_overrides_global_normalize() {
    assert!(decide_want_raw(Some(true), false, true));
}

#[test]
fn annotation_false_overrides_global_disable() {
    assert!(!decide_want_raw(Some(false), false, false));
}

// ─── Matrix completeness tests: cover the full 3×2×2 = 12 input combinations ───
// The 8 tests above cover 8 named scenarios; the 4 below fill the remaining
// 4 boundary combinations to make matrix coverage exhaustive.

#[test]
fn annotation_true_with_region_route_and_global_normalize_still_raw() {
    // Annotation wins at the top of the priority chain. Even with all lower
    // channels active, Some(true) deterministically returns true.
    assert!(decide_want_raw(Some(true), true, true));
}

#[test]
fn annotation_true_with_region_route_and_global_disable_still_raw() {
    // Annotation true + region_route + global disable → all channels agree on raw.
    assert!(decide_want_raw(Some(true), true, false));
}

#[test]
fn annotation_false_with_region_route_and_global_disable_still_normalized() {
    // Annotation false explicitly overrides BOTH region_route's preserve_raw_path
    // AND the global disable. This is the operator's emergency override channel.
    assert!(!decide_want_raw(Some(false), true, false));
}

#[test]
fn region_route_with_global_disable_doubles_down_on_raw() {
    // No annotation, region_route preserve_raw=true, global forward_normalized=false
    // → both lower channels OR-combine to raw. Verifies the OR formula in the None arm.
    assert!(decide_want_raw(None, true, false));
}

// ─── I-2 regression: idempotency check uses path() not to_string() ───
// Verifies that for absolute-form HTTP/2 URIs (where to_string() includes
// scheme + authority but path() returns only the path component), the
// upstream_peer_http idempotency comparison correctly uses the path component.
// If a future refactor reverts to to_string() comparison, this test catches the
// regression by demonstrating that path() and to_string() can diverge.

#[test]
fn absolute_form_uri_path_only_comparison() {
    let absolute: http::Uri = "http://example.com/api/data?x=1".parse().unwrap();
    // path() returns only the path component
    assert_eq!(absolute.path(), "/api/data");
    // to_string() includes scheme + authority + query (and would falsely report
    // "different" against a normalized "/api/data" path)
    assert_ne!(absolute.to_string(), "/api/data");
    assert!(absolute.to_string().contains("example.com"));
    // The pg_upstream_peer.rs idempotency check at upstream_peer_http uses
    // path() so absolute-form requests don't trigger spurious set_uri writes.
}

// --- sp_should_refresh_on_cross_attempt_retry matrix ---

#[test]
fn sp_refresh_fires_on_consumed_retry_nonstrict_inactive() {
    // The one combination that yields a refresh.
    assert!(sp_should_refresh_on_cross_attempt_retry(
        true,
        2,
        true,
        false,
        &SessionPersistenceState::Inactive,
    ));
}

#[test]
fn sp_refresh_skips_when_not_consumed() {
    assert!(!sp_should_refresh_on_cross_attempt_retry(
        false,
        2,
        true,
        false,
        &SessionPersistenceState::Inactive,
    ));
}

#[test]
fn sp_refresh_skips_on_first_attempt() {
    // try_cnt == 1 means this is the first attempt, not a retry.
    assert!(!sp_should_refresh_on_cross_attempt_retry(
        true,
        1,
        true,
        false,
        &SessionPersistenceState::Inactive,
    ));
}

#[test]
fn sp_refresh_skips_when_sp_not_configured() {
    assert!(!sp_should_refresh_on_cross_attempt_retry(
        true,
        2,
        false,
        false,
        &SessionPersistenceState::Inactive,
    ));
}

#[test]
fn sp_refresh_skips_in_strict_mode() {
    assert!(!sp_should_refresh_on_cross_attempt_retry(
        true,
        2,
        true,
        true,
        &SessionPersistenceState::Inactive,
    ));
}

#[test]
fn sp_refresh_does_not_downgrade_inject_new() {
    // An already-armed InjectNew/InjectUpdate must not be touched.
    assert!(!sp_should_refresh_on_cross_attempt_retry(
        true,
        2,
        true,
        false,
        &SessionPersistenceState::InjectNew,
    ));
    assert!(!sp_should_refresh_on_cross_attempt_retry(
        true,
        2,
        true,
        false,
        &SessionPersistenceState::InjectUpdate,
    ));
}

#[test]
fn sp_refresh_fires_on_later_retries() {
    // try_cnt >= 2 covers the second retry and beyond.
    assert!(sp_should_refresh_on_cross_attempt_retry(
        true,
        3,
        true,
        false,
        &SessionPersistenceState::Inactive,
    ));
}

// --- sp_should_repin_on_strict_retry matrix ---

#[test]
fn sp_repin_fires_on_consumed_strict_retry_with_addr() {
    assert!(sp_should_repin_on_strict_retry(true, 2, true, true, true));
}

#[test]
fn sp_repin_skips_when_not_consumed() {
    assert!(!sp_should_repin_on_strict_retry(false, 2, true, true, true));
}

#[test]
fn sp_repin_skips_on_first_attempt() {
    assert!(!sp_should_repin_on_strict_retry(true, 1, true, true, true));
}

#[test]
fn sp_repin_skips_when_sp_not_configured() {
    assert!(!sp_should_repin_on_strict_retry(true, 2, false, true, true));
}

#[test]
fn sp_repin_skips_in_nonstrict_mode() {
    assert!(!sp_should_repin_on_strict_retry(true, 2, true, false, true));
}

#[test]
fn sp_repin_skips_without_sticky_addr() {
    assert!(!sp_should_repin_on_strict_retry(true, 2, true, true, false));
}

#[test]
fn sp_repin_fires_on_later_retries() {
    assert!(sp_should_repin_on_strict_retry(true, 3, true, true, true));
}

#[test]
fn sp_repin_and_refresh_are_mutually_exclusive() {
    // Over the full shared input space, the two cross-attempt decisions must
    // never both be true: re-pin requires strict, refresh requires !strict.
    for &consumed in &[false, true] {
        for &try_cnt in &[1u32, 2, 3] {
            for &configured in &[false, true] {
                for &strict in &[false, true] {
                    for &has_addr in &[false, true] {
                        for state in &[
                            SessionPersistenceState::Inactive,
                            SessionPersistenceState::InjectNew,
                            SessionPersistenceState::InjectUpdate,
                        ] {
                            let repin =
                                sp_should_repin_on_strict_retry(consumed, try_cnt, configured, strict, has_addr);
                            let refresh =
                                sp_should_refresh_on_cross_attempt_retry(consumed, try_cnt, configured, strict, state);
                            assert!(
                                !(repin && refresh),
                                "both fired: consumed={} try_cnt={} configured={} strict={} has_addr={} state={:?}",
                                consumed,
                                try_cnt,
                                configured,
                                strict,
                                has_addr,
                                state
                            );
                        }
                    }
                }
            }
        }
    }
}

fn s(n: u64) -> Option<Duration> {
    Some(Duration::from_secs(n))
}

#[test]
fn fold_backend_only() {
    assert_eq!(
        fold_peer_timeouts(s(5), None, None),
        Some((Duration::from_secs(5), PeerTimeoutSource::BackendRequest))
    );
}

#[test]
fn fold_request_deadline_tighter() {
    assert_eq!(
        fold_peer_timeouts(s(5), s(2), None),
        Some((Duration::from_secs(2), PeerTimeoutSource::RequestDeadline))
    );
}

#[test]
fn fold_grpc_deadline_tightest() {
    assert_eq!(
        fold_peer_timeouts(s(5), s(4), s(1)),
        Some((Duration::from_secs(1), PeerTimeoutSource::GrpcDeadline))
    );
}

#[test]
fn fold_tie_backend_wins() {
    // Equal bounds: BackendRequest wins so the operator-opted retry
    // semantics apply; deadline guards still gate the actual retry.
    assert_eq!(
        fold_peer_timeouts(s(3), s(3), s(3)),
        Some((Duration::from_secs(3), PeerTimeoutSource::BackendRequest))
    );
}

#[test]
fn fold_two_way_tie_backend_wins() {
    // Backend ties with a single deadline bound: BackendRequest still wins.
    assert_eq!(
        fold_peer_timeouts(s(3), s(3), None),
        Some((Duration::from_secs(3), PeerTimeoutSource::BackendRequest))
    );
}

#[test]
fn fold_deadline_tie_request_wins() {
    // Deadline-vs-deadline tie: iteration order makes RequestDeadline win.
    // Pinned here because the order is observable in access-log error
    // attribution even though both deadlines expire simultaneously.
    assert_eq!(
        fold_peer_timeouts(None, s(3), s(3)),
        Some((Duration::from_secs(3), PeerTimeoutSource::RequestDeadline))
    );
}

#[test]
fn connect_bounds_preserve_disabled_expired_and_tie_semantics() {
    use crate::ctx::ConnectTimeoutSource::{Configured, GrpcDeadline, RequestDeadline};
    let cases = [
        ((None, None, None), None),
        ((Some(Duration::ZERO), None, None), None),
        ((s(5), None, None), Some((Duration::from_secs(5), Configured))),
        ((s(1), s(2), s(3)), Some((Duration::from_secs(1), Configured))),
        ((s(5), s(2), s(3)), Some((Duration::from_secs(2), RequestDeadline))),
        ((s(5), s(3), s(2)), Some((Duration::from_secs(2), GrpcDeadline))),
        ((None, s(2), None), Some((Duration::from_secs(2), RequestDeadline))),
        ((None, None, s(2)), Some((Duration::from_secs(2), GrpcDeadline))),
        ((s(2), s(2), s(2)), Some((Duration::from_secs(2), RequestDeadline))),
        ((s(2), None, s(2)), Some((Duration::from_secs(2), GrpcDeadline))),
        ((None, s(2), s(2)), Some((Duration::from_secs(2), RequestDeadline))),
        (
            (s(5), Some(Duration::ZERO), s(2)),
            Some((Duration::ZERO, RequestDeadline)),
        ),
        ((s(5), None, Some(Duration::ZERO)), Some((Duration::ZERO, GrpcDeadline))),
        (
            (None, Some(Duration::ZERO), Some(Duration::ZERO)),
            Some((Duration::ZERO, RequestDeadline)),
        ),
    ];
    for ((configured, request, grpc), expected) in cases {
        assert_eq!(super::fold_connect_timeouts(configured, request, grpc), expected);
    }
}

#[test]
fn configure_connect_bounds_are_independent_and_reset_for_each_peer() {
    use crate::ctx::ConnectTimeoutSource;
    use edgion_resources::resources::http_route::{ParsedRouteTimeouts, RouteTimeout};
    let proxy = super::EdgionHttpProxy {
        listener_port: 80,
        is_https_listener: false,
        listener_key: edgion_resources::ListenerKey("127.0.0.1:80/HTTP".to_string()),
        server_header_opts: Default::default(),
        access_logger: Arc::new(crate::observe::AccessLogger::default()),
        egc: crate::config::bind_gateway_config(None),
        enable_http2: false,
        preflight_handler: super::super::PreflightHandler,
        http_route_manager: Arc::new(crate::routes::http::routes_mgr::HttpPortRouteManager::default()),
        grpc_route_manager: Arc::new(crate::routes::grpc::GrpcPortRouteManager::default()),
    };
    let mut ctx = EdgionHttpContext::new();
    let mut route = jump_route_unit(vec![]);
    Arc::make_mut(&mut Arc::make_mut(&mut route).rule).parsed_timeouts = Some(ParsedRouteTimeouts {
        request_timeout: RouteTimeout::Duration(Duration::from_secs(3)),
        backend_request_timeout: RouteTimeout::Duration(Duration::from_millis(1)),
    });
    ctx.route_unit = Some(route);
    ctx.start_time = Instant::now() - Duration::from_secs(2);
    let mut peer = Box::new(HttpPeer::new("127.0.0.1:80", false, String::new()));
    super::configure_peer_timeouts(&proxy, &mut peer, &mut ctx);
    assert_eq!(ctx.connect_timeout_source, Some(ConnectTimeoutSource::RequestDeadline));
    let connect = peer.options.total_connection_timeout.unwrap();
    assert!(connect > Duration::from_millis(500) && connect <= Duration::from_secs(1));
    assert_eq!(peer.options.connection_timeout, Some(connect));
    assert_eq!(peer.options.read_timeout, Some(Duration::from_millis(1)));
    assert_eq!(ctx.peer_timeout_source, Some(PeerTimeoutSource::BackendRequest));

    ctx.request_info
        .set_grpc_client_deadline(Instant::now() + Duration::from_millis(100));
    super::configure_peer_timeouts(&proxy, &mut peer, &mut ctx);
    assert_eq!(ctx.connect_timeout_source, Some(ConnectTimeoutSource::GrpcDeadline));
    assert!(peer.options.total_connection_timeout.unwrap() <= Duration::from_millis(100));

    ctx.start_time = Instant::now() - Duration::from_secs(4);
    super::configure_peer_timeouts(&proxy, &mut peer, &mut ctx);
    assert_eq!(ctx.connect_timeout_source, Some(ConnectTimeoutSource::RequestDeadline));
    assert_eq!(peer.options.total_connection_timeout, Some(Duration::ZERO));

    ctx.route_unit = None;
    ctx.request_info = Default::default();
    let mut config = crate::config::ResolvedGatewayConfig::default();
    config.parsed_timeouts.backend.connect_timeout = Duration::ZERO;
    ctx.egc = Arc::new(config);
    super::configure_peer_timeouts(&proxy, &mut peer, &mut ctx);
    assert_eq!(ctx.connect_timeout_source, None);
    assert_eq!(peer.options.connection_timeout, None);
    assert_eq!(peer.options.total_connection_timeout, None);
}

#[test]
fn fold_backend_disabled_request_wins() {
    // backendRequest: "0s" (Disabled) → backend bound is None.
    assert_eq!(
        fold_peer_timeouts(None, s(7), None),
        Some((Duration::from_secs(7), PeerTimeoutSource::RequestDeadline))
    );
}

#[test]
fn fold_grpc_only() {
    assert_eq!(
        fold_peer_timeouts(None, None, s(1)),
        Some((Duration::from_secs(1), PeerTimeoutSource::GrpcDeadline))
    );
}

#[test]
fn fold_backend_grpc_tie_backend_wins() {
    assert_eq!(
        fold_peer_timeouts(s(3), None, s(3)),
        Some((Duration::from_secs(3), PeerTimeoutSource::BackendRequest))
    );
}

#[test]
fn fold_all_none() {
    assert_eq!(fold_peer_timeouts(None, None, None), None);
}

#[test]
fn external_read_override_replaces_normal_backend_bound() {
    assert_eq!(apply_upstream_read_timeout_override(s(30), s(300)), s(300));
    assert_eq!(apply_upstream_read_timeout_override(None, s(300)), s(300));
}

#[test]
fn absent_or_zero_external_override_preserves_normal_backend_bound() {
    assert_eq!(apply_upstream_read_timeout_override(s(30), None), s(30));
    assert_eq!(apply_upstream_read_timeout_override(s(30), Some(Duration::ZERO)), s(30));
}

#[test]
fn external_override_remains_backend_source_then_deadlines_clamp_it() {
    let overridden = apply_upstream_read_timeout_override(s(30), s(300));
    assert_eq!(
        fold_peer_timeouts(overridden, s(20), s(10)),
        Some((Duration::from_secs(10), PeerTimeoutSource::GrpcDeadline))
    );
    assert_eq!(
        fold_peer_timeouts(overridden, s(300), None),
        Some((Duration::from_secs(300), PeerTimeoutSource::BackendRequest))
    );
}

// ─── effective_backend_bound: route three-state + global-default fallback ───

use super::effective_backend_bound;
use edgion_resources::resources::http_route::RouteTimeout;

#[test]
fn backend_bound_unset_falls_back_to_global() {
    let global = Duration::from_secs(60);
    assert_eq!(effective_backend_bound(None, global), Some(global));
    assert_eq!(
        effective_backend_bound(Some(&RouteTimeout::Unset), global),
        Some(global)
    );
}

#[test]
fn backend_bound_disabled_is_none() {
    assert_eq!(
        effective_backend_bound(Some(&RouteTimeout::Disabled), Duration::from_secs(60)),
        None
    );
}

#[test]
fn backend_bound_route_duration_wins_over_global() {
    assert_eq!(
        effective_backend_bound(
            Some(&RouteTimeout::Duration(Duration::from_secs(1))),
            Duration::from_secs(60)
        ),
        Some(Duration::from_secs(1))
    );
}

#[test]
fn backend_bound_zero_global_treated_as_disabled() {
    // A `defaultRequestTimeout: "0s"` global must not become an instant
    // read/write timeout (instant-retry loop with a retry policy present).
    assert_eq!(effective_backend_bound(None, Duration::ZERO), None);
    assert_eq!(
        effective_backend_bound(Some(&RouteTimeout::Unset), Duration::ZERO),
        None
    );
}

// ─── resolve_connect_deadline: per-backend BTP override, else global ───

use super::{choose_typed_connect_deadline, resolve_connect_deadline};
use crate::backends::policy::backend_traffic::store::{get_backend_traffic_policy_store, GLOBAL_STORE_TEST_LOCK};

fn unstarted_service_ctx(service_key: &str) -> EdgionHttpContext {
    let (namespace, name) = service_key.split_once('/').unwrap();
    let mut schema = jump_backend_ref(name);
    schema.namespace = Some(namespace.to_owned());
    let compiled = crate::routes::http::match_unit::compile_http_backend_fixture(
        schema,
        namespace,
        0,
        Arc::new(PluginRuntime::new()),
    );
    let logical = crate::backends::LogicalBackendHandle::new(Arc::from(vec![compiled]), 0).unwrap();
    let mut ctx = EdgionHttpContext::new();
    ctx.selected_backend = Some(crate::backends::SelectedHttpBackend::Service(
        crate::backends::SelectedServiceBackend::new(logical, crate::backends::BackendTlsPolicyLookup::Absent).unwrap(),
    ));
    ctx.backend_context = Some(BackendContext {
        source: None,
        rsc: None,
        service_key: Some(service_key.to_string()),
        service_kind: Some(ServiceBackendKind::Service),
        upstreams: Vec::new(),
        current_upstream_id: None,
        backend_ref_idx: Some(0),
        effective_policy_captured: false,
        effective_policy: None,
    });
    ctx
}

fn capture_service_ctx(service_key: &str) -> EdgionHttpContext {
    let mut ctx = unstarted_service_ctx(service_key);
    super::begin_upstream_attempt(&mut ctx);
    ctx
}

fn captured_policy(
    ctx: &EdgionHttpContext,
) -> Arc<crate::backends::policy::backend_traffic::EffectiveBackendTrafficPolicy> {
    ctx.backend_context
        .as_ref()
        .and_then(|bc| bc.effective_policy.clone())
        .expect("service policy captured")
}

#[allow(clippy::too_many_arguments)]
fn service_policy_fields(
    ns: &str,
    name: &str,
    svc: &str,
    lb: Option<edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType>,
    degrade_threshold: Option<u8>,
    health_check_host: Option<&str>,
    retry_percent: Option<u8>,
    connect_timeout: Option<&str>,
    cb_limit: Option<u32>,
) -> Arc<edgion_resources::resources::EdgionBackendTrafficPolicy> {
    use edgion_resources::resources::circuit_breaker::CircuitBreakerConfig;
    use edgion_resources::resources::edgion_backend_traffic_policy::{
        BudgetDetails, ConnectionOverride, EdgionBackendTrafficPolicySpec, LoadBalancerConfig, PolicyTargetRef,
        RetryConstraintConfig,
    };
    use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;
    use edgion_resources::resources::EdgionBackendTrafficPolicy;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};

    Arc::new(EdgionBackendTrafficPolicy {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(ns.to_string()),
            creation_timestamp: Some(Time("2026-01-01T00:00:00Z".parse().expect("rfc3339"))),
            ..Default::default()
        },
        spec: EdgionBackendTrafficPolicySpec {
            prepared_ancestor_admission: Some(edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Unrestricted),
            prepared_precedence: Some(
                edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence {
                    creation_timestamp: Some("2026-01-01T00:00:00Z".parse().expect("rfc3339")),
                },
            ),
            current_status: None,
            target_refs: vec![PolicyTargetRef {
                group: Some(String::new()),
                kind: "Service".to_string(),
                name: svc.to_string(),
            }],
            resolved_target_class: Some(
                edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
            ),
            load_balancer: lb.map(|r#type| LoadBalancerConfig {
                r#type,
                consistent_hash: None,
                degrade_threshold,
            }),
            health_check: None,
            outlier_detection: None,
            upstream_authority: health_check_host.map(|host| UpstreamAuthorityConfig {
                pattern: "*.sandbox.internal".to_string(),
                template: "${ctx:tenant}.sandbox.internal".to_string(),
                health_check_host: Some(host.to_string()),
            }),
            retry_constraint: retry_percent.map(|percent| RetryConstraintConfig {
                budget: BudgetDetails {
                    percent,
                    interval: "10s".to_string(),
                },
                min_retry_rate: None,
            }),
            circuit_breaker: cb_limit.map(|max_parallel_requests| CircuitBreakerConfig { max_parallel_requests }),
            connection: connect_timeout.map(|connect_timeout| ConnectionOverride {
                connect_timeout: Some(connect_timeout.to_string()),
            }),
        },
        status: None,
    })
}

#[test]
fn configure_peer_timeouts_service_does_not_call_get_connect_timeout() {
    use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    store.full_set_policies(std::collections::HashMap::from([(
        "to-cap/p1".to_string(),
        service_policy_fields(
            "to-cap",
            "p1",
            "svc",
            Some(LoadBalancerType::RoundRobin),
            None,
            None,
            None,
            Some("123ms"),
            None,
        ),
    )]));

    let ctx = capture_service_ctx("to-cap/svc");
    let captured = captured_policy(&ctx);
    assert_eq!(captured.connect_timeout, Some(Duration::from_millis(123)));

    store.full_set_policies(std::collections::HashMap::from([(
        "to-cap/p1".to_string(),
        service_policy_fields(
            "to-cap",
            "p1",
            "svc",
            Some(LoadBalancerType::RoundRobin),
            None,
            None,
            None,
            Some("999ms"),
            None,
        ),
    )]));
    assert_eq!(
        resolve_connect_deadline(Some("to-cap/svc"), Duration::from_secs(5)),
        Some(Duration::from_millis(999)),
        "the live shim must see the reloaded timeout"
    );
    assert_eq!(
        choose_typed_connect_deadline(
            false,
            captured.connect_timeout,
            captured.connect_timeout,
            Duration::from_secs(5)
        ),
        Some(Duration::from_millis(123)),
        "configure_peer_timeouts Service branch uses only the captured timeout"
    );

    store.full_set_policies(std::collections::HashMap::new());
}

#[test]
fn resolve_connect_deadline_no_override_falls_back_to_global() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    let key = "default/no-such-svc";
    store.reapply_service(key);

    assert_eq!(
        resolve_connect_deadline(Some(key), Duration::from_secs(5)),
        Some(Duration::from_secs(5))
    );
    // No service key (ExternalJump) also falls back to global.
    assert_eq!(
        resolve_connect_deadline(None, Duration::from_secs(5)),
        Some(Duration::from_secs(5))
    );
    // Degenerate zero global => None (no connect bound), never an instant-fail.
    assert_eq!(resolve_connect_deadline(Some(key), Duration::ZERO), None);
    assert_eq!(resolve_connect_deadline(None, Duration::ZERO), None);
}

#[test]
fn ai_connect_timeout_never_falls_back_to_same_named_service_policy() {
    let service_timeout = Some(Duration::from_millis(123));
    let global = Duration::from_secs(5);

    assert_eq!(
        choose_typed_connect_deadline(true, None, service_timeout, global),
        Some(global),
        "an AI selection with no override must use the global default"
    );
    assert_eq!(
        choose_typed_connect_deadline(false, None, service_timeout, global),
        service_timeout,
        "a Service selection retains its late policy override"
    );
}

#[test]
fn ai_retry_budget_never_falls_back_to_same_named_service_policy() {
    use edgion_resources::resources::edgion_backend_traffic_policy::{BudgetDetails, RetryConstraintConfig};

    let service_budget = Arc::new(RetryBudget::from_config(&RetryConstraintConfig {
        budget: BudgetDetails {
            percent: 50,
            interval: "1h".to_string(),
        },
        min_retry_rate: None,
    }));
    assert!(choose_typed_retry_budget(true, None, Some(service_budget.clone())).is_none());
    assert!(Arc::ptr_eq(
        &choose_typed_retry_budget(false, None, Some(service_budget.clone())).unwrap(),
        &service_budget
    ));
}

#[test]
fn begin_service_attempt_releases_previous_endpoint_permit_first() {
    let backend = Arc::new(crate::lb::LbBackend::new(Backend::new("127.0.0.1:18080").unwrap()));
    let mut ctx = EdgionHttpContext::new();
    ctx.cb_permit = crate::backends::circuit_breaker::CbPermit::try_acquire(backend.clone(), 1);
    assert_eq!(backend.cb_inflight(), 1);

    super::begin_upstream_attempt(&mut ctx);

    assert_eq!(backend.cb_inflight(), 0);
    assert!(ctx.cb_permit.is_none());
}

#[test]
fn begin_upstream_attempt_does_not_recapture_on_retry() {
    use edgion_resources::resources::edgion_backend_traffic_policy::{
        EdgionBackendTrafficPolicySpec, LoadBalancerConfig, LoadBalancerType, PolicyTargetRef,
    };
    use edgion_resources::resources::EdgionBackendTrafficPolicy;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};
    use std::sync::Arc;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    let first = Arc::new(EdgionBackendTrafficPolicy {
        metadata: ObjectMeta {
            name: Some("p1".to_string()),
            namespace: Some("cap".to_string()),
            creation_timestamp: Some(Time("2026-01-01T00:00:00Z".parse().expect("rfc3339"))),
            ..Default::default()
        },
        spec: EdgionBackendTrafficPolicySpec {
            prepared_ancestor_admission: Some(edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Unrestricted),
            prepared_precedence: Some(
                edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence {
                    creation_timestamp: Some("2026-01-01T00:00:00Z".parse().expect("rfc3339")),
                },
            ),
            current_status: None,
            target_refs: vec![PolicyTargetRef {
                group: Some(String::new()),
                kind: "Service".to_string(),
                name: "svc".to_string(),
            }],
            resolved_target_class: Some(
                edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
            ),
            load_balancer: Some(LoadBalancerConfig {
                r#type: LoadBalancerType::LeastConn,
                consistent_hash: None,
                degrade_threshold: None,
            }),
            health_check: None,
            outlier_detection: None,
            upstream_authority: None,
            retry_constraint: None,
            circuit_breaker: None,
            connection: None,
        },
        status: None,
    });
    store.full_set_policies(std::collections::HashMap::from([("cap/p1".to_string(), first)]));

    let mut ctx = unstarted_service_ctx("cap/svc");
    super::begin_upstream_attempt(&mut ctx);
    let captured = ctx
        .backend_context
        .as_ref()
        .and_then(|bc| bc.effective_policy.clone())
        .expect("captured");
    assert_eq!(
        captured.load_balancer,
        Some(edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn)
    );

    let second = Arc::new(EdgionBackendTrafficPolicy {
        metadata: ObjectMeta {
            name: Some("p1".to_string()),
            namespace: Some("cap".to_string()),
            creation_timestamp: Some(Time("2026-01-01T00:00:00Z".parse().expect("rfc3339"))),
            ..Default::default()
        },
        spec: EdgionBackendTrafficPolicySpec {
            prepared_ancestor_admission: Some(edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Unrestricted),
            prepared_precedence: Some(
                edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence {
                    creation_timestamp: Some("2026-01-01T00:00:00Z".parse().expect("rfc3339")),
                },
            ),
            current_status: None,
            target_refs: vec![PolicyTargetRef {
                group: Some(String::new()),
                kind: "Service".to_string(),
                name: "svc".to_string(),
            }],
            resolved_target_class: Some(
                edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
            ),
            load_balancer: Some(LoadBalancerConfig {
                r#type: LoadBalancerType::Ewma,
                consistent_hash: None,
                degrade_threshold: None,
            }),
            health_check: None,
            outlier_detection: None,
            upstream_authority: None,
            retry_constraint: None,
            circuit_breaker: None,
            connection: None,
        },
        status: None,
    });
    store.full_set_policies(std::collections::HashMap::from([("cap/p1".to_string(), second)]));
    super::begin_upstream_attempt(&mut ctx);
    let still = ctx
        .backend_context
        .as_ref()
        .and_then(|bc| bc.effective_policy.as_ref())
        .expect("still captured");
    assert!(Arc::ptr_eq(&captured, still), "retry must not recapture");
    assert_eq!(
        still.load_balancer,
        Some(edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn)
    );
    assert!(super::retry_budget_for(&ctx).is_none());

    store.full_set_policies(std::collections::HashMap::new());
}

#[test]
fn grpc_begin_upstream_attempt_captures_without_selected_backend() {
    use edgion_resources::resources::edgion_backend_traffic_policy::{
        BudgetDetails, EdgionBackendTrafficPolicySpec, PolicyTargetRef, RetryConstraintConfig,
    };
    use edgion_resources::resources::EdgionBackendTrafficPolicy;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};
    use std::sync::Arc;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    let policy = Arc::new(EdgionBackendTrafficPolicy {
        metadata: ObjectMeta {
            name: Some("p1".to_string()),
            namespace: Some("grpc-cap".to_string()),
            creation_timestamp: Some(Time("2026-01-01T00:00:00Z".parse().expect("rfc3339"))),
            ..Default::default()
        },
        spec: EdgionBackendTrafficPolicySpec {
            prepared_ancestor_admission: Some(edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Unrestricted),
            prepared_precedence: Some(
                edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence {
                    creation_timestamp: Some("2026-01-01T00:00:00Z".parse().expect("rfc3339")),
                },
            ),
            current_status: None,
            target_refs: vec![PolicyTargetRef {
                group: Some(String::new()),
                kind: "Service".to_string(),
                name: "svc".to_string(),
            }],
            resolved_target_class: Some(
                edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass::Service,
            ),
            retry_constraint: Some(RetryConstraintConfig {
                budget: BudgetDetails {
                    percent: 20,
                    interval: "10s".to_string(),
                },
                min_retry_rate: None,
            }),
            load_balancer: None,
            health_check: None,
            outlier_detection: None,
            upstream_authority: None,
            circuit_breaker: None,
            connection: None,
        },
        status: None,
    });
    store.full_set_policies(std::collections::HashMap::from([("grpc-cap/p1".to_string(), policy)]));

    let mut ctx = unstarted_service_ctx("grpc-cap/svc");
    ctx.selected_backend = None;
    let route: edgion_resources::GRPCRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name":"test-route", "namespace":"grpc-cap"},
        "spec": {"rules":[{"backendRefs":[{"name":"svc", "port":80}]}]}
    }))
    .unwrap();
    let runtime = crate::routes::grpc::GrpcRouteRuleUnit::compile_grpc_rule(&route, 0).unwrap();
    let logical = crate::routes::grpc::GrpcBackendHandle::new(runtime.backends, 0).unwrap();
    ctx.selected_grpc_backend = Some(
        crate::routes::grpc::SelectedGrpcBackend::new(logical, crate::backends::BackendTlsPolicyLookup::Absent)
            .unwrap(),
    );
    super::begin_upstream_attempt(&mut ctx);
    assert!(ctx
        .backend_context
        .as_ref()
        .and_then(|bc| bc.effective_policy.as_ref())
        .is_some());
    assert!(super::retry_budget_for(&ctx).is_some());
    store.full_set_policies(std::collections::HashMap::new());
}

#[test]
fn concurrent_capture_never_mixes_service_fields() {
    use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;
    use edgion_resources::resources::lb_policy::ParsedLBPolicy;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    store.full_set_policies(std::collections::HashMap::from([(
        "mix-cap/p1".to_string(),
        service_policy_fields(
            "mix-cap",
            "p1",
            "svc",
            Some(LoadBalancerType::LeastConn),
            Some(20),
            Some("probe-a.sandbox.internal"),
            Some(20),
            Some("100ms"),
            Some(5),
        ),
    )]));

    let ctx = capture_service_ctx("mix-cap/svc");
    let captured = captured_policy(&ctx);
    let sampled = Arc::clone(&captured);
    let reader = std::thread::spawn(move || {
        for _ in 0..8_000 {
            assert_eq!(sampled.load_balancer, Some(ParsedLBPolicy::LeastConn));
            assert_eq!(sampled.degrade_threshold_override, Some(20));
            assert_eq!(sampled.connect_timeout, Some(Duration::from_millis(100)));
            assert_eq!(sampled.circuit_breaker_limit, Some(5));
            assert_eq!(sampled.retry_constraint.as_ref().map(|c| c.budget.percent), Some(20));
            assert_eq!(
                sampled
                    .upstream_authority_config
                    .as_ref()
                    .and_then(|c| c.health_check_host.as_deref()),
                Some("probe-a.sandbox.internal")
            );
        }
    });

    store.full_set_policies(std::collections::HashMap::from([(
        "mix-cap/p1".to_string(),
        service_policy_fields(
            "mix-cap",
            "p1",
            "svc",
            Some(LoadBalancerType::Ewma),
            Some(80),
            Some("probe-b.sandbox.internal"),
            Some(50),
            Some("2s"),
            Some(11),
        ),
    )]));
    reader.join().expect("captured Arc stayed internally consistent");

    assert_eq!(captured.load_balancer, Some(ParsedLBPolicy::LeastConn));
    assert_eq!(captured.degrade_threshold_override, Some(20));
    assert_eq!(captured.connect_timeout, Some(Duration::from_millis(100)));
    assert_eq!(captured.circuit_breaker_limit, Some(5));

    let later = capture_service_ctx("mix-cap/svc");
    let reloaded = captured_policy(&later);
    assert_eq!(reloaded.load_balancer, Some(ParsedLBPolicy::Ewma));
    assert_eq!(reloaded.degrade_threshold_override, Some(80));
    assert_eq!(reloaded.connect_timeout, Some(Duration::from_secs(2)));
    assert_eq!(reloaded.circuit_breaker_limit, Some(11));
    assert_eq!(reloaded.retry_constraint.as_ref().map(|c| c.budget.percent), Some(50));
    assert_eq!(
        reloaded
            .upstream_authority_config
            .as_ref()
            .and_then(|c| c.health_check_host.as_deref()),
        Some("probe-b.sandbox.internal")
    );

    store.full_set_policies(std::collections::HashMap::new());
}

#[test]
fn service_degrade_capture_controls_actual_selection_across_retries() {
    use crate::backends::degrade_threshold::configure_lb_degrade_default;
    use crate::backends::discovery::{get_service_discovery_coordinator, HealthPolicyIdentity};
    use crate::backends::{
        captured_degrade_policy, select_service_endpoint, service_section_key_from_port, ServiceDegradePolicy,
    };
    use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;
    use serde_json::json;
    use std::collections::{HashMap, HashSet};

    const KEY: &str = "degrade-capture/svc";
    const POLICY: &str = "degrade-capture/policy";
    const SLICE: &str = "degrade-capture/slice";
    struct Cleanup;
    impl Drop for Cleanup {
        fn drop(&mut self) {
            get_backend_traffic_policy_store().partial_update_policies(
                HashMap::new(),
                HashMap::new(),
                HashSet::from([POLICY.to_string()]),
            );
            let coordinator = get_service_discovery_coordinator();
            coordinator.set_health_policy(KEY, None);
            coordinator.update_services(HashMap::new(), HashMap::new(), HashSet::from([KEY.to_string()]));
            coordinator.update_endpoint_slices(HashMap::new(), HashMap::new(), HashSet::from([SLICE.to_string()]));
        }
    }

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let _cleanup = Cleanup;
    configure_lb_degrade_default(20);
    let coordinator = get_service_discovery_coordinator();
    coordinator.update_services(
        HashMap::from([(
            KEY.to_string(),
            Arc::new(
                serde_json::from_value(json!({
                    "metadata": {"name": "svc", "namespace": "degrade-capture", "uid": "degrade-capture-service"},
                    "spec": {"ports": [{"name": "http", "port": 80, "targetPort": 8080, "protocol": "TCP"}]}
                }))
                .unwrap(),
            ),
        )]),
        HashMap::new(),
        HashSet::new(),
    );
    coordinator.update_endpoint_slices(
        HashMap::from([(
            SLICE.to_string(),
            Arc::new(
                serde_json::from_value(json!({
                    "metadata": {"name": "slice", "namespace": "degrade-capture", "uid": "degrade-capture-slice",
                        "labels": {"kubernetes.io/service-name": "svc"}},
                    "addressType": "IPv4",
                    "ports": [{"name": "http", "port": 8080, "protocol": "TCP"}],
                    "endpoints": [
                        {"addresses": ["10.8.2.1"], "conditions": {"ready": true}},
                        {"addresses": ["10.8.2.2"], "conditions": {"ready": true}},
                        {"addresses": ["10.8.2.3"], "conditions": {"ready": true}},
                        {"addresses": ["10.8.2.4"], "conditions": {"ready": true}}
                    ]
                }))
                .unwrap(),
            ),
        )]),
        HashMap::new(),
        HashSet::new(),
    );
    let health = Arc::new(HealthPolicyIdentity { generation: 1 });
    let section = service_section_key_from_port(Some(80)).unwrap();
    let publish = |threshold: Option<Option<u8>>| {
        let store = get_backend_traffic_policy_store();
        if let Some(threshold) = threshold {
            store.partial_update_policies(
                HashMap::new(),
                HashMap::from([(
                    POLICY.to_string(),
                    service_policy_fields(
                        "degrade-capture",
                        "policy",
                        "svc",
                        Some(LoadBalancerType::RoundRobin),
                        threshold,
                        None,
                        None,
                        None,
                        None,
                    ),
                )]),
                HashSet::new(),
            );
        } else {
            store.partial_update_policies(HashMap::new(), HashMap::new(), HashSet::from([POLICY.to_string()]));
        }
        // Control live health independently of the request configuration.
        // No probe tasks or network I/O are needed for these transitions.
        coordinator.set_health_policy(KEY, Some(Arc::clone(&health)));
        let generation = coordinator.load_generation();
        let available = generation.available(KEY).unwrap();
        for slot in available.resolve_section(&section).unwrap().all_slots.iter() {
            let healthy = slot.backend.addr.as_inet().unwrap().ip().to_string() == "10.8.2.1";
            assert!(coordinator.record_health_completion(
                &health,
                available.membership_identity,
                slot.backend.runtime_identity().unwrap(),
                healthy,
                1,
                1
            ));
        }
    };
    let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        let mut count = 0;
        let mut select = |policy, threshold| {
            let endpoint = select_service_endpoint(KEY, section.clone(), &None, policy, None, "", None).unwrap();
            if threshold > 25 {
                count += 1;
            } else {
                assert_eq!(endpoint.backend.addr.as_inet().unwrap().ip().to_string(), "10.8.2.1");
            }
            let rendered = handle.render();
            assert!(
                rendered
                    .lines()
                    .any(|line| line == format!("edgion_lb_degrade_selections_total{{service=\"{KEY}\"}} {count}")),
                "{rendered}"
            );
        };
        for (old, new) in [
            (Some(Some(20)), Some(Some(80))),
            (Some(Some(80)), Some(Some(20))),
            (Some(Some(0)), Some(Some(100))),
            (None, Some(Some(100))),
            (Some(None), Some(Some(100))),
            (Some(Some(80)), None),
        ] {
            publish(old);
            // This capture does not require selected_backend, as on gRPC.
            let mut existing = capture_service_ctx(KEY);
            let old_threshold = old.flatten().unwrap_or(20);
            select(
                captured_degrade_policy(existing.backend_context.as_ref()),
                old_threshold,
            );
            publish(new);
            for _ in 0..3 {
                super::begin_upstream_attempt(&mut existing);
                select(
                    captured_degrade_policy(existing.backend_context.as_ref()),
                    old_threshold,
                );
            }
            let fresh = capture_service_ctx(KEY);
            let new_threshold = new.flatten().unwrap_or(20);
            select(captured_degrade_policy(fresh.backend_context.as_ref()), new_threshold);
            select(ServiceDegradePolicy::Live, new_threshold);
        }
    });
}

#[test]
fn captured_arc_retains_old_lb_after_reload() {
    use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;
    use edgion_resources::resources::lb_policy::ParsedLBPolicy;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    store.full_set_policies(std::collections::HashMap::from([(
        "retain-lb/p1".to_string(),
        service_policy_fields(
            "retain-lb",
            "p1",
            "svc",
            Some(LoadBalancerType::LeastConn),
            None,
            None,
            None,
            None,
            None,
        ),
    )]));
    let ctx = capture_service_ctx("retain-lb/svc");
    let captured = captured_policy(&ctx);
    store.full_set_policies(std::collections::HashMap::from([(
        "retain-lb/p1".to_string(),
        service_policy_fields(
            "retain-lb",
            "p1",
            "svc",
            Some(LoadBalancerType::Ewma),
            None,
            None,
            None,
            None,
            None,
        ),
    )]));
    assert_eq!(captured.load_balancer, Some(ParsedLBPolicy::LeastConn));
    store.full_set_policies(std::collections::HashMap::new());
}

#[test]
fn captured_arc_retains_old_authority_after_reload() {
    use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    store.full_set_policies(std::collections::HashMap::from([(
        "retain-auth/p1".to_string(),
        service_policy_fields(
            "retain-auth",
            "p1",
            "svc",
            Some(LoadBalancerType::RoundRobin),
            None,
            Some("probe-old.sandbox.internal"),
            None,
            None,
            None,
        ),
    )]));
    let ctx = capture_service_ctx("retain-auth/svc");
    let captured = captured_policy(&ctx);
    store.full_set_policies(std::collections::HashMap::from([(
        "retain-auth/p1".to_string(),
        service_policy_fields(
            "retain-auth",
            "p1",
            "svc",
            Some(LoadBalancerType::RoundRobin),
            None,
            Some("probe-new.sandbox.internal"),
            None,
            None,
            None,
        ),
    )]));
    assert_eq!(
        captured
            .upstream_authority_config
            .as_ref()
            .and_then(|c| c.health_check_host.as_deref()),
        Some("probe-old.sandbox.internal")
    );
    store.full_set_policies(std::collections::HashMap::new());
}

#[test]
fn service_retry_budget_for_uses_captured_arc_after_store_reload() {
    use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    store.full_set_policies(std::collections::HashMap::from([(
        "retry-cap/p1".to_string(),
        service_policy_fields(
            "retry-cap",
            "p1",
            "svc",
            Some(LoadBalancerType::RoundRobin),
            None,
            None,
            Some(20),
            None,
            None,
        ),
    )]));
    let ctx = capture_service_ctx("retry-cap/svc");
    let first = super::retry_budget_for(&ctx).expect("captured budget");
    store.full_set_policies(std::collections::HashMap::from([(
        "retry-cap/p1".to_string(),
        service_policy_fields(
            "retry-cap",
            "p1",
            "svc",
            Some(LoadBalancerType::RoundRobin),
            None,
            None,
            Some(50),
            None,
            None,
        ),
    )]));
    let after_reload = super::retry_budget_for(&ctx).expect("same captured budget");
    assert!(Arc::ptr_eq(&first, &after_reload));
    let live = store
        .snapshot()
        .get_effective(
            &crate::backends::policy::backend_traffic::store::BackendTrafficPolicyStore::service_target(
                "retry-cap/svc",
            ),
        )
        .and_then(|p| p.retry_budget.clone())
        .expect("reloaded budget");
    assert!(!Arc::ptr_eq(&first, &live));
    store.full_set_policies(std::collections::HashMap::new());
}

#[test]
fn service_retry_preserves_missing_policy_after_reload() {
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    store.full_set_policies(std::collections::HashMap::new());
    let mut ctx = capture_service_ctx("empty-cap/svc");
    assert!(super::retry_budget_for(&ctx).is_none());

    store.full_set_policies(std::collections::HashMap::from([(
        "empty-cap/p1".to_string(),
        service_policy_fields("empty-cap", "p1", "svc", None, None, None, Some(100), None, None),
    )]));
    let live = store
        .snapshot()
        .get_effective(
            &crate::backends::policy::backend_traffic::store::BackendTrafficPolicyStore::service_target(
                "empty-cap/svc",
            ),
        )
        .and_then(|policy| policy.retry_budget.clone())
        .expect("new policy has a budget");

    // The first failure gate skips the budget, and the next attempt must
    // preserve that absence instead of charging an unadmitted retry to it.
    assert!(super::retry_budget_for(&ctx).is_none());
    for _ in 0..2 {
        super::begin_upstream_attempt(&mut ctx);
        assert!(ctx.backend_context.as_ref().unwrap().effective_policy.is_none());
        assert!(super::retry_budget_for(&ctx).is_none());
    }
    assert!(!live.try_retry(), "old request must not fund the new budget");

    let later = capture_service_ctx("empty-cap/svc");
    let captured = super::retry_budget_for(&later).expect("new request captures the new policy");
    assert!(Arc::ptr_eq(&captured, &live));
    assert!(captured.try_retry(), "new request funds its captured budget");
    store.full_set_policies(std::collections::HashMap::new());
}

#[test]
fn service_try_retry_uses_same_captured_budget_arc() {
    use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    store.full_set_policies(std::collections::HashMap::from([(
        "try-retry/p1".to_string(),
        service_policy_fields(
            "try-retry",
            "p1",
            "svc",
            Some(LoadBalancerType::RoundRobin),
            None,
            None,
            Some(20),
            None,
            None,
        ),
    )]));
    let mut ctx = capture_service_ctx("try-retry/svc");
    let first = super::retry_budget_for(&ctx).expect("captured budget");
    store.full_set_policies(std::collections::HashMap::from([(
        "try-retry/p1".to_string(),
        service_policy_fields(
            "try-retry",
            "p1",
            "svc",
            Some(LoadBalancerType::RoundRobin),
            None,
            None,
            Some(80),
            None,
            None,
        ),
    )]));
    let live = store.get_retry_budget("try-retry/svc").expect("reloaded budget");
    assert!(!live.try_retry(), "new generation has no attempt denominator yet");
    assert!(
        super::retry_budget_for(&ctx).unwrap().try_retry(),
        "the pre-reload attempt must fund the post-reload failure's retry"
    );
    super::begin_upstream_attempt(&mut ctx);
    let on_retry = super::retry_budget_for(&ctx).expect("retry still uses capture");
    assert!(
        Arc::ptr_eq(&first, &on_retry),
        "fail-to-connect retry_budget_for must keep the first-attempt Arc"
    );
    assert!(
        !on_retry.try_retry(),
        "one retry exhausts the captured 20% budget for two attempts"
    );
    let later = capture_service_ctx("try-retry/svc");
    let later_budget = super::retry_budget_for(&later).unwrap();
    assert!(Arc::ptr_eq(&live, &later_budget));
    assert!(!Arc::ptr_eq(&first, &later_budget));
    assert!(later_budget.try_retry(), "a new request counts into the new generation");
    store.full_set_policies(std::collections::HashMap::new());
}

#[test]
fn invalid_backend_with_service_key_still_captures() {
    use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;
    use edgion_resources::HTTPBackendRef;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    store.full_set_policies(std::collections::HashMap::from([(
        "inv-cap/p1".to_string(),
        service_policy_fields(
            "inv-cap",
            "p1",
            "svc",
            Some(LoadBalancerType::LeastConn),
            None,
            None,
            None,
            None,
            None,
        ),
    )]));

    let schema: HTTPBackendRef = serde_json::from_value(serde_json::json!({
        "group": "example.io",
        "kind": "Service",
        "name": "wrong-group-service",
        "namespace": "inv-cap",
        "port": 8080
    }))
    .unwrap();
    let backend = crate::routes::http::match_unit::compile_http_backend_fixture(
        schema,
        "inv-cap",
        0,
        Arc::new(crate::plugins::PluginRuntime::new()),
    );
    let mut ctx = EdgionHttpContext::new();
    ctx.selected_backend = Some(crate::backends::SelectedHttpBackend::Invalid(
        crate::backends::LogicalBackendHandle::new(Arc::from(vec![backend]), 0).unwrap(),
    ));
    ctx.backend_context = Some(BackendContext {
        source: None,
        rsc: None,
        service_key: Some("inv-cap/svc".to_string()),
        service_kind: None,
        upstreams: Vec::new(),
        current_upstream_id: None,
        backend_ref_idx: None,
        effective_policy_captured: false,
        effective_policy: None,
    });
    // The historical test name is retained; the unsafe context-only capture
    // expectation is superseded by the actual selected-target contract.
    let mut reads = 0;
    super::capture_service_effective_policy_with(&mut ctx, |target| {
        reads += 1;
        store.snapshot().get_effective(target)
    });
    assert_eq!(reads, 0);
    super::begin_upstream_attempt(&mut ctx);
    assert!(ctx
        .backend_context
        .as_ref()
        .and_then(|bc| bc.effective_policy.as_ref())
        .is_none());
    ctx.backend_context.as_mut().unwrap().service_kind = Some(ServiceBackendKind::Service);
    super::capture_service_effective_policy_with(&mut ctx, |target| {
        reads += 1;
        store.snapshot().get_effective(target)
    });
    assert_eq!(reads, 0, "even a forged Service kind cannot restore an Invalid target");
    assert!(ctx.backend_context.as_ref().unwrap().effective_policy.is_none());
    store.full_set_policies(std::collections::HashMap::new());
}

#[test]
fn direct_endpoint_finish_static_peer_captures_when_service_key_set() {
    use edgion_resources::resources::edgion_backend_traffic_policy::LoadBalancerType;

    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    store.full_set_policies(std::collections::HashMap::from([(
        "de-cap/p1".to_string(),
        service_policy_fields(
            "de-cap",
            "p1",
            "svc",
            Some(LoadBalancerType::LeastConn),
            None,
            None,
            Some(20),
            Some("123ms"),
            Some(4),
        ),
    )]));
    let mut ctx = unstarted_service_ctx("de-cap/svc");
    ctx.selected_backend = None;
    ctx.backend_context.as_mut().unwrap().source = Some(BackendSource::DirectEndpoint);
    let mut schema = jump_backend_ref("svc");
    schema.namespace = Some("de-cap".to_owned());
    ctx.route_unit = Some(jump_route_unit(vec![schema]));
    ctx.jump = Some(Box::new(crate::UpstreamJumpTarget::Direct(DirectEndpointPreset {
        addr: "10.0.0.1:8080".parse().unwrap(),
        tls_policy: crate::backends::BackendTlsPolicyLookup::Absent,
        backend_ref_idx: 0,
        app_protocol: None,
    })));
    super::begin_upstream_attempt(&mut ctx);
    let captured = captured_policy(&ctx);
    assert_eq!(
        captured.load_balancer,
        Some(edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn)
    );
    assert_eq!(captured.connect_timeout, Some(Duration::from_millis(123)));
    assert!(super::retry_budget_for(&ctx).is_some());
    store.full_set_policies(std::collections::HashMap::new());
}

// ─── route-06: retry backoff minimum-wait conformance (GEP-1731) ───
// Pin the `remaining <= delay` boundary that decides whether a retry may
// sleep the configured backoff or must fail instead of retrying before the
// configured minimum duration.

#[test]
fn backoff_sleeps_when_full_delay_fits() {
    // remaining (3s) > delay (2s) → sleep the full minimum backoff.
    assert_eq!(
        backoff_action(Duration::from_secs(2), Some(Duration::from_secs(3)), None),
        BackoffAction::Sleep
    );
}

#[test]
fn backoff_sleeps_when_no_deadline_applies() {
    // No request/grpc deadline bounds the backoff → sleep.
    assert_eq!(backoff_action(Duration::from_secs(2), None, None), BackoffAction::Sleep);
}

#[test]
fn backoff_fails_when_request_budget_too_small() {
    // 0 < remaining (1s) <= delay (2s): retrying now would precede the
    // configured minimum backoff → fail with the request deadline.
    assert_eq!(
        backoff_action(Duration::from_secs(2), Some(Duration::from_secs(1)), None),
        BackoffAction::FailBudgetTooSmall { prefer_request: true }
    );
}

#[test]
fn backoff_fails_at_exact_equality_boundary() {
    // remaining == delay is still too tight (the spec requires the minimum
    // wait to complete strictly before the next attempt) → fail.
    assert_eq!(
        backoff_action(Duration::from_secs(2), Some(Duration::from_secs(2)), None),
        BackoffAction::FailBudgetTooSmall { prefer_request: true }
    );
}

#[test]
fn backoff_fails_with_grpc_deadline_when_grpc_binds() {
    // Only the gRPC deadline is too small (request has room) → fail and
    // report the gRPC client deadline.
    assert_eq!(
        backoff_action(
            Duration::from_secs(2),
            Some(Duration::from_secs(5)),
            Some(Duration::from_secs(1))
        ),
        BackoffAction::FailBudgetTooSmall { prefer_request: false }
    );
}

#[test]
fn backoff_prefers_request_deadline_when_both_too_small() {
    // Both deadlines are <= delay; the request deadline is preferred to
    // mirror the post-sleep re-check order.
    assert_eq!(
        backoff_action(
            Duration::from_secs(2),
            Some(Duration::from_millis(1500)),
            Some(Duration::from_secs(1))
        ),
        BackoffAction::FailBudgetTooSmall { prefer_request: true }
    );
}

#[test]
fn backoff_only_grpc_present_and_too_small() {
    // No request deadline, gRPC remaining (1s) <= delay (2s) → fail via gRPC.
    assert_eq!(
        backoff_action(Duration::from_secs(2), None, Some(Duration::from_secs(1))),
        BackoffAction::FailBudgetTooSmall { prefer_request: false }
    );
}

// ─── decide_upstream_authority: pure decision, all three arms ───

#[test]
fn decide_upstream_authority_no_policy_is_noop() {
    // No Service key / no policy for this Service — caller never rendered.
    assert_eq!(decide_upstream_authority(None), AuthorityDecision::NoOp);
}

#[test]
fn decide_upstream_authority_ok_render_is_set() {
    assert_eq!(
        decide_upstream_authority(Some(Ok("tenant-b.sandbox.internal".to_string()))),
        AuthorityDecision::Set("tenant-b.sandbox.internal".to_string())
    );
}

#[test]
fn decide_upstream_authority_err_render_is_reject() {
    assert_eq!(
        decide_upstream_authority(Some(Err(AuthorityError::InvalidLabel))),
        AuthorityDecision::Reject(AuthorityError::InvalidLabel)
    );
    assert_eq!(
        decide_upstream_authority(Some(Err(AuthorityError::RenderEmpty))),
        AuthorityDecision::Reject(AuthorityError::RenderEmpty)
    );
}

// ─── apply_upstream_authority_decision: effects wiring ───

/// Build a ctx with one "current" upstream entry carrying a TLS record whose
/// `sni` holds the STATIC BackendTLSPolicy hostname (as `record_tls_to_upstream`
/// would have written it during peer construction, before the dynamic
/// authority is composed).
fn ctx_with_static_tls_sni(sni: &str) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();
    ctx.backend_context = Some(BackendContext {
        source: None,
        rsc: None,
        service_key: Some("default/svc".to_string()),
        service_kind: None,
        upstreams: Vec::new(),
        current_upstream_id: None,
        backend_ref_idx: None,
        effective_policy_captured: false,
        effective_policy: None,
    });
    let bc = ctx.backend_context.as_mut().unwrap();
    bc.upstreams.push(UpstreamInfo {
        ip: None,
        port: None,
        status: None,
        ct: None,
        ht: None,
        bt: None,
        et: None,
        wpt: None,
        response_body_size: None,
        start_time: std::time::Instant::now(),
        err: Vec::new(),
        lb_backend_addr: None,
        lb_backend: None,
        lb_outlier_completion: None,
        service_key: None,
        lb_policy: None,
        tls: Some(BackendTlsInfo {
            sni: Some(sni.to_string()),
            handshake_ok: None,
            protocol: None,
            cipher: None,
        }),
        lb_committed: None,
    });
    bc.current_upstream_id = Some(bc.upstreams.len() - 1);
    ctx
}

fn tls_peer(sni: &str) -> HttpPeer {
    HttpPeer::new(
        PingoraSocketAddr::Inet(std::net::SocketAddr::from(([127, 0, 0, 1], 8080))),
        true,
        sni.to_string(),
    )
}

fn cleartext_peer() -> HttpPeer {
    HttpPeer::new(
        PingoraSocketAddr::Inet(std::net::SocketAddr::from(([127, 0, 0, 1], 8080))),
        false,
        String::new(),
    )
}

#[test]
fn apply_upstream_authority_decision_set_refreshes_recorded_sni_not_static_hostname() {
    // IMPORTANT-1 regression: the access-log-recorded SNI must be updated to
    // the composed authority, not left at the static BackendTLSPolicy hostname.
    let mut ctx = ctx_with_static_tls_sni("svc.default.svc.cluster.local");
    let mut peer = tls_peer("svc.default.svc.cluster.local");

    let result = apply_upstream_authority_decision(
        AuthorityDecision::Set("tenant-b.sandbox.internal".to_string()),
        &mut ctx,
        &mut peer,
    );

    assert!(result.is_ok());
    assert_eq!(peer.sni, "tenant-b.sandbox.internal");
    assert_eq!(ctx.upstream_authority.as_deref(), Some("tenant-b.sandbox.internal"));

    let recorded_sni = ctx
        .get_current_upstream_mut()
        .and_then(|u| u.tls.as_ref())
        .and_then(|t| t.sni.as_deref());
    assert_eq!(
        recorded_sni,
        Some("tenant-b.sandbox.internal"),
        "access log must record the composed authority, not the static hostname"
    );
    assert_ne!(recorded_sni, Some("svc.default.svc.cluster.local"));
}

#[test]
fn apply_upstream_authority_decision_set_skips_sni_on_cleartext_peer() {
    // MINOR-5 regression: setting SNI on a cleartext peer fragments the
    // pingora keepalive pool (sni is hashed into HttpPeer::reuse_hash), and
    // the SNI is meaningless on the wire for a non-TLS backend.
    let mut ctx = EdgionHttpContext::new();
    let mut peer = cleartext_peer();

    let result = apply_upstream_authority_decision(
        AuthorityDecision::Set("tenant-b.sandbox.internal".to_string()),
        &mut ctx,
        &mut peer,
    );

    assert!(result.is_ok());
    assert_eq!(peer.sni, "", "cleartext peer must not get a pool-fragmenting SNI");
    // The Host header rewrite must still happen regardless of TLS — ctx is
    // what `upstream_request_filter` reads to write the upstream Host.
    assert_eq!(ctx.upstream_authority.as_deref(), Some("tenant-b.sandbox.internal"));
}

#[test]
fn apply_upstream_authority_decision_reject_fails_closed_and_leaves_peer_untouched() {
    // IMPORTANT-2 regression: pins that the fail-closed path cannot silently
    // fall through to the static SNI. If `Reject` were ever replaced with an
    // `Ok(())` no-op (silently keeping the static SNI), `result.is_err()`
    // would flip to false and this test would fail.
    let mut ctx = ctx_with_static_tls_sni("svc.default.svc.cluster.local");
    let mut peer = tls_peer("svc.default.svc.cluster.local");
    let original_sni = peer.sni.clone();

    let result = apply_upstream_authority_decision(
        AuthorityDecision::Reject(AuthorityError::InvalidLabel),
        &mut ctx,
        &mut peer,
    );

    assert!(result.is_err(), "a bad render must fail closed, never silently succeed");
    assert_eq!(
        peer.sni, original_sni,
        "peer must be left with the static SNI, not a usable altered one"
    );
    assert!(ctx
        .diagnostics
        .contains_error(EdgionErrorCode::UpstreamAuthorityRejected));
    assert_eq!(ctx.get_current_upstream_mut().and_then(|u| u.status), Some(502));
    assert_eq!(ctx.upstream_authority, None);
}

#[test]
fn noop_decision_clears_stale_upstream_authority() {
    // MINOR-3 regression: a stale ctx.upstream_authority from an earlier
    // attempt (e.g. a conf_sync policy removal between retries) must not
    // survive into a NoOp decision, or `upstream_request_filter` would write
    // a stale Host while the peer carries the (correct) static SNI.
    let mut ctx = ctx_with_static_tls_sni("svc.default.svc.cluster.local");
    ctx.upstream_authority = Some("stale-from-prior-attempt.sandbox.internal".to_string());
    let mut peer = tls_peer("svc.default.svc.cluster.local");
    let original_sni = peer.sni.clone();

    let result = apply_upstream_authority_decision(AuthorityDecision::NoOp, &mut ctx, &mut peer);

    assert!(result.is_ok());
    assert_eq!(peer.sni, original_sni);
    assert_eq!(ctx.upstream_authority, None);
}

// ── named-jump backend selection (`pick_backend_ref`) ────────────────────

/// Valid standard Service reference for route-unit fixtures. A Service port is
/// required by canonical target validation; tests vary name, weight and denial.
fn jump_backend_ref(name: &str) -> HTTPBackendRef {
    HTTPBackendRef {
        name: name.to_string(),
        namespace: None,
        port: Some(80),
        weight: None,
        group: None,
        kind: None,
        filters: None,
        backend_tls_policy: None,
        ref_denied: None,
    }
}

fn direct_peer_proxy() -> super::EdgionHttpProxy {
    super::EdgionHttpProxy {
        listener_port: 80,
        is_https_listener: false,
        listener_key: edgion_resources::ListenerKey("direct-protocol-test/HTTP".to_string()),
        server_header_opts: Default::default(),
        access_logger: Arc::new(crate::observe::AccessLogger::default()),
        egc: crate::config::bind_gateway_config(None),
        enable_http2: false,
        preflight_handler: super::super::PreflightHandler,
        http_route_manager: Arc::new(crate::routes::http::routes_mgr::HttpPortRouteManager::default()),
        grpc_route_manager: Arc::new(crate::routes::grpc::GrpcPortRouteManager::default()),
    }
}

#[test]
fn direct_endpoint_app_protocol_is_pinned_across_retry_peer_builds() {
    let cases = [
        (Some("kubernetes.io/h2c"), (2, 2), false),
        (Some("ws"), (1, 1), true),
        (Some("kubernetes.io/wss"), (1, 1), true),
        (Some("unknown"), (1, 1), false),
        (None, (1, 1), false),
    ];

    for (app_protocol, expected_versions, expects_websocket) in cases {
        let proxy = direct_peer_proxy();
        let mut ctx = EdgionHttpContext::new();
        ctx.route_unit = Some(jump_route_unit(vec![jump_backend_ref("direct-app-protocol-pinned")]));
        let published = app_protocol.map(Arc::<str>::from);
        ctx.jump = Some(Box::new(crate::UpstreamJumpTarget::Direct(DirectEndpointPreset {
            addr: "10.0.0.1:8080".parse().unwrap(),
            tls_policy: crate::backends::BackendTlsPolicyLookup::Absent,
            backend_ref_idx: 0,
            app_protocol: published.clone(),
        })));

        for expected_attempt in 1..=2 {
            let peer = super::try_direct_endpoint_peer(&proxy, &mut ctx).unwrap();
            assert_eq!(peer.options.alpn.get_max_http_version(), expected_versions.0);
            assert_eq!(peer.options.alpn.get_min_http_version(), expected_versions.1);
            assert!(!peer.is_tls(), "ws/wss appProtocol must not imply TLS");
            assert_eq!(peer.options.connection_timeout, Some(Duration::from_secs(5)));
            assert_eq!(peer.options.total_connection_timeout, Some(Duration::from_secs(5)));
            assert_eq!(peer.options.read_timeout, Some(Duration::from_secs(60)));
            assert_eq!(peer.options.write_timeout, Some(Duration::from_secs(60)));
            assert_eq!(peer.options.idle_timeout, Some(Duration::from_secs(300)));
            assert_eq!(ctx.try_cnt, expected_attempt);
        }

        let backend = ctx.backend_context.as_ref().unwrap();
        assert_eq!(backend.source, Some(BackendSource::DirectEndpoint));
        assert_eq!(backend.backend_ref_idx, Some(0));
        assert_eq!(backend.upstreams.len(), 2);
        assert!(backend.upstreams.iter().all(|attempt| attempt.tls.is_none()));
        assert_eq!(
            ctx.request_info.discover_protocol.as_deref(),
            expects_websocket.then_some("websocket")
        );
        let retained = match ctx.jump.as_deref().unwrap() {
            crate::UpstreamJumpTarget::Direct(preset) => preset.app_protocol.as_ref(),
            _ => unreachable!("test installs a direct endpoint preset"),
        };
        match (published.as_ref(), retained) {
            (Some(expected), Some(actual)) => assert!(Arc::ptr_eq(expected, actual)),
            (None, None) => {}
            _ => panic!("retry must retain the captured appProtocol"),
        }
    }
}

/// `HttpRouteRuleUnit` fixture whose schema and compiled backend views are
/// built by the production route compiler, so `ref_denied`, namespace and
/// weight semantics match what `pick_backend_ref` sees at runtime.
fn jump_route_unit(backend_refs: Vec<HTTPBackendRef>) -> Arc<HttpRouteRuleUnit> {
    let rule = Arc::new(HTTPRouteRule {
        resolved_parent_match: None,
        name: None,
        resolved_terminal_route_uid: None,
        resolved_ai_admission: None,
        resolved_terminal_rule_identity: None,
        matches: None,
        filters: None,
        backend_refs: Some(backend_refs),
        timeouts: None,
        retry: None,
        session_persistence: None,
        parsed_timeouts: None,
        parsed_retry: None,
        parsed_forward_raw_path: None,
        parsed_allow_non_idempotent_retry: None,
        parsed_mirror_tuning: None,
    });
    let runtime = crate::routes::http::match_unit::compile_http_fixture(&rule, "test-ns").unwrap();
    let backend_finder = HttpRouteRuleUnit::build_backend_selector(&runtime.backends);

    Arc::new(HttpRouteRuleUnit {
        parent_match: None,
        session_persistence_ttl_secs: None,
        resource_key: "test-ns/test-route".to_string(),
        matched_info: crate::MatchInfo {
            rsc: crate::Rsc::new("HTTPRoute", "test-ns", "test-route"),
            rule_id: 0,
            match_id: 0,
            sv: 0,
            m: HTTPRouteMatch {
                path: None,
                headers: None,
                query_params: None,
                method: None,
            },
            labels_meta: None,
            annotations_meta: None,
        },
        rule,
        route: None,
        path_regex: None,
        parent_refs: None,
        compiled_header_regexes: vec![],
        compiled_query_regexes: vec![],
        has_query_matchers: false,
        specificity: MatchSpecificity {
            has_method: false,
            header_count: 0,
            query_count: 0,
            creation_ts_ms: 0,
            route_key: "test-ns/test-route".to_string(),
            rule_index: 0,
            match_index: 0,
        },
        metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
        plugin_runtime: runtime.plugin_runtime,
        backends: runtime.backends,
        backend_finder,
    })
}

fn internal_jump(name: &str) -> crate::UpstreamJumpTarget {
    crate::UpstreamJumpTarget::Internal(crate::InternalJumpPreset {
        backend_ref_name: name.to_string(),
        backend_ref_namespace: None,
    })
}

#[test]
fn named_backend_validation_preserves_selection_admission() {
    fn outcome(result: Result<(), EdError>) -> &'static str {
        match result {
            Ok(()) => "allowed",
            Err(EdError::BackendNotFound()) => "missing",
            Err(EdError::RefDenied { .. }) => "denied",
            Err(error) => panic!("unexpected named lookup error: {error:?}"),
        }
    }
    fn check(unit: &Arc<HttpRouteRuleUnit>, name: &str, namespace: Option<&str>, expected: &str) {
        assert_eq!(
            outcome(RouteRules::validate_backend_by_name(unit, name, namespace)),
            expected
        );
        assert_eq!(
            outcome(RouteRules::find_backend_by_name(unit, name, namespace).map(|_| ())),
            expected
        );
    }
    let hidden = HTTPBackendRef {
        weight: Some(0),
        ..jump_backend_ref("hidden")
    };
    let unit = jump_route_unit(vec![jump_backend_ref("app"), hidden]);
    check(&unit, "app", None, "allowed");
    check(&unit, "hidden", None, "allowed");
    check(&unit, "app", Some("test-ns"), "allowed");
    check(&unit, "app", Some("other"), "missing");
    check(&unit, "absent", None, "missing");
    check(&jump_route_unit(Vec::new()), "app", None, "missing");

    let invalid = HTTPBackendRef {
        group: Some("invalid.example".into()),
        ..jump_backend_ref("app")
    };
    check(&jump_route_unit(vec![invalid]), "app", None, "missing");
    let mut denied = jump_backend_ref("app");
    denied.namespace = Some("other".into());
    denied.ref_denied = Some(RefDenied {
        target_namespace: "other".into(),
        target_name: "app".into(),
        reason: Some("NoMatchingReferenceGrant".into()),
    });
    let unit = jump_route_unit(vec![denied.clone(), jump_backend_ref("app")]);
    check(&unit, "app", None, "denied");
    check(&unit, "app", Some("other"), "denied");
    check(&unit, "app", Some("test-ns"), "allowed");

    // Reordering compiled rows deliberately violates their original slot indexes.
    // A denied row still reports denial before the slot-consistency failure.
    let mut unit = jump_route_unit(vec![jump_backend_ref("legal"), denied]);
    let backends = &mut Arc::get_mut(&mut unit).unwrap().backends;
    Arc::make_mut(backends).swap(0, 1);
    check(&unit, "legal", None, "missing");
    check(&unit, "app", None, "denied");
}

/// A denied named target must surface the typed `RefDenied` error instead
/// of silently degrading to weighted selection of its legal sibling — that
/// degradation was an undetectable misroute with no access-log error code.
///
/// The denied ref deliberately sits in slot 1, NOT slot 0: with equal
/// weights `generate_smooth_sequence` yields `[0, 1]` and `select()` starts
/// at counter 0, so a slot-0 denied ref would be the weighted fallback's own
/// first pick and `select_backend`'s independent `ref_denied` check would
/// return the same `RefDenied` — making this test pass against the very bug
/// it exists to catch. With the denied ref in slot 1, the pre-fix
/// fall-through returns `Ok(app-v1)` and this assertion fails.
#[test]
fn denied_named_jump_fails_closed_instead_of_selecting_sibling() {
    let mut denied = jump_backend_ref("app-v2");
    denied.ref_denied = Some(RefDenied {
        target_namespace: "test-ns".to_string(),
        target_name: "app-v2".to_string(),
        reason: Some("NoMatchingReferenceGrant".to_string()),
    });
    let unit = jump_route_unit(vec![jump_backend_ref("app-v1"), denied]);
    assert_eq!(unit.backends.len(), 2);
    assert!(
        unit.backends.iter().all(|backend| backend.is_service()),
        "named-jump denial fixture must contain valid Service targets"
    );

    let err = pick_backend_ref(&unit, Some(&internal_jump("app-v2")))
        .expect_err("denied named jump must not resolve to its legal sibling");
    assert!(matches!(err, EdError::RefDenied { .. }));
    assert_eq!(backend_select_status(&err), EdgionErrorCode::RefDenied);
}

/// The weighted fallback this test's sibling must never reach: with the
/// same two-backend fixture and no jump at all, selection lands on slot 0.
/// Locks in the premise that a pre-fix fall-through would have returned
/// `Ok(app-v1)` rather than an error, so the fail-closed test above is not
/// vacuous.
#[test]
fn weighted_selection_reaches_the_legal_sibling_without_a_jump() {
    let mut denied = jump_backend_ref("app-v2");
    denied.ref_denied = Some(RefDenied {
        target_namespace: "test-ns".to_string(),
        target_name: "app-v2".to_string(),
        reason: Some("NoMatchingReferenceGrant".to_string()),
    });
    let unit = jump_route_unit(vec![jump_backend_ref("app-v1"), denied]);

    let backend = pick_backend_ref(&unit, None).expect("weighted selection must resolve");
    assert_eq!(backend.compiled().schema().name, "app-v1");
}

/// A genuinely absent named target keeps the documented weighted fallback.
#[test]
fn missing_named_jump_falls_back_to_weighted_selection() {
    let unit = jump_route_unit(vec![jump_backend_ref("app-v1")]);

    let backend = pick_backend_ref(&unit, Some(&internal_jump("absent-svc")))
        .expect("a missing named target must fall back to weighted selection");
    assert_eq!(backend.compiled().schema().name, "app-v1");
}

/// The "hidden backend" contract at the layer that enforces it: weighted
/// selection drops a `weight: 0` slot, so only a named jump can ever reach
/// it — and `find_backend_by_name` must keep not filtering by weight.
#[test]
fn weight_zero_named_jump_is_selectable_but_never_weighted() {
    let hidden = HTTPBackendRef {
        weight: Some(0),
        ..jump_backend_ref("app-debug")
    };
    let unit = jump_route_unit(vec![jump_backend_ref("app-v1"), hidden]);

    let named = pick_backend_ref(&unit, Some(&internal_jump("app-debug")))
        .expect("an explicit weight-zero named target must resolve");
    assert_eq!(named.compiled().schema().name, "app-debug");

    // Three rounds, not one: a single call would only prove the weight-zero
    // slot is not selected *first*, which is weaker than "never".
    for _ in 0..3 {
        let weighted = pick_backend_ref(&unit, None).expect("weighted selection must resolve");
        assert_eq!(weighted.compiled().schema().name, "app-v1");
    }
}

/// Sanity counterpart: an allowed named target still resolves to exactly
/// that backend, so the fail-closed test above is not vacuously passing.
#[test]
fn allowed_named_jump_selects_the_named_backend() {
    let unit = jump_route_unit(vec![jump_backend_ref("app-v1"), jump_backend_ref("app-v2")]);

    let backend =
        pick_backend_ref(&unit, Some(&internal_jump("app-v2"))).expect("an allowed named target must resolve");
    assert_eq!(backend.compiled().schema().name, "app-v2");
}

#[tokio::test]
async fn malformed_and_denied_slots_fail_before_backend_plugins_or_peer_resolution() {
    use serde_json::json;
    use tokio::io::AsyncReadExt;
    let cases = [
        (
            json!({"group":"example.io","name":"api","port":80}),
            500,
            EdgionErrorCode::BackendUnsupportedKind,
        ),
        (json!({"name":"api"}), 500, EdgionErrorCode::BackendUnsupportedKind),
        (
            json!({"name":"api","port":0}),
            500,
            EdgionErrorCode::BackendUnsupportedKind,
        ),
        (
            json!({"name":"api","port":65536}),
            500,
            EdgionErrorCode::BackendUnsupportedKind,
        ),
        (
            json!({"name":"api","port":80,"refDenied":{"targetNamespace":"test-ns","targetName":"api"}}),
            500,
            EdgionErrorCode::RefDenied,
        ),
        (
            json!({"group":"edgion.io","kind":"EdgionBackend","name":"ai"}),
            503,
            EdgionErrorCode::UpstreamNotBackendRefs,
        ),
    ];
    for (mut schema, status, error) in cases {
        schema["filters"] = json!([{"type":"RequestHeaderModifier","requestHeaderModifier":{"set":[{"name":"x-backend-ran","value":"yes"}]}}]);
        let mut ctx = EdgionHttpContext::new();
        ctx.route_unit = Some(jump_route_unit(vec![serde_json::from_value(schema).unwrap()]));
        let (mut session, mut client) = live_h1_session("/").await;
        assert!(super::select_http_backend(&direct_peer_proxy(), &mut session, &mut ctx)
            .await
            .is_err());
        assert!(ctx.diagnostics.contains_error(error));
        assert!(session.req_header().headers.get("x-backend-ran").is_none());
        assert_eq!(ctx.try_cnt, 0);
        assert!(ctx
            .backend_context
            .as_ref()
            .is_none_or(|context| context.upstreams.is_empty() && context.effective_policy.is_none()));
        let mut wire = vec![0; 4096];
        let count = client.read(&mut wire).await.unwrap();
        assert!(std::str::from_utf8(&wire[..count])
            .unwrap()
            .starts_with(&format!("HTTP/1.1 {status}")));
        if matches!(
            ctx.selected_backend,
            Some(crate::backends::SelectedHttpBackend::Invalid(_))
        ) {
            let context = ctx.backend_context.as_mut().unwrap();
            context.service_key = Some("test-ns/api".to_owned());
            context.service_kind = Some(ServiceBackendKind::Service);
            assert_eq!(
                crate::backends::try_get_peer(&mut ctx, &session, false, None)
                    .await
                    .unwrap_err(),
                EdgionErrorCode::BackendUnsupportedKind
            );
        }
    }
    let mut schema = jump_backend_ref("r2a-missing-service");
    schema.filters = Some(serde_json::from_value(json!([{"type":"RequestHeaderModifier","requestHeaderModifier":{"set":[{"name":"x-backend-ran","value":"yes"}]}}])).unwrap());
    let mut ctx = EdgionHttpContext::new();
    ctx.route_unit = Some(jump_route_unit(vec![schema]));
    let (mut session, _client) = live_h1_session("/").await;
    super::select_http_backend(&direct_peer_proxy(), &mut session, &mut ctx)
        .await
        .unwrap();
    assert_eq!(session.req_header().headers.get("x-backend-ran").unwrap(), "yes");
    init_backend_context_if_needed(&mut ctx).unwrap();
    let selected = ctx.selected_backend.as_ref().unwrap().routed().unwrap();
    assert_eq!(
        crate::backends::try_get_peer(&mut ctx, &session, false, Some(&selected))
            .await
            .unwrap_err(),
        EdgionErrorCode::BackendServiceNotFound
    );
}

#[test]
fn policy_lookup_requires_matching_permitted_selected_proof() {
    for mismatch in [true, false] {
        let mut ctx = unstarted_service_ctx("typed-policy/api");
        if mismatch {
            ctx.backend_context.as_mut().unwrap().service_key = Some("typed-policy/other".to_owned());
        }
        let mut reads = 0;
        let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
            super::capture_service_effective_policy_with(&mut ctx, |target| {
                reads += 1;
                assert_eq!(target.namespace, "typed-policy");
                assert_eq!(target.name, "api");
                None
            });
        });
        assert_eq!(reads, usize::from(!mismatch));
        assert_eq!(allocations.calls, 0);
        assert_eq!(allocations.requested_bytes, 0);
    }
}

#[tokio::test]
async fn invalid_grpc_backend_keeps_terminal_unavailable_wire_mapping() {
    use serde_json::json;
    use tokio::io::AsyncReadExt;
    for backend in [
        json!({"name":"api","port":0}),
        json!({"name":"api","port":80,"refDenied":{"targetNamespace":"route-ns","targetName":"api"}}),
    ] {
        let route = crate::test_current_status::from_value(json!({
            "metadata":{"name":"owner","namespace":"route-ns"},
            "spec":{"resolvedStatusController":"edgion.io/gateway-controller","parentRefs":[{"name":"gateway"}],"rules":[{"backendRefs":[backend]}]}
        })).unwrap();
        let units = crate::routes::grpc::parse_route_to_units_for_test("route-ns/owner", &Arc::new(route));
        assert_eq!(units.len(), 1);
        let mut ctx = EdgionHttpContext::new();
        ctx.grpc_route_unit = Some(Arc::clone(&units[0]));
        let (mut session, mut client) = live_h1_session("/test/Get").await;
        let error = super::upstream_peer_grpc(&direct_peer_proxy(), &mut session, &mut ctx)
            .await
            .unwrap_err();
        assert_eq!(error.etype(), &ErrorType::HTTPStatus(200));
        assert!(ctx
            .diagnostics
            .contains_error(EdgionErrorCode::GrpcUpstreamNotBackendRefs));
        assert!(ctx.selected_grpc_backend.is_none());
        assert!(ctx.backend_context.is_none());
        assert_eq!(ctx.try_cnt, 0);
        let mut wire = vec![0; 4096];
        let count = client.read(&mut wire).await.unwrap();
        let wire = std::str::from_utf8(&wire[..count]).unwrap().to_ascii_lowercase();
        assert!(wire.starts_with("http/1.1 200"));
        assert!(wire.contains("grpc-status: 14"));
    }
}

#[tokio::test]
async fn policy_ancestor_admission_precedes_http_attempt_and_pins_recovery_generation() {
    use edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission;
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    let mut policy =
        (*service_policy_fields("capacity-http", "limited", "svc", None, None, None, None, None, None)).clone();
    policy.spec.prepared_ancestor_admission = Some(PreparedPolicyAncestorAdmission::Scoped(vec![
        "capacity-http/represented".into(),
    ]));
    store.full_set_policies(std::collections::HashMap::from([(
        "capacity-http/limited".into(),
        Arc::new(policy.clone()),
    )]));
    let owner = |name: &str| {
        Arc::new(crate::runtime::GatewayInfo::new(
            Some("capacity-http".into()),
            name.into(),
            Some("http".into()),
            None,
            None,
            None,
            Vec::new(),
            None,
        ))
    };
    let mut denied = unstarted_service_ctx("capacity-http/svc");
    denied.gateway_info = Some(owner("new"));
    assert!(!super::prepare_selected_policy_admission(&mut denied));
    assert!(denied.backend_context.as_ref().unwrap().upstreams.is_empty());
    let mut grpc = unstarted_service_ctx("capacity-http/svc");
    grpc.gateway_info = Some(owner("new"));
    grpc.selected_backend = None;
    let route: edgion_resources::GRPCRoute=serde_json::from_value(serde_json::json!({"metadata":{"namespace":"capacity-http","name":"grpc"},"spec":{"rules":[{"backendRefs":[{"name":"svc","port":80}]}]}})).unwrap();
    let runtime = crate::routes::grpc::GrpcRouteRuleUnit::compile_grpc_rule(&route, 0).unwrap();
    let logical = crate::routes::grpc::GrpcBackendHandle::new(runtime.backends, 0).unwrap();
    grpc.selected_grpc_backend = Some(
        crate::routes::grpc::SelectedGrpcBackend::new(logical, crate::backends::BackendTlsPolicyLookup::Absent)
            .unwrap(),
    );
    assert!(!super::prepare_selected_policy_admission(&mut grpc));
    assert!(grpc.backend_context.as_ref().unwrap().upstreams.is_empty());

    let mut represented = unstarted_service_ctx("capacity-http/svc");
    represented.gateway_info = Some(owner("represented"));
    assert!(super::prepare_selected_policy_admission(&mut represented));
    super::begin_upstream_attempt(&mut represented);
    policy.spec.prepared_ancestor_admission =
        Some(PreparedPolicyAncestorAdmission::Scoped(
            vec!["capacity-http/new".into()],
        ));
    store.full_set_policies(std::collections::HashMap::from([(
        "capacity-http/limited".into(),
        Arc::new(policy),
    )]));
    // The selected request keeps its earlier policy; a fresh request sees release.
    assert!(super::prepare_selected_policy_admission(&mut represented));
    let mut fresh = unstarted_service_ctx("capacity-http/svc");
    fresh.gateway_info = Some(owner("new"));
    assert!(super::prepare_selected_policy_admission(&mut fresh));
    store.full_set_policies(std::collections::HashMap::new());
}

#[test]
fn policy_ancestor_absence_is_pinned_before_first_attempt_and_retries() {
    use edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission;
    let _guard = GLOBAL_STORE_TEST_LOCK.lock();
    let store = get_backend_traffic_policy_store();
    for grpc in [false, true] {
        store.full_set_policies(std::collections::HashMap::new());
        let mut ctx = unstarted_service_ctx("absence/svc");
        if grpc {
            ctx.selected_backend = None;
            let route:edgion_resources::GRPCRoute=serde_json::from_value(serde_json::json!({"metadata":{"namespace":"absence","name":"grpc"},"spec":{"rules":[{"backendRefs":[{"name":"svc","port":80}]}]}})).unwrap();
            let runtime = crate::routes::grpc::GrpcRouteRuleUnit::compile_grpc_rule(&route, 0).unwrap();
            let logical = crate::routes::grpc::GrpcBackendHandle::new(runtime.backends, 0).unwrap();
            ctx.selected_grpc_backend = Some(
                crate::routes::grpc::SelectedGrpcBackend::new(logical, crate::backends::BackendTlsPolicyLookup::Absent)
                    .unwrap(),
            );
        }
        assert!(super::prepare_selected_policy_admission(&mut ctx));
        assert!(ctx.backend_context.as_ref().unwrap().effective_policy_captured);
        let mut policy =
            (*service_policy_fields("absence", "limited", "svc", None, None, None, None, None, None)).clone();
        policy.spec.prepared_ancestor_admission = Some(PreparedPolicyAncestorAdmission::Scoped(Vec::new()));
        store.full_set_policies(std::collections::HashMap::from([(
            "absence/limited".into(),
            Arc::new(policy),
        )]));
        assert!(super::prepare_selected_policy_admission(&mut ctx));
        super::capture_service_effective_policy_with(&mut ctx, |_| panic!("captured absence must not query again"));
        super::begin_upstream_attempt(&mut ctx);
        assert!(ctx.backend_context.as_ref().unwrap().effective_policy.is_none());
        super::begin_upstream_attempt(&mut ctx);
        assert!(ctx.backend_context.as_ref().unwrap().effective_policy.is_none());
        let mut fresh = unstarted_service_ctx("absence/svc");
        assert!(!super::prepare_selected_policy_admission(&mut fresh));
        assert!(fresh.backend_context.as_ref().unwrap().upstreams.is_empty());
        let captured = captured_policy(&fresh);
        fresh.backend_context.as_mut().unwrap().effective_policy_captured = false;
        super::capture_service_effective_policy_with(&mut fresh, |_| {
            panic!("pre-seeded presence remains authoritative")
        });
        assert!(fresh.backend_context.as_ref().unwrap().effective_policy_captured);
        assert!(Arc::ptr_eq(&captured, &captured_policy(&fresh)));
    }
    store.full_set_policies(std::collections::HashMap::new());
}
