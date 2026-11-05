use super::record_lb_latency_sample;
use crate::lb::backend::LbBackend;
use crate::lb::ewma::score::{units_at, DEFAULT_RTT_US};
use crate::routes::http::match_unit::{HttpRouteRuleUnit, MatchSpecificity};
use crate::{BackendContext, EdgionHttpContext, MatchInfo, Rsc};
use edgion_resources::resources::http_route::ParsedRouteRetry;
use edgion_resources::resources::lb_policy::ParsedLBPolicy;
use edgion_resources::{HTTPRouteMatch, HTTPRouteRule};
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_load_balancing::Backend;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn make_backend() -> Arc<LbBackend> {
    Arc::new(LbBackend::new(Backend {
        addr: SocketAddr::Inet(std::net::SocketAddr::from(([127, 0, 0, 1], 18080))),
        weight: 1,
        ext: Default::default(),
    }))
}

fn ctx_with_attempt(
    backend: Arc<LbBackend>,
    policy: Option<ParsedLBPolicy>,
    start_time: Instant,
    retry_codes: Option<Vec<u16>>,
) -> EdgionHttpContext {
    let mut ctx = EdgionHttpContext::new();
    ctx.backend_context = Some(BackendContext {
        effective_policy_captured: false,
        effective_policy: None,
        source: None,
        rsc: None,
        service_key: None,
        service_kind: None,
        upstreams: Vec::new(),
        current_upstream_id: None,
        backend_ref_idx: None,
    });
    let bc = ctx.backend_context.as_mut().unwrap();
    bc.upstreams.push(crate::UpstreamInfo {
        ip: None,
        port: None,
        status: None,
        ct: None,
        ht: None,
        bt: None,
        et: None,
        wpt: None,
        response_body_size: None,
        start_time,
        err: Vec::new(),
        lb_backend_addr: None,
        lb_backend: Some(backend),
        lb_outlier_completion: None,
        service_key: None,
        lb_policy: policy,
        tls: None,
        lb_committed: None,
    });
    bc.current_upstream_id = Some(0);
    if let Some(codes) = retry_codes {
        ctx.route_unit = Some(Arc::new(route_unit_with_retry_codes(codes)));
    }
    ctx
}

fn route_unit_with_retry_codes(codes: Vec<u16>) -> HttpRouteRuleUnit {
    HttpRouteRuleUnit {
        delegate: None,
        filter_mask: 0,
        session_persistence_ttl_secs: None,
        resource_key: "test-ns/lb-sample".to_string(),
        matched_info: MatchInfo {
            rsc: Rsc::new("HTTPRoute", "test-ns", "lb-sample"),
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
        rule: Arc::new(HTTPRouteRule {
            name: None,
            resolved_terminal_route_uid: None,
            resolved_ai_admission: None,
            resolved_terminal_rule_identity: None,
            matches: None,
            filters: None,
            backend_refs: None,
            timeouts: None,
            retry: None,
            session_persistence: None,
            parsed_timeouts: None,
            parsed_retry: Some(ParsedRouteRetry {
                attempts: None,
                backoff: None,
                codes: Some(codes),
            }),
            parsed_forward_raw_path: None,
            parsed_allow_non_idempotent_retry: None,
            parsed_mirror_tuning: None,
        }),
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
            route_key: "test-ns/lb-sample".to_string(),
            rule_index: 0,
            match_index: 0,
        },
        metric_handles: Arc::new(crate::observe::metrics::route_handles::RouteMetricHandles::empty()),
        plugin_runtime: Arc::new(crate::plugins::PluginRuntime::new()),
        backends: Arc::from(vec![]),
        backend_finder: Arc::new(crate::lb::BackendSelector::new(vec![], vec![])),
    }
}

#[test]
fn ewma_fast_success_is_recorded_at_header_time() {
    let backend = make_backend();
    // A future Instant makes `units_at(header_at)` far from process start.
    // Passing `0` instead would decay this 50 ms peak back to default.
    let header_at = Instant::now() + Duration::from_secs(20);
    let start_time = header_at - Duration::from_millis(50);
    let ctx = ctx_with_attempt(backend.clone(), Some(ParsedLBPolicy::Ewma), start_time, None);

    assert!(record_lb_latency_sample(&ctx, 200, header_at));
    assert_eq!(backend.latency_estimate(units_at(header_at)), 50_000);
}

#[test]
fn non_ewma_policy_does_not_score() {
    let backend = make_backend();
    let header_at = Instant::now();
    let ctx = ctx_with_attempt(
        backend.clone(),
        Some(ParsedLBPolicy::LeastConn),
        header_at - Duration::from_millis(50),
        None,
    );

    assert!(!record_lb_latency_sample(&ctx, 200, header_at));
    assert_eq!(backend.latency_estimate(units_at(header_at)), DEFAULT_RTT_US);
}

#[test]
fn fast_5xx_does_not_lower_the_score() {
    let backend = make_backend();
    let header_at = Instant::now();
    let ctx = ctx_with_attempt(
        backend.clone(),
        Some(ParsedLBPolicy::Ewma),
        header_at - Duration::from_micros(2_000),
        None,
    );

    assert!(!record_lb_latency_sample(&ctx, 503, header_at));
    assert_eq!(backend.latency_estimate(units_at(header_at)), DEFAULT_RTT_US);
}

#[test]
fn slow_5xx_still_raises_the_score() {
    let backend = make_backend();
    let header_at = Instant::now();
    let ctx = ctx_with_attempt(
        backend.clone(),
        Some(ParsedLBPolicy::Ewma),
        header_at - Duration::from_secs(8),
        None,
    );

    assert!(record_lb_latency_sample(&ctx, 503, header_at));
    assert_eq!(backend.latency_estimate(units_at(header_at)), 8_000_000);
}

#[test]
fn retryable_429_is_read_from_the_route_and_is_peak_only() {
    let backend = make_backend();
    let header_at = Instant::now();
    let ctx = ctx_with_attempt(
        backend.clone(),
        Some(ParsedLBPolicy::Ewma),
        header_at - Duration::from_micros(2_000),
        Some(vec![429]),
    );

    assert!(!record_lb_latency_sample(&ctx, 429, header_at));
    assert_eq!(backend.latency_estimate(units_at(header_at)), DEFAULT_RTT_US);
}
