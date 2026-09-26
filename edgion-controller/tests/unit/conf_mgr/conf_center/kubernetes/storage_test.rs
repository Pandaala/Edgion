use super::*;

#[tokio::test]
async fn gateway_class_crud_uses_selected_transport_and_projects_beta_source() {
    let storage = KubernetesStorage::new_unconnected_for_test();
    storage.gateway_class_version.set(GatewayClassVersion::V1Beta1).unwrap();
    let (transport, scope) = storage.resolve_api_resource("GatewayClass").await.unwrap();
    assert_eq!(scope, Scope::Cluster);
    assert_eq!(transport.api_version, "gateway.networking.k8s.io/v1beta1");
    let source = json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "GatewayClass",
        "metadata": {"name": "edge", "resourceVersion": "42"},
        "spec": {"controllerName": "example.com/edge", "description": "edge"},
        "status": {"supportedFeatures": [{"name": "Gateway"}]}
    });
    let object = storage
        .parse_dynamic_object("GatewayClass", &source.to_string(), &transport)
        .unwrap();
    let projected = serde_json::to_value(object).unwrap();
    assert_eq!(projected["apiVersion"], source["apiVersion"]);
    assert_eq!(projected["metadata"], source["metadata"]);
    assert_eq!(projected["spec"], source["spec"]);
    assert_eq!(projected["status"], source["status"]);
    let v1_transport = ApiResource::from_gvk(&GroupVersionKind::gvk(
        "gateway.networking.k8s.io",
        "v1",
        "GatewayClass",
    ));
    let canonical = storage
        .parse_dynamic_object("GatewayClass", &source.to_string(), &v1_transport)
        .unwrap();
    let canonical = serde_json::to_value(canonical).unwrap();
    assert_eq!(canonical["apiVersion"], "gateway.networking.k8s.io/v1");
    assert_eq!(canonical["metadata"], source["metadata"]);
    assert_eq!(canonical["spec"], source["spec"]);
    assert_eq!(canonical["status"], source["status"]);
}

#[tokio::test]
async fn gateway_class_beta_create_and_get_share_selected_endpoint() {
    let source = json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "GatewayClass",
        "metadata": {"name": "edge", "resourceVersion": "42"},
        "spec": {"controllerName": "example.com/edge"}
    });
    let captured = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
    let requests = captured.clone();
    let object = source.clone();
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = captured.clone();
        let object = object.clone();
        async move {
            let (parts, body) = request.into_parts();
            let bytes = body.collect_bytes().await.unwrap();
            let submitted = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap()
            };
            captured
                .lock()
                .push((parts.method.clone(), parts.uri.path().into(), submitted.clone()));
            let response = if parts.method == Method::POST {
                json_response(StatusCode::CREATED, submitted)
            } else {
                json_response(StatusCode::OK, object)
            };
            Ok::<_, Infallible>(response)
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };
    storage.gateway_class_version.set(GatewayClassVersion::V1Beta1).unwrap();
    storage
        .create_one("GatewayClass", None, "edge", source.to_string())
        .await
        .unwrap();
    let fetched: Value = serde_yaml::from_str(&storage.get_one("GatewayClass", None, "edge").await.unwrap()).unwrap();
    assert_eq!(fetched["apiVersion"], source["apiVersion"]);
    let requests = requests.lock();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].1, "/apis/gateway.networking.k8s.io/v1beta1/gatewayclasses");
    assert_eq!(requests[0].2["apiVersion"], source["apiVersion"]);
    assert_eq!(
        requests[1].1,
        "/apis/gateway.networking.k8s.io/v1beta1/gatewayclasses/edge"
    );
}
#[tokio::test]
async fn gateway_class_selection_prefers_v1_and_falls_back_only_when_absent() {
    for served_v1 in [true, false] {
        let visited = Arc::new(Mutex::new(Vec::<String>::new()));
        let requests = visited.clone();
        let service = service_fn(move |request: http::Request<Body>| {
            let visited = visited.clone();
            async move {
                let path = request.uri().path().to_string();
                visited.lock().push(path.clone());
                let version = path.rsplit('/').next().unwrap();
                let response = if version == "v1" && !served_v1 {
                    not_found_response()
                } else {
                    json_response(
                        StatusCode::OK,
                        json!({
                            "kind": "APIResourceList", "apiVersion": "v1",
                            "groupVersion": format!("gateway.networking.k8s.io/{version}"),
                            "resources": [{"name": "gatewayclasses", "singularName": "gatewayclass",
                                "namespaced": false, "kind": "GatewayClass", "verbs": ["get", "list", "watch"]}]
                        }),
                    )
                };
                Ok::<_, Infallible>(response)
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
            legacy_versions: LegacySelections::default(),
        };
        let selected = storage.selected_gateway_class_version().await.unwrap();
        assert_eq!(
            selected,
            if served_v1 {
                GatewayClassVersion::V1
            } else {
                GatewayClassVersion::V1Beta1
            }
        );
        assert_eq!(storage.selected_gateway_class_version().await.unwrap(), selected);
        assert_eq!(requests.lock().len(), if served_v1 { 1 } else { 2 });
    }
}

#[tokio::test]
async fn http_route_selection_prefers_v1_and_falls_back_only_when_absent() {
    for served_v1 in [true, false] {
        let visited = Arc::new(Mutex::new(Vec::<String>::new()));
        let requests = visited.clone();
        let service = service_fn(move |request: http::Request<Body>| {
            let visited = requests.clone();
            async move {
                let path = request.uri().path().to_string();
                visited.lock().push(path.clone());
                let version = path.rsplit('/').next().unwrap();
                let response = if version == "v1" && !served_v1 {
                    not_found_response()
                } else {
                    json_response(
                        StatusCode::OK,
                        json!({
                            "kind": "APIResourceList", "apiVersion": "v1",
                            "groupVersion": format!("gateway.networking.k8s.io/{version}"),
                            "resources": [{"name": "httproutes", "singularName": "httproute",
                                "namespaced": true, "kind": "HTTPRoute", "verbs": ["get", "list", "watch"]}]
                        }),
                    )
                };
                Ok::<_, Infallible>(response)
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
            legacy_versions: LegacySelections::default(),
        };
        let selected = storage.selected_http_route_version().await.unwrap();
        assert_eq!(
            selected,
            if served_v1 {
                HTTPRouteVersion::V1
            } else {
                HTTPRouteVersion::V1Beta1
            }
        );
        let (wire, _) = storage.resolve_api_resource("HTTPRoute").await.unwrap();
        assert_eq!(
            wire.api_version,
            if served_v1 {
                "gateway.networking.k8s.io/v1"
            } else {
                "gateway.networking.k8s.io/v1beta1"
            }
        );
        assert_eq!(visited.lock().len(), if served_v1 { 1 } else { 2 });
    }
}

#[tokio::test]
async fn gateway_crud_projects_beta_source_onto_selected_transport() {
    let storage = KubernetesStorage::new_unconnected_for_test();
    storage.gateway_version.set(GatewayVersion::V1).unwrap();
    let (transport, scope) = storage.resolve_api_resource("Gateway").await.unwrap();
    assert_eq!(scope, Scope::Namespaced);
    assert_eq!(transport.api_version, "gateway.networking.k8s.io/v1");
    let source = json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "Gateway",
        "metadata": {"name": "edge", "namespace": "demo", "resourceVersion": "42"},
        "spec": {"gatewayClassName": "edgion", "addresses": [{"value": "192.0.2.1"}],
            "listeners": [{"name": "https", "hostname": "example.com", "port": 443,
                "protocol": "HTTPS", "tls": {"certificateRefs": [{"name": "server"}]},
                "allowedRoutes": {"namespaces": {"from": "All"}}}]},
        "status": {"listeners": [{"name": "https", "supportedKinds": [],
                                 "attachedRoutes": 2, "conditions": []}]}
    });
    let projected = serde_json::to_value(
        storage
            .parse_dynamic_object("Gateway", &source.to_string(), &transport)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(projected["apiVersion"], "gateway.networking.k8s.io/v1");
    assert_eq!(projected["metadata"], source["metadata"]);
    assert_eq!(projected["spec"], source["spec"]);
    assert_eq!(projected["status"], source["status"]);
    let beta_transport = ApiResource::from_gvk(&GroupVersionKind::gvk(
        "gateway.networking.k8s.io",
        "v1beta1",
        "Gateway",
    ));
    let beta = serde_json::to_value(
        storage
            .parse_dynamic_object("Gateway", &source.to_string(), &beta_transport)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(beta, source);
}

#[tokio::test]
async fn http_route_beta_crud_projects_to_selected_wire_version() {
    let storage = KubernetesStorage::new_unconnected_for_test();
    storage.http_route_version.set(HTTPRouteVersion::V1Beta1).unwrap();
    let (transport, scope) = storage.resolve_api_resource("HTTPRoute").await.unwrap();
    assert_eq!(scope, Scope::Namespaced);
    assert_eq!(transport.api_version, "gateway.networking.k8s.io/v1beta1");
    let source = json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "HTTPRoute",
        "metadata": {"name": "route", "namespace": "demo", "resourceVersion": "42"},
        "spec": {"parentRefs": [{"name": "edge"}], "hostnames": ["example.com"],
            "futureStandardField": {"mode": "future"},
            "rules": [{"backendRefs": [{"name": "api", "port": 80}],
                "filters": [{"type": "ExternalAuth", "externalAuth": {
                    "backendRef": {"name": "auth", "port": 8080}, "protocol": "HTTP", "http": {"path": "/check"}
                }}]}]},
        "status": {"parents": [{"parentRef": {"name": "edge"}, "controllerName": "example.com/other",
            "conditions": []}]}
    });
    let projected = serde_json::to_value(
        storage
            .parse_dynamic_object("HTTPRoute", &source.to_string(), &transport)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(projected["apiVersion"], source["apiVersion"]);
    assert_eq!(projected["metadata"], source["metadata"]);
    assert_eq!(projected["spec"], source["spec"]);
    assert_eq!(projected["status"], source["status"]);

    let v1 = ApiResource::from_gvk(&GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "HTTPRoute"));
    let projected = serde_json::to_value(
        storage
            .parse_dynamic_object("HTTPRoute", &source.to_string(), &v1)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(projected["apiVersion"], "gateway.networking.k8s.io/v1");
    assert_eq!(projected["spec"], source["spec"]);
}

#[tokio::test]
async fn http_route_beta_create_and_get_use_selected_namespaced_endpoint() {
    let source = json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "HTTPRoute",
        "metadata": {"name": "route", "namespace": "demo", "resourceVersion": "42"},
        "spec": {"parentRefs": [{"name": "edge"}], "rules": [{"backendRefs": [{"name": "api", "port": 80}]}]}
    });
    let captured = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
    let requests = captured.clone();
    let object = source.clone();
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = requests.clone();
        let object = object.clone();
        async move {
            let (parts, body) = request.into_parts();
            let bytes = body.collect_bytes().await.unwrap();
            let submitted = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap()
            };
            captured
                .lock()
                .push((parts.method.clone(), parts.uri.path().into(), submitted.clone()));
            let response = if parts.method == Method::POST {
                json_response(StatusCode::CREATED, submitted)
            } else {
                json_response(StatusCode::OK, object)
            };
            Ok::<_, Infallible>(response)
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };
    storage.http_route_version.set(HTTPRouteVersion::V1Beta1).unwrap();
    storage
        .create_one("HTTPRoute", Some("demo"), "route", source.to_string())
        .await
        .unwrap();
    let fetched: Value =
        serde_yaml::from_str(&storage.get_one("HTTPRoute", Some("demo"), "route").await.unwrap()).unwrap();
    assert_eq!(fetched["apiVersion"], source["apiVersion"]);
    let requests = captured.lock();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].1,
        "/apis/gateway.networking.k8s.io/v1beta1/namespaces/demo/httproutes"
    );
    assert_eq!(requests[0].2["spec"], source["spec"]);
    assert_eq!(
        requests[1].1,
        "/apis/gateway.networking.k8s.io/v1beta1/namespaces/demo/httproutes/route"
    );
}

