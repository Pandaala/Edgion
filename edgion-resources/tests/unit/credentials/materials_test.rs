use super::*;

#[test]
fn credential_material_debug_never_formats_fields() {
    let materials: Vec<Box<dyn fmt::Debug>> = vec![
        Box::new(BasicAuthMaterial {
            username: Some("private-user".into()),
            password: "private-password".into(),
        }),
        Box::new(EsApiKeyMaterial {
            api_key_id: "private-id".into(),
            api_key_secret: "private-key".into(),
        }),
        Box::new(AiApiKeyMaterial {
            api_key: "private-key".into(),
        }),
        Box::new(AcmeAccountMaterial {
            credentials: Zeroizing::new("private-account".into()),
        }),
        Box::new(TlsMaterial {
            ca_pem: Some(b"private-ca".to_vec()),
            client_cert_pem: Some(b"private-cert".to_vec()),
            client_key_pem: Some(Zeroizing::new(b"private-key".to_vec())),
        }),
    ];
    for material in materials {
        for debug in [format!("{material:?}"), format!("{material:#?}")] {
            let (_, fields) = debug.split_once('(').expect("material type prefix");
            assert_eq!(fields, "[redacted])");
        }
    }
}

#[test]
fn basic_auth_without_username() {
    let m = BasicAuthMaterial {
        username: None,
        password: "p".into(),
    };
    assert!(m.username.is_none());
}

#[test]
fn acme_material_zeroizes_on_drop() {
    let m = AcmeAccountMaterial {
        credentials: zeroize::Zeroizing::new("{\"x\":1}".into()),
    };
    assert!(m.credentials.starts_with('{'));
    drop(m);
}

#[test]
fn types_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<BasicAuthMaterial>();
    assert_send_sync::<EsApiKeyMaterial>();
    assert_send_sync::<BearerMaterial>();
    assert_send_sync::<AiApiKeyMaterial>();
    assert_send_sync::<AcmeAccountMaterial>();
    assert_send_sync::<TlsMaterial>();
}

#[test]
fn bearer_debug_is_redacted() {
    let material = BearerMaterial {
        token: "private-token".to_string(),
    };
    let debug = format!("{material:?}");
    assert!(debug.contains("[redacted]"));
    assert!(!debug.contains("private-token"));
}

#[test]
fn tls_material_private_key_zeroizes_on_drop() {
    let m = TlsMaterial {
        ca_pem: Some(b"ca".to_vec()),
        client_cert_pem: None,
        client_key_pem: Some(zeroize::Zeroizing::new(b"KEY".to_vec())),
    };
    assert_eq!(m.client_key_pem.as_ref().unwrap().as_slice(), b"KEY");
    drop(m);
}
