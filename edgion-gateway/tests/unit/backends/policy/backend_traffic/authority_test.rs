use super::*;
use crate::plugins::runtime::traits::session::MockPluginSession;
use edgion_resources::resources::upstream_authority::UpstreamAuthorityConfig;

fn compiled(template: &str) -> CompiledUpstreamAuthority {
    CompiledUpstreamAuthority::compile(&UpstreamAuthorityConfig {
        pattern: "*.sandbox.internal".to_string(),
        template: template.to_string(),
        health_check_host: None,
    })
    .expect("compiles")
}

/// A `MockPluginSession` whose `get_ctx_var("tenant")` returns `value`.
fn session_with_tenant(value: &str) -> MockPluginSession {
    let mut s = MockPluginSession::new();
    let value = value.to_string();
    s.expect_get_ctx_var()
        .withf(|n| n == "tenant")
        .return_const(Some(value));
    s
}

#[test]
fn renders_and_composes_a_valid_label() {
    let c = compiled("${ctx:tenant}.sandbox.internal");
    let s = session_with_tenant("tenant-a");
    assert_eq!(render_authority(&c, &s).unwrap(), "tenant-a.sandbox.internal");
}

#[test]
fn lowercases_the_rendered_label() {
    let c = compiled("${ctx:tenant}.sandbox.internal");
    let s = session_with_tenant("Tenant-A");
    assert_eq!(render_authority(&c, &s).unwrap(), "tenant-a.sandbox.internal");
}

#[test]
fn authority_budget_counts_combined_segments_before_validation() {
    let c = compiled("prefix-${ctx:tenant}.sandbox.internal");
    assert_eq!(
        render_authority(&c, &session_with_tenant(&"a".repeat(56))).unwrap(),
        format!("prefix-{}.sandbox.internal", "a".repeat(56))
    );
    for length in [57, 100_000] {
        assert_eq!(
            render_authority(&c, &session_with_tenant(&"a".repeat(length))),
            Err(AuthorityError::InvalidLabel)
        );
    }
}

#[test]
fn unset_ctx_var_is_render_empty_not_an_empty_label() {
    let c = compiled("${ctx:tenant}.sandbox.internal");
    let mut s = MockPluginSession::new();
    s.expect_get_ctx_var().withf(|n| n == "tenant").return_const(None);
    assert!(matches!(render_authority(&c, &s), Err(AuthorityError::RenderEmpty)));
}

#[test]
fn injected_separators_are_rejected_not_escaped() {
    let c = compiled("${ctx:tenant}.sandbox.internal");
    for bad in ["evil.com", "a/b", "a@evil.com", "a:8080", "a\r\nX: y"] {
        let s = session_with_tenant(bad);
        assert!(
            matches!(render_authority(&c, &s), Err(AuthorityError::InvalidLabel)),
            "must reject {bad:?}"
        );
    }
}

#[test]
fn render_authority_rejects_a_composed_ip_literal() {
    // Admission (`pattern_suffix`) already rejects any pattern whose final
    // suffix label is all-numeric, so a compiled pattern can never reach
    // this state in practice. This test pins the layer-2 belt-and-suspenders
    // guard directly by constructing a `CompiledUpstreamAuthority` whose
    // suffix bypasses `compile()`'s validation, so a request-controlled
    // label of "127" composes to the IPv4 literal "127.0.0.1".
    use edgion_resources::resources::common::request_template::CompiledTemplate;

    let c = CompiledUpstreamAuthority {
        label_template: CompiledTemplate::compile("${ctx:tenant}").expect("compiles"),
        suffix: ".0.0.1".to_string(),
        health_check_host: None,
    };
    let s = session_with_tenant("127");
    assert!(
        matches!(render_authority(&c, &s), Err(AuthorityError::InvalidLabel)),
        "a composed IPv4 literal must be rejected even if it bypassed admission"
    );
}

#[test]
fn authority_reserves_short_hosts_and_grows_without_relaxing_label_budget() {
    let c = compiled("${ctx:tenant}.sandbox.internal");
    for label in ["a", "A"] {
        let host = render_authority(&c, &session_with_tenant(label)).unwrap();
        assert_eq!(host, "a.sandbox.internal");
        assert!(host.capacity() >= 64);
    }
    for label in ["a".repeat(63), "A".repeat(63)] {
        let host = render_authority(&c, &session_with_tenant(&label)).unwrap();
        assert_eq!(host, format!("{}.sandbox.internal", "a".repeat(63)));
        assert!(host.len() > 64);
    }
    assert_eq!(
        render_authority(&c, &session_with_tenant(&"a".repeat(64))),
        Err(AuthorityError::InvalidLabel)
    );
}
