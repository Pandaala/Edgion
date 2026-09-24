//! Typed mTLS SAN allowlist matching shared by Controller validation and the
//! Gateway handshake.
//!
//! The handshake path must never compile a regex. `RegularExpression` entries
//! with `compiled_regex == None` fail closed.

use edgion_algo::matcher::rfc6125_hostname::matches;
use edgion_resources::resources::allowed_san::{allowed_san_schema_error, AllowedSan, AllowedSanMatch, AllowedSanType};
use std::net::IpAddr;

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
    matches(allowed, cert_dns) || matches(cert_dns, allowed)
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
#[path = "../tests/unit/allowed_san_test.rs"]
mod tests;
