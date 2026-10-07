use super::*;

fn tls() -> serde_json::Value {
    serde_json::json!({"enabled":true,"verify":false,
        "validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},"clientCertificateRef":{"name":"client"},
        "resolvedCaCertificates":[{"metadata":{"name":"ca","namespace":"apps"}}],
        "resolvedClientCertificate":{"metadata":{"name":"client","namespace":"apps"}}})
}

fn plugin(variant: &str, stage: &str, enabled: bool, tls: serde_json::Value) -> crate::EdgionPlugins {
    let config = match variant {
        "OpenidConnect" => serde_json::json!({"discovery":"https://issuer.example","clientId":"client",
            "tls":{"verify":tls["verify"],"validation":tls["validation"],"clientCertificateRef":tls["clientCertificateRef"]},
            "resolvedCaCertificates":tls["resolvedCaCertificates"],"resolvedClientCertificate":tls["resolvedClientCertificate"]}),
        "LdapAuth" => serde_json::json!({"ldapHost":"ldap.example","baseDn":"dc=example","attribute":"uid",
            "tls":{"verify":tls["verify"],"validation":tls["validation"],"clientCertificateRef":tls["clientCertificateRef"]},
            "resolvedCaCertificates":tls["resolvedCaCertificates"],"resolvedClientCertificate":tls["resolvedClientCertificate"]}),
        "ExtProc" => serde_json::json!({"grpcService":{"target":"processor.example:9000","tls":tls}}),
        "Wasm" => serde_json::json!({"source":{"url":"https://repo.example/module.wasm","fetch":{"tls":tls}}}),
        "ForwardAuth" => serde_json::json!({"target":{"url":"https://auth.example"},"decision":{},"tls":tls}),
        "Guardrail" => serde_json::json!({"target":{"url":"https://guard.example"},"request":{},"tls":tls}),
        "AiGuard" => {
            serde_json::json!({"rules":[],"evaluator":{"target":{"url":"https://guard.example"},"request":{},"tls":tls}})
        }
        _ => unreachable!(),
    };
    let mut wire =
        serde_json::json!({"metadata":{"name":"plugins","namespace":"apps","uid":"uid","generation":1},"spec":{}});
    wire["spec"][stage] = serde_json::json!([{"type":variant,"enable":enabled,"config":config}]);
    serde_json::from_value(wire).unwrap()
}

fn assert_pairs(field: BoundOutboundTls<'_>, kind: &str) {
    field.validate_source().unwrap();
    assert_eq!(field.policy().verify, Some(false));
    assert_eq!(field.ca_reference_count(), 1);
    let mut count = 0;
    field.visit_ca(|pair| {
        count += 1;
        let reference = pair.reference().validated().unwrap();
        assert_eq!(reference.source().group_kind().kind(), kind);
        assert_eq!(reference.target().name(), "ca");
        assert_eq!(pair.material().unwrap().metadata.name.as_deref(), Some("ca"));
    });
    assert_eq!(count, 1);
    let pair = field.client_certificate().unwrap();
    assert_eq!(pair.reference().validated().unwrap().target().name(), "client");
    assert_eq!(pair.material().unwrap().metadata.namespace.as_deref(), Some("apps"));
}

#[test]
fn every_plugin_tls_location_pairs_only_its_actual_active_field() {
    use crate::resources::refs::plugin_fields::PreparedPluginReferences;
    for variant in [
        "OpenidConnect",
        "LdapAuth",
        "ExtProc",
        "Wasm",
        "ForwardAuth",
        "Guardrail",
        "AiGuard",
    ] {
        let owner = plugin(variant, "requestPlugins", true, tls());
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let selection = entry.outbound_tls_selection().unwrap();
        selection.validate_source().unwrap();
        assert!(selection.active_field().is_some(), "{variant}");
        assert_pairs(entry.outbound_tls().unwrap().unwrap(), "EdgionPlugins");
        let owner = plugin(variant, "requestPlugins", false, tls());
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let selection = entry.outbound_tls_selection().unwrap();
        selection.validate_source().unwrap();
        assert!(selection.active_field().is_none(), "{variant}");
        assert!(
            binding
                .request_entries()
                .next()
                .unwrap()
                .outbound_tls()
                .unwrap()
                .is_none(),
            "{variant}"
        );
        let owner = plugin(variant, "upstreamResponsePlugins", true, tls());
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        assert!(
            binding
                .response_entries()
                .next()
                .unwrap()
                .outbound_tls()
                .unwrap()
                .is_none(),
            "{variant}"
        );
        if !matches!(variant, "OpenidConnect" | "LdapAuth") {
            let mut inactive = tls();
            inactive["enabled"] = serde_json::json!(false);
            let owner = plugin(variant, "requestPlugins", true, inactive);
            let prepared = PreparedPluginReferences::from_plugins(&owner);
            let binding = prepared.bind(&owner).unwrap();
            assert!(
                binding
                    .request_entries()
                    .next()
                    .unwrap()
                    .outbound_tls()
                    .unwrap()
                    .is_none(),
                "{variant}"
            );
        }
    }
}

