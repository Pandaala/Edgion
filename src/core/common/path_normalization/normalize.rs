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
mod tests {
    use super::PathNormalizationConfig;
    use super::*;

    // ------------------------------------------------------------------
    // Happy paths
    // ------------------------------------------------------------------

    #[test]
    fn no_change_for_normalized_path() {
        assert_eq!(normalize_path("/api/users", &PathNormalizationConfig::default()), None);
        assert_eq!(normalize_path("/", &PathNormalizationConfig::default()), None);
        assert_eq!(normalize_path("", &PathNormalizationConfig::default()), None);
        assert_eq!(
            normalize_path("/api/users/123", &PathNormalizationConfig::default()),
            None
        );
    }

    #[test]
    fn percent_decode_unreserved() {
        // %61 = 'a'
        assert_eq!(
            normalize_path("/%61dmin", &PathNormalizationConfig::default()),
            Some("/admin".to_string())
        );
        // %2D = '-'
        assert_eq!(
            normalize_path("/foo%2Dbar", &PathNormalizationConfig::default()),
            Some("/foo-bar".to_string())
        );
        // %5F = '_'
        assert_eq!(
            normalize_path("/foo%5Fbar", &PathNormalizationConfig::default()),
            Some("/foo_bar".to_string())
        );
        // %7E = '~'
        assert_eq!(
            normalize_path("/foo%7Ebar", &PathNormalizationConfig::default()),
            Some("/foo~bar".to_string())
        );
        // %2E = '.'
        assert_eq!(
            normalize_path("/foo%2Ebar", &PathNormalizationConfig::default()),
            Some("/foo.bar".to_string())
        );
    }

    #[test]
    fn percent_decode_slash_and_backslash() {
        assert_eq!(
            normalize_path("/admin%2Fextra", &PathNormalizationConfig::default()),
            Some("/admin/extra".to_string())
        );
        assert_eq!(
            normalize_path("/admin%5Cextra", &PathNormalizationConfig::default()),
            Some("/admin\\extra".to_string())
        );
    }

    #[test]
    fn reserved_chars_kept_encoded() {
        // Reserved / sub-delim chars are not in the decode whitelist, so the path is
        // unchanged by normalization: normalize_path returns None (contract), while
        // do_normalize keeps the bytes encoded.
        let cfg = PathNormalizationConfig::default();
        // %3F = '?'
        assert_eq!(normalize_path("/path%3Fquery", &cfg), None);
        assert!(do_normalize("/path%3Fquery", &cfg).contains("%3F"));
        // %23 = '#'
        assert_eq!(normalize_path("/path%23frag", &cfg), None);
        assert!(do_normalize("/path%23frag", &cfg).contains("%23"));
        // sub-delim %21 = '!'
        assert_eq!(normalize_path("/path%21x", &cfg), None);
        assert!(do_normalize("/path%21x", &cfg).contains("%21"));
    }

    #[test]
    fn merge_double_slash() {
        assert_eq!(
            normalize_path("/foo//bar", &PathNormalizationConfig::default()),
            Some("/foo/bar".to_string())
        );
        assert_eq!(
            normalize_path("/foo///bar", &PathNormalizationConfig::default()),
            Some("/foo/bar".to_string())
        );
        assert_eq!(
            normalize_path("//", &PathNormalizationConfig::default()),
            Some("/".to_string())
        );
    }

    #[test]
    fn remove_dot_segments_basic() {
        assert_eq!(
            normalize_path("/foo/./bar", &PathNormalizationConfig::default()),
            Some("/foo/bar".to_string())
        );
        assert_eq!(
            normalize_path("/foo/../bar", &PathNormalizationConfig::default()),
            Some("/bar".to_string())
        );
        assert_eq!(
            normalize_path("/foo/bar/.", &PathNormalizationConfig::default()),
            Some("/foo/bar".to_string())
        );
        assert_eq!(
            normalize_path("/foo/bar/..", &PathNormalizationConfig::default()),
            Some("/foo".to_string())
        );
    }

    #[test]
    fn does_not_misinterpret_dotdot_prefix() {
        // /foo/..bar must NOT be treated as parent-traversal
        assert_eq!(normalize_path("/foo/..bar", &PathNormalizationConfig::default()), None);
        assert_eq!(normalize_path("/foo/.bar", &PathNormalizationConfig::default()), None);
    }

