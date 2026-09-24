//! Bounded SQLi and XSS detection used by SecLang operators.
//!
//! Detection behavior and static data are pinned to libinjection-go v0.3.2.
//! The inner Rust scanners never retain request bytes and use stack-backed
//! token storage. The work charge reserves an upper bound for all SQL contexts
//! or all five HTML contexts before scanning, including keyword lookups.

use crate::{sqli, xss};

pub(crate) const MAX_INPUT_BYTES: usize = 4 * 1024 * 1024;
const BASE_WORK: usize = 4096;
// SQLi can reparse in five dialect/quote contexts. XSS checks five HTML
// contexts; reserve two logical byte visits per context for token handling.
const SQLI_WORK_PER_BYTE: usize = 8;
const XSS_WORK_PER_BYTE: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    bytes: [u8; 5],
    len: u8,
}

impl Fingerprint {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

pub(crate) enum Kind {
    Sqli,
    Xss,
}

/// Returns a reserved scan cost or `None` if the hard input cap is exceeded.
pub(crate) fn work_cost(kind: Kind, input_len: usize) -> Option<usize> {
    if input_len > MAX_INPUT_BYTES {
        return None;
    }
    let multiplier = match kind {
        Kind::Sqli => SQLI_WORK_PER_BYTE,
        Kind::Xss => XSS_WORK_PER_BYTE,
    };
    input_len.checked_mul(multiplier)?.checked_add(BASE_WORK)
}

pub(crate) fn detect_sqli(input: &[u8]) -> Option<Fingerprint> {
    let mut state = sqli::SqliState::new(input, sqli::SqliFlags::FLAG_NONE);
    if !state.detect() {
        return None;
    }
    let mut bytes = [0_u8; 5];
    let raw = &state.fingerprint;
    let len = raw.iter().position(|byte| *byte == 0).unwrap_or(5).min(5);
    bytes[..len].copy_from_slice(&raw[..len]);
    Some(Fingerprint { bytes, len: len as u8 })
}

pub(crate) fn detect_xss(input: &[u8]) -> bool {
    xss::XssDetector::new().detect(input).is_injection()
}

#[cfg(test)]
#[path = "../tests/unit/injection_test.rs"]
mod tests;
