use super::*;

#[derive(Clone, Copy)]
enum ChainOrder {
    LeafFirst,
    RootFirst,
}

#[derive(Debug, PartialEq, Eq)]
struct ChainWalk {
    leaf_evaluations: usize,
    terminal_logs: usize,
    rejected: bool,
}

fn walk_verify_stages(
    cert_count: usize,
    order: ChainOrder,
    mut preverify_ok_at: impl FnMut(c_int) -> bool,
    context_present: bool,
    leaf_allowlist_ok: bool,
    stop_on_reject: bool,
) -> ChainWalk {
    assert!((1..=9).contains(&cert_count));
    let mut depths: Vec<c_int> = (0..cert_count as c_int).collect();
    if matches!(order, ChainOrder::RootFirst) {
        depths.reverse();
    }

    let audit = AtomicBool::new(false);
    let mut walk = ChainWalk {
        leaf_evaluations: 0,
        terminal_logs: 0,
        rejected: false,
    };

    for depth in depths {
        let stage = classify_verify_callback(preverify_ok_at(depth), depth, context_present);
        match stage {
            VerifyStage::RejectChain | VerifyStage::RejectMissingContext | VerifyStage::RejectInvalidDepth => {
                walk.rejected = true;
                if claim_terminal_audit(&audit) {
                    walk.terminal_logs += 1;
                }
                if stop_on_reject {
                    break;
                }
            }
            VerifyStage::AcceptIntermediate => {}
            VerifyStage::EvaluateLeafAllowlist => {
                walk.leaf_evaluations += 1;
                if !leaf_allowlist_ok {
                    walk.rejected = true;
                    if claim_terminal_audit(&audit) {
                        walk.terminal_logs += 1;
                    }
                    if stop_on_reject {
                        break;
                    }
                }
            }
        }
    }
    walk
}

#[test]
fn classify_rejects_failed_preverify_at_every_depth() {
    for depth in 0..=8 {
        assert_eq!(classify_verify_callback(false, depth, true), VerifyStage::RejectChain);
        assert_eq!(classify_verify_callback(false, depth, false), VerifyStage::RejectChain);
    }
}

#[test]
fn classify_accepts_intermediate_without_context() {
    for depth in 1..=8 {
        assert_eq!(
            classify_verify_callback(true, depth, false),
            VerifyStage::AcceptIntermediate
        );
        assert_eq!(
            classify_verify_callback(true, depth, true),
            VerifyStage::AcceptIntermediate
        );
    }
}

#[test]
fn classify_leaf_requires_context_and_evaluates_allowlist() {
    assert_eq!(
        classify_verify_callback(true, 0, true),
        VerifyStage::EvaluateLeafAllowlist
    );
    assert_eq!(
        classify_verify_callback(true, 0, false),
        VerifyStage::RejectMissingContext
    );
}

#[test]
fn classify_negative_depth_is_fail_closed() {
    assert_eq!(
        classify_verify_callback(true, -1, true),
        VerifyStage::RejectInvalidDepth
    );
}

#[test]
fn successful_chains_evaluate_leaf_once_for_one_through_nine_certs() {
    for cert_count in 1..=9 {
        for order in [ChainOrder::LeafFirst, ChainOrder::RootFirst] {
            let walk = walk_verify_stages(cert_count, order, |_| true, true, true, true);
            assert_eq!(
                walk,
                ChainWalk {
                    leaf_evaluations: 1,
                    terminal_logs: 0,
                    rejected: false,
                },
                "cert_count={cert_count}"
            );
        }
    }
}

#[test]
fn preverify_failure_rejects_immediately_without_leaf_work() {
    for cert_count in 1..=9 {
        for fail_depth in 0..cert_count as c_int {
            let walk = walk_verify_stages(
                cert_count,
                ChainOrder::RootFirst,
                |depth| depth != fail_depth,
                true,
                true,
                true,
            );
            assert!(walk.rejected, "fail_depth={fail_depth}");
            assert_eq!(walk.terminal_logs, 1, "fail_depth={fail_depth}");
            assert_eq!(walk.leaf_evaluations, 0, "fail_depth={fail_depth}");
        }
    }
}

#[test]
fn missing_leaf_context_is_fail_closed() {
    let walk = walk_verify_stages(3, ChainOrder::LeafFirst, |_| true, false, true, true);
    assert_eq!(
        walk,
        ChainWalk {
            leaf_evaluations: 0,
            terminal_logs: 1,
            rejected: true,
        }
    );
}

#[test]
fn intermediate_success_does_not_extract_or_log() {
    let walk = walk_verify_stages(4, ChainOrder::RootFirst, |_| true, true, true, true);
    assert_eq!(walk.leaf_evaluations, 1);
    assert_eq!(walk.terminal_logs, 0);
    assert!(!walk.rejected);
}

