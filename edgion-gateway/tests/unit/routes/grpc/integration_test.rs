use super::*;
use serde_json::json;
use tokio::io::AsyncWriteExt;

async fn selected_fixture(backend: serde_json::Value) -> (Result<Option<()>, EdError>, Session, EdgionHttpContext) {
    let route = crate::test_current_status::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "GRPCRoute",
        "metadata": {"name": "owner", "namespace": "route-ns"},
        "spec": {"resolvedStatusController": "edgion.io/gateway-controller",
            "parentRefs": [{"name": "gateway"}], "rules": [{"backendRefs": [backend]}]}
    }))
    .unwrap();
    let units =
        crate::routes::grpc::conf_handler_impl::parse_route_to_units_for_test("route-ns/owner", &Arc::new(route));
    assert_eq!(units.len(), 1);
    let (mut client, server) = tokio::io::duplex(4096);
    client
        .write_all(b"POST /test/Get HTTP/1.1\r\nHost: test\r\nContent-Length: 0\r\n\r\n")
        .await
        .unwrap();
    let mut session = Session::new_h1(Box::new(server));
    assert!(session.read_request().await.unwrap());
    let mut ctx = EdgionHttpContext::new();
    ctx.grpc_route_unit = Some(Arc::clone(&units[0]));
    let result = handle_grpc_upstream(&mut session, &mut ctx).await;
    (result, session, ctx)
}

#[tokio::test]
async fn invalid_and_denied_slots_never_dispatch_backend_plugins() {
    for (group, denied) in [("example.io", false), ("", true)] {
        let mut backend = json!({"group": group, "name": "api", "port": 80,
        "filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
            "set": [{"name": "x-backend-ran", "value": "yes"}]
        }}]});
        if denied {
            backend["refDenied"] = json!({"targetNamespace": "route-ns", "targetName": "api"});
        }
        let (result, session, ctx) = selected_fixture(backend).await;
        assert!(result.is_err());
        assert!(session.req_header().headers.get("x-backend-ran").is_none());
        assert!(ctx.selected_grpc_backend.is_none());
        assert!(ctx.backend_context.is_none());
    }
    let (result, session, ctx) = selected_fixture(json!({"name": "api", "port": 80,
    "filters": [{"type": "RequestHeaderModifier", "requestHeaderModifier": {
        "set": [{"name": "x-backend-ran", "value": "yes"}]
    }}]}))
    .await;
    assert!(matches!(result, Ok(Some(()))));
    assert_eq!(session.req_header().headers.get("x-backend-ran").unwrap(), "yes");
    assert_eq!(
        ctx.selected_grpc_backend
            .unwrap()
            .logical()
            .policy_target_key()
            .unwrap()
            .name,
        "api"
    );
}
