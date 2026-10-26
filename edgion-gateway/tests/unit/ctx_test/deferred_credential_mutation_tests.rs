use super::DeferredCredentialMutations;
use http::{HeaderName, HeaderValue};
use std::sync::Arc;

fn value(raw: &'static str) -> Arc<HeaderValue> {
    Arc::new(HeaderValue::from_static(raw))
}

#[test]
fn later_value_replaces_same_target_and_removal_is_case_insensitive() {
    let mut mutations = DeferredCredentialMutations::default();
    mutations.upsert(HeaderName::from_static("authorization"), value("Bearer first"), None);
    mutations.upsert(HeaderName::from_static("authorization"), value("Bearer second"), None);

    let mut request = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
    mutations.apply(&mut request).unwrap();
    assert_eq!(request.headers.get("authorization").unwrap(), "Bearer second");

    mutations.remove("Authorization");
    let mut request = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
    mutations.apply(&mut request).unwrap();
    assert!(request.headers.get("authorization").is_none());
}

#[test]
fn applying_to_each_attempt_is_non_destructive_and_debug_is_redacted() {
    let secret = "private-upstream-token";
    let mut mutations = DeferredCredentialMutations::default();
    let mut value = HeaderValue::from_str(secret).unwrap();
    value.set_sensitive(true);
    mutations.upsert(HeaderName::from_static("x-provider-token"), Arc::new(value), None);

    for _ in 0..2 {
        let mut request = pingora_http::RequestHeader::build("GET", b"/", None).unwrap();
        mutations.apply(&mut request).unwrap();
        let value = request.headers.get("x-provider-token").unwrap();
        assert_eq!(value, secret);
        assert!(value.is_sensitive());
        assert_eq!(format!("{value:?}"), "Sensitive");
        let request_debug = format!("{request:?}");
        assert!(request_debug.contains("Sensitive"));
        assert!(!request_debug.contains(secret));
    }

    let debug = format!("{mutations:?}");
    assert!(debug.contains("x-provider-token"));
    assert!(!debug.contains(secret));
}

#[test]
fn preflight_rejects_any_expired_deferred_credential() {
    let now = std::time::Instant::now();
    let mut mutations = DeferredCredentialMutations::default();
    mutations.upsert(
        HeaderName::from_static("authorization"),
        value("Bearer current"),
        Some(now + std::time::Duration::from_secs(30)),
    );
    assert!(mutations.all_unexpired(now));

    mutations.upsert(
        HeaderName::from_static("x-provider-token"),
        value("expired"),
        Some(now - std::time::Duration::from_secs(1)),
    );
    assert!(!mutations.all_unexpired(now));
}

#[test]
fn borrowed_value_lookup_preserves_identity_sensitivity_and_winning_upsert() {
    let mut mutations = DeferredCredentialMutations::default();
    let mut raw = HeaderValue::from_bytes(b"\xffprivate").unwrap();
    raw.set_sensitive(true);
    let original = Arc::new(raw);
    mutations.upsert(HeaderName::from_static("x-provider-token"), original.clone(), None);
    let borrowed = mutations.value_for("X-Provider-Token").unwrap();
    assert!(std::ptr::eq(borrowed, original.as_ref()));
    assert!(borrowed.is_sensitive());
    assert_eq!(borrowed.as_bytes(), b"\xffprivate");
    assert!(mutations.value_for("missing").is_none());

    let replacement = value("replacement");
    mutations.upsert(
        HeaderName::from_bytes(b"X-Provider-Token").unwrap(),
        replacement.clone(),
        None,
    );
    assert!(std::ptr::eq(
        mutations.value_for("x-provider-token").unwrap(),
        replacement.as_ref()
    ));
    mutations.remove("X-Provider-Token");
    assert!(mutations.value_for("x-provider-token").is_none());
    assert!(original.is_sensitive());
}
