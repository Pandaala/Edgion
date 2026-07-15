//! Dynamic upstream authority: a wildcard pattern a backend declares, plus the
//! per-request template that fills its single wildcard label.
//!
//! Safety here is structural. The pattern's suffix is a compile-time constant that
//! rendering never touches, and the template is admitted only if its literal tail
//! equals that suffix and its dynamic slot contains no literal `.`. Rendering can
//! therefore only ever produce ONE label, which is concatenated onto the fixed
//! suffix — an injected value has nowhere to go. See
//! docs/superpowers/specs/2026-07-14-dynamic-upstream-authority-design.md §5.4.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::edgion_plugins::plugin_configs::{CompiledTemplate, TemplateSegment};

/// Operator-facing config. Attached to a Service through
/// `EdgionBackendTrafficPolicy.spec.upstreamAuthority`.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpstreamAuthorityConfig {
    /// Wildcard pattern the backend serves, e.g. `*.sandbox.internal`.
    /// Exactly one leading `*.` label. This is the allowlist AND the shape.
    pub pattern: String,
    /// How the wildcard label is produced per request, e.g.
    /// `${ctx:tenant}.sandbox.internal`. Its literal tail must equal the
    /// pattern's suffix, and its dynamic slot must hold no literal `.`.
    pub template: String,
    /// Static authority for active health probes, which have no request to
    /// render from. Required whenever an active health check is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check_host: Option<String>,
}

/// `"*.a.b"` -> `".a.b"`. Errors unless the pattern is exactly one leading
/// wildcard label followed by a non-empty suffix whose every label is a
/// valid DNS label (per `validate_label`). The returned suffix is always
/// lowercase, so `CompiledUpstreamAuthority::compile` and
/// `host_matches_pattern` can both compare against it directly, with no
/// separate case-normalization step of their own.
pub fn pattern_suffix(pattern: &str) -> Result<String, String> {
    let rest = pattern
        .strip_prefix("*.")
        .ok_or_else(|| format!("pattern must start with '*.': got '{pattern}'"))?;
    if rest.is_empty() {
        return Err("pattern must have a suffix after '*.'".to_string());
    }
    if rest.contains('*') {
        return Err(format!("pattern may contain only one wildcard label: got '{pattern}'"));
    }
    // The suffix is spliced verbatim into the upstream Host header and TLS
    // SNI, so it must be validated as strictly as the rendered label: every
    // dot-separated label must pass the same character/length/hyphen rules.
    // A trailing dot (or a doubled dot) surfaces here as a trailing/empty
    // label and is rejected the same way.
    let labels: Vec<&str> = rest.split('.').collect();
    for label in &labels {
        validate_label(label).map_err(|e| format!("pattern suffix label '{label}' is invalid: {e}"))?;
    }
    // The composed host is `<one rendered label>.<suffix>`. The rendered
    // label is validated to `[a-z0-9-]`, so it can itself be all-digits
    // (e.g. "127"). If the suffix's FINAL label were also all-digits, a
    // pattern like `*.0.0.1` would let a request-controlled label compose
    // to a 4-label all-numeric host such as "127.0.0.1" — a valid IPv4
    // literal that hyper/reqwest's IP-literal fast path dials directly,
    // bypassing the AnchorPinnedResolver (see call.rs / authority.rs). A
    // real DNS TLD is never all-digits, so rejecting an all-numeric final
    // suffix label costs no legitimate pattern while making an IPv4
    // composition impossible: `std::net::Ipv4Addr::from_str` requires
    // exactly four dotted decimal octets, so a non-numeric final label
    // (guaranteed here) can never parse as an IP address regardless of
    // what the other labels are, and it also rejects hex/octal/dword
    // forms outright (it only accepts dotted-quad decimal).
    if let Some(last) = labels.last() {
        if !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()) {
            return Err(
                "pattern's final label must not be all-numeric (would allow composing an IP address)".to_string(),
            );
        }
    }
    Ok(format!(".{}", rest.to_ascii_lowercase()))
}

/// True when `host` is the pattern's suffix preceded by exactly one label.
/// Case-insensitive, per DNS.
pub fn host_matches_pattern(host: &str, pattern: &str) -> bool {
    let Ok(suffix) = pattern_suffix(pattern) else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    let suffix = suffix.to_ascii_lowercase();
    let Some(label) = host.strip_suffix(&suffix) else {
        return false;
    };
    validate_label(label).is_ok()
}

