use super::*;
use crate::resources::edgion_plugins::EdgionPlugin;
use serde_json::json;

fn descriptor_plugins() -> EdgionPlugins {
    serde_json::from_value(json!({"metadata":{"name":"descriptor-owner","namespace":"apps","uid":"actual"},"spec":{
        "requestPlugins":[{"type":"GrpcJsonTranscoder","config":{"descriptor":{"type":"configMapRef","name":"schema","key":"test.pb"},"services":["test.TestService"]}}]
    }})).unwrap()
}

#[test]
fn descriptor_fixed_target_source_and_exact_selector_are_owner_bound() {
    for namespace in [Some("apps"), None] {
        let mut owner = descriptor_plugins();
        owner.metadata.namespace = namespace.map(str::to_owned);
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let refs = entry.references().unwrap();
        let field = refs.grpc_descriptor().unwrap().unwrap();
        assert_eq!(field.key(), "test.pb");
        let target = field.reference().validated().unwrap();
        assert_eq!(target.source().group_kind().kind(), "EdgionPlugins");
        assert_eq!(
            target.source().scope().namespace(),
            Some(namespace.unwrap_or("default"))
        );
        assert_eq!(target.target().group_kind().group(), "");
        assert_eq!(target.target().group_kind().kind(), "ConfigMap");
        assert_eq!(target.target().name(), "schema");
        assert!(target.is_same_namespace());
    }
}

#[test]
fn descriptor_key_branch_identity_and_activity_drift_invalidate_frozen_owner() {
    let owner = descriptor_plugins();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let refs = entry.references().unwrap();
    let field = refs.grpc_descriptor().unwrap().unwrap();
    let frozen = field.reference().freeze().unwrap();
    for case in ["key", "name", "inline", "owner", "disabled"] {
        let mut changed = owner.clone();
        let entry = &mut changed.spec.request_plugins.as_mut().unwrap()[0];
        if case == "disabled" {
            entry.enable = false;
        } else if case == "owner" {
            changed.metadata.uid = Some("other".into());
        } else {
            let EdgionPlugin::GrpcJsonTranscoder(config) = &mut entry.plugin else {
                panic!()
            };
            match case {
                "inline" => {
                    config.descriptor = crate::grpc_transcoding::GrpcDescriptorSource::Inline {
                        base64: "literal".into(),
                    }
                }
                _ => {
                    let crate::grpc_transcoding::GrpcDescriptorSource::ConfigMapRef { name, key } =
                        &mut config.descriptor
                    else {
                        panic!()
                    };
                    if case == "name" {
                        *name = "other".into();
                    } else {
                        *key = "other.pb".into();
                    }
                }
            }
        }
        assert!(prepared.bind(&changed).is_err(), "{case}");
        assert!(!frozen.matches_plugins(&changed), "{case}");
    }
    let mut material = owner.clone();
    let EdgionPlugin::GrpcJsonTranscoder(config) = &mut material.spec.request_plugins.as_mut().unwrap()[0].plugin
    else {
        panic!()
    };
    config.resolved_descriptor = Some(vec![1, 2, 3]);
    config.descriptor_error = Some("fixed error".into());
    assert!(prepared.bind(&material).is_ok() && frozen.matches_plugins(&material));
}

#[test]
fn descriptor_invalid_selector_or_identity_never_exposes_a_usable_loan() {
    for case in ["name", "namespace", "owner", "empty-key", "long-key"] {
        let mut owner = descriptor_plugins();
        match case {
            "namespace" => owner.metadata.namespace = Some("Bad_Namespace".into()),
            "owner" => owner.metadata.name = Some("Bad_Owner".into()),
            _ => {
                let EdgionPlugin::GrpcJsonTranscoder(config) =
                    &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin
                else {
                    panic!()
                };
                let crate::grpc_transcoding::GrpcDescriptorSource::ConfigMapRef { name, key } = &mut config.descriptor
                else {
                    panic!()
                };
                match case {
                    "name" => *name = "Bad_Name".into(),
                    "empty-key" => key.clear(),
                    _ => *key = "x".repeat(254),
                }
            }
        }
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let refs = entry.references().unwrap();
        let Err(error) = refs.grpc_descriptor().unwrap() else {
            panic!("{case}");
        };
        assert_eq!(
            matches!(error, DescriptorReferenceError::InvalidSelector),
            case.ends_with("key")
        );
    }
}

