fn make_self_signed_pem(cn: &str) -> String {
    use rcgen::{CertificateParams, KeyPair};
    let key = KeyPair::generate().expect("rcgen key generation failed");
    let params = CertificateParams::new(vec![cn.to_string()]).expect("rcgen params failed");
    params.self_signed(&key).expect("rcgen self-sign failed").pem()
}

#[test]
fn ca_bundle_loads_all_pem_blocks() {
    use super::add_ca_bundle_to_store;
    use pingora_core::tls::x509::store::X509StoreBuilder;
    use pingora_core::tls::x509::X509;

    // Realistic shape: a root + two intermediates concatenated into ca.crt.
    let bundle = format!(
        "{}{}{}",
        make_self_signed_pem("ca-root"),
        make_self_signed_pem("ca-intermediate-1"),
        make_self_signed_pem("ca-intermediate-2"),
    );

    // Regression guard: the old `X509::from_pem` path saw only the first
    // block; the helper must load all three.
    assert_eq!(X509::stack_from_pem(bundle.as_bytes()).unwrap().len(), 3);

    let mut builder = X509StoreBuilder::new().expect("store builder");
    let digest = add_ca_bundle_to_store(&mut builder, bundle.as_bytes()).expect("add bundle");
    // The bundle is canonicalised (sorted, deduplicated) before hashing.
    let mut ders: Vec<Vec<u8>> = X509::stack_from_pem(bundle.as_bytes())
        .unwrap()
        .iter()
        .map(|cert| cert.to_der().unwrap())
        .collect();
    ders.sort();
    ders.dedup();
    assert_eq!(ders.len(), 3);
    assert_eq!(
        digest,
        crate::runtime::matching::tls::der_list_digest(ders.iter().map(Vec::as_slice)),
        "all three CA certs must be loaded"
    );
}

#[test]
fn ca_bundle_rejects_empty_or_garbage() {
    use super::add_ca_bundle_to_store;
    use pingora_core::tls::x509::store::X509StoreBuilder;

    let mut builder = X509StoreBuilder::new().expect("store builder");
    assert!(add_ca_bundle_to_store(&mut builder, b"").is_err());
    assert!(add_ca_bundle_to_store(&mut builder, b"not a pem at all").is_err());
}

#[test]
fn direct_cold_compile_rejects_detached_server_and_keeps_terminate_ca_inactive() {
    use super::CompiledEdgionTls;
    use edgion_resources::EdgionTls;
    use std::sync::Arc;
    let pair = rcgen::generate_simple_self_signed(vec!["edge.example".into()]).unwrap();
    let mut tls: EdgionTls = serde_json::from_value(serde_json::json!({"metadata":{"name":"edge","namespace":"default"},"spec":{"hosts":["edge.example"],"secretRef":{"name":"server"},"clientAuth":{"mode":"Terminate","caSecretRef":{"group":"core","name":"ca"}}}})).unwrap();
    tls.spec.secret = Some(k8s_openapi::api::core::v1::Secret {
        metadata: kube::api::ObjectMeta {
            name: Some("server".into()),
            namespace: None,
            ..Default::default()
        },
        data: Some(std::collections::BTreeMap::from([
            ("tls.crt".into(), k8s_openapi::ByteString(pair.cert.pem().into_bytes())),
            (
                "tls.key".into(),
                k8s_openapi::ByteString(pair.signing_key.serialize_pem().into_bytes()),
            ),
        ])),
        ..Default::default()
    });
    let good = CompiledEdgionTls::compile(Arc::new(tls.clone()));
    assert!(good.identity.is_ok());
    assert!(good.client_ca_store.as_ref().unwrap().is_none());
    tls.spec.secret.as_mut().unwrap().metadata.name = Some("decoy".into());
    let bad = CompiledEdgionTls::compile(Arc::new(tls));
    assert!(bad.identity.is_err());
    assert!(bad.client_ca_store.as_ref().unwrap().is_none());
}

