//! Human-readable byte-size parsing, shared across resource schemas.
//!
//! Used by both plugin `body.maxBodySize` (`edgion_plugins::body_requirement`)
//! and `EdgionGatewayConfig.spec.requestBody` — hence it lives in `common`
//! rather than under a specific resource submodule.

/// Parse a human-readable byte-size string into bytes.
///
/// Accepts a plain byte count (`"1024"`) or a number with a 1024-based unit
/// suffix, case-insensitive: `k`/`kb`/`ki`/`kib`, `m`/`mb`/`mi`/`mib`,
/// `g`/`gb`/`gi`/`gib`, `b`. Decimals are allowed (`"1.5m"`). Returns `None` on a
/// malformed, negative, or non-finite value. Overflow saturates to `u64::MAX`.
pub fn parse_byte_size(s: &str) -> Option<u64> {
    const K: u64 = 1024;
    const M: u64 = 1024 * 1024;
    const G: u64 = 1024 * 1024 * 1024;
    // Longest suffix per unit first so `kb` is not shadowed by `b`.
    const UNITS: &[(&str, u64)] = &[
        ("kib", K),
        ("kb", K),
        ("ki", K),
        ("k", K),
        ("mib", M),
        ("mb", M),
        ("mi", M),
        ("m", M),
        ("gib", G),
        ("gb", G),
        ("gi", G),
        ("g", G),
        ("b", 1),
    ];

    let t = s.trim().to_ascii_lowercase();
    if t.is_empty() {
        return None;
    }

    let (num_str, mult) = UNITS
        .iter()
        .find_map(|(suffix, mult)| t.strip_suffix(suffix).map(|n| (n, *mult)))
        .unwrap_or((t.as_str(), 1));

    let num_str = num_str.trim();
    if num_str.is_empty() {
        return None;
    }
    let num: f64 = num_str.parse().ok()?;
    if num < 0.0 || !num.is_finite() {
        return None;
    }
    Some((num * mult as f64) as u64)
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/common/byte_size_test.rs"]
mod tests;