#[test]
fn descriptor_inactive_stages_and_inline_branch_expose_no_reference() {
    let config =
        json!({"descriptor":{"type":"configMapRef","name":"schema","key":"test.pb"},"services":["test.TestService"]});
    for stage in [
        "requestPlugins",
        "upstreamResponseFilterPlugins",
        "upstreamResponseBodyFilterPlugins",
        "upstreamResponsePlugins",
    ] {
        for enabled in [true, false] {
            let owner: EdgionPlugins = serde_json::from_value(json!({"metadata":{"name":"owner"},"spec":{
                stage:[{"type":"GrpcJsonTranscoder","enable":enabled,"config":config}]
            }}))
            .unwrap();
            let prepared = PreparedPluginReferences::from_plugins(&owner);
            let binding = prepared.bind(&owner).unwrap();
            let mut count = 0;
            for entry in binding
                .request_entries()
                .chain(binding.response_filter_entries())
                .chain(binding.response_body_filter_entries())
                .chain(binding.response_entries())
            {
                count += 1;
                let refs = entry.references().unwrap();
                assert_eq!(refs.grpc_descriptor().is_some(), stage == "requestPlugins" && enabled);
            }
            assert_eq!(count, 1, "stage fixture must contain actual entry");
        }
    }
    let mut owner = descriptor_plugins();
    let EdgionPlugin::GrpcJsonTranscoder(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
        panic!()
    };
    config.descriptor = crate::grpc_transcoding::GrpcDescriptorSource::Inline {
        base64: "not retained in topology".into(),
    };
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let refs = entry.references().unwrap();
    assert!(refs.grpc_descriptor().is_none());
    assert!(!serde_json::to_string(prepared.snapshot.as_ref())
        .unwrap()
        .contains("not retained in topology"));
}

fn plugins() -> EdgionPlugins {
    serde_json::from_value(json!({
        "apiVersion":"edgion.io/v1", "kind":"EdgionPlugins",
        "metadata":{"name":"auth", "namespace":"plugins", "uid":"uid", "generation":7},
        "spec":{"requestPlugins":[{"type":"Csrf", "config":{"secretRef":{"name":"key"}}}]}
    }))
    .unwrap()
}

#[test]
fn capability_source_is_actual_plugin_owner_and_default_namespace() {
    let owner = plugins();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let bindings = prepared.bind(&owner).unwrap();
    let entry = bindings.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let token = references.csrf_secret().unwrap();
    let reference = token.validated().unwrap();
    assert_eq!(reference.source().group_kind().kind(), "EdgionPlugins");
    assert_eq!(reference.source().scope().namespace(), Some("plugins"));
    assert_eq!(reference.target().scope().namespace(), Some("plugins"));
    assert_eq!(reference.target().group_kind().group(), "");
    assert_eq!(reference.target().group_kind().kind(), "Secret");
    assert!(reference.is_same_namespace());
}

#[test]
fn owner_metadata_and_current_field_drift_reject_old_snapshot() {
    let owner = plugins();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    for mutation in 0..7 {
        let mut changed = owner.clone();
        match mutation {
            0 => changed.metadata.uid = Some("other".to_string()),
            1 => changed.metadata.generation = Some(8),
            2 => changed.metadata.namespace = Some("other".to_string()),
            3 => changed.metadata.name = Some("other".to_string()),
            4 => changed.spec.request_plugins.as_mut().unwrap()[0].enable = false,
            5 => changed.spec.request_plugins.as_mut().unwrap().clear(),
            _ => {
                let EdgionPlugin::Csrf(config) = &mut changed.spec.request_plugins.as_mut().unwrap()[0].plugin else {
                    panic!("expected Csrf")
                };
                config.secret_ref.as_mut().unwrap().name = "other".to_string();
            }
        }
        assert!(matches!(prepared.bind(&changed), Err(ReferenceError::StaleBinding)));
    }
}

#[test]
fn new_reference_loan_rechecks_authored_fields_after_material_writeback() {
    let mut owner = plugins();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let mut bindings = prepared.bind_mut(&mut owner).unwrap();
    let mut entry = bindings.request_entries().next().unwrap();
    let EdgionPlugin::Csrf(config) = entry.config_mut() else {
        panic!("expected Csrf")
    };
    config.resolved_key = Some("resolved-value".to_string());
    assert!(entry.references().is_ok());
    let EdgionPlugin::Csrf(config) = entry.config_mut() else {
        panic!("expected Csrf")
    };
    config.secret_ref.as_mut().unwrap().name = "new-key".to_string();
    assert!(matches!(entry.references(), Err(ReferenceError::StaleBinding)));
}

#[test]
fn groups_preserve_order_labels_and_valid_siblings_of_invalid_slots() {
    let mut owner = plugins();
    owner.spec.request_plugins = serde_json::from_value(json!([{
        "type":"BasicAuth", "config":{"secretGroups":[
            {"name":"first", "secretRefs":[{"group":"core", "name":"one"}, {"kind":"", "name":"bad"}]},
            {"name":"second", "secretRefs":[{"name":"two"}]}
        ]}
    }]))
    .unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let groups: Vec<_> = references.secret_groups().collect();
    assert_eq!(
        groups.iter().map(|group| group.name()).collect::<Vec<_>>(),
        ["first", "second"]
    );
    let tokens: Vec<_> = groups[0].references().collect();
    assert_eq!(tokens[0].validated().unwrap().target().group_kind().group(), "");
    assert!(matches!(tokens[1].validated(), Err(ReferenceError::InvalidKind)));
    assert!(groups[1].references().next().unwrap().validated().is_ok());
    let mut changed = owner.clone();
    let EdgionPlugin::BasicAuth(config) = &mut changed.spec.request_plugins.as_mut().unwrap()[0].plugin else {
        panic!("expected BasicAuth")
    };
    config.secret_groups.as_mut().unwrap().swap(0, 1);
    assert!(matches!(prepared.bind(&changed), Err(ReferenceError::StaleBinding)));
}

