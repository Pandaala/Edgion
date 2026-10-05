use super::*;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};

#[test]
fn ai_runtime_store_collector_reports_populated_stores() {
    let credential_outliers = crate::ai::AiCredentialOutlierStore::new();
    let adaptive_telemetry = crate::ai::AdaptiveTelemetryStore::new();
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::EdgionBackend,
        namespace: "default".to_string(),
        name: "provider".to_string(),
    };
    credential_outliers.configure_backend(target.clone(), None, ["primary".to_string()]);
    let telemetry_key =
        crate::ai::AdaptiveTelemetryKey::new("route-uid".to_string(), "rule".to_string(), target, "model".to_string())
            .unwrap();
    adaptive_telemetry.reconcile_keys(&std::collections::HashSet::from([telemetry_key]));

    let (credential_stats, telemetry_stats) =
        collect_ai_runtime_store_counts(&credential_outliers, &adaptive_telemetry);
    assert_eq!(credential_stats.count, 1);
    assert_eq!(telemetry_stats.count, 1);
}

#[test]
fn store_stats_response_serializes() {
    let resp = StoreStatsResponse {
        etcd_operation_metrics: Default::default(),
        wasm_shared_data_metrics: Default::default(),
        plugin_publication: Default::default(),
        plugin_publication_inventory: Default::default(),
        wasm_oci_cache: crate::plugins::http::wasm::oci::cache::OciMemoryCacheStats {
            entries: 0,
            keys: Vec::new(),
            retained_bytes: 0,
        },
        http_routes: crate::routes::http::HttpRouteManagerStats {
            exact_domains: 0,
            wildcard_domains: 0,
            has_catch_all: false,
            http_routes: 0,
            listener_count: 0,
        },
        grpc_routes: crate::routes::grpc::GrpcRouteManagerStats {
            grpc_routes: 0,
            resource_keys: 0,
            route_units_cache: 0,
            listener_count: 0,
        },
        tcp_routes: crate::routes::tcp::TcpRouteManagerStats {
            route_cache: 0,
            listener_count: 0,
            total_routes_in_tables: 0,
        },
        udp_routes: crate::routes::udp::UdpRouteManagerStats {
            route_cache: 0,
            listener_count: 0,
            total_routes_in_tables: 0,
        },
        tls_routes: crate::routes::tls::TlsRouteManagerStats {
            route_cache: 0,
            listener_count: 0,
            total_candidate_entries_in_tables: 0,
        },
        gateway_config: GatewayConfigStats { gateways: 0 },
        listener_gateway_info: crate::runtime::store::listener_gateway_info::ListenerGatewayInfoStats {
            listener_count: 0,
            total_entries: 0,
        },
        gateway_tls_matcher: crate::runtime::matching::tls::GatewayTlsMatcherStats {
            listener_count: 0,
            is_empty: true,
        },
        tls_store: TlsStoreStats {
            entries: 0,
            valid: 0,
            invalid: 0,
        },
        tls_cert_matcher: TlsCertMatcherStats { listener_count: 0 },
        link_sys_store: LinkSysStoreStats { resources: 0 },
        lb_policies: std::collections::BTreeMap::new(),
        backend_tls_policy: crate::backends::policy::BackendTLSPolicyStoreStats {
            policies: 0,
            reverse_index_targets: 0,
            invalid_index_targets: 0,
        },
        gateway_class_store: SimpleCountStats { count: 0 },
        edgion_gateway_config_store: SimpleCountStats { count: 0 },
        egc_bindings: Default::default(),
        client_caches: ClientCachesStats {
            by_kind: std::collections::BTreeMap::new(),
        },
        service_discovery: crate::backends::discovery::ServiceDiscoveryStatsSnapshot::default(),
        health_status_store: SimpleCountStats { count: 0 },
        outlier_detection_store: SimpleCountStats { count: 0 },
        health_check_config_store: SimpleCountStats { count: 0 },
        gateway_backend_tls_store: SimpleCountStats { count: 0 },
        edgion_config_data_store: SimpleCountStats { count: 0 },
        edgion_backend_store: EdgionBackendStoreStats {
            count: 0,
            keys: Vec::new(),
        },
        ai_credential_outlier_store: SimpleCountStats { count: 0 },
        adaptive_telemetry_store: SimpleCountStats { count: 0 },
        link_sys_runtime: LinkSysRuntimeStats {
            count: 0,
            keys: Vec::new(),
            redis_clients: 0,
            etcd_clients: 0,
            es_clients: 0,
            kafka_clients: 0,
            webhook_clients: 0,
            otlp_clients: 0,
            otlp_retiring_clients: 0,
        },
        plugin_store_v2: PluginStoreStatsV2 {
            total: 0,
            by_kind: std::collections::BTreeMap::new(),
        },
        stream_plugin_store_v2: PluginStoreStatsV2 {
            total: 0,
            by_kind: std::collections::BTreeMap::new(),
        },
    };
    let json = serde_json::to_value(&resp).unwrap();
    assert!(json.is_object());
    assert_eq!(json["etcd_operation_metrics"]["count"], 0);
    assert_eq!(json["etcd_operation_metrics"]["keys"], serde_json::json!([]));
    assert_eq!(json["etcd_operation_metrics"]["keys_truncated"], false);
    assert_eq!(json["wasm_oci_cache"]["entries"], 0);
    assert_eq!(json["wasm_oci_cache"]["retained_bytes"], 0);
    assert_eq!(json["wasm_oci_cache"]["keys"], serde_json::json!([]));
    assert_eq!(json["tls_routes"]["total_candidate_entries_in_tables"], 0);
    assert_eq!(json["udp_routes"]["total_routes_in_tables"], 0);
    assert!(json.get("client_caches").is_some());
    assert!(json.get("service_discovery").is_some());
    assert!(json.get("service_store").is_none());
    assert!(json.get("ep_slice_store").is_none());
    assert!(json.get("edgion_backend_store").is_some());
    assert_eq!(json["ai_credential_outlier_store"]["count"], 0);
    assert_eq!(json["adaptive_telemetry_store"]["count"], 0);
    assert_eq!(json["link_sys_runtime"]["otlp_clients"], 0);
    assert_eq!(json["link_sys_runtime"]["count"], 0);
    assert_eq!(json["link_sys_runtime"]["keys"], serde_json::json!([]));
    assert_eq!(json["link_sys_runtime"]["kafka_clients"], 0);
    assert_eq!(json["link_sys_runtime"]["webhook_clients"], 0);
    assert_eq!(json["link_sys_runtime"]["otlp_retiring_clients"], 0);
    assert!(json.get("plugin_store_v2").is_some());
    assert!(json.get("stream_plugin_store_v2").is_some());
    assert!(json.get("plugin_store").is_none());
    assert!(json.get("stream_plugin_store").is_none());
}