/// Validate a rendered wildcard label and return it lowercase-normalized.
///
/// The character set excludes `.`, `/`, `?`, `#`, `@`, `:` and CR/LF — every
/// character an injected value would need to grow a second label, escape the
/// host, or split the request.
pub fn validate_label(label: &str) -> Result<String, &'static str> {
    if label.is_empty() {
        return Err("empty");
    }
    if label.len() > 63 {
        return Err("longer than 63 bytes");
    }
    let lower = label.to_ascii_lowercase();
    if !lower
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("must contain only [a-z0-9-]");
    }
    if lower.starts_with('-') || lower.ends_with('-') {
        return Err("must not start or end with '-'");
    }
    Ok(lower)
}

/// The compiled form: a template for the label slot ONLY, plus the fixed suffix.
/// Rendering never touches the suffix — that is what makes escape impossible.
#[derive(Debug, Clone)]
pub struct CompiledUpstreamAuthority {
    pub label_template: CompiledTemplate,
    pub suffix: String,
    pub health_check_host: Option<String>,
}

impl CompiledUpstreamAuthority {
    pub fn compile(cfg: &UpstreamAuthorityConfig) -> Result<Self, String> {
        let suffix = pattern_suffix(&cfg.pattern)?;

        // DNS is case-insensitive, so the tail is matched case-insensitively — but the cut
        // is taken from the ORIGINAL template. Variable types are camelCase (`requestPath`,
        // `matchedHttpRoute`) and ctx/query/cookie/secretRef names are case-sensitive, so the
        // template's interior must survive verbatim. `to_ascii_lowercase` is a 1:1 byte
        // mapping, so the offset is valid in both strings, and `suffix` is ASCII by
        // construction (`pattern_suffix` validates every label).
        if !cfg.template.to_ascii_lowercase().ends_with(&suffix) {
            return Err(format!("template must end with the pattern suffix '{suffix}'"));
        }
        let label_src = &cfg.template[..cfg.template.len() - suffix.len()];
        if label_src.is_empty() {
            return Err("template must have a label slot before the pattern suffix".to_string());
        }

        let label_template =
            CompiledTemplate::compile(label_src).map_err(|e| format!("template is not compilable: {e}"))?;

        // A literal '.' anywhere in the label slot would yield two labels;
        // that is the security-relevant case and gets its own message. Any
        // other character outside the rendered label's allowed set would
        // make every request to this backend fail closed at runtime (the
        // literal fragment can never pass `validate_label`), so reject that
        // too, with a separate, more general message.
        for seg in label_template.segments() {
            if let TemplateSegment::Literal(lit) = seg {
                if lit.contains('.') {
                    return Err("template's label slot must not contain a literal '.'".to_string());
                }
                if !lit
                    .to_ascii_lowercase()
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                {
                    return Err(format!(
                        "template's label slot contains a literal fragment '{lit}' with a character \
                         outside [a-z0-9-]; it can never pass validation at request time"
                    ));
                }
            }
        }
        // A fully-literal slot is legal but pointless; a slot with no variable at
        // all means the authority is static, which this feature is not for.
        if !label_template
            .segments()
            .iter()
            .any(|s| matches!(s, TemplateSegment::Variable { .. }))
        {
            return Err("template's label slot must contain at least one ${...} variable".to_string());
        }
        // A rendered label can never start or end with '-' (`validate_label` rejects it on
        // every request), so a template whose first or last segment is a literal starting or
        // ending with '-' is statically doomed to fail-closed on every request. Admission
        // should prove a render can succeed, not just that its charset is legal. An interior
        // '-' remains legal.
        if let Some(TemplateSegment::Literal(lit)) = label_template.segments().first() {
            if lit.starts_with('-') {
                return Err(
                    "template's label slot must not start with a literal '-': the rendered label \
                     would fail validation on every request"
                        .to_string(),
                );
            }
        }
        if let Some(TemplateSegment::Literal(lit)) = label_template.segments().last() {
            if lit.ends_with('-') {
                return Err(
                    "template's label slot must not end with a literal '-': the rendered label \
                     would fail validation on every request"
                        .to_string(),
                );
            }
        }

        Ok(Self {
            label_template,
            suffix,
            health_check_host: cfg.health_check_host.clone(),
        })
    }

    /// `<label>` + the fixed suffix. `label` must already be `validate_label`-ed.
    pub fn compose(&self, label: &str) -> String {
        let mut host = String::with_capacity(label.len() + self.suffix.len());
        host.push_str(label);
        host.push_str(&self.suffix);
        host
    }
}

