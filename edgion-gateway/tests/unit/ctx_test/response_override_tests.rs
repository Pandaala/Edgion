use super::*;

#[test]
fn lazy_holder_is_pointer_sized_and_defaults_inactive() {
    let former_inline_fields = std::mem::size_of::<Option<bytes::Bytes>>()
        + std::mem::size_of::<Option<Box<ResponseHeader>>>()
        + std::mem::size_of::<bool>()
        + std::mem::size_of::<Option<ResponseStageDenial>>();
    assert_eq!(
        std::mem::size_of::<Option<Box<ResponseOverride>>>(),
        std::mem::size_of::<usize>()
    );
    assert!(std::mem::size_of::<Option<Box<ResponseOverride>>>() < former_inline_fields);
    let ctx = EdgionHttpContext::default();
    assert!(!ctx.response_stage_settled());
    assert!(!ctx.local_reply_written());
    assert_eq!(ctx.response_denial(), None);
}

#[test]
fn terminal_states_are_mutually_exclusive() {
    let mut ctx = EdgionHttpContext::default();
    let header = ResponseHeader::build(403, None).unwrap();
    ctx.stage_response_local_reply(Box::new(header), bytes::Bytes::from_static(b"denied"));
    assert!(ctx.response_stage_settled());
    assert_eq!(ctx.response_denial(), None);

    ctx.set_response_denial(ResponseStageDenial::Http(502));
    assert!(ctx.take_pending_response_local_reply().is_none());
    assert_eq!(ctx.response_denial(), Some(&ResponseStageDenial::Http(502)));

    ctx.mark_local_reply_written();
    assert!(ctx.local_reply_written());
    assert!(!ctx.response_stage_settled());
    assert_eq!(ctx.response_denial(), None);
}

#[test]
fn pending_reply_round_trips_and_clears_state() {
    let mut ctx = EdgionHttpContext::default();
    let header = ResponseHeader::build(403, None).unwrap();
    ctx.stage_response_local_reply(Box::new(header), bytes::Bytes::from_static(b"denied"));

    let (header, body) = ctx.take_pending_response_local_reply().unwrap();
    assert_eq!(header.status.as_u16(), 403);
    assert_eq!(body, bytes::Bytes::from_static(b"denied"));
    assert!(!ctx.response_stage_settled());
}

#[test]
fn denial_round_trips_and_clears_state() {
    let mut ctx = EdgionHttpContext::default();
    let denial = ResponseStageDenial::Grpc {
        status: 8,
        message: Some("quota exceeded".to_string()),
    };
    ctx.set_response_denial(denial.clone());

    assert_eq!(ctx.take_response_denial(), Some(denial));
    assert_eq!(ctx.response_denial(), None);
    assert!(!ctx.response_stage_settled());
}
