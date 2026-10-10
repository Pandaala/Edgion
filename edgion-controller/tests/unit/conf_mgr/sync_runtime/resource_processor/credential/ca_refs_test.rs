use super::super::pipeline::fetch_bound_configmap;
use super::*;
use edgion_resources::resources::common::CustomCACertificateRef;
use edgion_resources::resources::refs::plugin_fields::PreparedPluginReferences;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::ByteString;
use kube::api::ObjectMeta;

use crate::conf_mgr::sync_runtime::resource_processor::{HandlerContext, ResourceRef};
use crate::conf_mgr::sync_runtime::workqueue::TriggerChain;
use crate::conf_mgr::ControllerRuntime;
use edgion_resources::ResourceKind;

// ── Test fixture helpers ─────────────────────────────────────────────────

#[test]
fn configmap_material_preserves_fetched_identity_and_replaces_markers() {
    let (_, mut configmap) = make_configmap("material-ns", "actual-name", "certificate");
    configmap.metadata.uid = Some("cm-uid".into());
    configmap.metadata.resource_version = Some("42".into());
    configmap.metadata.annotations = Some(BTreeMap::from([
        (CONTROLLER_CA_SOURCE.into(), "Secret".into()),
        ("operator.example/annotation".into(), "ignored".into()),
    ]));
    let material = configmap_ca_material(&configmap).unwrap();
    assert_eq!(material.metadata.name, configmap.metadata.name);
    assert_eq!(material.metadata.namespace, configmap.metadata.namespace);
    assert_eq!(material.metadata.uid, configmap.metadata.uid);
    assert_eq!(material.metadata.resource_version, configmap.metadata.resource_version);
    assert_eq!(
        material.metadata.annotations,
        Some(BTreeMap::from([(
            CONTROLLER_CA_SOURCE.into(),
            CONTROLLER_CA_SOURCE_CONFIG_MAP.into()
        ),]))
    );
    assert_eq!(material.data.unwrap()[CA_CERT].0, b"certificate");
    for value in [None, Some(""), Some(" \n\t")] {
        configmap.data = value.map(|value| BTreeMap::from([(CA_CERT.into(), value.into())]));
        assert!(configmap_ca_material(&configmap).is_none());
    }
}

#[test]
fn ordinary_secret_cannot_supply_configmap_source_marker() {
    for value in ["ConfigMap", "Secret", "unknown", ""] {
        let (_, mut secret) = make_secret("material-ns", "same-name", b"certificate");
        secret.metadata.annotations = Some(BTreeMap::from([
            (CONTROLLER_CA_SOURCE.into(), value.into()),
            ("operator.example/annotation".into(), "preserved".into()),
        ]));
        let material = ordinary_secret_material(secret);
        let annotations = material.metadata.annotations.unwrap();
        assert!(!annotations.contains_key(CONTROLLER_CA_SOURCE));
        assert_eq!(annotations["operator.example/annotation"], "preserved");
        assert_eq!(material.data.unwrap()[CA_CERT].0, b"certificate");
    }
}

fn make_ctx(
    secrets: HashMap<String, Secret>,
    configmaps: HashMap<String, ConfigMap>,
) -> (HandlerContext, Arc<ControllerRuntime>) {
    let runtime = Arc::new(ControllerRuntime::default());
    runtime.secret_store.replace_all(secrets);
    runtime.configmap_store.replace_all(configmaps);
    let ctx = HandlerContext::new(None, None, TriggerChain::default(), 5, 20, runtime.clone());
    (ctx, runtime)
}

fn make_secret(namespace: &str, name: &str, ca_pem: &[u8]) -> (String, Secret) {
    let mut data = BTreeMap::new();
    data.insert(CA_CERT.to_string(), ByteString(ca_pem.to_vec()));
    let secret = Secret {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    };
    (format!("{}/{}", namespace, name), secret)
}

fn make_configmap(namespace: &str, name: &str, ca_pem: &str) -> (String, ConfigMap) {
    let mut data = BTreeMap::new();
    data.insert(CA_CERT.to_string(), ca_pem.to_string());
    let cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            namespace: Some(namespace.to_string()),
            ..Default::default()
        },
        data: Some(data),
        ..Default::default()
    };
    (format!("{}/{}", namespace, name), cm)
}

fn secret_ref(kind: &str, name: &str, namespace: Option<&str>) -> CustomCACertificateRef {
    CustomCACertificateRef {
        group: Some(String::new()),
        kind: kind.to_string(),
        name: name.to_string(),
        namespace: namespace.map(str::to_string),
    }
}

fn owner_ref(ns: &str, name: &str) -> ResourceRef {
    ResourceRef::new(ResourceKind::EdgionPlugins, Some(ns.to_string()), name.to_string())
}

