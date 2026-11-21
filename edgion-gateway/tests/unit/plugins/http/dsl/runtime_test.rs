use super::*;

#[test]
fn configured_cidrs_guard_default_client_and_replayable_redirects() {
    crate::test_blocked_cidr::install();
    let replayable = dsl_http_client_config(false, true);
    assert!(replayable.dns_resolver.is_some());
    assert!(matches!(replayable.redirect, OutboundRedirect::Custom(_)));
    let opaque = dsl_http_client_config(false, false);
    assert!(opaque.dns_resolver.is_some());
    assert!(matches!(opaque.redirect, OutboundRedirect::Deny));
}
