//! Unit tests for the g1-007 LB runtime ledger cleanup contract.
//!
//! These tests verify the invariants of [`lb_runtime_cleanup`] without
//! standing up a Pingora session: by directly mutating `EdgionHttpContext`
//! fields and calling per-backend atomic methods we reproduce the exact
//! stage sequence that the proxy would emit.
use super::*;
use crate::lb::backend::LbBackend;
use crate::lb::commit_attempt;
use crate::BackendContext;
use pingora_core::protocols::l4::socket::SocketAddr;
use pingora_load_balancing::Backend;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn attached_owner() -> Arc<crate::plugins::http::ext_proc::AttachedExtProc> {
    Arc::new(crate::plugins::http::ext_proc::AttachedExtProc::test_lazy())
}

#[tokio::test]
async fn attached_summary_finalizes_top_level_token_once() {
    use crate::plugins::runtime::log::{PluginLog, StageLogs};

    let identity = "route/requestPlugins/#0".to_string();
    let owner = attached_owner();
    let mut log = PluginLog::new("ExtProc");
    log.set_internal_token(identity.clone());
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.install_attached_ext_proc(identity, owner, false));
    ctx.push_stage_log(StageLogs {
        stage: "request_filters",
        filters: vec![log],
        edgion_plugins: Vec::new(),
    });

    finalize_attached_ext_proc_logs(&mut ctx).await;
    finalize_attached_ext_proc_logs(&mut ctx).await;
    let rendered = serde_json::to_string(&ctx.stage_logs()[0].filters[0]).unwrap();
    assert_eq!(rendered.matches("ext_proc attached").count(), 1);
    assert!(rendered.contains("reason=logging-fallback"));
}

#[tokio::test]
async fn attached_owner_without_stage_log_still_terminalizes() {
    let identity = "route/requestPlugins/#0".to_string();
    let owner = attached_owner();
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.install_attached_ext_proc(identity, Arc::clone(&owner), false));

    finalize_attached_ext_proc_logs(&mut ctx).await;

    assert_eq!(owner.test_state(), ("cancelled", "logging-fallback", true));
    assert!(ctx.stage_logs().is_empty());
}

#[tokio::test]
async fn attached_summary_finds_nested_extension_ref_token_once() {
    use crate::plugins::runtime::log::{EdgionPluginsLog, PluginLog, StageLogs};

    let identity = "extension/ref/requestPlugins/#0".to_string();
    let owner = attached_owner();
    let mut log = PluginLog::new("ExtProc");
    log.set_internal_token(identity.clone());
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.install_attached_ext_proc(identity, owner, false));
    ctx.push_stage_log(StageLogs {
        stage: "request_filters",
        filters: Vec::new(),
        edgion_plugins: vec![EdgionPluginsLog {
            name: "extension-ref".to_string(),
            logs: vec![log],
        }],
    });

    finalize_attached_ext_proc_logs(&mut ctx).await;
    finalize_attached_ext_proc_logs(&mut ctx).await;
    let rendered = serde_json::to_string(&ctx.stage_logs()[0].edgion_plugins[0].logs[0]).unwrap();
    assert_eq!(rendered.matches("ext_proc attached").count(), 1);
    assert!(rendered.contains("reason=logging-fallback"));
}

fn make_backend(ip: [u8; 4], port: u16) -> Arc<LbBackend> {
    Arc::new(LbBackend::new(Backend {
        addr: SocketAddr::Inet(std::net::SocketAddr::from((ip, port))),
        weight: 1,
        ext: Default::default(),
    }))
}

/// Push and commit a new upstream entry through the production ownership path.
fn setup_committed_attempt(
    ctx: &mut EdgionHttpContext,
    backend: Arc<LbBackend>,
    policy: edgion_resources::resources::lb_policy::ParsedLBPolicy,
    start_time: Instant,
) {
    if ctx.backend_context.is_none() {
        ctx.backend_context = Some(BackendContext {
            source: None,
            rsc: None,
            service_key: None,
            service_kind: None,
            upstreams: Vec::new(),
            current_upstream_id: None,
            backend_ref_idx: None,
            effective_policy_captured: false,
            effective_policy: None,
        });
    }
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
        lb_policy: Some(policy),
        tls: None,
        lb_committed: None,
    });
    bc.current_upstream_id = Some(bc.upstreams.len() - 1);
    commit_attempt(bc.upstreams.last_mut().unwrap());
}

#[test]
fn cleanup_no_increment_does_not_touch_other_inflight() {
    let a = make_backend([127, 0, 0, 1], 30001);
    // Another request has an in-flight on A.
    a.inc_conn();
    assert_eq!(a.conn(), 1);

    // This request: an upstream entry exists but connect failed — lb_committed never set.
    let mut ctx = EdgionHttpContext::new();
    ctx.backend_context = Some(BackendContext {
        source: None,
        rsc: None,
        service_key: None,
        service_kind: None,
        upstreams: Vec::new(),
        current_upstream_id: None,
        backend_ref_idx: None,
        effective_policy_captured: false,
        effective_policy: None,
    });
    ctx.push_upstream(None, None);
    // lb_committed stays None (default from push_upstream)

    lb_runtime_cleanup(&mut ctx);

    assert_eq!(
        a.conn(),
        1,
        "cleanup with lb_committed=None must not touch another in-flight count"
    );
}