#[test]
fn unsupported_stage_is_traversed_for_clearing_but_mints_no_credential() {
    let mut owner = plugins();
    owner.spec.upstream_response_plugins = serde_json::from_value(json!([{
        "type":"Csrf", "config":{"secretRef":{"name":"key"}}
    }]))
    .unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let mut bindings = prepared.bind_mut(&mut owner).unwrap();
    let mut entry = bindings.response_entries().next().unwrap();
    assert!(entry.is_enabled());
    let references = entry.references().unwrap();
    assert!(references.csrf_secret().is_none());
    let EdgionPlugin::Csrf(config) = entry.config_mut() else {
        panic!("expected Csrf")
    };
    config.resolved_key = None;
}

#[test]
fn frozen_descriptor_retains_identity_but_requires_current_owner_topology() {
    let mut owner = plugins();
    let frozen = {
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let references = entry.references().unwrap();
        references.csrf_secret().unwrap().freeze().unwrap()
    };
    assert!(frozen.matches_plugins(&owner));
    let EdgionPlugin::Csrf(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
        panic!("expected Csrf");
    };
    config.resolved_key = Some("private-material".to_string());
    assert!(frozen.matches_plugins(&owner));
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let rebuilt = references.csrf_secret().unwrap().freeze().unwrap();
    assert_eq!(frozen, rebuilt);
    assert_eq!(frozen.owner_revision(), rebuilt.owner_revision());
    let EdgionPlugin::Csrf(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
        panic!("expected Csrf");
    };
    config.secret_ref.as_mut().unwrap().name = "replacement".to_string();
    assert!(!frozen.matches_plugins(&owner));
    assert_eq!(frozen.grant_query().unwrap().target().name(), "key");
    assert!(!format!("{frozen:?}").contains("private-material"));
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let changed = references.csrf_secret().unwrap().freeze().unwrap();
    assert_ne!(frozen, changed);
    assert_ne!(frozen.owner_revision(), changed.owner_revision());
}

#[test]
fn frozen_slots_share_one_owner_and_keep_distinct_field_positions() {
    let mut owner = plugins();
    owner.spec.request_plugins = serde_json::from_value(json!([{
        "type":"BasicAuth", "config":{"secretGroups":[{"name":"g", "secretRefs":[{"name":"one"}, {"name":"two"}]}]}
    }]))
    .unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let bindings = prepared.bind(&owner).unwrap();
    let entry = bindings.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let group = references.secret_groups().next().unwrap();
    let frozen: Vec<_> = group.references().map(|token| token.freeze().unwrap()).collect();
    assert!(frozen[0].same_owner_snapshot(&frozen[1]));
    assert_ne!(frozen[0], frozen[1]);
    assert_ne!(frozen[0].slot_revision(), frozen[1].slot_revision());
    let EdgionPlugin::BasicAuth(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
        panic!("expected BasicAuth");
    };
    config.secret_groups.as_mut().unwrap()[0].secret_refs.swap(0, 1);
    assert!(frozen.iter().all(|reference| !reference.matches_plugins(&owner)));
}

#[test]
fn invalid_reference_cannot_be_frozen_for_async_use() {
    let mut owner = plugins();
    let EdgionPlugin::Csrf(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
        panic!("expected Csrf");
    };
    config.secret_ref.as_mut().unwrap().kind = Some("ConfigMap".to_string());
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let bindings = prepared.bind(&owner).unwrap();
    let entry = bindings.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    assert!(matches!(
        references.csrf_secret().unwrap().freeze(),
        Err(ReferenceError::UnsupportedTarget)
    ));
}

#[test]
fn managed_output_is_derived_from_the_actual_injector_owner() {
    let mut owner = plugins();
    owner.spec.request_plugins = serde_json::from_value(json!([{
        "type":"CredentialInjector", "config":{"credentialRef":{"group":"edgion.io", "kind":"LinkSys", "namespace":"sources", "name":"issuer"}}
    }])).unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let token = references.credential().unwrap();
    assert_eq!(token.validated().unwrap().target().scope().namespace(), Some("sources"));
    let output = token.managed_credential_output().unwrap();
    assert_eq!(output.target().scope().namespace(), Some("plugins"));
    assert_eq!(output.target().name(), "auth");
    assert_eq!(output.target().group_kind().group(), "");
    assert_eq!(output.target().group_kind().kind(), "Secret");
    assert!(output.is_same_namespace());
}

