//! Real reqwest embedding of the Gateway's complete SAN-aware TLS verifier.
#[path = "../../edgion-common/tests/support/reqwest_tls_peer.rs"]
mod peer;
use edgion_common::link_sys_core::http_client::{build_outbound_client, OutboundClientConfig};
use edgion_common::link_sys_core::outbound_tls::{EffectiveCa, ResolvedOutboundTls};
use edgion_gateway::tls::validation::build_verified_outbound_tls;
use edgion_resources::resources::backend_tls_policy::{SubjectAltName, SubjectAltNameType};
use std::time::Duration;
#[tokio::test]
async fn reqwest_preserves_the_supplied_san_verifier_and_http2_alpn() {
    let material = peer::Material::new();
    for (name, allowed) in [("localhost", true), ("other.example", false)] {
        let peer = peer::Peer::start(&material, None).await;
        let resolved = ResolvedOutboundTls {
            verify: true,
            ca: EffectiveCa::Custom(vec![material.ca.as_bytes().to_vec()]),
            client_identity: None,
            san: None,
        };
        let config = build_verified_outbound_tls(
            &resolved,
            vec![SubjectAltName {
                san_type: SubjectAltNameType::Hostname,
                hostname: Some(name.into()),
                uri: None,
            }],
            "Test",
        )
        .unwrap();
        let client = build_outbound_client(OutboundClientConfig {
            preconfigured_tls: Some(config),
            disable_proxy: true,
            connect_timeout: Some(Duration::from_secs(2)),
            total_timeout: Some(Duration::from_secs(3)),
            ..Default::default()
        })
        .unwrap();
        let response = client.get(peer.url()).send().await;
        if allowed {
            let response = response.unwrap();
            assert_eq!(response.version(), reqwest::Version::HTTP_2);
            assert_eq!(response.bytes().await.unwrap().as_ref(), b"ok");
        } else {
            assert!(response.is_err());
        }
        drop(client);
        let observed = peer.finish().await;
        if allowed {
            let observed = observed.unwrap();
            assert_eq!(observed.alpn, b"h2");
            assert_eq!(observed.sni.as_deref(), Some("localhost"));
            assert_eq!(observed.client_certificates, 0);
        } else {
            assert!(observed.is_err());
        }
    }
}
