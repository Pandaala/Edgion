use super::{
    install_compiled_serving_identity, pick_secret_index, select_gateway_edgion_tls_overlay, CompiledEdgionTls, SslCtx,
    TlsCallback,
};
use crate::runtime::matching::tls::{der_list_digest, SessionContextId};
use crate::runtime::matching::{FrontendMtlsOutcome, GatewayTlsEntry};
use edgion_resources::resources::edgion_tls::{
    ClientAuthConfig, ClientAuthMode, EdgionTls, EdgionTlsSpec, ResolvedListenerAttachment,
};
use edgion_resources::resources::gateway::SecretObjectReference;
use edgion_resources::ListenerKey;
use kube::api::ObjectMeta;
use std::net::SocketAddr;
use std::sync::Arc;

static HANDSHAKE_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

const RSA: Option<bool> = Some(false);
const ECDSA: Option<bool> = Some(true);
const UNKNOWN: Option<bool> = None;

struct ServingChainFixture {
    chain_pem: String,
    root: boring::x509::X509,
}

fn serving_chain(prefix: &str, leaf_key: &rcgen::KeyPair) -> ServingChainFixture {
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};

    let root_key = KeyPair::generate().unwrap();
    let mut root_params = CertificateParams::default();
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, format!("{prefix}-root"));
    let root_issuer = Issuer::from_params(&root_params, &root_key);
    let root = root_params.self_signed(&root_key).unwrap();

    let intermediate_key = KeyPair::generate().unwrap();
    let mut intermediate_params = CertificateParams::default();
    intermediate_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    intermediate_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, format!("{prefix}-intermediate"));
    let intermediate_issuer = Issuer::from_params(&intermediate_params, &intermediate_key);
    let intermediate = intermediate_params.signed_by(&intermediate_key, &root_issuer).unwrap();

    let leaf = CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(leaf_key, &intermediate_issuer)
        .unwrap();
    ServingChainFixture {
        chain_pem: format!("{}{}", leaf.pem(), intermediate.pem()),
        root: boring::x509::X509::from_der(root.der()).unwrap(),
    }
}

/// Wrap a fixture resource without compiling secret-bearing state, for the
/// pure attachment/precedence assertions that never touch the identity.
fn uncompiled(tls: Arc<EdgionTls>) -> Arc<CompiledEdgionTls> {
    Arc::new(CompiledEdgionTls::not_compiled(tls))
}

/// Wrap a fixture resource through the real publication-time compiler, so
/// the handshake assertions exercise the cold-compiled path end to end.
fn compiled(tls: Arc<EdgionTls>) -> Arc<CompiledEdgionTls> {
    Arc::new(CompiledEdgionTls::compile(tls))
}

fn edgion_tls_identity(cert_pem: String, key_pem: String) -> Arc<EdgionTls> {
    use edgion_resources::constants::secret_keys::tls::{CERT, KEY};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    let (fixture, _) = overlay_fixture("winner", false);
    let mut tls = (*fixture).clone();
    tls.spec.secret = Some(Secret {
        metadata: kube::api::ObjectMeta {
            name: Some(tls.spec.secret_ref.name.clone()),
            namespace: tls.spec.secret_ref.namespace.clone().or(tls.metadata.namespace.clone()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([
            (CERT.to_string(), ByteString(cert_pem.into_bytes())),
            (KEY.to_string(), ByteString(key_pem.into_bytes())),
        ])),
        ..Default::default()
    });
    Arc::new(tls)
}

fn install_test_identity(ssl: &mut boring::ssl::SslRef, chain_pem: &str, key_pem: &str) {
    let chain = edgion_common::tls_validation::parse_strict_certificate_chain(chain_pem.as_bytes()).unwrap();
    let certificates = chain
        .certificates_der
        .iter()
        .map(|der| boring::x509::X509::from_der(der).unwrap())
        .collect::<Vec<_>>();
    pingora_core::tls::ext::ssl_use_certificate(ssl, &certificates[0]).unwrap();
    for intermediate in certificates.iter().skip(1) {
        pingora_core::tls::ext::ssl_add_chain_cert(ssl, intermediate).unwrap();
    }
    let key = boring::pkey::PKey::private_key_from_pem(key_pem.as_bytes()).unwrap();
    pingora_core::tls::ext::ssl_use_private_key(ssl, &key).unwrap();
}

fn loopback_handshake(ssl: boring::ssl::Ssl, trusted_root: boring::x509::X509) -> (bool, bool) {
    use boring::ssl::{Ssl, SslContext, SslMethod, SslVerifyMode};
    use boring::x509::store::X509StoreBuilder;
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        ssl.accept(tcp).is_ok()
    });

    let mut store = X509StoreBuilder::new().unwrap();
    store.add_cert(trusted_root).unwrap();
    let mut client_builder = SslContext::builder(SslMethod::tls()).unwrap();
    client_builder.set_verify(SslVerifyMode::PEER);
    client_builder.set_verify_cert_store(store.build()).unwrap();
    let tcp = TcpStream::connect(address).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut client_ssl = Ssl::new(&client_builder.build()).unwrap();
    client_ssl.set_hostname("localhost").unwrap();
    let client_ok = client_ssl.connect(tcp).is_ok();
    (server.join().unwrap(), client_ok)
}

#[test]
fn compiled_certificate_validity_bounds_are_inclusive() {
    let validity = edgion_common::cert_util::LeafValidity {
        not_before: 10,
        not_after: 20,
    };
    assert_eq!(validity.validity_error(9), Some("cert_not_yet_valid"));
    assert_eq!(validity.validity_error(10), None);
    assert_eq!(validity.validity_error(20), None);
    assert_eq!(validity.validity_error(21), Some("cert_expired"));
}

fn overlay_fixture(attached_gateway: &str, client_auth: bool) -> (Arc<EdgionTls>, GatewayTlsEntry) {
    let listener_key = ListenerKey::new("0.0.0.0:443".parse::<SocketAddr>().unwrap(), "HTTPS");
    let gateway_proof = edgion_resources::resources::edgion_tls::GatewayAttachmentProof {
        instance: "gateway-uid".to_string(),
        generation: 7,
        epoch: "resource-version-9".to_string(),
        attachment_digest: "a".repeat(64),
    };
    let tls = Arc::new(EdgionTls {
        metadata: ObjectMeta {
            namespace: Some("default".to_string()),
            name: Some("tls".to_string()),
            ..Default::default()
        },
        spec: EdgionTlsSpec {
            current_status: None,
            parent_refs: None,
            hosts: vec!["example.com".to_string()],
            secret_ref: SecretObjectReference {
                group: None,
                kind: None,
                name: "server".to_string(),
                namespace: None,
            },
            client_auth: client_auth.then_some(ClientAuthConfig {
                mode: ClientAuthMode::Mutual,
                ca_secret_ref: None,
                verify_depth: 1,
                ca_secret: None,
                allowed_sans: None,
                allowed_cns: None,
            }),
            min_tls_version: None,
            ciphers: None,
            secret: None,
            resolved_listeners: Some(vec![listener_key.clone()]),
            resolved_listener_attachments: Some(vec![ResolvedListenerAttachment {
                gateway_namespace: "default".to_string(),
                gateway_name: attached_gateway.to_string(),
                listener_name: "https".to_string(),
                listener_key: listener_key.clone(),
                gateway_proof: gateway_proof.clone(),
            }]),
            resolved_listener_attachment_index: Default::default(),
            resolved_log_labels: None,
        },
        status: None,
    });
    let gateway = GatewayTlsEntry::for_test(
        ResolvedListenerAttachment {
            gateway_namespace: "default".to_string(),
            gateway_name: "winner".to_string(),
            listener_name: "https".to_string(),
            listener_key,
            gateway_proof,
        },
        FrontendMtlsOutcome::StrictEffective,
    );
    (tls, gateway)
}

