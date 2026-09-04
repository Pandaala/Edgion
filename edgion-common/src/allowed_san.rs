//! Typed mTLS SAN allowlist matching shared by Controller validation and the
//! Gateway handshake.
//!
//! The handshake path must never compile a regex. `RegularExpression` entries
//! with `compiled_regex == None` fail closed.

use edgion_resources::resources::allowed_san::{allowed_san_schema_error, AllowedSan, AllowedSanMatch, AllowedSanType};
use std::net::IpAddr;

use crate::tls_validation::matches_host_pattern;

/// A certificate SAN classified by ASN.1 GeneralName type, not by string prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedSan {
    pub san_type: AllowedSanType,
    pub oid: Option<String>,
    pub value: String,
}

impl PresentedSan {
    pub fn dns(value: impl Into<String>) -> Self {
        Self {
            san_type: AllowedSanType::Dns,
            oid: None,
            value: value.into(),
        }
    }

    pub fn uri(value: impl Into<String>) -> Self {
        Self {
            san_type: AllowedSanType::Uri,
            oid: None,
            value: value.into(),
        }
    }

    pub fn email(value: impl Into<String>) -> Self {
        Self {
            san_type: AllowedSanType::Email,
            oid: None,
            value: value.into(),
        }
    }

    pub fn ip(value: impl Into<String>) -> Self {
        Self {
            san_type: AllowedSanType::Ip,
            oid: None,
            value: value.into(),
        }
    }

    pub fn other_name(oid: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            san_type: AllowedSanType::OtherName,
            oid: Some(oid.into()),
            value: value.into(),
        }
    }
}

/// Display encoding used by `ClientCertInfo.sans` and backend SAN strings.
/// DNS names are unprefixed; every other ASN.1 type carries one of these prefixes.
pub fn has_typed_san_display_prefix(s: &str) -> bool {
    s.starts_with("uri:") || s.starts_with("email:") || s.starts_with("IP:") || s.starts_with("otherName:")
}

/// True when the certificate SAN list satisfies the typed allowlist.
///
/// An empty allowlist means "no extra SAN constraint". A config-invalid or
/// uncompiled RegularExpression list rejects every client.
pub fn client_sans_match_allowed(cert_sans: &[PresentedSan], allowed: &[AllowedSan]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    if allowed.iter().any(allowlist_entry_unusable) {
        return false;
    }
    cert_sans
        .iter()
        .any(|cert| allowed.iter().any(|entry| cert_san_matches_entry(cert, entry)))
}

fn allowlist_entry_unusable(entry: &AllowedSan) -> bool {
    if allowed_san_schema_error(entry).is_some() {
        return true;
    }
    entry.match_kind == AllowedSanMatch::RegularExpression && entry.compiled_regex.is_none()
}

fn cert_san_matches_entry(presented: &PresentedSan, entry: &AllowedSan) -> bool {
    if presented.san_type != entry.san_type {
        return false;
    }
    if entry.san_type == AllowedSanType::OtherName {
        let Some(entry_oid) = entry.oid.as_deref().map(str::trim) else {
            return false;
        };
        if presented.oid.as_deref() != Some(entry_oid) {
            return false;
        }
    }
    match_presented_value(&presented.value, entry)
}

fn match_presented_value(presented: &str, entry: &AllowedSan) -> bool {
    let allowed = entry.value.trim();
    match (entry.san_type, entry.match_kind) {
        (AllowedSanType::Dns, AllowedSanMatch::Exact) => dns_exact_rfc6125(presented, allowed),
        (AllowedSanType::Ip, AllowedSanMatch::Exact) => ip_exact(presented, allowed),
        (_, AllowedSanMatch::Exact) => string_eq(presented, allowed, entry.ignore_case),
        (_, AllowedSanMatch::Prefix) => string_prefix(presented, allowed, entry.ignore_case),
        (_, AllowedSanMatch::Suffix) => string_suffix(presented, allowed, entry.ignore_case),
        (_, AllowedSanMatch::Contains) => string_contains(presented, allowed, entry.ignore_case),
        (_, AllowedSanMatch::RegularExpression) => entry
            .compiled_regex
            .as_ref()
            .is_some_and(|regex| regex.is_match(presented)),
    }
}

/// RFC 6125 §6.4.3 wildcards on either the allowlist or the presented DNS SAN.
fn dns_exact_rfc6125(cert_dns: &str, allowed: &str) -> bool {
    matches_host_pattern(cert_dns, allowed) || matches_host_pattern(allowed, cert_dns)
}

