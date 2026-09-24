//! Path normalization algorithm — Edgion BASE.
//!
//! Implements the four-step fixed-order BASE normalization:
//! 1. `strip_path_parameters` — strip `;param=...` from each path segment
//! 2. percent-decode (whitelist: RFC 3986 unreserved chars + `%2F` + `%5C` only)
//! 3. `merge_slashes` — collapse consecutive `/` to one
//! 4. `remove_dot_segments` — RFC 3986 §5.2.4
//!
//! See `docs/superpowers/specs/2026-04-26-uri-normalization-design.md` §3.3 for
//! the full specification, whitelist table, edge-case handling, and security
//! commitment scope.

use super::config::PathNormalizationConfig;

/// Returns `true` if `path` is not already in normalized form and would change
/// after [`do_normalize`].
///
/// O(n) byte scan, zero allocation.
pub fn needs_normalization(path: &str, cfg: &PathNormalizationConfig) -> bool {
    let bytes = path.as_bytes();
    // Contains a `%XX` sequence that would actually decode. A bare `%`, invalid
    // hex (`%XY`), a truncated tail (`%2`), or a non-decodable byte (`%3F`,
    // `%C0`, `%25`) is left untouched by `do_normalize`, so it must NOT trigger
    // the allocating pipeline here. `has_decodable_percent` mirrors the scan in
    // `percent_decode_whitelist`, keeping this an exact zero-allocation predicate.
    if has_decodable_percent(bytes) {
        return true;
    }
    // Only treat `;` as a normalization trigger when path-parameter stripping is enabled.
    if !cfg.preserve_path_parameters && bytes.contains(&b';') {
        return true;
    }
    // Contains consecutive slashes
    if path.contains("//") {
        return true;
    }
    // Contains dot-segment sequences (use exact patterns to avoid false positives
    // like `/foo/..bar` or `/foo/.bar`)
    if path.contains("/./") || path.contains("/../") {
        return true;
    }
    if path.ends_with("/.") || path.ends_with("/..") {
        return true;
    }
    false
}