/// gw-infra-01: the EdgionTls session id context encodes exactly the effective
/// policy. Every authorization input flips it; inputs the runtime ignores
/// (Terminate settings, empty vs absent allow-lists) do not.
#[test]
fn edgion_tls_session_context_tracks_effective_client_auth_policy() {
    use super::{edgion_tls_session_context, AllowedSanMatch, AllowedSanType, ClientAuthMode, SessionContextId};
    use edgion_resources::resources::edgion_tls::ClientAuthConfig;
    use edgion_resources::EdgionTls;

    fn auth(tls: &mut EdgionTls) -> &mut ClientAuthConfig {
        tls.spec.client_auth.as_mut().unwrap()
    }

    let base: EdgionTls = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "edge", "namespace": "default"},
        "spec": {
            "hosts": ["edge.example"],
            "secretRef": {"name": "server"},
            "clientAuth": {
                "mode": "Mutual",
                "caSecretRef": {"name": "ca"},
                "verifyDepth": 2,
                "allowedSans": [{"type": "DNS", "value": "client.example.com"}],
                "allowedCns": ["client"]
            }
        }
    }))
    .unwrap();
    let (chain, other_chain, ca, other_ca) = ([1u8; 32], [2u8; 32], [3u8; 32], [4u8; 32]);
    let id = |tls: &EdgionTls| edgion_tls_session_context(tls, Some(&chain), Some(&ca));
    let edit = |change: &dyn Fn(&mut EdgionTls)| {
        let mut tls = base.clone();
        change(&mut tls);
        id(&tls)
    };
    let reference = id(&base);
    assert_eq!(reference, id(&base.clone()), "identical inputs must be stable");

    // Every effective input flips the id.
    let flips: Vec<(&str, SessionContextId)> = vec![
        (
            "chain",
            edgion_tls_session_context(&base, Some(&other_chain), Some(&ca)),
        ),
        ("chain missing", edgion_tls_session_context(&base, None, Some(&ca))),
        ("ca", edgion_tls_session_context(&base, Some(&chain), Some(&other_ca))),
        ("ca missing", edgion_tls_session_context(&base, Some(&chain), None)),
        ("name", edit(&|tls| tls.metadata.name = Some("other".into()))),
        ("namespace", edit(&|tls| tls.metadata.namespace = Some("other".into()))),
        ("optional", edit(&|tls| auth(tls).mode = ClientAuthMode::OptionalMutual)),
        ("terminate", edit(&|tls| auth(tls).mode = ClientAuthMode::Terminate)),
        ("depth", edit(&|tls| auth(tls).verify_depth = 3)),
        (
            "san value",
            edit(&|tls| auth(tls).allowed_sans.as_mut().unwrap()[0].value = "x".into()),
        ),
        (
            "san ignore case",
            edit(&|tls| auth(tls).allowed_sans.as_mut().unwrap()[0].ignore_case = true),
        ),
        (
            "san match",
            edit(&|tls| auth(tls).allowed_sans.as_mut().unwrap()[0].match_kind = AllowedSanMatch::Suffix),
        ),
        (
            "san type",
            edit(&|tls| auth(tls).allowed_sans.as_mut().unwrap()[0].san_type = AllowedSanType::Uri),
        ),
        ("san removed", edit(&|tls| auth(tls).allowed_sans = None)),
        (
            "cn value",
            edit(&|tls| auth(tls).allowed_cns = Some(vec!["other".into()])),
        ),
        ("cn removed", edit(&|tls| auth(tls).allowed_cns = None)),
    ];
    for (label, flipped) in &flips {
        assert_ne!(*flipped, reference, "{label} must change the session context");
    }

    // Runtime-equivalent policies share an id.
    let terminate = edit(&|tls| auth(tls).mode = ClientAuthMode::Terminate);
    let terminate_other = edit(&|tls| {
        let auth = auth(tls);
        auth.mode = ClientAuthMode::Terminate;
        auth.verify_depth = 5;
        auth.allowed_cns = None;
    });
    let none = edit(&|tls| tls.spec.client_auth = None);
    assert_eq!(terminate, terminate_other);
    assert_eq!(terminate, none);
    let empty_lists = edit(&|tls| {
        auth(tls).allowed_sans = Some(Vec::new());
        auth(tls).allowed_cns = Some(Vec::new());
    });
    let absent_lists = edit(&|tls| {
        auth(tls).allowed_sans = None;
        auth(tls).allowed_cns = None;
    });
    assert_eq!(empty_lists, absent_lists);

    // The OID of a valid OtherName entry is an input.
    let other_name = |oid: &str| {
        edit(&|tls| {
            let san = &mut auth(tls).allowed_sans.as_mut().unwrap()[0];
            san.san_type = AllowedSanType::OtherName;
            san.oid = Some(oid.into());
        })
    };
    assert_ne!(
        other_name("1.3.6.1.4.1.311.20.2.3"),
        other_name("1.3.6.1.4.1.311.20.2.4")
    );
}