#[tokio::test]
async fn gateway_beta_create_and_get_use_selected_namespaced_endpoint() {
    let source = json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "Gateway",
        "metadata": {"name": "edge", "namespace": "demo", "resourceVersion": "42"},
        "spec": {"gatewayClassName": "edgion",
            "listeners": [{"name": "http", "port": 80, "protocol": "HTTP"}]}
    });
    let captured = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
    let requests = captured.clone();
    let object = source.clone();
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = requests.clone();
        let object = object.clone();
        async move {
            let (parts, body) = request.into_parts();
            let bytes = body.collect_bytes().await.unwrap();
            let submitted = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap()
            };
            captured
                .lock()
                .push((parts.method.clone(), parts.uri.path().into(), submitted.clone()));
            let response = if parts.method == Method::POST {
                json_response(StatusCode::CREATED, submitted)
            } else {
                json_response(StatusCode::OK, object)
            };
            Ok::<_, Infallible>(response)
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };
    storage.gateway_version.set(GatewayVersion::V1Beta1).unwrap();
    storage
        .create_one("Gateway", Some("demo"), "edge", source.to_string())
        .await
        .unwrap();
    let fetched: Value =
        serde_yaml::from_str(&storage.get_one("Gateway", Some("demo"), "edge").await.unwrap()).unwrap();
    assert_eq!(fetched["apiVersion"], source["apiVersion"]);
    let requests = captured.lock();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].1,
        "/apis/gateway.networking.k8s.io/v1beta1/namespaces/demo/gateways"
    );
    assert_eq!(requests[0].2["spec"], source["spec"]);
    assert_eq!(
        requests[1].1,
        "/apis/gateway.networking.k8s.io/v1beta1/namespaces/demo/gateways/edge"
    );
}

