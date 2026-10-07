use super::*;
use edgion_resources::resources::common::{ExternalHttpConnection, WebhookServiceRef, WebhookTarget};
use edgion_resources::resources::link_sys::webhook::WebhookServiceConfig;
use edgion_resources::resources::link_sys::LinkSysSpec;

fn fingerprint(owner: &LinkSys) -> RuntimeFingerprint {
    fingerprint_webhook(owner, &GlobalOutboundTlsSnapshot::from_configs([])).unwrap()
}

fn webhook_ls(port: i32, rv: Option<&str>) -> LinkSys {
    LinkSys {
        metadata: kube::core::ObjectMeta {
            namespace: Some("fp".to_string()),
            name: Some("wh".to_string()),
            resource_version: rv.map(str::to_string),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Webhook(WebhookServiceConfig {
                conn: ExternalHttpConnection {
                    target: WebhookTarget {
                        service: WebhookServiceRef {
                            name: Some("svc".to_string()),
                            namespace: Some("fp".to_string()),
                            port: Some(port),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            }),
        },
        status: None,
    }
}

#[test]
fn equal_port_matches() {
    let a = webhook_ls(8080, Some("1"));
    let b = webhook_ls(8080, Some("2"));
    assert!(fingerprint(&a).equal_runtime(&fingerprint(&b)));
}

#[test]
fn port_change_rebuilds() {
    let a = webhook_ls(8080, None);
    let b = webhook_ls(8081, None);
    assert!(!fingerprint(&a).equal_runtime(&fingerprint(&b)));
}

#[test]
fn absent_and_disabled_tls_have_equal_runtime_identity() {
    let absent = webhook_ls(8080, Some("1"));
    let mut disabled = webhook_ls(8080, Some("2"));
    let SystemConfig::Webhook(config) = &mut disabled.spec.config else {
        unreachable!()
    };
    config.conn.tls = Some(
        serde_json::from_value(serde_json::json!({
            "enabled":false,
            "validation":{"caCertificateRefs":[{"group":"foreign.example","kind":"Secret","name":"bad"}]},
            "resolvedCaCertificates":[{"metadata":{"name":"wrong","namespace":"wrong"}}]
        }))
        .unwrap(),
    );
    assert!(fingerprint(&absent).equal_runtime(&fingerprint(&disabled)));
}