/// Returns `true` if `bytes` contains at least one `%XX` sequence whose two hex
/// digits decode to an [`is_decodable`] byte — the only case in which
/// [`percent_decode_whitelist`] would actually change the path.
///
/// The scan mirrors [`percent_decode_whitelist`] exactly: it advances one byte
/// at a time, so overlapping forms such as `%%41` (where `%41` decodes after the
/// leading `%` is consumed) align identically. Using a fixed `+= 3` stride here
/// would miss those, so the predicate stays byte-for-byte equivalent to the
/// decoder. Zero allocation.
fn has_decodable_percent(bytes: &[u8]) -> bool {
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'%' {
            let h = bytes[i + 1];
            let l = bytes[i + 2];
            if h.is_ascii_hexdigit() && l.is_ascii_hexdigit() && is_decodable((hex_val(h) << 4) | hex_val(l)) {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Normalize `raw` path using the Edgion BASE algorithm.
///
/// Returns `Some(normalized)` when the path changes after normalization.
/// Returns `None` when the path is already in normalized form (caller may use
/// `raw` as-is with zero allocation).
pub fn normalize_path(raw: &str, cfg: &PathNormalizationConfig) -> Option<String> {
    if !needs_normalization(raw, cfg) {
        return None;
    }
    // `needs_normalization` is a zero-allocation pre-filter; with
    // `has_decodable_percent` it only fires when `do_normalize` would actually
    // change the path. The comparison below is kept as a cheap defense-in-depth
    // safety net so the documented contract — return `Some` only when the path
    // changed — still holds if the pre-filter ever over-approximates.
    let normalized = do_normalize(raw, cfg);
    if normalized == raw {
        None
    } else {
        Some(normalized)
    }
}

/// Normalize a config-side path string at load time.
/// Returns normalized form when normalization changes the path; returns input as-is otherwise.
/// Use to keep config-side and request-side in the same normalized space.
pub fn normalize_config_path(s: &str, cfg: &PathNormalizationConfig) -> String {
    normalize_path(s, cfg).unwrap_or_else(|| s.to_string())
}

/// Canonicalize an already-normalized PathPrefix matcher at configuration build time.
/// Gateway API ignores its trailing slash; the root prefix must remain `/`.
/// Do not apply this to request paths, Exact matchers, or regular expressions.
pub fn canonicalize_path_prefix(path: &mut String) {
    if path.len() > 1 && path.ends_with('/') {
        path.pop();
    }
}

/// Returns `true` if `path` contains `%2F` or `%5C` (case-insensitive).
///
/// Used by the entry-filter to honour `reject_escaped_slashes` configuration.
pub fn contains_escaped_slash(path: &str) -> bool {
    let bytes = path.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'%' {
            let h = bytes[i + 1].to_ascii_uppercase();
            let l = bytes[i + 2].to_ascii_uppercase();
            if (h == b'2' && l == b'F') || (h == b'5' && l == b'C') {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Returns `true` if `path` contains any malformed percent-encoding:
/// - `%` followed by fewer than 2 characters
/// - `%XX` where `XX` contains a non-hex digit
/// - `%XX` sequences whose decoded byte sequence is not valid UTF-8 (e.g. overlong encodings)
///
/// Used by the entry-filter to honour `reject_invalid_percent_encoding` configuration.
pub fn has_invalid_percent_encoding(path: &str) -> bool {
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                // Truncated percent sequence
                return true;
            }
            let h = bytes[i + 1];
            let l = bytes[i + 2];
            if !h.is_ascii_hexdigit() || !l.is_ascii_hexdigit() {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    // All `%XX` sequences have valid hex digits; now check whether their decoded
    // byte values form a valid UTF-8 string.
    !decode_all_hex_check_utf8(path)
}

// ---------------------------------------------------------------------------
// pub(crate) helpers
// ---------------------------------------------------------------------------

/// Apply Edgion BASE normalization unconditionally.
///
/// Callers should prefer [`normalize_path`] (which short-circuits via
/// [`needs_normalization`]).  This function is `pub(crate)` so that tests in
/// sibling modules can call it directly.
pub(crate) fn do_normalize(raw: &str, cfg: &PathNormalizationConfig) -> String {
    let step1_owned: String;
    let step1: &str = if cfg.preserve_path_parameters {
        raw
    } else {
        step1_owned = strip_path_parameters(raw);
        &step1_owned
    };
    let step2 = percent_decode_whitelist(step1);
    let step3 = merge_slashes(&step2);
    remove_dot_segments(&step3)
}

// ---------------------------------------------------------------------------
// Private algorithm helpers
// ---------------------------------------------------------------------------

/// Strip `;param=...` from every path segment.
///
/// Everything from the first `;` in a segment to the next `/` or end-of-string
/// is removed.  The `/` separators themselves are preserved.
///
/// Examples:
/// - `/admin;jsessionid=x` → `/admin`
/// - `/api/users;v=1/data;type=json` → `/api/users/data`
/// - `/admin/..;/secret` → `/admin/../secret`
fn strip_path_parameters(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for (i, segment) in path.split('/').enumerate() {
        if i > 0 {
            out.push('/');
        }
        match segment.find(';') {
            Some(idx) => out.push_str(&segment[..idx]),
            None => out.push_str(segment),
        }
    }
    out
}

/// Decode `%XX` sequences that fall within the whitelist, leave everything
/// else encoded.
///
/// Whitelist: RFC 3986 §2.3 unreserved characters (`A-Z`, `a-z`, `0-9`,
/// `-`, `.`, `_`, `~`) plus the path delimiters `/` (`%2F`) and `\` (`%5C`).
///
/// Edge cases:
/// - Invalid hex (`%XY` where a digit is non-hex): kept as literal `%XY`.
/// - Decoded bytes that form an invalid UTF-8 sequence (e.g. `%C0%80`):
///   the entire path falls back to the original encoded form.
fn percent_decode_whitelist(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = bytes[i + 1];
            let l = bytes[i + 2];
            if h.is_ascii_hexdigit() && l.is_ascii_hexdigit() {
                let v = (hex_val(h) << 4) | hex_val(l);
                if is_decodable(v) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // If the resulting byte sequence is not valid UTF-8, preserve the original.
    match String::from_utf8(out) {
        Ok(s) => s,
        Err(_) => path.to_string(),
    }
}

/// `true` for characters that may be decoded from percent-encoded form.
///
/// Matches RFC 3986 §2.3 unreserved characters plus `/` and `\`.
#[inline]
fn is_decodable(b: u8) -> bool {
    matches!(
        b,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b'\\'
    )
}

/// Collapse runs of consecutive `/` characters into a single `/`.
fn merge_slashes(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut prev_slash = false;
    for ch in path.chars() {
        if ch == '/' {
            if !prev_slash {
                out.push('/');
            }
            prev_slash = true;
        } else {
            out.push(ch);
            prev_slash = false;
        }
    }
    out
}

/// Remove dot-segments per RFC 3986 §5.2.4.
///
/// Trailing-slash handling note (Edgion deviation from raw RFC algorithm):
/// the RFC algorithm naturally produces a trailing `/` after consuming a
/// terminal `/.` or `/..` (e.g. `/foo/bar/.` → `/foo/bar/`). We strip that
/// trailing `/` so the result equals what an operator typically expects
/// (`/foo/bar`).
///
/// **Important**: we only strip the trailing `/` when a terminal `/.` or
/// `/..` rule actually fired during processing. We do NOT strip when the
/// trailing `/` is a legitimate part of the input path that survived
/// `merge_slashes`. Without this distinction, `/admin//` (which
/// `merge_slashes` collapses to `/admin/`) would lose its trailing slash and
/// fail to match an operator's `Exact: /admin/` route.
fn remove_dot_segments(path: &str) -> String {
    let bytes = path.as_bytes();
    let n = bytes.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut i = 0usize;

    // Tracks whether the algorithm consumed a terminal `/.` or `/..` rule.
    // Only set by the B' (`rest == b"/."`) and C' (`rest == b"/.."`) branches.
    // Once set, stays set for the remainder of processing.
    let mut terminal_dot_segment_consumed = false;

    while i < n {
        let rest = &bytes[i..];

        if rest.starts_with(b"../") {
            // A: strip leading `../`
            i += 3;
        } else if rest.starts_with(b"./") {
            // A: strip leading `./`
            i += 2;
        } else if rest.starts_with(b"/./") {
            // B: `/./` → `/`; advance past `/.`, leaving the trailing `/` as new head
            i += 2;
        } else if rest == b"/." {
            // B': terminal `/.` → push `/` and stop (trailing `/` stripped below if needed)
            out.push(b'/');
            i = n;
            terminal_dot_segment_consumed = true;
        } else if rest.starts_with(b"/../") {
            // C: `/../` → `/` + pop; advance past `/..`, leaving trailing `/` as new head
            i += 3;
            pop_last_segment_bytes(&mut out);
        } else if rest == b"/.." {
            // C': terminal `/..` → pop last segment, push `/`, stop
            pop_last_segment_bytes(&mut out);
            out.push(b'/');
            i = n;
            terminal_dot_segment_consumed = true;
        } else if rest == b"." || rest == b".." {
            // D: lone `.` or `..` → discard
            i = n;
        } else {
            // E: copy first path segment (including leading `/` if present) to output
            let skip = usize::from(bytes[i] == b'/');
            let end = bytes[i + skip..]
                .iter()
                .position(|&b| b == b'/')
                .map(|p| i + skip + p)
                .unwrap_or(n);
            out.extend_from_slice(&bytes[i..end]);
            i = end;
        }
    }

    // Strip a trailing slash ONLY when produced by terminal dot-segment resolution.
    // Preserves trailing slashes that belong to the input (e.g. after `merge_slashes`
    // collapses `/admin//` to `/admin/`, the trailing `/` is part of operator semantics).
    if terminal_dot_segment_consumed && out.len() > 1 && out.last() == Some(&b'/') {
        out.pop();
    }

    // SAFETY: `out` is built by copying byte slices from `path` (valid UTF-8) plus
    // ASCII b'/'; slash-bounded copies preserve UTF-8 character boundaries.
    unsafe { String::from_utf8_unchecked(out) }
}

/// Remove the last segment from `out` (everything from the last `b'/'` onward).
fn pop_last_segment_bytes(out: &mut Vec<u8>) {
    if let Some(idx) = out.iter().rposition(|&b| b == b'/') {
        out.truncate(idx);
    } else {
        out.clear();
    }
}

/// Decode every syntactically valid `%XX` sequence to bytes and verify the
/// resulting byte string is valid UTF-8.
///
/// Used by [`has_invalid_percent_encoding`] to detect overlong or otherwise
/// invalid UTF-8 encodings that slip past the hex-digit check.
fn decode_all_hex_check_utf8(path: &str) -> bool {
    let bytes = path.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = bytes[i + 1];
            let l = bytes[i + 2];
            if h.is_ascii_hexdigit() && l.is_ascii_hexdigit() {
                out.push((hex_val(h) << 4) | hex_val(l));
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    std::str::from_utf8(&out).is_ok()
}

/// Convert a single ASCII hex digit to its numeric value.
#[inline]
fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "../../tests/unit/path_normalization/normalize_test.rs"]
mod tests;