    #[test]
    fn strip_path_parameters_basic() {
        assert_eq!(
            normalize_path("/admin;jsessionid=x", &PathNormalizationConfig::default()),
            Some("/admin".to_string())
        );
        assert_eq!(
            normalize_path("/api/users;v=1/data;type=json", &PathNormalizationConfig::default()),
            Some("/api/users/data".to_string())
        );
    }

    #[test]
    fn dotdot_with_path_parameter() {
        // /admin/..;/secret → strip param → /admin/../secret → dot-segment → /secret
        assert_eq!(
            normalize_path("/admin/..;jsessionid=x/secret", &PathNormalizationConfig::default()),
            Some("/secret".to_string())
        );
        assert_eq!(
            normalize_path("/admin/..;/secret", &PathNormalizationConfig::default()),
            Some("/secret".to_string())
        );
    }

    #[test]
    fn encoded_dotdot_traversal() {
        // %2E%2E = '..' after decode, then dot-segment processing → /secret
        assert_eq!(
            normalize_path("/admin/%2E%2E/secret", &PathNormalizationConfig::default()),
            Some("/secret".to_string())
        );
    }

    #[test]
    fn combined_attack_variants() {
        // /admin%2F%2F../etc → decode %2F → /admin///../etc → merge → /admin/../etc → /etc
        assert_eq!(
            normalize_path("/admin%2F%2F../etc", &PathNormalizationConfig::default()),
            Some("/etc".to_string())
        );
    }

    // ------------------------------------------------------------------
    // Edge cases
    // ------------------------------------------------------------------

    #[test]
    fn invalid_hex_preserved() {
        // Invalid / truncated `%` sequences are kept as literals, so the path is
        // unchanged by normalization → normalize_path returns None (contract).
        let cfg = PathNormalizationConfig::default();
        // %XY where Y is not hex
        assert_eq!(normalize_path("/admin%XY/x", &cfg), None);
        // % at end of path
        assert_eq!(normalize_path("/admin%", &cfg), None);
        // % followed by single hex digit only
        assert_eq!(normalize_path("/admin%2", &cfg), None);
    }

    #[test]
    fn overlong_utf8_preserved_encoded() {
        // %C0%80 = overlong encoding of NUL — not in is_decodable whitelist, so the
        // bytes stay encoded and the path is unchanged → normalize_path returns None
        // (contract). do_normalize keeps the encoded bytes (asserted below).
        let cfg = PathNormalizationConfig::default();
        assert_eq!(normalize_path("/admin/%C0%80/secret", &cfg), None);
        let r = do_normalize("/admin/%C0%80/secret", &cfg);
        assert!(
            r.contains("%C0") || r.contains("%c0"),
            "overlong bytes must remain encoded, got: {r}"
        );
        assert!(r.contains("%80"), "overlong bytes must remain encoded, got: {r}");
    }

    #[test]
    fn double_encoding_decoded_once_only() {
        // %252E = %25 (literal %) + 2E.
        // %25 decodes to '%' which is NOT is_decodable, so it stays as %25.
        // Result: /admin/%252E%252E/secret is unchanged by percent_decode_whitelist.
        // needs_normalization sees %, returns true; do_normalize strip + decode +
        // merge + dot-segment all leave it unchanged, so the path is unchanged →
        // normalize_path returns None (contract).
        let cfg = PathNormalizationConfig::default();
        assert_eq!(normalize_path("/admin/%252E%252E/secret", &cfg), None);
        // do_normalize must NOT recursively decode; %25 stays encoded.
        let r = do_normalize("/admin/%252E%252E/secret", &cfg);
        assert!(
            !r.starts_with("/secret"),
            "double-encoded dots must NOT be recursively decoded, got: {r}"
        );
        assert!(r.contains("%25"), "percent literal (%25) must stay encoded, got: {r}");
    }

    #[test]
    fn has_invalid_percent_encoding_valid_paths() {
        assert!(!has_invalid_percent_encoding("/admin"));
        assert!(!has_invalid_percent_encoding("/api/users/123"));
        assert!(!has_invalid_percent_encoding("/%61dmin")); // valid ASCII decode
    }

    #[test]
    fn has_invalid_percent_encoding_invalid_hex() {
        assert!(has_invalid_percent_encoding("/%XY/x"));
        assert!(has_invalid_percent_encoding("/admin%"));
        assert!(has_invalid_percent_encoding("/admin%2"));
    }

    #[test]
    fn has_invalid_percent_encoding_overlong_utf8() {
        // %C0%80 decodes to bytes that are invalid UTF-8
        assert!(has_invalid_percent_encoding("/%C0%80"));
    }