#[tokio::test]
async fn gateway_selection_prefers_v1_and_falls_back_only_on_absence() {
    for served_v1 in [true, false] {
        let visited = Arc::new(Mutex::new(Vec::<String>::new()));
        let requests = visited.clone();
        let service = service_fn(move |request: http::Request<Body>| {
            let visited = visited.clone();
            async move {
                let path = request.uri().path().to_string();
                visited.lock().push(path.clone());
                let version = path.rsplit('/').next().unwrap();
                let response = if version == "v1" && !served_v1 {
                    not_found_response()
                } else {
                    json_response(
                        StatusCode::OK,
                        json!({
                            "kind": "APIResourceList", "apiVersion": "v1",
                            "groupVersion": format!("gateway.networking.k8s.io/{version}"),
                            "resources": [{"name": "gateways", "singularName": "gateway",
                                "namespaced": true, "kind": "Gateway", "verbs": ["get", "list", "watch"]}]
                        }),
                    )
                };
                Ok::<_, Infallible>(response)
            }
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
            legacy_versions: LegacySelections::default(),
        };
        let selected = storage.selected_gateway_version().await.unwrap();
        assert_eq!(
            selected,
            if served_v1 {
                GatewayVersion::V1
            } else {
                GatewayVersion::V1Beta1
            }
        );
        assert_eq!(storage.selected_gateway_version().await.unwrap(), selected);
        assert_eq!(requests.lock().len(), if served_v1 { 1 } else { 2 });
    }
}

use http::{Method, Response, StatusCode};
use kube::api::ObjectMeta;
use kube::client::Body;
use kube::core::PartialObjectMeta;
use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;
use tower::service_fn;

#[tokio::test]
async fn canonical_crud_resolver_matches_the_typed_source_inventory() {
    use edgion_resources::gateway_api_support::{canonical_sources, GROUP};
    let service = service_fn(|_req: http::Request<Body>| async {
        panic!("canonical CRUD resolution must not perform discovery");
        #[allow(unreachable_code)]
        Ok::<_, Infallible>(Response::new(Body::empty()))
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };
    storage.gateway_class_version.set(GatewayClassVersion::V1).unwrap();
    storage.gateway_version.set(GatewayVersion::V1).unwrap();
    storage.http_route_version.set(HTTPRouteVersion::V1).unwrap();
    for (kind, _) in super::super::version_detection::LEGACY_SOURCE_KINDS {
        storage
            .legacy_versions
            .cell(kind)
            .unwrap()
            .set(Some(LegacyRouteVersion::V1))
            .unwrap();
    }
    for source in canonical_sources().filter(|source| source.kind != "ReferenceGrant") {
        let (resolved, scope) = storage.resolve_api_resource(source.kind).await.unwrap();
        assert_eq!(resolved.api_version, format!("{GROUP}/{}", source.version));
        assert_eq!(resolved.kind, source.kind);
        assert_eq!(
            scope,
            if source.kind == "GatewayClass" {
                Scope::Cluster
            } else {
                Scope::Namespaced
            }
        );
    }
}

#[tokio::test]
async fn reference_grant_crud_converts_beta_source_to_selected_transport() {
    let storage = KubernetesStorage::new_unconnected_for_test();
    let mut source = canonical_reference_grant_value();
    source["apiVersion"] = json!("gateway.networking.k8s.io/v1beta1");
    let transport = ApiResource::from_gvk(&GroupVersionKind::gvk(
        "gateway.networking.k8s.io",
        "v1",
        "ReferenceGrant",
    ));
    let object = storage
        .parse_dynamic_object("ReferenceGrant", &source.to_string(), &transport)
        .unwrap();
    let projected = serde_json::to_value(object).unwrap();
    assert_eq!(projected["apiVersion"], "gateway.networking.k8s.io/v1");
    assert_eq!(projected["spec"], source["spec"]);
    let beta_transport = ApiResource::from_gvk(&GroupVersionKind::gvk(
        "gateway.networking.k8s.io",
        "v1beta1",
        "ReferenceGrant",
    ));
    let beta_object = storage
        .parse_dynamic_object("ReferenceGrant", &source.to_string(), &beta_transport)
        .unwrap();
    assert_eq!(
        serde_json::to_value(beta_object).unwrap()["apiVersion"],
        "gateway.networking.k8s.io/v1beta1"
    );
    for invalid in [
        {
            let mut value = source.clone();
            value["kind"] = json!("ReferenceGrantBeta");
            value
        },
        {
            let mut value = source.clone();
            value["spec"].as_object_mut().unwrap().remove("from");
            value
        },
    ] {
        assert!(storage
            .parse_dynamic_object("ReferenceGrant", &invalid.to_string(), &transport)
            .is_err());
    }
}

#[tokio::test]
async fn reference_grant_crud_resolver_follows_real_discovery() {
    for served in ["v1", "v1beta1"] {
        let service = service_fn(move |request: http::Request<Body>| async move {
            let path = request.uri().path();
            let response = if path == format!("/apis/gateway.networking.k8s.io/{served}") {
                json_response(
                    StatusCode::OK,
                    json!({
                        "kind": "APIResourceList", "apiVersion": "v1",
                        "groupVersion": format!("gateway.networking.k8s.io/{served}"),
                        "resources": [{"name":"referencegrants", "singularName":"referencegrant",
                            "namespaced":true, "kind":"ReferenceGrant", "verbs":["get","list","watch"]}]
                    }),
                )
            } else {
                not_found_response()
            };
            Ok::<_, Infallible>(response)
        });
        let storage = KubernetesStorage {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
            legacy_versions: LegacySelections::default(),
        };
        let (resolved, scope) = storage.resolve_api_resource("ReferenceGrant").await.unwrap();
        assert_eq!(resolved.api_version, format!("gateway.networking.k8s.io/{served}"));
        assert_eq!(resolved.kind, "ReferenceGrant");
        assert_eq!(scope, Scope::Namespaced);
    }
}

#[tokio::test]
async fn legacy_route_crud_selects_alpha_only_and_prefers_v1_when_available() {
    for (kind, alpha, plural) in [
        ("GRPCRoute", "v1alpha2", "grpcroutes"),
        ("TLSRoute", "v1alpha3", "tlsroutes"),
        ("BackendTLSPolicy", "v1alpha3", "backendtlspolicies"),
        ("TCPRoute", "v1alpha2", "tcproutes"),
        ("UDPRoute", "v1alpha2", "udproutes"),
    ] {
        for (serve_v1, serve_alpha) in [(false, false), (false, true), (true, false), (true, true)] {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let observed = requests.clone();
            let service = service_fn(move |request: http::Request<Body>| {
                let observed = observed.clone();
                async move {
                    let path = request.uri().path().to_string();
                    observed.lock().push(path.clone());
                    let version = path.rsplit('/').next().unwrap_or_default();
                    let served = (serve_alpha && version == alpha) || (serve_v1 && version == "v1");
                    let response = if served {
                        json_response(
                            StatusCode::OK,
                            json!({
                                "kind": "APIResourceList", "apiVersion": "v1",
                                "groupVersion": format!("gateway.networking.k8s.io/{version}"),
                                "resources": [{"name": plural, "singularName": "",
                                    "namespaced": true, "kind": kind, "verbs": ["get", "list", "watch"]}]
                            }),
                        )
                    } else {
                        not_found_response()
                    };
                    Ok::<_, Infallible>(response)
                }
            });
            let storage = KubernetesStorage {
                client: Client::new(service, "default"),
                allowed_namespaces: BTreeSet::new(),
                reference_grant_version: Arc::new(OnceCell::new()),
                gateway_class_version: Arc::new(OnceCell::new()),
                gateway_version: Arc::new(OnceCell::new()),
                http_route_version: Arc::new(OnceCell::new()),
                legacy_versions: LegacySelections::default(),
            };
            let selected = storage.selected_legacy_version(kind).await.unwrap();
            if serve_v1 || serve_alpha {
                let (resource, scope) = storage.resolve_api_resource(kind).await.unwrap();
                assert_eq!(scope, Scope::Namespaced);
                assert_eq!(
                    resource.api_version,
                    format!("gateway.networking.k8s.io/{}", if serve_v1 { "v1" } else { alpha })
                );
                assert_eq!(selected.unwrap().as_str(), if serve_v1 { "v1" } else { alpha });
            } else {
                assert!(selected.is_none());
                assert!(storage.resolve_api_resource(kind).await.is_err());
            }
            assert_eq!(requests.lock().len(), if serve_v1 { 1 } else { 2 });
        }
    }
}

#[tokio::test]
async fn reference_grant_prefers_v1_when_both_versions_are_served() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let probes = Arc::new(AtomicUsize::new(0));
    let count = probes.clone();
    let service = service_fn(move |_request: http::Request<Body>| {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            Ok::<_, Infallible>(reference_grant_discovery_response())
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };
    let (resource, _) = storage.resolve_api_resource("ReferenceGrant").await.unwrap();
    assert_eq!(resource.api_version, "gateway.networking.k8s.io/v1");
    let _ = storage.resolve_api_resource("ReferenceGrant").await.unwrap();
    assert_eq!(probes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reference_grant_selection_is_shared_and_pinned_after_discovery() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let v1_available = Arc::new(AtomicBool::new(false));
    let probes = Arc::new(AtomicUsize::new(0));
    let available = v1_available.clone();
    let count = probes.clone();
    let service = service_fn(move |request: http::Request<Body>| {
        let available = available.clone();
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            let path = request.uri().path();
            let served = path == "/apis/gateway.networking.k8s.io/v1beta1"
                || (path == "/apis/gateway.networking.k8s.io/v1" && available.load(Ordering::SeqCst));
            Ok::<_, Infallible>(if served {
                reference_grant_discovery_response()
            } else {
                not_found_response()
            })
        }
    });
    let client = Client::new(service, "default");
    let selected = Arc::new(OnceCell::new());
    let make_storage = || KubernetesStorage {
        client: client.clone(),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: selected.clone(),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };
    let main = make_storage();
    let runtime = make_storage();
    assert_eq!(
        main.selected_reference_grant_version().await.unwrap(),
        ReferenceGrantVersion::V1Beta1
    );
    assert_eq!(probes.load(Ordering::SeqCst), 2);
    v1_available.store(true, Ordering::SeqCst);
    for storage in [&main, &runtime] {
        let (resource, _) = storage.resolve_api_resource("ReferenceGrant").await.unwrap();
        assert_eq!(resource.api_version, "gateway.networking.k8s.io/v1beta1");
    }
    assert_eq!(probes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn shared_reference_grant_selection_pins_main_and_runtime_crud_paths() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let v1_available = Arc::new(AtomicBool::new(false));
    let requests = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
    let available = v1_available.clone();
    let captured = requests.clone();
    let service = service_fn(move |request: http::Request<Body>| {
        let available = available.clone();
        let captured = captured.clone();
        async move {
            let (parts, body) = request.into_parts();
            let bytes = body.collect_bytes().await.expect("request body must be readable");
            let value = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).unwrap()
            };
            let path = parts.uri.path().to_string();
            captured
                .lock()
                .push((parts.method.clone(), path.clone(), value.clone()));
            let current = || {
                let mut resource = canonical_reference_grant_value();
                resource["apiVersion"] = json!("gateway.networking.k8s.io/v1beta1");
                resource["metadata"]["resourceVersion"] = json!("42");
                resource
            };
            let response = if path == "/apis/gateway.networking.k8s.io/v1" {
                if available.load(Ordering::SeqCst) {
                    reference_grant_discovery_response()
                } else {
                    not_found_response()
                }
            } else if path == "/apis/gateway.networking.k8s.io/v1beta1" {
                reference_grant_discovery_response()
            } else if parts.method == Method::GET && path.ends_with("/referencegrants") {
                json_response(
                    StatusCode::OK,
                    json!({
                        "apiVersion": "gateway.networking.k8s.io/v1beta1",
                        "kind": "ReferenceGrantList",
                        "metadata": {"resourceVersion": "42"},
                        "items": [current()]
                    }),
                )
            } else if parts.method == Method::GET || parts.method == Method::DELETE {
                json_response(StatusCode::OK, current())
            } else {
                json_response(
                    if parts.method == Method::POST {
                        StatusCode::CREATED
                    } else {
                        StatusCode::OK
                    },
                    value,
                )
            };
            Ok::<_, Infallible>(response)
        }
    });
    let client = Client::new(service, "default");
    let selection = Arc::new(OnceCell::new());
    let storage = || KubernetesStorage {
        client: client.clone(),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: selection.clone(),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };
    let main = storage();
    let runtime = storage();
    let mut beta_source = canonical_reference_grant_value();
    beta_source["apiVersion"] = json!("gateway.networking.k8s.io/v1beta1");
    main.create_one(
        "ReferenceGrant",
        Some("backend"),
        "allow-route",
        beta_source.to_string(),
    )
    .await
    .unwrap();
    assert_eq!(
        main.selected_reference_grant_version().await.unwrap(),
        ReferenceGrantVersion::V1Beta1
    );
    v1_available.store(true, Ordering::SeqCst);
    runtime
        .update_one_if_version(
            "ReferenceGrant",
            Some("backend"),
            "allow-route",
            canonical_reference_grant_value().to_string(),
            Some("42"),
        )
        .await
        .unwrap();
    let fetched = main
        .get_one("ReferenceGrant", Some("backend"), "allow-route")
        .await
        .unwrap();
    assert!(fetched.contains("gateway.networking.k8s.io/v1beta1"));
    let listed = runtime
        .get_list_by_kind_ns("ReferenceGrant", "backend", None)
        .await
        .unwrap();
    assert_eq!(listed.items.len(), 1);
    assert!(listed.items[0].content.contains("gateway.networking.k8s.io/v1beta1"));
    main.delete_one_if_version("ReferenceGrant", Some("backend"), "allow-route", Some("42"))
        .await
        .unwrap();
    let requests = requests.lock();
    assert_eq!(
        requests
            .iter()
            .filter(|(_, path, _)| path == "/apis/gateway.networking.k8s.io/v1")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|(_, path, _)| path == "/apis/gateway.networking.k8s.io/v1beta1")
            .count(),
        1
    );
    let operations: Vec<_> = requests
        .iter()
        .filter(|(_, path, _)| path.contains("/referencegrants"))
        .collect();
    assert_eq!(
        operations
            .iter()
            .map(|(method, _, _)| method)
            .filter(|method| **method == Method::POST)
            .count(),
        1
    );
    assert_eq!(
        operations
            .iter()
            .map(|(method, _, _)| method)
            .filter(|method| **method == Method::PUT)
            .count(),
        1
    );
    assert_eq!(
        operations
            .iter()
            .map(|(method, _, _)| method)
            .filter(|method| **method == Method::DELETE)
            .count(),
        1
    );
    assert!(operations.iter().all(
        |(_, path, _)| path.starts_with("/apis/gateway.networking.k8s.io/v1beta1/") && !path.ends_with("/status")
    ));
    for (method, _, value) in operations {
        if method == Method::POST || method == Method::PUT {
            assert_eq!(value["apiVersion"], "gateway.networking.k8s.io/v1beta1");
            assert_eq!(value["kind"], "ReferenceGrant");
        }
    }
}

