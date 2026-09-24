use super::*;

#[test]
fn finalization_prefers_written_final_header_then_selected_status_then_zero() {
    let mut written = EdgionHttpContext::default();
    assert!(written.select_final_downstream_status(502));
    written.finalize_downstream_status(Some(200));
    assert_eq!(written.request_info.status(), 200);

    let mut selected = EdgionHttpContext::default();
    assert!(selected.claim_final_response(503));
    selected.finalize_downstream_status(None);
    assert_eq!(selected.request_info.status(), 503);

    let mut unknown = EdgionHttpContext::default();
    unknown.finalize_downstream_status(None);
    assert_eq!(unknown.request_info.status(), 0);
    assert_eq!(unknown.request_info.status_option(), None);
}

#[test]
fn interim_headers_do_not_replace_selected_status_but_101_is_terminal() {
    let mut interim = EdgionHttpContext::default();
    assert!(interim.select_final_downstream_status(502));
    interim.finalize_downstream_status(Some(103));
    assert_eq!(interim.request_info.status(), 502);

    let mut switching = EdgionHttpContext::default();
    assert!(switching.select_final_downstream_status(502));
    switching.finalize_downstream_status(Some(101));
    assert_eq!(switching.request_info.status(), 101);
}

#[test]
fn candidates_and_claims_reject_informational_or_invalid_statuses() {
    for status in [100, 101, 102, 103, 199, 600, 700] {
        let mut ctx = EdgionHttpContext::default();
        assert!(!ctx.select_final_downstream_status(status), "candidate {status}");
        assert!(!ctx.claim_final_response(status), "claim {status}");
        assert!(!ctx.select_terminal_downstream_status(status), "terminal {status}");
        assert!(!ctx.response_write_started());
        assert_eq!(ctx.final_downstream_status(), None);
    }
}

#[test]
fn terminal_finalization_filters_an_invalid_internal_candidate() {
    let mut ctx = EdgionHttpContext {
        final_downstream_status: Some(103),
        ..Default::default()
    };
    ctx.finalize_downstream_status(None);
    assert_eq!(ctx.request_info.status_option(), None);
}

#[test]
fn write_claim_rejects_late_replacement_and_terminal_499() {
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.claim_final_response(200));
    assert!(!ctx.select_final_downstream_status(502));
    assert!(!ctx.select_terminal_downstream_status(499));
    assert!(!ctx.claim_final_response(503));
    ctx.finalize_downstream_status(None);
    assert_eq!(ctx.request_info.status(), 200);
}

#[test]
fn claimed_error_status_survives_delivery_failure() {
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.select_final_downstream_status(200));
    assert!(ctx.claim_final_response(502));
    assert!(!ctx.select_terminal_downstream_status(499));
    assert!(!ctx.select_final_downstream_status(504));

    ctx.finalize_downstream_status(None);
    assert_eq!(ctx.request_info.status(), 502);
}

#[test]
fn selected_terminal_status_blocks_later_candidate_or_writer() {
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.select_terminal_downstream_status(499));
    assert!(!ctx.select_final_downstream_status(502));
    assert!(!ctx.claim_final_response(502));
    assert_eq!(ctx.final_downstream_status(), Some(499));
}

#[test]
fn grpc_error_keeps_http_and_grpc_status_independent() {
    let mut ctx = EdgionHttpContext::default();
    assert!(ctx.select_final_downstream_status(504));
    assert!(ctx.claim_final_response(200));
    ctx.request_info.set_grpc_status(4);

    ctx.finalize_downstream_status(None);

    assert_eq!(ctx.request_info.status(), 200);
    assert_eq!(ctx.request_info.grpc_status(), Some(4));
}