#[test]
fn disabled_plugin_tls_preserves_independent_wasm_auth_reference() {
    let mut owner = plugins();
    owner.spec.request_plugins = serde_json::from_value(json!([{
        "type":"Wasm", "config":{"source":{"url":"https://artifact.example/module.wasm", "fetch":{
            "authHeaderSecretRef":{"name":"artifact-auth", "key":"authorization"},
            "tls":{"enabled":false, "validation":{"caCertificateRefs":[{"kind":"Secret", "name":"ca"}]},
                "clientCertificateRef":{"name":"identity"}}
        }}}
    }]))
    .unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    assert!(references.tls().is_none());
    assert!(references.wasm_auth_header_secret().unwrap().validated().is_ok());
}

#[test]
fn managed_output_rejects_static_and_non_credential_fields_and_inactive_entries() {
    let owner = plugins();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    assert!(matches!(
        references.csrf_secret().unwrap().managed_credential_output(),
        Err(ReferenceError::UnsupportedTarget)
    ));
    let mut owner = plugins();
    owner.spec.request_plugins = serde_json::from_value(json!([{
        "type":"CredentialInjector", "config":{"credentialRef":{"name":"static-token"}}
    }]))
    .unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    assert!(matches!(
        references.credential().unwrap().managed_credential_output(),
        Err(ReferenceError::UnsupportedTarget)
    ));
    owner.spec.request_plugins.as_mut().unwrap()[0].enable = false;
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    assert!(entry.references().unwrap().credential().is_none());
    owner.spec.upstream_response_plugins = serde_json::from_value(json!([{
        "type":"CredentialInjector", "config":{"credentialRef":{"group":"edgion.io", "kind":"LinkSys", "name":"issuer"}}
    }]))
    .unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.response_entries().next().unwrap();
    assert!(entry.references().unwrap().credential().is_none());
}

#[test]
fn mirror_plugin_loans_keep_disabled_original_ordinals_and_source_identity() {
    let owner: EdgionPlugins = serde_json::from_value(serde_json::json!({"metadata":{"name":"plugins","namespace":"app","uid":"plugins-uid","generation":4},"spec":{"requestPlugins":[
        {"type":"RequestMirror","enable":false,"config":{"backendRef":{"name":"first","port":80}}},
        {"type":"RequestMirror","config":{"backendRef":{"name":"second","namespace":"target","port":81}}}
    ]}})).unwrap();
    let refs = PluginMirrorRefs::from_plugins(&owner);
    for (index, field) in refs.authored().enumerate() {
        let terminal = field.validated().unwrap().unwrap();
        assert_eq!(
            terminal.reference().field().slot(),
            super::super::route_fields::MirrorSlot::Plugin(index)
        );
        assert_eq!(terminal.reference().field().source_uid(), Some("plugins-uid"));
        assert_eq!(terminal.reference().field().source_generation(), Some(4));
        assert_eq!(terminal.grant_query().source().group_kind().group(), "edgion.io");
    }
    assert!(refs.effective_at(2).is_none());
}

#[test]
fn descriptor_generation_and_ordinal_are_bound_without_material_provenance_claim() {
    let mut owner = descriptor_plugins();
    owner.metadata.generation = Some(3);
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let bindings = prepared.bind(&owner).unwrap();
    let entry = bindings.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let descriptor = references.grpc_descriptor().unwrap().unwrap();
    let frozen = descriptor.reference().freeze().unwrap();
    let mut replacement = owner.clone();
    replacement.metadata.generation = Some(4);
    assert!(!frozen.matches_plugins(&replacement));
    assert!(prepared.bind(&replacement).is_err());
    let mut shifted = owner.clone();
    let mut disabled = shifted.spec.request_plugins.as_ref().unwrap()[0].clone();
    disabled.enable = false;
    shifted.spec.request_plugins.as_mut().unwrap().insert(0, disabled);
    assert!(!frozen.matches_plugins(&shifted));
    let current = PreparedPluginReferences::from_plugins(&shifted);
    let bound = current.bind(&shifted).unwrap();
    let entries: Vec<_> = bound.request_entries().collect();
    assert!(entries[0].references().unwrap().grpc_descriptor().is_none());
    assert!(entries[1].references().unwrap().grpc_descriptor().unwrap().is_ok());
}

fn fixed_link_sys_entries() -> Vec<serde_json::Value> {
    vec![
        json!({"type":"Dsl","config":{"name":"program","source":"return next()","linkSysRefs":[{"namespace":"raw/ns","name":"redis_alias"}]}}),
        json!({"type":"RateLimitRedis","config":{"redisRef":"apps/cache","quotas":[]}}),
        json!({"type":"GraphqlRateLimit","config":{"schema":"type Query { name: String }","redisRef":"apps/cache","policies":[{"rate":100,"interval":"1s"}]}}),
        json!({"type":"WebhookKeyGet","config":{"vars":[{"name":"result","webhookRef":"apps/hook","extract":{"source":"bodyText"}}]}}),
        json!({"type":"AiProxy","config":{"api":"openaiChat","usageObservation":"provider","tokenQuota":{"redisRef":"apps/cache","key":[],"limits":[{"tokens":100,"window":"1h"}]}}}),
    ]
}

