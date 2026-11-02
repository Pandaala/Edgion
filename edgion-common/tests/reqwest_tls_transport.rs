//! Actual canonical HTTP TLS policy against owned loopback peers, without product processes.
#[path = "support/reqwest_tls_peer.rs"]
mod peer;
use edgion_common::link_sys_core::http_client::{build_outbound_client, OutboundClientConfig};
use edgion_resources::resources::common::outbound_tls::OutboundTlsValidation;
use edgion_resources::resources::common::{CustomCACertificateRef, SecretObjectReference, TlsConfig};
use k8s_openapi::{api::core::v1::Secret, ByteString};
use peer::{Material, Peer};
use std::time::Duration;

fn tls(ca: &str, identity: Option<&Material>, verify: bool) -> TlsConfig {
    let mut tls = TlsConfig {
        enabled: true,
        ..Default::default()
    };
    tls.outbound.verify = Some(verify);
    tls.outbound.validation = Some(OutboundTlsValidation {
        ca_certificate_refs: Some(vec![CustomCACertificateRef {
            group: Some(String::new()),
            kind: "Secret".into(),
            name: "ca".into(),
            namespace: Some("test".into()),
        }]),
        ..Default::default()
    });
    tls.resolved_ca_certificates = Some(vec![Secret {
        metadata: kube::core::ObjectMeta {
            name: Some("ca".into()),
            namespace: Some("test".into()),
            ..Default::default()
        },
        data: Some([("ca.crt".into(), ByteString(ca.as_bytes().to_vec()))].into()),
        ..Default::default()
    }]);
    if let Some(identity) = identity {
        tls.outbound.client_certificate_ref = Some(SecretObjectReference {
            name: "client".into(),
            namespace: Some("test".into()),
            group: None,
            kind: None,
        });
        tls.resolved_client_certificate = Some(Secret {
            metadata: kube::core::ObjectMeta {
                name: Some("client".into()),
                namespace: Some("test".into()),
                ..Default::default()
            },
            data: Some(
                [
                    ("tls.crt".into(), ByteString(identity.cert.as_bytes().to_vec())),
                    ("tls.key".into(), ByteString(identity.key.as_bytes().to_vec())),
                ]
                .into(),
            ),
            ..Default::default()
        });
    }
    tls
}
fn client(tls: Option<TlsConfig>) -> reqwest::Client {
    build_outbound_client(OutboundClientConfig {
        tls,
        disable_proxy: true,
        connect_timeout: Some(Duration::from_secs(2)),
        total_timeout: Some(Duration::from_secs(3)),
        ..Default::default()
    })
    .unwrap()
}
async fn success(client: &reqwest::Client, peer: &Peer) {
    let response = client.get(peer.url()).send().await.unwrap();
    assert_eq!(response.version(), reqwest::Version::HTTP_2);
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"ok");
}
#[tokio::test]
async fn canonical_tls_preserves_trust_mtls_alpn_and_insecure_identity_omission() {
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    let first = Material::new();
    let second = Material::new();
    let a = Peer::start(&first, None).await;
    let b = Peer::start(&second, None).await;
    let roots = format!("{}\n{}", first.ca, second.ca);
    let verified = client(Some(tls(&roots, None, true)));
    success(&verified, &a).await;
    success(&verified, &b).await;
    drop(verified);
    for peer in [a, b] {
        let observed = peer.finish().await.unwrap();
        assert_eq!(observed.sni.as_deref(), Some("localhost"));
        assert_eq!(observed.alpn, b"h2");
        assert_eq!(observed.client_certificates, 0);
    }
    let a = Peer::start(&first, Some(true)).await;
    let verified = client(Some(tls(&first.ca, Some(&first), true)));
    success(&verified, &a).await;
    drop(verified);
    assert!(a.finish().await.unwrap().client_certificates > 0);
    for config in [None, Some(tls(&second.ca, None, true))] {
        let a = Peer::start(&first, None).await;
        let untrusted = client(config);
        assert!(untrusted.get(a.url()).send().await.is_err());
        drop(untrusted);
        assert!(a.finish().await.is_err());
    }
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    rustls::crypto::ring::default_provider().install_default().unwrap();
    let a = Peer::start(&first, Some(false)).await;
    let insecure = client(Some(tls(&second.ca, Some(&first), false)));
    success(&insecure, &a).await;
    drop(insecure);
    assert_eq!(a.finish().await.unwrap().client_certificates, 0);
}
