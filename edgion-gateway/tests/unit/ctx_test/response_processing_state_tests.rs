use super::*;

#[test]
#[cfg(target_pointer_width = "64")]
fn response_processing_state_has_one_scenario_box() {
    assert_eq!(std::mem::size_of::<ResponseProcessingState>(), 120);
    assert_eq!(std::mem::size_of::<Option<Box<ResponseProcessingState>>>(), 8);
}

#[test]
fn freezing_an_empty_processor_set_does_not_allocate() {
    let mut ctx = EdgionHttpContext::new();
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        ctx.freeze_response_processor_driver();
    });

    assert!(ctx.response_processors_frozen());
    assert!(ctx.response_processor_driver().is_none());
    assert!(!ctx.has_response_processing());
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}

#[test]
fn first_framing_reservation_allocates_once_and_reuses_state() {
    let mut ctx = EdgionHttpContext::new();
    let (_, first) = crate::test_allocation_counter::count_allocation_stats(|| {
        ctx.reserve_response_framing();
    });
    assert!(ctx.has_response_processing());
    assert!(ctx.has_response_framing_reservation());
    assert_eq!(first.calls, 1);
    assert_eq!(first.requested_bytes, std::mem::size_of::<ResponseProcessingState>());

    let (_, later) = crate::test_allocation_counter::count_allocation_stats(|| {
        ctx.reserve_response_framing();
    });
    assert_eq!(later.calls, 0);
    assert!(ctx.has_response_framing_reservation());
}

#[test]
fn absent_response_processing_terminal_hooks_do_not_allocate() {
    let mut ctx = EdgionHttpContext::new();
    let (_, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        assert!(!ctx.claim_upstream_response_trailers(false));
        assert!(!ctx.mark_upstream_before_trailers());
        assert!(!ctx.claim_upstream_trailer_cleanup());
    });

    assert!(!ctx.has_response_processing());
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}