#[test]
fn cleanup_after_connected_balances_to_zero() {
    let a = make_backend([127, 0, 0, 2], 30001);

    let mut ctx = EdgionHttpContext::new();
    setup_committed_attempt(
        &mut ctx,
        a.clone(),
        edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn,
        Instant::now(),
    );
    assert_eq!(a.conn(), 1);

    lb_runtime_cleanup(&mut ctx);

    assert_eq!(a.conn(), 0);
    assert!(
        ctx.get_current_upstream().unwrap().lb_committed.is_none(),
        "lb_committed must be taken by cleanup"
    );
}

#[test]
fn retry_then_connected_then_logging_balances_both() {
    use crate::routes::http::proxy_http::pg_upstream_peer::begin_upstream_attempt;

    let a = make_backend([127, 0, 0, 3], 30001);
    let b = make_backend([127, 0, 0, 4], 30001);

    let mut ctx = EdgionHttpContext::new();

    // Attempt 1: connected to A.
    setup_committed_attempt(
        &mut ctx,
        a.clone(),
        edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn,
        Instant::now(),
    );
    assert_eq!(a.conn(), 1);

    // Retry: begin_upstream_attempt finalizes attempt 1 with Discard (dec A),
    // then pushes an empty slot for attempt 2.
    begin_upstream_attempt(&mut ctx);
    assert_eq!(a.conn(), 0);
    assert!(
        ctx.backend_context.as_ref().unwrap().upstreams[0]
            .lb_committed
            .is_none(),
        "begin_upstream_attempt must consume upstream 0's lb_committed"
    );

    // Attempt 2: connected to B.
    if let Some(up) = ctx.get_current_upstream_mut() {
        up.lb_backend = Some(b.clone());
        up.lb_policy = Some(edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn);
        commit_attempt(up);
    }
    assert_eq!(b.conn(), 1);

    // Logging cleanup: settles attempt 2 (dec B).
    lb_runtime_cleanup(&mut ctx);

    assert_eq!(a.conn(), 0);
    assert_eq!(b.conn(), 0);
}

#[test]
fn cleanup_records_no_latency_sample() {
    use crate::routes::http::proxy_http::pg_upstream_peer::begin_upstream_attempt;

    // The sample moved to `pg_upstream_response_filter` (response headers).
    // Request-end cleanup must only balance the outstanding-connection
    // count — it must never touch the score, in either mode.
    let a = make_backend([127, 0, 0, 5], 30001);
    let now = crate::lb::ewma::score::now_units();
    let initial = a.latency_estimate(now);

    let mut ctx = EdgionHttpContext::new();
    setup_committed_attempt(
        &mut ctx,
        a.clone(),
        edgion_resources::resources::lb_policy::ParsedLBPolicy::Ewma,
        Instant::now(),
    );

    // Retry finalizes attempt 1; its guard is consumed.
    begin_upstream_attempt(&mut ctx);
    assert!(
        ctx.backend_context.as_ref().unwrap().upstreams[0]
            .lb_committed
            .is_none(),
        "retry must consume upstream 0's guard"
    );

    lb_runtime_cleanup(&mut ctx);

    assert_eq!(
        a.latency_estimate(now),
        initial,
        "neither retry-finalize nor request-end cleanup may score"
    );
    assert_eq!(a.conn(), 0, "but both must balance the connection count");
}

#[test]
fn ewma_policy_balances_conn_like_leastconn() {
    // Ewma now maintains conn_count too, because its score is
    // `latency_estimate * (conn + 1)`.
    let a = make_backend([127, 0, 0, 6], 30001);
    let mut ctx = EdgionHttpContext::new();
    ctx.backend_context = Some(BackendContext {
        source: None,
        rsc: None,
        service_key: None,
        service_kind: None,
        upstreams: Vec::new(),
        current_upstream_id: None,
        backend_ref_idx: None,
        effective_policy_captured: false,
        effective_policy: None,
    });
    ctx.push_upstream(None, None);
    if let Some(up) = ctx.get_current_upstream_mut() {
        up.lb_backend = Some(a.clone());
        up.lb_policy = Some(edgion_resources::resources::lb_policy::ParsedLBPolicy::Ewma);
        up.start_time = Instant::now() - Duration::from_millis(50);
        commit_attempt(up);
    }
    assert_eq!(a.conn(), 1, "Ewma commit must increment");

    lb_runtime_cleanup(&mut ctx);
    assert_eq!(a.conn(), 0, "cleanup must decrement");
}

#[test]
fn cleanup_idempotent_on_double_call() {
    let a = make_backend([127, 0, 0, 7], 30001);

    let mut ctx = EdgionHttpContext::new();
    setup_committed_attempt(
        &mut ctx,
        a.clone(),
        edgion_resources::resources::lb_policy::ParsedLBPolicy::LeastConn,
        Instant::now(),
    );
    assert_eq!(a.conn(), 1);

    lb_runtime_cleanup(&mut ctx);
    assert_eq!(a.conn(), 0);

    // Simulate a separate concurrent in-flight on A.
    a.inc_conn();
    assert_eq!(a.conn(), 1);

    // Second cleanup: lb_committed was taken; must be a no-op.
    lb_runtime_cleanup(&mut ctx);
    assert_eq!(a.conn(), 1, "second cleanup must not decrement again");
}