#[test]
fn fixed_link_sys_configs_bind_actual_owner_and_original_variant() {
    for value in fixed_link_sys_entries() {
        let mut owner: EdgionPlugins =
            serde_json::from_value(json!({"metadata":{"name":"plugins"},"spec":{"requestPlugins":[value]}})).unwrap();
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        assert!(entry.link_sys_config().unwrap().is_some());
        assert_eq!(owner.metadata.namespace, None);
        if let EdgionPlugin::GraphqlRateLimit(original) = &owner.spec.request_plugins.as_ref().unwrap()[0].plugin {
            let Some(HttpLinkSysConfig::GraphqlRateLimit(bound)) = entry.link_sys_config().unwrap() else {
                panic!("original Graphql variant required")
            };
            assert!(std::ptr::eq(original, bound));
        }
        for namespace in ["", "bad/ns"] {
            owner.metadata.namespace = Some(namespace.into());
            assert!(prepared.bind(&owner).is_err());
            let current = PreparedPluginReferences::from_plugins(&owner);
            let bound = current.bind(&owner).unwrap();
            assert!(bound.request_entries().next().unwrap().link_sys_config().is_err());
        }
        owner.metadata.namespace = None;
        owner.metadata.name = None;
        let current = PreparedPluginReferences::from_plugins(&owner);
        assert!(current
            .bind(&owner)
            .unwrap()
            .request_entries()
            .next()
            .unwrap()
            .link_sys_config()
            .is_err());
        owner.spec.request_plugins.as_mut().unwrap()[0].enable = false;
        let current = PreparedPluginReferences::from_plugins(&owner);
        assert!(current
            .bind(&owner)
            .unwrap()
            .request_entries()
            .next()
            .unwrap()
            .link_sys_config()
            .unwrap()
            .is_none());
    }
}

#[test]
fn fixed_link_sys_topology_rejects_mutated_keys_presence_order_and_enable() {
    for value in fixed_link_sys_entries() {
        let owner: EdgionPlugins =
            serde_json::from_value(json!({"metadata":{"name":"plugins"},"spec":{"requestPlugins":[value]}})).unwrap();
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let mut changed = owner.clone();
        match &mut changed.spec.request_plugins.as_mut().unwrap()[0].plugin {
            EdgionPlugin::Dsl(config) => config.linksys_refs[0].namespace.push_str("-changed"),
            EdgionPlugin::RateLimitRedis(config) => config.redis_ref.push_str("-changed"),
            EdgionPlugin::GraphqlRateLimit(config) => config.redis_ref.push_str("-changed"),
            EdgionPlugin::WebhookKeyGet(config) => config.vars[0].webhook_ref.push_str("-changed"),
            EdgionPlugin::AiProxy(config) => config.token_quota.as_mut().unwrap().redis_ref.push_str("-changed"),
            _ => unreachable!(),
        }
        assert!(prepared.bind(&changed).is_err());
        assert!(prepared.bind_mut(&mut changed).is_err());
        let mut changed = owner.clone();
        changed.spec.request_plugins.as_mut().unwrap()[0].enable = false;
        assert!(prepared.bind(&changed).is_err());
        let mut changed = owner.clone();
        changed.spec.request_plugins.as_mut().unwrap().clear();
        assert!(prepared.bind(&changed).is_err());
    }
    let mut values = fixed_link_sys_entries();
    let mut owner: EdgionPlugins =
        serde_json::from_value(json!({"metadata":{"name":"plugins"},"spec":{"requestPlugins":values}})).unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    owner.spec.request_plugins.as_mut().unwrap().swap(0, 1);
    assert!(prepared.bind(&owner).is_err());
    values[0]["config"]["linkSysRefs"] = json!([]);
    values[4]["config"].as_object_mut().unwrap().remove("tokenQuota");
    let owner: EdgionPlugins =
        serde_json::from_value(json!({"metadata":{},"spec":{"requestPlugins":[values[0].clone(),values[4].clone()]}}))
            .unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    assert!(prepared
        .bind(&owner)
        .unwrap()
        .request_entries()
        .all(|entry| entry.link_sys_config().unwrap().is_none()));
}

