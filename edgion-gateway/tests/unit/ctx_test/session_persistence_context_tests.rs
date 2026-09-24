use super::*;

#[test]
fn ordinary_context_keeps_session_persistence_unallocated() {
    let ctx = EdgionHttpContext::new();
    assert!(ctx.session_persistence().is_none());
}

#[test]
fn first_session_persistence_use_allocates_default_state() {
    let mut ctx = EdgionHttpContext::new();
    let persistence = ctx.ensure_session_persistence();

    assert_eq!(persistence.state, SessionPersistenceState::Inactive);
    assert!(persistence.target_addr.is_none());
    assert!(persistence.sticky_addr.is_none());
    assert!(persistence.token_scope.is_none());
    assert!(!persistence.sticky_consumed);
}
