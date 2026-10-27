//! Canonical current-request Cookie reads and synchronous removal planning.
//!
//! Headers remain authoritative. Reading never rewrites them, and a removal
//! without a match never allocates an output or normalizes existing fields.

use http::{header::COOKIE, HeaderMap, HeaderValue};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CookieEditOutcome {
    Absent,
    NoChange,
    Removed,
    Replaced,
}

/// Fixed classifications only: errors never retain credential bytes or names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CookieEditError {
    Unsanitizable,
    Internal,
}

impl std::fmt::Display for CookieEditError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unsanitizable => "request cookie cannot be sanitized",
            Self::Internal => "request cookie update failed",
        })
    }
}

impl std::error::Error for CookieEditError {}

#[derive(Clone, Copy)]
pub(crate) enum FieldScope {
    All,
    First,
}

pub(crate) struct Pair<'a> {
    pub(crate) raw: &'a str,
    pub(crate) name: &'a str,
    pub(crate) value: Option<&'a str>,
}

/// Preserve the established grammar, including raw names and quoted values.
pub(crate) fn pairs(text: &str) -> impl Iterator<Item = Pair<'_>> {
    text.split(';').filter_map(|segment| {
        let raw = segment.trim();
        if raw.is_empty() {
            return None;
        }
        let (name, value) = match raw.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (raw, None),
        };
        Some(Pair { raw, name, value })
    })
}

pub(crate) fn lookup<'a>(headers: &'a HeaderMap, name: &str, scope: FieldScope) -> Option<&'a str> {
    let count = match scope {
        FieldScope::All => usize::MAX,
        FieldScope::First => 1,
    };
    for field in headers.get_all(COOKIE).iter().take(count) {
        // Lookup's HeaderValue eligibility intentionally differs from editing's
        // UTF-8 eligibility. An unreadable field before a match stops lookup.
        let text = field.to_str().ok()?;
        for pair in pairs(text) {
            if pair.name == name {
                if let Some(value) = pair.value {
                    return Some(value);
                }
            }
        }
    }
    None
}

#[derive(Debug)]
pub(crate) enum CookieEditPlan {
    Absent,
    NoChange,
    Remove,
    Replace(HeaderValue),
}

/// Single-pass borrowed survivors preserve the measured baseline's scan cost.
/// No-match may allocate metadata, but never allocates output or rewrites headers.
/// The caller publishes only after this complete plan has succeeded.
pub(crate) fn plan_removal(headers: &HeaderMap, names: &[&str]) -> Result<CookieEditPlan, CookieEditError> {
    if names.is_empty() {
        return Ok(CookieEditPlan::NoChange);
    }
    let mut saw_field = false;
    let mut matched = false;
    let mut output_len = 0usize;
    let mut survivors = Vec::new();
    for field in headers.get_all(COOKIE) {
        saw_field = true;
        let text = std::str::from_utf8(field.as_bytes()).map_err(|_| CookieEditError::Unsanitizable)?;
        for pair in pairs(text) {
            if names.contains(&pair.name) {
                matched = true;
            } else {
                output_len = output_len
                    .checked_add(pair.raw.len())
                    .and_then(|length| length.checked_add(if survivors.is_empty() { 0 } else { 2 }))
                    .ok_or(CookieEditError::Internal)?;
                if survivors.len() == survivors.capacity() {
                    survivors.try_reserve(1).map_err(|_| CookieEditError::Internal)?;
                }
                survivors.push(pair.raw);
            }
        }
    }
    if !saw_field {
        return Ok(CookieEditPlan::Absent);
    }
    if !matched {
        return Ok(CookieEditPlan::NoChange);
    }
    if survivors.is_empty() {
        return Ok(CookieEditPlan::Remove);
    }
    let mut output = String::new();
    output
        .try_reserve_exact(output_len)
        .map_err(|_| CookieEditError::Internal)?;
    for raw in survivors {
        if !output.is_empty() {
            output.push_str("; ");
        }
        output.push_str(raw);
    }
    debug_assert_eq!(output.len(), output_len);
    let mut value = HeaderValue::try_from(output).map_err(|_| CookieEditError::Internal)?;
    value.set_sensitive(true);
    Ok(CookieEditPlan::Replace(value))
}

#[cfg(test)]
#[path = "../tests/unit/request_cookie_test.rs"]
mod tests;
