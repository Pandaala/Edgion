use super::*;

#[test]
fn ordinary_request_keeps_tls_and_grpc_state_absent() {
    let info = RequestInfo::default();

    assert!(!info.has_tls_info());
    assert!(!info.has_grpc_info());
    // Immutable Content-Length and the frozen entry Method retain their audited
    // inline storage; HEAD and CONNECT facts derive from that single snapshot.
    #[cfg(target_pointer_width = "64")]
    assert_eq!(
        std::mem::size_of::<RequestInfo>(),
        432,
        "RequestInfo exceeds its audited layout"
    );
}

#[test]
fn tls_facts_share_one_lazy_allocation_and_keep_flat_json() {
    let cert = ClientCertInfo {
        subject: "CN=client".to_string(),
        sans: vec!["client.example.com".to_string()],
        cn: Some("client".to_string()),
        fingerprint: "sha256".to_string(),
    };
    let tls_id = "tls-1".to_string();
    let sni = "api.example.com".to_string();
    let listener = "https".to_string();
    let gateway = "default/gateway".to_string();
    let (info, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        let mut info = RequestInfo::default();
        info.install_tls_info(Some(tls_id), Some(sni), Some(cert), Some(listener), Some(gateway));
        assert!(info.client_cert_info().is_some());
        assert_eq!(info.gateway_tls_match(), Some(("https", "default/gateway")));
        info
    });

    assert_eq!(allocations.calls, 1);
    let json = serde_json::to_value(&info).unwrap();
    assert!(json.get("sni").is_some());
    assert_eq!(json["tls_id"], "tls-1");
    assert!(json.get("tls").is_none());
}

#[test]
fn grpc_facts_share_one_lazy_allocation_and_keep_flat_json() {
    let service = "catalog.v1.Catalog".to_string();
    let method = "GetItem".to_string();
    let deadline = Instant::now() + Duration::from_secs(3);
    let (info, allocations) = crate::test_allocation_counter::count_allocation_stats(|| {
        let mut info = RequestInfo::default();
        info.set_grpc_method(service, method);
        info.set_grpc_status(8);
        info.set_grpc_client_deadline(deadline);
        assert_eq!(info.grpc_service(), Some("catalog.v1.Catalog"));
        assert_eq!(info.grpc_method(), Some("GetItem"));
        assert_eq!(info.grpc_client_deadline(), Some(deadline));
        info
    });

    assert_eq!(allocations.calls, 1);
    let json = serde_json::to_value(&info).unwrap();
    assert_eq!(json["grpc_service"], "catalog.v1.Catalog");
    assert_eq!(json["grpc_method"], "GetItem");
    assert_eq!(json["grpc_status"], 8);
    assert!(json.get("grpc").is_none());
    assert!(json.get("client_deadline").is_none());
}