#[test]
fn every_link_sys_provider_pairs_its_actual_tls_block_and_disabled_never_lends_it() {
    use crate::resources::refs::link_sys_fields::PreparedLinkSysReferences;
    for provider in [
        "redis",
        "etcd",
        "elasticsearch",
        "kafka",
        "otlp",
        "webhook",
        "httpdns",
        "credentialSource",
    ] {
        for enabled in [true, false] {
            let mut tls = tls();
            tls["enabled"] = serde_json::json!(enabled);
            let owner = link_sys(provider, tls);
            let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
            let binding = prepared.bind(&owner).unwrap();
            let field = binding.outbound_tls().unwrap();
            let selection = binding.outbound_tls_selection().unwrap();
            selection.validate_source().unwrap();
            assert_eq!(selection.active_field().is_some(), enabled, "{provider}");
            if enabled {
                assert_pairs(field.unwrap(), "LinkSys");
            } else {
                assert!(field.is_none(), "{provider}");
            }
        }
    }
}

fn link_sys(provider: &str, tls: serde_json::Value) -> crate::LinkSys {
    let config = match provider {
        "redis" | "etcd" | "elasticsearch" => serde_json::json!({"endpoints":["https://provider.example"],"tls":tls}),
        "kafka" => serde_json::json!({"brokers":["provider.example:9093"],"tls":tls}),
        "otlp" => serde_json::json!({"endpoint":"https://provider.example","tls":tls}),
        "webhook" => serde_json::json!({"target":{"url":"https://provider.example"},"tls":tls}),
        "httpdns" => {
            serde_json::json!({"urlTemplate":"https://provider.example?host={domain}","connection":{"tls":tls}})
        }
        "credentialSource" => {
            serde_json::json!({"provider":{"type":"oauth2ClientCredentials","tokenEndpoint":"https://issuer.example",
            "clientAuthentication":{"activeSecretRef":{"name":"bootstrap"}},"tls":tls}})
        }
        _ => unreachable!(),
    };
    serde_json::from_value(
        serde_json::json!({"metadata":{"name":"link","namespace":"apps"},"spec":{"type":provider,"config":config}}),
    )
    .unwrap()
}

#[test]
fn missing_optional_tls_fetch_evaluator_or_connection_has_no_local_loan() {
    use crate::resources::edgion_plugins::EdgionPlugin;
    use crate::resources::link_sys::SystemConfig;
    use crate::resources::refs::link_sys_fields::PreparedLinkSysReferences;
    use crate::resources::refs::plugin_fields::PreparedPluginReferences;
    for variant in ["Wasm", "ForwardAuth", "Guardrail", "AiGuard"] {
        let owner = plugin(variant, "requestPlugins", true, serde_json::Value::Null);
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        assert!(
            binding
                .request_entries()
                .next()
                .unwrap()
                .outbound_tls()
                .unwrap()
                .is_none(),
            "{variant}"
        );
    }
    for variant in ["Wasm", "AiGuard"] {
        let mut owner = plugin(variant, "requestPlugins", true, tls());
        match &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin {
            EdgionPlugin::Wasm(config) => config.source.fetch = None,
            EdgionPlugin::AiGuard(config) => config.evaluator = None,
            _ => unreachable!(),
        }
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        assert!(
            binding
                .request_entries()
                .next()
                .unwrap()
                .outbound_tls()
                .unwrap()
                .is_none(),
            "{variant}"
        );
    }
    for provider in [
        "redis",
        "etcd",
        "elasticsearch",
        "kafka",
        "otlp",
        "webhook",
        "httpdns",
        "credentialSource",
    ] {
        let owner = link_sys(provider, serde_json::Value::Null);
        let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
        assert!(
            prepared.bind(&owner).unwrap().outbound_tls().unwrap().is_none(),
            "{provider}"
        );
    }
    let mut owner = link_sys("httpdns", tls());
    let SystemConfig::HttpDns(config) = &mut owner.spec.config else {
        unreachable!()
    };
    config.connection = None;
    let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    assert!(prepared.bind(&owner).unwrap().outbound_tls().unwrap().is_none());
}

#[test]
fn wasm_supported_response_stages_keep_their_fetch_tls_loan() {
    use crate::resources::refs::plugin_fields::PreparedPluginReferences;
    for stage in ["upstreamResponseFilterPlugins", "upstreamResponseBodyFilterPlugins"] {
        let owner = plugin("Wasm", stage, true, tls());
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = if stage == "upstreamResponseFilterPlugins" {
            binding.response_filter_entries().next().unwrap()
        } else {
            binding.response_body_filter_entries().next().unwrap()
        };
        assert_pairs(entry.outbound_tls().unwrap().unwrap(), "EdgionPlugins");
    }
}