fn tls_owner(owner: &ResourceRef, references: &[CustomCACertificateRef]) -> edgion_resources::EdgionPlugins {
    serde_json::from_value(serde_json::json!({
        "metadata":{"name":owner.name,"namespace":owner.namespace},
        "spec":{"requestPlugins":[{"type":"LdapAuth","config":{
            "ldapHost":"ldap.example","baseDn":"dc=example","attribute":"uid",
            "tls":{"validation":{"caCertificateRefs":references}}
        }}]}
    }))
    .unwrap()
}

// ── Table tests ──────────────────────────────────────────────────────────

/// (a) Owning same-namespace TLS fields + two Secret refs in the same namespace resolve to two
/// Secrets with the expected CA PEM data.
#[test]
fn resolve_ca_refs_two_secrets_same_ns_direct() {
    let pem_a = b"PEM-CA-A";
    let pem_b = b"PEM-CA-B";
    let (key_a, sec_a) = make_secret("default", "ca-secret-a", pem_a);
    let (key_b, sec_b) = make_secret("default", "ca-secret-b", pem_b);
    let (ctx, _rt) = make_ctx(HashMap::from([(key_a, sec_a), (key_b, sec_b)]), HashMap::new());
    let resource_ref = owner_ref("default", "my-btp");
    let refs = vec![
        secret_ref("Secret", "ca-secret-a", None),
        secret_ref("Secret", "ca-secret-b", None),
    ];

    let resource = tls_owner(&resource_ref, &refs);
    let prepared = PreparedPluginReferences::from_plugins(&resource);
    let bound = prepared.bind(&resource).unwrap();
    let entry = bound.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let result = resolve_bound_tls(&ctx, &resource_ref, references.tls().unwrap())
        .ca_certificates
        .unwrap_or_default();

    assert_eq!(result.len(), 2, "expected 2 resolved CA secrets, got {}", result.len());

    let names: Vec<_> = result.iter().filter_map(|s| s.metadata.name.as_deref()).collect();
    assert!(names.contains(&"ca-secret-a"), "result should contain ca-secret-a");
    assert!(names.contains(&"ca-secret-b"), "result should contain ca-secret-b");

    // Verify data is preserved.
    let data_a = result
        .iter()
        .find(|s| s.metadata.name.as_deref() == Some("ca-secret-a"))
        .and_then(|s| s.data.as_ref())
        .and_then(|d| d.get(CA_CERT))
        .map(|b| b.0.as_slice());
    assert_eq!(
        data_a,
        Some(pem_a.as_slice()),
        "ca-secret-a must carry the original PEM"
    );

    // SecretRefManager must have registered both refs.
    let refs_a = ctx.secret_ref_manager().get_refs("default/ca-secret-a");
    assert!(refs_a.contains(&resource_ref), "ca-secret-a ref must be registered");
    let refs_b = ctx.secret_ref_manager().get_refs("default/ca-secret-b");
    assert!(refs_b.contains(&resource_ref), "ca-secret-b ref must be registered");
}

/// (b) Owning same-namespace TLS fields + one ConfigMap ref resolves to a single synthetic Secret
/// carrying the ConfigMap's `ca.crt` PEM.
#[test]
fn resolve_ca_refs_configmap_synthetic_secret() {
    let ca_pem = "-----BEGIN CERTIFICATE-----\nMIIFAKE\n-----END CERTIFICATE-----\n";
    let (cm_key, cm) = make_configmap("default", "corp-ca-cm", ca_pem);
    let (ctx, _rt) = make_ctx(HashMap::new(), HashMap::from([(cm_key, cm)]));
    let resource_ref = owner_ref("default", "my-btp");
    let refs = vec![secret_ref("ConfigMap", "corp-ca-cm", None)];

    let resource = tls_owner(&resource_ref, &refs);
    let prepared = PreparedPluginReferences::from_plugins(&resource);
    let bound = prepared.bind(&resource).unwrap();
    let entry = bound.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let result = resolve_bound_tls(&ctx, &resource_ref, references.tls().unwrap())
        .ca_certificates
        .unwrap_or_default();

    assert_eq!(result.len(), 1, "expected 1 synthetic CA secret, got {}", result.len());

    let synthetic = &result[0];
    assert_eq!(
        synthetic.metadata.name.as_deref(),
        Some("corp-ca-cm"),
        "synthetic Secret must take the ConfigMap's name"
    );
    assert_eq!(
        synthetic.metadata.namespace.as_deref(),
        Some("default"),
        "synthetic Secret must carry the effective namespace"
    );
    let data = synthetic.data.as_ref().expect("synthetic Secret must have data");
    let got_pem = data.get(CA_CERT).expect("synthetic Secret must have ca.crt key");
    assert_eq!(
        got_pem.0,
        ca_pem.as_bytes(),
        "synthetic Secret ca.crt must contain the ConfigMap PEM"
    );

    // SecretRefManager must have registered the ref.
    let refs = ctx.secret_ref_manager().get_refs("ConfigMap/default/corp-ca-cm");
    assert!(
        refs.contains(&resource_ref),
        "ConfigMap ref must be registered in SecretRefManager"
    );
}

