use super::validate_resource_api_version;
use edgion_resources::{
    BackendTLSPolicy, Gateway, GatewayClass, HTTPRoute, ReferenceGrant, TCPRoute, TLSRoute, UDPRoute,
};

#[test]
fn last_pre_v1_sources_convert_to_canonical_resources() {
    let cases = [
        ("GRPCRoute", "v1alpha2"),
        ("TLSRoute", "v1alpha3"),
        ("TCPRoute", "v1alpha2"),
        ("UDPRoute", "v1alpha2"),
        ("BackendTLSPolicy", "v1alpha3"),
    ];
    for (kind, version) in cases {
        let mut source = serde_json::json!({
            "apiVersion": format!("gateway.networking.k8s.io/{version}"),
            "kind": kind,
            "metadata": {"name": "route", "namespace": "default", "resourceVersion": "42"},
            "spec": {}
        });
        if kind == "BackendTLSPolicy" {
            source["spec"] = serde_json::json!({
                "targetRefs": [{"group": "", "kind": "Service", "name": "backend"}],
                "validation": {"hostname": "backend.example.com"}
            });
            source["status"] = serde_json::json!({"ancestors": []});
        } else {
            source["spec"]["rules"] = serde_json::json!([{"backendRefs": [{"name": "backend", "port": 8080}]}]);
            if kind == "TLSRoute" {
                source["spec"]["hostnames"] = serde_json::json!(["example.com"]);
            }
            source["status"] = serde_json::json!({"parents": []});
        }
        let canonical = match kind {
            "GRPCRoute" => serde_json::to_value(
                super::parse_resource_content_offline::<edgion_resources::GRPCRoute>(&source.to_string()).unwrap(),
            )
            .unwrap(),
            "TLSRoute" => {
                serde_json::to_value(super::parse_resource_content_offline::<TLSRoute>(&source.to_string()).unwrap())
                    .unwrap()
            }
            "TCPRoute" => {
                serde_json::to_value(super::parse_resource_content_offline::<TCPRoute>(&source.to_string()).unwrap())
                    .unwrap()
            }
            "UDPRoute" => {
                serde_json::to_value(super::parse_resource_content_offline::<UDPRoute>(&source.to_string()).unwrap())
                    .unwrap()
            }
            "BackendTLSPolicy" => serde_json::to_value(
                super::parse_resource_content_offline::<BackendTLSPolicy>(&source.to_string()).unwrap(),
            )
            .unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(canonical["apiVersion"], "gateway.networking.k8s.io/v1", "{kind}");
        assert_eq!(canonical["metadata"], source["metadata"], "{kind}");
        assert_eq!(canonical["spec"]["rules"], source["spec"]["rules"], "{kind}");
        assert_eq!(canonical["status"], source["status"], "{kind}");
    }
}

#[test]
fn grpc_alpha_source_schema_rejects_too_many_matches_before_conversion() {
    let matches = vec![serde_json::json!({"method": {"service": "example.Service"}}); 9];
    let source = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1alpha2",
        "kind": "GRPCRoute",
        "metadata": {"name": "route", "namespace": "default"},
        "spec": {"rules": [{"matches": matches, "backendRefs": [{"name": "backend", "port": 8080}]}]}
    });
    assert!(super::parse_resource_content_offline::<edgion_resources::GRPCRoute>(&source.to_string()).is_err());
}

