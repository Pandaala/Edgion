use super::*;
use serde_json::json;

#[test]
fn acl_removed_fields_presence_roundtrips_and_clean_recovers() {
    for side in ["allow", "deny"] {
        for value in [
            json!(null),
            json!([]),
            json!({"private": "discarded"}),
            json!([{"cidrs":["192.0.2.0/24"]}]),
        ] {
            let mut wire = json!({"allowRefs":[{"name":"clients"}],"invalidInput":false});
            wire[side] = value;
            let set: IpAclReferenceSet = serde_json::from_value(wire).unwrap();
            assert!(set.invalid_input);
            assert!(set.validation_error().is_some());
            let encoded = serde_json::to_value(&set).unwrap();
            assert!(encoded.get(side).is_none());
            assert_eq!(encoded["invalidInput"], true);
            let roundtrip: IpAclReferenceSet = serde_json::from_value(encoded).unwrap();
            assert!(roundtrip.invalid_input);
        }
    }
}

#[test]
fn reference_order_and_whitelist_mode_survive_transport() {
    let set: IpAclReferenceSet = serde_json::from_value(json!({
        "allowRefs":[{"name":"first"},{"name":"second","namespace":"other"}],
        "denyRefs":[{"name":"blocked"}]
    }))
    .unwrap();
    assert!(set.allow_mode());
    assert!(set.validation_error().is_none());
    let encoded = serde_json::to_value(&set).unwrap();
    assert_eq!(encoded["allowRefs"][0]["name"], "first");
    assert_eq!(encoded["allowRefs"][1]["name"], "second");
    assert!(encoded.get("resolvedAllowRefs").is_none());
}

#[test]
fn clean_unknown_fields_remain_warning_only_and_do_not_retain_payloads() {
    let set: IpAclReferenceSet = serde_json::from_value(json!({
        "denyRefs":[{"name":"blocked"}],"denys":{"discard":"payload"}
    }))
    .unwrap();
    assert!(!set.invalid_input);
    assert!(set.validation_error().is_none());
    assert!(serde_json::to_value(set).unwrap().get("denys").is_none());
}

#[test]
fn clean_generation_recovers_without_reusing_invalid_marker() {
    let invalid: IpAclReferenceSet =
        serde_json::from_value(json!({"allow":null,"allowRefs":[{"name":"clients"}]})).unwrap();
    let clean: IpAclReferenceSet = serde_json::from_value(json!({"allowRefs":[{"name":"clients"}]})).unwrap();
    assert!(invalid.validation_error().is_some());
    assert!(clean.validation_error().is_none());
}