    #[test]
    fn contains_escaped_slash_detection() {
        assert!(contains_escaped_slash("/admin%2F"));
        assert!(contains_escaped_slash("/admin%2f")); // lowercase
        assert!(contains_escaped_slash("/admin%5C"));
        assert!(contains_escaped_slash("/admin%5c")); // lowercase
        assert!(!contains_escaped_slash("/admin"));
        assert!(!contains_escaped_slash("/admin%21")); // '!' not a slash
    }

    #[test]
    fn empty_and_root_paths_not_normalized() {
        assert_eq!(normalize_path("", &PathNormalizationConfig::default()), None);
        assert_eq!(normalize_path("/", &PathNormalizationConfig::default()), None);
    }

    #[test]
    fn needs_normalization_negative_cases() {
        assert!(!needs_normalization("/api/users", &PathNormalizationConfig::default()));
        // /foo/..bar must NOT trigger (the `/../` check is used, not `/..`)
        assert!(!needs_normalization("/foo/..bar", &PathNormalizationConfig::default()));
        // /foo/.bar must NOT trigger
        assert!(!needs_normalization("/foo/.bar", &PathNormalizationConfig::default()));
        assert!(!needs_normalization("", &PathNormalizationConfig::default()));
        assert!(!needs_normalization("/", &PathNormalizationConfig::default()));
        // A `%` that does not decode must NOT trigger the allocating pipeline:
        // invalid hex, truncated tails, non-decodable reserved/overlong bytes,
        // and double-encoded percents are all inert under `do_normalize`.
        let cfg = PathNormalizationConfig::default();
        assert!(!needs_normalization("/admin%XY/x", &cfg)); // invalid hex
        assert!(!needs_normalization("/admin%", &cfg)); // bare `%`
        assert!(!needs_normalization("/admin%2", &cfg)); // truncated tail
        assert!(!needs_normalization("/path%3Fquery", &cfg)); // %3F = '?' (reserved)
        assert!(!needs_normalization("/admin/%C0%80/secret", &cfg)); // overlong UTF-8
        assert!(!needs_normalization("/admin/%252E%252E/secret", &cfg)); // %25 = '%'
    }

    #[test]
    fn needs_normalization_positive_cases() {
        assert!(needs_normalization("/%61dmin", &PathNormalizationConfig::default())); // %
        assert!(needs_normalization(
            "/admin;session",
            &PathNormalizationConfig::default()
        )); // ;
        assert!(needs_normalization("/foo//bar", &PathNormalizationConfig::default())); // //
        assert!(needs_normalization("/foo/./bar", &PathNormalizationConfig::default())); // /./
        assert!(needs_normalization("/foo/../bar", &PathNormalizationConfig::default())); // /../
        assert!(needs_normalization("/foo/.", &PathNormalizationConfig::default())); // ends_with /.
        assert!(needs_normalization("/foo/..", &PathNormalizationConfig::default()));
        // ends_with /..
    }

    #[test]
    fn has_decodable_percent_matches_decoder() {
        // Positive: at least one `%XX` decodes to an `is_decodable` byte.
        assert!(has_decodable_percent(b"/%61dmin")); // %61 = 'a'
        assert!(has_decodable_percent(b"/admin%2Fextra")); // %2F = '/'
        assert!(has_decodable_percent(b"/foo%2Ebar")); // %2E = '.'
                                                       // Byte-alignment with `percent_decode_whitelist`: a non-decodable `%`
                                                       // is consumed one byte at a time, so `%41` is still found after the
                                                       // leading `%` in `%%41`. A fixed `+= 3` stride would miss this and is
                                                       // exactly the bug avoided here.
        assert!(has_decodable_percent(b"%%41")); // decodes to "%A"
                                                 // Negative: nothing decodes — must stay zero-allocation `false`.
        assert!(!has_decodable_percent(b"/admin%XY/x")); // invalid hex
        assert!(!has_decodable_percent(b"/admin%")); // bare `%`
        assert!(!has_decodable_percent(b"/admin%2")); // truncated tail
        assert!(!has_decodable_percent(b"/path%3Fquery")); // %3F = '?' (reserved)
        assert!(!has_decodable_percent(b"/%C0%80")); // overlong UTF-8
        assert!(!has_decodable_percent(b"/admin/%252E%252E/secret")); // %25 = '%'
        assert!(!has_decodable_percent(b"/api/users")); // no `%` at all
    }

