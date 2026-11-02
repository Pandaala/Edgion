use super::*;
#[test]
fn actual_account_and_eab_fields_are_distinct_strict_and_api_bound() {
    let owner: EdgionAcme = serde_json::from_value(serde_json::json!({"metadata":{"name":"issuer"},"spec":{"email":"ops@example.com","domains":["example.com"],"privateKeySecretRef":{"name":"account"},"externalAccountBinding":{"keyId":"id","keySecretRef":{"name":"eab"}},"challenge":{"type":"http-01","gatewayRef":{"name":"edge"}},"storage":{"secretName":"cert"}}})).unwrap();
    let mut secret = Secret {
        metadata: kube::core::ObjectMeta {
            name: Some("account".into()),
            namespace: Some("default".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let refs = AcmeRefs::from_acme(&owner);
    assert!(refs.account().validate_api_secret(&secret).is_ok());
    assert!(refs.eab().unwrap().validate_api_secret(&secret).is_err());
    assert!(refs.account().token().freeze().is_err());
    assert!(refs.eab().unwrap().token().freeze().is_err());
    secret.metadata.name = Some("eab".into());
    assert!(refs.eab().unwrap().validate_api_secret(&secret).is_ok());
    assert!(refs.account().validate_api_secret(&secret).is_err());
    secret.metadata.namespace = None;
    assert!(refs.eab().unwrap().validate_api_secret(&secret).is_err());
    for case in 0..7 {
        let mut invalid = owner.clone();
        match case {
            0 => invalid.metadata.name = None,
            1 => invalid.metadata.namespace = Some(String::new()),
            2 => invalid.spec.private_key_secret_ref.group = Some("core".into()),
            3 => invalid.spec.private_key_secret_ref.kind = Some("ConfigMap".into()),
            4 => invalid.spec.private_key_secret_ref.namespace = Some(String::new()),
            5 => invalid.spec.private_key_secret_ref.name = "bad/name".into(),
            _ => invalid.spec.private_key_secret_ref.kind = Some(String::new()),
        }
        assert!(
            AcmeRefs::from_acme(&invalid).account().token().validated().is_err(),
            "case {case}"
        );
    }
}

#[test]
fn actual_generated_destinations_validate_active_fields_and_api_metadata() {
    let owner: EdgionAcme = serde_json::from_value(serde_json::json!({"metadata":{"name":"issuer","namespace":"source-ns"},"spec":{"email":"ops@example.com","domains":["example.com"],"privateKeySecretRef":{"name":"account"},"challenge":{"type":"http-01","gatewayRef":{"name":"http-edge"}},"storage":{"secretName":"certificate","secretNamespace":"cert-ns"},"autoEdgionTls":{"name":"tls-output"}}})).unwrap();
    let refs = AcmeRefs::from_acme(&owner);
    let outputs = refs.outputs().unwrap();
    assert_eq!(outputs.certificate().unwrap().name(), "certificate");
    assert_eq!(outputs.certificate().unwrap().scope().namespace(), Some("cert-ns"));
    assert_eq!(outputs.tls().unwrap().unwrap().name(), "tls-output");
    assert_eq!(outputs.tls().unwrap().unwrap().scope().namespace(), Some("source-ns"));
    for tls in [false, true] {
        let target = if tls {
            outputs.tls().unwrap().unwrap()
        } else {
            outputs.certificate().unwrap()
        };
        for case in 0..5 {
            let mut metadata = kube::core::ObjectMeta {
                name: Some(target.name().into()),
                namespace: target.scope().namespace().map(str::to_string),
                ..Default::default()
            };
            match case {
                1 => metadata.name = Some("decoy".into()),
                2 => metadata.namespace = Some("other".into()),
                3 => metadata.name = None,
                4 => metadata.namespace = None,
                _ => {}
            }
            let result = if tls {
                outputs.validate_tls_metadata(&metadata)
            } else {
                outputs.validate_certificate_metadata(&metadata)
            };
            assert_eq!(result.is_ok(), case == 0, "tls={tls}, case={case}");
        }
    }
    for namespace in [None, Some("default".into())] {
        let mut defaulted = owner.clone();
        defaulted.metadata.namespace = namespace;
        defaulted.spec.storage.secret_namespace = None;
        defaulted.spec.auto_edgion_tls.name = None;
        let refs = AcmeRefs::from_acme(&defaulted);
        let outputs = refs.outputs().unwrap();
        assert_eq!(outputs.certificate().unwrap().scope().namespace(), Some("default"));
        assert_eq!(outputs.tls().unwrap().unwrap().name(), "acme-issuer");
    }
    for case in 0..9 {
        let mut invalid = owner.clone();
        match case {
            0 => invalid.metadata.name = None,
            1 => invalid.metadata.namespace = Some(String::new()),
            2 => invalid.spec.storage.secret_name.clear(),
            3 => invalid.spec.storage.secret_name = "bad/name".into(),
            4 => invalid.spec.storage.secret_namespace = Some(String::new()),
            5 => invalid.spec.auto_edgion_tls.name = Some(String::new()),
            6 => invalid.spec.auto_edgion_tls.name = Some("bad/name".into()),
            7 => {
                invalid.metadata.name =
                    Some(["a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(61)].join("."));
                invalid.spec.auto_edgion_tls.name = None;
            }
            _ => invalid.metadata.name = Some("bad/name".into()),
        }
        assert!(AcmeRefs::from_acme(&invalid).outputs().is_err(), "case {case}");
    }
    let mut disabled = owner;
    disabled.spec.auto_edgion_tls.enabled = false;
    disabled.spec.auto_edgion_tls.name = Some("bad/name".into());
    let refs = AcmeRefs::from_acme(&disabled);
    let outputs = refs.outputs().unwrap();
    assert!(outputs.tls().unwrap().is_none());
    assert!(outputs.validate_tls_metadata(&Default::default()).is_err());
}
