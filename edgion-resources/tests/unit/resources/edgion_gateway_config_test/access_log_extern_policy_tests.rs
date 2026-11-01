use super::*;
use crate::resources::edgion_plugins::LogExtSource;

#[test]
fn default_whitelist_allows_safe_headers_only() {
    let w = UnmaskedKeys::default();
    assert!(w.allows(LogExtSource::Header, "user-agent"));
    assert!(w.allows(LogExtSource::Header, "content-type"));
    // Header matching is case-insensitive (HTTP semantics).
    assert!(w.allows(LogExtSource::Header, "User-Agent"));

    // `referer` leaks someone else's full URL (session ids in the query);
    // `location` carries OAuth implicit tokens and reset links. Neither is a
    // default. Both remain addable by an operator.
    assert!(!w.allows(LogExtSource::Header, "referer"));
    assert!(!w.allows(LogExtSource::RespHeader, "location"));
    // Weak PII / noise.
    assert!(!w.allows(LogExtSource::Header, "accept-language"));
    assert!(!w.allows(LogExtSource::Header, "accept-encoding"));
}

#[test]
fn default_whitelist_is_empty_for_business_defined_sources() {
    // No key name is universally safe for these — operators opt in per cluster.
    let w = UnmaskedKeys::default();
    assert!(!w.allows(LogExtSource::Query, "page"));
    assert!(!w.allows(LogExtSource::Cookie, "session"));
    assert!(!w.allows(LogExtSource::Ctx, "tenant_id"));
}

#[test]
fn category_one_sources_are_never_consulted() {
    // routeLabel/routeAnnotation bypass the whitelist entirely upstream of
    // this call; `allows` must still answer false so a caller that wrongly
    // consults it cannot accidentally widen anything.
    let w = UnmaskedKeys::default();
    assert!(!w.allows(LogExtSource::RouteLabel, "app"));
    assert!(!w.allows(LogExtSource::RouteAnnotation, "owner"));
}

#[test]
fn cookie_and_ctx_names_are_case_sensitive() {
    let w = UnmaskedKeys {
        cookie: vec!["Session".to_string()],
        ctx: vec!["tenant_id".to_string()],
        ..Default::default()
    };
    assert!(w.allows(LogExtSource::Cookie, "Session"));
    assert!(!w.allows(LogExtSource::Cookie, "session"));
    assert!(w.allows(LogExtSource::Ctx, "tenant_id"));
    assert!(!w.allows(LogExtSource::Ctx, "TENANT_ID"));
}

#[test]
fn operator_overrides_only_the_named_list() {
    let policy: AccessLogExternPolicy = yaml_serde::from_str("unmaskedKeys:\n  cookie: [tier]\n").unwrap();
    // The named list is replaced...
    assert!(policy.unmasked_keys.allows(LogExtSource::Cookie, "tier"));
    // ...and the unnamed ones keep their defaults.
    assert!(policy.unmasked_keys.allows(LogExtSource::Header, "user-agent"));
}