#[test]
fn gateway_class_beta_preserves_source_fields_and_status() {
    let raw = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "GatewayClass",
        "metadata": {"name": "edge", "uid": "uid-1", "resourceVersion": "42", "generation": 3},
        "spec": {"controllerName": "example.com/edge", "description": "edge class",
            "parametersRef": {"group": "edgion.io", "kind": "EdgionGatewayConfig", "name": "config", "namespace": "infra"}},
        "status": {"conditions": [{"type": "Accepted", "status": "True", "reason": "Accepted",
            "message": "ready", "lastTransitionTime": "2026-01-01T00:00:00Z"}],
            "supportedFeatures": [{"name": "Gateway"}]}
    });
    let parsed: GatewayClass = super::parse_resource_content(&raw.to_string()).unwrap();
    let canonical = serde_json::to_value(parsed).unwrap();
    assert_eq!(canonical["apiVersion"], "gateway.networking.k8s.io/v1");
    for key in ["metadata", "status"] {
        assert_eq!(canonical[key], raw[key]);
    }
    assert_eq!(canonical["spec"], raw["spec"]);
    for (api_version, kind) in [
        ("gateway.networking.k8s.io/v1alpha2", "GatewayClass"),
        ("gateway.networking.k8s.io/v1beta1", "Other"),
    ] {
        let mut invalid = raw.clone();
        invalid["apiVersion"] = serde_json::json!(api_version);
        invalid["kind"] = serde_json::json!(kind);
        assert!(super::parse_resource_content::<GatewayClass>(&invalid.to_string()).is_err());
    }
    let mut future = raw;
    future["spec"]["futureStandardField"] = serde_json::json!({"mode": "future"});
    let parsed: GatewayClass = super::parse_resource_content(&future.to_string()).unwrap();
    assert_eq!(parsed.spec.controller_name, "example.com/edge");
    assert_eq!(parsed.metadata.resource_version.as_deref(), Some("42"));
}

#[test]
fn gateway_beta_source_converts_and_preserves_known_unsupported_fields() {
    let source = serde_json::json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "Gateway",
        "metadata": {"name": "edge", "namespace": "demo", "resourceVersion": "17"},
        "spec": {"gatewayClassName": "edgion",
            "listeners": [{"name": "https", "port": 443, "protocol": "HTTPS"}],
            "infrastructure": {"labels": {"team": "web"}}}
    });
    let gateway: Gateway = super::parse_resource_content(&source.to_string()).unwrap();
    assert_eq!(gateway.metadata.resource_version.as_deref(), Some("17"));
    assert_eq!(gateway.spec.listeners.as_ref().unwrap()[0].name, "https");
    assert!(gateway.spec.infrastructure.is_some());
    let mut future = source;
    future["spec"].as_object_mut().unwrap().remove("infrastructure");
    future["spec"]["futureStandardField"] = serde_json::json!(true);
    assert!(super::parse_resource_content::<Gateway>(&future.to_string()).is_ok());
}

#[test]
fn reference_grant_sources_convert_to_the_same_canonical_rules() {
    let document = |version: &str| {
        format!(
            "apiVersion: gateway.networking.k8s.io/{version}\nkind: ReferenceGrant\nmetadata:\n  name: allow\n  namespace: backend\n  resourceVersion: \"17\"\n  labels: {{team: edge}}\nspec:\n  from:\n    - {{group: gateway.networking.k8s.io, kind: HTTPRoute, namespace: app}}\n    - {{group: gateway.networking.k8s.io, kind: GRPCRoute, namespace: grpc}}\n  to:\n    - {{group: '', kind: Service, name: exact}}\n    - {{group: '', kind: Secret}}\n"
        )
    };
    let v1: ReferenceGrant = super::parse_resource_content(&document("v1")).unwrap();
    let beta: ReferenceGrant = super::parse_resource_content(&document("v1beta1")).unwrap();
    assert_eq!(beta.metadata, v1.metadata);
    assert_eq!(beta.spec.from, v1.spec.from);
    assert_eq!(beta.spec.to, v1.spec.to);
    assert_eq!(beta.metadata.resource_version.as_deref(), Some("17"));
    assert_eq!(beta.spec.from.len(), 2);
    assert_eq!(beta.spec.to.len(), 2);
}

#[test]
fn reference_grant_rejects_invalid_source_identity_and_required_rules() {
    let valid = "apiVersion: gateway.networking.k8s.io/v1beta1\nkind: ReferenceGrant\nmetadata:\n  name: allow\n  namespace: backend\nspec:\n  from: []\n  to: []\n";
    for bad in [
        valid.replace("kind: ReferenceGrant", "kind: ReferenceGrantBeta"),
        valid.replace("gateway.networking.k8s.io", "other.example"),
        valid.replace("v1beta1", "v1alpha2"),
        valid.replace("  from: []\n", ""),
        valid.replace("  to: []\n", ""),
    ] {
        assert!(
            super::parse_resource_content::<ReferenceGrant>(&bad).is_err(),
            "accepted {bad}"
        );
    }
}