#[test]
fn gateway_overlay_requires_exact_attachment_and_rejects_dual_client_auth() {
    let (exact, gateway) = overlay_fixture("winner", false);
    assert!(
        select_gateway_edgion_tls_overlay(Some(uncompiled(exact.clone())), &gateway)
            .unwrap()
            .is_none()
    );
    exact.spec.warm_listener_attachment_index();
    assert!(
        select_gateway_edgion_tls_overlay(Some(uncompiled(exact.clone())), &gateway)
            .unwrap()
            .is_some()
    );

    let (mismatch, _) = overlay_fixture("other", false);
    mismatch.spec.warm_listener_attachment_index();
    assert!(select_gateway_edgion_tls_overlay(Some(uncompiled(mismatch)), &gateway)
        .unwrap()
        .is_none());

    let (conflict, _) = overlay_fixture("winner", true);
    conflict.spec.warm_listener_attachment_index();
    assert!(select_gateway_edgion_tls_overlay(Some(uncompiled(conflict)), &gateway).is_err());

    let mut stale_value = exact.as_ref().clone();
    stale_value.spec.resolved_listener_attachments.as_mut().unwrap()[0]
        .gateway_proof
        .epoch = "stale-resource-version".to_string();
    let stale = Arc::new(stale_value);
    stale.spec.warm_listener_attachment_index();
    assert!(select_gateway_edgion_tls_overlay(Some(uncompiled(stale)), &gateway)
        .unwrap()
        .is_none());

    let selected = select_gateway_edgion_tls_overlay(Some(uncompiled(exact.clone())), &gateway)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&selected.tls, &exact));
}