/// Admission-time validation. `has_active_health_check` comes from the sibling
/// `spec.healthCheck.active` on the same policy.
pub fn validate_upstream_authority(cfg: &UpstreamAuthorityConfig, has_active_health_check: bool) -> Vec<String> {
    let mut errors = Vec::new();

    if let Err(e) = CompiledUpstreamAuthority::compile(cfg) {
        errors.push(format!("upstreamAuthority: {e}"));
        // Every rule below depends on a well-formed pattern; stop here.
        return errors;
    }

    match (&cfg.health_check_host, has_active_health_check) {
        (None, true) => errors.push(
            "upstreamAuthority.healthCheckHost is required when healthCheck.active is set: an active probe \
             has no request to render the authority from, and a wildcard certificate does not match its own \
             parent domain"
                .to_string(),
        ),
        (Some(h), _) if !host_matches_pattern(h, &cfg.pattern) => errors.push(format!(
            "upstreamAuthority.healthCheckHost '{h}' is outside pattern '{}'",
            cfg.pattern
        )),
        _ => {}
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(pattern: &str, template: &str, hc: Option<&str>) -> UpstreamAuthorityConfig {
        UpstreamAuthorityConfig {
            pattern: pattern.to_string(),
            template: template.to_string(),
            health_check_host: hc.map(|s| s.to_string()),
        }
    }

    #[test]
    fn pattern_suffix_extracts_tail() {
        assert_eq!(pattern_suffix("*.sandbox.internal").unwrap(), ".sandbox.internal");
        assert!(pattern_suffix("sandbox.internal").is_err(), "no wildcard label");
        assert!(pattern_suffix("*.*.internal").is_err(), "two wildcards");
        assert!(pattern_suffix("*").is_err(), "wildcard with no suffix");
    }

    #[test]
    fn pattern_suffix_rejects_a_malformed_suffix() {
        // The suffix is spliced verbatim into the upstream Host header and TLS
        // SNI, so every dot-separated label of it must pass the same rules a
        // rendered label does.
        for bad in [
            "*.a\r\nX-Injected: 1",
            "*.evil.com/x",
            "*.sandbox.internal ", // trailing space
            "*..b",                // doubled dot -> empty label
            "*.a.b.",              // trailing dot -> trailing empty label
            "*.a${ctx:t}",         // template syntax leaking into the pattern
        ] {
            assert!(pattern_suffix(bad).is_err(), "must reject pattern {bad:?}");
        }
    }

    #[test]
    fn pattern_suffix_rejects_an_all_numeric_final_label() {
        // `*.0.0.1` + a request-controlled label that renders "127" would
        // compose to "127.0.0.1", a valid IPv4 literal. Reject at the
        // pattern's final suffix label so this can never compile.
        for bad in ["*.0.0.1", "*.1.2.3"] {
            let err = pattern_suffix(bad).expect_err(&format!("must reject {bad:?}"));
            assert!(
                err.contains("all-numeric"),
                "error must mention the numeric-label reason, got: {err}"
            );
        }

        // A normal DNS suffix, and labels that merely CONTAIN digits
        // (but are not entirely numeric), must still be accepted.
        assert!(pattern_suffix("*.sandbox.internal").is_ok());
        assert!(pattern_suffix("*.a1").is_ok(), "label containing a digit is fine");
        assert!(pattern_suffix("*.v2").is_ok(), "label containing a digit is fine");

        // The same rejection must flow through compile() and
        // validate_upstream_authority(), not just pattern_suffix() directly.
        assert!(
            CompiledUpstreamAuthority::compile(&cfg("*.0.0.1", "${ctx:tenant}.0.0.1", None)).is_err(),
            "compile must reject an all-numeric-final-label pattern"
        );
        let errs = validate_upstream_authority(&cfg("*.0.0.1", "${ctx:tenant}.0.0.1", None), false);
        assert_eq!(errs.len(), 1);
        assert!(
            errs[0].contains("all-numeric"),
            "validate_upstream_authority must surface the numeric-label reason, got: {}",
            errs[0]
        );
    }

    #[test]
    fn host_matches_pattern_accepts_exactly_one_label() {
        let p = "*.sandbox.internal";
        assert!(host_matches_pattern("a.sandbox.internal", p));
        assert!(host_matches_pattern("A.Sandbox.Internal", p), "case-insensitive");
        assert!(!host_matches_pattern("a.b.sandbox.internal", p), "two labels");
        assert!(!host_matches_pattern("sandbox.internal", p), "bare parent");
        assert!(!host_matches_pattern("a.sandbox.evil.com", p));
        assert!(!host_matches_pattern("", p));
    }

    #[test]
    fn validate_label_rejects_every_escape_character() {
        assert_eq!(validate_label("Tenant-A").unwrap(), "tenant-a");
        assert!(validate_label("").is_err(), "empty render");
        assert!(validate_label(&"x".repeat(64)).is_err(), "over 63 bytes");
        for bad in [
            "a.b",
            "a/b",
            "a?b",
            "a#b",
            "a@b",
            "a:b",
            "a\rb",
            "a\nb",
            "a b",
            "a_b",
            // Pin the non-ASCII, NUL, tab, and other escape-shaped rejections so a
            // `to_lowercase()` / `is_alphanumeric()` "cleanup" (which would accept
            // these) cannot slip through with this test still green.
            "tenant-é",
            "a\0b",
            "a\tb",
            "a%b",
            "a$b",
        ] {
            assert!(validate_label(bad).is_err(), "must reject {bad:?}");
        }
        assert!(validate_label("-a").is_err(), "leading hyphen");
        assert!(validate_label("a-").is_err(), "trailing hyphen");

        // Note on byte length vs. char count: this does NOT distinguish a
        // byte-counting `len()` bound from a char-counting `chars().count()`
        // bound. 32 copies of the 2-byte 'é' fail the CHARSET check first
        // (its bytes are all >= 0x80), so the length check is never reached
        // either way. The distinction is provably unobservable: any input
        // that survives the charset check is pure ASCII, and for ASCII text
        // `len()` (bytes) and `chars().count()` are always equal — so the
        // byte bound and the char bound agree on every input that could
        // otherwise pass.
        let multi_byte = "é".repeat(32);
        assert!(multi_byte.chars().count() < 63, "sanity: char count stays under 63");
        assert!(multi_byte.len() > 63, "sanity: byte length exceeds 63");
        assert!(validate_label(&multi_byte).is_err(), "must reject on charset");
    }

    #[test]
    fn compile_accepts_matching_suffix_and_rejects_the_rest() {
        // Accepted: literal tail equals the pattern suffix, label slot has no dot.
        let ok = CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "${ctx:tenant}.sandbox.internal", None))
            .expect("must compile");
        assert_eq!(ok.suffix, ".sandbox.internal");
        assert_eq!(ok.compose("tenant-a"), "tenant-a.sandbox.internal");

        // Accepted: literals may mix into the label slot.
        assert!(
            CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "t-${ctx:id}.sandbox.internal", None))
                .is_ok(),
            "literal prefix inside the label slot is fine"
        );

        // Rejected: suffix mismatch.
        assert!(
            CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "${ctx:tenant}.sandbox.evil.com", None))
                .is_err()
        );

        // Rejected: no literal suffix at all (this is the `auto_sni` shape, spec §8.2).
        assert!(CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "${header:Host}", None)).is_err());

        // Rejected: a literal dot inside the label slot would produce two labels.
        assert!(CompiledUpstreamAuthority::compile(&cfg(
            "*.sandbox.internal",
            "${ctx:tenant}.extra.sandbox.internal",
            None
        ))
        .is_err());
    }

    #[test]
    fn compile_rejects_a_label_slot_doomed_to_start_or_end_with_a_hyphen() {
        // A rendered label can never start or end with '-' (`validate_label`
        // rejects it on every request), so a template whose first or last
        // segment is a literal starting or ending with '-' is statically
        // doomed to fail-closed on every single request. Admission must
        // catch this, not just the charset.
        assert!(
            CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "-${ctx:t}.sandbox.internal", None)).is_err(),
            "leading literal hyphen must be rejected at compile time"
        );
        assert!(
            CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "${ctx:t}-.sandbox.internal", None)).is_err(),
            "trailing literal hyphen must be rejected at compile time"
        );

        // An interior '-' remains legal.
        assert!(
            CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "t-${ctx:id}-a.sandbox.internal", None))
                .is_ok(),
            "an interior literal hyphen is fine"
        );
    }

    #[test]
    fn compile_and_host_matches_pattern_agree_on_case() {
        // `compile` and `host_matches_pattern` must agree on case: DNS is
        // case-insensitive, so a mixed-case pattern must compile, a
        // mixed-case template must compile against it, and `compose` must
        // always emit a lowercase host regardless of the source casing.
        let c = CompiledUpstreamAuthority::compile(&cfg("*.Sandbox.Internal", "${ctx:tenant}.Sandbox.INTERNAL", None))
            .expect("mixed-case pattern and template must both normalize and compile");
        assert_eq!(c.suffix, ".sandbox.internal", "stored suffix must be lowercase");

        let host = c.compose(&validate_label("Tenant-A").unwrap());
        assert_eq!(
            host, "tenant-a.sandbox.internal",
            "compose must always emit a lowercase host"
        );
        assert!(host_matches_pattern(&host, "*.Sandbox.Internal"));

        // What `compile` accepts and what `host_matches_pattern` accepts for
        // the same pattern must not disagree: a case-varied health check
        // host is in-pattern under both.
        assert!(host_matches_pattern("probe.Sandbox.Internal", "*.sandbox.internal"));
    }

    #[test]
    fn compile_normalizes_only_the_suffix_cut_not_the_template_interior() {
        // The tail matches case-insensitively, but the template's interior
        // (variable names, ctx/query/cookie keys) must survive verbatim —
        // those lookups are exact-match at render time.
        let c = CompiledUpstreamAuthority::compile(&cfg("*.Sandbox.Internal", "${ctx:Tenant}.Sandbox.INTERNAL", None))
            .unwrap();
        assert!(
            matches!(&c.label_template.segments()[0],
                TemplateSegment::Variable { var_name: Some(n), .. } if n == "Tenant"),
            "ctx var name is case-sensitive and must not be lowercased"
        );

        // camelCase variable types (`matchedHttpRoute`, `requestPath`, ...)
        // must still parse: lowercasing the template would turn this into
        // the unrecognized type "matchedhttproute".
        assert!(CompiledUpstreamAuthority::compile(&cfg(
            "*.sandbox.internal",
            "${matchedHttpRoute:name}.sandbox.internal",
            None
        ))
        .is_ok());
    }

    #[test]
    fn compose_can_never_escape_the_pattern() {
        let c = CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "${ctx:tenant}.sandbox.internal", None))
            .unwrap();

        // Adversarial half: nothing that fails `validate_label` may ever reach
        // `compose` — this is the actual security property, not a happy-path
        // round-trip. If `validate_label` ever started accepting a literal '.'
        // (or any other escape character), this is what would catch it, since
        // `compose` itself does no validation of its input.
        for rejected in ["evil.com", "a/b", "a@evil.com", "a:8080", "a\r\nX: y", "tenant-é"] {
            assert!(
                validate_label(rejected).is_err(),
                "must reject {rejected:?} before it can ever reach compose"
            );
        }

        // Happy half: for accepted labels, the composed host's label count
        // is fixed by the pattern's suffix and cannot grow — a validated
        // label is proven to contain no '.', so it can only ever contribute
        // exactly one label.
        let suffix = ".sandbox.internal";
        let expected_labels = suffix.matches('.').count() + 1;
        for label in ["a", "tenant-9", &"z".repeat(63)] {
            let host = c.compose(&validate_label(label).unwrap());
            assert!(host_matches_pattern(&host, "*.sandbox.internal"), "{host}");
            assert_eq!(
                host.matches('.').count() + 1,
                expected_labels,
                "label count must be fixed by the pattern, got host {host:?}"
            );
        }
    }

    #[test]
    fn compile_rejects_a_variable_straddling_the_pattern_suffix_cut() {
        // A template whose literal tail equals the pattern suffix but whose
        // `${...}` variable straddles the cut must be rejected. This
        // currently passes only because `CompiledTemplate::compile`
        // (in `plugin_configs/ctx_set.rs`) errors on the unclosed `${` left
        // behind once `strip_suffix` cuts the suffix out of the variable
        // expression — that is load-bearing behavior in a different module,
        // recorded here so a change to `CompiledTemplate::compile` that
        // silently tolerates an unclosed `${` cannot regress this guarantee
        // unnoticed.
        assert!(
            CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "${ctx:t.sandbox.internal", None)).is_err()
        );
    }

    #[test]
    fn validate_requires_health_check_host_only_with_active_hc() {
        let no_hc = cfg("*.sandbox.internal", "${ctx:t}.sandbox.internal", None);
        assert!(validate_upstream_authority(&no_hc, false).is_empty());

        let errs = validate_upstream_authority(&no_hc, true);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("healthCheckHost"), "got: {}", errs[0]);

        let good = cfg(
            "*.sandbox.internal",
            "${ctx:t}.sandbox.internal",
            Some("probe.sandbox.internal"),
        );
        assert!(validate_upstream_authority(&good, true).is_empty());

        let outside = cfg(
            "*.sandbox.internal",
            "${ctx:t}.sandbox.internal",
            Some("probe.other.com"),
        );
        let errs = validate_upstream_authority(&outside, true);
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("healthCheckHost"), "got: {}", errs[0]);
    }
}