#[test]
fn canonical_raw_guards_reject_missing_and_old_versions() {
    for kind in ["TLSRoute", "BackendTLSPolicy"] {
        let document = |version: Option<&str>| {
            let api_version = version
                .map(|value| format!("apiVersion: {value}\n"))
                .unwrap_or_default();
            format!("{api_version}kind: {kind}\nmetadata:\n  name: test\nspec: {{}}\n")
        };
        for version in [None, Some("gateway.networking.k8s.io/v1beta1")] {
            let raw = document(version);
            let result = if kind == "TLSRoute" {
                validate_resource_api_version::<TLSRoute>(&raw)
            } else {
                validate_resource_api_version::<BackendTLSPolicy>(&raw)
            };
            assert!(result.is_err(), "{kind} accepted {version:?}");
        }
        let raw = document(Some("gateway.networking.k8s.io/v1"));
        let result = if kind == "TLSRoute" {
            validate_resource_api_version::<TLSRoute>(&raw)
        } else {
            validate_resource_api_version::<BackendTLSPolicy>(&raw)
        };
        assert!(result.is_ok(), "{kind} rejected v1");
    }
}

#[test]
fn canonical_sources_reject_bad_type_meta_in_yaml_and_json() {
    type Check = fn(&str) -> serde_yaml::Result<()>;
    for (kind, check) in [
        ("GatewayClass", validate_resource_api_version::<GatewayClass> as Check),
        ("Gateway", validate_resource_api_version::<Gateway> as Check),
        ("HTTPRoute", validate_resource_api_version::<HTTPRoute> as Check),
        ("TCPRoute", validate_resource_api_version::<TCPRoute> as Check),
        ("UDPRoute", validate_resource_api_version::<UDPRoute> as Check),
        (
            "ReferenceGrant",
            validate_resource_api_version::<ReferenceGrant> as Check,
        ),
    ] {
        for api_version in ["gateway.networking.k8s.io/v1beta1", "other.example/v1"] {
            if matches!(kind, "ReferenceGrant" | "GatewayClass" | "Gateway" | "HTTPRoute")
                && api_version.ends_with("/v1beta1")
            {
                continue;
            }
            let yaml = format!("apiVersion: {api_version}\nkind: {kind}\nmetadata:\n  name: test\nspec: {{}}\n");
            let json = serde_json::json!({"apiVersion":api_version,"kind":kind,"metadata":{"name":"test"},"spec":{}});
            assert!(check(&yaml).is_err(), "{kind} accepted {api_version} YAML");
            assert!(check(&json.to_string()).is_err(), "{kind} accepted {api_version} JSON");
        }
        let missing = format!("kind: {kind}\nmetadata:\n  name: test\nspec: {{}}\n");
        let malformed = format!("apiVersion: [v1]\nkind: {kind}\nmetadata:\n  name: test\nspec: {{}}\n");
        let wrong_kind = "apiVersion: gateway.networking.k8s.io/v1\nkind: Other\nmetadata:\n  name: test\nspec: {}\n";
        assert!(check(&missing).is_err());
        assert!(check(&malformed).is_err());
        assert!(check(wrong_kind).is_err());
    }
}

#[test]
fn tcp_and_udp_alpha_ingress_converts_to_v1() {
    for (kind, check) in [
        (
            "TCPRoute",
            validate_resource_api_version::<TCPRoute> as fn(&str) -> serde_yaml::Result<()>,
        ),
        (
            "UDPRoute",
            validate_resource_api_version::<UDPRoute> as fn(&str) -> serde_yaml::Result<()>,
        ),
    ] {
        let alpha = format!(
                "apiVersion: gateway.networking.k8s.io/v1alpha2\nkind: {kind}\nmetadata:\n  name: route\n  namespace: default\nspec:\n  parentRefs:\n    - name: edge\n  rules:\n    - backendRefs:\n        - name: backend\n          port: 8080\n"
            );
        assert!(check(&alpha).is_ok(), "{kind} rejected alpha source");
        let canonical = alpha.replace("/v1alpha2", "/v1");
        assert!(check(&canonical).is_ok(), "{kind} rejected v1 source");
        if kind == "TCPRoute" {
            let route: TCPRoute = super::parse_resource_content(&canonical).unwrap();
            assert_eq!(
                route.spec.rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0].name,
                "backend"
            );
        } else {
            let route: UDPRoute = super::parse_resource_content(&canonical).unwrap();
            assert_eq!(
                route.spec.rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0].name,
                "backend"
            );
        }
    }
}