#[test]
fn fixed_link_sys_collection_and_optional_topology_changes_reject_both_bindings() {
    let entries = fixed_link_sys_entries();
    for kind in ["Dsl", "WebhookKeyGet", "AiProxy"] {
        let value = entries.iter().find(|value| value["type"] == kind).unwrap();
        let mut owner: EdgionPlugins =
            serde_json::from_value(json!({"metadata":{"name":"plugins"},"spec":{"requestPlugins":[value]}})).unwrap();
        match &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin {
            EdgionPlugin::Dsl(config) => {
                let mut second = config.linksys_refs[0].clone();
                second.name = "second".into();
                config.linksys_refs.push(second);
            }
            EdgionPlugin::WebhookKeyGet(config) => {
                let mut second = config.vars[0].clone();
                second.webhook_ref = "apps/second".into();
                config.vars.push(second);
            }
            _ => {}
        }
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        for mutation in 0..4 {
            let mut changed = owner.clone();
            match &mut changed.spec.request_plugins.as_mut().unwrap()[0].plugin {
                EdgionPlugin::Dsl(config) => match mutation {
                    0 => config.linksys_refs[0].name = "changed".into(),
                    1 => config.linksys_refs.swap(0, 1),
                    2 => {
                        config.linksys_refs.pop();
                    }
                    _ => config.linksys_refs.push(config.linksys_refs[0].clone()),
                },
                EdgionPlugin::WebhookKeyGet(config) => match mutation {
                    0 => config.vars[0].webhook_ref = "apps/changed".into(),
                    1 => config.vars.swap(0, 1),
                    2 => {
                        config.vars.pop();
                    }
                    _ => config.vars.push(config.vars[0].clone()),
                },
                EdgionPlugin::AiProxy(config) => config.token_quota = None,
                _ => unreachable!(),
            }
            assert!(prepared.bind(&changed).is_err(), "{kind}/{mutation}");
            assert!(prepared.bind_mut(&mut changed).is_err(), "{kind}/{mutation}");
        }
    }
}

fn managed_plugins(kind: &str) -> EdgionPlugins {
    let mut connection = json!({"target":{"name":"auth","port":8080}, "request":{"path":{"template":"/decision"}}});
    if kind == "ForwardAuth" {
        connection["decision"] = json!({});
    }
    let config = if kind == "AiGuard" {
        json!({"evaluator":connection})
    } else {
        connection
    };
    serde_json::from_value(json!({"metadata":{"name":"actual","namespace":"tenant"},"spec":{"requestPlugins":[{"type":kind,"config":config}]}})).unwrap()
}

#[test]
fn managed_http_fields_borrow_actual_configs_and_default_only_absent_namespaces() {
    for kind in ["ForwardAuth", "Guardrail", "AiGuard"] {
        for namespace in [None, Some("tenant")] {
            let mut owner = managed_plugins(kind);
            owner.metadata.namespace = namespace.map(str::to_owned);
            let prepared = PreparedPluginReferences::from_plugins(&owner);
            let binding = prepared.bind(&owner).unwrap();
            let entry = binding.request_entries().next().unwrap();
            let field = entry.managed_http_config().unwrap().unwrap();
            assert!(std::ptr::eq(
                field.plugin(),
                &owner.spec.request_plugins.as_ref().unwrap()[0].plugin
            ));
            assert_eq!(field.owner_namespace(), namespace.unwrap_or("default"));
            assert!(field.connection().target.service.namespace.is_none());
            assert!(field.outbound_tls_selection().is_ok());
        }
        for (namespace, name) in [
            (Some(""), Some("actual")),
            (Some("tenant"), None),
            (Some("tenant"), Some("bad/name")),
        ] {
            let mut owner = managed_plugins(kind);
            owner.metadata.namespace = namespace.map(str::to_owned);
            owner.metadata.name = name.map(str::to_owned);
            owner.metadata.uid = Some("valid-uid".into());
            let prepared = PreparedPluginReferences::from_plugins(&owner);
            let binding = prepared.bind(&owner).unwrap();
            assert!(binding.request_entries().next().unwrap().managed_http_config().is_err());
        }
    }
}

#[test]
fn managed_http_reference_topology_rejects_drift_without_snapshotting_url_or_body() {
    let owner = managed_plugins("AiGuard");
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    for case in ["namespace", "name", "group", "mode", "port", "url", "evaluator", "add"] {
        let mut changed = owner.clone();
        let entries = changed.spec.request_plugins.as_mut().unwrap();
        if case == "add" {
            entries.push(entries[0].clone());
        } else {
            let EdgionPlugin::AiGuard(config) = &mut entries[0].plugin else {
                panic!()
            };
            if case == "evaluator" {
                config.evaluator = None;
            } else {
                let target = &mut config.evaluator.as_mut().unwrap().conn.target;
                match case {
                    "namespace" => target.service.namespace = Some("shared".into()),
                    "name" => target.service.name = Some("other".into()),
                    "group" => target.service.group = Some("core".into()),
                    "mode" => target.service.kind = Some("ServiceClusterIp".into()),
                    "port" => target.service.port = Some(9090),
                    "url" => target.url = Some("https://example.com".into()),
                    _ => panic!(),
                }
            }
        }
        assert!(prepared.bind(&changed).is_err(), "{case}");
        assert!(prepared.bind_mut(&mut changed).is_err(), "{case}");
    }
    let mut changed = owner.clone();
    let EdgionPlugin::AiGuard(config) = &mut changed.spec.request_plugins.as_mut().unwrap()[0].plugin else {
        panic!()
    };
    config
        .evaluator
        .as_mut()
        .unwrap()
        .request
        .path
        .as_mut()
        .unwrap()
        .template = "/changed".into();
    assert!(
        prepared.bind(&changed).is_ok(),
        "literal request configuration is not reference topology"
    );
}

