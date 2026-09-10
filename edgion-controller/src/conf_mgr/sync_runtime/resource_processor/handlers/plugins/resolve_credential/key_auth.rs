//! KeyAuth credential resolution.

use std::collections::{hash_map::Entry, HashMap, HashSet};

use crate::conf_mgr::sync_runtime::resource_processor::credential::{fetch_secret, FetchOutcome, PendingReason};
use crate::conf_mgr::sync_runtime::resource_processor::{format_secret_key, HandlerContext, ResourceRef};
use edgion_resources::prelude_resources::EdgionPlugins;
use edgion_resources::resources::edgion_plugins::plugin_configs::KeyMetadata;
use edgion_resources::resources::edgion_plugins::EdgionPlugin;

/// Resolve all Secret refs for a single group, appending entries to
/// `all_keys`. Cross-group duplicate keys follow **first-wins**: if a
/// key already exists in `all_keys`, the new entry is dropped
/// and a warn is logged with both groups.
#[allow(clippy::too_many_arguments)]
fn resolve_group(
    config_key_field: &str,
    whitelist: &HashSet<&str>,
    group_name: &str,
    secret_refs: &[edgion_resources::resources::gateway::SecretObjectReference],
    all_keys: &mut HashMap<String, KeyMetadata>,
    ep_namespace: Option<&String>,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    for sor in secret_refs {
        let secret_key = format_secret_key(sor.namespace.as_ref().or(ep_namespace), &sor.name);
        let secret = match fetch_secret(ctx, resource_ref, "edgion.io", "EdgionPlugins", sor) {
            FetchOutcome::Ok(s) => s,
            FetchOutcome::Pending(PendingReason::SecretNotFound { .. }) => {
                tracing::info!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    "KeyAuth: Secret not found yet, will be reprocessed when Secret arrives"
                );
                continue;
            }
            FetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
                ref from_ns,
                ref to_ns,
                ref secret_name,
            }) => {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    from_ns = %from_ns,
                    to_ns = %to_ns,
                    secret_name = %secret_name,
                    group = %group_name,
                    "KeyAuth: cross-namespace Secret reference denied (ReferenceGrant missing)"
                );
                continue;
            }
        };

        let Some(data) = &secret.data else {
            tracing::warn!(
                edgion_plugins = %resource_ref.key(),
                secret_key = %secret_key,
                group = %group_name,
                "KeyAuth: Secret has no data"
            );
            continue;
        };
        let Some(keys_yaml_bytes) = data.get("keys.yaml") else {
            tracing::warn!(
                edgion_plugins = %resource_ref.key(),
                secret_key = %secret_key,
                group = %group_name,
                "KeyAuth: Secret missing 'keys.yaml' field"
            );
            continue;
        };
        let keys_yaml_str = match std::str::from_utf8(&keys_yaml_bytes.0) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    error = %e,
                    "KeyAuth: Failed to decode keys.yaml as UTF-8"
                );
                continue;
            }
        };

        let keys_list: Vec<HashMap<String, String>> = match serde_yaml::from_str(keys_yaml_str) {
            Ok(list) => list,
            Err(e) => {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    line = e.location().map(|location| location.line()),
                    column = e.location().map(|location| location.column()),
                    "KeyAuth: Failed to parse keys.yaml"
                );
                continue;
            }
        };

        for mut key_entry in keys_list {
            let Some(key_value) = key_entry.remove(config_key_field) else {
                tracing::warn!(
                    edgion_plugins = %resource_ref.key(),
                    secret_key = %secret_key,
                    group = %group_name,
                    key_field = %config_key_field,
                    "KeyAuth: Key entry missing key field"
                );
                continue;
            };

            let headers = key_entry
                .into_iter()
                .filter(|(field, _)| whitelist.contains(field.as_str()))
                .collect();

            if let Err(prev_group) = first_wins_insert(all_keys, key_value, headers, group_name) {
                // first-wins rejected this entry; warn with both groups (or the
                // single group for an intra-group duplicate). secret_key keeps
                // operators able to locate the offending Secret entry.
                if prev_group != group_name {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        secret_key = %secret_key,
                        existing_group = %prev_group,
                        incoming_group = %group_name,
                        "KeyAuth: duplicate API key across groups; first-wins kept"
                    );
                } else {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        secret_key = %secret_key,
                        group = %group_name,
                        "KeyAuth: Duplicate API key within group, skipping"
                    );
                }
            }
        }
    }
}

