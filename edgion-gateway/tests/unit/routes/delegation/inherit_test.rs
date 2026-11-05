use super::*;
use crate::ctx::{EdgionHttpContext, NormalizedHostname};
use crate::routes::delegation::DelegatedMatch;
use crate::routes::http::routes_mgr::{DomainRouteRules, GlobalHttpRouteManagers};
use crate::runtime::gateway_info::GatewayInfo;
use crate::runtime::store::listener_gateway_info::{GatewayInfoIndex, ListenerEntry};
use edgion_common::conf_sync::traits::ConfHandler;
use edgion_resources::resources::route_delegation::{
    key_for, DelegationKey, DelegationTarget, DelegationTargetName, RouteKindName,
};
use edgion_resources::{HTTPRoute, ListenerKey, ResourceMeta};
use pingora_proxy::Session;
use serde_json::{json, Value};
use std::collections::HashMap;
use tokio::io::AsyncWriteExt;

const PORT: u16 = 18731;

fn listener() -> ListenerKey {
    ListenerKey::new(format!("0.0.0.0:{PORT}").parse().unwrap(), "HTTP")
}

fn key(parent: &str, child: &str) -> DelegationKey {
    key_for(
        RouteKindName::HTTPRoute,
        "default",
        parent,
        &DelegationTarget {
            ns: "default".into(),
            name: DelegationTargetName::Named(child.into()),
        },
    )
}

fn route(name: &str, root: bool, parent: Option<&str>, rule: Value) -> Arc<HTTPRoute> {
    let mut parent_refs = Vec::new();
    if let Some(parent) = parent {
        parent_refs.push(json!({"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": parent}));
    }
    if root {
        parent_refs.push(
            json!({"group": "gateway.networking.k8s.io", "kind": "Gateway", "namespace": "default", "name": "gw1"}),
        );
    }
    let mut route: HTTPRoute = crate::test_current_status::from_value(json!({
        "apiVersion": "gateway.networking.k8s.io/v1", "kind": "HTTPRoute",
        "metadata": {"namespace": "default", "name": name, "creationTimestamp": "2026-09-20T00:00:00Z"},
        "spec": {
            "resolvedStatusController": "edgion.io/gateway-controller",
            "parentRefs": parent_refs,
            "rules": [rule],
        }
    }))
    .unwrap();
    if root {
        route.spec.resolved_listeners = Some(vec![listener()]);
    }
    route.spec.resolved_delegation_keys = parent.iter().map(|p| key(p, name)).collect();
    route.pre_parse();
    Arc::new(route)
}

fn delegate_to(target: &str) -> Value {
    json!([{"group": "gateway.networking.k8s.io", "kind": "HTTPRoute", "name": target}])
}

fn index() -> GatewayInfoIndex {
    GatewayInfoIndex::from_entries_for_test(vec![(
        "default/gw1".to_string(),
        ListenerEntry {
            port: 8080,
            info: Arc::new(GatewayInfo::new(
                Some("default".into()),
                "gw1".into(),
                Some("web".into()),
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
            )),
            gateway_proof: None,
            hostname: None,
            allowed_routes: None,
        },
    )])
}

/// Match `path` through a real three-level chain and install the result the
/// way `pg_request_filter` does.
async fn matched_ctx(routes: &[&Arc<HTTPRoute>], path: &str) -> EdgionHttpContext {
    let mgr = GlobalHttpRouteManagers::new();
    let data: HashMap<_, _> = routes.iter().map(|r| (r.key_name(), Arc::clone(r))).collect();
    mgr.full_set(&data);
    let table: Arc<DomainRouteRules> = Arc::clone(&mgr.get_or_create_listener_manager(&listener()).load_route_table());
    let (mut client, server) = tokio::io::duplex(4096);
    let mut session = Session::new_h1(Box::new(server));
    client
        .write_all(format!("GET {path} HTTP/1.1\r\nHost: api.example.com\r\n\r\n").as_bytes())
        .await
        .unwrap();
    drop(client);
    assert!(session.read_request().await.unwrap());
    let mut ctx = EdgionHttpContext::new();
    ctx.request_info.hostname = NormalizedHostname::new("api.example.com");
    ctx.request_info.entry_raw_path = path.to_string();
    ctx.request_info.normalized_current_path = path.to_string();
    let result = table
        .match_route(&mut session, &ctx, &index())
        .expect("chain must match");
    ctx.route_unit = Some(result.route_unit);
    ctx.delegated = result.delegated;
    ctx
}