#[test]
fn published_regex_san_preserves_attachment_and_rejects_dual_client_auth() {
    use crate::tls::store::TlsStore;
    use edgion_resources::constants::secret_keys::tls::{CERT, KEY};
    use edgion_resources::resources::edgion_tls::{AllowedSan, AllowedSanMatch, AllowedSanType};
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use rcgen::{CertificateParams, KeyPair};
    use std::collections::{BTreeMap, HashMap, HashSet};

    let (fixture, gateway) = overlay_fixture("winner", true);
    let mut tls = (*fixture).clone();
    let key = KeyPair::generate().unwrap();
    let cert = CertificateParams::new(vec!["example.com".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    tls.spec.secret = Some(Secret {
        metadata: kube::api::ObjectMeta {
            name: Some(tls.spec.secret_ref.name.clone()),
            namespace: tls.spec.secret_ref.namespace.clone().or(tls.metadata.namespace.clone()),
            ..Default::default()
        },
        data: Some(BTreeMap::from([
            (CERT.to_string(), ByteString(cert.pem().into_bytes())),
            (KEY.to_string(), ByteString(key.serialize_pem().into_bytes())),
        ])),
        ..Default::default()
    });
    let mut san = AllowedSan::exact(AllowedSanType::Dns, r"^client\.example\.com$");
    san.match_kind = AllowedSanMatch::RegularExpression;
    let client_auth = tls.spec.client_auth.as_mut().unwrap();
    client_auth.allowed_sans = Some(vec![san]);
    client_auth.ca_secret_ref = Some(SecretObjectReference {
        name: "client-ca".to_string(),
        ..tls.spec.secret_ref.clone()
    });

    let store = TlsStore::new();
    for phase in ["full", "add", "update"] {
        let resource = Arc::new(tls.clone());
        let data = HashMap::from([("default/tls".to_string(), resource)]);
        match phase {
            "full" => store.replace_all_tls(data),
            "add" => {
                store.replace_all_tls(HashMap::new());
                store.apply_partial_update(data, HashMap::new(), &HashSet::new());
            }
            "update" => store.apply_partial_update(HashMap::new(), data, &HashSet::new()),
            _ => unreachable!(),
        }
        assert!(store.get_validation_status("default/tls").unwrap().is_valid);
        let published = store
            .snapshot()
            .match_attachment_sni(&gateway.attachment, "example.com")
            .expect("valid published certificate must match its exact attachment");
        let san = &published
            .tls
            .spec
            .client_auth
            .as_ref()
            .unwrap()
            .allowed_sans
            .as_ref()
            .unwrap()[0];
        assert!(san.compiled.is_some(), "SAN must compile during {phase}");
        // Do not manually warm the candidate: publication owns this cache.
        assert!(
            select_gateway_edgion_tls_overlay(Some(published), &gateway).is_err(),
            "dual client-auth must reject after {phase} publication"
        );
    }
}

#[test]
fn empty_list_returns_none() {
    assert_eq!(pick_secret_index(&[], true), None);
    assert_eq!(pick_secret_index(&[], false), None);
}

#[cfg(feature = "boringssl")]
#[test]
fn strict_serving_parsers_preserve_chain_and_accept_supported_key_formats() {
    use boring::asn1::{Asn1Integer, Asn1Time};
    use boring::bn::BigNum;
    use boring::ec::{EcGroup, EcKey};
    use boring::hash::MessageDigest;
    use boring::nid::Nid;
    use boring::pkey::{PKey, Private};
    use boring::rsa::Rsa;
    use boring::x509::{X509NameBuilder, X509};

    fn certificate(
        cn: &str,
        subject_key: &PKey<Private>,
        issuer: Option<&X509>,
        signing_key: &PKey<Private>,
        serial: u32,
    ) -> X509 {
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", cn).unwrap();
        let name = name.build();
        let mut builder = X509::builder().unwrap();
        builder.set_version(2).unwrap();
        let serial = Asn1Integer::from_bn(&BigNum::from_u32(serial).unwrap()).unwrap();
        builder.set_serial_number(&serial).unwrap();
        builder.set_subject_name(&name).unwrap();
        builder
            .set_issuer_name(issuer.map_or(name.as_ref(), |certificate| certificate.subject_name()))
            .unwrap();
        builder.set_pubkey(subject_key).unwrap();
        builder.set_not_before(&Asn1Time::days_from_now(0).unwrap()).unwrap();
        builder.set_not_after(&Asn1Time::days_from_now(30).unwrap()).unwrap();
        builder.sign(signing_key, MessageDigest::sha256()).unwrap();
        builder.build()
    }

    let root_key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let root = certificate("root", &root_key, None, &root_key, 1);
    let leaf_rsa = Rsa::generate(2048).unwrap();
    let rsa_pkcs1 = String::from_utf8(leaf_rsa.private_key_to_pem().unwrap()).unwrap();
    let leaf_key = PKey::from_rsa(leaf_rsa).unwrap();
    let rsa_pkcs8 = String::from_utf8(leaf_key.private_key_to_pem_pkcs8().unwrap()).unwrap();
    let leaf = certificate("leaf", &leaf_key, Some(&root), &root_key, 2);
    let chain_pem = format!(
        "{}{}",
        String::from_utf8(leaf.to_pem().unwrap()).unwrap(),
        String::from_utf8(root.to_pem().unwrap()).unwrap()
    );
    let parsed = edgion_common::tls_validation::parse_strict_certificate_chain(chain_pem.as_bytes()).unwrap();
    assert_eq!(
        parsed.certificates_der,
        vec![leaf.to_der().unwrap(), root.to_der().unwrap()]
    );
    edgion_common::tls_validation::check_key_matches_cert(&chain_pem, &rsa_pkcs8).unwrap();
    edgion_common::tls_validation::check_key_matches_cert(&chain_pem, &rsa_pkcs1).unwrap();

    let ec = EcKey::generate(&EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap()).unwrap();
    let ec_sec1 = String::from_utf8(ec.private_key_to_pem().unwrap()).unwrap();
    let ec_key = PKey::from_ec_key(ec).unwrap();
    let ec_cert = certificate("ec-leaf", &ec_key, None, &ec_key, 3);
    let ec_cert_pem = String::from_utf8(ec_cert.to_pem().unwrap()).unwrap();
    edgion_common::tls_validation::check_key_matches_cert(&ec_cert_pem, &ec_sec1).unwrap();
}

#[test]
fn edgion_tls_clears_inherited_identity_before_strict_parse() {
    let _serial = HANDSHAKE_TEST_LOCK.lock();
    use boring::ssl::{Ssl, SslContext, SslMethod};

    let prior_key = rcgen::KeyPair::generate().unwrap();
    let prior = serving_chain("entry-clear", &prior_key);
    let prior_key_pem = prior_key.serialize_pem();
    let server_ctx = SslContext::builder(SslMethod::tls()).unwrap().build();
    let mut ssl = Ssl::new(&server_ctx).unwrap();
    install_test_identity(ssl.as_mut(), &prior.chain_pem, &prior_key_pem);

    let malformed_chain = format!("{}not-a-certificate", prior.chain_pem);
    let tls = edgion_tls_identity(malformed_chain, prior_key_pem);
    let mut ssl_ctx = SslCtx::new(443);
    TlsCallback::new(443).apply_edgion_tls_cert(ssl.as_mut(), &compiled(tls), &mut ssl_ctx);

    assert_eq!(ssl_ctx.meta.err_log.as_deref(), Some("parse certificate failed"));
    let (server_ok, client_ok) = loopback_handshake(ssl, prior.root);
    assert!(!server_ok);
    assert!(
        !client_ok,
        "a strict-chain failure must not serve the inherited identity"
    );
}

/// The operator cipher string is rendered when the generation is published,
/// but a rendering failure must still abort the handshake with no identity
/// bound — cipher policy is a security boundary, so it can never degrade to
/// "serve anyway on BoringSSL's default suite" (tls-F002).
#[test]
fn uncompilable_operator_cipher_fails_closed_before_binding_identity() {
    let _serial = HANDSHAKE_TEST_LOCK.lock();
    use boring::ssl::{Ssl, SslContext, SslMethod};

    let key = rcgen::KeyPair::generate().unwrap();
    let serving = serving_chain("cipher-fail-close", &key);
    let mut resource = (*edgion_tls_identity(serving.chain_pem, key.serialize_pem())).clone();
    // An interior NUL cannot be rendered into the C string BoringSSL needs.
    resource.spec.ciphers = Some(vec!["ECDHE-RSA-AES128-GCM-SHA256\0EXTRA".to_string()]);
    let compiled_resource = compiled(Arc::new(resource));
    assert!(
        matches!(
            compiled_resource.operator_cipher,
            crate::tls::downstream_policy::CipherPolicy::Reject(_)
        ),
        "the cipher string must fail at publication, not at handshake"
    );

    let server_ctx = SslContext::builder(SslMethod::tls()).unwrap().build();
    let mut ssl = Ssl::new(&server_ctx).unwrap();
    let result = crate::tls::downstream_policy::DownstreamTlsPolicy::default().apply(
        ssl.as_mut(),
        None,
        Some(&compiled_resource.operator_cipher),
        false,
    );
    assert_eq!(result, Err("ciphers must not contain NUL bytes"));
    assert!(
        ssl.certificate().is_none(),
        "a cipher policy failure must return before any identity is bound"
    );
}

#[test]
fn edgion_tls_clears_partial_identity_after_key_install_failure() {
    let _serial = HANDSHAKE_TEST_LOCK.lock();
    use boring::ssl::{Ssl, SslContext, SslMethod};

    let shared_key = rcgen::KeyPair::generate().unwrap();
    let shared_key_pem = shared_key.serialize_pem();
    let prior = serving_chain("key-clear-prior", &shared_key);
    let new = serving_chain("key-clear-new", &shared_key);
    let wrong_key_pem = rcgen::KeyPair::generate().unwrap().serialize_pem();
    let callback = TlsCallback::new(443);
    let server_ctx = SslContext::builder(SslMethod::tls()).unwrap().build();

    // Positive control: the new leaf/intermediate fixture and shared key
    // form a complete identity trusted by a root-only client.
    let mut control_ssl = Ssl::new(&server_ctx).unwrap();
    let control_tls = edgion_tls_identity(new.chain_pem.clone(), shared_key_pem.clone());
    let mut control_ctx = SslCtx::new(443);
    callback.apply_edgion_tls_cert(control_ssl.as_mut(), &compiled(control_tls), &mut control_ctx);
    assert_eq!(control_ctx.meta.err_log, None);
    let (control_server_ok, control_client_ok) = loopback_handshake(control_ssl, new.root.clone());
    assert!(control_server_ok);
    assert!(
        control_client_ok,
        "a client trusting only the root must validate the served intermediate"
    );

    let mut ssl = Ssl::new(&server_ctx).unwrap();
    install_test_identity(ssl.as_mut(), &prior.chain_pem, &shared_key_pem);
    let mismatched_tls = edgion_tls_identity(new.chain_pem, wrong_key_pem);
    let mut ssl_ctx = SslCtx::new(443);
    callback.apply_edgion_tls_cert(ssl.as_mut(), &compiled(mismatched_tls), &mut ssl_ctx);

    // The cold-compiled identity is installed atomically, so a mismatched
    // key surfaces as the single install category rather than the former
    // per-step one. A key/cert mismatch is rejected at publication by
    // `validate_cert` -> `check_key_matches_cert`, so this state is
    // reachable only by constructing it directly as this test does.
    assert_eq!(ssl_ctx.meta.err_log.as_deref(), Some("set serving identity"));
    assert!(
        ssl.certificate().is_none(),
        "a key-install failure must clear the partially installed identity"
    );
    let (server_ok, client_ok) = loopback_handshake(ssl, new.root);
    assert!(!server_ok);
    assert!(!client_ok, "the cleared partial identity must not complete a handshake");
}

#[test]
fn atomic_identity_rotation_sends_intermediate_and_replaces_old_chain() {
    let _serial = HANDSHAKE_TEST_LOCK.lock();
    use crate::runtime::matching::tls::{CompiledBoringCertificateChain, CompiledServingCertificate};
    use boring::pkey::PKey;
    use boring::ssl::{Ssl, SslContext, SslMethod, SslVerifyMode};
    use boring::x509::store::X509StoreBuilder;
    use boring::x509::X509;
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    fn identity(prefix: &str) -> (Arc<CompiledServingCertificate>, X509) {
        let root_key = KeyPair::generate().unwrap();
        let mut root_params = CertificateParams::default();
        root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        root_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, format!("{prefix}-root"));
        let root_issuer = Issuer::from_params(&root_params, &root_key);
        let root = root_params.self_signed(&root_key).unwrap();

        let intermediate_key = KeyPair::generate().unwrap();
        let mut intermediate_params = CertificateParams::default();
        intermediate_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        intermediate_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, format!("{prefix}-intermediate"));
        let intermediate_issuer = Issuer::from_params(&intermediate_params, &intermediate_key);
        let intermediate = intermediate_params.signed_by(&intermediate_key, &root_issuer).unwrap();

        let leaf_key = KeyPair::generate().unwrap();
        let leaf = CertificateParams::new(vec!["localhost".to_string()])
            .unwrap()
            .signed_by(&leaf_key, &intermediate_issuer)
            .unwrap();
        let chain_der = vec![leaf.der().to_vec(), intermediate.der().to_vec()];
        let chain = chain_der
            .iter()
            .map(|der| X509::from_der(der).unwrap())
            .collect::<Vec<_>>();
        let private_key = PKey::private_key_from_pem(leaf_key.serialize_pem().as_bytes()).unwrap();
        let compiled = Arc::new(CompiledServingCertificate {
            chain: Arc::from(chain),
            chain_digest: der_list_digest(chain_der.iter().map(Vec::as_slice)),
            atomic_chain: Arc::new(CompiledBoringCertificateChain::from_der(&chain_der).unwrap()),
            key_type: private_key.id(),
            private_key,
            validity: edgion_common::cert_util::LeafValidity {
                not_before: i64::MIN,
                not_after: i64::MAX,
            },
        });
        (compiled, X509::from_der(root.der()).unwrap())
    }

    let (old_identity, old_root) = identity("old");
    let (new_identity, new_root) = identity("new");
    let invalid_replacement = Arc::new(CompiledServingCertificate {
        chain: old_identity.chain.clone(),
        chain_digest: old_identity.chain_digest,
        atomic_chain: Arc::new(CompiledBoringCertificateChain::from_der(&[b"not-der".to_vec()]).unwrap()),
        private_key: old_identity.private_key.clone(),
        key_type: old_identity.key_type,
        validity: edgion_common::cert_util::LeafValidity {
            not_before: i64::MIN,
            not_after: i64::MAX,
        },
    });
    let server_ctx = SslContext::builder(SslMethod::tls()).unwrap().build();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut ssl = Ssl::new(&server_ctx).unwrap();
        assert!(install_compiled_serving_identity(ssl.as_mut(), &old_identity));
        assert!(install_compiled_serving_identity(ssl.as_mut(), &new_identity));
        let valid_rotation = ssl.accept(tcp).is_ok();

        let (tcp, _) = listener.accept().unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut ssl = Ssl::new(&server_ctx).unwrap();
        assert!(install_compiled_serving_identity(ssl.as_mut(), &old_identity));
        assert!(!install_compiled_serving_identity(ssl.as_mut(), &invalid_replacement));
        (valid_rotation, ssl.accept(tcp).is_err())
    });

    let mut store = X509StoreBuilder::new().unwrap();
    store.add_cert(new_root).unwrap();
    let mut client_builder = SslContext::builder(SslMethod::tls()).unwrap();
    client_builder.set_verify(SslVerifyMode::PEER);
    client_builder.set_verify_cert_store(store.build()).unwrap();
    let client_ctx = client_builder.build();
    let tcp = TcpStream::connect(address).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut client_ssl = Ssl::new(&client_ctx).unwrap();
    client_ssl.set_hostname("localhost").unwrap();
    let client = client_ssl.connect(tcp);
    assert!(
        client.is_ok(),
        "client trusting only the new root must validate the sent intermediate"
    );
    let mut old_store = X509StoreBuilder::new().unwrap();
    old_store.add_cert(old_root).unwrap();
    let mut old_client_builder = SslContext::builder(SslMethod::tls()).unwrap();
    old_client_builder.set_verify(SslVerifyMode::PEER);
    old_client_builder.set_verify_cert_store(old_store.build()).unwrap();
    let tcp = TcpStream::connect(address).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    tcp.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut old_client_ssl = Ssl::new(&old_client_builder.build()).unwrap();
    old_client_ssl.set_hostname("localhost").unwrap();
    assert!(
        old_client_ssl.connect(tcp).is_err(),
        "failed replacement must not continue serving the old identity"
    );
    let (valid_rotation, failed_replacement) = server.join().unwrap();
    assert!(valid_rotation);
    assert!(failed_replacement);
}