/// Resolve KeyAuth keys from Secrets and register references to SecretRefManager.
pub(in crate::conf_mgr::sync_runtime::resource_processor::handlers) fn resolve(
    ep: &mut EdgionPlugins,
    resource_ref: &ResourceRef,
    ctx: &HandlerContext,
) {
    if let Some(ref mut plugins) = ep.spec.request_plugins {
        for entry in plugins.iter_mut() {
            if let EdgionPlugin::KeyAuth(ref mut config) = entry.plugin {
                // This field is Controller-owned transport state. Never trust
                // a value deserialized from the operator-facing resource, and
                // do not retain credentials from a previous resolution pass.
                config.resolved_keys = None;

                let Some(ref groups) = config.secret_groups else {
                    continue;
                };

                let whitelist: HashSet<&str> = config.upstream_header_fields.iter().map(|s| s.as_str()).collect();
                let key_field = config.key_field.clone();

                let mut all_keys: HashMap<String, KeyMetadata> = HashMap::new();

                for g in groups {
                    let group_name: String = g.name.clone();
                    resolve_group(
                        &key_field,
                        &whitelist,
                        &group_name,
                        &g.secret_refs,
                        &mut all_keys,
                        ep.metadata.namespace.as_ref(),
                        resource_ref,
                        ctx,
                    );
                }

                if !all_keys.is_empty() {
                    tracing::info!(
                        edgion_plugins = %resource_ref.key(),
                        key_count = all_keys.len(),
                        group_count = groups.len(),
                        "KeyAuth: Resolved API keys from Secret groups"
                    );
                    config.resolved_keys = Some(all_keys);
                } else {
                    tracing::warn!(
                        edgion_plugins = %resource_ref.key(),
                        "KeyAuth: No API keys resolved from Secret groups"
                    );
                }
            }
        }
    }
}