fn ip_exact(cert_ip: &str, allowed: &str) -> bool {
    match (cert_ip.parse::<IpAddr>(), allowed.parse::<IpAddr>()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn string_eq(presented: &str, allowed: &str, ignore_case: bool) -> bool {
    if ignore_case {
        presented.eq_ignore_ascii_case(allowed)
    } else {
        presented == allowed
    }
}

fn string_prefix(presented: &str, allowed: &str, ignore_case: bool) -> bool {
    if ignore_case {
        presented
            .get(..allowed.len())
            .is_some_and(|slice| slice.eq_ignore_ascii_case(allowed))
    } else {
        presented.starts_with(allowed)
    }
}

fn string_suffix(presented: &str, allowed: &str, ignore_case: bool) -> bool {
    if ignore_case {
        presented
            .len()
            .checked_sub(allowed.len())
            .and_then(|start| presented.get(start..))
            .is_some_and(|slice| slice.eq_ignore_ascii_case(allowed))
    } else {
        presented.ends_with(allowed)
    }
}

fn string_contains(presented: &str, allowed: &str, ignore_case: bool) -> bool {
    if ignore_case {
        presented.to_ascii_lowercase().contains(&allowed.to_ascii_lowercase())
    } else {
        presented.contains(allowed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dns(value: &str) -> AllowedSan {
        AllowedSan::exact(AllowedSanType::Dns, value)
    }

    fn compile_regex(san_type: AllowedSanType, pattern: &str) -> AllowedSan {
        let mut entry = AllowedSan::exact(san_type, pattern);
        entry.match_kind = AllowedSanMatch::RegularExpression;
        entry.compile().expect("test regex must compile");
        entry
    }

    #[test]
    fn empty_allowlist_is_unconstrained() {
        assert!(client_sans_match_allowed(
            &[PresentedSan::dns("client.example.com")],
            &[]
        ));
    }

    #[test]
    fn dns_exact_and_case_fold() {
        let allowed = [dns("client.example.com")];
        assert!(client_sans_match_allowed(
            &[PresentedSan::dns("client.example.com")],
            &allowed
        ));
        assert!(client_sans_match_allowed(
            &[PresentedSan::dns("CLIENT.EXAMPLE.COM")],
            &allowed
        ));
        assert!(!client_sans_match_allowed(
            &[PresentedSan::dns("other.example.com")],
            &allowed
        ));
    }

    #[test]
    fn dns_rfc6125_both_directions() {
        let allowlist_wild = [dns("*.internal.example.com")];
        assert!(client_sans_match_allowed(
            &[PresentedSan::dns("a.internal.example.com")],
            &allowlist_wild
        ));
        assert!(!client_sans_match_allowed(
            &[PresentedSan::dns("a.b.internal.example.com")],
            &allowlist_wild
        ));

        let allowlist_exact = [dns("api.example.com")];
        assert!(client_sans_match_allowed(
            &[PresentedSan::dns("*.example.com")],
            &allowlist_exact
        ));
        assert!(!client_sans_match_allowed(
            &[PresentedSan::dns("*.internal.example.com")],
            &allowlist_exact
        ));
    }

    #[test]
    fn cross_type_dns_wildcard_does_not_match_uri() {
        let allowed = [dns("*.internal.example.com")];
        assert!(!client_sans_match_allowed(
            &[PresentedSan::uri("https://a.internal.example.com")],
            &allowed
        ));
    }

    #[test]
    fn dns_value_that_looks_like_uri_does_not_match_uri_allowlist() {
        let allowed = [AllowedSan::exact(AllowedSanType::Uri, "https://a.internal.example.com")];
        assert!(!client_sans_match_allowed(
            &[PresentedSan::dns("https://a.internal.example.com")],
            &allowed
        ));
    }

    #[test]
    fn uri_exact_is_case_sensitive() {
        let allowed = [AllowedSan::exact(
            AllowedSanType::Uri,
            "spiffe://cluster.local/ns/foo/sa/bar",
        )];
        assert!(client_sans_match_allowed(
            &[PresentedSan::uri("spiffe://cluster.local/ns/foo/sa/bar")],
            &allowed
        ));
        assert!(!client_sans_match_allowed(
            &[PresentedSan::uri("SPIFFE://cluster.local/ns/foo/sa/bar")],
            &allowed
        ));
    }

    #[test]
    fn uri_prefix() {
        let mut entry = AllowedSan::exact(AllowedSanType::Uri, "spiffe://cluster.local/");
        entry.match_kind = AllowedSanMatch::Prefix;
        assert!(client_sans_match_allowed(
            &[PresentedSan::uri("spiffe://cluster.local/ns/foo/sa/bar")],
            std::slice::from_ref(&entry)
        ));
        assert!(!client_sans_match_allowed(
            &[PresentedSan::uri("spiffe://other.local/ns/foo/sa/bar")],
            std::slice::from_ref(&entry)
        ));
    }

    #[test]
    fn email_ignore_case_and_suffix() {
        let mut exact = AllowedSan::exact(AllowedSanType::Email, "alice@example.com");
        assert!(!client_sans_match_allowed(
            &[PresentedSan::email("Alice@example.com")],
            std::slice::from_ref(&exact)
        ));
        exact.ignore_case = true;
        assert!(client_sans_match_allowed(
            &[PresentedSan::email("Alice@example.com")],
            std::slice::from_ref(&exact)
        ));

        let mut suffix = AllowedSan::exact(AllowedSanType::Email, "@example.com");
        suffix.match_kind = AllowedSanMatch::Suffix;
        assert!(client_sans_match_allowed(
            &[PresentedSan::email("bob@example.com")],
            std::slice::from_ref(&suffix)
        ));
    }

    #[test]
    fn ip_exact_canonical_and_mapped() {
        let v6 = [AllowedSan::exact(AllowedSanType::Ip, "2001:0db8::1")];
        assert!(client_sans_match_allowed(&[PresentedSan::ip("2001:db8::1")], &v6));
        let mapped = [AllowedSan::exact(AllowedSanType::Ip, "::ffff:127.0.0.1")];
        assert!(!client_sans_match_allowed(&[PresentedSan::ip("127.0.0.1")], &mapped));
        assert!(!client_sans_match_allowed(
            &[PresentedSan::ip("10.0.0.1")],
            &[dns("10.0.0.1")]
        ));
    }

    #[test]
    fn regex_match_requires_precompile() {
        let compiled = compile_regex(AllowedSanType::Dns, r"^client[0-9]+\.example\.com$");
        assert!(client_sans_match_allowed(
            &[PresentedSan::dns("client7.example.com")],
            std::slice::from_ref(&compiled)
        ));
        assert!(!client_sans_match_allowed(
            &[PresentedSan::dns("other.example.com")],
            std::slice::from_ref(&compiled)
        ));

        let mut uncompiled = AllowedSan::exact(AllowedSanType::Dns, r"^client[0-9]+\.example\.com$");
        uncompiled.match_kind = AllowedSanMatch::RegularExpression;
        assert!(
            !client_sans_match_allowed(
                &[PresentedSan::dns("client7.example.com")],
                std::slice::from_ref(&uncompiled)
            ),
            "uncompiled regex must fail closed"
        );
    }

    #[test]
    fn other_name_matches_oid_and_value() {
        let allowed = [AllowedSan::other_name(
            "1.3.6.1.4.1.311.20.2.3",
            "alice@corp.example.com",
        )];
        assert!(client_sans_match_allowed(
            &[PresentedSan::other_name(
                "1.3.6.1.4.1.311.20.2.3",
                "alice@corp.example.com"
            )],
            &allowed
        ));
        assert!(!client_sans_match_allowed(
            &[PresentedSan::other_name("1.2.3", "alice@corp.example.com")],
            &allowed
        ));
        assert!(!client_sans_match_allowed(
            &[PresentedSan::other_name(
                "1.3.6.1.4.1.311.20.2.3",
                "bob@corp.example.com"
            )],
            &allowed
        ));
    }

    #[test]
    fn invalid_entry_fails_closed() {
        let bad = [AllowedSan::exact(AllowedSanType::Ip, "not-an-ip")];
        assert!(!client_sans_match_allowed(&[PresentedSan::ip("127.0.0.1")], &bad));
    }

    #[test]
    fn typed_display_prefix_covers_other_name() {
        assert!(!has_typed_san_display_prefix("client.example.com"));
        assert!(has_typed_san_display_prefix("IP:127.0.0.1"));
        assert!(has_typed_san_display_prefix("uri:spiffe://cluster.local/ns/foo"));
        assert!(has_typed_san_display_prefix("email:alice@example.com"));
        assert!(has_typed_san_display_prefix(
            "otherName:1.3.6.1.4.1.311.20.2.3=alice@corp.example.com"
        ));
    }
}
