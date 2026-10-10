use super::*;
#[test]
fn actual_account_and_eab_fields_are_distinct_strict_and_api_bound() {
    let owner: EdgionAcme = serde_json::from_value(serde_json::json!({"metadata":{"name":"issuer"},"spec":{"email":"ops@example.com","domains":["example.com"],"privateKeySecretRef":{"name":"account"},"externalAccountBinding":{"keyId":"id","keySecretRef":{"name":"eab"}},"challenge":{"type":"http-01","gatewayRef":{"name":"edge"}},"storage":{"secretName":"cert"}}})).unwrap();
    let mut secret = Secret {
        metadata: kube::api::ObjectMeta {
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
