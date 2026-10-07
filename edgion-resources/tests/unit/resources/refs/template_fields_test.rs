use crate::resources::edgion_plugins::EdgionPlugin;
use crate::resources::refs::link_sys_fields::PreparedLinkSysReferences;
use crate::resources::refs::plugin_fields::PreparedPluginReferences;
use serde_json::{json, Value};

fn plugins(variant: &str, stage: &str, enabled: bool, request: Value) -> crate::EdgionPlugins {
    let config = match variant {
        "ForwardAuth" => json!({"target":{"url":"https://auth.example"},"decision":{},"request":request}),
        "Guardrail" => json!({"target":{"url":"https://guard.example"},"request":request}),
        "AiGuard" => json!({"evaluator":{"target":{"url":"https://guard.example"},"request":request}}),
        _ => panic!("unexpected fixture variant"),
    };
    let mut wire = json!({"metadata":{"name":"plugins","namespace":"apps","uid":"uid","generation":1},"spec":{}});
    wire["spec"][stage] = json!([{"type":variant,"enable":enabled,"config":config}]);
    serde_json::from_value(wire).unwrap()
}

fn request(template: &str) -> Value {
    json!({"headers":{"custom":[{"name":"Authorization","template":template}]}})
}

fn webhook(request: Value) -> crate::LinkSys {
    serde_json::from_value(
        json!({"metadata":{"name":"hook","namespace":"links","uid":"uid","generation":1},
        "spec":{"type":"webhook","config":{"target":{"url":"https://hook.example"},"request":request}}}),
    )
    .unwrap()
}

#[test]
fn request_secrets_derive_actual_owner_for_each_consumer() {
    for variant in ["ForwardAuth", "Guardrail", "AiGuard"] {
        let owner = plugins(
            variant,
            "requestPlugins",
            true,
            request("Bearer ${secretRef:ops/token:key}"),
        );
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let references = entry.references().unwrap();
        let secret = references.request_secrets().next().unwrap();
        let reference = secret.reference().validated().unwrap();
        assert_eq!(secret.map_key(), "ops/token:key");
        assert_eq!(secret.data_key(), "key");
        assert_eq!(reference.source().group_kind().kind(), "EdgionPlugins");
        assert_eq!(reference.source().scope().namespace(), Some("apps"));
        assert_eq!(reference.target().scope().namespace(), Some("ops"));
        assert_eq!(reference.target().group_kind().group(), "");
        assert_eq!(reference.target().group_kind().kind(), "Secret");
    }
    let owner = webhook(request("${secretRef:ops/token:key}"));
    let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    let binding = prepared.bind(&owner).unwrap();
    let references = binding.references().unwrap();
    let secret = references.request_secrets().next().unwrap();
    let reference = secret.reference().validated().unwrap();
    assert_eq!(reference.source().group_kind().kind(), "LinkSys");
    assert_eq!(reference.source().scope().namespace(), Some("links"));
    assert_eq!(reference.target().scope().namespace(), Some("ops"));
}

#[test]
fn request_secret_grammar_survives_borrowing_and_freezing() {
    for (variable, namespace, name, key) in [
        ("ops/token:key", "ops", "token", "key"),
        ("NS/nested/token:part:key", "NS", "nested/token:part", "key"),
        ("ops/token:field#part", "ops", "token", "field#part"),
        (" ops/token : key ", " ops", "token ", " key "),
    ] {
        let owner = webhook(request(&format!("${{secretRef:{variable}}}")));
        let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let references = binding.references().unwrap();
        let secret = references.request_secrets().next().unwrap();
        let reference = secret.reference().validated().unwrap();
        assert_eq!(secret.map_key(), variable);
        assert_eq!(secret.data_key(), key);
        assert_eq!(reference.target().scope().namespace(), Some(namespace));
        assert_eq!(reference.target().name(), name);
        let frozen = secret.reference().freeze().unwrap();
        assert!(frozen.matches_link_sys(&owner));
        let query = frozen.grant_query().unwrap();
        assert_eq!(query.target().scope().namespace(), Some(namespace));
        assert_eq!(query.target().name(), name);
    }
}

#[test]
fn malformed_template_secret_cannot_freeze_or_create_identity() {
    for template in [
        "${secretRef}",
        "${secretRef:token:key}",
        "${secretRef:/token:key}",
        "${secretRef:ops/:key}",
        "${secretRef:ops/token:}",
    ] {
        let owner = webhook(request(template));
        let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let references = binding.references().unwrap();
        let secret = references.request_secrets().next().unwrap();
        assert!(secret.reference().validated().is_err(), "{template}");
        assert!(secret.reference().freeze().is_err(), "{template}");
    }
    let owner = webhook(request("${secretRef:ops/token:key"));
    let prepared = PreparedLinkSysReferences::from_link_sys(&owner);
    let binding = prepared.bind(&owner).unwrap();
    assert_eq!(binding.references().unwrap().request_secrets().count(), 0);
}