/// Pure helper for first-wins duplicate detection.
///
/// Inserts `key` into `all_keys` with the given `headers` and `group` only if
/// `all_keys` does not already contain `key`. Returns the retained group on a
/// duplicate (first-wins: original entry is kept), without copying its name.
fn first_wins_insert<'a>(
    all_keys: &'a mut HashMap<String, KeyMetadata>,
    key: String,
    headers: HashMap<String, String>,
    group: &str,
) -> Result<(), &'a str> {
    match all_keys.entry(key) {
        Entry::Occupied(entry) => Err(&entry.into_mut().group),
        Entry::Vacant(entry) => {
            entry.insert(KeyMetadata {
                headers,
                group: group.to_string(),
            });
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_preserves_metadata_warnings_and_replaces_credentials() {
        use crate::conf_mgr::ControllerRuntime;
        use edgion_resources::resources::edgion_plugins::plugin_configs::{KeyAuthConfig, KeyAuthSecretGroup};
        use edgion_resources::resources::edgion_plugins::{EdgionPluginsSpec, RequestFilterEntry};
        use edgion_resources::resources::gateway::SecretObjectReference;
        use k8s_openapi::api::core::v1::Secret;
        use k8s_openapi::ByteString;
        use std::io::{Read, Seek};
        use std::sync::Arc;

        let secret = |name: &str, yaml: &str| Secret {
            metadata: kube::api::ObjectMeta {
                name: Some(name.into()),
                namespace: Some("default".into()),
                ..Default::default()
            },
            data: Some(std::collections::BTreeMap::from([(
                "keys.yaml".into(),
                ByteString(yaml.as_bytes().to_vec()),
            )])),
            ..Default::default()
        };
        let runtime = Arc::new(ControllerRuntime::default());
        runtime.secret_store.replace_all(HashMap::from([
            (
                "default/first".into(),
                secret("first", "- apiKey: ' private-key-é '\n  x-owner: first\n  ignored: private-metadata\n- apiKey: ' private-key-é '\n  x-owner: rejected-within\n- x-owner: missing-key\n- apiKey: ''\n  x-owner: empty-key\n"),
            ),
            (
                "default/second".into(),
                secret("second", "- apiKey: ' private-key-é '\n  x-owner: rejected-across\n- apiKey: other-key\n  x-owner: second\n"),
            ),
        ]));
        let ctx = HandlerContext::new(None, None, Default::default(), 5, 20, Arc::clone(&runtime));
        let mut ep = EdgionPlugins::new("key-ownership", EdgionPluginsSpec::default());
        ep.metadata.namespace = Some("default".into());
        ep.spec.request_plugins = Some(vec![RequestFilterEntry::new(EdgionPlugin::KeyAuth(KeyAuthConfig {
            key_field: "apiKey".into(),
            upstream_header_fields: vec!["apiKey".into(), "x-owner".into()],
            secret_groups: Some(
                [("group-a", "first"), ("group-b", "second")]
                    .into_iter()
                    .map(|(group, name)| KeyAuthSecretGroup {
                        name: group.into(),
                        description: None,
                        secret_refs: vec![SecretObjectReference {
                            group: None,
                            kind: None,
                            namespace: None,
                            name: name.into(),
                        }],
                    })
                    .collect(),
            ),
            ..Default::default()
        }))]);
        let owner = ResourceRef::new(
            edgion_resources::ResourceKind::EdgionPlugins,
            Some("default".into()),
            "key-ownership".into(),
        );
        let mut log_file = tempfile::tempfile().unwrap();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(log_file.try_clone().unwrap())
            .with_max_level(tracing::Level::WARN)
            .without_time()
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || resolve(&mut ep, &owner, &ctx));
        log_file.rewind().unwrap();
        let mut logs = String::new();
        log_file.read_to_string(&mut logs).unwrap();
        assert!(!logs.contains("private-key"));
        assert!(!logs.contains("private-metadata"));
        let within = logs
            .lines()
            .find(|line| line.contains("Duplicate API key within group"))
            .unwrap();
        assert!(within.contains("secret_key=default/first") && within.contains("group=group-a"));
        let across = logs
            .lines()
            .find(|line| line.contains("duplicate API key across groups"))
            .unwrap();
        assert!(across.contains("existing_group=group-a") && across.contains("incoming_group=group-b"));
        assert!(across.contains("secret_key=default/second"));
        assert!(logs.contains("Key entry missing key field"));

        let EdgionPlugin::KeyAuth(config) = &ep.spec.request_plugins.as_ref().unwrap()[0].plugin else {
            panic!("expected KeyAuth")
        };
        let keys = config.resolved_keys.as_ref().unwrap();
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[" private-key-é "].group, "group-a");
        assert_eq!(
            keys[" private-key-é "].headers,
            HashMap::from([("x-owner".into(), "first".into())])
        );
        assert_eq!(keys[""].headers["x-owner"], "empty-key");
        assert_eq!(keys["other-key"].group, "group-b");
        for name in ["first", "second"] {
            assert_eq!(ctx.secret_ref_manager().get_refs(&format!("default/{name}")).len(), 1);
        }

        runtime.secret_store.replace_all(HashMap::from([(
            "default/first".into(),
            secret("first", "- apiKey: rotated-key\n  x-owner: rotated\n"),
        )]));
        resolve(&mut ep, &owner, &ctx);
        let EdgionPlugin::KeyAuth(config) = &ep.spec.request_plugins.as_ref().unwrap()[0].plugin else {
            panic!("expected KeyAuth")
        };
        let keys = config.resolved_keys.as_ref().unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys["rotated-key"].headers["x-owner"], "rotated");
        runtime.secret_store.replace_all(HashMap::new());
        resolve(&mut ep, &owner, &ctx);
        let EdgionPlugin::KeyAuth(config) = &ep.spec.request_plugins.as_ref().unwrap()[0].plugin else {
            panic!("expected KeyAuth")
        };
        assert!(config.resolved_keys.is_none());
    }

    #[test]
    fn first_wins_second_group_rejected() {
        let mut all_keys: HashMap<String, KeyMetadata> = HashMap::new();

        let group_a: String = "partner-a".to_string();
        let group_b: String = "partner-b".to_string();

        // First group inserts the key — should succeed.
        let inserted = first_wins_insert(&mut all_keys, "dup-key".to_string(), HashMap::new(), &group_a);
        assert!(inserted.is_ok(), "first insert must succeed");

        // Second group tries the same key — must be rejected.
        let inserted_again = first_wins_insert(&mut all_keys, "dup-key".to_string(), HashMap::new(), &group_b);
        assert_eq!(
            inserted_again,
            Err("partner-a"),
            "duplicate must report the retained group"
        );

        // Verify first-wins: the stored group is partner-a, not partner-b.
        assert_eq!(
            all_keys.get("dup-key").unwrap().group.as_str(),
            "partner-a",
            "first group must win"
        );

        assert_eq!(all_keys.len(), 1, "duplicate must not add an entry");
    }

    #[test]
    fn first_wins_distinct_keys_both_inserted() {
        let mut all_keys: HashMap<String, KeyMetadata> = HashMap::new();

        let group_a: String = "partner-a".to_string();
        let group_b: String = "partner-b".to_string();

        assert!(first_wins_insert(&mut all_keys, "key-a".to_string(), HashMap::new(), &group_a,).is_ok());
        assert!(first_wins_insert(&mut all_keys, "key-b".to_string(), HashMap::new(), &group_b,).is_ok());

        assert_eq!(all_keys.len(), 2);
        assert_eq!(all_keys.get("key-a").unwrap().group.as_str(), "partner-a");
        assert_eq!(all_keys.get("key-b").unwrap().group.as_str(), "partner-b");
    }
}