#[test]
fn ecdsa_preferred_picks_ecdsa_regardless_of_order() {
    assert_eq!(pick_secret_index(&[RSA, ECDSA], true), Some(1));
    assert_eq!(pick_secret_index(&[ECDSA, RSA], true), Some(0));
}

#[test]
fn ecdsa_not_preferred_picks_first_rsa() {
    assert_eq!(pick_secret_index(&[ECDSA, RSA], false), Some(1));
    assert_eq!(pick_secret_index(&[RSA, ECDSA], false), Some(0));
}

#[test]
fn ecdsa_preferred_but_only_rsa_falls_back_to_rsa() {
    assert_eq!(pick_secret_index(&[RSA, RSA], true), Some(0));
}

#[test]
fn no_rsa_present_falls_back_to_first() {
    // prefer_ecdsa false but only ECDSA certs: no Some(false), fall to index 0.
    assert_eq!(pick_secret_index(&[ECDSA, ECDSA], false), Some(0));
}

#[test]
fn unparseable_certs_are_skipped_then_fall_back() {
    // ECDSA preferred, first is unknown, second is ECDSA.
    assert_eq!(pick_secret_index(&[UNKNOWN, ECDSA], true), Some(1));
    // RSA preferred path skips unknown, picks the RSA.
    assert_eq!(pick_secret_index(&[UNKNOWN, RSA], false), Some(1));
    // All unknown: fall back to first.
    assert_eq!(pick_secret_index(&[UNKNOWN, UNKNOWN], true), Some(0));
}

#[test]
fn single_cert_is_index_zero() {
    assert_eq!(pick_secret_index(&[RSA], true), Some(0));
    assert_eq!(pick_secret_index(&[ECDSA], false), Some(0));
}

#[test]
fn handshake_failure_audit_survives_certificate_finalization_and_is_claimed_once() {
    use crate::observe::logs::ssl_log::{log_ssl, tests::capture};
    use boring::ssl::{Ssl, SslContext, SslMethod};
    use foreign_types::ForeignType;

    let context = SslContext::builder(SslMethod::tls()).unwrap().build();
    for finalized in [false, true] {
        for claimed in [false, true] {
            let entries = capture(|| {
                let mut ssl = Ssl::new(&context).unwrap();
                let mut ctx = SslCtx::new(443);
                if finalized {
                    ctx.meta.tls_id = Some("audit-test".to_string());
                }
                assert!(super::ssl_ctx_ex_data::store_ssl_ctx(&mut ssl, &ctx));
                if claimed {
                    let mut meta = unsafe { super::claim_tls_failure_meta(ssl.as_ptr()) }.unwrap();
                    meta.err_log = Some("client verification failed".to_string());
                    log_ssl(&meta);
                    assert!(unsafe { super::claim_tls_failure_meta(ssl.as_ptr()) }.is_none());
                }
                drop(ssl);
            });
            assert_eq!(entries.len(), 1);
            assert_eq!(
                entries[0].err_log.as_deref(),
                Some(if claimed {
                    "client verification failed"
                } else if finalized {
                    "handshake failed"
                } else {
                    "early abort"
                })
            );
        }
    }
    let entries = capture(|| {
        let mut ssl = Ssl::new(&context).unwrap();
        assert!(super::ssl_ctx_ex_data::store_ssl_ctx(&mut ssl, &SslCtx::new(443)));
        assert!(super::ssl_ctx_ex_data::take_ssl_ctx(&ssl).is_some());
        drop(ssl);
    });
    assert!(entries.is_empty());
}

