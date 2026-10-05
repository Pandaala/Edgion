use super::{
    install_compiled_serving_identity, pick_secret_index, select_gateway_edgion_tls_overlay, CompiledEdgionTls, SslCtx,
    TlsCallback,
};
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
    let gateway = GatewayTlsEntry {
        attachment: ResolvedListenerAttachment {
            gateway_namespace: "default".to_string(),
            gateway_name: "winner".to_string(),
            listener_name: "https".to_string(),
            listener_key,
            gateway_proof,
        },
        gateway_creation_ts: None,
        certificate_refs: Vec::new(),
        compiled_serving_certificates: None,
        compiled_frontend_ca_store: None,
        frontend_mtls: FrontendMtlsOutcome::StrictEffective,
    };
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
        assert!(san.compiled_regex.is_some(), "SAN must compile during {phase}");
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
        compiled_resource.operator_cipher.is_err(),
        "the cipher string must fail at publication, not at handshake"
    );

    let server_ctx = SslContext::builder(SslMethod::tls()).unwrap().build();
    let mut ssl = Ssl::new(&server_ctx).unwrap();
    let mut ssl_ctx = SslCtx::new(443);
    TlsCallback::new(443).apply_edgion_tls_cert(ssl.as_mut(), &compiled_resource, &mut ssl_ctx);

    let err_log = ssl_ctx.meta.err_log.as_deref().expect("cipher failure must be audited");
    assert!(
        err_log.starts_with("cipher config: "),
        "unexpected audit category: {err_log}"
    );
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
