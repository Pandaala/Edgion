use super::*;

#[test]
#[cfg(target_pointer_width = "64")]
fn plugin_state_layout_keeps_hot_and_sparse_costs_separate() {
    assert_eq!(std::mem::size_of::<PluginExecutionState>(), 176);
    assert_eq!(std::mem::size_of::<PluginAuxState>(), 296);
    assert_eq!(std::mem::size_of::<Option<Box<PluginAuxState>>>(), 8);
}

#[test]
fn ordinary_context_keeps_plugin_aux_unallocated_on_reads() {
    let mut ctx = EdgionHttpContext::new();
    assert!(ctx.begin_rate_limit_local_frame(0).is_none());
    assert_eq!(
        ctx.rate_limit_local_outcome(0),
        crate::plugins::runtime::conditions::rate_limit_local_outcome::RateLimitLocalOutcome::NotEvaluated
    );
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert!(!ctx.has_plugin_aux());
        assert!(ctx.stage_logs().is_empty());
        assert!(!ctx.plugin_terminated());
        assert!(ctx.get_ctx_var("missing").is_none());
        assert!(!ctx.has_pending_response_header_ops());
        assert!(ctx.pending_response_header_ops().is_empty());
        assert!(ctx.current_dye_plan().is_none());
        assert!(!ctx.has_mirror_state());
        assert!(ctx.resolved_global_plugins().is_none());
        assert!(ctx.resolved_gateway_plugins().is_none());
        assert!(ctx.resolved_route_ext_refs().is_none());
        assert!(ctx.take_mirror_state().is_none());
    });

    assert!(!ctx.has_plugin_aux());
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}

#[test]
fn hot_plugin_execution_state_remains_allocation_free() {
    let mut ctx = EdgionHttpContext::new();
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        ctx.terminate_plugin_execution();
        assert!(ctx.plugin_terminated());
    });
    assert!(!ctx.has_plugin_aux());
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}

#[test]
fn suppressed_only_stage_does_not_create_an_empty_stage_log() {
    let mut ctx = EdgionHttpContext::new();
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        ctx.finish_plugin_stage("request_filters", Vec::new());
    });
    assert!(ctx.stage_logs().is_empty());
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}

#[test]
fn first_sparse_plugin_effect_allocates_one_aux_box_and_reuses_it() {
    let mut ctx = EdgionHttpContext::new();
    let (_, first) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert!(ctx.claim_json_schema_validation());
    });
    assert!(ctx.has_plugin_aux());
    assert_eq!(first.calls, 1);
    assert_eq!(first.requested_bytes, std::mem::size_of::<PluginAuxState>());

    let (_, later) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert!(!ctx.claim_json_schema_validation());
    });
    assert_eq!(later.calls, 0);
    assert_eq!(later.requested_bytes, 0);
}

#[test]
fn runtime_snapshots_do_not_instantiate_plugin_aux() {
    let mut ctx = EdgionHttpContext::new();
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        ctx.install_resolved_global_plugins(Vec::new());
        ctx.install_resolved_gateway_plugins(Vec::new());
        ctx.install_resolved_route_ext_refs(Vec::new());
    });

    assert!(!ctx.has_plugin_aux());
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);

    ctx.install_resolved_route_ext_refs(vec![None]);
    assert!(!ctx.has_plugin_aux());
}

#[test]
fn dangling_route_and_backend_refs_remain_request_pinned() {
    let mut ctx = EdgionHttpContext::new();
    ctx.install_resolved_route_ext_refs(vec![None]);
    assert!(!ctx.has_plugin_aux());
    assert_eq!(ctx.resolved_route_ext_refs().unwrap().len(), 1);
    assert!(ctx.resolved_route_ext_ref(0).is_none());

    let mut resolve_calls = 0;
    assert!(ctx
        .pinned_backend_ext_ref("default/missing", || {
            resolve_calls += 1;
            None
        })
        .is_none());
    assert!(ctx.has_plugin_aux());
    assert!(ctx
        .pinned_backend_ext_ref("default/missing", || {
            resolve_calls += 1;
            None
        })
        .is_none());
    assert_eq!(resolve_calls, 1);
}
