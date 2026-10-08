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
