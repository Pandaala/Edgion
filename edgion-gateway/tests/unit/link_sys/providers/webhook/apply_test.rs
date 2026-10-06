use super::*;
use edgion_resources::resources::common::{ExternalHttpConnection, WebhookServiceRef, WebhookTarget};
use edgion_resources::resources::link_sys::webhook::WebhookServiceConfig;
use edgion_resources::resources::link_sys::LinkSysSpec;

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
    assert!(fingerprint_webhook(&a)
        .unwrap()
        .equal_runtime(&fingerprint_webhook(&b).unwrap()));
}

#[test]
fn port_change_rebuilds() {
    let a = webhook_ls(8080, None);
    let b = webhook_ls(8081, None);
    assert!(!fingerprint_webhook(&a)
        .unwrap()
        .equal_runtime(&fingerprint_webhook(&b).unwrap()));
}