#[test]
fn all_request_surfaces_keep_distinct_positions_and_exclude_literals() {
    for body in [
        json!({"type":"json","template":"{\"token\":\"${secretRef:ops/token:key}\"}"}),
        json!({"type":"raw","template":"${secretRef:ops/token:key}"}),
        json!({"type":"form","fields":[{"name":"token","template":"${secretRef:ops/token:key}"},{"name":"literal","value":"${secretRef:ops/ignored:key}"}]}),
    ] {
        let field = json!({"custom":[{"name":"token","template":"${secretRef:ops/token:key}"},
            {"name":"literal","value":"${secretRef:ops/ignored:key}"}],"origin":[{"name":"x-source"}]});
        let owner = plugins(
            "ForwardAuth",
            "requestPlugins",
            true,
            json!({
            "path":{"template":"/${secretRef:ops/token:key}"},"method":{"template":"${secretRef:ops/token:key}"},
            "args":field,"headers":field,"cookies":field,"body":body}),
        );
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let references = entry.references().unwrap();
        let frozen: Vec<_> = references
            .request_secrets()
            .map(|secret| secret.reference().freeze().unwrap())
            .collect();
        assert_eq!(frozen.len(), 6);
        let positions: std::collections::BTreeSet<_> =
            frozen.iter().map(|reference| reference.slot_revision()).collect();
        assert_eq!(positions.len(), 6);
        assert!(frozen.iter().all(|reference| reference.same_owner_snapshot(&frozen[0])));
    }
}

#[test]
fn literal_and_non_secret_variable_changes_preserve_reference_authority() {
    let mut owner = plugins(
        "ForwardAuth",
        "requestPlugins",
        true,
        request("Bearer ${secretRef:ops/token:key}"),
    );
    let prepared = PreparedPluginReferences::from_plugins(&owner);
    let frozen = {
        let binding = prepared.bind(&owner).unwrap();
        let entry = binding.request_entries().next().unwrap();
        let references = entry.references().unwrap();
        let frozen = references
            .request_secrets()
            .next()
            .unwrap()
            .reference()
            .freeze()
            .unwrap();
        frozen
    };
    let EdgionPlugin::ForwardAuth(config) = &mut owner.spec.request_plugins.as_mut().unwrap()[0].plugin else {
        panic!("expected ForwardAuth")
    };
    config.request =
        serde_json::from_value(request("prefix ${header:x-prefix}/${secretRef:ops/token:key} suffix")).unwrap();
    config.resolved_secrets = Some(crate::resources::common::ResolvedSecrets(
        std::collections::BTreeMap::from([("ops/token:key".into(), "sensitive-material".into())]),
    ));
    assert!(prepared.bind(&owner).is_ok());
    assert!(frozen.matches_plugins(&owner));
    let current = PreparedPluginReferences::from_plugins(&owner);
    let binding = current.bind(&owner).unwrap();
    let entry = binding.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    assert_eq!(
        frozen,
        references
            .request_secrets()
            .next()
            .unwrap()
            .reference()
            .freeze()
            .unwrap()
    );
}

#[test]
fn changed_reference_key_order_variant_owner_and_activity_reject_snapshot() {
    let original =
        webhook(json!({"body":{"type":"json","template":"\"${secretRef:ops/one:key}${secretRef:ops/two:key}\""}}));
    let prepared = PreparedLinkSysReferences::from_link_sys(&original);
    let binding = prepared.bind(&original).unwrap();
    let references = binding.references().unwrap();
    let frozen = references
        .request_secrets()
        .next()
        .unwrap()
        .reference()
        .freeze()
        .unwrap();
    for body in [
        json!({"type":"json","template":"\"${secretRef:ops/one:other}${secretRef:ops/two:key}\""}),
        json!({"type":"json","template":"\"${secretRef:ops/two:key}${secretRef:ops/one:key}\""}),
        json!({"type":"raw","template":"\"${secretRef:ops/one:key}${secretRef:ops/two:key}\""}),
    ] {
        let changed = webhook(json!({"body":body}));
        assert!(prepared.bind(&changed).is_err());
        assert!(!frozen.matches_link_sys(&changed));
    }
    for (stage, enabled) in [
        ("requestPlugins", false),
        ("upstreamResponsePlugins", true),
        ("upstreamResponseFilterPlugins", true),
        ("upstreamResponseBodyFilterPlugins", true),
    ] {
        let owner = plugins("ForwardAuth", stage, enabled, request("${secretRef:ops/token:key}"));
        let prepared = PreparedPluginReferences::from_plugins(&owner);
        let binding = prepared.bind(&owner).unwrap();
        let mut count = 0;
        for entry in binding
            .request_entries()
            .chain(binding.response_entries())
            .chain(binding.response_filter_entries())
            .chain(binding.response_body_filter_entries())
        {
            count += 1;
            assert_eq!(entry.references().unwrap().request_secrets().count(), 0);
        }
        assert_eq!(count, 1);
    }
}