/// gw-infra-01: the real cold compile feeds the serving-chain and full CA
/// bundle digests into the session context. An unchanged generation keeps its
/// id (resumption survives relists); changing a non-first CA bundle entry or
/// the serving certificate changes it.
#[test]
fn compiled_edgion_tls_session_context_follows_ca_bundle_and_serving_chain() {
    use super::CompiledEdgionTls;
    use edgion_resources::EdgionTls;
    use std::sync::Arc;

    fn secret(name: &str, entries: Vec<(&str, String)>) -> k8s_openapi::api::core::v1::Secret {
        k8s_openapi::api::core::v1::Secret {
            metadata: kube::api::ObjectMeta {
                name: Some(name.into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            data: Some(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.to_string(), k8s_openapi::ByteString(value.into_bytes())))
                    .collect(),
            ),
            ..Default::default()
        }
    }
    let server = rcgen::generate_simple_self_signed(vec!["edge.example".into()]).unwrap();
    let other_server = rcgen::generate_simple_self_signed(vec!["edge.example".into()]).unwrap();
    let (first_ca, second_ca, third_ca) = (
        make_self_signed_pem("ca-1"),
        make_self_signed_pem("ca-2"),
        make_self_signed_pem("ca-3"),
    );
    let compile = |server: &rcgen::CertifiedKey<rcgen::KeyPair>, ca_bundle: String| {
        let mut tls: EdgionTls = serde_json::from_value(serde_json::json!({
            "metadata": {"name": "edge", "namespace": "default"},
            "spec": {
                "hosts": ["edge.example"],
                "secretRef": {"name": "server"},
                "clientAuth": {"mode": "Mutual", "caSecretRef": {"name": "ca"}}
            }
        }))
        .unwrap();
        tls.spec.secret = Some(secret(
            "server",
            vec![
                ("tls.crt", server.cert.pem()),
                ("tls.key", server.signing_key.serialize_pem()),
            ],
        ));
        tls.spec.client_auth.as_mut().unwrap().ca_secret = Some(secret("ca", vec![("ca.crt", ca_bundle)]));
        let compiled = CompiledEdgionTls::compile(Arc::new(tls));
        assert!(compiled.identity.is_ok());
        assert!(matches!(compiled.client_ca_store, Ok(Some(_))));
        compiled.session_context
    };
    let reference = compile(&server, format!("{first_ca}{second_ca}"));
    assert_eq!(reference, compile(&server, format!("{first_ca}{second_ca}")));
    assert_ne!(reference, compile(&server, format!("{first_ca}{third_ca}")));
    assert_ne!(reference, compile(&server, first_ca.clone()));
    assert_ne!(reference, compile(&other_server, format!("{first_ca}{second_ca}")));
}