#[tokio::test]
async fn transient_reference_grant_discovery_failure_retries_before_selection() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    let service = service_fn(move |_request: http::Request<Body>| {
        let count = count.clone();
        async move {
            let attempt = count.fetch_add(1, Ordering::SeqCst);
            Ok::<_, Infallible>(if attempt == 0 {
                json_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    json!({
                        "kind": "Status", "apiVersion": "v1", "status": "Failure",
                        "reason": "ServiceUnavailable", "message": "temporarily unavailable", "code": 503
                    }),
                )
            } else {
                reference_grant_discovery_response()
            })
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };
    assert!(storage.selected_reference_grant_version().await.is_err());
    assert_eq!(
        storage.selected_reference_grant_version().await.unwrap(),
        ReferenceGrantVersion::V1
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn tls_route_storage_uses_selected_v1_and_rejects_other_versions() {
    let service = service_fn(|_req: http::Request<Body>| async {
        panic!("TLSRoute endpoint selection and raw parsing must not perform discovery");
        #[allow(unreachable_code)]
        Ok::<_, Infallible>(Response::new(Body::empty()))
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };
    storage
        .legacy_versions
        .cell("TLSRoute")
        .unwrap()
        .set(Some(LegacyRouteVersion::V1))
        .unwrap();
    let (api_resource, _) = storage.resolve_api_resource("TLSRoute").await.unwrap();
    assert_eq!(api_resource.api_version, "gateway.networking.k8s.io/v1");
    assert_eq!(api_resource.kind, "TLSRoute");
    for version in [
        None,
        Some(Value::Null),
        Some(json!(42)),
        Some(json!("")),
        Some(json!("gateway.networking.k8s.io/v1alpha2")),
        Some(json!("gateway.networking.k8s.io/v1alpha3")),
        Some(json!("gateway.networking.k8s.io/v1")),
    ] {
        let valid = matches!(
            version.as_ref().and_then(Value::as_str),
            Some("gateway.networking.k8s.io/v1" | "gateway.networking.k8s.io/v1alpha3")
        );
        let mut document = json!({"kind": "TLSRoute", "metadata": {"name": "route", "namespace": "default"}, "spec": {"hostnames": ["example.com"]}});
        if let Some(value) = version {
            document["apiVersion"] = value;
        }
        let parsed = storage.parse_dynamic_object("TLSRoute", &document.to_string(), &api_resource);
        if valid {
            assert_eq!(
                parsed.unwrap().types.unwrap().api_version,
                "gateway.networking.k8s.io/v1"
            );
        } else {
            assert!(matches!(parsed, Err(ConfWriterError::ValidationError(_))));
        }
    }
}

#[tokio::test]
async fn tls_route_create_projects_alpha3_to_v1() {
    let requests = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
    let captured = requests.clone();
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = captured.clone();
        async move {
            let (parts, body) = request.into_parts();
            let bytes = body.collect_bytes().await.unwrap();
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            captured
                .lock()
                .push((parts.method, parts.uri.path().to_string(), value.clone()));
            Ok::<_, Infallible>(json_response(StatusCode::CREATED, value))
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };
    storage
        .legacy_versions
        .cell("TLSRoute")
        .unwrap()
        .set(Some(LegacyRouteVersion::V1))
        .unwrap();
    let value = json!({
        "apiVersion": "gateway.networking.k8s.io/v1alpha3", "kind": "TLSRoute",
        "metadata": {"name": "route", "namespace": "default"},
        "spec": {"hostnames": ["example.com"]}
    });
    storage
        .create_one("TLSRoute", Some("default"), "route", value.to_string())
        .await
        .unwrap();
    let requests = requests.lock();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, Method::POST);
    assert_eq!(
        requests[0].1,
        "/apis/gateway.networking.k8s.io/v1/namespaces/default/tlsroutes"
    );
    assert_eq!(requests[0].2["apiVersion"], "gateway.networking.k8s.io/v1");
}

fn json_response(status: StatusCode, value: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&value).expect("response JSON must serialize"),
        ))
        .expect("response must build")
}

