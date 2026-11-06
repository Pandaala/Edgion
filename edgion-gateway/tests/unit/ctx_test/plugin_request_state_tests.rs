use super::*;

#[test]
#[cfg(target_pointer_width = "64")]
fn plugin_state_layout_keeps_hot_and_sparse_costs_separate() {
    assert_eq!(std::mem::size_of::<PluginExecutionState>(), 184);
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
    // Publication prepares these slices; installing them per request must only clone the Arc.
    let prepared: Option<Arc<[Arc<PluginRuntime>]>> =
        Some(Arc::from(vec![Arc::new(crate::plugins::runtime::PluginRuntime::new())]));
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        ctx.install_resolved_global_plugins(prepared.clone());
        ctx.install_resolved_gateway_plugins(prepared.clone());
        ctx.install_resolved_route_ext_refs(Vec::new());
    });

    assert!(!ctx.has_plugin_aux());
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
    let (global, gateway) = (ctx.resolved_global_plugins(), ctx.resolved_gateway_plugins());
    assert!(Arc::ptr_eq(global.as_ref().unwrap(), prepared.as_ref().unwrap()));
    assert!(Arc::ptr_eq(gateway.as_ref().unwrap(), prepared.as_ref().unwrap()));
    // Conservative helper contract: installing `None` never clears an installed
    // snapshot (the request path never reaches this case today).
    ctx.install_resolved_global_plugins(None);
    ctx.install_resolved_gateway_plugins(None);
    assert!(ctx.resolved_global_plugins().is_some());
    assert!(ctx.resolved_gateway_plugins().is_some());

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

#[test]
fn backend_hits_and_misses_stay_pinned_through_stage_and_retry_reentry() {
    let original = std::sync::Arc::new(crate::plugins::runtime::PluginRuntime::new());
    let replacement = std::sync::Arc::new(crate::plugins::runtime::PluginRuntime::new());
    let mut ctx = EdgionHttpContext::new();
    let mut reads = 0;
    for attempt in 0..3 {
        for _stage in 0..4 {
            let hit = ctx
                .pinned_backend_ext_ref("default/hit", || {
                    reads += 1;
                    Some(if attempt == 0 {
                        original.clone()
                    } else {
                        replacement.clone()
                    })
                })
                .unwrap();
            assert!(std::sync::Arc::ptr_eq(&hit, &original));
            assert!(ctx
                .pinned_backend_ext_ref("default/missing", || {
                    reads += 1;
                    (attempt > 0).then(|| replacement.clone())
                })
                .is_none());
        }
    }
    assert_eq!(reads, 2);
    assert!(ctx.stage_logs().is_empty());
    assert!(ctx.listener_publication().is_none());
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert!(ctx
            .pinned_backend_ext_ref("default/missing", || panic!("cached miss"))
            .is_none());
        assert!(std::sync::Arc::ptr_eq(
            &ctx.pinned_backend_ext_ref("default/hit", || panic!("cached hit"))
                .unwrap(),
            &original
        ));
        assert!(ctx.resolve_pinned_backend_ext_ref("default/missing").is_none());
        assert!(Arc::ptr_eq(
            &ctx.resolve_pinned_backend_ext_ref("default/hit").unwrap(),
            &original
        ));
    });
    assert_eq!(allocations.calls, 0);
    ctx.capture_listener_publication(crate::plugins::publication::HttpListenerPublication::empty());
    assert!(ctx.resolve_pinned_backend_ext_ref("default/missing").is_none());
    assert!(Arc::ptr_eq(
        &ctx.resolve_pinned_backend_ext_ref("default/hit").unwrap(),
        &original
    ));
}

#[test]
fn listener_capture_is_inline_allocation_free_and_preserves_captured_absence() {
    let empty = crate::plugins::publication::HttpListenerPublication::empty();
    let mut ctx = EdgionHttpContext::new();
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert!(ctx.listener_publication().is_none());
        ctx.capture_listener_publication(empty.clone());
        ctx.capture_listener_publication(empty.clone());
        assert!(Arc::ptr_eq(ctx.listener_publication().unwrap(), &empty));
        assert!(!ctx.has_plugin_aux());
    });
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}

impl EdgionHttpContext {
    pub(crate) fn pinned_backend_ext_ref(
        &mut self,
        key: &str,
        resolve: impl FnOnce() -> Option<Arc<PluginRuntime>>,
    ) -> Option<Arc<PluginRuntime>> {
        self.ensure_plugin_aux().pinned_backend_ext_ref(key, resolve)
    }

    /// Existing policy-only fixtures supply their owner before request entry.
    pub(crate) fn capture_plugin_scope(&mut self, scope: Arc<crate::plugins::publication::GatewayScope>) -> bool {
        if let Some(current) = self.plugin_scope() {
            return current.gateway_key == scope.gateway_key;
        }
        let view = if let Some(view) = self.listener_publication() {
            let Some(selected) = view.select_gateway(&scope.gateway_key) else {
                return false;
            };
            selected
        } else {
            crate::plugins::publication::HttpListenerPublication::fixture_for_scope(scope)
        };
        self.plugin_execution.publication = Some(view);
        true
    }
}