#[test]
fn allowlist_failure_emits_one_terminal_log() {
    for order in [ChainOrder::LeafFirst, ChainOrder::RootFirst] {
        let walk = walk_verify_stages(5, order, |_| true, true, false, false);
        assert_eq!(
            walk,
            ChainWalk {
                leaf_evaluations: 1,
                terminal_logs: 1,
                rejected: true,
            }
        );
    }
}

#[test]
fn terminal_audit_is_claimed_once_across_repeated_failures() {
    let flag = AtomicBool::new(false);
    assert!(claim_terminal_audit(&flag));
    assert!(!claim_terminal_audit(&flag));
    assert!(!claim_terminal_audit(&flag));
}

fn allowlist_certificate(cn: Option<&str>) -> pingora_core::tls::x509::X509 {
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(vec!["client.example".into()]).unwrap();
    params.distinguished_name = DistinguishedName::new();
    params.distinguished_name.push(DnType::OrganizationName, "example");
    if let Some(cn) = cn {
        params.distinguished_name.push(DnType::CommonName, cn);
    }
    params
        .subject_alt_names
        .push(SanType::URI("spiffe://example/client".try_into().unwrap()));
    let cert = params.self_signed(&key).unwrap();
    pingora_core::tls::x509::X509::from_der(cert.der()).unwrap()
}

fn allowlist_config(san: Option<&str>, cn: Option<&str>) -> ClientAuthConfig {
    use edgion_resources::resources::edgion_tls::{AllowedSan, AllowedSanType};
    let mut config: ClientAuthConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    config.allowed_sans = san.map(|value| vec![AllowedSan::exact(AllowedSanType::Dns, value)]);
    config.allowed_cns = cn.map(|value| vec![value.to_string()]);
    config
}

fn masked_fixture(value: &str) -> String {
    edgion_resources::sensitive::apply_sensitive(value, str::to_string)
}

#[test]
fn missing_certificate_rejects_even_without_nonempty_constraints() {
    for mut config in [
        allowlist_config(None, None),
        allowlist_config(Some("client.example"), Some("client-cn")),
    ] {
        assert_eq!(
            validate_certificate_whitelist(None, &config),
            Err("mTLS no client certificate".into())
        );
        config.allowed_sans = Some(vec![]);
        config.allowed_cns = Some(vec![]);
        assert_eq!(
            validate_certificate_whitelist(None, &config),
            Err("mTLS no client certificate".into())
        );
    }
}

#[test]
fn present_certificate_accepts_absent_empty_and_matching_constraints() {
    let cert = allowlist_certificate(Some("client-cn"));
    let mut empty = allowlist_config(None, None);
    empty.allowed_sans = Some(vec![]);
    empty.allowed_cns = Some(vec![]);
    for config in [
        allowlist_config(None, None),
        empty,
        allowlist_config(Some("client.example"), None),
        allowlist_config(None, Some("client-cn")),
        allowlist_config(Some("client.example"), Some("client-cn")),
    ] {
        assert_eq!(validate_certificate_whitelist(Some(&cert), &config), Ok(()));
    }
}

#[test]
fn san_failure_preserves_diagnostics_and_precedes_cn_failure() {
    let cert = allowlist_certificate(Some("client-cn"));
    let expected = format!(
        "mTLS SAN whitelist failed: subject={}, sans={:?}",
        masked_fixture("O=example, CN=client-cn"),
        [
            masked_fixture("client.example"),
            masked_fixture("uri:spiffe://example/client")
        ],
    );
    for cn in [None, Some("client-cn"), Some("other-cn")] {
        let config = allowlist_config(Some("other.example"), cn);
        assert_eq!(
            validate_certificate_whitelist(Some(&cert), &config),
            Err(expected.clone())
        );
    }
    // A URI must not satisfy a DNS allowlist entry with the same text.
    let config = allowlist_config(Some("spiffe://example/client"), None);
    assert_eq!(validate_certificate_whitelist(Some(&cert), &config), Err(expected));
}

#[test]
fn cn_failure_preserves_case_sensitive_matching_and_diagnostics() {
    let cert = allowlist_certificate(Some("client-cn"));
    let expected = format!(
        "mTLS CN whitelist failed: subject={}, cn={:?}",
        masked_fixture("O=example, CN=client-cn"),
        Some(masked_fixture("client-cn")),
    );
    for san in [None, Some("client.example")] {
        let config = allowlist_config(san, Some("CLIENT-CN"));
        assert_eq!(
            validate_certificate_whitelist(Some(&cert), &config),
            Err(expected.clone())
        );
    }
    let cert = allowlist_certificate(None);
    let config = allowlist_config(None, Some("client-cn"));
    assert_eq!(
        validate_certificate_whitelist(Some(&cert), &config),
        Err(format!(
            "mTLS CN whitelist failed: subject={}, cn=None",
            masked_fixture("O=example"),
        ))
    );
}
