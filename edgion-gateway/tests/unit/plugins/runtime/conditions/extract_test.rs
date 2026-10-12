use super::*;
use crate::ctx::ClientCertInfo;
use crate::plugins::runtime::traits::session::MockPluginSession;

fn cert() -> &'static ClientCertInfo {
    static CERT: std::sync::OnceLock<ClientCertInfo> = std::sync::OnceLock::new();
    CERT.get_or_init(|| ClientCertInfo {
        subject: "CN=alice, O=acme".to_string(),
        sans: vec![
            "alice.example.com".to_string(),
            "IP:192.0.2.1".to_string(),
            "uri:spiffe://edgion.io/ns/foo".to_string(),
            "email:alice@example.com".to_string(),
            "otherName:1.3.6.1.4.1.311.20.2.3=alice@corp.example.com".to_string(),
        ],
        cn: Some("alice".to_string()),
        fingerprint: "ab:cd".to_string(),
    })
}

#[test]
fn jwt_claim_nested_array_any_match() {
    let mut s = MockPluginSession::new();
    s.expect_get_ctx_var().returning(|k| {
        if k == "jwt_claims" {
            Some(r#"{"sub":"u1","realm_access":{"roles":["admin","dev"]}}"#.to_string())
        } else {
            None
        }
    });
    let out = extract_jwt_claim_values(&s, "realm_access.roles");
    assert_eq!(out, vec!["admin".to_string(), "dev".to_string()]);
}

#[test]
fn jwt_claim_scalar_and_number_and_bool() {
    let mut s = MockPluginSession::new();
    s.expect_get_ctx_var().returning(|k| {
        if k == "jwt_claims" {
            Some(r#"{"sub":"u1","age":42,"active":true}"#.to_string())
        } else {
            None
        }
    });
    assert_eq!(extract_jwt_claim_values(&s, "sub"), vec!["u1".to_string()]);
    assert_eq!(extract_jwt_claim_values(&s, "age"), vec!["42".to_string()]);
    assert_eq!(extract_jwt_claim_values(&s, "active"), vec!["true".to_string()]);
}

#[test]
fn jwt_claim_falls_back_to_oidc_claims() {
    let mut s = MockPluginSession::new();
    s.expect_get_ctx_var().returning(|k| match k {
        "jwt_claims" => None,
        "oidc_claims" => Some(r#"{"groups":["g1"]}"#.to_string()),
        _ => None,
    });
    assert_eq!(extract_jwt_claim_values(&s, "groups"), vec!["g1".to_string()]);
}

#[test]
fn jwt_claim_missing_branches() {
    let mut s = MockPluginSession::new();
    s.expect_get_ctx_var().returning(|_| None);
    assert!(extract_jwt_claim_values(&s, "sub").is_empty());

    let mut s2 = MockPluginSession::new();
    s2.expect_get_ctx_var().returning(|k| {
        if k == "jwt_claims" {
            Some(r#"{"realm_access":{"roles":["x"]}}"#.to_string())
        } else {
            None
        }
    });
    assert!(extract_jwt_claim_values(&s2, "realm_access").is_empty());
    assert!(extract_jwt_claim_values(&s2, "missing.path").is_empty());
}

#[test]
fn mtls_subject_and_cn() {
    let mut s = MockPluginSession::new();
    s.expect_client_cert_info().returning(|| Some(cert()));
    assert_eq!(
        extract_mtls_values(&s, MtlsField::Subject, None).collect::<Vec<_>>(),
        vec!["CN=alice, O=acme".to_string()]
    );
    assert_eq!(
        extract_mtls_values(&s, MtlsField::Cn, None).collect::<Vec<_>>(),
        vec!["alice".to_string()]
    );
}

#[test]
fn mtls_missing_when_no_cert() {
    let mut s = MockPluginSession::new();
    s.expect_client_cert_info().returning(|| None);
    assert!(extract_mtls_values(&s, MtlsField::Cn, None).next().is_none());
    assert!(extract_mtls_values(&s, MtlsField::San, None).next().is_none());
}

#[test]
fn mtls_san_untyped_returns_raw() {
    let mut s = MockPluginSession::new();
    s.expect_client_cert_info().returning(|| Some(cert()));
    let out = extract_mtls_values(&s, MtlsField::San, None).collect::<Vec<_>>();
    assert!(out.contains(&"uri:spiffe://edgion.io/ns/foo"));
    assert!(out.contains(&"alice.example.com"));
}

#[test]
fn mtls_san_typed_strips_prefix() {
    let mut s = MockPluginSession::new();
    s.expect_client_cert_info().returning(|| Some(cert()));
    assert_eq!(
        extract_mtls_values(&s, MtlsField::San, Some(&SanType::Uri)).collect::<Vec<_>>(),
        vec!["spiffe://edgion.io/ns/foo".to_string()]
    );
    assert_eq!(
        extract_mtls_values(&s, MtlsField::San, Some(&SanType::Dns)).collect::<Vec<_>>(),
        vec!["alice.example.com".to_string()]
    );
}

#[test]
fn mtls_candidates_borrow_selected_fields_and_strip_email() {
    let mut session = MockPluginSession::new();
    session.expect_client_cert_info().return_const(Some(cert()));
    let subject = extract_mtls_values(&session, MtlsField::Subject, None).next().unwrap();
    let cn = extract_mtls_values(&session, MtlsField::Cn, None).next().unwrap();
    let uri = extract_mtls_values(&session, MtlsField::San, Some(&SanType::Uri))
        .next()
        .unwrap();
    let email = extract_mtls_values(&session, MtlsField::San, Some(&SanType::Email))
        .next()
        .unwrap();
    assert_eq!(subject.as_ptr(), cert().subject.as_ptr());
    assert_eq!(cn.as_ptr(), cert().cn.as_deref().unwrap().as_ptr());
    assert_eq!(uri.as_ptr(), cert().sans[2][4..].as_ptr());
    assert_eq!(email, "alice@example.com");
    assert_eq!(email.as_ptr(), cert().sans[3][6..].as_ptr());
}

#[test]
fn mtls_missing_cn_and_typed_sans_stay_empty() {
    static CERT: std::sync::OnceLock<ClientCertInfo> = std::sync::OnceLock::new();
    let cert = CERT.get_or_init(|| ClientCertInfo {
        subject: String::new(),
        cn: None,
        sans: vec!["IP:192.0.2.1".to_string()],
        fingerprint: String::new(),
    });
    let mut session = MockPluginSession::new();
    session.expect_client_cert_info().return_const(Some(cert));
    assert_eq!(extract_mtls_values(&session, MtlsField::Subject, None).next(), Some(""));
    assert!(extract_mtls_values(&session, MtlsField::Cn, None).next().is_none());
    for kind in [SanType::Dns, SanType::Uri, SanType::Email] {
        assert!(extract_mtls_values(&session, MtlsField::San, Some(&kind))
            .next()
            .is_none());
    }
}