fn not_found_response() -> Response<Body> {
    json_response(
        StatusCode::NOT_FOUND,
        json!({
            "apiVersion": "v1",
            "kind": "Status",
            "status": "Failure",
            "message": "the server could not find the requested resource",
            "reason": "NotFound",
            "code": 404
        }),
    )
}

fn conflict_response(message: &str) -> Response<Body> {
    json_response(
        StatusCode::CONFLICT,
        json!({
            "apiVersion": "v1",
            "kind": "Status",
            "status": "Failure",
            "message": message,
            "reason": "Conflict",
            "code": 409
        }),
    )
}

fn config_map_value(resource_version: Option<&str>, status_state: Option<&str>, data_value: &str) -> Value {
    let mut value = json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": {
            "name": "hot-config",
            "namespace": "default",
            "uid": "uid-1"
        },
        "data": {"value": data_value}
    });
    if let Some(resource_version) = resource_version {
        value["metadata"]["resourceVersion"] = json!(resource_version);
    }
    if let Some(status_state) = status_state {
        value["status"] = json!({"state": status_state});
    }
    value
}

fn reference_grant_discovery_response() -> Response<Body> {
    json_response(
        StatusCode::OK,
        json!({
            "apiVersion": "v1",
            "groupVersion": "gateway.networking.k8s.io/v1beta1",
            "kind": "APIResourceList",
            "resources": [{
                "name": "referencegrants",
                "singularName": "referencegrant",
                "namespaced": true,
                "kind": "ReferenceGrant",
                "verbs": ["create", "delete", "get", "list", "patch", "update", "watch"]
            }]
        }),
    )
}

#[test]
fn project_partial_meta_strips_last_applied_and_preserves_others() {
    let mut annotations = BTreeMap::new();
    annotations.insert(
        "kubectl.kubernetes.io/last-applied-configuration".into(),
        "{\"big\":\"yaml\"}".repeat(200),
    );
    annotations.insert("edgion.io/enable-http".into(), "true".into());
    annotations.insert("kubernetes.io/ingress.class".into(), "edgion".into());

    let part: PartialObjectMeta<kube::api::DynamicObject> = PartialObjectMeta {
        types: Some(kube::core::TypeMeta {
            api_version: "gateway.networking.k8s.io/v1".into(),
            kind: "HTTPRoute".into(),
        }),
        metadata: ObjectMeta {
            name: Some("my-route".into()),
            namespace: Some("default".into()),
            resource_version: Some("42".into()),
            annotations: Some(annotations),
            labels: Some({
                let mut l = BTreeMap::new();
                l.insert("app".into(), "demo".into());
                l
            }),
            ..Default::default()
        },
        _phantom: std::marker::PhantomData,
    };

    let key = project_partial_meta_to_key("HTTPRoute", part);
    assert_eq!(key.kind, "HTTPRoute");
    assert_eq!(key.metadata.name, "my-route");
    assert_eq!(key.metadata.namespace.as_deref(), Some("default"));
    assert_eq!(key.metadata.resource_version.as_deref(), Some("42"));
    assert!(!key
        .metadata
        .annotations
        .contains_key("kubectl.kubernetes.io/last-applied-configuration"));
    assert_eq!(
        key.metadata
            .annotations
            .get("edgion.io/enable-http")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        key.metadata
            .annotations
            .get("kubernetes.io/ingress.class")
            .map(String::as_str),
        Some("edgion")
    );
    assert_eq!(key.metadata.labels.get("app").map(String::as_str), Some("demo"));
}

#[test]
fn map_kube_error_410_to_stale_pagination() {
    let ae = kube::core::ErrorResponse {
        status: "Failure".into(),
        message: "The provided continue parameter is too old".into(),
        reason: "Expired".into(),
        code: 410,
    };
    let err = kube::Error::Api(ae);
    let mapped = map_kube_error(err);
    assert!(
        matches!(mapped, ConfWriterError::StalePagination(_)),
        "expected StalePagination, got: {:?}",
        mapped
    );
    if let ConfWriterError::StalePagination(msg) = mapped {
        assert!(
            msg.contains("continue parameter is too old"),
            "unexpected message: {msg}"
        );
    }
}