#[test]
fn managed_http_disabled_and_inline_guard_do_not_acquire_an_outbound_loan() {
    let mut owner = managed_plugins("Guardrail");
    owner.metadata.name = None;
    owner.spec.request_plugins.as_mut().unwrap()[0].enable = false;
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    assert!(binding
        .request_entries()
        .next()
        .unwrap()
        .managed_http_config()
        .unwrap()
        .is_none());
    let mut owner = managed_plugins("AiGuard");
    owner.metadata.name = None;
    let EdgionPlugin::AiGuard(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
        panic!()
    };
    config.evaluator = None;
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    assert!(binding
        .request_entries()
        .next()
        .unwrap()
        .managed_http_config()
        .unwrap()
        .is_none());
}

#[test]
fn frozen_managed_output_preserves_actual_projection_and_checks_raw_material() {
    for namespace in [None, Some("default".to_string()), Some("plugins".to_string())] {
        let mut owner = plugins();
        owner.metadata.namespace = namespace;
        owner.spec.request_plugins=serde_json::from_value(json!([{"type":"CredentialInjector","config":{"credentialRef":{"group":"edgion.io","kind":"LinkSys","namespace":"sources","name":"issuer"}}}])).unwrap();
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let references = entry.references().unwrap();
        let token = references.credential().unwrap();
        let frozen = token.freeze().unwrap();
        assert_eq!(
            token.managed_credential_output().unwrap().target(),
            frozen.managed_credential_output().unwrap().target()
        );
        let target = frozen.managed_credential_output().unwrap().target();
        assert_eq!(target.name(), "auth");
        assert_eq!(
            target.scope().namespace(),
            Some(owner.metadata.namespace.as_deref().unwrap_or("default"))
        );
        for case in 0..5 {
            let mut secret = k8s_openapi::api::core::v1::Secret::default();
            secret.metadata.name = Some("auth".into());
            secret.metadata.namespace = target.scope().namespace().map(str::to_string);
            match case {
                1 => secret.metadata.name = Some("decoy".into()),
                2 => secret.metadata.namespace = Some("sources".into()),
                3 => secret.metadata.name = None,
                4 => secret.metadata.namespace = None,
                _ => {}
            }
            assert_eq!(token.validate_managed_credential_secret(&secret).is_ok(), case == 0);
            assert_eq!(frozen.validate_managed_credential_secret(&secret).is_ok(), case == 0);
        }
        let mut retargeted = owner.clone();
        retargeted.metadata.name = Some("changed".into());
        assert!(!frozen.matches_plugins(&retargeted));
    }
    let owner = plugins();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    assert!(references
        .csrf_secret()
        .unwrap()
        .freeze()
        .unwrap()
        .managed_credential_output()
        .is_err());
}

