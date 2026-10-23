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

    // The byte-length guard precedes charset validation, including for UTF-8.
    let multi_byte = "é".repeat(32);
    assert!(multi_byte.chars().count() < 63, "sanity: char count stays under 63");
    assert_eq!(validate_label(&multi_byte).unwrap_err(), "longer than 63 bytes");
}

#[test]
fn label_normalization_preserves_ascii_grammar_and_error_priority() {
    let alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    for byte in 0u8..=127 {
        let label = format!("a{}z", char::from(byte));
        let result = validate_label(&label);
        if alphabet.contains(char::from(byte)) || byte == b'-' {
            assert_eq!(result.unwrap(), label.to_ascii_lowercase());
        } else {
            assert_eq!(result.unwrap_err(), "must contain only [a-z0-9-]");
        }
    }
    assert_eq!(validate_label("").unwrap_err(), "empty");
    assert_eq!(validate_label("-a.").unwrap_err(), "must contain only [a-z0-9-]");
    assert_eq!(validate_label("-A").unwrap_err(), "must not start or end with '-'");
    assert_eq!(validate_label("A-").unwrap_err(), "must not start or end with '-'");
    assert_eq!(validate_label(&"A".repeat(63)).unwrap(), "a".repeat(63));
    assert_eq!(
        validate_label(&format!("{}.", "A".repeat(63))).unwrap_err(),
        "longer than 63 bytes"
    );
    assert_eq!(validate_label("tenant-42").unwrap(), "tenant-42");
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
        CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "t-${ctx:id}.sandbox.internal", None)).is_ok(),
        "literal prefix inside the label slot is fine"
    );

    // Rejected: suffix mismatch.
    assert!(
        CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "${ctx:tenant}.sandbox.evil.com", None)).is_err()
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
fn compile_enforces_trusted_variables_without_secrets() {
    for expression in [
        "${header:X-Tenant}",
        "${query:tenant}",
        "${cookie:tenant}",
        "${path}",
        "${uri}",
        "${requestPath}",
        "${requestUri}",
        "${method}",
        "${secretRef:ns/token:key}",
        "${secretRef}",
    ] {
        let config = cfg("*.sandbox.internal", &format!("{expression}.sandbox.internal"), None);
        assert!(CompiledUpstreamAuthority::compile(&config).is_err(), "{expression}");
        assert_eq!(validate_upstream_authority(&config, false).len(), 1, "{expression}");
    }
    for expression in [
        "${ctx:tenant}",
        "${remoteIp}",
        "${matchedHttpRoute:name}",
        "${matchedGrpcRoute:namespace}",
    ] {
        let config = cfg("*.sandbox.internal", &format!("{expression}.sandbox.internal"), None);
        assert!(CompiledUpstreamAuthority::compile(&config).is_ok(), "{expression}");
        assert!(validate_upstream_authority(&config, false).is_empty(), "{expression}");
    }
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
        CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "t-${ctx:id}-a.sandbox.internal", None)).is_ok(),
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
    let c =
        CompiledUpstreamAuthority::compile(&cfg("*.Sandbox.Internal", "${ctx:Tenant}.Sandbox.INTERNAL", None)).unwrap();
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
    let c =
        CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "${ctx:tenant}.sandbox.internal", None)).unwrap();

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
    assert!(CompiledUpstreamAuthority::compile(&cfg("*.sandbox.internal", "${ctx:t.sandbox.internal", None)).is_err());
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
