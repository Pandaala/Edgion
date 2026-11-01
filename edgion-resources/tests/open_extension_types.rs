//! Unknown extension types stay readable, invalid, and free of opaque payloads.
use edgion_resources::resources::edgion_config_data::{ConfigEntry, EdgionConfigData};
use edgion_resources::resources::link_sys::{LinkSys, SystemConfig, SystemType};
use serde_json::{json, Value};

fn resource(kind: &str, spec: Value) -> Value {
    json!({"apiVersion":"edgion.io/v1","kind":kind,"metadata":{"name":"extension","namespace":"default"},"spec":spec})
}

#[test]
fn open_extensions_decode_lists_watch_and_wire_with_both_ingest_modes() {
    // This integration target owns its process-global ingest flag; no other test
    // in this binary changes it. Restore even if an assertion unwinds.
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            edgion_resources::schema_check::set_strict_ingest(self.0);
        }
    }
    let _restore = Restore(edgion_resources::schema_check::strict_ingest_enabled());
    for strict in [false, true] {
        edgion_resources::schema_check::set_strict_ingest(strict);
        let unknown = json!({"config":{"password":"discard-this-secret","nested":[{"token":"discard-this-secret"}]},"type":"future-secret-name"});
        let link = resource("LinkSys", unknown.clone());
        let data = resource("EdgionConfigData", json!({"enable":false,"data":unknown}));
        let link_list = json!({"apiVersion":"edgion.io/v1","kind":"LinkSysList","metadata":{},"items":[resource("LinkSys",json!({"type":"redis","config":{"endpoints":["redis://localhost:6379"]}})),link.clone()]});
        let data_list = json!({"apiVersion":"edgion.io/v1","kind":"EdgionConfigDataList","metadata":{},"items":[resource("EdgionConfigData",json!({"data":{"type":"Selector","config":{"active":"base"}}})),data.clone()]});
        for yaml in [false, true] {
            let links: kube::core::ObjectList<LinkSys> = if yaml {
                serde_yaml::from_str(&serde_yaml::to_string(&link_list).unwrap()).unwrap()
            } else {
                serde_json::from_value(link_list.clone()).unwrap()
            };
            let datas: kube::core::ObjectList<EdgionConfigData> = if yaml {
                serde_yaml::from_str(&serde_yaml::to_string(&data_list).unwrap()).unwrap()
            } else {
                serde_json::from_value(data_list.clone()).unwrap()
            };
            assert_eq!(links.items.len(), 2);
            assert_eq!(datas.items.len(), 2);
            assert!(links.items[0].validate_config().is_empty());
            assert!(datas.items[0].validate_pre_parse().is_empty());
            let ls = &links.items[1];
            let cd = &datas.items[1];
            assert!(matches!(ls.spec.config, SystemConfig::Unsupported));
            assert_eq!(ls.spec.config.system_type(), SystemType::Unsupported);
            assert!(matches!(cd.spec.data, ConfigEntry::Unsupported));
            assert_eq!(
                ls.validate_config(),
                ["Unsupported spec.type; use a supported type or upgrade Edgion"]
            );
            assert_eq!(
                cd.validate_pre_parse(),
                ["Unsupported spec.data.type; use a supported type or upgrade Edgion"]
            );
            assert!(
                ls.spec.unknown_fields.is_empty(),
                "unknown extension payload is not a known schema"
            );
            assert!(cd.spec.unknown_fields.is_empty());
            assert_eq!(
                serde_json::to_value(&ls.spec.config).unwrap(),
                json!({"type":"unsupported","config":{}})
            );
            assert_eq!(
                serde_json::to_value(&cd.spec.data).unwrap(),
                json!({"type":"Unsupported","config":{}})
            );
            for value in [serde_json::to_value(ls).unwrap(), serde_json::to_value(cd).unwrap()] {
                let wire = value.to_string();
                assert!(!wire.contains("discard-this-secret"));
                assert!(!wire.contains("future-secret-name"));
                assert!(!wire.contains("password"));
            }
            let ls: LinkSys = serde_json::from_value(serde_json::to_value(ls).unwrap()).unwrap();
            let cd: EdgionConfigData = serde_json::from_value(serde_json::to_value(cd).unwrap()).unwrap();
            assert!(!ls.validate_config().is_empty());
            assert!(!cd.validate_pre_parse().is_empty());
        }
        for event_type in ["ADDED", "MODIFIED"] {
            let event = json!({"type":event_type,"object":link});
            let _: kube::core::WatchEvent<LinkSys> = serde_json::from_value(event.clone()).unwrap();
            let _: kube::core::WatchEvent<LinkSys> =
                serde_yaml::from_str(&serde_yaml::to_string(&event).unwrap()).unwrap();
            let event = json!({"type":event_type,"object":data});
            let _: kube::core::WatchEvent<EdgionConfigData> = serde_json::from_value(event.clone()).unwrap();
            let _: kube::core::WatchEvent<EdgionConfigData> =
                serde_yaml::from_str(&serde_yaml::to_string(&event).unwrap()).unwrap();
        }
        for spec in [
            json!({"config":{}}),
            json!({"type":5,"config":{}}),
            json!({"type":"redis","config":{"endpoints":42}}),
            json!({"type":"redis","config":{"endpoints":[],"auth":{"secretRef":{"name":"auth"},"password":"inline"}}}),
        ] {
            assert!(serde_json::from_value::<LinkSys>(resource("LinkSys", spec)).is_err());
        }
        for data in [
            json!({"config":{}}),
            json!({"type":5,"config":{}}),
            json!({"type":"KeyList","config":{"matchMode":"future","items":[]}}),
            json!({"type":"WafRuleBundle","config":{}}),
            json!({"type":"ProxyProtocolTrust","config":{"mode":"future"}}),
        ] {
            assert!(
                serde_json::from_value::<EdgionConfigData>(resource("EdgionConfigData", json!({"data":data}))).is_err()
            );
        }
    }
}