#[test]
fn fresh_loan_sees_material_changes_but_rejects_authored_reference_drift() {
    use crate::resources::edgion_plugins::EdgionPlugin;
    use crate::resources::refs::plugin_fields::PreparedPluginReferences;
    let mut owner = plugin("LdapAuth", "requestPlugins", true, tls());
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let mut binding = prepared.bind_mut(&mut owner).unwrap();
    let mut entry = binding.request_entries().next().unwrap();
    let EdgionPlugin::LdapAuth(config) = entry.config_mut() else {
        unreachable!()
    };
    config.resolved_ca_certificates = None;
    assert!(entry.outbound_tls().unwrap().unwrap().ca_material().is_none());
    let EdgionPlugin::LdapAuth(config) = entry.config_mut() else {
        unreachable!()
    };
    config
        .tls
        .validation
        .as_mut()
        .unwrap()
        .ca_certificate_refs
        .as_mut()
        .unwrap()[0]
        .name = "other".into();
    assert!(matches!(
        entry.outbound_tls(),
        Err(super::super::ReferenceError::StaleBinding)
    ));
    assert!(matches!(
        entry.outbound_tls_selection(),
        Err(super::super::ReferenceError::StaleBinding)
    ));
}

#[test]
fn every_local_selection_checks_source_even_without_active_reference_slots() {
    use crate::resources::refs::link_sys_fields::PreparedLinkSysReferences;
    use crate::resources::refs::plugin_fields::PreparedPluginReferences;
    for wire in [
        serde_json::Value::Null,
        serde_json::json!({"enabled":false}),
        serde_json::json!({"enabled":true,"verify":false}),
    ] {
        for bad in ["missing-name", "bad-name", "empty-namespace", "bad-namespace"] {
            let expected = if bad.ends_with("namespace") {
                ReferenceError::InvalidNamespace
            } else {
                ReferenceError::InvalidName
            };
            let mut plugin = plugin("ForwardAuth", "requestPlugins", true, wire.clone());
            let mut link = link_sys("httpdns", wire.clone());
            for metadata in [&mut plugin.metadata, &mut link.metadata] {
                match bad {
                    "missing-name" => metadata.name = None,
                    "bad-name" => metadata.name = Some("Bad_Name".into()),
                    "empty-namespace" => metadata.namespace = Some(String::new()),
                    "bad-namespace" => metadata.namespace = Some("Bad_NS".into()),
                    _ => unreachable!(),
                }
            }
            let prepared = PreparedPluginReferences::from_plugins(&plugin);
            let binding = prepared.bind(&plugin).unwrap();
            let entry = binding.request_entries().next().unwrap();
            let selection = entry.outbound_tls_selection().unwrap();
            assert_eq!(selection.validate_source(), Err(expected), "plugin {bad}: {wire}");
            if let Some(field) = selection.active_field() {
                assert_eq!(field.validate_source(), Err(expected));
                assert_eq!(field.ca_reference_count(), 0);
            }
            let prepared = PreparedLinkSysReferences::from_link_sys(&link);
            let binding = prepared.bind(&link).unwrap();
            assert_eq!(
                binding.outbound_tls_selection().unwrap().validate_source(),
                Err(expected),
                "LinkSys {bad}: {wire}"
            );
        }
    }
    let mut owner = link_sys("httpdns", serde_json::Value::Null);
    owner.metadata.namespace = None;
    let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    let binding = prepared.bind(&owner).unwrap();
    assert_eq!(binding.outbound_tls_selection().unwrap().validate_source(), Ok(()));
    owner.metadata.uid = Some("replacement".into());
    assert!(matches!(prepared.bind(&owner), Err(ReferenceError::StaleBinding)));
}

#[test]
fn actual_field_pairs_reference_and_material_without_copying() {
    use crate::resources::refs::plugin_fields::PreparedPluginReferences;
    let owner: crate::EdgionPlugins = serde_json::from_value(serde_json::json!({
        "metadata":{"name":"plugins","namespace":"apps"},"spec":{"requestPlugins":[
            {"type":"OpenidConnect","config":{"discovery":"https://issuer.example","clientId":"client",
                "tls":{"validation":{"caCertificateRefs":[{"kind":"Secret","name":"ca"}]},"clientCertificateRef":{"name":"client"}},
                "resolvedCaCertificates":[{"metadata":{"name":"ca","namespace":"apps"}}],
                "resolvedClientCertificate":{"metadata":{"name":"client","namespace":"apps"}}}}
        ]}
    })).unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let field = entry.outbound_tls().unwrap().unwrap();
    field.visit_ca(|pair| {
        assert_eq!(pair.reference().validated().unwrap().target().name(), "ca");
        assert!(std::ptr::eq(pair.material().unwrap(), &field.ca_material().unwrap()[0]));
    });
    let pair = field.client_certificate().unwrap();
    assert_eq!(
        pair.reference().validated().unwrap().target().scope().namespace(),
        Some("apps")
    );
    assert!(std::ptr::eq(pair.material().unwrap(), field.client_material().unwrap()));
}
