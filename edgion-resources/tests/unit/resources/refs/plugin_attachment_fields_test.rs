use super::*;
use crate::resources::edgion_gateway_config::PluginReference;
use std::cell::Cell;
fn gateway(value: &str) -> Gateway {
    serde_json::from_value(serde_json::json!({"metadata":{"name":"source","annotations":{EDGION_PLUGINS:value}},"spec":{"gatewayClassName":"test","listeners":[]}})).unwrap()
}
#[test]
fn gateway_actual_fields_keep_order_default_and_dedup_before_target_checks() {
    let owner = gateway("first, default/first second other/third /bad bad/a/b");
    let entries = HttpPluginAttachments::Gateway(&owner).entries();
    assert_eq!(
        entries
            .iter()
            .filter_map(|r| r.as_ref().ok().map(PluginAttachmentKey::as_str))
            .collect::<Vec<_>>(),
        vec!["default/first", "default/second", "other/third"]
    );
    assert_eq!(entries.iter().filter(|r| r.is_err()).count(), 2);
    let reads = Cell::new(0);
    for entry in entries.into_iter().flatten() {
        let _ = super::super::plugin_target::classify::<()>(entry.as_str(), |_| {
            reads.set(reads.get() + 1);
            None
        });
    }
    assert_eq!(
        reads.get(),
        5,
        "only valid targets reach canonical lookup; default has two raw candidates"
    );
    assert!(
        owner.metadata.namespace.is_none(),
        "semantic default must not mutate raw owner"
    );
}
#[test]
fn empty_fields_do_not_validate_unreferenced_owner_and_bad_owner_never_yields_key() {
    for annotation in ["", " , \t\n", "valid"] {
        let mut owner = gateway(annotation);
        owner.metadata.name = None;
        let entries = HttpPluginAttachments::Gateway(&owner).entries();
        if annotation == "valid" {
            assert_eq!(entries, vec![Err(ReferenceError::InvalidName)]);
        } else {
            assert!(entries.is_empty());
        }
    }
    let mut owner = gateway("valid");
    owner.metadata.namespace = Some("".into());
    assert_eq!(
        HttpPluginAttachments::Gateway(&owner).entries(),
        vec![Err(ReferenceError::InvalidNamespace)]
    );
}
#[test]
fn egc_actual_fields_are_cluster_owned_preserve_duplicates_and_explicit_namespace() {
    let mut owner = EdgionGatewayConfig::new("config", Default::default());
    owner.spec.global_plugins_ref = Some(vec![
        PluginReference {
            name: "first".into(),
            namespace: None,
        },
        PluginReference {
            name: "first".into(),
            namespace: None,
        },
        PluginReference {
            name: "bad".into(),
            namespace: Some("".into()),
        },
    ]);
    let entries = HttpPluginAttachments::EgcGlobals(&owner).entries();
    assert_eq!(entries[0].as_ref().unwrap().as_str(), "default/first");
    assert_eq!(entries[0], entries[1]);
    assert_eq!(entries[2], Err(ReferenceError::InvalidNamespace));
    owner.metadata.namespace = Some("default".into());
    assert!(HttpPluginAttachments::EgcGlobals(&owner)
        .entries()
        .iter()
        .all(Result::is_err));
    owner.spec.global_plugins_ref = Some(vec![]);
    assert!(HttpPluginAttachments::EgcGlobals(&owner).entries().is_empty());
}
