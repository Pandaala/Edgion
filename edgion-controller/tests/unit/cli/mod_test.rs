use super::*;

fn security(yaml: &str) -> ConfSyncSecurityConfig {
    serde_yaml::from_str(yaml).unwrap()
}

fn prepare_without_rotation(config: &ConfSyncSecurityConfig) -> Result<ConfSyncTransport> {
    ConfSyncTransport::prepare(config, |_, _, _| panic!("unexpected rotation initialization"))
}

#[tokio::test]
async fn selected_plaintext_skips_ca_and_identity_even_with_invalid_rotation() {
    for auto_ca in [false, true] {
        let config = security(&format!(
            "auto_ca: {auto_ca}\nactive: emergency\ncerts:\n  - name: emergency\n    skip_tls: true\nrotation:\n  policy: scheduled\n  schedule: invalid-cron\n"
        ));
        let transport = prepare_without_rotation(&config).unwrap();
        assert!(matches!(transport, ConfSyncTransport::Plaintext));
        assert!(transport
            .build_identity(CancellationToken::new())
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn plaintext_never_reads_authored_certificate_paths() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.pem").display().to_string();
    let mut config = security("auto_ca: true\ncerts:\n  - skip_tls: true\n");
    config.certs[0].cert = missing.clone();
    config.certs[0].key = missing.clone();
    config.certs[0].ca = missing;
    assert!(prepare_without_rotation(&config)
        .unwrap()
        .build_identity(CancellationToken::new())
        .await
        .unwrap()
        .is_none());
}

#[test]
fn bare_auto_ca_initializes_the_parameters_carried_to_provisioning() {
    let config = security("auto_ca: true\nrotation:\n  policy: manual\n");
    let mut initialized = Vec::new();
    let transport = ConfSyncTransport::prepare(&config, |ns, sans, rotation| {
        initialized.push((ns, sans, serde_json::to_value(rotation).unwrap()));
    })
    .unwrap();
    let ConfSyncTransport::AutoCa {
        namespace,
        sans,
        rotation,
    } = transport
    else {
        panic!("bare auto-CA must provision an identity");
    };
    assert_eq!(
        initialized,
        vec![(namespace.clone(), sans.clone(), serde_json::to_value(rotation).unwrap())]
    );
    assert_eq!(sans, crate::services::conf_sync_ca::server_sans_for(&namespace));
}

#[test]
fn inactive_bypass_never_disables_selected_tls_or_auto_ca() {
    for auto_ca in [false, true] {
        let config = security(&format!(
            "auto_ca: {auto_ca}\nactive: primary\ncerts:\n  - name: emergency\n    skip_tls: true\n  - name: primary\n    cert: primary.crt\n    key: primary.key\n    ca: root.crt\n"
        ));
        let mut initialized = 0;
        let transport = ConfSyncTransport::prepare(&config, |_, _, _| initialized += 1).unwrap();
        assert_eq!(initialized, usize::from(auto_ca));
        if auto_ca {
            assert!(matches!(transport, ConfSyncTransport::AutoCa { .. }));
        } else {
            let ConfSyncTransport::FileTls(tls) = transport else {
                panic!("selected file TLS must be retained")
            };
            assert_eq!(tls.name.as_deref(), Some("primary"));
            assert_eq!(tls.cert, "primary.crt");
        }
    }
}

#[test]
fn invalid_selection_and_rotation_fail_before_initialization() {
    for yaml in [
        "{}",
        "auto_ca: true\nactive: missing",
        "auto_ca: true\nactive: missing\ncerts:\n  - name: emergency\n    skip_tls: true",
        "auto_ca: true\ncerts:\n  - name: first\n    skip_tls: true\n  - name: second\n    skip_tls: true",
        "auto_ca: true\ncerts:\n  - name: duplicate\n    skip_tls: true\n  - name: duplicate\n    skip_tls: true",
        "auto_ca: true\ncerts:\n  - name: primary",
        "auto_ca: true\nrotation:\n  policy: scheduled\n  schedule: invalid-cron",
    ] {
        assert!(
            prepare_without_rotation(&security(yaml)).is_err(),
            "accepted invalid config: {yaml}"
        );
    }
}

#[tokio::test]
async fn file_tls_missing_material_fails_without_plaintext_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.pem").display().to_string();
    let mut config = security("certs:\n  - cert: unused\n    key: unused\n    ca: unused\nrotation:\n  policy: scheduled\n  schedule: invalid-cron\n");
    config.certs[0].cert = missing.clone();
    config.certs[0].key = missing.clone();
    config.certs[0].ca = missing;
    let transport = prepare_without_rotation(&config).unwrap();
    assert!(matches!(transport, ConfSyncTransport::FileTls(_)));
    assert!(transport.build_identity(CancellationToken::new()).await.is_err());
}