#[tokio::test]
async fn delete_sends_kubernetes_resource_version_precondition() {
    let requests = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
    let captured = Arc::clone(&requests);
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = Arc::clone(&captured);
        async move {
            let (parts, body) = request.into_parts();
            let bytes = body.collect_bytes().await.expect("request body must be readable");
            let value = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).expect("request body must be JSON")
            };
            captured
                .lock()
                .push((parts.method.clone(), parts.uri.to_string(), value));
            let response = if parts.uri.path() == "/apis/gateway.networking.k8s.io/v1" {
                not_found_response()
            } else if parts.method == Method::GET {
                reference_grant_discovery_response()
            } else {
                json_response(
                    StatusCode::OK,
                    json!({
                        "apiVersion": "v1",
                        "kind": "Status",
                        "status": "Success",
                        "code": 200
                    }),
                )
            };
            Ok::<_, Infallible>(response)
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };

    storage
        .delete_one_if_version("ReferenceGrant", Some("backend"), "allow-route", Some("42"))
        .await
        .expect("conditional Kubernetes delete must succeed");

    let requests = requests.lock();
    let delete = requests
        .iter()
        .find(|(method, _, _)| method == Method::DELETE)
        .expect("delete request must be captured");
    assert_eq!(delete.2["preconditions"]["resourceVersion"], "42");
}

#[tokio::test]
async fn configmap_key_listing_is_limited_to_watched_namespaces_and_paginates_locally() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&paths);
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = Arc::clone(&captured);
        async move {
            captured.lock().push(request.uri().path().to_string());
            Ok::<_, Infallible>(json_response(
                StatusCode::OK,
                json!({
                    "apiVersion": "meta.k8s.io/v1",
                    "kind": "PartialObjectMetadataList",
                    "metadata": {"resourceVersion": "7"},
                    "items": [
                        {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "a-2", "namespace": "allowed"}},
                        {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "a-1", "namespace": "allowed"}}
                    ]
                }),
            ))
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::from(["allowed".to_string()]),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };

    let first = storage
        .list_keys_by_kind(
            "ConfigMap",
            Some(ListOptions {
                limit: 1,
                continue_token: None,
            }),
        )
        .await
        .expect("first page must succeed");
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.items[0].metadata.name, "a-1");
    let token = first.continue_token.expect("first page must continue");

    let second = storage
        .list_keys_by_kind(
            "ConfigMap",
            Some(ListOptions {
                limit: 1,
                continue_token: Some(token),
            }),
        )
        .await
        .expect("second page must succeed");
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].metadata.name, "a-2");
    assert!(second.continue_token.is_none());
    let paths = paths.lock();
    assert_eq!(
        paths.as_slice(),
        [
            "/api/v1/namespaces/allowed/configmaps",
            "/api/v1/namespaces/allowed/configmaps"
        ]
    );
}

#[test]
fn namespace_pagination_rejects_foreign_tokens() {
    let error = paginate_namespaced(
        vec![1],
        Some(ListOptions {
            limit: 1,
            continue_token: Some("kubernetes-token".to_string()),
        }),
        "ConfigMap",
        &BTreeSet::from(["allowed".to_string()]),
        Some("7"),
    )
    .expect_err("foreign tokens must be rejected");
    assert!(matches!(error, ConfWriterError::StalePagination(_)));
}

#[test]
fn namespace_pagination_token_is_bound_to_query_and_snapshot() {
    let namespaces = BTreeSet::from(["allowed".to_string()]);
    let first = paginate_namespaced(
        vec![1, 2],
        Some(ListOptions {
            limit: 1,
            continue_token: None,
        }),
        "ConfigMap",
        &namespaces,
        Some("7"),
    )
    .expect("first page must succeed");
    let token = first.continue_token.expect("first page must continue");

    for (kind, resource_version) in [("Secret", "7"), ("ConfigMap", "8")] {
        let error = paginate_namespaced(
            vec![1, 2],
            Some(ListOptions {
                limit: 1,
                continue_token: Some(token.clone()),
            }),
            kind,
            &namespaces,
            Some(resource_version),
        )
        .expect_err("token reuse across a query or snapshot must fail");
        assert!(matches!(error, ConfWriterError::StalePagination(_)));
    }
}

fn canonical_reference_grant_value() -> Value {
    json!({
        "apiVersion": "gateway.networking.k8s.io/v1",
        "kind": "ReferenceGrant",
        "metadata": {
            "name": "allow-route",
            "namespace": "backend"
        },
        "spec": {
            "from": [{
                "group": "gateway.networking.k8s.io",
                "kind": "HTTPRoute",
                "namespace": "frontend"
            }],
            "to": [{"group": "", "kind": "Service"}]
        }
    })
}

#[test]
fn backend_tls_policy_api_version_accepts_registered_sources() {
    let canonical = json!({"apiVersion": "gateway.networking.k8s.io/v1"});
    assert!(validate_backend_tls_policy_api_version(canonical.get("apiVersion").and_then(Value::as_str)).is_ok());
    assert!(validate_backend_tls_policy_api_version(Some("gateway.networking.k8s.io/v1alpha3")).is_ok());

    for value in [json!({"apiVersion": "gateway.networking.k8s.io/v1alpha2"}), json!({})] {
        let error =
            validate_backend_tls_policy_api_version(value.get("apiVersion").and_then(Value::as_str)).unwrap_err();
        assert!(error.contains("BackendTLSPolicy apiVersion must use a supported"));
    }
}

#[tokio::test]
async fn last_pre_v1_sources_project_to_the_v1_crud_endpoint() {
    let storage = KubernetesStorage::new_unconnected_for_test();
    for (kind, version) in [
        ("GRPCRoute", "v1alpha2"),
        ("TLSRoute", "v1alpha3"),
        ("BackendTLSPolicy", "v1alpha3"),
        ("TCPRoute", "v1alpha2"),
        ("UDPRoute", "v1alpha2"),
    ] {
        let (api_resource, _) = storage.resolve_api_resource(kind).await.unwrap();
        let source = json!({
            "apiVersion": format!("gateway.networking.k8s.io/{version}"),
            "kind": kind,
            "metadata": {"name": "resource", "namespace": "default"},
            "spec": {}
        });
        let object = storage
            .parse_dynamic_object(kind, &source.to_string(), &api_resource)
            .unwrap();
        assert_eq!(
            object.types.unwrap().api_version,
            "gateway.networking.k8s.io/v1",
            "{kind}"
        );
        assert_eq!(object.data["spec"], source["spec"], "{kind}");
    }
}

#[test]
fn create_projects_canonical_reference_grant_to_detected_v1beta1_gvk() {
    let mut value = canonical_reference_grant_value();
    project_gateway_api_version(&mut value, "ReferenceGrant", "gateway.networking.k8s.io/v1beta1");

    let object: DynamicObject = serde_json::from_value(value).expect("projected object must deserialize");
    let types = object.types.expect("projected object must retain type metadata");
    assert_eq!(types.api_version, "gateway.networking.k8s.io/v1beta1");
    assert_eq!(types.kind, "ReferenceGrant");
    assert_eq!(object.data["spec"]["from"][0]["namespace"], "frontend");
}

