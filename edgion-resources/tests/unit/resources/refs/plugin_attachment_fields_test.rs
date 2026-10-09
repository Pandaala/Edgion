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

#[test]
fn stream_actual_owners_preserve_finite_lexical_grammars_and_absence() {
    use super::{StreamAttachmentError, StreamPluginAttachment};
    for kind in ["Gateway", "TCPRoute", "TLSRoute", "UDPRoute"] {
        for (namespace, name) in [
            (None, Some("owner")),
            (Some("apps"), Some("owner")),
            (Some(""), Some("owner")),
            (Some("UPPER"), Some("owner")),
            (None, None),
            (None, Some("bad/name")),
        ] {
            for value in [
                None,
                Some(" \t "),
                Some(" plugin "),
                Some(" shared/plugin "),
                Some("a/b/c"),
                Some("/"),
                Some("UPPER name"),
            ] {
                let mut raw = serde_json::json!({"metadata":{"name":name,"namespace":namespace},"spec":{"gatewayClassName":"public"}});
                if let Some(value) = value {
                    raw["metadata"]["annotations"] = serde_json::json!({"edgion.io/edgion-stream-plugins":value});
                }
                let result = match kind {
                    "Gateway" => StreamPluginAttachment::Gateway(&serde_json::from_value(raw).unwrap()).key(),
                    "TCPRoute" => StreamPluginAttachment::Tcp(&serde_json::from_value(raw).unwrap()).key(),
                    "TLSRoute" => StreamPluginAttachment::Tls(&serde_json::from_value(raw).unwrap()).key(),
                    _ => StreamPluginAttachment::Udp(&serde_json::from_value(raw).unwrap()).key(),
                };
                let value = value.unwrap_or("").trim();
                if value.is_empty() {
                    assert_eq!(result, Ok(None));
                } else if namespace == Some("") || namespace == Some("UPPER") || name != Some("owner") {
                    assert!(matches!(result, Err(StreamAttachmentError::InvalidOwner(_))));
                } else if kind == "Gateway" && value.contains('/') {
                    assert_eq!(result, Err(StreamAttachmentError::QualifiedGatewayName));
                } else {
                    assert_eq!(
                        result,
                        Ok(Some(if value.contains('/') {
                            value.into()
                        } else {
                            format!("{}/{}", namespace.unwrap_or("default"), value)
                        }))
                    );
                }
            }
        }
    }
}
