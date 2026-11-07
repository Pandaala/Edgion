//! Typed mTLS SAN allowlist matching for the Gateway handshake. Controller
//! admission validates entries with `allowed_san_config_error` instead.
//!
//! The handshake path only consumes the matcher prepared by
//! `AllowedSan::compile` at publication: it never validates configuration or
//! compiles a regex. An entry with `compiled == None` (invalid or never
//! prepared) fails the whole list closed.

use edgion_algo::matcher::rfc6125_hostname::matches;
use edgion_resources::resources::allowed_san::{
    AllowedSan, AllowedSanType, CompiledAllowedSan, CompiledSanMatcher, TextSanMatch,
};
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
/// An empty allowlist means "no extra SAN constraint". If any entry is not
/// prepared (config-invalid, uncompiled regex, or never compiled), the whole
/// list rejects every client, even when another entry would match.
pub fn client_sans_match_allowed(cert_sans: &[PresentedSan], allowed: &[AllowedSan]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    if allowed.iter().any(|entry| entry.compiled.is_none()) {
        return false;
    }
    cert_sans.iter().any(|cert| {
        allowed.iter().any(|entry| {
            entry
                .compiled
                .as_ref()
                .is_some_and(|compiled| cert_san_matches_entry(cert, entry.san_type, compiled))
        })
    })
}

fn cert_san_matches_entry(presented: &PresentedSan, san_type: AllowedSanType, compiled: &CompiledAllowedSan) -> bool {
    if presented.san_type != san_type {
        return false;
    }
    if san_type == AllowedSanType::OtherName {
        let Some(entry_oid) = compiled.oid.as_deref() else {
            return false;
        };
        if presented.oid.as_deref() != Some(entry_oid) {
            return false;
        }
    }
    match_presented_value(&presented.value, &compiled.matcher)
}

fn match_presented_value(presented: &str, matcher: &CompiledSanMatcher) -> bool {
    match matcher {
        CompiledSanMatcher::DnsExact(allowed) => dns_exact_rfc6125(presented, allowed),
        CompiledSanMatcher::IpExact(allowed) => ip_exact(presented, allowed),
        CompiledSanMatcher::Text {
            kind,
            value,
            ignore_case,
        } => match kind {
            TextSanMatch::Exact => string_eq(presented, value, *ignore_case),
            TextSanMatch::Prefix => string_prefix(presented, value, *ignore_case),
            TextSanMatch::Suffix => string_suffix(presented, value, *ignore_case),
            TextSanMatch::Contains => string_contains(presented, value, *ignore_case),
        },
        CompiledSanMatcher::Regex(regex) => regex.is_match(presented),
    }
}

/// RFC 6125 §6.4.3 wildcards on either the allowlist or the presented DNS SAN.
fn dns_exact_rfc6125(cert_dns: &str, allowed: &str) -> bool {
    matches(allowed, cert_dns) || matches(cert_dns, allowed)
}

fn ip_exact(cert_ip: &str, allowed: &IpAddr) -> bool {
    cert_ip.parse::<IpAddr>().is_ok_and(|presented| presented == *allowed)
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