    // ------------------------------------------------------------------
    // do_normalize internal steps
    // ------------------------------------------------------------------

    #[test]
    fn strip_then_decode_then_merge_then_dot() {
        // Verify that the order is: strip → decode → merge → dot-segment.
        // /admin%2F%2F../etc  → step1: no `;` → /admin%2F%2F../etc
        //                     → step2: %2F→/ → /admin///../etc
        //                     → step3: merge → /admin/../etc
        //                     → step4: dot  → /etc
        assert_eq!(
            do_normalize("/admin%2F%2F../etc", &PathNormalizationConfig::default()),
            "/etc"
        );
    }

    #[test]
    fn overlong_utf8_fallback_in_do_normalize() {
        // %C0 and %80 are not is_decodable, so they stay encoded throughout.
        // The String::from_utf8 fallback is only triggered if we accidentally
        // push non-UTF-8 bytes, which we should not since we check is_decodable.
        let result = do_normalize("/x/%C0%80/y", &PathNormalizationConfig::default());
        assert!(result.contains("%C0") || result.contains("%c0"));
    }

    #[test]
    fn path_parameter_with_encoded_segment() {
        // /api/users;v=1%2F/data → strip → /api/users/data → decode (no % left) → /api/users/data
        // Wait: %2F is IN the segment AFTER the ; was stripped, so:
        // strip: "/api/users" then "/data" → "/api/users/data"  (the %2F was in the param part)
        // Actually the input is: /api/users;v=1%2F/data
        // split by '/': ["", "api", "users;v=1%2F", "data"]
        // segment "users;v=1%2F" → strip at ';' → "users"
        // result: /api/users/data
        assert_eq!(
            do_normalize("/api/users;v=1%2F/data", &PathNormalizationConfig::default()),
            "/api/users/data"
        );
    }

    #[test]
    fn root_traversal_clamped() {
        // Going above root stays at root
        assert_eq!(
            do_normalize("/../../etc/passwd", &PathNormalizationConfig::default()),
            "/etc/passwd"
        );
    }

    // ----------------------------------------------------------------------
    // Trailing-slash preservation (regression for Issue 1)
    // ----------------------------------------------------------------------
    // Operator may distinguish `/admin/` from `/admin` semantically. The
    // normalization layer must NOT strip a trailing `/` that came from
    // legitimate input (after merge_slashes). It MUST still strip when the
    // trailing `/` is the byproduct of consuming a terminal `/.` or `/..`.

    #[test]
    fn merge_slashes_preserves_trailing_slash() {
        // `/admin//` → merge to `/admin/`; trailing `/` must survive
        assert_eq!(
            normalize_path("/admin//", &PathNormalizationConfig::default()),
            Some("/admin/".to_string())
        );
        // `/admin///` → merge to `/admin/`; same
        assert_eq!(
            normalize_path("/admin///", &PathNormalizationConfig::default()),
            Some("/admin/".to_string())
        );
        // `/foo//bar/` (mid-double + trailing) → `/foo/bar/`
        assert_eq!(
            normalize_path("/foo//bar/", &PathNormalizationConfig::default()),
            Some("/foo/bar/".to_string())
        );
    }

    #[test]
    fn mid_string_dot_segments_preserve_trailing_slash() {
        // `/foo/./bar/` → `/foo/bar/` — mid `/./` resolves but does NOT strip trailing `/`
        assert_eq!(
            normalize_path("/foo/./bar/", &PathNormalizationConfig::default()),
            Some("/foo/bar/".to_string())
        );
        // `/foo/bar/../` → `/foo/` — mid `/../` resolves but trailing `/` is part of input
        assert_eq!(
            normalize_path("/foo/bar/../", &PathNormalizationConfig::default()),
            Some("/foo/".to_string())
        );
    }

    #[test]
    fn terminal_dot_segments_strip_trailing_slash() {
        // `/foo/.` → `/foo` (terminal `/.` consumed; trailing `/` stripped) — existing behavior
        assert_eq!(
            normalize_path("/foo/.", &PathNormalizationConfig::default()),
            Some("/foo".to_string())
        );
        // `/foo/..` → `/` (pop_last + strip trailing) — output is `/`, len <=1 so untouched
        assert_eq!(
            normalize_path("/foo/..", &PathNormalizationConfig::default()),
            Some("/".to_string())
        );
        // `/foo/bar/.` → `/foo/bar` (existing behavior preserved)
        assert_eq!(
            normalize_path("/foo/bar/.", &PathNormalizationConfig::default()),
            Some("/foo/bar".to_string())
        );
        // `/foo/bar/..` → `/foo` (existing behavior preserved)
        assert_eq!(
            normalize_path("/foo/bar/..", &PathNormalizationConfig::default()),
            Some("/foo".to_string())
        );
    }

