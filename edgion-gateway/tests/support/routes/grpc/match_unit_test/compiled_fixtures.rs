// Test fixtures own the unchanged rule before invoking the production compiler.
pub(crate) fn compile_grpc_fixture(rule: &GRPCRouteRule, namespace: &str) -> GrpcRuleRuntime {
    let mut route: GRPCRoute = serde_json::from_value(serde_json::json!({
        "metadata": {"name": "test-route", "namespace": namespace}, "spec": {}
    }))
    .expect("the fixture has a valid Route envelope");
    route.spec.rules = Some(vec![rule.clone()]);
    GrpcRouteRuleUnit::compile_grpc_rule(&route, 0).expect("valid owning fixture")
}

pub(crate) fn compile_grpc_backend_fixture(
    schema: GRPCBackendRef,
    namespace: &str,
    runtime: Arc<PluginRuntime>,
) -> CompiledGrpcBackend {
    let mut rule: GRPCRouteRule = serde_json::from_value(serde_json::json!({})).expect("empty rule fixture");
    rule.backend_refs = Some(vec![schema]);
    let mut backends = compile_grpc_fixture(&rule, namespace).backends;
    let backend = &mut Arc::get_mut(&mut backends).expect("new fixture is uniquely owned")[0];
    backend.plugin_runtime = runtime;
    backend.clone()
}
