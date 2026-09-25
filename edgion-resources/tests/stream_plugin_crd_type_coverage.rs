//! Stream plugin type names are validated by code, not a CRD whitelist.
use serde_json::Value;
use std::path::PathBuf;

#[test]
fn stream_crd_types_are_open_strings_in_both_stages() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/crd/edgion-crd/edgion_stream_plugins_crd.yaml");
    let doc: Value = serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let props = &doc["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]["properties"];
    for stage in ["plugins", "tlsRoutePlugins"] {
        let item = &props[stage]["items"];
        let mut discriminator = item["properties"]["type"].clone();
        discriminator.as_object_mut().unwrap().remove("description");
        assert_eq!(discriminator, serde_json::json!({"type":"string"}));
        assert_eq!(
            item["properties"]["config"]["x-kubernetes-preserve-unknown-fields"],
            true
        );
    }
}

#[test]
fn unsupported_stream_resource_does_not_poison_typed_lists() {
    use edgion_resources::resources::edgion_stream_plugins::EdgionStreamPlugins;
    for stage in ["plugins", "tlsRoutePlugins"] {
        let value = serde_json::json!({"apiVersion":"v1", "kind":"EdgionStreamPluginsList",
        "metadata":{"resourceVersion":"1"}, "items":[
            {"apiVersion":"edgion.io/v1", "kind":"EdgionStreamPlugins", "metadata":{"name":"bad"},
             "spec":{stage:[{"type":"FuturePlugin", "config":{"opaque":"discard"}}]}},
            {"apiVersion":"edgion.io/v1", "kind":"EdgionStreamPlugins", "metadata":{"name":"good"}, "spec":{}}
        ]});
        let list: kube::core::ObjectList<EdgionStreamPlugins> = serde_json::from_value(value).unwrap();
        assert_eq!(list.items.len(), 2);
        assert_eq!(
            list.items[0].unsupported_plugin_errors(),
            vec![format!("{stage}[0]: unsupported plugin type")]
        );
        assert!(list.items[1].unsupported_plugin_errors().is_empty());
    }
}
