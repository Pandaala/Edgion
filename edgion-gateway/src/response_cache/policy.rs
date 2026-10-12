//! Narrow shared-response contract and monotonic freshness accounting.

use std::time::{Duration, Instant, SystemTime};

use crate::routes::http::proxy_http::content_type::{is_sse_content_type, response_is_identity_encoded};
use http::HeaderMap;
use pingora_http::ResponseHeader;

#[derive(Clone, Copy)]
pub(crate) struct GraphqlProof {
    pub generation: u64,
    pub operation: edgion_resources::graphql_request::GraphqlOperationType,
    pub name_digest: [u8; 32],
}

pub(super) const MAX_FIELDS: usize = 64;
pub(super) const MAX_METADATA: usize = 16 * 1024;
pub(super) const MAX_BODY: usize = 1024 * 1024;

/// Optional cache admission never implements HTTP range or validator semantics.
/// Presence alone bypasses; ordinary origin processing remains their owner.
pub(super) fn request_is_eligible(headers: &HeaderMap) -> bool {
    ![
        "authorization",
        "cookie",
        "upgrade",
        "cache-control",
        "pragma",
        "range",
        "if-range",
        "if-match",
        "if-none-match",
        "if-modified-since",
        "if-unmodified-since",
    ]
    .iter()
    .any(|name| headers.contains_key(*name))
}

#[derive(Clone, Copy)]
pub(super) struct Freshness {
    pub header_at: Instant,
    pub initial_age: Duration,
    pub lifetime: Duration,
}

fn singleton<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a [u8]> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    values.next().is_none().then_some(value.as_bytes())
}

fn decimal(value: &[u8]) -> Option<u64> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return None;
    }
    value
        .iter()
        .try_fold(0u64, |n, digit| n.checked_mul(10)?.checked_add(u64::from(digit - b'0')))
}

fn ows(value: &[u8]) -> &[u8] {
    value.trim_ascii_start().trim_ascii_end()
}

/// Recognize only the reviewed two-directive contract, never a general HTTP parser.
pub(super) fn public_max_age(value: &[u8]) -> Option<Duration> {
    if value.iter().any(|b| matches!(b, b'\r' | b'\n' | b'\x0b' | b'\x0c')) {
        return None;
    }
    let mut directives = value.split(|b| *b == b',');
    let first = ows(directives.next()?);
    let second = ows(directives.next()?);
    if directives.next().is_some() {
        return None;
    }
    let age = if first.eq_ignore_ascii_case(b"public") {
        second
    } else if second.eq_ignore_ascii_case(b"public") {
        first
    } else {
        return None;
    };
    let position = age.iter().position(|b| *b == b'=')?;
    if !ows(&age[..position]).eq_ignore_ascii_case(b"max-age") {
        return None;
    }
    let seconds = decimal(ows(&age[position + 1..]))?;
    (seconds != 0).then(|| Duration::from_secs(seconds))
}

pub(super) fn supported_headers(response: &ResponseHeader) -> bool {
    response.status.as_u16() == 200
        && response.headers.len() <= MAX_FIELDS
        && [
            "set-cookie",
            "www-authenticate",
            "proxy-authenticate",
            "pragma",
            "expires",
            "trailer",
        ]
        .iter()
        .all(|name| !response.headers.contains_key(*name))
        && response
            .headers
            .get("content-type")
            .is_none_or(|value| value.to_str().is_ok_and(|value| !is_sse_content_type(value)))
        && response_is_identity_encoded(&response.headers)
        && super::key::VaryCertificate::parse(&response.headers).is_some()
        && singleton(&response.headers, "cache-control")
            .and_then(public_max_age)
            .is_some()
}

impl Freshness {
    pub(super) fn from_headers(
        response: &ResponseHeader,
        request_at: Instant,
        header_at: Instant,
        received_wall: SystemTime,
        max_ttl: Duration,
    ) -> Option<Self> {
        if !supported_headers(response) {
            return None;
        }
        let lifetime = public_max_age(singleton(&response.headers, "cache-control")?)?.min(max_ttl);
        let date = httpdate::parse_http_date(std::str::from_utf8(singleton(&response.headers, "date")?).ok()?).ok()?;
        let age = if response.headers.contains_key("age") {
            decimal(singleton(&response.headers, "age")?)?
        } else {
            0
        };
        let apparent_age = received_wall.duration_since(date).unwrap_or(Duration::ZERO);
        let corrected_age = Duration::from_secs(age).checked_add(header_at.saturating_duration_since(request_at))?;
        Some(Self {
            header_at,
            initial_age: apparent_age.max(corrected_age),
            lifetime,
        })
    }

    pub(super) fn age(&self, now: Instant) -> Option<Duration> {
        self.initial_age
            .checked_add(now.saturating_duration_since(self.header_at))
    }

    pub(super) fn remaining(&self, now: Instant) -> Option<Duration> {
        let remaining = self.lifetime.checked_sub(self.age(now)?)?;
        (!remaining.is_zero()).then_some(remaining)
    }
}

#[cfg(test)]
#[path = "../../tests/unit/response_cache/policy_test.rs"]
mod tests;
