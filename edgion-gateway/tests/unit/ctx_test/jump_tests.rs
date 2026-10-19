use super::*;

#[test]
#[cfg(target_pointer_width = "64")]
fn jump_target_uses_one_pointer_in_the_base_context() {
    assert_eq!(std::mem::size_of::<UpstreamJumpTarget>(), 72);
    assert_eq!(std::mem::size_of::<Option<Box<UpstreamJumpTarget>>>(), 8);
}

fn fresh_ctx() -> EdgionHttpContext {
    EdgionHttpContext::new()
}

fn sample_internal() -> InternalJumpPreset {
    InternalJumpPreset {
        backend_ref_name: "svc-a".to_string(),
        backend_ref_namespace: None,
    }
}

fn sample_external() -> ExternalJumpPreset {
    ExternalJumpPreset {
        domain: "api.example.com".to_string(),
        port: 443,
        use_tls: true,
        sni: "api.example.com".to_string(),
        preserve_raw_path: false,
        upstream_read_timeout_override: None,
    }
}

fn sample_direct() -> DirectEndpointPreset {
    DirectEndpointPreset {
        addr: "10.0.0.1:8080".parse().unwrap(),
        tls_policy: crate::backends::BackendTlsPolicyLookup::Absent,
        backend_ref_idx: 0,
        app_protocol: None,
    }
}

#[test]
fn direct_preset_clone_retains_published_app_protocol() {
    let app_protocol = BackendAppProtocol::H2c;
    let mut preset = sample_direct();
    preset.app_protocol = Some(app_protocol);

    let cloned = preset.clone();
    assert_eq!(preset.app_protocol, cloned.app_protocol);
}

#[test]
fn first_internal_write_returns_set() {
    let mut ctx = fresh_ctx();
    let outcome = ctx.try_set_jump(UpstreamJumpTarget::Internal(sample_internal()));
    assert_eq!(outcome, SetJumpOutcome::Set);
    assert!(matches!(ctx.jump.as_deref(), Some(UpstreamJumpTarget::Internal(_))));
    assert!(
        ctx.selected_backend.is_none(),
        "Internal must NOT co-write selected_backend"
    );
}

#[test]
fn first_external_write_returns_set() {
    let mut ctx = fresh_ctx();
    let outcome = ctx.try_set_jump(UpstreamJumpTarget::External(sample_external()));
    assert_eq!(outcome, SetJumpOutcome::Set);
    assert!(matches!(ctx.jump.as_deref(), Some(UpstreamJumpTarget::External(_))));
    assert!(
        ctx.selected_backend.is_none(),
        "External must NOT co-write selected_backend"
    );
}

#[test]
fn first_direct_write_keeps_backend_selection_empty() {
    // Exclude process-global initialization from the jump-storage measurement.
    drop(fresh_ctx());
    let ((ctx, outcome), allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        let mut ctx = fresh_ctx();
        let outcome = ctx.try_set_jump(UpstreamJumpTarget::Direct(sample_direct()));
        (ctx, outcome)
    });
    assert_eq!(outcome, SetJumpOutcome::Set);
    assert!(matches!(ctx.jump.as_deref(), Some(UpstreamJumpTarget::Direct(_))));
    assert!(ctx.selected_backend.is_none());
    assert_eq!(allocations.calls, 1);
    assert_eq!(allocations.requested_bytes, std::mem::size_of::<UpstreamJumpTarget>());
}

#[test]
fn second_write_returns_rejected_and_does_not_mutate_ctx() {
    let mut ctx = fresh_ctx();
    let first = ctx.try_set_jump(UpstreamJumpTarget::External(sample_external()));
    assert_eq!(first, SetJumpOutcome::Set);
    let selected_before_name = ctx.selected_backend.as_ref().map(|b| b.name.clone());

    let second = ctx.try_set_jump(UpstreamJumpTarget::Direct(sample_direct()));
    assert_eq!(second, SetJumpOutcome::Rejected);

    assert!(matches!(ctx.jump.as_deref(), Some(UpstreamJumpTarget::External(_))));
    assert_eq!(
        ctx.selected_backend.as_ref().map(|b| b.name.as_str()),
        selected_before_name.as_deref(),
        "selected_backend must not change on rejected second write"
    );
}

#[test]
fn second_direct_write_after_internal_does_not_leak_backend() {
    let mut ctx = fresh_ctx();
    ctx.try_set_jump(UpstreamJumpTarget::Internal(sample_internal()));
    assert!(ctx.selected_backend.is_none());

    let outcome = ctx.try_set_jump(UpstreamJumpTarget::Direct(sample_direct()));
    assert_eq!(outcome, SetJumpOutcome::Rejected);
    assert!(
        ctx.selected_backend.is_none(),
        "Rejected Direct must NOT touch selected_backend"
    );
}
