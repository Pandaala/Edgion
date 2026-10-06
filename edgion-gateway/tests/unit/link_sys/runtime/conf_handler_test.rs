use super::*;
use edgion_resources::resources::common::{ExternalHttpConnection, WebhookServiceRef, WebhookTarget};
use edgion_resources::resources::link_sys::webhook::WebhookServiceConfig;
use edgion_resources::resources::link_sys::{LinkSysSpec, SystemConfig};

fn create_test_linksys(namespace: &str, name: &str) -> LinkSys {
    LinkSys {
        metadata: kube::core::ObjectMeta {
            namespace: Some(namespace.to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        },
        spec: LinkSysSpec {
            current_status: None,
            unknown_fields: Vec::new(),
            config: SystemConfig::Webhook(WebhookServiceConfig {
                conn: ExternalHttpConnection {
                    target: WebhookTarget {
                        service: WebhookServiceRef {
                            kind: None,
                            name: Some("resolve-svc".to_string()),
                            namespace: Some(namespace.to_string()),
                            port: Some(8080),
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
fn test_conf_handler_full_set() {
    let store = LinkSysStore::new();

    let mut data = HashMap::new();
    data.insert(
        "default/wh1".to_string(),
        Arc::new(create_test_linksys("default", "wh1")),
    );
    data.insert(
        "default/wh2".to_string(),
        Arc::new(create_test_linksys("default", "wh2")),
    );

    ConfHandler::full_set(&store, &data);

    assert!(store.contains("default/wh1"));
    assert!(store.contains("default/wh2"));
}

#[test]
fn test_conf_handler_partial_update() {
    let store = LinkSysStore::new();

    // Add initial data
    let mut data = HashMap::new();
    data.insert(
        "default/wh1".to_string(),
        Arc::new(create_test_linksys("default", "wh1")),
    );
    ConfHandler::full_set(&store, &data);

    // Partial update: add wh2, remove wh1
    let mut add = HashMap::new();
    add.insert(
        "default/wh2".to_string(),
        Arc::new(create_test_linksys("default", "wh2")),
    );

    let mut remove = HashSet::new();
    remove.insert("default/wh1".to_string());

    ConfHandler::partial_update(&store, add, HashMap::new(), remove);

    assert!(!store.contains("default/wh1"));
    assert!(store.contains("default/wh2"));
}