#[test]
fn extension_sentinels_are_schema_hidden_and_crds_preserve_open_contract() {
    for schema in [
        serde_json::to_value(schemars::schema_for!(SystemConfig)).unwrap(),
        serde_json::to_value(schemars::schema_for!(ConfigEntry)).unwrap(),
    ] {
        let variants = schema["oneOf"].as_array().unwrap();
        assert!(variants.len() >= 8);
        assert!(variants.iter().all(|v| !matches!(
            v["properties"]["type"]["const"].as_str(),
            Some("Unsupported" | "unsupported")
        )));
    }
    for (filename, nested) in [("link_sys_crd.yaml", false), ("edgion_config_data_crd.yaml", true)] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../config/crd/edgion-crd")
            .join(filename);
        let doc: Value = serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for version in doc["spec"]["versions"].as_array().unwrap() {
            let spec = &version["schema"]["openAPIV3Schema"]["properties"]["spec"];
            let parent = if nested { &spec["properties"]["data"] } else { spec };
            let mut tag = parent["properties"]["type"].clone();
            tag.as_object_mut().unwrap().remove("description");
            assert_eq!(tag, json!({"type":"string"}));
            let required = parent["required"].as_array().unwrap();
            assert!(required.contains(&json!("type")) && required.contains(&json!("config")));
            assert_eq!(
                parent["properties"]["config"]["x-kubernetes-preserve-unknown-fields"],
                true
            );
        }
    }
}