/// (c) Owning grant-gated TLS field + one cross-namespace Secret ref with no ReferenceGrant
/// configured → empty result, and the Secret is NOT accessed directly
/// (verified by confirming the Secret genuinely exists in the store but
/// the grant-check path returns empty).
#[test]
fn resolve_ca_refs_cross_ns_no_grant_returns_empty() {
    // Secret genuinely exists in "other-ns".
    let (secret_key, secret) = make_secret("other-ns", "cross-ns-ca", b"REAL-PEM");
    let (ctx, _rt) = make_ctx(HashMap::from([(secret_key, secret)]), HashMap::new());

    // Verify the Secret is reachable directly (proves the empty result is
    // not because of an absent secret but because of the missing grant).
    let direct = ctx.runtime().secret_store.get(Some("other-ns"), "cross-ns-ca");
    assert!(
        direct.is_some(),
        "Secret must exist in the store for this test to be meaningful"
    );

    let resource_ref = owner_ref("default", "my-plugin");
    // Ref explicitly targets "other-ns" — cross-namespace from "default".
    let refs = vec![secret_ref("Secret", "cross-ns-ca", Some("other-ns"))];

    let resource = tls_owner(&resource_ref, &refs);
    let prepared = PreparedPluginReferences::from_plugins(&resource);
    let bound = prepared.bind(&resource).unwrap();
    let entry = bound.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let result = resolve_bound_tls(&ctx, &resource_ref, references.tls().unwrap())
        .ca_certificates
        .unwrap_or_default();

    // No ReferenceGrant in "other-ns" → ReferenceGrantMissing → skipped.
    assert_eq!(
        result.len(),
        0,
        "cross-ns ref without a grant must not resolve; got {} items",
        result.len()
    );
}

// ── Additional test: fetch_configmap directly ────────────────────────────

/// fetch_configmap returns Pending(SecretNotFound) when the ConfigMap is absent.
#[test]
fn fetch_configmap_returns_pending_when_missing() {
    use super::super::pipeline::{BoundFetchOutcome, PendingReason};

    let (ctx, _rt) = make_ctx(HashMap::new(), HashMap::new());
    let resource_ref = owner_ref("default", "my-resource");

    let resource = tls_owner(&resource_ref, &[secret_ref("ConfigMap", "missing-cm", None)]);
    let prepared = PreparedPluginReferences::from_plugins(&resource);
    let bound = prepared.bind(&resource).unwrap();
    let entry = bound.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let tls = references.tls().unwrap();
    let outcome = fetch_bound_configmap(&ctx, &resource_ref, tls.ca_certificates().next().unwrap());

    match outcome {
        BoundFetchOutcome::Pending(PendingReason::SecretNotFound { namespace, name }) => {
            assert_eq!(namespace, "default");
            assert_eq!(name, "missing-cm");
        }
        other => panic!("expected Pending(SecretNotFound), got {:?}", other),
    }
}

/// fetch_configmap returns Pending(ReferenceGrantMissing) when the ConfigMap
/// is in a different namespace and no ReferenceGrant is present.
#[test]
fn fetch_configmap_cross_ns_no_grant_returns_pending() {
    use super::super::pipeline::{BoundFetchOutcome, PendingReason};

    let ca_pem = "PEM";
    let (cm_key, cm) = make_configmap("other-ns", "ca-cm", ca_pem);
    let (ctx, _rt) = make_ctx(HashMap::new(), HashMap::from([(cm_key, cm)]));
    let resource_ref = owner_ref("default", "my-resource");

    let resource = tls_owner(&resource_ref, &[secret_ref("ConfigMap", "ca-cm", Some("other-ns"))]);
    let prepared = PreparedPluginReferences::from_plugins(&resource);
    let bound = prepared.bind(&resource).unwrap();
    let entry = bound.request_entries().next().unwrap();
    let references = entry.references().unwrap();
    let tls = references.tls().unwrap();
    let outcome = fetch_bound_configmap(&ctx, &resource_ref, tls.ca_certificates().next().unwrap());

    match outcome {
        BoundFetchOutcome::Pending(PendingReason::ReferenceGrantMissing {
            from_ns,
            to_ns,
            secret_name,
        }) => {
            assert_eq!(from_ns, "default");
            assert_eq!(to_ns, "other-ns");
            assert_eq!(secret_name, "ca-cm");
        }
        other => panic!("expected Pending(ReferenceGrantMissing), got {:?}", other),
    }
    assert!(ctx
        .runtime()
        .cross_ns_ref_mgr
        .get_refs("other-ns")
        .contains(&resource_ref));
}