#[test]
fn update_projection_keeps_v1beta1_and_preserves_server_fields() {
    let mut requested = canonical_reference_grant_value();
    requested["metadata"]["labels"] = json!({"edited": "true"});
    requested["spec"]["to"] = json!([{"group": "", "kind": "Secret"}]);
    project_gateway_api_version(&mut requested, "ReferenceGrant", "gateway.networking.k8s.io/v1beta1");

    let current: DynamicObject = serde_json::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1beta1",
        "kind": "ReferenceGrant",
        "metadata": {
            "name": "allow-route",
            "namespace": "backend",
            "uid": "uid-1",
            "resourceVersion": "42",
            "generation": 3,
            "ownerReferences": [{
                "apiVersion": "v1",
                "kind": "ConfigMap",
                "name": "owner",
                "uid": "owner-uid"
            }],
            "finalizers": ["edgion.io/finalizer"]
        },
        "spec": {
            "from": [{
                "group": "gateway.networking.k8s.io",
                "kind": "HTTPRoute",
                "namespace": "old"
            }],
            "to": [{"group": "", "kind": "Service"}],
            "runtimeOnly": "must-not-return"
        },
        "status": {"conditions": [{"type": "Accepted", "status": "True"}]}
    }))
    .expect("current object must deserialize");
    let requested: DynamicObject = serde_json::from_value(requested).expect("projected request must deserialize");

    let merged = merge_dynamic_operator_update(current, requested).expect("update merge must succeed");
    let merged = serde_json::to_value(merged).expect("merged object must serialize");
    assert_eq!(merged["apiVersion"], "gateway.networking.k8s.io/v1beta1");
    assert_eq!(merged["metadata"]["resourceVersion"], "42");
    assert_eq!(merged["metadata"]["uid"], "uid-1");
    assert_eq!(merged["metadata"]["ownerReferences"][0]["name"], "owner");
    assert_eq!(merged["metadata"]["finalizers"][0], "edgion.io/finalizer");
    assert_eq!(merged["metadata"]["labels"]["edited"], "true");
    assert_eq!(merged["spec"]["to"][0]["kind"], "Secret");
    assert!(merged["spec"].get("runtimeOnly").is_none());
    assert_eq!(merged["status"]["conditions"][0]["type"], "Accepted");
}

#[test]
fn stale_expected_resource_version_stops_kubernetes_replace() {
    let current = json!({"metadata": {"resourceVersion": "42"}});
    let error = ensure_expected_resource_version_value(&current, Some("41"))
        .expect_err("stale Kubernetes update must conflict before replace");
    assert!(matches!(error, ConfWriterError::Conflict(_)));
}

#[tokio::test]
async fn unconditional_update_surfaces_replace_conflict_without_server_retry() {
    let requests = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
    let captured = Arc::clone(&requests);
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = Arc::clone(&captured);
        async move {
            let (parts, body) = request.into_parts();
            let bytes = body.collect_bytes().await.expect("request body must be readable");
            let value = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).expect("request body must be JSON")
            };
            let prior_puts = captured
                .lock()
                .iter()
                .filter(|(method, _, _)| *method == Method::PUT)
                .count();
            captured
                .lock()
                .push((parts.method.clone(), parts.uri.to_string(), value.clone()));

            let response = match parts.method {
                Method::GET if prior_puts == 0 => {
                    json_response(StatusCode::OK, config_map_value(Some("42"), Some("old"), "old"))
                }
                Method::GET => json_response(StatusCode::OK, config_map_value(Some("43"), Some("latest"), "old")),
                Method::PUT if prior_puts == 0 => conflict_response("object was modified by status update"),
                Method::PUT => json_response(StatusCode::OK, value),
                _ => panic!("unexpected request: {} {}", parts.method, parts.uri),
            };
            Ok::<_, Infallible>(response)
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };

    let error = storage
        .update_one_if_version(
            "ConfigMap",
            Some("default"),
            "hot-config",
            serde_yaml::to_string(&config_map_value(None, None, "requested")).expect("fixture must serialize"),
            None,
        )
        .await
        .expect_err("the Admin API must surface the backend conflict to its caller");

    assert!(matches!(error, ConfWriterError::Conflict(_)));

    let requests = requests.lock();
    let methods = requests.iter().map(|(method, _, _)| method.clone()).collect::<Vec<_>>();
    assert_eq!(methods, vec![Method::GET, Method::PUT]);
    let puts = requests
        .iter()
        .filter(|(method, _, _)| *method == Method::PUT)
        .map(|(_, _, body)| body)
        .collect::<Vec<_>>();
    assert_eq!(puts[0]["metadata"]["resourceVersion"], "42");
    assert_eq!(puts[0]["status"]["state"], "old");
    assert_eq!(puts[0]["data"]["value"], "requested");
}

#[tokio::test]
async fn conditional_update_does_not_retry_replace_conflict() {
    let requests = Arc::new(Mutex::new(Vec::<Method>::new()));
    let captured = Arc::clone(&requests);
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = Arc::clone(&captured);
        async move {
            let method = request.method().clone();
            captured.lock().push(method.clone());
            let response = if method == Method::GET {
                json_response(StatusCode::OK, config_map_value(Some("42"), Some("old"), "old"))
            } else if method == Method::PUT {
                conflict_response("object was modified by status update")
            } else {
                panic!("unexpected request method: {method}")
            };
            Ok::<_, Infallible>(response)
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };

    let error = storage
        .update_one_if_version(
            "ConfigMap",
            Some("default"),
            "hot-config",
            serde_yaml::to_string(&config_map_value(None, None, "requested")).expect("fixture must serialize"),
            Some("42"),
        )
        .await
        .expect_err("explicit CAS must surface a replace conflict");

    assert!(matches!(error, ConfWriterError::Conflict(_)));
    assert_eq!(*requests.lock(), vec![Method::GET, Method::PUT]);
}

#[tokio::test]
async fn unconditional_update_does_not_retry_non_conflict_replace_error() {
    let requests = Arc::new(Mutex::new(Vec::<Method>::new()));
    let captured = Arc::clone(&requests);
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = Arc::clone(&captured);
        async move {
            let method = request.method().clone();
            captured.lock().push(method.clone());
            let response = if method == Method::GET {
                json_response(StatusCode::OK, config_map_value(Some("42"), Some("old"), "old"))
            } else if method == Method::PUT {
                json_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    json!({
                        "apiVersion": "v1",
                        "kind": "Status",
                        "status": "Failure",
                        "message": "invalid update",
                        "reason": "Invalid",
                        "code": 422
                    }),
                )
            } else {
                panic!("unexpected request method: {method}")
            };
            Ok::<_, Infallible>(response)
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };

    storage
        .update_one_if_version(
            "ConfigMap",
            Some("default"),
            "hot-config",
            serde_yaml::to_string(&config_map_value(None, None, "requested")).expect("fixture must serialize"),
            None,
        )
        .await
        .expect_err("a non-conflict replace error must be returned without retry");

    assert_eq!(*requests.lock(), vec![Method::GET, Method::PUT]);
}

#[tokio::test]
async fn unconditional_update_does_not_retry_a_persistent_conflict() {
    let requests = Arc::new(Mutex::new(Vec::<Method>::new()));
    let captured = Arc::clone(&requests);
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = Arc::clone(&captured);
        async move {
            let method = request.method().clone();
            captured.lock().push(method.clone());
            let response = if method == Method::GET {
                json_response(StatusCode::OK, config_map_value(Some("42"), Some("busy"), "old"))
            } else if method == Method::PUT {
                conflict_response("object keeps changing")
            } else {
                panic!("unexpected request method: {method}")
            };
            Ok::<_, Infallible>(response)
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };

    let error = storage
        .update_one_if_version(
            "ConfigMap",
            Some("default"),
            "hot-config",
            serde_yaml::to_string(&config_map_value(None, None, "requested")).expect("fixture must serialize"),
            None,
        )
        .await
        .expect_err("the first backend conflict must be surfaced");

    assert!(matches!(error, ConfWriterError::Conflict(_)));
    let methods = requests.lock();
    assert_eq!(methods.as_slice(), &[Method::GET, Method::PUT]);
}