fn three_levels() -> [Arc<HTTPRoute>; 3] {
    let root = route(
        "inh-root",
        true,
        None,
        json!({
            "matches": [{"path": {"type": "PathPrefix", "value": "/api"}}],
            "backendRefs": delegate_to("inh-mid"),
            "timeouts": {"request": "10s", "backendRequest": "3s"},
            "retry": {"attempts": 2, "codes": [503]},
            "sessionPersistence": {"sessionName": "root-sp", "type": "Cookie"},
        }),
    );
    let mid = route(
        "inh-mid",
        false,
        Some("inh-root"),
        json!({
            "matches": [{"path": {"type": "PathPrefix", "value": "/api/v1"}}],
            "backendRefs": delegate_to("inh-leaf"),
            "timeouts": {"request": "5s"},
        }),
    );
    let leaf = route(
        "inh-leaf",
        false,
        Some("inh-mid"),
        json!({
            "matches": [{"path": {"type": "PathPrefix", "value": "/api/v1/x"}}],
            "backendRefs": [{"name": "svc", "port": 80}],
            "timeouts": {"backendRequest": "1s"},
            "retry": {"backoff": "100ms"},
        }),
    );
    [root, mid, leaf]
}

#[tokio::test]
async fn timeouts_leaf_wins_then_nearest_ancestor_per_subfield() {
    let [root, mid, leaf] = three_levels();
    let ctx = matched_ctx(&[&root, &mid, &leaf], "/api/v1/x/y").await;
    let delegated = ctx.delegated.as_deref().expect("delegated chain");
    assert_eq!(delegated.ancestors.len(), 2);
    assert_eq!(
        delegated.inherited,
        InheritedRuleValues {
            request_timeout: Some(RouteTimeout::Duration(Duration::from_secs(5))),
            backend_request_timeout: Some(RouteTimeout::Duration(Duration::from_secs(3))),
            retry_attempts: Some(2),
            retry_backoff: None,
            retry_codes_level: Some(0),
            retry_configured: true,
            session_persistence_level: Some(0),
        },
        "the mid level's request timeout overrides the root's; other sub-fields come from the root"
    );

    // request: leaf unset -> nearest ancestor (mid, 5s).
    assert_eq!(ctx.request_timeout(), Some(Duration::from_secs(5)));
    // backendRequest: leaf (1s) wins over the root's 3s.
    assert_eq!(
        ctx.route_backend_request_timeout(),
        Some(RouteTimeout::Duration(Duration::from_secs(1)))
    );
    // retry: attempts and codes from the root, backoff from the leaf.
    assert_eq!(ctx.retry_attempts(), Some(2));
    assert_eq!(ctx.retry_backoff(), Some(Duration::from_millis(100)));
    assert_eq!(ctx.retry_codes(), Some(&[503u16][..]));
    assert!(ctx.retry_configured());
    // sessionPersistence: the leaf has none, so the root's block applies whole.
    assert_eq!(
        ctx.effective_session_persistence()
            .and_then(|sp| sp.session_name.as_deref()),
        Some("root-sp")
    );
}

#[tokio::test]
async fn ordinary_request_delegated_is_none() {
    let ordinary = route(
        "inh-ordinary",
        true,
        None,
        json!({
            "matches": [{"path": {"type": "PathPrefix", "value": "/plain"}}],
            "backendRefs": [{"name": "svc", "port": 80}],
            "timeouts": {"request": "7s"},
        }),
    );
    let ctx = matched_ctx(&[&ordinary], "/plain/a").await;
    assert!(ctx.delegated.is_none());
    assert!(EdgionHttpContext::new().delegated.is_none());
    assert_eq!(ctx.request_timeout(), Some(Duration::from_secs(7)));
    assert_eq!(ctx.route_backend_request_timeout(), None);
    assert_eq!(ctx.retry_attempts(), None);
    assert_eq!(ctx.retry_codes(), None);
    assert!(!ctx.retry_configured());
    assert!(ctx.effective_session_persistence().is_none());
}

#[tokio::test]
async fn leaf_disabled_timeout_is_set_and_shadows_ancestor() {
    let [root, mid, _] = three_levels();
    let leaf = route(
        "inh-leaf",
        false,
        Some("inh-mid"),
        json!({
            "matches": [{"path": {"type": "PathPrefix", "value": "/api/v1/x"}}],
            "backendRefs": [{"name": "svc", "port": 80}],
            "timeouts": {"request": "0s"},
        }),
    );
    let ctx = matched_ctx(&[&root, &mid, &leaf], "/api/v1/x").await;
    assert!(ctx.delegated.is_some());
    assert_eq!(ctx.request_timeout(), None, "an explicit 0s at the leaf disables it");
}

#[test]
fn fold_of_no_ancestors_is_empty() {
    assert_eq!(InheritedRuleValues::fold(&[]), InheritedRuleValues::default());
    let _ = DelegatedMatch::new(Default::default());
}