#[test]
fn reference_only_acl_fixtures_decode_through_canonical_resource_types() {
    use crate::{EdgionConfigData, EdgionPlugins, EdgionStreamPlugins, EdgionTls, Gateway, HTTPRoute, TLSRoute};
    use serde::Deserialize;
    let paths=[
        "edgion-tests/integration/conf/EdgionPlugins/EdgionConfigDataStatus/00_EdgionConfigData_acl-data-status-gcir-invalid-allow-f12fd46ff5.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/EdgionConfigDataStatus/02_EdgionStreamPlugins_data-status-gcir-invalid.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/GeoRestriction/00_EdgionConfigData_acl-geo-chain-deny-93ed78e133.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/GeoRestriction/04_EdgionPlugins_geo_gac.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/IpRestriction/00_EdgionConfigData_acl-ip-restriction-allow-hit-allow-d955c7052a.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/IpRestriction/00_EdgionConfigData_acl-ip-restriction-allow-mis-allow-773e935e7f.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/IpRestriction/00_EdgionConfigData_acl-ip-restriction-deny-hit-deny-ea2773d774.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/IpRestriction/00_EdgionConfigData_acl-ip-restriction-deny-miss-deny-7360096093.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/IpRestriction/01_EdgionPlugins_allow_miss.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/IpRestriction/02_EdgionPlugins_allow_hit.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/IpRestriction/03_EdgionPlugins_deny_hit.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/IpRestriction/04_EdgionPlugins_deny_miss.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/IpRestrictionPartialGood/00_EdgionConfigData_acl-ip-restriction-inline-de-deny-3d58b7ea62.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/IpRestrictionPartialGood/00_EdgionConfigData_acl-ip-restriction-inline-de-deny-f1570dac16.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/IpRestrictionPartialGood/02_EdgionPlugins_inline_deny_hit.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/IpRestrictionPartialGood/02_EdgionPlugins_inline_deny_miss.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/ResolvedRefs/00_EdgionConfigData_acl-esp-gcir-data-exists-allow-7a62762bd4.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/ResolvedRefs/00_EdgionConfigData_acl-esp-gcir-data-missing-allow-8a2d9d3e29.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/ResolvedRefs/05_EdgionStreamPlugins_edgion-test_esp-gcir-data-exists.yaml",
        "edgion-tests/integration/conf/EdgionPlugins/ResolvedRefs/06_EdgionStreamPlugins_edgion-test_esp-gcir-data-missing.yaml",
        "edgion-tests/integration/conf/Gateway/StreamPlugins/00_EdgionConfigData_acl-allow-all-ips-allow-1746028f34.yaml",
        "edgion-tests/integration/conf/Gateway/StreamPlugins/00_EdgionConfigData_acl-deny-all-ips-deny-66b24144d6.yaml",
        "edgion-tests/integration/conf/Gateway/StreamPlugins/01_EdgionStreamPlugins.yaml",
        "edgion-tests/integration/conf/Gateway/StreamPlugins/02_EdgionStreamPlugins_allow.yaml",
        "edgion-tests/integration/conf/PluginPolicy/DynamicTest/initial/00_EdgionConfigData_acl-pp-stream-deny-79a0da5df8.yaml",
        "edgion-tests/integration/conf/PluginPolicy/DynamicTest/initial/00_EdgionConfigData_acl-pp-stream-deny-ec8783a4c8.yaml",
        "edgion-tests/integration/conf/PluginPolicy/DynamicTest/initial/EdgionStreamPlugins_pp-stream.yaml",
        "edgion-tests/integration/conf/Status/RefGrant/EdgionConfigData/00_EdgionConfigData_acl-ecd-allow-refs-cross-ns-deny-4e9f106305.yaml",
        "edgion-tests/integration/conf/Status/RefGrant/EdgionConfigData/05_EdgionPlugins_edgion-test_ecd-allow-refs-cross-ns.yaml",
        "edgion-tests/integration/conf/TCPRoute/GlobalConnectionIpRestriction/00_EdgionConfigData_acl-gcir-stream-plugins-allow-af75fff99c.yaml",
        "edgion-tests/integration/conf/TCPRoute/GlobalConnectionIpRestriction/00_EdgionConfigData_acl-gcir-stream-plugins-deny-59e25a3eea.yaml",
        "edgion-tests/integration/conf/TCPRoute/GlobalConnectionIpRestriction/02_EdgionStreamPlugins.yaml",
        "edgion-tests/integration/conf/TCPRoute/StreamPlugins/00_EdgionConfigData_acl-tcp-allow-all-allow-dbaa50dfd5.yaml",
        "edgion-tests/integration/conf/TCPRoute/StreamPlugins/00_EdgionConfigData_acl-tcp-allow-localhost-allow-71a00a9a1e.yaml",
        "edgion-tests/integration/conf/TCPRoute/StreamPlugins/02_EdgionStreamPlugins_allow.yaml",
        "edgion-tests/integration/conf/TCPRoute/StreamPlugins/02_EdgionStreamPlugins_allow_localhost.yaml",
        "edgion-tests/integration/conf/TCPRoute/StreamPlugins/DynamicTest/initial/00_EdgionConfigData_acl-tcp-sp-reload-esp-allow-2358b2f618.yaml",
        "edgion-tests/integration/conf/TCPRoute/StreamPlugins/DynamicTest/initial/EdgionStreamPlugins_edgion-test_tcp-sp-reload-esp.yaml",
        "edgion-tests/integration/conf/TCPRoute/StreamPlugins/DynamicTest/updates/to_allow/EdgionStreamPlugins_edgion-test_tcp-sp-reload-esp.yaml",
        "edgion-tests/integration/conf/TLSRoute/GeoRestriction/00_EdgionConfigData_acl-tls-geo-deny-deny-b8fb957b20.yaml",
        "edgion-tests/integration/conf/TLSRoute/GeoRestriction/01_EdgionStreamPlugins_geo_deny.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/00_EdgionConfigData_acl-tls-allow-localhost-allow-741d74d152.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/00_EdgionConfigData_acl-tls-deny-all-deny-dd12b47039.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/00_EdgionConfigData_acl-tls-route-deny-all-deny-4353ab26de.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/01_EdgionStreamPlugins_deny.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/02_EdgionStreamPlugins_allow_localhost.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/06_EdgionStreamPlugins_tls_route_deny.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/DynamicTest/initial/00_EdgionConfigData_acl-tls-sp-reload-allow-04118e969b.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/DynamicTest/initial/00_EdgionConfigData_acl-tls-sp-reload-deny-023cfd3052.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/DynamicTest/initial/00_EdgionConfigData_acl-tls-sp-reload-deny-28471a46e9.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/DynamicTest/initial/01_EdgionStreamPlugins.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/DynamicTest/updates/deny/EdgionStreamPlugins.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/DynamicTest/updates/disabled/EdgionStreamPlugins.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/DynamicTest/updates/empty/EdgionStreamPlugins.yaml",
        "edgion-tests/integration/conf/TLSRoute/StreamPlugins/DynamicTest/updates/stage1_only/EdgionStreamPlugins.yaml",
        "edgion-tests/integration/conf/UDPRoute/StreamPlugins/00_EdgionConfigData_acl-udp-allow-localhost-allow-1f567d51d3.yaml",
        "edgion-tests/integration/conf/UDPRoute/StreamPlugins/02_EdgionStreamPlugins_allow_localhost.yaml",
        "edgion-tests/integration/fixtures/unknown-config-fields/EdgionConfigData_edgion-test_acl-uf-ipr-allow.yaml",
        "edgion-tests/integration/fixtures/unknown-config-fields/EdgionConfigData_edgion-test_acl-uf-ipr-deny.yaml",
        "edgion-tests/integration/fixtures/unknown-config-fields/EdgionConfigData_edgion-test_acl-uf-sp-allow.yaml",
        "edgion-tests/integration/fixtures/unknown-config-fields/EdgionConfigData_edgion-test_acl-uf-sp-deny.yaml",
        "edgion-tests/integration/fixtures/unknown-config-fields/EdgionPlugins_edgion-test_uf-ipr.fixed.yaml",
        "edgion-tests/integration/fixtures/unknown-config-fields/EdgionPlugins_edgion-test_uf-ipr.yaml",
        "edgion-tests/integration/fixtures/unknown-config-fields/EdgionStreamPlugins_edgion-test_uf-sp.fixed.yaml",
        "edgion-tests/integration/fixtures/unknown-config-fields/EdgionStreamPlugins_edgion-test_uf-sp.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/EdgionConfigDataStatus/00_EdgionConfigData_acl-data-status-gcir-invalid-allow-f12fd46ff5.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/EdgionConfigDataStatus/02_EdgionStreamPlugins_data-status-gcir-invalid.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/GeoRestriction/00_EdgionConfigData_acl-geo-chain-deny-93ed78e133.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/GeoRestriction/resources.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/IpRestriction/00_EdgionConfigData_acl-ip-restriction-allow-mis-allow-065d3760ce.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/IpRestriction/00_EdgionConfigData_acl-ip-restriction-deny-miss-deny-a361c57bab.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/IpRestriction/01_EdgionPlugins_allow_miss.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/IpRestriction/04_EdgionPlugins_deny_miss.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/IpRestrictionPartialGood/00_EdgionConfigData_acl-ip-restriction-inline-de-deny-66dd1d77d6.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/IpRestrictionPartialGood/00_EdgionConfigData_acl-ip-restriction-inline-de-deny-f1570dac16.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/IpRestrictionPartialGood/02_EdgionPlugins_inline_deny_hit.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/IpRestrictionPartialGood/02_EdgionPlugins_inline_deny_miss.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/ResolvedRefs/00_EdgionConfigData_acl-esp-gcir-data-exists-allow-7a62762bd4.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/ResolvedRefs/00_EdgionConfigData_acl-esp-gcir-data-missing-allow-8a2d9d3e29.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/ResolvedRefs/05_EdgionStreamPlugins_edgion-test_esp-gcir-data-exists.yaml",
        "edgion-tests/k8s/conf/EdgionPlugins/ResolvedRefs/06_EdgionStreamPlugins_edgion-test_esp-gcir-data-missing.yaml",
        "edgion-tests/k8s/conf/Gateway/StreamPlugins/00_EdgionConfigData_acl-allow-all-ips-allow-976c8bc59f.yaml",
        "edgion-tests/k8s/conf/Gateway/StreamPlugins/00_EdgionConfigData_acl-deny-all-ips-deny-13f0a213bf.yaml",
        "edgion-tests/k8s/conf/Gateway/StreamPlugins/01_EdgionStreamPlugins.yaml",
        "edgion-tests/k8s/conf/Gateway/StreamPlugins/02_EdgionStreamPlugins_allow.yaml",
        "edgion-tests/k8s/conf/PluginPolicy/DynamicTest/initial/00_EdgionConfigData_acl-pp-stream-deny-79a0da5df8.yaml",
        "edgion-tests/k8s/conf/PluginPolicy/DynamicTest/initial/00_EdgionConfigData_acl-pp-stream-deny-ec8783a4c8.yaml",
        "edgion-tests/k8s/conf/PluginPolicy/DynamicTest/initial/EdgionStreamPlugins_pp-stream.yaml",
        "edgion-tests/k8s/conf/Status/RefGrant/EdgionConfigData/00_EdgionConfigData_acl-ecd-allow-refs-cross-ns-deny-4e9f106305.yaml",
        "edgion-tests/k8s/conf/Status/RefGrant/EdgionConfigData/resources.yaml",
        "edgion-tests/k8s/conf/TCPRoute/StreamPlugins/00_EdgionConfigData_acl-tcp-allow-all-allow-65220eb7b7.yaml",
        "edgion-tests/k8s/conf/TCPRoute/StreamPlugins/00_EdgionConfigData_acl-tcp-allow-localhost-allow-005960c787.yaml",
        "edgion-tests/k8s/conf/TCPRoute/StreamPlugins/00_EdgionConfigData_acl-tcp-deny-all-deny-72b2c23a10.yaml",
        "edgion-tests/k8s/conf/TCPRoute/StreamPlugins/01_EdgionStreamPlugins_deny.yaml",
        "edgion-tests/k8s/conf/TCPRoute/StreamPlugins/02_EdgionStreamPlugins_allow.yaml",
        "edgion-tests/k8s/conf/TCPRoute/StreamPlugins/02_EdgionStreamPlugins_allow_localhost.yaml",
        "edgion-tests/k8s/conf/TCPRoute/StreamPlugins/DynamicTest/initial/00_EdgionConfigData_acl-tcp-sp-reload-esp-allow-54ba6a1a02.yaml",
        "edgion-tests/k8s/conf/TCPRoute/StreamPlugins/DynamicTest/initial/EdgionStreamPlugins_edgion-test_tcp-sp-reload-esp.yaml",
        "edgion-tests/k8s/conf/TCPRoute/StreamPlugins/DynamicTest/updates/to_allow/EdgionStreamPlugins_edgion-test_tcp-sp-reload-esp.yaml",
        "edgion-tests/k8s/conf/TLSRoute/GeoRestriction/00_EdgionConfigData_acl-tls-geo-deny-deny-b8fb957b20.yaml",
        "edgion-tests/k8s/conf/TLSRoute/GeoRestriction/resources.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/00_EdgionConfigData_acl-tls-allow-localhost-allow-b39391c4f3.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/00_EdgionConfigData_acl-tls-deny-all-deny-2f9945393c.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/00_EdgionConfigData_acl-tls-route-deny-all-deny-4353ab26de.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/01_EdgionStreamPlugins_deny.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/02_EdgionStreamPlugins_allow_localhost.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/06_EdgionStreamPlugins_tls_route_deny.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/DynamicTest/initial/00_EdgionConfigData_acl-tls-sp-reload-allow-04118e969b.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/DynamicTest/initial/00_EdgionConfigData_acl-tls-sp-reload-deny-023cfd3052.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/DynamicTest/initial/00_EdgionConfigData_acl-tls-sp-reload-deny-28471a46e9.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/DynamicTest/initial/01_EdgionStreamPlugins.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/DynamicTest/updates/deny/EdgionStreamPlugins.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/DynamicTest/updates/disabled/EdgionStreamPlugins.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/DynamicTest/updates/empty/EdgionStreamPlugins.yaml",
        "edgion-tests/k8s/conf/TLSRoute/StreamPlugins/DynamicTest/updates/stage1_only/EdgionStreamPlugins.yaml",
    ];
    fn assert_reference_shapes(value: &serde_yaml::Value) {
        match value {
            serde_yaml::Value::Mapping(fields) => {
                for (key, child) in fields {
                    if matches!(key.as_str(), Some("allowRefs" | "denyRefs")) {
                        for reference in child.as_sequence().expect("ACL references must be an object array") {
                            assert!(reference.is_mapping());
                            assert!(reference["name"].as_str().is_some());
                            assert!(reference["namespace"].is_null() || reference["namespace"].as_str().is_some());
                        }
                    }
                    assert_reference_shapes(child);
                }
            }
            serde_yaml::Value::Sequence(values) => {
                for child in values {
                    assert_reference_shapes(child);
                }
            }
            _ => {}
        }
    }
    let mut documents = 0;
    let mut partial_targets = Vec::new();
    let mut invalid_policies = Vec::new();
    for path in paths {
        let input =
            std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(path)).unwrap();
        for document in serde_yaml::Deserializer::from_str(&input) {
            let value = serde_yaml::Value::deserialize(document).unwrap();
            if value.is_null() {
                continue;
            }
            assert_reference_shapes(&value);
            macro_rules! decode {
                ($ty:ty) => {{
                    let _: $ty =
                        serde_yaml::from_value(value.clone()).unwrap_or_else(|error| panic!("{path}: {error}"));
                }};
            }
            match value["kind"].as_str().expect("resource kind") {
                "EdgionConfigData" => {
                    let owner: EdgionConfigData = serde_yaml::from_value(value.clone()).unwrap();
                    if let crate::resources::edgion_config_data::ConfigEntry::IpList(data) = &owner.spec.data {
                        let (matcher, errors) =
                            crate::matcher::ip_cidr_helpers::build_grouped_matcher_partial(&data.items);
                        if !errors.is_empty() {
                            // Intentional malformed siblings remain target-local,
                            // with at least one valid entry compiled successfully.
                            assert!(
                                data.items
                                    .iter()
                                    .flat_map(|group| &group.cidrs)
                                    .filter_map(|cidr| cidr.split('/').next().unwrap().parse::<std::net::IpAddr>().ok())
                                    .any(|ip| matcher.contains(&ip)),
                                "{path}: partial-good target lost valid entries"
                            );
                            assert_eq!(errors.len(), 1, "{path}: unexpected target diagnostics");
                            assert!(owner.validate_pre_parse().is_empty());
                            partial_targets.push(path);
                        }
                    }
                }
                "EdgionPlugins" => {
                    let owner: EdgionPlugins = serde_yaml::from_value(value.clone()).unwrap();
                    let errors = owner.validate_plugins();
                    if !errors.is_empty() {
                        invalid_policies.push((path, owner.metadata.name.unwrap(), errors));
                    }
                }
                "EdgionStreamPlugins" => {
                    use crate::resources::edgion_plugins::PluginValidation;
                    use crate::resources::edgion_stream_plugins::EdgionStreamPlugin;
                    let owner: EdgionStreamPlugins = serde_yaml::from_value(value.clone()).unwrap();
                    for entry in owner.spec.plugins.iter().flatten() {
                        let error = match &entry.plugin {
                            EdgionStreamPlugin::IpRestriction(cfg) => cfg.validation_error(),
                            EdgionStreamPlugin::GlobalConnectionIpRestriction(cfg) => cfg.validation_error(),
                            _ => None,
                        };
                        if let Some(error) = error {
                            invalid_policies.push((path, owner.metadata.name.clone().unwrap(), vec![error]));
                        }
                    }
                }
                "EdgionTls" => decode!(EdgionTls),
                "Gateway" => decode!(Gateway),
                "HTTPRoute" => decode!(HTTPRoute),
                "TLSRoute" => decode!(TLSRoute),
                kind => panic!("Unexpected fixture resource {kind}: {path}"),
            }
            documents += 1;
        }
    }
    assert_eq!(paths.len(), 115);
    assert_eq!(documents, 136);
    assert_eq!(partial_targets.len(), 4);
    assert_eq!(
        invalid_policies.len(),
        3,
        "unexpected static-invalid fixture: {invalid_policies:?}"
    );
    for (path, name, errors) in invalid_policies {
        match name.as_str() {
            "data-status-gcir-invalid" => {
                assert!(path.ends_with("02_EdgionStreamPlugins_data-status-gcir-invalid.yaml"));
                assert!(errors.iter().any(|error| error.contains("activeProfile 'missing'")));
            }
            "geo-rr-invalid" => {
                assert_eq!(
                    path,
                    "edgion-tests/k8s/conf/EdgionPlugins/GeoRestriction/resources.yaml"
                );
                assert!(errors.iter().any(|error| error.contains("failOpen is required")));
            }
            _ => panic!("Unexpected invalid fixture {path}: {name}"),
        }
    }
}