#[tokio::test]
async fn create_reuses_one_reference_grant_resolution_for_url_and_body_gvk() {
    let requests = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
    let captured = Arc::clone(&requests);
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = Arc::clone(&captured);
        async move {
            let (parts, body) = request.into_parts();
            let bytes = body.collect_bytes().await.expect("request body must be readable");
            let value = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).expect("request body must be JSON")
            };
            captured
                .lock()
                .push((parts.method.clone(), parts.uri.to_string(), value.clone()));

            let response = if parts.uri.path() == "/apis/gateway.networking.k8s.io/v1" {
                not_found_response()
            } else if parts.method == Method::GET && parts.uri.path() == "/apis/gateway.networking.k8s.io/v1beta1" {
                reference_grant_discovery_response()
            } else {
                json_response(StatusCode::CREATED, value)
            };
            Ok::<_, Infallible>(response)
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };

    storage
        .create_one(
            "ReferenceGrant",
            Some("backend"),
            "allow-route",
            serde_yaml::to_string(&canonical_reference_grant_value()).expect("fixture must serialize"),
        )
        .await
        .expect("v1beta1 create must succeed");

    let requests = requests.lock();
    assert!(requests.iter().all(|(_, path, _)| !path.ends_with("/status")));
    assert_eq!(requests.len(), 3, "one v1 probe, one v1beta1 probe, and one create");
    let create = requests
        .iter()
        .find(|(method, _, _)| method == Method::POST)
        .expect("create request must be captured");
    assert_eq!(
        create.1.trim_end_matches('?'),
        "/apis/gateway.networking.k8s.io/v1beta1/namespaces/backend/referencegrants"
    );
    assert_eq!(create.2["apiVersion"], "gateway.networking.k8s.io/v1beta1");
    assert_eq!(create.2["kind"], "ReferenceGrant");
}

#[tokio::test]
async fn update_reuses_one_reference_grant_resolution_for_url_and_body_gvk() {
    let requests = Arc::new(Mutex::new(Vec::<(Method, String, Value)>::new()));
    let captured = Arc::clone(&requests);
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = Arc::clone(&captured);
        async move {
            let (parts, body) = request.into_parts();
            let bytes = body.collect_bytes().await.expect("request body must be readable");
            let value = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).expect("request body must be JSON")
            };
            captured
                .lock()
                .push((parts.method.clone(), parts.uri.to_string(), value.clone()));

            let path = parts.uri.path();
            let response = if path == "/apis/gateway.networking.k8s.io/v1" {
                not_found_response()
            } else if parts.method == Method::GET && path == "/apis/gateway.networking.k8s.io/v1beta1" {
                reference_grant_discovery_response()
            } else if parts.method == Method::GET {
                json_response(
                    StatusCode::OK,
                    json!({
                        "apiVersion": "gateway.networking.k8s.io/v1beta1",
                        "kind": "ReferenceGrant",
                        "metadata": {
                            "name": "allow-route",
                            "namespace": "backend",
                            "uid": "uid-1",
                            "resourceVersion": "42",
                            "finalizers": ["edgion.io/finalizer"]
                        },
                        "spec": canonical_reference_grant_value()["spec"].clone(),
                        "status": {"conditions": [{"type": "Accepted", "status": "True"}]}
                    }),
                )
            } else {
                json_response(StatusCode::OK, value)
            };
            Ok::<_, Infallible>(response)
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };
    let mut requested = canonical_reference_grant_value();
    requested["metadata"]["labels"] = json!({"edited": "true"});

    storage
        .update_one_if_version(
            "ReferenceGrant",
            Some("backend"),
            "allow-route",
            serde_yaml::to_string(&requested).expect("fixture must serialize"),
            Some("42"),
        )
        .await
        .expect("v1beta1 update must succeed");

    let requests = requests.lock();
    assert!(requests.iter().all(|(_, path, _)| !path.ends_with("/status")));
    assert_eq!(
        requests.len(),
        4,
        "one v1 probe, one v1beta1 probe, one get, and one replace"
    );
    let replace = requests
        .iter()
        .find(|(method, _, _)| method == Method::PUT)
        .expect("replace request must be captured");
    assert_eq!(
        replace.1.trim_end_matches('?'),
        "/apis/gateway.networking.k8s.io/v1beta1/namespaces/backend/referencegrants/allow-route"
    );
    assert_eq!(replace.2["apiVersion"], "gateway.networking.k8s.io/v1beta1");
    assert_eq!(replace.2["metadata"]["resourceVersion"], "42");
    assert_eq!(replace.2["metadata"]["finalizers"][0], "edgion.io/finalizer");
    assert_eq!(replace.2["status"]["conditions"][0]["type"], "Accepted");
}

#[tokio::test]
async fn reference_grant_probe_propagates_forbidden_without_fallback() {
    let paths = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&paths);
    let service = service_fn(move |request: http::Request<Body>| {
        let captured = Arc::clone(&captured);
        async move {
            captured.lock().push(request.uri().to_string());
            Ok::<_, Infallible>(json_response(
                StatusCode::FORBIDDEN,
                json!({
                    "apiVersion": "v1",
                    "kind": "Status",
                    "status": "Failure",
                    "message": "referencegrants is forbidden",
                    "reason": "Forbidden",
                    "code": 403
                }),
            ))
        }
    });
    let storage = KubernetesStorage {
        client: Client::new(service, "default"),
        allowed_namespaces: BTreeSet::new(),
        reference_grant_version: Arc::new(OnceCell::new()),
        gateway_class_version: Arc::new(OnceCell::new()),
        gateway_version: Arc::new(OnceCell::new()),
        http_route_version: Arc::new(OnceCell::new()),
        legacy_versions: LegacySelections::default(),
    };

    let err = storage
        .create_one(
            "ReferenceGrant",
            Some("backend"),
            "allow-route",
            serde_yaml::to_string(&canonical_reference_grant_value()).expect("fixture must serialize"),
        )
        .await
        .expect_err("forbidden probe must fail");
    let message = match err {
        ConfWriterError::InternalError(message) => message,
        other => panic!("expected internal probe error, got {other:?}"),
    };
    assert!(message.to_ascii_lowercase().contains("forbidden"));
    let paths = paths.lock();
    assert_eq!(paths.len(), 1, "RBAC failure must not be treated as v1 absence");
    assert_eq!(paths[0].trim_end_matches('?'), "/apis/gateway.networking.k8s.io/v1");
}

impl KubernetesStorage {
    /// Test-only constructor backed by an in-process stub transport.
    ///
    /// Lets a unit test build a `KubernetesCenter` without a cluster and
    /// without a TLS stack. The stub service answers every request with an
    /// empty 200; no test using this issues a request. Must be called from
    /// within a Tokio runtime (`Client::new` spawns a buffer worker).
    #[cfg(test)]
    pub(in crate::conf_mgr::conf_center::kubernetes) fn new_unconnected_for_test() -> Self {
        let service = tower::service_fn(|_req: http::Request<kube::client::Body>| async {
            Ok::<_, std::convert::Infallible>(http::Response::new(kube::client::Body::empty()))
        });
        let storage = Self {
            client: Client::new(service, "default"),
            allowed_namespaces: BTreeSet::new(),
            reference_grant_version: Arc::new(OnceCell::new()),
            gateway_class_version: Arc::new(OnceCell::new()),
            gateway_version: Arc::new(OnceCell::new()),
            http_route_version: Arc::new(OnceCell::new()),
            legacy_versions: LegacySelections::default(),
        };
        for (kind, _) in crate::conf_mgr::conf_center::kubernetes::version_detection::LEGACY_SOURCE_KINDS {
            storage
                .legacy_versions
                .cell(kind)
                .unwrap()
                .set(Some(LegacyRouteVersion::V1))
                .unwrap();
        }
        storage
    }

    /// Route this storage's version probes through the given test client, as
    /// production shares one client between the center and its writer.
    #[cfg(test)]
    pub(in crate::conf_mgr::conf_center::kubernetes) fn with_client_for_test(mut self, client: Client) -> Self {
        self.client = client;
        self
    }
}
