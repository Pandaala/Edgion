use super::*;
use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
use edgion_resources::resources::common::OutboundTlsValidation;

fn san_only() -> OutboundTlsValidation {
    OutboundTlsValidation {
        subject_alt_names: Some(vec![SubjectAltName {
            san_type: SubjectAltNameType::Hostname,
            hostname: Some("idp.example.com".to_string()),
            uri: None,
        }]),
        ..Default::default()
    }
}

fn hostname_only() -> OutboundTlsValidation {
    OutboundTlsValidation {
        hostname: Some("idp.example.com".to_string()),
        ..Default::default()
    }
}

// ── predicates ────────────────────────────────────────────────────────────

#[test]
fn predicates_on_empty_and_none() {
    assert!(!subject_alt_names_requested(None));
    assert!(!hostname_requested(None));
    let empty = OutboundTlsValidation::default();
    assert!(!subject_alt_names_requested(Some(&empty)));
    assert!(!hostname_requested(Some(&empty)));
}

#[test]
fn only_ca_refs_is_neither_san_nor_hostname() {
    use edgion_resources::resources::backend_tls_policy::BackendTLSPolicyCACertificateRef;
    let v = OutboundTlsValidation {
        ca_certificate_refs: Some(vec![BackendTLSPolicyCACertificateRef {
            kind: "Secret".to_string(),
            name: "my-ca".to_string(),
            namespace: None,
            ..Default::default()
        }]),
        ..Default::default()
    };
    assert!(!subject_alt_names_requested(Some(&v)));
    assert!(!hostname_requested(Some(&v)));
}

#[test]
fn subject_alt_names_and_hostname_predicates_are_independent() {
    assert!(subject_alt_names_requested(Some(&san_only())));
    assert!(!hostname_requested(Some(&san_only())));
    assert!(hostname_requested(Some(&hostname_only())));
    assert!(!subject_alt_names_requested(Some(&hostname_only())));
}

#[test]
fn empty_subject_alt_names_vec_is_not_requested() {
    let v = OutboundTlsValidation {
        subject_alt_names: Some(vec![]),
        ..Default::default()
    };
    assert!(!subject_alt_names_requested(Some(&v)));
}

// ── check_validation_block: the SAN gate is per-provider, hostname is not ──

#[test]
fn san_rejected_when_provider_gate_off() {
    let errs = check_validation_block(Some(&san_only()), "ExtProc", false);
    assert_eq!(errs.len(), 1);
    assert!(errs[0].contains("subjectAltNames"));
}

#[test]
fn san_allowed_when_provider_gate_on() {
    // oidc/ldap-style: subjectAltNames is enforced, so it passes the guard.
    assert!(check_validation_block(Some(&san_only()), "OpenidConnect", true).is_empty());
}

#[test]
fn hostname_rejected_even_when_san_gate_on() {
    // The key fix: a provider that enforces subjectAltNames (gate on) must
    // still reject hostname, because SNI override is implemented nowhere.
    let errs = check_validation_block(Some(&hostname_only()), "OpenidConnect", true);
    assert_eq!(errs.len(), 1);
    assert!(errs[0].contains("hostname"));
}

#[test]
fn both_fields_yield_two_errors_when_both_unsupported() {
    let v = OutboundTlsValidation {
        hostname: Some("idp.example.com".to_string()),
        subject_alt_names: san_only().subject_alt_names,
        ..Default::default()
    };
    let errs = check_validation_block(Some(&v), "ExtProc", false);
    assert_eq!(
        errs.len(),
        2,
        "expected both subjectAltNames and hostname errors, got: {errs:?}"
    );
}

#[test]
fn enabled_plugin_source_is_validated_for_san_guard() {
    use edgion_resources::resources::edgion_plugins::{
        EdgionPlugin, EdgionPluginsSpec, ExtProcConfig, RequestFilterEntry,
    };

    let mut config = ExtProcConfig::default();
    config.grpc_service.tls.outbound.validation = Some(hostname_only());
    let entry = RequestFilterEntry::new(EdgionPlugin::ExtProc(config));
    let plugins = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![entry]),
            ..Default::default()
        },
        status: None,
    };

    assert!(!check_plugins_san(&plugins).is_empty());
}

#[test]
fn guardrail_allows_enforced_san_but_rejects_unsupported_hostname_override() {
    use edgion_resources::resources::common::TlsConfig;
    use edgion_resources::resources::edgion_plugins::{
        EdgionPlugin, EdgionPluginsSpec, GuardrailConfig, RequestFilterEntry,
    };

    let mut san_config = GuardrailConfig::default();
    san_config.conn.tls = Some(TlsConfig::default());
    san_config.conn.tls.as_mut().unwrap().outbound.validation = Some(san_only());
    let san_plugins = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::Guardrail(san_config))]),
            ..Default::default()
        },
        status: None,
    };
    assert!(check_plugins_san(&san_plugins).is_empty());

    let mut hostname_config = GuardrailConfig::default();
    hostname_config.conn.tls = Some(TlsConfig::default());
    hostname_config.conn.tls.as_mut().unwrap().outbound.validation = Some(hostname_only());
    let hostname_plugins = EdgionPlugins {
        metadata: Default::default(),
        spec: EdgionPluginsSpec {
            current_status: None,
            request_plugins: Some(vec![RequestFilterEntry::new(EdgionPlugin::Guardrail(hostname_config))]),
            ..Default::default()
        },
        status: None,
    };
    let errors = check_plugins_san(&hostname_plugins);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("hostname"));
}