#[test]
fn tls13_post_certificate_group_failure_and_success_emit_one_audit() {
    use crate::observe::logs::ssl_log::tests::capture;
    use crate::tls::curve_policy::CurvePolicy;
    use boring::ssl::{ErrorCode, Ssl, SslContext, SslMethod, SslStream, SslVerifyMode, SslVersion};
    use edgion_resources::resources::downstream_tls::parse_ecdh_curves;
    use foreign_types::{ForeignType, ForeignTypeRef};
    use pingora_core::listeners::TlsAccept;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io::{self, Read, Write};
    use std::rc::Rc;

    struct MemoryStream {
        incoming: Rc<RefCell<VecDeque<u8>>>,
        outgoing: Rc<RefCell<VecDeque<u8>>>,
    }
    impl Read for MemoryStream {
        fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
            let mut incoming = self.incoming.borrow_mut();
            if incoming.is_empty() && !target.is_empty() {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let count = target.len().min(incoming.len());
            for slot in &mut target[..count] {
                *slot = incoming.pop_front().unwrap();
            }
            Ok(count)
        }
    }
    impl Write for MemoryStream {
        fn write(&mut self, source: &[u8]) -> io::Result<usize> {
            self.outgoing.borrow_mut().extend(source);
            Ok(source.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    unsafe extern "C" fn pause_certificate(_: *mut boring_sys::SSL, _: *mut std::ffi::c_void) -> std::ffi::c_int {
        -1
    }
    let _guard = HANDSHAKE_TEST_LOCK.lock();
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let chain = serving_chain("audit", &key);
    let identity = compiled(edgion_tls_identity(chain.chain_pem, key.serialize_pem()));
    for offered in ["P-256", "P-384"] {
        let entries = capture(|| {
            let mut server = SslContext::builder(SslMethod::tls()).unwrap();
            server.set_min_proto_version(Some(SslVersion::TLS1_3)).unwrap();
            let identity = identity.clone();
            server.set_select_certificate_callback(move |mut hello| {
                let mut ctx = SslCtx::new(443);
                ctx.matched_edgion_tls = Some(identity.clone());
                let policy = crate::tls::downstream_policy::DownstreamTlsPolicy::compile(Some(
                    &edgion_resources::resources::downstream_tls::DownstreamTlsConfig {
                        ecdh_curves: Some(vec!["P-384".into()]),
                        ..Default::default()
                    },
                ));
                super::apply_selected_downstream_policy(hello.ssl_mut(), &ctx, &policy, false).unwrap();
                assert!(super::ssl_ctx_ex_data::store_ssl_ctx(hello.ssl_mut(), &ctx));
                Ok(())
            });
            let server_ssl = Ssl::new(&server.build()).unwrap();
            // Match Pingora's pause -> async certificate callback -> resume order.
            unsafe { boring_sys::SSL_set_cert_cb(server_ssl.as_ptr(), Some(pause_certificate), std::ptr::null_mut()) };
            let mut client = SslContext::builder(SslMethod::tls()).unwrap();
            client.set_min_proto_version(Some(SslVersion::TLS1_3)).unwrap();
            client.set_verify(SslVerifyMode::NONE);
            let mut client_ssl = Ssl::new(&client.build()).unwrap();
            CurvePolicy::compile(parse_ecdh_curves([offered]).map(Some))
                .apply(&mut client_ssl, false)
                .unwrap();
            let a = Rc::new(RefCell::new(VecDeque::new()));
            let b = Rc::new(RefCell::new(VecDeque::new()));
            let mut server = SslStream::new(
                server_ssl,
                MemoryStream {
                    incoming: a.clone(),
                    outgoing: b.clone(),
                },
            )
            .unwrap();
            let mut client = SslStream::new(
                client_ssl,
                MemoryStream {
                    incoming: b,
                    outgoing: a,
                },
            )
            .unwrap();
            let callback = TlsCallback { port: 443 };
            let (mut client_done, mut server_done, mut certificate_called, mut group_failed) =
                (false, false, false, false);
            for _ in 0..32 {
                if !client_done {
                    match client.connect() {
                        Ok(()) => client_done = true,
                        Err(error) if error.code() == ErrorCode::WANT_READ => {}
                        Err(error) => panic!("unexpected client error: {error}"),
                    }
                }
                if !server_done {
                    match server.accept() {
                        Ok(()) => server_done = true,
                        Err(error) if error.code() == ErrorCode::WANT_READ => {}
                        Err(error) if error.code() == ErrorCode::WANT_X509_LOOKUP => {
                            assert!(!certificate_called);
                            runtime.block_on(callback.certificate_callback(server.ssl_mut()));
                            certificate_called = true;
                            unsafe { boring_sys::SSL_set_cert_cb(server.ssl().as_ptr(), None, std::ptr::null_mut()) };
                        }
                        Err(error) => {
                            assert!(certificate_called);
                            assert!(error.to_string().contains("NO_SHARED_GROUP"), "{error}");
                            group_failed = true;
                            break;
                        }
                    }
                }
                if client_done && server_done {
                    break;
                }
            }
            assert!(certificate_called);
            assert_eq!(group_failed, offered == "P-256");
            assert_eq!(client_done && server_done, offered == "P-384");
            if server_done {
                assert!(runtime
                    .block_on(callback.handshake_complete_callback(server.ssl()))
                    .is_some());
            }
            drop(server);
        });
        assert_eq!(entries.len(), 1, "offered {offered}");
        assert!(
            entries[0].tls_id.is_some(),
            "real certificate finalization must have run"
        );
        assert_eq!(
            entries[0].err_log.as_deref(),
            (offered == "P-256").then_some("handshake failed")
        );
        assert_eq!(entries[0].handshake_complete_time.is_some(), offered == "P-384");
    }
}

#[test]
fn selected_policy_negotiates_native_edgion_and_overlay_with_pinned_generation() {
    use crate::observe::logs::ssl_log::tests::capture;
    use crate::tls::downstream_policy::DownstreamTlsPolicy;
    use boring::ssl::{ErrorCode, SelectCertError, Ssl, SslContext, SslMethod, SslStream, SslVerifyMode, SslVersion};
    use edgion_resources::resources::downstream_tls::DownstreamTlsConfig;
    use edgion_resources::resources::edgion_tls::TlsVersion;
    use foreign_types::{ForeignType, ForeignTypeRef};
    use pingora_core::listeners::TlsAccept;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io::{self, Read, Write};
    use std::rc::Rc;
    struct MemoryStream {
        incoming: Rc<RefCell<VecDeque<u8>>>,
        outgoing: Rc<RefCell<VecDeque<u8>>>,
    }
    impl Read for MemoryStream {
        fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
            let mut incoming = self.incoming.borrow_mut();
            if incoming.is_empty() && !target.is_empty() {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let count = target.len().min(incoming.len());
            for slot in &mut target[..count] {
                *slot = incoming.pop_front().unwrap();
            }
            Ok(count)
        }
    }
    impl Write for MemoryStream {
        fn write(&mut self, source: &[u8]) -> io::Result<usize> {
            self.outgoing.borrow_mut().extend(source);
            Ok(source.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    unsafe extern "C" fn pause_certificate(_: *mut boring_sys::SSL, _: *mut std::ffi::c_void) -> std::ffi::c_int {
        -1
    }

    let _guard = HANDSHAKE_TEST_LOCK.lock();
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let chain = serving_chain("policy", &key);
    let base = edgion_tls_identity(chain.chain_pem, key.serialize_pem());
    let aes128 = "ECDHE-ECDSA-AES128-GCM-SHA256";
    let aes256 = "ECDHE-ECDSA-AES256-GCM-SHA384";
    let chacha = "ECDHE-ECDSA-CHACHA20-POLY1305";
    // label, default minimum, default ciphers, local minimum, local ciphers,
    // client version, client cipher, FIPS policy, expected handshake.
    let cases = [
        (
            "inherit",
            TlsVersion::Tls12,
            Some(vec![aes128]),
            None,
            None,
            SslVersion::TLS1_2,
            aes128,
            false,
            true,
        ),
        (
            "minimum rejects",
            TlsVersion::Tls13,
            None,
            None,
            None,
            SslVersion::TLS1_2,
            aes128,
            false,
            false,
        ),
        (
            "minimum control",
            TlsVersion::Tls13,
            None,
            None,
            None,
            SslVersion::TLS1_3,
            aes128,
            false,
            true,
        ),
        (
            "cipher rejects",
            TlsVersion::Tls12,
            Some(vec![aes128]),
            None,
            None,
            SslVersion::TLS1_2,
            aes256,
            false,
            false,
        ),
        (
            "invalid default",
            TlsVersion::Tls12,
            Some(vec!["NOT-A-CIPHER"]),
            None,
            None,
            SslVersion::TLS1_2,
            aes128,
            false,
            false,
        ),
        (
            "FIPS default",
            TlsVersion::Tls12,
            None,
            None,
            None,
            SslVersion::TLS1_2,
            chacha,
            true,
            false,
        ),
        (
            "EGC empty FIPS",
            TlsVersion::Tls12,
            Some(vec![]),
            None,
            None,
            SslVersion::TLS1_2,
            chacha,
            true,
            true,
        ),
        (
            "local lower",
            TlsVersion::Tls13,
            Some(vec![aes128]),
            Some(TlsVersion::Tls12),
            None,
            SslVersion::TLS1_2,
            aes128,
            false,
            true,
        ),
        (
            "local higher",
            TlsVersion::Tls12,
            None,
            Some(TlsVersion::Tls13),
            None,
            SslVersion::TLS1_2,
            aes128,
            false,
            false,
        ),
        (
            "local cipher",
            TlsVersion::Tls12,
            Some(vec![aes128]),
            None,
            Some(vec![aes256]),
            SslVersion::TLS1_2,
            aes256,
            false,
            true,
        ),
        (
            "masked invalid",
            TlsVersion::Tls12,
            Some(vec!["NOT-A-CIPHER"]),
            None,
            Some(vec![aes256]),
            SslVersion::TLS1_2,
            aes256,
            false,
            true,
        ),
        (
            "local empty FIPS",
            TlsVersion::Tls12,
            Some(vec![aes128]),
            None,
            Some(vec![]),
            SslVersion::TLS1_2,
            chacha,
            true,
            true,
        ),
    ];
    for source in ["native", "edgion", "overlay"] {
        for (label, minimum, ciphers, local_minimum, local_ciphers, version, client_cipher, fips, expected) in &cases {
            if source == "native" && (local_minimum.is_some() || local_ciphers.is_some()) {
                continue;
            }
            let entries = capture(|| {
                let config = DownstreamTlsConfig {
                    min_tls_version: Some(*minimum),
                    ciphers: ciphers
                        .as_ref()
                        .map(|values| values.iter().map(|v| (*v).to_owned()).collect()),
                    ecdh_curves: Some(vec!["P-256".into()]),
                };
                let published = Arc::new(arc_swap::ArcSwap::from_pointee(DownstreamTlsPolicy::compile(Some(
                    &config,
                ))));
                let mut local = (*base).clone();
                local.spec.min_tls_version = *local_minimum;
                local.spec.ciphers = local_ciphers
                    .as_ref()
                    .map(|values| values.iter().map(|v| (*v).to_owned()).collect());
                let identity = compiled(Arc::new(local));
                let mut selected = SslCtx::new(443);
                if source == "edgion" {
                    selected.matched_edgion_tls = Some(identity);
                } else {
                    let (_, mut gateway) = overlay_fixture("winner", false);
                    gateway.certificate_refs.push(identity.tls.spec.secret_ref.clone());
                    gateway.compiled_serving_certificates =
                        Some(Arc::from(vec![identity.identity.as_ref().unwrap().clone()]));
                    gateway.frontend_mtls = if source == "overlay" {
                        FrontendMtlsOutcome::InsecureFallback
                    } else {
                        FrontendMtlsOutcome::NotConfigured
                    };
                    gateway.refresh_session_context_for_test(None);
                    let gateway = Arc::new(gateway);
                    if source == "overlay" {
                        identity.tls.spec.warm_listener_attachment_index();
                        selected.gateway_edgion_tls_overlay =
                            select_gateway_edgion_tls_overlay(Some(identity), &gateway).unwrap();
                        assert!(selected.gateway_edgion_tls_overlay.is_some());
                    }
                    selected.matched_gateway_tls = Some(gateway);
                }
                let mut server = SslContext::builder(SslMethod::tls()).unwrap();
                let current = published.clone();
                let fips = *fips;
                server.set_select_certificate_callback(move |mut hello| {
                    let policy = current.load_full();
                    let mut selected = selected.clone();
                    if let Err(reason) =
                        super::apply_selected_downstream_policy(hello.ssl_mut(), &selected, &policy, fips)
                    {
                        selected.meta.err_log = Some(reason.into());
                        super::log_ssl(&selected.meta);
                        return Err(SelectCertError::ERROR);
                    }
                    assert!(super::ssl_ctx_ex_data::store_ssl_ctx(hello.ssl_mut(), &selected));
                    Ok(())
                });
                let server_ssl = Ssl::new(&server.build()).unwrap();
                unsafe {
                    boring_sys::SSL_set_cert_cb(server_ssl.as_ptr(), Some(pause_certificate), std::ptr::null_mut())
                };
                let mut client = SslContext::builder(SslMethod::tls()).unwrap();
                client.set_min_proto_version(Some(*version)).unwrap();
                client.set_max_proto_version(Some(*version)).unwrap();
                client.set_cipher_list(client_cipher).unwrap();
                client.set_verify(SslVerifyMode::NONE);
                let client_ssl = Ssl::new(&client.build()).unwrap();
                let a = Rc::new(RefCell::new(VecDeque::new()));
                let b = Rc::new(RefCell::new(VecDeque::new()));
                let mut server = SslStream::new(
                    server_ssl,
                    MemoryStream {
                        incoming: a.clone(),
                        outgoing: b.clone(),
                    },
                )
                .unwrap();
                let mut client = SslStream::new(
                    client_ssl,
                    MemoryStream {
                        incoming: b,
                        outgoing: a,
                    },
                )
                .unwrap();
                let callback = TlsCallback::new(443);
                let (mut client_done, mut server_done) = (false, false);
                for _ in 0..32 {
                    if !client_done {
                        match client.connect() {
                            Ok(()) => client_done = true,
                            Err(error) if error.code() == ErrorCode::WANT_READ => {}
                            Err(_) => break,
                        }
                    }
                    if !server_done {
                        match server.accept() {
                            Ok(()) => server_done = true,
                            Err(error) if error.code() == ErrorCode::WANT_READ => {}
                            Err(error) if error.code() == ErrorCode::WANT_X509_LOOKUP => {
                                // A later publication must not alter this handshake.
                                published.store(Arc::new(DownstreamTlsPolicy::compile(Some(&DownstreamTlsConfig {
                                    ciphers: Some(vec!["NOT-A-CIPHER".into()]),
                                    min_tls_version: Some(TlsVersion::Tls13),
                                    ..Default::default()
                                }))));
                                runtime.block_on(callback.certificate_callback(server.ssl_mut()));
                                unsafe {
                                    boring_sys::SSL_set_cert_cb(server.ssl().as_ptr(), None, std::ptr::null_mut())
                                };
                            }
                            Err(_) => break,
                        }
                    }
                    if client_done && server_done {
                        break;
                    }
                }
                assert_eq!(client_done && server_done, *expected, "{source}: {label}");
                if server_done {
                    assert_eq!(server.ssl().version2(), Some(*version), "{source}: {label}");
                    if *version == SslVersion::TLS1_2 {
                        assert_eq!(
                            server.ssl().current_cipher().unwrap().name(),
                            *client_cipher,
                            "{source}: {label}"
                        );
                    }
                    runtime.block_on(callback.handshake_complete_callback(server.ssl()));
                }
                drop(server);
            });
            assert_eq!(entries.len(), 1, "{source}: {label} must emit one audit");
            assert_eq!(
                entries[0].handshake_complete_time.is_some(),
                *expected,
                "{source}: {label}"
            );
        }
    }
}

/// Two EdgionTls resources sharing one serving identity on one listener: a
/// one-way TLS host and a `Mutual` host, plus a client identity issued by the
/// Mutual host's CA.
struct ResumptionFixture {
    public: Arc<CompiledEdgionTls>,
    admin: Arc<CompiledEdgionTls>,
    client_cert: boring::x509::X509,
    client_key: boring::pkey::PKey<boring::pkey::Private>,
    ca_der: Vec<u8>,
}

fn resumption_fixture() -> ResumptionFixture {
    use edgion_resources::constants::secret_keys::tls::CA_CERT;
    use k8s_openapi::api::core::v1::Secret;
    use k8s_openapi::ByteString;
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
    use std::collections::BTreeMap;

    let server_key = KeyPair::generate().unwrap();
    let chain = serving_chain("resume", &server_key);
    let base = edgion_tls_identity(chain.chain_pem, server_key.serialize_pem());

    let mut public = (*base).clone();
    public.metadata.name = Some("public".to_string());
    public.spec.hosts = vec!["public.example.com".to_string()];
    public.spec.client_auth = None;

    let ca_key = KeyPair::generate().unwrap();
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "resume-client-ca");
    let ca_issuer = Issuer::from_params(&ca_params, &ca_key);
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let client_key = KeyPair::generate().unwrap();
    let client = CertificateParams::new(vec!["client.example.com".to_string()])
        .unwrap()
        .signed_by(&client_key, &ca_issuer)
        .unwrap();

    let mut admin = (*base).clone();
    admin.metadata.name = Some("admin".to_string());
    admin.spec.hosts = vec!["admin.example.com".to_string()];
    admin.spec.client_auth = Some(ClientAuthConfig {
        mode: ClientAuthMode::Mutual,
        ca_secret_ref: Some(SecretObjectReference {
            group: None,
            kind: None,
            name: "client-ca".to_string(),
            namespace: None,
        }),
        verify_depth: 1,
        ca_secret: Some(Secret {
            metadata: ObjectMeta {
                name: Some("client-ca".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            data: Some(BTreeMap::from([(
                CA_CERT.to_string(),
                ByteString(ca.pem().into_bytes()),
            )])),
            ..Default::default()
        }),
        allowed_sans: None,
        allowed_cns: None,
    });

    let public = compiled(Arc::new(public));
    let admin = compiled(Arc::new(admin));
    assert!(public.identity.is_ok() && admin.identity.is_ok());
    assert!(matches!(admin.client_ca_store, Ok(Some(_))));
    ResumptionFixture {
        public,
        admin,
        client_cert: boring::x509::X509::from_der(client.der()).unwrap(),
        client_key: boring::pkey::PKey::private_key_from_der(&client_key.serialize_der()).unwrap(),
        ca_der: ca.der().to_vec(),
    }
}

/// Outcome of one in-memory handshake against the shared server context.
struct ResumptionOutcome {
    completed: bool,
    resumed: bool,
}

/// Drive one in-memory handshake through the shared server `SslContext`,
/// with the real certificate callback. `offer` is the session the client tries
/// to resume, regardless of SNI (an attacker controls its own ClientHello).
/// Newly issued sessions are pushed into `issued`. With `tickets == false` the
/// client refuses tickets, so TLS 1.2 resumes through the server session-ID
/// cache instead.
#[allow(clippy::too_many_arguments)]
fn resumption_handshake(
    runtime: &tokio::runtime::Runtime,
    server_context: &boring::ssl::SslContext,
    version: boring::ssl::SslVersion,
    tickets: bool,
    sni: &str,
    offer: Option<&boring::ssl::SslSession>,
    identity: Option<(&boring::x509::X509, &boring::pkey::PKey<boring::pkey::Private>)>,
    issued: &Arc<parking_lot::Mutex<Vec<boring::ssl::SslSession>>>,
) -> ResumptionOutcome {
    use boring::ssl::{
        ErrorCode, Ssl, SslContext, SslMethod, SslOptions, SslSessionCacheMode, SslStream, SslVerifyMode,
    };
    use foreign_types::ForeignTypeRef;
    use pingora_core::listeners::TlsAccept;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io::{self, Read, Write};
    use std::rc::Rc;

    struct MemoryStream {
        incoming: Rc<RefCell<VecDeque<u8>>>,
        outgoing: Rc<RefCell<VecDeque<u8>>>,
    }
    impl Read for MemoryStream {
        fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
            let mut incoming = self.incoming.borrow_mut();
            if incoming.is_empty() && !target.is_empty() {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let count = target.len().min(incoming.len());
            for slot in &mut target[..count] {
                *slot = incoming.pop_front().unwrap();
            }
            Ok(count)
        }
    }
    impl Write for MemoryStream {
        fn write(&mut self, source: &[u8]) -> io::Result<usize> {
            self.outgoing.borrow_mut().extend(source);
            Ok(source.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    unsafe extern "C" fn pause_certificate(_: *mut boring_sys::SSL, _: *mut std::ffi::c_void) -> std::ffi::c_int {
        -1
    }

    let server_ssl = Ssl::new(server_context).unwrap();
    unsafe { boring_sys::SSL_set_cert_cb(server_ssl.as_ptr(), Some(pause_certificate), std::ptr::null_mut()) };

    let mut client = SslContext::builder(SslMethod::tls()).unwrap();
    client.set_min_proto_version(Some(version)).unwrap();
    client.set_max_proto_version(Some(version)).unwrap();
    client.set_verify(SslVerifyMode::NONE);
    client.set_session_cache_mode(SslSessionCacheMode::CLIENT);
    if !tickets {
        client.set_options(SslOptions::NO_TICKET);
    }
    let sink = issued.clone();
    client.set_new_session_callback(move |_, session| sink.lock().push(session));
    if let Some((certificate, key)) = identity {
        client.set_certificate(certificate).unwrap();
        client.set_private_key(key).unwrap();
    }
    let mut client_ssl = Ssl::new(&client.build()).unwrap();
    client_ssl.set_hostname(sni).unwrap();
    if let Some(session) = offer {
        unsafe { client_ssl.set_session(session).unwrap() };
    }

    let a = Rc::new(RefCell::new(VecDeque::new()));
    let b = Rc::new(RefCell::new(VecDeque::new()));
    let mut server = SslStream::new(
        server_ssl,
        MemoryStream {
            incoming: a.clone(),
            outgoing: b.clone(),
        },
    )
    .unwrap();
    let mut client = SslStream::new(
        client_ssl,
        MemoryStream {
            incoming: b,
            outgoing: a,
        },
    )
    .unwrap();
    let callback = TlsCallback::new(443);
    let (mut client_done, mut server_done, mut failed) = (false, false, false);
    for _ in 0..32 {
        if !client_done {
            match client.connect() {
                Ok(()) => client_done = true,
                Err(error) if error.code() == ErrorCode::WANT_READ => {}
                Err(_) => failed = true,
            }
        }
        if !server_done && !failed {
            match server.accept() {
                Ok(()) => server_done = true,
                Err(error) if error.code() == ErrorCode::WANT_READ => {}
                Err(error) if error.code() == ErrorCode::WANT_X509_LOOKUP => {
                    runtime.block_on(callback.certificate_callback(server.ssl_mut()));
                    unsafe { boring_sys::SSL_set_cert_cb(server.ssl().as_ptr(), None, std::ptr::null_mut()) };
                }
                Err(_) => failed = true,
            }
        }
        if failed || (client_done && server_done) {
            break;
        }
    }
    let completed = !failed && client_done && server_done;
    let resumed = completed && server.ssl().session_reused();
    if completed {
        runtime.block_on(callback.handshake_complete_callback(server.ssl()));
        // BoringSSL defers TLS 1.3 tickets until the server's first write, so
        // write one byte and let the client read past the ticket records.
        server.ssl_write(b"x").unwrap();
        let mut scratch = [0u8; 1];
        assert_eq!(client.ssl_read(&mut scratch).unwrap(), 1);
    }
    ResumptionOutcome { completed, resumed }
}

/// Shared server context whose select callback maps SNI to a prepared
/// selection and then runs the real `apply_selected_downstream_policy`.
fn isolation_server(selections: Vec<(&'static str, SslCtx)>) -> boring::ssl::SslContext {
    use crate::tls::downstream_policy::DownstreamTlsPolicy;
    use boring::ssl::{NameType, SelectCertError, SslContext, SslMethod};

    let policy = Arc::new(DownstreamTlsPolicy::compile(None));
    let mut server = SslContext::builder(SslMethod::tls()).unwrap();
    server.set_select_certificate_callback(move |mut hello| {
        let sni = hello.servername(NameType::HOST_NAME).ok_or(SelectCertError::ERROR)?;
        let selected = selections
            .iter()
            .find(|(host, _)| *host == sni)
            .map(|(_, selected)| selected.clone())
            .ok_or(SelectCertError::ERROR)?;
        if super::apply_selected_downstream_policy(hello.ssl_mut(), &selected, &policy, false).is_err() {
            return Err(SelectCertError::ERROR);
        }
        if !super::ssl_ctx_ex_data::store_ssl_ctx(hello.ssl_mut(), &selected) {
            return Err(SelectCertError::ERROR);
        }
        Ok(())
    });
    server.build()
}

/// For TLS 1.2 tickets, TLS 1.2 session-ID cache and TLS 1.3 PSK: a session
/// from the one-way `public` host resumes on itself (control), never on the
/// mTLS `admin` host without a certificate (the full handshake then fails
/// closed), and a legitimate client still completes in full. The admin session
/// resumes on its own policy but never on the `public` host, and no attempt
/// fails because of session context selection or installation.
fn assert_cross_policy_isolation(
    runtime: &tokio::runtime::Runtime,
    label: &str,
    public: SslCtx,
    admin: SslCtx,
    fixture: &ResumptionFixture,
) {
    use boring::ssl::SslVersion;

    for (version, tickets) in [
        (SslVersion::TLS1_2, true),
        (SslVersion::TLS1_2, false),
        (SslVersion::TLS1_3, true),
    ] {
        let case = format!("{label} {version:?} tickets={tickets}");
        let entries = crate::observe::logs::ssl_log::tests::capture(|| {
            let server = isolation_server(vec![
                ("public.example.com", public.clone()),
                ("admin.example.com", admin.clone()),
            ]);
            let issued = Arc::new(parking_lot::Mutex::new(Vec::new()));
            let run = |sni: &str, offer: Option<&boring::ssl::SslSession>, identity| {
                resumption_handshake(runtime, &server, version, tickets, sni, offer, identity, &issued)
            };

            let first = run("public.example.com", None, None);
            assert!(first.completed && !first.resumed, "{case}: public full handshake");
            let session = issued.lock().last().cloned().expect("public host must issue a session");

            // Control: the same policy resumes, so the probe is not vacuous.
            let same = run("public.example.com", Some(&session), None);
            assert!(same.completed && same.resumed, "{case}: same-policy resumption");

            // Attack: the public session offered under the mTLS host's SNI.
            let cross = run("admin.example.com", Some(&session), None);
            assert!(!cross.resumed, "{case}: cross-policy session must not resume");
            assert!(!cross.completed, "{case}: mTLS host must demand a certificate");

            let identity = Some((&fixture.client_cert, &fixture.client_key));
            let legit = run("admin.example.com", Some(&session), identity);
            assert!(legit.completed && !legit.resumed, "{case}: mTLS full handshake");
            let admin_session = issued.lock().last().cloned().expect("admin host must issue a session");

            // The mTLS policy keeps resumption for its own sessions.
            let admin_again = run("admin.example.com", Some(&admin_session), identity);
            assert!(admin_again.completed && admin_again.resumed, "{case}: mTLS resumption");

            // Reverse direction: an mTLS session does not resume on the public host.
            let reverse = run("public.example.com", Some(&admin_session), None);
            assert!(reverse.completed && !reverse.resumed, "{case}: reverse cross-policy");
        });
        for entry in &entries {
            let reason = entry.err_log.as_deref();
            assert!(
                !matches!(reason, Some("session context" | "session context selection")),
                "{case}: unexpected session context failure: {reason:?}"
            );
        }
    }
}

/// gw-infra-01: EdgionTls selections sharing one serving identity must not
/// share sessions across client-auth policies.
#[test]
fn session_resumption_is_isolated_per_selected_policy() {
    let _guard = HANDSHAKE_TEST_LOCK.lock();
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let fixture = resumption_fixture();
    let select = |identity: &Arc<CompiledEdgionTls>| {
        let mut selected = SslCtx::new(443);
        selected.meta.is_mtls = identity.tls.is_mtls_enabled();
        selected.matched_edgion_tls = Some(identity.clone());
        selected
    };
    assert_cross_policy_isolation(
        &runtime,
        "EdgionTls",
        select(&fixture.public),
        select(&fixture.admin),
        &fixture,
    );
}

/// gw-infra-01: two Gateway entries with the same attachment and serving
/// certificate that differ only in the frontend client-auth decision (none vs
/// strict `frontendValidation`) must not share sessions.
#[test]
fn gateway_frontend_validation_sessions_are_isolated() {
    use pingora_core::tls::x509::store::X509StoreBuilder;

    let _guard = HANDSHAKE_TEST_LOCK.lock();
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let fixture = resumption_fixture();
    let identity = fixture.public.identity.as_ref().unwrap().clone();
    let gateway = |outcome: FrontendMtlsOutcome| {
        let (_, mut entry) = overlay_fixture("winner", false);
        entry.certificate_refs = vec![fixture.public.tls.spec.secret_ref.clone()];
        entry.compiled_serving_certificates = Some(Arc::from(vec![identity.clone()]));
        entry.frontend_mtls = outcome;
        let mut ca_digest = None;
        if outcome == FrontendMtlsOutcome::StrictEffective {
            let mut store = X509StoreBuilder::new().unwrap();
            store
                .add_cert(boring::x509::X509::from_der(&fixture.ca_der).unwrap())
                .unwrap();
            entry.compiled_frontend_ca_store = Some(Arc::new(store.build()));
            ca_digest = Some(der_list_digest([fixture.ca_der.as_slice()].into_iter()));
        }
        entry.refresh_session_context_for_test(ca_digest.as_ref());
        let mut selected = SslCtx::new(443);
        selected.matched_gateway_tls = Some(Arc::new(entry));
        selected
    };
    assert_cross_policy_isolation(
        &runtime,
        "Gateway",
        gateway(FrontendMtlsOutcome::NotConfigured),
        gateway(FrontendMtlsOutcome::StrictEffective),
        &fixture,
    );
}

/// `apply_selected_downstream_policy` installs the id of the branch cert_cb
/// will apply for each shape select_cb produces, and an overlay pairing is
/// distinct from both of its parts.
#[test]
fn selected_session_context_follows_the_applied_branch() {
    use crate::tls::downstream_policy::DownstreamTlsPolicy;
    use boring::ssl::{Ssl, SslContext, SslMethod};
    use foreign_types::ForeignTypeRef;

    let fixture = resumption_fixture();
    let (_, mut gateway) = overlay_fixture("winner", false);
    gateway.frontend_mtls = FrontendMtlsOutcome::InsecureFallback;
    gateway.refresh_session_context_for_test(None);
    let gateway = Arc::new(gateway);
    let edgion_tls = fixture.public.clone();
    let overlay = SessionContextId::overlay(&gateway.session_context(), &edgion_tls.session_context);
    assert_ne!(overlay, gateway.session_context());
    assert_ne!(overlay, edgion_tls.session_context);
    assert_ne!(gateway.session_context(), edgion_tls.session_context);

    let mut native = SslCtx::new(443);
    native.matched_gateway_tls = Some(gateway.clone());
    let mut edgion = SslCtx::new(443);
    edgion.matched_edgion_tls = Some(edgion_tls.clone());
    let mut paired = native.clone();
    paired.gateway_edgion_tls_overlay = Some(edgion_tls.clone());
    let cases = [
        (native, gateway.session_context()),
        (edgion, edgion_tls.session_context),
        (paired, overlay),
    ];
    let policy = DownstreamTlsPolicy::compile(None);
    let context = SslContext::builder(SslMethod::tls()).unwrap().build();
    for (selected, expected) in cases {
        let mut ssl = Ssl::new(&context).unwrap();
        super::apply_selected_downstream_policy(&mut ssl, &selected, &policy, false).unwrap();
        let installed = unsafe {
            let mut length = 0usize;
            let pointer = boring_sys::SSL_get0_session_id_context(ssl.as_ptr(), &mut length);
            std::slice::from_raw_parts(pointer, length).to_vec()
        };
        assert_eq!(installed, expected.as_bytes());
    }
    // Shapes select_cb never produces fail closed.
    let mut orphan_overlay = SslCtx::new(443);
    orphan_overlay.gateway_edgion_tls_overlay = Some(edgion_tls.clone());
    orphan_overlay.matched_edgion_tls = Some(edgion_tls);
    for selected in [SslCtx::new(443), orphan_overlay] {
        let mut ssl = Ssl::new(&context).unwrap();
        assert_eq!(
            super::apply_selected_downstream_policy(&mut ssl, &selected, &policy, false),
            Err("session context selection")
        );
    }
}