    #[test]
    fn combined_terminal_dot_after_merge_slashes() {
        // `/admin//foo/.` → merge to `/admin/foo/.` → terminal `/.` consumed → `/admin/foo`
        assert_eq!(
            normalize_path("/admin//foo/.", &PathNormalizationConfig::default()),
            Some("/admin/foo".to_string())
        );
    }

    #[test]
    fn percent_decoded_double_slash_preserves_trailing() {
        // `/admin%2F` → decode `%2F`=`/` → `/admin/` → no terminal dot → preserve
        assert_eq!(
            normalize_path("/admin%2F", &PathNormalizationConfig::default()),
            Some("/admin/".to_string())
        );
    }

    #[test]
    fn preserve_path_parameters_keeps_param_literal() {
        let cfg = PathNormalizationConfig {
            preserve_path_parameters: true,
            ..Default::default()
        };
        // ;param segment is kept verbatim; dot-segment / merge_slashes / decode still work normally
        assert_eq!(normalize_path("/api/users;v=1/data", &cfg), None);
        // % still triggers normalization but ;param is not stripped
        assert_eq!(
            normalize_path("/%61pi/users;v=1/data", &cfg),
            Some("/api/users;v=1/data".to_string())
        );
    }

    #[test]
    fn preserve_path_parameters_documents_bypass_scope_removal() {
        // (Security scope §14.4): When preserve_path_parameters=true, the `..;/` style
        // bypass is explicitly removed from the §1.4 commitment scope. This test pins
        // that behavior — the path is NOT normalized to /secret because `;` is not a
        // normalization trigger when stripping is disabled, and the dot-segment
        // algorithm uses strict `/../` (not `/..;/`) matching.
        let cfg = PathNormalizationConfig {
            preserve_path_parameters: true,
            ..Default::default()
        };
        assert_eq!(normalize_path("/admin/..;/secret", &cfg), None);
    }

    #[test]
    fn strip_path_parameters_default_strips() {
        let cfg = PathNormalizationConfig::default(); // preserve = false
        assert_eq!(
            normalize_path("/api/users;v=1/data", &cfg),
            Some("/api/users/data".to_string())
        );
    }

    #[test]
    fn preserve_path_parameters_only_param_no_other_triggers() {
        // Path containing only `;param` (no %, no //, no /../) under preserve=true
        // should pass through unchanged — `needs_normalization` short-circuits to None
        // because `;` no longer triggers normalization in preserve mode.
        let cfg = PathNormalizationConfig {
            preserve_path_parameters: true,
            ..Default::default()
        };
        assert_eq!(normalize_path("/api/users;v=1/data", &cfg), None);
        assert_eq!(normalize_path("/admin;jsessionid=x", &cfg), None);
    }

    #[test]
    fn normalize_output_is_always_a_valid_uri_path() {
        // Defensive corpus test (spec §14, addresses parse failure path):
        // Every output of normalize_path() must be parseable as `http::Uri`
        // (path component) without error. If a future normalize change ever emits
        // a byte sequence that http::Uri rejects, this test catches it before
        // production users see a 500 from the upstream_peer single-point block.
        let cfg = PathNormalizationConfig::default();
        let inputs: &[&str] = &[
            "/%61dmin/data",
            "/admin//x",
            "/admin/../secret",
            "/admin/..;jsessionid=abc/secret",
            "/admin/%2E%2E/secret",
            "/admin%2F%2F../etc",
            "/api/users;v=1/data",
            "/admin/.",
            "/admin/..",
            "//",
            "/foo/./bar",
            "/foo/bar/.",
            "/admin%2Fextra",
            "/admin%5Cextra",
            "/.well-known/acme-challenge/abc",
        ];
        for input in inputs {
            let normalized = match normalize_path(input, &cfg) {
                Some(s) => s,
                None => input.to_string(),
            };
            // Compose `path?query` style URI (mirrors `ctx.uri()` behaviour).
            let composed = normalized.clone();
            composed.parse::<http::Uri>().unwrap_or_else(|e| {
                panic!("normalize output {composed:?} (from input {input:?}) failed http::Uri parse: {e}")
            });
        }
    }
}
