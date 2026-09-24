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
mod tests {
    use super::*;

    #[test]
    fn work_cost_enforces_hard_input_and_checked_arithmetic() {
        assert_eq!(work_cost(Kind::Sqli, 0), Some(BASE_WORK));
        assert_eq!(work_cost(Kind::Xss, 1), Some(BASE_WORK + XSS_WORK_PER_BYTE));
        assert!(work_cost(Kind::Sqli, MAX_INPUT_BYTES).is_some());
        assert_eq!(work_cost(Kind::Xss, MAX_INPUT_BYTES + 1), None);
        assert_eq!(work_cost(Kind::Xss, usize::MAX), None);
    }

    #[test]
    fn detection_and_fingerprint_match_pinned_examples() {
        assert_eq!(
            detect_sqli(b"1 OR 1=1").map(|fp| fp.as_bytes().to_vec()),
            Some(b"1&1".to_vec())
        );
        assert_eq!(detect_sqli(b"ordinary text"), None);
        assert!(detect_xss(b"<script>alert(1)</script>"));
        assert!(!detect_xss(b"<p>Hello</p>"));
        assert!(detect_sqli(b"'-(f=q'\xbf").is_some());
        assert_eq!(detect_sqli(b"1 OR q'\xbf\xc2\xbf'").unwrap().as_bytes(), b"1&s");
        assert!(detect_xss(b"<a \xffid=x><script>alert(1)</script>"));
    }

    #[test]
    fn full_size_and_adversarial_inputs_finish_within_default_work_budget() {
        let full = vec![b'a'; MAX_INPUT_BYTES];
        assert!(work_cost(Kind::Sqli, full.len()).unwrap() <= crate::seclang::DEFAULT_MAX_RUNTIME_OPERATOR_WORK_BYTES);
        assert!(work_cost(Kind::Xss, full.len()).unwrap() <= crate::seclang::DEFAULT_MAX_RUNTIME_OPERATOR_WORK_BYTES);
        assert!(detect_sqli(&full).is_none());
        assert!(!detect_xss(&full));

        for pattern in [
            b"'/*".as_slice(),
            b"<!--".as_slice(),
            b"<&".as_slice(),
            b"<a ".as_slice(),
            b"a.a`".as_slice(),
        ] {
            let mut input = Vec::with_capacity(256 * 1024);
            while input.len() < 256 * 1024 {
                input.extend_from_slice(pattern);
            }
            input.truncate(256 * 1024);
            let _ = detect_sqli(&input);
            let _ = detect_xss(&input);
        }
    }
}