#[test]
fn known_extension_serializers_preserve_every_tag_and_concrete_config() {
    use std::collections::BTreeSet;
    let mut link_tags = BTreeSet::new();
    let mut data_tags = BTreeSet::new();
    macro_rules! check {
        ($enum:ident, $variant:ident, $tag:literal, $config:expr, $tags:ident) => {{
            let config = serde_json::from_value($config).unwrap();
            let expected_config = serde_json::to_value(&config).unwrap();
            let value = $enum::$variant(config);
            let wire = serde_json::to_value(&value).unwrap();
            assert_eq!(wire, json!({"type":$tag,"config":expected_config}));
            let roundtrip: $enum = serde_json::from_value(wire.clone()).unwrap();
            assert!(matches!(roundtrip, $enum::$variant(_)));
            assert_eq!(serde_json::to_value(roundtrip).unwrap(),wire);
            $tags.insert($tag.to_string());
        }};
    }
    check!(
        SystemConfig,
        Redis,
        "redis",
        json!({"endpoints":["redis://localhost:6379"]}),
        link_tags
    );
    check!(
        SystemConfig,
        Etcd,
        "etcd",
        json!({"endpoints":["http://localhost:2379"]}),
        link_tags
    );
    check!(
        SystemConfig,
        Elasticsearch,
        "elasticsearch",
        json!({"endpoints":["http://localhost:9200"]}),
        link_tags
    );
    check!(
        SystemConfig,
        Kafka,
        "kafka",
        json!({"brokers":["localhost:9092"]}),
        link_tags
    );
    check!(
        SystemConfig,
        Otlp,
        "otlp",
        json!({"endpoint":"http://localhost:4317"}),
        link_tags
    );
    check!(
        SystemConfig,
        Webhook,
        "webhook",
        json!({"target":{"uri":"http://example.com/"}}),
        link_tags
    );
    check!(SystemConfig, HttpDns, "httpdns", json!({"preset":"tencent"}), link_tags);
    check!(
        SystemConfig,
        CredentialSource,
        "credentialSource",
        json!({"provider":{"type":"oauth2ClientCredentials","tokenEndpoint":"https://issuer.example/token","clientAuthentication":{"activeSecretRef":{"name":"bootstrap"}}}}),
        link_tags
    );
    check!(ConfigEntry, KeyList, "KeyList", json!({"items":[]}), data_tags);
    check!(ConfigEntry, IpList, "IpList", json!({"items":[]}), data_tags);
    check!(
        ConfigEntry,
        RequestAccessUrlAllowList,
        "RequestAccessUrlAllowList",
        json!({"items":[]}),
        data_tags
    );
    check!(
        ConfigEntry,
        ProxyProtocolTrust,
        "ProxyProtocolTrust",
        json!({"mode":"required"}),
        data_tags
    );
    check!(ConfigEntry, Selector, "Selector", json!({"active":"blue"}), data_tags);
    check!(
        ConfigEntry,
        ClusterRegionSetting,
        "ClusterRegionSetting",
        json!({"clusterName":"prod","regionName":"local","regions":[]}),
        data_tags
    );
    check!(
        ConfigEntry,
        WafRuleBundle,
        "WafRuleBundle",
        json!({"version":"1","profile":"test","provenance":"test/revision","roots":["main.conf"],"rules":[{"name":"main.conf","content":"SecAction \"id:1,phase:1,pass\""}]}),
        data_tags
    );
    check!(
        ConfigEntry,
        WafPolicy,
        "WafPolicy",
        json!({"profiles":{"base":{"bundleRefs":[{"name":"rules"}]}},"defaultProfile":"base"}),
        data_tags
    );
    check!(
        ConfigEntry,
        Misc,
        "Misc",
        json!({"vendor":[true,42,"opaque"]}),
        data_tags
    );
    for (schema, tags) in [
        (
            serde_json::to_value(schemars::schema_for!(SystemConfig)).unwrap(),
            link_tags,
        ),
        (
            serde_json::to_value(schemars::schema_for!(ConfigEntry)).unwrap(),
            data_tags,
        ),
    ] {
        let supported: BTreeSet<String> = schema["oneOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|variant| variant["properties"]["type"]["const"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            supported, tags,
            "every supported variant must have a wire-shape regression"
        );
    }
}

#[test]
fn extension_envelope_preserves_concrete_config_redaction() {
    let content = "SecAction \"id:1,phase:1,pass\"";
    let entry: ConfigEntry = serde_json::from_value(json!({"type":"WafRuleBundle","config":{
        "version":"1","profile":"test","provenance":"test/revision","roots":["main.conf"],
        "rules":[{"name":"main.conf","content":content}]
    }}))
    .unwrap();
    assert_eq!(
        serde_json::to_value(&entry).unwrap()["config"]["rules"][0]["content"],
        content
    );
    let _redact = edgion_resources::redact::AdminRedactGuard::new();
    let redacted = serde_json::to_value(&entry).unwrap();
    assert_eq!(redacted["type"], "WafRuleBundle");
    assert_eq!(redacted["config"]["rules"][0]["content"], "[redacted]");
}