#[test]
fn header_ca_compressed_material_is_bound_to_actual_targets_and_active_owner() {
    use crate::constants::annotations::edgion::CONTROLLER_CA_SOURCE;
    let base = json!({"metadata":{"name":"auth","namespace":"default","uid":"first"},"spec":{
        "requestPlugins":[{"type":"HeaderCertAuth","config":{"mode":"Header",
        "caSecretRefs":[{"name":"missing"},{"name":"ca","group":"core"}],
        "resolvedCaSecrets":[{"metadata":{"name":"ca","namespace":"default","resourceVersion":"7"},"stringData":{"ca.crt":"not parsed here"}}]}}]
    }});
    for case in [
        "matching",
        "default-owner",
        "invalid-first",
        "wrong-name",
        "no-name",
        "empty-name",
        "wrong-ns",
        "no-ns",
        "empty-ns",
        "marker",
        "no-ref",
        "retarget",
        "invalid-owner",
        "invalid-owner-ns",
        "partial",
        "connection",
        "disabled",
    ] {
        let mut value = base.clone();
        let config = &mut value["spec"]["requestPlugins"][0]["config"];
        match case {
            "wrong-name" => config["resolvedCaSecrets"][0]["metadata"]["name"] = json!("other"),
            "no-name" => {
                config["resolvedCaSecrets"][0]["metadata"]
                    .as_object_mut()
                    .unwrap()
                    .remove("name");
            }
            "empty-name" => config["resolvedCaSecrets"][0]["metadata"]["name"] = json!(""),
            "wrong-ns" => config["resolvedCaSecrets"][0]["metadata"]["namespace"] = json!("other"),
            "no-ns" => {
                config["resolvedCaSecrets"][0]["metadata"]
                    .as_object_mut()
                    .unwrap()
                    .remove("namespace");
            }
            "empty-ns" => config["resolvedCaSecrets"][0]["metadata"]["namespace"] = json!(""),
            "marker" => {
                config["resolvedCaSecrets"][0]["metadata"]["annotations"] = json!({(CONTROLLER_CA_SOURCE):"ConfigMap"})
            }
            "no-ref" => config["caSecretRefs"] = json!([]),
            "retarget" => config["caSecretRefs"][1]["name"] = json!("other"),
            "invalid-first" => config["caSecretRefs"][0]["name"] = json!("bad/name"),
            "partial" => {
                let mut bad = config["resolvedCaSecrets"][0].clone();
                bad["metadata"]["name"] = json!("other");
                config["resolvedCaSecrets"].as_array_mut().unwrap().insert(0, bad);
            }
            "connection" => {
                config["mode"] = json!("Connection");
                config["caSecretRefs"][1]["name"] = json!("bad/name");
            }
            _ => {}
        }
        match case {
            "default-owner" => {
                value["metadata"].as_object_mut().unwrap().remove("namespace");
            }
            "invalid-owner" => value["metadata"]["name"] = json!("bad/name"),
            "invalid-owner-ns" => value["metadata"]["namespace"] = json!("Bad_NS"),
            "disabled" => value["spec"]["requestPlugins"][0]["enable"] = json!(false),
            _ => {}
        }
        let owner: EdgionPlugins = serde_json::from_value(value).unwrap();
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let result = entry.header_cert_auth();
        if matches!(case, "invalid-owner" | "invalid-owner-ns") {
            assert!(result.is_err(), "{case}");
            continue;
        }
        if case == "disabled" {
            assert!(result.unwrap().is_none());
            continue;
        }
        let field = result.unwrap().unwrap();
        let pairs: Vec<_> = field.ca_materials().collect();
        let good = matches!(case, "matching" | "default-owner" | "invalid-first" | "partial");
        assert_eq!(pairs.len(), usize::from(good), "{case}");
        assert_eq!(
            field.cache_reusable(),
            matches!(case, "matching" | "default-owner" | "invalid-first" | "connection"),
            "{case}"
        );
        if good {
            let actual = field.config().resolved_ca_secrets.as_ref().unwrap().last().unwrap();
            assert!(std::ptr::eq(pairs[0].material().unwrap(), actual));
            assert_eq!(pairs[0].target().unwrap().name(), "ca");
        }
    }
    let owner: EdgionPlugins = serde_json::from_value(base).unwrap();
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    for mode in [false, true] {
        let mut changed = owner.clone();
        if mode {
            let EdgionPlugin::HeaderCertAuth(config) = &mut changed.spec.request_plugins.as_mut().unwrap()[0].plugin
            else {
                panic!()
            };
            config.mode = crate::resources::edgion_plugins::CertSourceMode::Connection;
        } else {
            changed.metadata.uid = Some("second".into());
        }
        assert!(matches!(prepared.bind(&changed), Err(ReferenceError::StaleBinding)));
        let fresh = PreparedPluginReferences::from_plugins(&changed);
        let binding = fresh.bind(&changed).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let field = entry.header_cert_auth().unwrap().unwrap();
        let EdgionPlugin::HeaderCertAuth(actual) = &changed.spec.request_plugins.as_ref().unwrap()[0].plugin else {
            panic!()
        };
        assert!(std::ptr::eq(field.config(), actual));
    }
}

#[test]
fn checked_entry_namespace_comes_from_current_canonical_owner() {
    let owner: EdgionPlugins = serde_json::from_value(json!({"metadata":{"name":"oidc","uid":"first"},"spec":{
        "requestPlugins":[{"type":"OpenidConnect","config":{"discovery":"https://idp.example/discovery","clientId":"client"}}]
    }})).unwrap();
    for (namespace, valid) in [
        (None, true),
        (Some("apps"), true),
        (Some(""), false),
        (Some("Bad_NS"), false),
    ] {
        let mut owner = owner.clone();
        owner.metadata.namespace = namespace.map(str::to_owned);
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        if valid {
            assert_eq!(entry.owner_namespace().unwrap(), namespace.unwrap_or("default"));
        } else {
            assert_eq!(entry.owner_namespace().unwrap_err(), ReferenceError::InvalidNamespace);
        }
    }
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let mut changed = owner.clone();
    changed.metadata.uid = Some("second".into());
    assert!(matches!(prepared.bind(&changed), Err(ReferenceError::StaleBinding)));
    for name in [None, Some("bad/name")] {
        changed.metadata.name = name.map(str::to_owned);
        let fresh = PreparedPluginReferences::from_plugins(&changed);
        let binding = fresh.bind(&changed).unwrap();
        assert!(binding.request_entries().next().unwrap().owner_namespace().is_err());
    }
}
