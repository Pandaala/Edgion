use super::*;

fn field(key: &str, from: LogExtSource, name: &str) -> AccessLogExternField {
    AccessLogExternField {
        key: key.to_string(),
        from,
        name: name.to_string(),
    }
}

#[test]
fn valid_field_has_no_error() {
    assert!(field("tenant", LogExtSource::Ctx, "tenant_id")
        .validation_error()
        .is_none());
}

#[test]
fn empty_key_or_name_is_rejected() {
    assert!(field("", LogExtSource::Header, "accept").validation_error().is_some());
    assert!(field("k", LogExtSource::Header, "").validation_error().is_some());
}

#[test]
fn over_length_key_is_rejected() {
    let long = "k".repeat(LOG_LABEL_MAX_KEY_LEN + 1);
    assert!(field(&long, LogExtSource::Header, "accept")
        .validation_error()
        .is_some());
    assert!(field("k", LogExtSource::Header, &long).validation_error().is_some());
}

#[test]
fn blocked_credential_headers_are_rejected_case_insensitively() {
    for name in ["authorization", "Authorization", "X-API-Key", "set-cookie"] {
        assert!(
            field("k", LogExtSource::Header, name).validation_error().is_some(),
            "{name} must be rejected as a header source"
        );
        assert!(
            field("k", LogExtSource::RespHeader, name).validation_error().is_some(),
            "{name} must be rejected as a respHeader source"
        );
    }
}

#[test]
fn blocked_ctx_keys_are_rejected() {
    for name in ACCESS_LOG_BLOCKED_CTX_KEYS {
        assert!(field("k", LogExtSource::Ctx, name).validation_error().is_some());
    }
}

/// Independent of `ACCESS_LOG_BLOCKED_CTX_KEYS`: unlike the header arm, `ctx`
/// matching is exact/case-sensitive by design (`is_blocked`'s `Ctx` branch uses
/// `contains`, not `eq_ignore_ascii_case`). `blocked_ctx_keys_are_rejected`
/// above iterates the same list it is checking, so it cannot catch a
/// regression that switches the `Ctx` arm to case-insensitive matching — this
/// literal assertion can.
#[test]
fn ctx_key_matching_is_case_sensitive_not_case_insensitive() {
    assert!(
        field("k", LogExtSource::Ctx, "JWT_CLAIMS").validation_error().is_none(),
        "ctx blocklist matching must be exact/case-sensitive, not case-insensitive"
    );
}

#[test]
fn reserved_annotation_prefixes_are_rejected() {
    // A tenant must not be able to project the platform's own metadata.
    assert!(field(
        "k",
        LogExtSource::RouteAnnotation,
        "kubectl.kubernetes.io/last-applied-configuration"
    )
    .validation_error()
    .is_some());
    assert!(field("k", LogExtSource::RouteAnnotation, "edgion.io/edgion-plugins")
        .validation_error()
        .is_some());
    // A plain annotation is fine.
    assert!(field("k", LogExtSource::RouteAnnotation, "owner")
        .validation_error()
        .is_none());
    // The restriction is annotation-only: a label named the same way is fine.
    assert!(field("k", LogExtSource::RouteLabel, "edgion.io/team")
        .validation_error()
        .is_none());
}

#[test]
fn blocklist_does_not_leak_across_sources() {
    // `authorization` is meaningless as a ctx key; blocking is per-source.
    assert!(field("k", LogExtSource::Ctx, "authorization")
        .validation_error()
        .is_none());
    assert!(field("k", LogExtSource::Query, "authorization")
        .validation_error()
        .is_none());
}

#[test]
fn source_deserializes_from_camel_case() {
    let f: AccessLogExternField = serde_yaml::from_str("key: ua\nfrom: respHeader\nname: content-type\n").unwrap();
    assert_eq!(f.from, LogExtSource::RespHeader);
}
